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
fn done_policy_anchors_ssh_patterns_at_run_workdir_only_when_set() {
    use xrun_core::manifest::DonePolicy;
    let ssh = |run_extra: &str| {
        Manifest::from_yaml_str(&format!(
            "name: s\nvendor: ssh\nssh:\n  host_alias: box\nrun:\n  cmd: python t.py\n{run_extra}\
             artifacts:\n  patterns: [\"ckpt/best*.pt\", \"/abs/x.log\"]\n"
        ))
        .unwrap()
    };
    let p = DonePolicy::from_manifest(&ssh("  workdir: /home/u/proj/\n"));
    assert_eq!(
        p.pull_patterns,
        ["/home/u/proj/ckpt/best*.pt", "/abs/x.log"]
    );
    assert_eq!(p.anchor_dir.as_deref(), Some("/home/u/proj"));
    assert_eq!(p.anchor("**/best*"), "/home/u/proj/**/best*");
    assert!(!p.kill_remote);

    let p = DonePolicy::from_manifest(&ssh(""));
    assert_eq!(p.pull_patterns, ["ckpt/best*.pt", "/abs/x.log"]);
    assert!(
        p.anchor_dir.is_none(),
        "default: the adapter anchors at run_dir"
    );

    // Relative / `~` workdir: the adapter `cd`s there from the remote home,
    // so anchor home-relative (`~/…`), never under the per-run dir.
    for wd in ["proj", "~/proj/"] {
        let p = DonePolicy::from_manifest(&ssh(&format!("  workdir: {wd}\n")));
        assert_eq!(
            p.pull_patterns,
            ["~/proj/ckpt/best*.pt", "/abs/x.log"],
            "{wd}"
        );
        assert_eq!(p.anchor("**/best*"), "~/proj/**/best*", "{wd}");
    }
}

#[test]
fn ssh_workdir_anchor_is_absolute_or_home_relative() {
    use xrun_core::manifest::ssh_workdir_anchor;
    assert_eq!(ssh_workdir_anchor(None), None);
    assert_eq!(ssh_workdir_anchor(Some("  ")), None);
    assert_eq!(
        ssh_workdir_anchor(Some("/srv/p/")).as_deref(),
        Some("/srv/p")
    );
    assert_eq!(ssh_workdir_anchor(Some("/")).as_deref(), Some("/"));
    assert_eq!(ssh_workdir_anchor(Some("p")).as_deref(), Some("~/p"));
    assert_eq!(ssh_workdir_anchor(Some("~/p")).as_deref(), Some("~/p"));
    assert_eq!(ssh_workdir_anchor(Some("~")).as_deref(), Some("~"));
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

fn manifest_with(vendor: &str, section: &str) -> String {
    format!("name: t\nvendor: {vendor}\n{section}run:\n  cmd: python train.py\n")
}

fn err_of(yaml: &str) -> String {
    Manifest::from_yaml_str(yaml).unwrap_err().to_string()
}

#[test]
fn lightning_minimal_without_section_ok() {
    let m = Manifest::from_yaml_str(&manifest_with("lightning", "")).unwrap();
    assert_eq!(m.vendor.as_str(), "lightning");
    assert!(m.lightning.is_none());
}

#[test]
fn lightning_full_roundtrips_through_yaml() {
    let m = Manifest::from_yaml_str(&manifest_with(
        "lightning",
        "lightning:\n  machine: T4\n  interruptible: true\n  studio: My-Studio\n  teamspace: owner/name\n  workdir: xrun\n  gpu: auto\n  max_runtime_secs: 10800\n",
    ))
    .unwrap();
    let l = m.lightning.as_ref().unwrap();
    assert_eq!(l.machine.as_deref(), Some("T4"));
    assert_eq!(l.max_runtime_secs, Some(10800));
    let back = Manifest::from_yaml_str(&serde_yaml::to_string(&m).unwrap()).unwrap();
    assert_eq!(m, back);
    assert_eq!(m.canonical_hash(), back.canonical_hash());
}

#[test]
fn lightning_unknown_field_rejected() {
    let err = err_of(&manifest_with("lightning", "lightning:\n  bogus: 1\n"));
    assert!(err.contains("bogus"), "{err}");
}

#[test]
fn lightning_workdir_must_be_relative() {
    let err = err_of(&manifest_with("lightning", "lightning:\n  workdir: /abs\n"));
    assert!(err.contains("lightning.workdir"), "{err}");
}

#[test]
fn lightning_teamspace_must_be_owner_slash_name() {
    for bad in ["solo", "a/b/c", "/name", "owner/"] {
        let err = err_of(&manifest_with(
            "lightning",
            &format!("lightning:\n  teamspace: \"{bad}\"\n"),
        ));
        assert!(err.contains("lightning.teamspace"), "{bad}: {err}");
    }
    Manifest::from_yaml_str(&manifest_with(
        "lightning",
        "lightning:\n  teamspace: owner/name\n",
    ))
    .unwrap();
}

#[test]
fn colab_run_workdir_must_be_absolute() {
    let rel = "name: t\nvendor: colab\nrun:\n  cmd: python train.py\n  workdir: proj\n";
    let err = err_of(rel);
    assert!(
        err.contains("vendor=colab: run.workdir must be absolute"),
        "{err}"
    );
    let abs = "name: t\nvendor: colab\nrun:\n  cmd: python train.py\n  workdir: /content/proj\n";
    Manifest::from_yaml_str(abs).unwrap();
}

#[test]
fn lightning_studio_name_validated() {
    for bad in ["has space", "under_score", "-lead", &"a".repeat(41)] {
        let err = err_of(&manifest_with(
            "lightning",
            &format!("lightning:\n  studio: \"{bad}\"\n"),
        ));
        assert!(err.contains("lightning.studio"), "{bad}: {err}");
    }
    for good in ["a", "xrun-my-exp1", &"a".repeat(40)] {
        Manifest::from_yaml_str(&manifest_with(
            "lightning",
            &format!("lightning:\n  studio: \"{good}\"\n"),
        ))
        .unwrap();
    }
}

#[test]
fn colab_minimal_and_full_ok() {
    Manifest::from_yaml_str(&manifest_with("colab", "")).unwrap();
    let m = Manifest::from_yaml_str(&manifest_with(
        "colab",
        "colab:\n  gpu: a100\n  high_mem: true\n  workdir: /content/xrun\n",
    ))
    .unwrap();
    let c = m.colab.as_ref().unwrap();
    assert_eq!(c.gpu.as_deref(), Some("a100"));
    let back = Manifest::from_yaml_str(&serde_yaml::to_string(&m).unwrap()).unwrap();
    assert_eq!(m, back);
}

#[test]
fn colab_unknown_field_rejected() {
    let err = err_of(&manifest_with("colab", "colab:\n  nope: 1\n"));
    assert!(err.contains("nope"), "{err}");
}

#[test]
fn colab_workdir_must_be_absolute() {
    let err = err_of(&manifest_with("colab", "colab:\n  workdir: rel/dir\n"));
    assert!(err.contains("colab.workdir"), "{err}");
}

#[test]
fn colab_gpu_allowed_set_case_insensitive() {
    for g in ["T4", "l4", "A100", "h100", "G4", "CPU"] {
        Manifest::from_yaml_str(&manifest_with("colab", &format!("colab:\n  gpu: {g}\n"))).unwrap();
    }
    let err = err_of(&manifest_with("colab", "colab:\n  gpu: V100\n"));
    assert!(err.contains("colab.gpu"), "{err}");
}

#[test]
fn foreign_sections_rejected_for_new_vendors() {
    let err = err_of(&manifest_with(
        "lightning",
        "ssh:\n  host_alias: box\ncolab: {}\n",
    ));
    assert!(err.contains("vendor=lightning must not have"), "{err}");
    let err = err_of(&manifest_with("colab", "lightning: {}\n"));
    assert!(
        err.contains("vendor=colab must not have a [lightning] section"),
        "{err}"
    );
}

#[test]
fn old_vendors_reject_lightning_and_colab_sections() {
    let vast = "vast:\n  image: i\n  gpu: {type: RTX_4090, count: 1}\n";
    let ssh = "ssh:\n  host_alias: box\n";
    let kaggle = "kaggle:\n  kernel_slug: u/k\n";
    for (vendor, base) in [
        ("vast", vast),
        ("kaggle", kaggle),
        ("local", ""),
        ("ssh", ssh),
    ] {
        for extra in ["lightning: {}\n", "colab: {}\n"] {
            let err = err_of(&manifest_with(vendor, &format!("{base}{extra}")));
            assert!(
                err.contains(&format!("vendor={vendor} must not have")),
                "{vendor}+{extra}: {err}"
            );
        }
    }
}

#[test]
fn vendor_from_str_and_all_cover_new_variants() {
    use std::str::FromStr;
    use xrun_core::manifest::Vendor;
    assert_eq!(Vendor::from_str("lightning").unwrap(), Vendor::Lightning);
    assert_eq!(Vendor::from_str("colab").unwrap(), Vendor::Colab);
    assert!(Vendor::all().contains(&Vendor::Lightning));
    assert!(Vendor::all().contains(&Vendor::Colab));
}

#[test]
fn lightning_data_dst_is_home_relative() {
    let ok = Manifest::from_yaml_str(&manifest_with(
        "lightning",
        "data:\n  - src: d.h5\n    dst: data/d.h5\n  - src: e.h5\n    dst: ~/data/e.h5\n",
    ));
    assert!(ok.is_ok(), "{:?}", ok.err());
    let err = err_of(&manifest_with(
        "lightning",
        "data:\n  - src: d.h5\n    dst: /abs/d.h5\n",
    ));
    assert!(err.contains("relative to the Studio home"), "{err}");
    // Colab keeps the absolute-dst rule of the other remote vendors.
    let err = err_of(&manifest_with(
        "colab",
        "data:\n  - src: d.h5\n    dst: data/d.h5\n",
    ));
    assert!(err.contains("must start with '/'"), "{err}");
}

#[test]
fn done_policy_lightning_colab_anchor_at_workdir_and_keep_kill_remote() {
    use xrun_core::manifest::DonePolicy;
    for vendor in ["lightning", "colab"] {
        let bare = Manifest::from_yaml_str(&format!(
            "name: s\nvendor: {vendor}\nrun:\n  cmd: python t.py\nartifacts:\n  patterns: [\"out/*\"]\n"
        ))
        .unwrap();
        let p = DonePolicy::from_manifest(&bare);
        assert!(
            p.kill_remote,
            "{vendor}: destroy must release the rented box"
        );
        assert_eq!(
            p.pull_patterns,
            ["out/*"],
            "{vendor}: adapter anchors at the run dir"
        );
        assert!(p.anchor_dir.is_none());

        // Colab requires an absolute workdir (validated), lightning a relative one.
        let wd = if vendor == "colab" {
            "/content/proj"
        } else {
            "proj"
        };
        let with_wd = Manifest::from_yaml_str(&format!(
            "name: s\nvendor: {vendor}\nrun:\n  cmd: python t.py\n  workdir: {wd}\n\
             artifacts:\n  patterns: [\"out/*\", \"/abs/x.log\"]\n"
        ))
        .unwrap();
        let p = DonePolicy::from_manifest(&with_wd);
        assert!(p.kill_remote);
        let anchor = if vendor == "colab" {
            "/content/proj"
        } else {
            "~/proj"
        };
        assert_eq!(p.anchor_dir.as_deref(), Some(anchor), "{vendor}");
        assert_eq!(
            p.pull_patterns,
            [format!("{anchor}/out/*"), "/abs/x.log".to_string()],
            "{vendor}"
        );
    }
}
