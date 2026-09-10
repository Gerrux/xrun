#![deny(unsafe_code)]

//! Inbound control over Telegram: reply to a notification with a command
//! and the next `xrun watchdog` pass executes it.
//!
//! ```text
//! /status            running runs with cost + heartbeat age
//! /stop <id>         graceful stop + destroy (id may be the 8-char tail)
//! /pull <id>         pull the best checkpoint into the run's artifacts/
//! /help
//! ```
//!
//! Security model: the bot token + chat id live in `credentials.toml`;
//! messages from any other chat are ignored (and counted, so a hijacked
//! token is visible in the watchdog log). The last processed `update_id`
//! is persisted next to the DB so each command runs once, even though the
//! TUI's 60 s tick and the scheduler both call the watchdog.

use std::path::Path;

use anyhow::Result;
use xrun_core::{store::RunStatus, Credentials, Run, Store};
use xrun_notify::{
    channels::{Channel, TelegramChannel, TelegramUpdate},
    messages, Priority,
};

/// Parsed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Status,
    Stop(String),
    Pull(String),
    Help,
    Unknown(String),
}

pub fn parse(text: &str) -> Option<Command> {
    let t = text.trim();
    if !t.starts_with('/') {
        return None;
    }
    let mut parts = t.split_whitespace();
    // Strip an optional `@botname` suffix (group chats append it).
    let cmd = parts
        .next()?
        .split('@')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let arg = parts.next().map(|s| s.trim().to_string());
    Some(match (cmd.as_str(), arg) {
        ("/status", _) | ("/st", _) => Command::Status,
        ("/stop", Some(id)) => Command::Stop(id),
        ("/pull", Some(id)) => Command::Pull(id),
        ("/help", _) | ("/start", _) => Command::Help,
        ("/stop", None) => Command::Unknown("/stop needs a run id (see /status)".into()),
        ("/pull", None) => Command::Unknown("/pull needs a run id (see /status)".into()),
        (other, _) => Command::Unknown(format!("unknown command {other}; try /help")),
    })
}

/// Resolve a full id or an unambiguous tail against running runs.
pub fn resolve_run<'a>(runs: &'a [Run], id: &str) -> Result<&'a Run, String> {
    let id = id.trim();
    let hits: Vec<&Run> = runs
        .iter()
        .filter(|r| {
            let full = r.id.to_string();
            full == id || (id.len() >= 4 && full.ends_with(id))
        })
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(format!("no running run matches `{id}` (see /status)")),
        n => Err(format!(
            "`{id}` is ambiguous ({n} runs); use more characters"
        )),
    }
}

fn offset_path(db_path: &Path) -> std::path::PathBuf {
    db_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("telegram.offset")
}

fn read_offset(db_path: &Path) -> Option<i64> {
    std::fs::read_to_string(offset_path(db_path))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn write_offset(db_path: &Path, next: i64) {
    let _ = std::fs::write(offset_path(db_path), next.to_string());
}

fn short(id: &str) -> &str {
    let n = id.len();
    if n > 8 {
        &id[n - 8..]
    } else {
        id
    }
}

pub fn status_text(store: &Store, runs: &[Run]) -> String {
    if runs.is_empty() {
        return "no running runs".into();
    }
    let now = chrono::Utc::now();
    let mut lines = Vec::new();
    for r in runs {
        let cost = r
            .instance_id
            .as_deref()
            .and_then(|i| store.get_instance(i).ok().flatten())
            .map(|i| i.accumulated_cost)
            .filter(|c| *c > 0.0)
            .or(r.cost_usd);
        let hb = r
            .poller_heartbeat_at
            .map(|t| format!("hb {} ago", messages::fmt_duration((now - t).num_seconds())))
            .unwrap_or_else(|| "no heartbeat".into());
        let age = r
            .started_at
            .map(|s| messages::fmt_duration((now - s).num_seconds()))
            .unwrap_or_else(|| "-".into());
        lines.push(format!(
            "• {} [{}] {} — {} running, {}, {}",
            r.name,
            r.vendor,
            short(&r.id.to_string()),
            age,
            messages::fmt_cost(cost),
            hb
        ));
    }
    lines.push(String::new());
    lines.push("/stop <id>  /pull <id>  (id = last 8 chars)".into());
    lines.join("\n")
}

const HELP: &str = "xrun bot commands:\n\
/status — running runs, cost, heartbeat\n\
/stop <id> — stop the run and destroy its instance\n\
/pull <id> — pull the best checkpoint\n\
Commands are picked up by `xrun watchdog` (every 5 min from the scheduler, \
every 60 s while the TUI is open).";

/// Poll the bot, execute commands from the configured chat, reply. Returns
/// one log line per handled update. `Ok(empty)` when Telegram is not
/// configured.
pub fn process(config_dir: &Path, db_path: &Path, runs_dir: &Path) -> Result<Vec<String>> {
    let creds = Credentials::load(config_dir).unwrap_or_default();
    let Ok(ch) = TelegramChannel::from_creds(&creds) else {
        return Ok(Vec::new());
    };
    let offset = read_offset(db_path);
    let updates = ch
        .poll_updates(offset)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if updates.is_empty() {
        return Ok(Vec::new());
    }
    // Advance the cursor *before* executing so a crash mid-command (or a
    // concurrent watchdog) never replays a /stop.
    if let Some(max) = updates.iter().map(|u| u.update_id).max() {
        write_offset(db_path, max + 1);
    }

    let mut log = Vec::new();
    let mut foreign = 0usize;
    for u in updates {
        if u.chat_id != ch.chat_id() {
            foreign += 1;
            continue;
        }
        log.push(handle_one(&ch, &u, db_path, runs_dir, config_dir));
    }
    if foreign > 0 {
        log.push(format!(
            "ignored {foreign} message(s) from other chats (bot token exposed?)"
        ));
    }
    Ok(log)
}

fn reply(ch: &TelegramChannel, text: &str) {
    let n = messages::manual("xrun", text, None, Priority::Default);
    // Telegram renders title + body; keep the title minimal.
    if let Err(e) = ch.send(&n) {
        tracing::warn!("telegram reply failed: {e}");
    }
}

fn handle_one(
    ch: &TelegramChannel,
    u: &TelegramUpdate,
    db_path: &Path,
    runs_dir: &Path,
    config_dir: &Path,
) -> String {
    let Some(cmd) = parse(&u.text) else {
        return format!("ignored non-command `{}`", truncate(&u.text, 40));
    };
    let store = match Store::open(db_path) {
        Ok(s) => s,
        Err(e) => {
            reply(ch, &format!("xrun: cannot open DB: {e}"));
            return format!("{:?}: db error {e}", cmd);
        }
    };
    let running: Vec<Run> = store
        .list_active_runs()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.status == RunStatus::Running)
        .collect();

    match cmd {
        Command::Help => {
            reply(ch, HELP);
            "/help".into()
        }
        Command::Status => {
            reply(ch, &status_text(&store, &running));
            format!("/status ({} running)", running.len())
        }
        Command::Unknown(msg) => {
            reply(ch, &msg);
            format!("rejected `{}`: {msg}", truncate(&u.text, 40))
        }
        Command::Stop(id) => {
            let run = match resolve_run(&running, &id) {
                Ok(r) => r.clone(),
                Err(e) => {
                    reply(ch, &e);
                    return format!("/stop {id}: {e}");
                }
            };
            drop(store);
            let args = crate::cli::StopArgs {
                id: Some(run.id.to_string()),
                all: false,
                force: false,
                keep_instance: false,
            };
            match crate::commands::stop::run(&args, db_path, runs_dir, config_dir) {
                Ok(()) => {
                    reply(
                        ch,
                        &format!(
                            "⏹ stopped {} [{}] and destroyed its instance.",
                            run.name,
                            short(&run.id.to_string())
                        ),
                    );
                    format!("/stop {} ok", short(&run.id.to_string()))
                }
                Err(e) => {
                    reply(ch, &format!("stop failed for {}: {e}", run.name));
                    format!("/stop {}: {e}", short(&run.id.to_string()))
                }
            }
        }
        Command::Pull(id) => {
            let run = match resolve_run(&running, &id) {
                Ok(r) => r.clone(),
                Err(e) => {
                    reply(ch, &e);
                    return format!("/pull {id}: {e}");
                }
            };
            drop(store);
            let args = crate::cli::PullArgs {
                id: Some(run.id.to_string()),
                ckpt: "best".into(),
                artifacts: false,
                into: None,
            };
            match crate::commands::pull::run(&args, db_path, runs_dir, config_dir) {
                Ok(()) => {
                    let into = runs_dir.join(run.id.to_string()).join("artifacts");
                    reply(
                        ch,
                        &format!(
                            "⬇ pulled best checkpoint of {} to {}",
                            run.name,
                            into.display()
                        ),
                    );
                    format!("/pull {} ok", short(&run.id.to_string()))
                }
                Err(e) => {
                    reply(ch, &format!("pull failed for {}: {e}", run.name));
                    format!("/pull {}: {e}", short(&run.id.to_string()))
                }
            }
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xrun_core::store::RunId;

    #[test]
    fn parse_commands() {
        assert_eq!(parse("/status"), Some(Command::Status));
        assert_eq!(parse("/status@xrun_bot"), Some(Command::Status));
        assert_eq!(
            parse("/stop abcd1234"),
            Some(Command::Stop("abcd1234".into()))
        );
        assert_eq!(parse("/PULL x"), Some(Command::Pull("x".into())));
        assert_eq!(parse("/help"), Some(Command::Help));
        assert_eq!(parse("hello"), None);
        assert!(matches!(parse("/stop"), Some(Command::Unknown(_))));
        assert!(matches!(parse("/nuke"), Some(Command::Unknown(_))));
    }

    fn run_named(name: &str) -> Run {
        Run {
            id: RunId::new(),
            name: name.into(),
            manifest_hash: "h".into(),
            manifest_path: "m".into(),
            vendor: "vast".into(),
            instance_id: None,
            status: RunStatus::Running,
            created_at: chrono::Utc::now(),
            started_at: None,
            ended_at: None,
            cost_usd: None,
            mlflow_run_id: None,
            notes: None,
            poller_pid: None,
            mlflow_run_url: None,
            wandb_run_id: None,
            wandb_run_url: None,
            poller_heartbeat_at: None,
        }
    }

    #[test]
    fn resolve_by_tail_and_full_id() {
        let runs = vec![run_named("a"), run_named("b")];
        let full = runs[0].id.to_string();
        assert_eq!(resolve_run(&runs, &full).unwrap().name, "a");
        assert_eq!(
            resolve_run(&runs, &full[full.len() - 8..]).unwrap().name,
            "a"
        );
        assert!(resolve_run(&runs, "zz").is_err());
        assert!(resolve_run(&runs, "nomatch99").is_err());
    }
}
