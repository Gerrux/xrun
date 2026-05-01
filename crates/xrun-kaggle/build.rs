// Build-time: locate (or build, best-effort) the xrun_hook wheel and embed it
// into the Kaggle adapter. The Kaggle workflow needs this wheel in every
// kernel push to enable live event/metric streaming.
//
// Behaviour, in order:
//   1. If a wheel exists at python/xrun_hook/dist/xrun_hook-*.whl → embed it.
//   2. Else, when XRUN_KAGGLE_AUTO_BUILD_WHEEL=1 (or the marker file is
//      missing entirely on a clean checkout but Python is available), try
//      `python -m build --wheel python/xrun_hook` once. If that succeeds and
//      produces a wheel, embed it.
//   3. Else, fall through with no embedding. The Rust code falls back to an
//      empty byte slice and the Kaggle adapter degrades to a warn at runtime
//      rather than failing the build.
//
// To force a hard error when the wheel can't be embedded (e.g. in release CI):
//   XRUN_KAGGLE_EMBED_WHEEL=strict cargo build
//
// Setting `cargo:rustc-cfg=xrun_hook_wheel_embedded` is the single switch that
// flips embed.rs from the empty-stub branch to the include_bytes!() branch.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = Path::new(&manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(&manifest_dir));

    let hook_dir = workspace_root.join("python").join("xrun_hook");
    let dist_dir = hook_dir.join("dist");
    let pyproject = hook_dir.join("pyproject.toml");

    println!("cargo::rustc-check-cfg=cfg(xrun_hook_wheel_embedded)");
    println!("cargo:rerun-if-changed={}", dist_dir.display());
    println!("cargo:rerun-if-changed={}", pyproject.display());
    println!("cargo:rerun-if-changed={}", hook_dir.join("src").display());
    println!("cargo:rerun-if-env-changed=XRUN_KAGGLE_EMBED_WHEEL");
    println!("cargo:rerun-if-env-changed=XRUN_KAGGLE_AUTO_BUILD_WHEEL");

    let strict = std::env::var("XRUN_KAGGLE_EMBED_WHEEL").as_deref() == Ok("strict");

    // Try existing wheel first.
    if let Some(path) = find_wheel(&dist_dir) {
        emit_embed(&path);
        return;
    }

    // Optional auto-build. Default off — opt-in via env to keep clean clones
    // fast. Wheel-builders (release CI) set XRUN_KAGGLE_AUTO_BUILD_WHEEL=1.
    let auto_build = std::env::var("XRUN_KAGGLE_AUTO_BUILD_WHEEL")
        .as_deref()
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
        || strict;

    if auto_build && pyproject.exists() {
        match try_build_wheel(&hook_dir) {
            Ok(()) => {
                if let Some(path) = find_wheel(&dist_dir) {
                    emit_embed(&path);
                    return;
                }
                println!(
                    "cargo:warning=xrun_hook auto-build succeeded but no wheel \
                     was found under {}",
                    dist_dir.display()
                );
            }
            Err(msg) => {
                println!("cargo:warning=xrun_hook auto-build skipped/failed: {msg}");
            }
        }
    }

    if strict {
        panic!(
            "\n\
             XRUN_KAGGLE_EMBED_WHEEL=strict but no xrun_hook wheel found in:\n  \
             {dist}\n\n\
             Build the wheel first:\n  \
             cd python/xrun_hook && python -m build --wheel\n",
            dist = dist_dir.display()
        );
    }

    println!(
        "cargo:warning=xrun_hook wheel not embedded — Kaggle live metrics \
         disabled. Run `cd python/xrun_hook && python -m build --wheel` \
         (or set XRUN_KAGGLE_AUTO_BUILD_WHEEL=1) and rebuild."
    );
}

fn emit_embed(path: &Path) {
    println!("cargo:rustc-cfg=xrun_hook_wheel_embedded");
    println!("cargo:rustc-env=XRUN_HOOK_WHEEL_PATH={}", path.display());
    println!("cargo:rerun-if-changed={}", path.display());
    println!(
        "cargo:warning=embedding xrun_hook wheel: {}",
        path.display()
    );
}

fn find_wheel(dist_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dist_dir).ok()?;
    let mut wheels: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().map(|ext| ext == "whl").unwrap_or(false)
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("xrun_hook"))
                    .unwrap_or(false)
        })
        .collect();
    // Prefer the wheel with the most recent mtime.
    wheels.sort_by_key(|p| p.metadata().and_then(|m| m.modified()).ok());
    wheels.pop()
}

fn try_build_wheel(hook_dir: &Path) -> Result<(), String> {
    let python = pick_python().ok_or("no python interpreter found in PATH")?;

    let attempt = |args: &[&str]| -> Result<(), String> {
        let mut cmd = Command::new(&python);
        cmd.args(args).current_dir(hook_dir);
        let output = cmd
            .output()
            .map_err(|e| format!("failed to spawn {python}: {e}", python = python.display()))?;
        if !output.status.success() {
            let tail: String = String::from_utf8_lossy(&output.stderr)
                .lines()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ");
            return Err(format!(
                "exit {code}: {tail}",
                code = output.status.code().unwrap_or(-1)
            ));
        }
        Ok(())
    };

    // Prefer `python -m build --wheel`. Fall back to `pip wheel . -w dist`
    // when `build` isn't installed (still a clean wheel via the build backend).
    if attempt(&["-m", "build", "--wheel", "--outdir", "dist"]).is_ok() {
        return Ok(());
    }
    attempt(&["-m", "pip", "wheel", ".", "-w", "dist", "--no-deps"])
}

fn pick_python() -> Option<PathBuf> {
    let names = if cfg!(windows) {
        ["python.exe", "python3.exe", "py.exe"].as_slice()
    } else {
        ["python3", "python"].as_slice()
    };
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
