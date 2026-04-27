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
