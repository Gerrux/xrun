use xrun_core::vendor::VendorAdapter;
use xrun_core::{Manifest, VendorError};
use xrun_vast::VastStub;

const VAST_FULL: &str = include_str!("../../xrun-core/tests/data/vast_full.yaml");

#[test]
fn dry_run_plan_vast_full_cmd_line() {
    let manifest = Manifest::from_yaml_str(VAST_FULL).unwrap();
    let stub = VastStub::new();
    let plan = stub.dry_run_plan(&manifest).unwrap();

    // Args sorted alphabetically: --batch-size, --cache, --dropout, --epochs,
    // --in-channels, --lr, --output
    let expected = concat!(
        "python train_v5_multichannel.py",
        " --batch-size 8",
        " --cache /workspace/data/cache",
        " --dropout 0.2",
        " --epochs 30",
        " --in-channels 2",
        " --lr 0.0001",
        " --output /workspace/run/output",
    );
    assert_eq!(plan.cmd_line, expected);
    assert!(plan.cmd_line.contains("--cache /workspace/data/cache"));
    assert!(plan.cmd_line.contains("--output /workspace/run/output"));
}

#[test]
fn dry_run_plan_gpu_query() {
    let manifest = Manifest::from_yaml_str(VAST_FULL).unwrap();
    let stub = VastStub::new();
    let plan = stub.dry_run_plan(&manifest).unwrap();

    assert!(plan.gpu_query.contains("RTX 4090"));
    assert!(plan.gpu_query.contains("x1"));
    assert_eq!(plan.estimated_price_max, 0.55);
}

#[test]
fn validate_rejects_empty_image() {
    let yaml = r#"
name: test-run
vendor: vast
vast:
  image: ""
  gpu:
    type: "RTX 4090"
    count: 1
run: {}
"#;
    // Core validate passes (does not check image emptiness).
    // VastStub.validate should reject empty image.
    let manifest = Manifest::from_yaml_str(yaml).unwrap();
    let stub = VastStub::new();
    let err = stub.validate(&manifest).unwrap_err();
    match err {
        VendorError::Validation(msg) => assert!(msg.contains("image"), "unexpected msg: {msg}"),
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[test]
fn validate_rejects_empty_gpu_type() {
    let yaml = r#"
name: test-run
vendor: vast
vast:
  image: "pytorch/pytorch:2.4.1"
  gpu:
    type: ""
    count: 1
run: {}
"#;
    let manifest = Manifest::from_yaml_str(yaml).unwrap();
    let stub = VastStub::new();
    let err = stub.validate(&manifest).unwrap_err();
    match err {
        VendorError::Validation(msg) => {
            assert!(
                msg.contains("gpu") || msg.contains("type"),
                "unexpected msg: {msg}"
            )
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[test]
fn provision_returns_not_implemented() {
    let manifest = Manifest::from_yaml_str(VAST_FULL).unwrap();
    let stub = VastStub::new();
    let err = stub.provision(&manifest).unwrap_err();
    assert!(matches!(err, VendorError::NotImplemented));
}

#[test]
fn dry_run_plan_no_args() {
    let yaml = r#"
name: min-run
vendor: vast
vast:
  image: "pytorch/pytorch:2.4.1"
  gpu:
    type: "RTX 3090"
    count: 1
run:
  cmd: python train.py
"#;
    let manifest = Manifest::from_yaml_str(yaml).unwrap();
    let stub = VastStub::new();
    let plan = stub.dry_run_plan(&manifest).unwrap();
    assert_eq!(plan.cmd_line, "python train.py");
    assert!(plan.gpu_query.contains("RTX 3090"));
    assert_eq!(plan.estimated_price_max, 0.0);
}
