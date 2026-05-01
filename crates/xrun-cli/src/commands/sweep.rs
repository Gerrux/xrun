#![deny(unsafe_code)]

//! `xrun sweep` — materialise a Cartesian grid of manifests from a base file.
//!
//! Each `--grid PATH=v1,v2,…` axis multiplies the search space. For every
//! combination we apply the values via the same path-based patcher used by
//! `xrun rerun --patch` and `xrun launch --override`, write the resulting
//! manifest into the output directory, and optionally launch it.
//!
//! Example:
//!     xrun sweep exp/base.yaml \
//!         --grid run.args.--lr=1e-3,5e-4,1e-4 \
//!         --grid run.args.--batch-size=4,8 \
//!         --launch --detach --yes

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use xrun_core::manifest::Manifest;

use crate::cli::{LaunchArgs, SweepArgs};
use crate::commands::{launch, patch};

pub fn run(args: &SweepArgs, db_path: &Path, runs_dir: &Path, config_dir: &Path) -> Result<()> {
    if args.grid.is_empty() {
        anyhow::bail!("no --grid axes provided. Example: --grid run.args.--lr=1e-3,5e-4");
    }
    if args.detach && !args.launch {
        anyhow::bail!("--detach requires --launch");
    }

    let yaml = std::fs::read_to_string(&args.manifest)
        .with_context(|| format!("failed to read {}", args.manifest.display()))?;
    let base = Manifest::from_yaml_str(&yaml)
        .with_context(|| format!("failed to parse {}", args.manifest.display()))?;

    let axes = parse_grid(&args.grid)?;
    let combos = cartesian(&axes);
    if combos.is_empty() {
        anyhow::bail!("grid expanded to zero combinations");
    }

    let out_dir = resolve_out_dir(args, &base);
    if !args.dry_run {
        std::fs::create_dir_all(&out_dir)
            .with_context(|| format!("failed to create output dir {}", out_dir.display()))?;
    }

    let mut materialised: Vec<MaterialisedRun> = Vec::with_capacity(combos.len());
    for combo in &combos {
        let overrides: Vec<String> = combo.iter().map(|(p, v)| format!("{p}={v}")).collect();
        let mut patched = patch::apply(&base, &overrides)
            .with_context(|| format!("failed to apply combo: {overrides:?}"))?;

        let suffix = combo_suffix(combo);
        patched.name = format!("{}_{}", patched.name, suffix);

        let path = out_dir.join(format!("{}.yaml", file_safe(&patched.name)));
        let yaml_out =
            serde_yaml::to_string(&patched).context("failed to serialise patched manifest")?;
        if !args.dry_run {
            std::fs::write(&path, &yaml_out)
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
        materialised.push(MaterialisedRun {
            name: patched.name.clone(),
            path,
            overrides,
        });
    }

    if args.json {
        emit_json(&materialised, args)?;
    } else {
        emit_table(&materialised, args, &out_dir);
    }

    if args.launch && !args.dry_run {
        launch_each(&materialised, args, db_path, runs_dir, config_dir)?;
    }

    Ok(())
}

struct MaterialisedRun {
    name: String,
    path: PathBuf,
    overrides: Vec<String>,
}

fn parse_grid(raw: &[String]) -> Result<Vec<(String, Vec<String>)>> {
    let mut out = Vec::with_capacity(raw.len());
    for entry in raw {
        let (path, values) = entry
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--grid expects PATH=v1,v2,..., got: {entry}"))?;
        if path.is_empty() {
            anyhow::bail!("--grid has empty path: {entry}");
        }
        let vals: Vec<String> = values
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if vals.is_empty() {
            anyhow::bail!("--grid axis {path} has no values");
        }
        out.push((path.to_string(), vals));
    }
    Ok(out)
}

fn cartesian(axes: &[(String, Vec<String>)]) -> Vec<Vec<(String, String)>> {
    let mut acc: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for (path, values) in axes {
        let mut next: Vec<Vec<(String, String)>> = Vec::with_capacity(acc.len() * values.len());
        for prefix in &acc {
            for v in values {
                let mut row = prefix.clone();
                row.push((path.clone(), v.clone()));
                next.push(row);
            }
        }
        acc = next;
    }
    acc
}

/// Build a short suffix like `lr-5e-4_batch-size-4` from a combo. The path
/// segments are stripped so the display name stays readable, but the order
/// matches the user's --grid order so rows in the table line up.
fn combo_suffix(combo: &[(String, String)]) -> String {
    combo
        .iter()
        .map(|(path, value)| {
            let leaf = path
                .rsplit('.')
                .next()
                .unwrap_or(path)
                .trim_start_matches('-');
            let v = file_safe(value);
            format!("{leaf}-{v}")
        })
        .collect::<Vec<_>>()
        .join("_")
}

/// Lowercase, alphanumeric/`-`/`.` only — safe across filesystems and as a
/// manifest name suffix that round-trips through later patches.
fn file_safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn resolve_out_dir(args: &SweepArgs, base: &Manifest) -> PathBuf {
    if let Some(dir) = &args.out {
        return dir.clone();
    }
    let stem = args
        .manifest
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&base.name);
    let ts = Utc::now().format("%Y%m%d-%H%M%S");
    PathBuf::from(format!("exp/sweep_{stem}_{ts}"))
}

fn emit_table(rows: &[MaterialisedRun], args: &SweepArgs, out_dir: &Path) {
    println!(
        "Sweep: {n} combination(s){suffix}",
        n = rows.len(),
        suffix = if args.dry_run {
            " (dry run, no files written)"
        } else {
            ""
        },
    );
    if !args.dry_run {
        println!("Output: {}", out_dir.display());
    }
    println!();
    println!("{:<3}  {:<48}  overrides", "#", "name");
    println!("{}", "-".repeat(110));
    for (i, r) in rows.iter().enumerate() {
        let ov = r.overrides.join("  ");
        println!("{:<3}  {:<48}  {}", i + 1, truncate(&r.name, 48), ov);
    }
    if !args.launch {
        println!();
        println!(
            "Launch them with:  xrun sweep ... --launch [--detach]\n\
             Or one at a time:  xrun launch <path>"
        );
    }
}

fn emit_json(rows: &[MaterialisedRun], args: &SweepArgs) -> Result<()> {
    let arr: Vec<_> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": r.name,
                "path": r.path.display().to_string(),
                "overrides": r.overrides,
            })
        })
        .collect();
    let out = serde_json::json!({
        "count": rows.len(),
        "dry_run": args.dry_run,
        "manifests": arr,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn launch_each(
    rows: &[MaterialisedRun],
    args: &SweepArgs,
    db_path: &Path,
    runs_dir: &Path,
    config_dir: &Path,
) -> Result<()> {
    println!();
    println!(
        "Launching {n} run(s){detached}…",
        n = rows.len(),
        detached = if args.detach { " (detached)" } else { "" },
    );
    let total = rows.len();
    for (i, r) in rows.iter().enumerate() {
        println!("\n[{}/{}] {}", i + 1, total, r.name);
        let launch_args = LaunchArgs {
            manifest: r.path.clone(),
            dry_run: false,
            allow_duplicate: true,
            name: None,
            json: false,
            detach: args.detach,
            max_cost: None,
            max_hours: None,
            idle_timeout: None,
            yes: args.yes,
            reuse_instance: None,
            upload_only: false,
            overrides: Vec::new(),
            trace: false,
        };
        if let Err(e) = launch::run(&launch_args, db_path, runs_dir, config_dir) {
            eprintln!("  failed: {e:#}");
            // Continue with the remaining combos — a single bad GPU offer
            // shouldn't sink the whole sweep.
        }
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_grid_handles_multi_value() {
        let axes = parse_grid(&[
            "run.args.--lr=1e-3,5e-4,1e-4".to_string(),
            "run.args.--batch=4,8".to_string(),
        ])
        .unwrap();
        assert_eq!(axes.len(), 2);
        assert_eq!(axes[0].1, vec!["1e-3", "5e-4", "1e-4"]);
        assert_eq!(axes[1].1, vec!["4", "8"]);
    }

    #[test]
    fn parse_grid_rejects_missing_equals() {
        let err = parse_grid(&["run.args.lr".to_string()]).unwrap_err();
        assert!(err.to_string().contains("PATH=v1"));
    }

    #[test]
    fn parse_grid_rejects_empty_values() {
        let err = parse_grid(&["lr=".to_string()]).unwrap_err();
        assert!(err.to_string().contains("no values"));
    }

    #[test]
    fn cartesian_3x2_yields_6_combinations() {
        let axes = vec![
            ("a".to_string(), vec!["1".into(), "2".into(), "3".into()]),
            ("b".to_string(), vec!["x".into(), "y".into()]),
        ];
        let combos = cartesian(&axes);
        assert_eq!(combos.len(), 6);
        assert_eq!(
            combos[0],
            vec![("a".into(), "1".into()), ("b".into(), "x".into())]
        );
        assert_eq!(
            combos[5],
            vec![("a".into(), "3".into()), ("b".into(), "y".into())]
        );
    }

    #[test]
    fn cartesian_single_axis_passes_through() {
        let axes = vec![("a".to_string(), vec!["1".into(), "2".into()])];
        assert_eq!(cartesian(&axes).len(), 2);
    }

    #[test]
    fn combo_suffix_strips_path_and_dashes() {
        let combo = vec![
            ("run.args.--lr".to_string(), "5e-4".to_string()),
            ("run.args.--batch-size".to_string(), "4".to_string()),
        ];
        assert_eq!(combo_suffix(&combo), "lr-5e-4_batch-size-4");
    }

    #[test]
    fn file_safe_strips_unfriendly_chars() {
        assert_eq!(file_safe("foo bar/baz?"), "foo-bar-baz");
        assert_eq!(file_safe("1e-3"), "1e-3");
        assert_eq!(file_safe("0.5"), "0.5");
    }
}
