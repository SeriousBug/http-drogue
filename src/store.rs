use cuttlestore::Cuttlestore;
use serde::{Deserialize, Serialize};

pub type DownloadProgressStore = Cuttlestore<Progress>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub target_file: Option<String>,
    pub failed: bool,
    pub url: String,
    pub progress: u64,
    pub total: Option<u64>,
    /// Bytes per second.
    pub speed: f64,
}

impl Progress {
    pub fn default_with(url: String) -> Self {
        Progress {
            target_file: None,
            failed: false,
            url,
            progress: 0,
            total: None,
            speed: 0f64,
        }
    }
}
