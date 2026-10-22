#![deny(unsafe_code)]

use std::path::{Path, PathBuf};

use anyhow::Result;
use xrun_core::config::credentials::Credentials;
use xrun_core::config::GlobalConfig;
use xrun_core::manifest::types::{DataMode, Vendor};
use xrun_core::{manifest::Manifest, Store};
use xrun_kaggle::KaggleAdapter;

use crate::cli::DoctorArgs;
use crate::commands::common::resolve_kaggle_credentials;

struct Check {
    name: &'static str,
    category: &'static str,
    ok: bool,
    /// If true the check is advisory only — shown as WARN, does not cause exit 1.
    warn_only: bool,
    detail: String,
}

/// Manifest discovered on disk (either via --manifest or auto-scanned from exp/).
struct DiscoveredManifest {
    path: PathBuf,
    /// True for paths passed via --manifest (parse failures are fatal); false
    /// for auto-discovered paths (parse failures are advisory).
    explicit: bool,
    /// `Some` if the file parsed; `None` if parse failed (we still record the
    /// failure as a manifest check, but cannot derive vendor info from it).
    parsed: Option<Manifest>,
    /// Read error or parse error message, if any.
    error: Option<String>,
}

pub fn run(args: &DoctorArgs, config_dir: &Path, db_path: Option<&Path>) -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();

    // Load config + credentials once. Treat read errors as "nothing configured"
    // — the core config_dir check below will already flag a real I/O problem.
    let config = GlobalConfig::load(config_dir).unwrap_or_default();
    let creds = Credentials::load(config_dir).unwrap_or_default();

    // Discover manifests: explicit --manifest args plus auto-scanned exp dir.
    let manifests = discover_manifests(args, &config);

    // Build the set of vendors that actually matter in this environment.
    let active_vendors = active_vendors(&creds, &manifests, args.all);

    // ---- core (always) ----
    let config_ok = dir_writable(config_dir);
    checks.push(Check {
        name: "config_dir",
        category: "core",
        ok: config_ok,
        warn_only: false,
        detail: config_dir.display().to_string(),
    });

    let db_ok = match db_path {
        Some(p) => Store::open(p).is_ok(),
        None => false,
    };
    checks.push(Check {
        name: "db_access",
        category: "core",
        ok: db_ok,
        warn_only: false,
        detail: db_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "path unavailable".to_string()),
    });

    // ---- vendors (conditional) ----
    if active_vendors.contains(&Vendor::Vast) {
        let vastai_ok = binary_available("vastai");
        checks.push(Check {
            name: "vastai_binary",
            category: "vendor:vast",
            ok: vastai_ok,
            warn_only: false,
            detail: if vastai_ok {
                "found in PATH".to_string()
            } else {
                "not found in PATH (install: pipx install vastai)".to_string()
            },
        });
        if vastai_ok {
            let (ssh_ok, ssh_detail) = check_vastai_ssh_key();
            checks.push(Check {
                name: "vastai_ssh_key",
                category: "vendor:vast",
                ok: ssh_ok,
                warn_only: true,
                detail: ssh_detail,
            });
        }
        checks.push(Check {
            name: "vast_credentials",
            category: "vendor:vast",
            ok: creds.vast.api_key.is_some(),
            warn_only: false,
            detail: if creds.vast.api_key.is_some() {
                "api_key set".to_string()
            } else {
                "no api_key — run `xrun config set vast.api_key <KEY>`".to_string()
            },
        });
    }

    if active_vendors.contains(&Vendor::Kaggle) {
        let kaggle_ok = binary_available("kaggle");
        checks.push(Check {
            name: "kaggle_binary",
            category: "vendor:kaggle",
            ok: kaggle_ok,
            warn_only: false,
            detail: if kaggle_ok {
                "found in PATH".to_string()
            } else {
                "not found in PATH (install: pip install kaggle)".to_string()
            },
        });
    }

    if active_vendors.contains(&Vendor::Ssh) {
        let ssh_ok = binary_available("ssh");
        checks.push(Check {
            name: "ssh_binary",
            category: "vendor:ssh",
            ok: ssh_ok,
            warn_only: false,
            detail: if ssh_ok {
                "found in PATH".to_string()
            } else {
                "not found in PATH".to_string()
            },
        });
        let alias_count = creds.ssh_hosts.len();
        checks.push(Check {
            name: "ssh_hosts",
            category: "vendor:ssh",
            ok: alias_count > 0,
            warn_only: false,
            detail: if alias_count > 0 {
                let mut names: Vec<&str> = creds.ssh_hosts.keys().map(String::as_str).collect();
                names.sort();
                format!("{alias_count} alias(es): {}", names.join(", "))
            } else {
                "no [vendors.ssh.<alias>] sections in credentials.toml".to_string()
            },
        });
    }

    // The bridge ping spawns a Python child, so these rows only exist for an
    // active vendor (configured, named by a manifest, or `--all`).
    if active_vendors.contains(&Vendor::Lightning) {
        use xrun_lightning::{LightningBridge, PyLightningBridge};
        let (ping_ok, ping_detail) = match PyLightningBridge::new(&creds.lightning).ping() {
            Ok(info) => (true, format!("lightning-sdk {}", info.sdk_version)),
            Err(e) => (
                false,
                format!(
                    "{e} (install: pip install lightning-sdk; XRUN_PYTHON picks the interpreter) — run `xrun install sdk lightning`"
                ),
            ),
        };
        checks.push(Check {
            name: "lightning_sdk",
            category: "vendor:lightning",
            ok: ping_ok,
            warn_only: false,
            detail: ping_detail,
        });
        let non_empty = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
        let (cred_ok, cred_detail) =
            if non_empty(&creds.lightning.api_key) && non_empty(&creds.lightning.user_id) {
                (true, "api_key+user_id set".to_string())
            } else if Credentials::lightning_configured(&creds) {
                (true, "~/.lightning/credentials.json found".to_string())
            } else {
                (
                false,
                "not configured — `xrun config set lightning.api_key …` and `lightning.user_id`"
                    .to_string(),
            )
            };
        checks.push(Check {
            name: "lightning_credentials",
            category: "vendor:lightning",
            ok: cred_ok,
            warn_only: false,
            detail: cred_detail,
        });
    }

    if active_vendors.contains(&Vendor::Colab) {
        use xrun_colab::{ColabBridge, PyColabBridge};
        let (ping_ok, ping_detail) = match PyColabBridge::new().ping() {
            Ok(info) => (true, format!("google-colab-cli {}", info.sdk_version)),
            Err(e) => (
                false,
                format!("{e} (install: pip install google-colab-cli; XRUN_PYTHON picks the interpreter) — run `xrun install sdk colab`"),
            ),
        };
        checks.push(Check {
            name: "colab_sdk",
            category: "vendor:colab",
            ok: ping_ok,
            warn_only: false,
            detail: ping_detail,
        });
        let logged_in = Credentials::colab_configured();
        checks.push(Check {
            name: "colab_login",
            category: "vendor:colab",
            ok: logged_in,
            warn_only: false,
            detail: if logged_in {
                "OAuth token file found".to_string()
            } else {
                "not logged in — run `xrun config login colab` in a terminal".to_string()
            },
        });
    }

    if active_vendors.contains(&Vendor::Local) {
        // Local has no external prerequisites beyond what core already checks;
        // surface a single OK row so users see local is wired up.
        checks.push(Check {
            name: "local_runtime",
            category: "vendor:local",
            ok: true,
            warn_only: false,
            detail: "host subprocess runner available".to_string(),
        });
    }

    // ---- logging / metrics ----
    let mlflow_active = config.mlflow.url.is_some()
        || config.metrics.sinks.iter().any(|s| s == "mlflow")
        || creds.mlflow.token.is_some()
        || creds.mlflow.username.is_some()
        || args.all;
    if mlflow_active {
        let url_ok = config.mlflow.url.is_some();
        checks.push(Check {
            name: "mlflow_url",
            category: "logging",
            ok: url_ok,
            warn_only: !args.all,
            detail: match &config.mlflow.url {
                Some(u) => format!("tracking URL: {u}"),
                None => "no mlflow.url set in config.toml".to_string(),
            },
        });
        let creds_present = creds.mlflow.token.is_some()
            || (creds.mlflow.username.is_some() && creds.mlflow.password.is_some());
        checks.push(Check {
            name: "mlflow_credentials",
            category: "logging",
            ok: creds_present,
            warn_only: true,
            detail: if creds_present {
                "auth configured".to_string()
            } else {
                "no auth — public/anonymous tracking only".to_string()
            },
        });
    }

    // ---- data ----
    let needs_rsync = manifests.iter().filter_map(|m| m.parsed.as_ref()).any(|m| {
        m.data
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .any(|d| matches!(d.mode, Some(DataMode::Rsync)))
    });
    if needs_rsync || args.all {
        let rsync_ok = binary_available("rsync");
        checks.push(Check {
            name: "rsync_binary",
            category: "data",
            ok: rsync_ok,
            warn_only: !needs_rsync, // fatal only when a manifest actually wants rsync
            detail: if rsync_ok {
                "found in PATH".to_string()
            } else if needs_rsync {
                "not found in PATH — at least one manifest uses data.mode: rsync".to_string()
            } else {
                "not found in PATH (only needed for data.mode: rsync)".to_string()
            },
        });
    }

    // ---- runtime hook ----
    // Only relevant when training runs in-process on this host (vendor: local).
    // For vast/ssh/kaggle the hook is bootstrapped on the remote instance, so
    // a local import-check is noise. `--all` still surfaces it on demand.
    let hook_relevant = active_vendors.iter().any(|v| matches!(v, Vendor::Local)) || args.all;
    if hook_relevant {
        let hook_ok = check_python_hook();
        checks.push(Check {
            name: "python_xrun_hook",
            category: "runtime",
            ok: hook_ok,
            warn_only: true,
            detail: if hook_ok {
                "importable".to_string()
            } else {
                "not importable (installed on instance automatically; optional locally)".to_string()
            },
        });
    }

    // ---- agent integrations ----
    let (skill_ok, skill_detail) = check_project_skill(".codex/skills/xrun/SKILL.md");
    checks.push(Check {
        name: "codex_skill",
        category: "agents",
        ok: skill_ok,
        warn_only: true,
        detail: skill_detail,
    });
    let (agents_ok, agents_detail) = check_project_instruction_file("AGENTS.md");
    checks.push(Check {
        name: "agents_md",
        category: "agents",
        ok: agents_ok,
        warn_only: true,
        detail: agents_detail,
    });
    let (skill_ok, skill_detail) = check_project_skill(".claude/skills/xrun/SKILL.md");
    checks.push(Check {
        name: "claude_skill",
        category: "agents",
        ok: skill_ok,
        warn_only: true,
        detail: skill_detail,
    });
    let (claude_md_ok, claude_md_detail) = check_project_instruction_file("CLAUDE.md");
    checks.push(Check {
        name: "claude_md",
        category: "agents",
        ok: claude_md_ok,
        warn_only: true,
        detail: claude_md_detail,
    });

    // ---- manifests ----
    // One Kaggle session for the whole run: authentication and per-slug
    // dataset readiness are each queried once, however many manifests share them.
    let mut kaggle_session: Option<KaggleSession> = None;
    for m in &manifests {
        if let Some(err) = &m.error {
            checks.push(Check {
                name: "manifest",
                category: "manifests",
                ok: false,
                // Auto-discovered (exp/) parse failures are advisory; explicit
                // --manifest failures are fatal — matches old behaviour.
                warn_only: !m.explicit,
                detail: format!("{}: {err}", m.path.display()),
            });
            continue;
        }
        let manifest = m.parsed.as_ref().unwrap();
        checks.push(Check {
            name: "manifest",
            category: "manifests",
            ok: true,
            warn_only: false,
            detail: format!("{} (vendor={:?})", m.path.display(), manifest.vendor),
        });
        if matches!(manifest.vendor, Vendor::Kaggle)
            && (creds.kaggle.token.is_some() || creds.kaggle.username.is_some() || args.all)
        {
            let session = kaggle_session.get_or_insert_with(|| {
                KaggleSession::new(
                    KaggleAdapter::new().with_credentials(resolve_kaggle_credentials(config_dir)),
                )
            });
            kaggle_manifest_checks(manifest, session, &mut checks);
        }
        requires_checks(manifest, &mut checks);
    }

    let any_fail = checks.iter().any(|c| !c.ok && !c.warn_only);

    if args.json {
        let out: Vec<_> = checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "check": c.name,
                    "category": c.category,
                    "status": check_status_str(c),
                    "detail": c.detail,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    } else {
        println!("{:<24}  {:<6}  detail", "check", "status");
        println!("{}", "-".repeat(70));
        let mut current_cat = "";
        for c in &checks {
            if c.category != current_cat {
                println!();
                println!("[{}]", c.category);
                current_cat = c.category;
            }
            println!("{:<24}  {:<6}  {}", c.name, check_status_str(c), c.detail);
        }
    }

    if any_fail {
        std::process::exit(1);
    }

    Ok(())
}

/// Identity of a manifest path for de-duplication: `exp/a.yaml` and
/// `exp\a.yaml` (or `./exp/a.yaml`) name the same file, so compare the
/// canonical form and fall back to the path as written when the file is
/// unreadable.
fn path_key(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Manifests to check. With `--manifest` that is exactly the named files;
/// without it, every `*.yaml` / `*.yml` in the exp dir (config
/// `defaults.exp_dir`, default `./exp`, non-recursive).
fn discover_manifests(args: &DoctorArgs, config: &GlobalConfig) -> Vec<DiscoveredManifest> {
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    // (path, explicit)
    let mut paths: Vec<(PathBuf, bool)> = Vec::new();
    for p in &args.manifests {
        if seen.insert(path_key(p)) {
            paths.push((p.clone(), true));
        }
    }

    if args.manifests.is_empty() {
        let exp_dir = config
            .defaults
            .exp_dir
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("exp"));
        if exp_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&exp_dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_file() {
                        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
                        if (ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
                            && seen.insert(path_key(&p))
                        {
                            paths.push((p, false));
                        }
                    }
                }
            }
        }
    }

    paths.sort();
    paths
        .into_iter()
        .map(|(path, is_explicit)| match std::fs::read_to_string(&path) {
            Ok(yaml) => match Manifest::from_yaml_str(&yaml) {
                Ok(m) => DiscoveredManifest {
                    path,
                    explicit: is_explicit,
                    parsed: Some(m),
                    error: None,
                },
                Err(e) => DiscoveredManifest {
                    path,
                    explicit: is_explicit,
                    parsed: None,
                    error: Some(e.to_string()),
                },
            },
            Err(e) => DiscoveredManifest {
                path,
                explicit: is_explicit,
                parsed: None,
                error: Some(format!("read failed: {e}")),
            },
        })
        .collect()
}

fn active_vendors(creds: &Credentials, manifests: &[DiscoveredManifest], all: bool) -> Vec<Vendor> {
    let mut set: Vec<Vendor> = Vec::new();
    let push = |set: &mut Vec<Vendor>, v: Vendor| {
        if !set.iter().any(|x| x == &v) {
            set.push(v);
        }
    };
    if all {
        push(&mut set, Vendor::Vast);
        push(&mut set, Vendor::Kaggle);
        push(&mut set, Vendor::Local);
        push(&mut set, Vendor::Ssh);
        push(&mut set, Vendor::Lightning);
        push(&mut set, Vendor::Colab);
        return set;
    }
    if creds.vast.api_key.is_some() {
        push(&mut set, Vendor::Vast);
    }
    if creds.kaggle.token.is_some()
        || (creds.kaggle.username.is_some() && creds.kaggle.key.is_some())
    {
        push(&mut set, Vendor::Kaggle);
    }
    if !creds.ssh_hosts.is_empty() {
        push(&mut set, Vendor::Ssh);
    }
    if creds.lightning_configured() {
        push(&mut set, Vendor::Lightning);
    }
    if Credentials::colab_configured() {
        push(&mut set, Vendor::Colab);
    }
    for m in manifests {
        if let Some(p) = &m.parsed {
            push(&mut set, p.vendor);
        }
    }
    // Fallback: nothing configured anywhere → at least show local so the
    // output isn't completely empty for a fresh install.
    if set.is_empty() {
        push(&mut set, Vendor::Local);
    }
    set
}

fn check_status_str(c: &Check) -> &'static str {
    if c.ok {
        "OK"
    } else if c.warn_only {
        "WARN"
    } else {
        "FAIL"
    }
}

/// Run `vastai show user --raw` and check for a non-empty ssh_key field.
fn check_vastai_ssh_key() -> (bool, String) {
    let mut vastai_cmd = std::process::Command::new("vastai");
    vastai_cmd.args(["show", "user", "--raw"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        vastai_cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let result = vastai_cmd.output();

    match result {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            match serde_json::from_str::<serde_json::Value>(text.trim()) {
                Ok(v) => {
                    let has_key = v
                        .get("ssh_key")
                        .and_then(|k| k.as_str())
                        .map(|s| !s.is_empty())
                        .unwrap_or(false);
                    if has_key {
                        (true, "SSH key registered on account".to_string())
                    } else {
                        (
                            false,
                            "no SSH key — add one at https://cloud.vast.ai/account/".to_string(),
                        )
                    }
                }
                Err(_) => (false, "could not parse vastai show user output".to_string()),
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let detail = if stderr.contains("unauthorized") || stderr.contains("api_key") {
                "not authenticated — run `vastai set api-key <KEY>`".to_string()
            } else {
                let first_line = stderr.lines().next().unwrap_or("unknown error");
                let truncated: String = first_line.chars().take(120).collect();
                format!("vastai show user failed: {}", truncated)
            };
            (false, detail)
        }
        Err(e) => (false, format!("could not run vastai: {e}")),
    }
}

/// Check if `python3 -c "import xrun_hook"` succeeds.
fn check_python_hook() -> bool {
    let mut cmd = std::process::Command::new("python3");
    cmd.args(["-c", "import xrun_hook"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

fn check_project_skill(relative: &str) -> (bool, String) {
    let cwd = std::env::current_dir().ok();
    let Some(skill_path) = cwd.as_deref().map(|d| d.join(relative)) else {
        return (false, "could not resolve cwd".to_string());
    };
    if skill_path.is_file() {
        (true, format!("installed at {}", skill_path.display()))
    } else {
        (
            false,
            format!(
                "not installed at {} (run `xrun install skill --codex` or `--claude`)",
                skill_path.display()
            ),
        )
    }
}

fn check_project_instruction_file(name: &str) -> (bool, String) {
    let cwd = std::env::current_dir().ok();
    let candidate = cwd.as_deref().map(|d| d.join(name));
    match candidate {
        Some(p) if p.is_file() => (true, format!("found at {}", p.display())),
        Some(p) => (false, format!("not found at {}", p.display())),
        None => (false, "could not resolve cwd".to_string()),
    }
}

fn dir_writable(path: &Path) -> bool {
    if std::fs::create_dir_all(path).is_err() {
        return false;
    }
    let probe = path.join(".xrun_doctor_probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Shared state for the Kaggle checks of one doctor run. Each query goes
/// through a subprocess with no timeout, so a project with many Kaggle
/// manifests must not repeat them: authentication happens at most once and
/// each dataset slug is asked about at most once, whatever the manifest count.
/// Errors are cached as their rendered text (the rows only ever show text).
struct KaggleSession {
    adapter: KaggleAdapter,
    auth: Option<Result<String, String>>,
    dataset_ready: std::collections::HashMap<String, Result<bool, String>>,
}

impl KaggleSession {
    fn new(adapter: KaggleAdapter) -> Self {
        Self {
            adapter,
            auth: None,
            dataset_ready: std::collections::HashMap::new(),
        }
    }

    fn authenticate(&mut self) -> Result<String, String> {
        let adapter = &self.adapter;
        self.auth
            .get_or_insert_with(|| adapter.cli().authenticate().map_err(|e| e.to_string()))
            .clone()
    }

    fn is_dataset_ready(&mut self, slug: &str) -> Result<bool, String> {
        if let Some(cached) = self.dataset_ready.get(slug) {
            return cached.clone();
        }
        let res = self
            .adapter
            .cli()
            .is_dataset_ready(slug)
            .map_err(|e| e.to_string());
        self.dataset_ready.insert(slug.to_string(), res.clone());
        res
    }
}

/// Kaggle-specific checks for manifests with `vendor: kaggle`.
fn kaggle_manifest_checks(
    manifest: &Manifest,
    session: &mut KaggleSession,
    checks: &mut Vec<Check>,
) {
    let kaggle_spec = match &manifest.kaggle {
        Some(s) => s,
        None => {
            checks.push(Check {
                name: "kaggle_spec",
                category: "manifests",
                ok: false,
                warn_only: false,
                detail: "vendor is kaggle but no [kaggle] section found in manifest".to_string(),
            });
            return;
        }
    };

    // Check kernel_slug consistency with manifest.name.
    let expected_slug_suffix = manifest
        .name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let actual_suffix = kaggle_spec
        .kernel_slug
        .rsplit('/')
        .next()
        .unwrap_or(&kaggle_spec.kernel_slug);
    // Slugs with `{run_id}` / `{date}` placeholders intentionally diverge from
    // the manifest name at write time and only match after expansion. Don't
    // flag them — that's exactly the new auto-bump path.
    let has_placeholder = actual_suffix.contains('{');
    let slug_ok = has_placeholder || actual_suffix == expected_slug_suffix;
    checks.push(Check {
        name: "kaggle_kernel_slug",
        category: "manifests",
        ok: slug_ok,
        warn_only: true,
        detail: if slug_ok {
            if has_placeholder {
                format!(
                    "kernel_slug '{}' uses placeholders ({{run_id}}/{{date}}) — auto-bumped per launch",
                    kaggle_spec.kernel_slug
                )
            } else {
                format!(
                    "kernel_slug '{}' matches manifest name",
                    kaggle_spec.kernel_slug
                )
            }
        } else {
            format!(
                "kernel_slug suffix '{}' differs from slugified manifest name '{}' — \
                 Kaggle may create the kernel at the wrong slug",
                actual_suffix, expected_slug_suffix
            )
        },
    });

    let creds_ok = match session.authenticate() {
        Ok(u) => {
            checks.push(Check {
                name: "kaggle_credentials",
                category: "manifests",
                ok: true,
                warn_only: false,
                detail: format!("authenticated as {u}"),
            });
            true
        }
        Err(e) => {
            checks.push(Check {
                name: "kaggle_credentials",
                category: "manifests",
                ok: false,
                warn_only: false,
                detail: format!("could not authenticate: {e}"),
            });
            false
        }
    };

    if creds_ok {
        let mut all_slugs: Vec<&str> = Vec::new();
        if let Some(s) = &kaggle_spec.dataset {
            all_slugs.push(s.as_str());
        }
        for s in &kaggle_spec.datasets {
            all_slugs.push(s.as_str());
        }
        for slug in all_slugs {
            let (ok, warn_only, detail) = match session.is_dataset_ready(slug) {
                Ok(true) => (true, false, format!("dataset '{slug}' is ready")),
                Ok(false) => (
                    false,
                    true,
                    format!(
                        "dataset '{slug}' exists but is not ready yet — \
                         training will fail until it finishes processing"
                    ),
                ),
                Err(e) => (false, false, format!("dataset '{slug}' check failed: {e}")),
            };
            checks.push(Check {
                name: "kaggle_dataset",
                category: "manifests",
                ok,
                warn_only,
                detail,
            });
        }
    }
}

/// Known hardware caps per vendor. `(ram_gb, working_disk_gb)`.
/// Vast hosts vary wildly so we don't assert there — vast manifests already
/// declare `disk_gb` and the offer search filters on it.
fn vendor_limits(vendor: Vendor) -> Option<(u32, u32)> {
    match vendor {
        // Kaggle P100 / T4 x2: ~13 GB RAM, ~73 GB writable on /kaggle/working.
        // Source: kaggle.com/docs/efficient-gpu-usage and field-tested.
        Vendor::Kaggle => Some((13, 73)),
        Vendor::Vast | Vendor::Local | Vendor::Ssh | Vendor::Lightning | Vendor::Colab => None,
    }
}

fn requires_checks(manifest: &Manifest, checks: &mut Vec<Check>) {
    let req = match &manifest.requires {
        Some(r) => r,
        None => return,
    };
    let limits = vendor_limits(manifest.vendor);

    if let Some(ram) = req.ram_gb {
        let (ok, warn_only, detail) = match limits {
            Some((cap, _)) if ram > cap => (
                false,
                false,
                format!(
                    "manifest requires {ram} GB RAM but {} caps at ~{cap} GB — \
                     run will likely OOM. Pick a vendor with more RAM or reduce batch size.",
                    manifest.vendor.as_str()
                ),
            ),
            Some((cap, _)) => (
                true,
                false,
                format!(
                    "RAM {ram} GB ≤ {} cap (~{cap} GB)",
                    manifest.vendor.as_str()
                ),
            ),
            None => (
                true,
                true,
                format!(
                    "RAM requirement {ram} GB declared but no static cap known for \
                     vendor '{}' — runtime will decide",
                    manifest.vendor.as_str()
                ),
            ),
        };
        checks.push(Check {
            name: "requires_ram",
            category: "manifests",
            ok,
            warn_only,
            detail,
        });
    }

    if let Some(disk) = req.disk_gb {
        let (ok, warn_only, detail) = match limits {
            Some((_, cap)) if disk > cap => (
                false,
                false,
                format!(
                    "manifest requires {disk} GB working-disk but {} caps at ~{cap} GB — \
                     run will run out of space. Trim datasets, push them as a Kaggle \
                     dataset, or pick a vendor with more disk.",
                    manifest.vendor.as_str()
                ),
            ),
            Some((_, cap)) => (
                true,
                false,
                format!(
                    "disk {disk} GB ≤ {} cap (~{cap} GB)",
                    manifest.vendor.as_str()
                ),
            ),
            None => (
                true,
                true,
                format!(
                    "disk requirement {disk} GB declared but no static cap known for \
                     vendor '{}' — runtime will decide",
                    manifest.vendor.as_str()
                ),
            ),
        };
        checks.push(Check {
            name: "requires_disk",
            category: "manifests",
            ok,
            warn_only,
            detail,
        });
    }
}

fn binary_available(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&paths) {
        if dir.join(name).is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            if dir.join(format!("{name}.exe")).is_file()
                || dir.join(format!("{name}.cmd")).is_file()
                || dir.join(format!("{name}.bat")).is_file()
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use xrun_kaggle::{KaggleError, KaggleProcess};

    #[derive(Default)]
    struct Counts {
        auth: AtomicUsize,
        status: AtomicUsize,
    }

    /// Counts the subprocess calls doctor makes; every dataset is "ready".
    struct CountingKaggle(Arc<Counts>);

    impl KaggleProcess for CountingKaggle {
        fn push(&self, _: &Path) -> Result<String, KaggleError> {
            unreachable!()
        }
        fn status(&self, _: &str) -> Result<String, KaggleError> {
            unreachable!()
        }
        fn output(&self, _: &str, _: &Path) -> Result<String, KaggleError> {
            unreachable!()
        }
        fn cancel(&self, _: &str) -> Result<String, KaggleError> {
            unreachable!()
        }
        fn list_mine(&self) -> Result<String, KaggleError> {
            unreachable!()
        }
        fn config_view(&self) -> Result<String, KaggleError> {
            unreachable!()
        }
        fn datasets_status(&self, _: &str) -> Result<String, KaggleError> {
            self.0.status.fetch_add(1, Ordering::SeqCst);
            Ok("ready".to_string())
        }
        fn datasets_create(&self, _: &Path) -> Result<String, KaggleError> {
            unreachable!()
        }
        fn datasets_version(&self, _: &Path, _: &str) -> Result<String, KaggleError> {
            unreachable!()
        }
        fn datasets_list_mine(&self) -> Result<String, KaggleError> {
            unreachable!()
        }
        fn authenticate_via_python(&self) -> Result<String, KaggleError> {
            self.0.auth.fetch_add(1, Ordering::SeqCst);
            Ok("tester".to_string())
        }
    }

    fn kaggle_manifest(name: &str, datasets: &[&str]) -> Manifest {
        let list: Vec<String> = datasets.iter().map(|d| format!("\"{d}\"")).collect();
        Manifest::from_yaml_str(&format!(
            "name: {name}\nvendor: kaggle\nkaggle:\n  kernel_slug: tester/{name}\n  datasets: [{}]\nrun:\n  cmd: echo hi\n",
            list.join(", ")
        ))
        .expect("test manifest parses")
    }

    #[test]
    fn kaggle_auth_and_dataset_status_are_queried_once_across_manifests() {
        let counts = Arc::new(Counts::default());
        let mut session = KaggleSession::new(KaggleAdapter::with_process(Box::new(
            CountingKaggle(counts.clone()),
        )));
        let manifests = [
            kaggle_manifest("a", &["tester/shared", "tester/only-a"]),
            kaggle_manifest("b", &["tester/shared"]),
            kaggle_manifest("c", &["tester/shared", "tester/only-a"]),
        ];
        let mut checks: Vec<Check> = Vec::new();
        for m in &manifests {
            kaggle_manifest_checks(m, &mut session, &mut checks);
        }

        assert_eq!(counts.auth.load(Ordering::SeqCst), 1, "one authenticate");
        assert_eq!(
            counts.status.load(Ordering::SeqCst),
            2,
            "one datasets status per unique slug"
        );
        // Rows are unchanged: every manifest still reports credentials and
        // each of its datasets.
        let named = |n: &str| checks.iter().filter(|c| c.name == n).count();
        assert_eq!(named("kaggle_credentials"), 3);
        assert_eq!(named("kaggle_dataset"), 5);
        assert!(checks
            .iter()
            .filter(|c| c.name == "kaggle_credentials" || c.name == "kaggle_dataset")
            .all(|c| c.ok));
    }

    #[test]
    fn path_key_equates_spellings_of_the_same_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("m.yaml");
        std::fs::write(&file, "name: m\n").unwrap();
        let dotted = tmp.path().join(".").join("m.yaml");
        assert_eq!(path_key(&file), path_key(&dotted));
        assert_ne!(path_key(&file), path_key(&tmp.path().join("other.yaml")));
    }
}
