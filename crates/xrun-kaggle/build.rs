fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    // crates/xrun-kaggle  →  ../..  →  workspace root
    let workspace_root = std::path::Path::new(&manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from(&manifest_dir));

    let dist_dir = workspace_root
        .join("python")
        .join("xrun_hook")
        .join("dist");

    // Rebuild when the dist directory or the env flag changes.
    println!("cargo:rerun-if-changed={}", dist_dir.display());
    println!("cargo:rerun-if-env-changed=XRUN_KAGGLE_EMBED_WHEEL");

    let embed_wheel = std::env::var("CARGO_FEATURE_EMBED_WHEEL").is_ok();
    if !embed_wheel {
        return;
    }

    // Feature is active — locate the wheel or abort with a clear message.
    match find_wheel(&dist_dir) {
        Some(wheel_path) => {
            println!(
                "cargo:rustc-env=XRUN_HOOK_WHEEL_PATH={}",
                wheel_path.display()
            );
            println!("cargo:rerun-if-changed={}", wheel_path.display());
            eprintln!("cargo:warning=embedding xrun_hook wheel: {}", wheel_path.display());
        }
        None => {
            panic!(
                "\n\
                 Feature 'embed-wheel' is enabled but no xrun_hook wheel was found in:\n  \
                 {dist}\n\n\
                 Build the wheel first:\n  \
                 cd python/xrun_hook && python -m build --wheel\n",
                dist = dist_dir.display()
            );
        }
    }
}

fn find_wheel(dist_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dist_dir).ok()?;
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.extension().map(|ext| ext == "whl").unwrap_or(false)
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("xrun_hook"))
                    .unwrap_or(false)
        })
}
