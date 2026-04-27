use std::{collections::HashMap, path::PathBuf};

use xrun_core::manifest::{DataMode, DataSource, RunSpec, UnpackSpec};
use xrun_vast::{
    cli::CopyEndpoint,
    error::VastError,
    execute::render_args,
    upload::{copy_endpoints, unpack_commands},
};

// ─── copy mode ───────────────────────────────────────────────────────────────

#[test]
fn copy_mode_builds_correct_endpoints() {
    let instance_id: u64 = 9876;
    let source = DataSource {
        src: "/local/dataset.tar".to_string(),
        dst: "/workspace/dataset.tar".to_string(),
        mode: None,
        unpack: None,
    };

    let (src, dst) = copy_endpoints(instance_id, &source);

    match src {
        CopyEndpoint::Local(p) => assert_eq!(p, PathBuf::from("/local/dataset.tar")),
        _ => panic!("expected Local endpoint for src"),
    }

    match dst {
        CopyEndpoint::Remote { instance, path } => {
            assert_eq!(instance, instance_id);
            assert_eq!(path, "/workspace/dataset.tar");
        }
        _ => panic!("expected Remote endpoint for dst"),
    }
}

#[test]
fn explicit_copy_mode_same_as_default() {
    let instance_id: u64 = 111;
    let source = DataSource {
        src: "/data/file.bin".to_string(),
        dst: "/remote/file.bin".to_string(),
        mode: Some(DataMode::Copy),
        unpack: None,
    };

    let (src, dst) = copy_endpoints(instance_id, &source);

    assert!(matches!(src, CopyEndpoint::Local(_)));
    assert!(matches!(dst, CopyEndpoint::Remote { instance: 111, .. }));
}

// ─── rsync mode ──────────────────────────────────────────────────────────────

#[test]
fn rsync_not_found_in_empty_search_path() {
    // Simulate "rsync not in PATH" by restricting the search to an empty temp dir.
    let temp = tempfile::tempdir().expect("tempdir");
    let result =
        which::which_in("rsync", Some(temp.path()), ".").map_err(|_| VastError::RsyncNotFound);

    assert!(
        matches!(result, Err(VastError::RsyncNotFound)),
        "expected RsyncNotFound when rsync is absent from the search path"
    );
}

// ─── unpack commands ─────────────────────────────────────────────────────────

#[test]
fn unpack_tar_generates_mkdir_and_extract() {
    let source = DataSource {
        src: "/local/data.tar".to_string(),
        dst: "/workspace/data.tar".to_string(),
        mode: None,
        unpack: Some(UnpackSpec {
            format: "tar".to_string(),
            into: "/workspace/data".to_string(),
        }),
    };

    let cmds = unpack_commands(&source).expect("should not fail for tar");
    assert_eq!(cmds.len(), 2);
    assert_eq!(cmds[0], "mkdir -p /workspace/data");
    assert_eq!(cmds[1], "tar xf /workspace/data.tar -C /workspace/data");
}

#[test]
fn unpack_tar_gz_uses_xzf_flag() {
    let source = DataSource {
        src: "/local/data.tar.gz".to_string(),
        dst: "/workspace/data.tar.gz".to_string(),
        mode: None,
        unpack: Some(UnpackSpec {
            format: "tar.gz".to_string(),
            into: "/workspace/data".to_string(),
        }),
    };

    let cmds = unpack_commands(&source).expect("should not fail for tar.gz");
    assert_eq!(cmds[1], "tar xzf /workspace/data.tar.gz -C /workspace/data");
}

#[test]
fn unpack_none_returns_empty_vec() {
    let source = DataSource {
        src: "/a".to_string(),
        dst: "/b".to_string(),
        mode: None,
        unpack: None,
    };
    let cmds = unpack_commands(&source).expect("no unpack");
    assert!(cmds.is_empty());
}

// ─── render_args: alphabetical order ─────────────────────────────────────────

#[test]
fn render_args_sorts_alphabetically() {
    let mut args: HashMap<String, serde_json::Value> = HashMap::new();
    args.insert("--lr".to_string(), serde_json::json!(0.0001_f64));
    args.insert("--batch".to_string(), serde_json::json!(8_u64));

    let rendered = render_args(&args);

    let batch_pos = rendered.find("--batch").expect("--batch in output");
    let lr_pos = rendered.find("--lr").expect("--lr in output");
    assert!(
        batch_pos < lr_pos,
        "--batch should appear before --lr (alphabetical): got '{}'",
        rendered
    );
    assert!(rendered.contains("8"), "batch value 8 should be present");
}

// ─── render_args: bool flags ──────────────────────────────────────────────────

#[test]
fn render_args_bool_true_is_bare_flag() {
    let mut args: HashMap<String, serde_json::Value> = HashMap::new();
    args.insert("--amp".to_string(), serde_json::json!(true));
    args.insert("--debug".to_string(), serde_json::json!(false));

    let rendered = render_args(&args);

    assert!(rendered.contains("--amp"), "true flag --amp should appear");
    assert!(
        !rendered.contains("--debug"),
        "false flag --debug should be omitted, got: '{}'",
        rendered
    );
    assert!(
        !rendered.contains("--amp true") && !rendered.contains("--amp false"),
        "bool true should render as bare flag, not '--amp true': got '{}'",
        rendered
    );
}

#[test]
fn render_args_empty_map_returns_empty_string() {
    let args: HashMap<String, serde_json::Value> = HashMap::new();
    assert_eq!(render_args(&args), "");
}

#[test]
fn render_args_string_value_appended() {
    let mut args: HashMap<String, serde_json::Value> = HashMap::new();
    args.insert("--model".to_string(), serde_json::json!("resnet50"));

    let rendered = render_args(&args);
    assert_eq!(rendered, "--model resnet50");
}

// ─── build_launch_command ────────────────────────────────────────────────────

#[test]
fn build_launch_command_includes_workdir_cmd_and_nohup() {
    use xrun_vast::execute::build_launch_command;

    let run_spec = RunSpec {
        workdir: Some("/workspace".to_string()),
        setup: None,
        cmd: Some("python train.py".to_string()),
        notebook: None,
        args: None,
    };

    let cmd = build_launch_command(&run_spec);
    assert!(cmd.contains("cd /workspace"), "should cd to workdir");
    assert!(cmd.contains("python train.py"), "should include cmd");
    assert!(cmd.contains("nohup"), "should run in background via nohup");
    assert!(cmd.contains("stdout.log"), "should tee to stdout.log");
}

#[test]
fn build_launch_command_renders_args_in_output() {
    use xrun_vast::execute::build_launch_command;

    let mut args = HashMap::new();
    args.insert("--epochs".to_string(), serde_json::json!(10_u64));
    args.insert("--amp".to_string(), serde_json::json!(true));

    let run_spec = RunSpec {
        workdir: None,
        setup: None,
        cmd: Some("python train.py".to_string()),
        notebook: None,
        args: Some(args),
    };

    let cmd = build_launch_command(&run_spec);
    assert!(cmd.contains("--amp"), "bool true flag should appear");
    assert!(cmd.contains("--epochs 10"), "numeric arg should appear");
}
