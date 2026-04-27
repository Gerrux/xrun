#![deny(unsafe_code)]

pub mod hash;
pub mod types;
pub mod validate;

pub use types::{
    Artifacts, CheckpointPull, Checkpoints, DataMode, DataSource, GpuSpec, KaggleSpec, KeepBest,
    Manifest, MlflowSpec, Policy, PriceSpec, RunSpec, UnpackSpec, VastSpec, Vendor,
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
