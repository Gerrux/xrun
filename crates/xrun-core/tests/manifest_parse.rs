use xrun_core::manifest::{validate, Manifest};

const VAST_FULL_HASH: &str = "a9e2782cc1262b81b25b381ab91e07f968041c3dd36bd3afa194644917cd7e9b";

#[test]
fn parse_vast_minimal_ok() {
    let yaml = include_str!("data/vast_minimal.yaml");
    let manifest = Manifest::from_yaml_str(yaml).unwrap();
    assert_eq!(manifest.name, "test-minimal");
}

#[test]
fn parse_vast_full_ok() {
    let yaml = include_str!("data/vast_full.yaml");
    let manifest = Manifest::from_yaml_str(yaml).unwrap();
    assert_eq!(manifest.name, "arborust-v7-c");
    let vast = manifest.vast.as_ref().unwrap();
    assert_eq!(vast.gpu.count, 1);
    assert_eq!(vast.gpu.gpu_type, "RTX 4090");
}

#[test]
fn parse_kaggle_minimal_ok() {
    let yaml = include_str!("data/kaggle_minimal.yaml");
    let manifest = Manifest::from_yaml_str(yaml).unwrap();
    assert_eq!(manifest.name, "classifier-eb0-baseline");
    let kaggle = manifest.kaggle.as_ref().unwrap();
    assert_eq!(kaggle.kernel_slug, "gerrux/classifier-eb0-baseline");
}

#[test]
fn validate_vast_without_vast_section_fails() {
    let yaml = r#"
name: my-run
vendor: vast
run:
  cmd: python train.py
"#;
    let manifest: Manifest = serde_yaml::from_str(yaml).unwrap();
    let err = validate(&manifest).unwrap_err();
    assert!(err.to_string().contains("requires a [vast] section"));
}

#[test]
fn validate_kaggle_without_kaggle_section_fails() {
    let yaml = r#"
name: my-run
vendor: kaggle
run:
  cmd: python train.py
"#;
    let manifest: Manifest = serde_yaml::from_str(yaml).unwrap();
    let err = validate(&manifest).unwrap_err();
    assert!(err.to_string().contains("requires a [kaggle] section"));
}

#[test]
fn validate_invalid_name_fails() {
    let yaml = r#"
name: My Invalid Name
vendor: vast
vast:
  image: pytorch/pytorch:2.4.1-cuda12.1-cudnn9-devel
  gpu:
    type: "RTX 4090"
    count: 1
run:
  cmd: python train.py
"#;
    let err = Manifest::from_yaml_str(yaml).unwrap_err();
    assert!(err.to_string().contains("name must match"));
}

#[test]
fn validate_data_dst_not_slash_fails() {
    let yaml = r#"
name: my-run
vendor: vast
vast:
  image: pytorch/pytorch:2.4.1-cuda12.1-cudnn9-devel
  gpu:
    type: "RTX 4090"
    count: 1
data:
  - src: /local/file.tar
    dst: relative/path
run:
  cmd: python train.py
"#;
    let err = Manifest::from_yaml_str(yaml).unwrap_err();
    assert!(err.to_string().contains("must start with '/'"));
}

#[test]
fn validate_args_key_with_space_fails() {
    let yaml = r#"
name: my-run
vendor: vast
vast:
  image: pytorch/pytorch:2.4.1-cuda12.1-cudnn9-devel
  gpu:
    type: "RTX 4090"
    count: 1
run:
  cmd: python train.py
  args:
    "bad key": value
"#;
    let err = Manifest::from_yaml_str(yaml).unwrap_err();
    assert!(err.to_string().contains("must not contain spaces"));
}

#[test]
fn hash_key_order_independent() {
    let yaml_a = r#"
name: test-minimal
vendor: vast
vast:
  image: pytorch/pytorch:2.4.1-cuda12.1-cudnn9-devel
  gpu:
    type: "RTX 4090"
    count: 1
run:
  cmd: python train.py
"#;
    let yaml_b = r#"
vendor: vast
run:
  cmd: python train.py
name: test-minimal
vast:
  gpu:
    count: 1
    type: "RTX 4090"
  image: pytorch/pytorch:2.4.1-cuda12.1-cudnn9-devel
"#;
    let manifest_a = Manifest::from_yaml_str(yaml_a).unwrap();
    let manifest_b = Manifest::from_yaml_str(yaml_b).unwrap();
    assert_eq!(manifest_a.canonical_hash(), manifest_b.canonical_hash());
}

#[test]
fn hash_stability_snapshot() {
    let yaml = include_str!("data/vast_full.yaml");
    let manifest = Manifest::from_yaml_str(yaml).unwrap();
    let actual = manifest.canonical_hash();
    assert_eq!(
        actual, VAST_FULL_HASH,
        "hash changed — if intentional, update VAST_FULL_HASH to: {actual}"
    );
}

fn local_manifest(extra: &str) -> String {
    format!("name: my-run\nvendor: local\nrun:\n  cmd: python train.py\n{extra}")
}

fn ssh_manifest(extra: &str) -> String {
    format!(
        "name: my-run\nvendor: ssh\nssh:\n  host_alias: box\nrun:\n  cmd: python train.py\n{extra}"
    )
}

#[test]
fn on_done_and_pull_on_accept_documented_values() {
    use xrun_core::manifest::DonePolicy;
    let m = Manifest::from_yaml_str(&ssh_manifest(
        "policy:\n  on_done: keep\nartifacts:\n  patterns: [\"ckpt/*.pt\"]\n  pull_on: done\n",
    ))
    .unwrap();
    let p = DonePolicy::from_manifest(&m);
    assert!(!p.stop_instance);
    assert_eq!(p.pull_patterns, vec!["ckpt/*.pt".to_string()]);
}

#[test]
fn done_policy_defaults_stop_instance_and_pull_when_patterns_set() {
    use xrun_core::manifest::DonePolicy;
    let bare = Manifest::from_yaml_str(&ssh_manifest("")).unwrap();
    let p = DonePolicy::from_manifest(&bare);
    assert!(p.stop_instance && p.pull_patterns.is_empty() && !p.explicit_on_done);
    let with =
        Manifest::from_yaml_str(&ssh_manifest("artifacts:\n  patterns: [\"a\", \"b\"]\n")).unwrap();
    let p = DonePolicy::from_manifest(&with);
    assert!(p.stop_instance);
    assert_eq!(p.pull_patterns.len(), 2);
}

#[test]
fn done_policy_anchors_vast_relative_patterns_at_workdir() {
    use xrun_core::manifest::DonePolicy;
    let yaml = |workdir: &str| {
        format!(
            "{}\n{workdir}artifacts:\n  patterns: [\"checkpoints/best*.pt\", \"/workspace/run/stdout.log\"]\n",
            include_str!("data/vast_minimal.yaml").trim_end()
        )
    };
    let default_wd = DonePolicy::from_manifest(&Manifest::from_yaml_str(&yaml("")).unwrap());
    assert_eq!(
        default_wd.pull_patterns,
        [
            "/workspace/checkpoints/best*.pt",
            "/workspace/run/stdout.log"
        ]
    );
    let custom = DonePolicy::from_manifest(
        &Manifest::from_yaml_str(&yaml("  workdir: /root/proj/\n")).unwrap(),
    );
    assert_eq!(
        custom.pull_patterns,
        [
            "/root/proj/checkpoints/best*.pt",
            "/workspace/run/stdout.log"
        ]
    );
}

#[test]
fn done_policy_pulls_kaggle_output_once() {
    use xrun_core::manifest::DonePolicy;
    // Fixture lists two patterns; Kaggle downloads the whole output once.
    let m = Manifest::from_yaml_str(include_str!("data/kaggle_minimal.yaml")).unwrap();
    assert_eq!(DonePolicy::from_manifest(&m).pull_patterns, ["**/*"]);
}

#[test]
fn done_policy_per_vendor_guard_anchor_and_kill() {
    use xrun_core::manifest::DonePolicy;
    let vast = Manifest::from_yaml_str(include_str!("data/vast_minimal.yaml")).unwrap();
    let p = DonePolicy::from_manifest(&vast);
    assert!(p.kill_remote && p.stop_instance && !p.explicit_on_done);
    assert_eq!(p.anchor_dir.as_deref(), Some("/workspace"));
    assert_eq!(p.guard_pattern.as_deref(), Some("/workspace/**/best*"));
    assert_eq!(p.anchor("x/*.pt"), "/workspace/x/*.pt");

    let local = Manifest::from_yaml_str(&local_manifest(
        "policy:\n  on_done: stop_instance\nartifacts:\n  patterns: [\"a\"]\n",
    ))
    .unwrap();
    let p = DonePolicy::from_manifest(&local);
    assert!(!p.kill_remote && p.explicit_on_done);
    assert!(p.pull_patterns.is_empty(), "local files are already local");
    assert!(p.guard_pattern.is_none());

    let ssh = Manifest::from_yaml_str(
        "name: s\nvendor: ssh\nssh:\n  host_alias: box\nrun:\n  cmd: python t.py\n\
         artifacts:\n  patterns: [\"out/*\"]\n",
    )
    .unwrap();
    let p = DonePolicy::from_manifest(&ssh);
    assert!(!p.kill_remote);
    assert_eq!(p.pull_patterns, ["out/*"], "ssh adapter anchors itself");
    assert!(p.guard_pattern.is_none());
}

#[test]
fn on_stage_failed_accepts_documented_values_and_rejects_others() {
    for v in ["stop_instance", "keep", "reprovision"] {
        Manifest::from_yaml_str(&local_manifest(&format!(
            "policy:\n  on_stage_failed: {v}\n"
        )))
        .unwrap_or_else(|e| panic!("{v}: {e}"));
    }
    let err = Manifest::from_yaml_str(&local_manifest("policy:\n  on_stage_failed: ignore\n"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("policy.on_stage_failed"), "{err}");
    assert!(err.contains("stop_instance | keep | reprovision"), "{err}");
}

#[test]
fn on_done_rejects_unknown_value() {
    let err = Manifest::from_yaml_str(&local_manifest("policy:\n  on_done: destroy\n"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("policy.on_done"), "{err}");
    assert!(err.contains("stop_instance | keep"), "{err}");
}

#[test]
fn pull_on_rejects_unknown_value() {
    let err = Manifest::from_yaml_str(&local_manifest(
        "artifacts:\n  patterns: [\"a\"]\n  pull_on: epoch\n",
    ))
    .unwrap_err()
    .to_string();
    assert!(err.contains("artifacts.pull_on"), "{err}");
    assert!(err.contains("done"), "{err}");
}
