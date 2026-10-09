#![deny(unsafe_code)]

//! Plumbing shared by the command modules: credential resolution, vendor
//! adapter construction, store access and run-id resolution. Each of these
//! used to be copy-pasted into five to ten commands, and the copies drifted.

use std::path::Path;

use anyhow::{bail, Context, Result};
use xrun_colab::ColabAdapter;
use xrun_core::{
    config::credentials::{KaggleCredentials, VastCredentials},
    manifest::Manifest,
    store::{ListFilter, Run, RunId},
    Credentials, GlobalConfig, Store, VendorAdapter,
};
use xrun_kaggle::KaggleAdapter;
use xrun_lightning::LightningAdapter;
use xrun_local::LocalAdapter;
use xrun_ssh::SshAdapter;
use xrun_vast::VastAdapter;

/// Every vendor a run can be recorded under.
pub const KNOWN_VENDORS: &[&str] = &["vast", "kaggle", "local", "ssh", "lightning", "colab"];

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

/// Resolve `vast.api_key` from xrun's `credentials.toml`, falling back to the
/// legacy `~/.config/vastai/vast_api_key` file. Never fails: when neither is
/// set the returned credentials are empty, so `--dry-run` / `validate` paths
/// that don't touch the network still work.
pub fn resolve_vast_credentials(config_dir: &Path) -> VastCredentials {
    if let Ok(creds) = Credentials::load(config_dir) {
        if creds.vast.api_key.is_some() {
            return creds.vast;
        }
    }
    if let Ok(Some(token)) = Credentials::import_vast_native() {
        return VastCredentials {
            api_key: Some(token),
        };
    }
    VastCredentials::default()
}

/// Resolve Kaggle credentials: xrun's `credentials.toml` (token, or
/// username+key), then the native `~/.kaggle/kaggle.json`, then the
/// access-token file / env var. Never fails; empty when nothing is set.
pub fn resolve_kaggle_credentials(config_dir: &Path) -> KaggleCredentials {
    if let Ok(creds) = Credentials::load(config_dir) {
        if creds.kaggle.token.is_some()
            || (creds.kaggle.username.is_some() && creds.kaggle.key.is_some())
        {
            return creds.kaggle;
        }
    }
    if let Ok(Some((username, key))) = Credentials::import_kaggle_native() {
        return KaggleCredentials {
            token: None,
            username: Some(username),
            key: Some(key),
        };
    }
    if let Ok(Some(token)) = Credentials::import_kaggle_access_token() {
        return KaggleCredentials {
            token: Some(token),
            username: None,
            key: None,
        };
    }
    KaggleCredentials::default()
}

// ---------------------------------------------------------------------------
// Store and run ids
// ---------------------------------------------------------------------------

/// Open the SQLite store with the uniform error context.
pub fn open_store(db_path: &Path) -> Result<Store> {
    Store::open(db_path).with_context(|| format!("failed to open store at {}", db_path.display()))
}

/// Shortest prefix / suffix that is accepted as a run-id abbreviation.
/// ULIDs created within the same few days share their leading characters, so
/// anything shorter is more likely a typo than an intent.
const MIN_PARTIAL_ID_LEN: usize = 4;

/// Runs matching `id`: an exact (full-ULID) match wins outright; otherwise
/// every run whose id starts or ends with `id`, case-insensitively. Partial
/// matches need at least [`MIN_PARTIAL_ID_LEN`] characters.
pub fn match_runs<'a>(runs: &'a [Run], id: &str) -> Vec<&'a Run> {
    // `RunId` renders as upper-case Crockford base32.
    let needle = id.trim().to_ascii_uppercase();
    if needle.is_empty() {
        return Vec::new();
    }
    if let Some(exact) = runs.iter().find(|r| r.id.to_string() == needle) {
        return vec![exact];
    }
    if needle.len() < MIN_PARTIAL_ID_LEN {
        return Vec::new();
    }
    runs.iter()
        .filter(|r| {
            let full = r.id.to_string();
            full.starts_with(&needle) || full.ends_with(&needle)
        })
        .collect()
}

/// Resolve a user-supplied run id: a full ULID, or a unique prefix / suffix
/// of one (case-insensitive).
pub fn resolve_run(store: &Store, id: &str) -> Result<Run> {
    let id = id.trim();
    // Full ULID: direct lookup, no scan.
    if let Ok(full) = id.parse::<RunId>() {
        return store
            .get_run(&full)?
            .ok_or_else(|| anyhow::anyhow!("run not found: {id} (see `xrun ls`)"));
    }
    let runs = store.list_runs(&ListFilter::default())?;
    let hits = match_runs(&runs, id);
    match hits.len() {
        1 => Ok(hits[0].clone()),
        0 => bail!("run not found: {id} (see `xrun ls`)"),
        n => {
            const SHOWN: usize = 5;
            let mut list: Vec<String> = hits
                .iter()
                .take(SHOWN)
                .map(|r| format!("{} ({})", r.id, r.name))
                .collect();
            if n > SHOWN {
                list.push(format!("... and {} more", n - SHOWN));
            }
            bail!(
                "run id `{id}` is ambiguous ({n} matches): {}; use more characters",
                list.join(", ")
            )
        }
    }
}

/// Outcome of [`select_run`].
pub enum RunSelection {
    Run(Box<Run>),
    /// No id given and nothing is active.
    NoActive,
    /// No id given and several runs are active (count).
    Multiple(usize),
}

/// `id` given: resolve it like [`resolve_run`]. No id: the single active run
/// if there is exactly one. The caller words the 0 / many cases, since each
/// command phrases them differently.
pub fn select_run(store: &Store, id: Option<&str>) -> Result<RunSelection> {
    if let Some(id) = id {
        return Ok(RunSelection::Run(Box::new(resolve_run(store, id)?)));
    }
    let mut active = store.list_active_runs()?;
    Ok(match active.len() {
        0 => RunSelection::NoActive,
        1 => RunSelection::Run(Box::new(active.remove(0))),
        n => RunSelection::Multiple(n),
    })
}

// ---------------------------------------------------------------------------
// Vendor adapters
// ---------------------------------------------------------------------------

/// Where an `ssh` run's connection details come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshSource {
    /// The `manifest.yaml` copy saved in the run dir names the host alias.
    SavedManifest,
    /// `XRUN_SSH_ALIAS` if set, else the saved manifest, else — only when the
    /// manifest copy is gone — the first host in `credentials.toml`.
    EnvThenSavedManifest,
}

/// Pick the ssh host alias and workdir for [`SshSource::EnvThenSavedManifest`].
///
/// The run's own manifest outranks "whatever host comes first": with several
/// hosts configured, guessing sent `pull` to a machine the run never ran on.
/// The env var stays an explicit override; the saved workdir only applies to
/// the host it was recorded for. The last-resort host is the alphabetically
/// first one, so the guess is at least the same on every call.
fn pick_ssh_target(
    env_alias: Option<String>,
    saved: Option<(String, Option<String>)>,
    mut hosts: Vec<String>,
) -> Option<(String, Option<String>)> {
    if let Some(alias) = env_alias {
        let workdir = saved.filter(|(a, _)| *a == alias).and_then(|(_, w)| w);
        return Some((alias, workdir));
    }
    if saved.is_some() {
        return saved;
    }
    hosts.sort();
    hosts.into_iter().next().map(|alias| (alias, None))
}

/// Everything `build_adapter` needs to reconstruct an adapter for an
/// existing run, plus the per-command knobs that legitimately differ.
pub struct AdapterCtx<'a> {
    /// Command name, used as an error prefix and in "not supported" errors.
    pub command: &'a str,
    pub db_path: &'a Path,
    pub runs_dir: &'a Path,
    pub config_dir: &'a Path,
    pub run_id: &'a RunId,
    /// Vendors this command accepts; a vendor outside it is an error naming
    /// the command. Defaults to all of [`KNOWN_VENDORS`].
    pub supported: &'a [&'a str],
    pub ssh: SshSource,
    /// Kaggle: hand the adapter the data dir (parent of `runs.db`), which
    /// its post-download ingest and telemetry backfill need. Default on.
    pub kaggle_store_path: bool,
    /// Kaggle: wire MLflow (when `mlflow.url` is configured) so live-log /
    /// telemetry chunks can be read back. Default off.
    pub kaggle_mlflow: bool,
}

impl<'a> AdapterCtx<'a> {
    pub fn new(
        command: &'a str,
        db_path: &'a Path,
        runs_dir: &'a Path,
        config_dir: &'a Path,
        run_id: &'a RunId,
    ) -> Self {
        Self {
            command,
            db_path,
            runs_dir,
            config_dir,
            run_id,
            supported: KNOWN_VENDORS,
            ssh: SshSource::SavedManifest,
            kaggle_store_path: true,
            kaggle_mlflow: false,
        }
    }
}

/// Reject a vendor that is unknown, or known but not handled by `command`.
pub fn ensure_vendor_supported(command: &str, vendor: &str, supported: &[&str]) -> Result<()> {
    if !KNOWN_VENDORS.contains(&vendor) {
        bail!(
            "unknown vendor `{vendor}` (known: {})",
            KNOWN_VENDORS.join(", ")
        );
    }
    if !supported.contains(&vendor) {
        bail!(
            "`xrun {command}` is not supported for vendor {vendor} (supported: {})",
            supported.join(", ")
        );
    }
    Ok(())
}

/// Vendor name to build an adapter for. Pollers and reconcilers have always
/// treated a vendor string they don't recognise as vast; commands that keep
/// that behaviour pass the stored string through this.
pub fn vendor_or_vast(vendor: &str) -> &str {
    if KNOWN_VENDORS.contains(&vendor) {
        vendor
    } else {
        "vast"
    }
}

/// Build the adapter for `vendor` (vast, kaggle, local, ssh, lightning, colab) bound to
/// `ctx.run_id`. Opens its own store handle where the adapter needs one.
pub fn build_adapter(vendor: &str, ctx: &AdapterCtx<'_>) -> Result<Box<dyn VendorAdapter>> {
    ensure_vendor_supported(ctx.command, vendor, ctx.supported)?;
    let adapter: Box<dyn VendorAdapter> = match vendor {
        "vast" => {
            let creds = resolve_vast_credentials(ctx.config_dir);
            Box::new(VastAdapter::new(creds, open_adapter_store(ctx)?))
        }
        "kaggle" => {
            let creds = resolve_kaggle_credentials(ctx.config_dir);
            let mut adapter = KaggleAdapter::new().with_credentials(creds);
            if ctx.kaggle_store_path {
                let data_dir = ctx.db_path.parent().unwrap_or(ctx.db_path);
                adapter = adapter.with_store_path(data_dir.to_path_buf());
            }
            if ctx.kaggle_mlflow {
                if let Ok(g) = GlobalConfig::load(ctx.config_dir) {
                    if let Some(url) = g.mlflow.url.clone() {
                        let creds = Credentials::load(ctx.config_dir).unwrap_or_default();
                        let auth = crate::commands::launch::mlflow_auth_from_creds(&creds.mlflow);
                        adapter = adapter.with_mlflow(url, auth);
                    }
                }
            }
            Box::new(adapter)
        }
        "local" => Box::new(LocalAdapter::with_store_and_runs_dir(
            open_adapter_store(ctx)?,
            ctx.runs_dir.to_path_buf(),
        )),
        "ssh" => build_ssh_adapter(ctx)?,
        // Studio name and teamspace live in the stored instance handle, so
        // neither adapter needs the manifest here.
        "lightning" => {
            let creds = Credentials::load(ctx.config_dir).unwrap_or_default();
            Box::new(LightningAdapter::new(
                open_adapter_store(ctx)?,
                creds.lightning,
            ))
        }
        "colab" => Box::new(ColabAdapter::new(open_adapter_store(ctx)?)),
        other => bail!("{}: no adapter for vendor {other}", ctx.command),
    };
    adapter.set_run_id(ctx.run_id);
    Ok(adapter)
}

fn open_adapter_store(ctx: &AdapterCtx<'_>) -> Result<Store> {
    Store::open(ctx.db_path)
        .with_context(|| format!("failed to open adapter store at {}", ctx.db_path.display()))
}

/// Host alias and workdir recorded in the run's saved `manifest.yaml`.
fn saved_ssh_spec(ctx: &AdapterCtx<'_>) -> Result<(String, Option<String>)> {
    let cmd = ctx.command;
    let manifest_path = ctx
        .runs_dir
        .join(ctx.run_id.to_string())
        .join("manifest.yaml");
    let yaml = std::fs::read_to_string(&manifest_path).with_context(|| {
        format!(
            "{cmd}: cannot read saved SSH manifest at {}",
            manifest_path.display()
        )
    })?;
    // Lenient parse: the copy was validated at launch time.
    let manifest: Manifest = serde_yaml::from_str(&yaml)
        .with_context(|| format!("{cmd}: saved SSH manifest is unparsable"))?;
    let spec = manifest
        .ssh
        .as_ref()
        .with_context(|| format!("{cmd}: saved manifest has no ssh configuration"))?;
    Ok((spec.host_alias.clone(), spec.workdir.clone()))
}

fn build_ssh_adapter(ctx: &AdapterCtx<'_>) -> Result<Box<dyn VendorAdapter>> {
    let cmd = ctx.command;
    let creds = Credentials::load(ctx.config_dir).unwrap_or_default();
    let (alias, manifest_workdir) = match ctx.ssh {
        SshSource::SavedManifest => saved_ssh_spec(ctx)?,
        SshSource::EnvThenSavedManifest => pick_ssh_target(
            std::env::var("XRUN_SSH_ALIAS").ok(),
            saved_ssh_spec(ctx).ok(),
            creds.ssh_hosts.keys().cloned().collect(),
        )
        .ok_or_else(|| anyhow::anyhow!("{cmd}: no ssh hosts in credentials.toml"))?,
    };
    let host_creds = creds.ssh_hosts.get(&alias).ok_or_else(|| {
        anyhow::anyhow!("{cmd}: ssh alias '{alias}' missing from credentials.toml")
    })?;
    let conn = SshAdapter::resolve_conn(&alias, host_creds)?;
    let workdir_root = xrun_ssh::resolve_workdir_root(
        manifest_workdir.as_deref(),
        host_creds.default_workdir.as_deref(),
    );
    Ok(Box::new(SshAdapter::new(
        open_adapter_store(ctx)?,
        conn,
        workdir_root,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_ssh_target_prefers_the_runs_own_host() {
        let saved = Some(("nas".to_string(), Some("/data/xrun".to_string())));
        let hosts = vec!["vps".to_string(), "nas".to_string()];

        // The run's manifest wins over "some configured host".
        assert_eq!(pick_ssh_target(None, saved.clone(), hosts.clone()), saved);
        // An explicit env override wins, and does not inherit a workdir that
        // was recorded for a different host.
        assert_eq!(
            pick_ssh_target(Some("vps".into()), saved.clone(), hosts.clone()),
            Some(("vps".to_string(), None))
        );
        assert_eq!(
            pick_ssh_target(Some("nas".into()), saved.clone(), hosts.clone()),
            saved
        );
        // No manifest copy: a stable guess, not HashMap order.
        assert_eq!(
            pick_ssh_target(None, None, hosts),
            Some(("nas".to_string(), None))
        );
        assert_eq!(pick_ssh_target(None, None, Vec::new()), None);
    }

    fn store_with_runs(n: usize) -> (tempfile::TempDir, Store, Vec<Run>) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("runs.db")).unwrap();
        let mut runs = Vec::new();
        for i in 0..n {
            let id = store
                .create_run(&format!("exp{i}"), "hash", "m.yaml", "local", &[])
                .unwrap();
            runs.push(store.get_run(&id).unwrap().unwrap());
        }
        (dir, store, runs)
    }

    /// Longest common prefix of all ids (they share the timestamp part).
    fn common_prefix(runs: &[Run]) -> String {
        let first = runs[0].id.to_string();
        let mut len = first.len();
        for r in runs {
            let s = r.id.to_string();
            len = len.min(
                first
                    .chars()
                    .zip(s.chars())
                    .take_while(|(a, b)| a == b)
                    .count(),
            );
        }
        first[..len].to_string()
    }

    #[test]
    fn resolve_full_id_any_case() {
        let (_d, store, runs) = store_with_runs(2);
        let full = runs[1].id.to_string();
        assert_eq!(resolve_run(&store, &full).unwrap().name, "exp1");
        assert_eq!(
            resolve_run(&store, &full.to_ascii_lowercase())
                .unwrap()
                .name,
            "exp1"
        );
    }

    #[test]
    fn resolve_unique_prefix_and_suffix() {
        let (_d, store, runs) = store_with_runs(2);
        let full = runs[0].id.to_string();
        // Past the shared timestamp part the prefix is unique.
        assert_eq!(resolve_run(&store, &full[..20]).unwrap().name, "exp0");
        assert_eq!(
            resolve_run(&store, &full[full.len() - 8..]).unwrap().name,
            "exp0"
        );
        assert_eq!(
            resolve_run(&store, &full[full.len() - 8..].to_ascii_lowercase())
                .unwrap()
                .name,
            "exp0"
        );
    }

    #[test]
    fn resolve_ambiguous_lists_candidates() {
        let (_d, store, runs) = store_with_runs(7);
        let prefix = common_prefix(&runs);
        assert!(prefix.len() >= MIN_PARTIAL_ID_LEN, "ids share a timestamp");
        let err = resolve_run(&store, &prefix).unwrap_err().to_string();
        assert!(err.contains("ambiguous (7 matches)"), "{err}");
        assert!(err.contains("exp"), "{err}");
        assert!(err.contains("and 2 more"), "{err}");
        // Only 5 candidate ids are spelled out.
        assert_eq!(err.matches(&prefix).count(), 5 + 1, "{err}");
    }

    #[test]
    fn resolve_missing_and_too_short() {
        let (_d, store, runs) = store_with_runs(2);
        let err = resolve_run(&store, "ZZZZZZZZ").unwrap_err().to_string();
        assert_eq!(err, "run not found: ZZZZZZZZ (see `xrun ls`)");
        // A valid-looking full id that does not exist.
        let other = RunId::new().to_string();
        let err = resolve_run(&store, &other).unwrap_err().to_string();
        assert!(err.starts_with("run not found:"), "{err}");
        // Below the minimum length nothing partial matches.
        let first_char = &runs[0].id.to_string()[..1];
        assert!(resolve_run(&store, first_char).is_err());
    }

    #[test]
    fn select_run_without_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("runs.db")).unwrap();
        assert!(matches!(
            select_run(&store, None).unwrap(),
            RunSelection::NoActive
        ));
        let a = store.create_run("a", "h", "m", "local", &[]).unwrap();
        match select_run(&store, None).unwrap() {
            RunSelection::Run(r) => assert_eq!(r.id, a),
            _ => panic!("expected the single active run"),
        }
        store.create_run("b", "h", "m", "local", &[]).unwrap();
        assert!(matches!(
            select_run(&store, None).unwrap(),
            RunSelection::Multiple(2)
        ));
    }

    /// Runs carrying hand-picked ids (the store assigns random ones), so the
    /// prefix-vs-suffix edge cases can be pinned exactly.
    fn runs_with_ids(ids: &[&str]) -> Vec<Run> {
        let (_d, _s, mut runs) = store_with_runs(ids.len());
        for (r, id) in runs.iter_mut().zip(ids) {
            r.id = id.parse().expect("valid ULID literal");
        }
        runs
    }

    const SUFFIX_RUN: &str = "01K6AAAAAAAAAAAAAAAAAA7XYZ";
    const PREFIX_RUN: &str = "7XYZBBBBBBBBBBBBBBBBBBBBBB";

    fn ids(hits: &[&Run]) -> Vec<String> {
        hits.iter().map(|r| r.id.to_string()).collect()
    }

    #[test]
    fn prefix_of_one_and_suffix_of_another_is_ambiguous() {
        // A destructive command must never pick "the first" of these.
        let runs = runs_with_ids(&[SUFFIX_RUN, PREFIX_RUN]);
        let hits = match_runs(&runs, "7xyz");
        assert_eq!(ids(&hits), vec![SUFFIX_RUN, PREFIX_RUN]);
        // One more character disambiguates in either direction.
        assert_eq!(ids(&match_runs(&runs, "7XYZB")), vec![PREFIX_RUN]);
        assert_eq!(ids(&match_runs(&runs, "a7xyz")), vec![SUFFIX_RUN]);
    }

    #[test]
    fn run_matching_as_prefix_and_suffix_counts_once() {
        let runs = runs_with_ids(&["7XYZCCCCCCCCCCCCCCCCCC7XYZ"]);
        assert_eq!(match_runs(&runs, "7XYZ").len(), 1);
    }

    #[test]
    fn partial_ids_need_four_characters_and_ignore_padding() {
        let runs = runs_with_ids(&[SUFFIX_RUN]);
        // Unique, but too short to be accepted.
        assert!(match_runs(&runs, "XYZ").is_empty());
        assert!(match_runs(&runs, "01K").is_empty());
        assert_eq!(ids(&match_runs(&runs, "7XYZ")), vec![SUFFIX_RUN]);
        assert_eq!(ids(&match_runs(&runs, "  01k6 ")), vec![SUFFIX_RUN]);
        assert!(match_runs(&runs, "   ").is_empty());
    }

    #[test]
    fn resolution_scans_every_run_not_a_page() {
        // A LIMIT on the scan would hide old runs: an abbreviation of one of
        // them would then read as "not found", or a collision with it would
        // look unique.
        let (_d, store, runs) = store_with_runs(150);
        let oldest = runs[0].id.to_string();
        let tail = &oldest[oldest.len() - 10..];
        assert_eq!(resolve_run(&store, tail).unwrap().name, "exp0");
        // An explicit id bypasses the "single active run" rule (150 active).
        match select_run(&store, Some(tail)).unwrap() {
            RunSelection::Run(r) => assert_eq!(r.name, "exp0"),
            _ => panic!("explicit id must resolve directly"),
        }
    }

    fn ctx_err(vendor: &str, supported: &[&str]) -> String {
        let id = RunId::new();
        let p = Path::new("unused");
        let mut ctx = AdapterCtx::new("shell", p, p, p, &id);
        ctx.supported = supported;
        match build_adapter(vendor, &ctx) {
            Ok(_) => panic!("expected an error for {vendor}"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn build_adapter_unknown_vendor_lists_known() {
        let err = ctx_err("runpod", KNOWN_VENDORS);
        assert_eq!(
            err,
            "unknown vendor `runpod` (known: vast, kaggle, local, ssh, lightning, colab)"
        );
    }

    #[test]
    fn build_adapter_unsupported_vendor_names_command() {
        let err = ctx_err("kaggle", &["vast", "ssh"]);
        assert_eq!(
            err,
            "`xrun shell` is not supported for vendor kaggle (supported: vast, ssh)"
        );
    }

    #[test]
    fn vendor_or_vast_falls_back() {
        assert_eq!(vendor_or_vast("kaggle"), "kaggle");
        assert_eq!(vendor_or_vast("whatever"), "vast");
    }
}
