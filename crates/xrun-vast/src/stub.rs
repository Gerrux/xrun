#![deny(unsafe_code)]

use std::path::{Path, PathBuf};

use xrun_core::{
    error::VendorError,
    manifest::{validate as core_validate, DataSource, Manifest, RunSpec},
    vendor::{DryRunPlan, InstanceHandle, VendorAdapter},
};

pub struct VastStub;

impl VastStub {
    pub fn new() -> Self {
        Self
    }
}

impl Default for VastStub {
    fn default() -> Self {
        Self::new()
    }
}

impl VendorAdapter for VastStub {
    fn name(&self) -> &'static str {
        "vast"
    }

    fn validate(&self, manifest: &Manifest) -> Result<(), VendorError> {
        core_validate(manifest)?;
        let vast = manifest
            .vast
            .as_ref()
            .ok_or_else(|| VendorError::Validation("vast section required".to_string()))?;
        if vast.image.is_empty() {
            return Err(VendorError::Validation(
                "vast.image must not be empty".to_string(),
            ));
        }
        if vast.gpu.gpu_type.is_empty() {
            return Err(VendorError::Validation(
                "vast.gpu.type must not be empty".to_string(),
            ));
        }
        Ok(())
    }

    fn dry_run_plan(&self, manifest: &Manifest) -> Result<DryRunPlan, VendorError> {
        self.validate(manifest)?;
        let vast = manifest
            .vast
            .as_ref()
            .ok_or_else(|| VendorError::Validation("vast section required".to_string()))?;

        let mut gpu_query = format!("{} x{}", vast.gpu.gpu_type, vast.gpu.count);
        if let Some(vram) = vast.gpu.vram_min_gb {
            gpu_query = format!("{gpu_query} vram>={vram}GB");
        }

        let estimated_price_max = vast.price.as_ref().map(|p| p.max_per_hour).unwrap_or(0.0);

        let mut data_items: Vec<(PathBuf, String)> = Vec::new();
        let mut data_total_bytes: u64 = 0;
        if let Some(data) = &manifest.data {
            for source in data {
                let src = PathBuf::from(&source.src);
                let bytes = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
                data_total_bytes += bytes;
                data_items.push((src, source.dst.clone()));
            }
        }

        let cmd_base = manifest.run.cmd.as_deref().unwrap_or("");
        let cmd_line = build_cmd_line(cmd_base, &manifest.run);

        Ok(DryRunPlan {
            gpu_query,
            estimated_price_max,
            data_total_bytes,
            data_items,
            cmd_line,
        })
    }

    fn provision(&self, _manifest: &Manifest) -> Result<InstanceHandle, VendorError> {
        Err(VendorError::NotImplemented)
    }

    fn upload(&self, _h: &InstanceHandle, _sources: &[DataSource]) -> Result<(), VendorError> {
        Err(VendorError::NotImplemented)
    }

    fn execute(&self, _h: &InstanceHandle, _run_spec: &RunSpec) -> Result<(), VendorError> {
        Err(VendorError::NotImplemented)
    }

    fn tail(&self, _h: &InstanceHandle, _file: &str, _offset: u64) -> Result<Vec<u8>, VendorError> {
        Err(VendorError::NotImplemented)
    }

    fn pull(&self, _h: &InstanceHandle, _remote: &str, _into: &Path) -> Result<(), VendorError> {
        Err(VendorError::NotImplemented)
    }

    fn destroy(&self, _h: &InstanceHandle) -> Result<(), VendorError> {
        Err(VendorError::NotImplemented)
    }
}

fn build_cmd_line(cmd_base: &str, run_spec: &RunSpec) -> String {
    let Some(args) = &run_spec.args else {
        return cmd_base.to_string();
    };

    let mut sorted: Vec<_> = args.iter().collect();
    sorted.sort_by_key(|(k, _)| k.as_str());

    let parts: Vec<String> = sorted
        .into_iter()
        .map(|(k, v)| {
            let val = v
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| v.to_string());
            format!("{k} {val}")
        })
        .collect();

    if parts.is_empty() {
        cmd_base.to_string()
    } else {
        format!("{cmd_base} {}", parts.join(" "))
    }
}
