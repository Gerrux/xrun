#![deny(unsafe_code)]

pub mod hash;
pub mod types;
pub mod validate;

pub use types::ssh_workdir_anchor;
pub use types::{
    anchor_vast_pattern, ckpt_to_remote_pattern, Artifacts, CheckpointPull, Checkpoints,
    DataCompress, DataMode, DataSource, DonePolicy, EarlyStop, EarlyStopMode, GpuSpec, KaggleSpec,
    KeepBest, LocalSpec, Manifest, MlflowSpec, Policy, PriceSpec, Requires, RunSpec, SshSpec,
    UnpackSpec, VastSpec, Vendor,
};
pub use validate::validate;

use crate::error::ManifestError;

impl Manifest {
    pub fn from_yaml_str(s: &str) -> Result<Manifest, ManifestError> {
        let manifest: Manifest = serde_yaml::from_str(s)?;
        validate(&manifest)?;
        Ok(manifest)
    }
}
