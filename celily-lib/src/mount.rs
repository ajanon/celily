use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Mount {
    pub source: PathBuf,
    pub target: PathBuf,
    #[serde(default)]
    pub readwrite: bool,
}
