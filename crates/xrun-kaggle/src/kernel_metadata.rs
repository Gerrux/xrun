#![deny(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Representation of `kernel-metadata.json` per Kaggle API docs.
/// See: https://github.com/Kaggle/kaggle-api/wiki/Kernel-Metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelMetadata {
    pub id: String,
    pub title: String,
    pub code_file: String,
    pub language: String,
    pub kernel_type: String,
    pub is_private: bool,
    pub enable_gpu: bool,
    pub enable_internet: bool,
    #[serde(default)]
    pub dataset_sources: Vec<String>,
    #[serde(default)]
    pub competition_sources: Vec<String>,
    #[serde(default)]
    pub kernel_sources: Vec<String>,
}

impl KernelMetadata {
    pub fn new_script(
        slug: &str,
        _title: &str,
        code_file: &str,
        enable_gpu: bool,
        enable_internet: bool,
        dataset_sources: Vec<String>,
    ) -> Self {
        let kernel_name = slug.split('/').last().unwrap_or(slug);
        Self {
            id: slug.to_string(),
            title: kernel_name.to_string(),
            code_file: code_file.to_string(),
            language: "python".to_string(),
            kernel_type: "script".to_string(),
            is_private: true,
            enable_gpu,
            enable_internet,
            dataset_sources,
            competition_sources: vec![],
            kernel_sources: vec![],
        }
    }

    pub fn new_notebook(
        slug: &str,
        _title: &str,
        code_file: &str,
        enable_gpu: bool,
        enable_internet: bool,
        dataset_sources: Vec<String>,
    ) -> Self {
        let kernel_name = slug.split('/').last().unwrap_or(slug);
        Self {
            id: slug.to_string(),
            title: kernel_name.to_string(),
            code_file: code_file.to_string(),
            language: "python".to_string(),
            kernel_type: "notebook".to_string(),
            is_private: true,
            enable_gpu,
            enable_internet,
            dataset_sources,
            competition_sources: vec![],
            kernel_sources: vec![],
        }
    }
}
