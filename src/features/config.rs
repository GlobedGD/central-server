use std::path::PathBuf;

use serde::{Deserialize, Serialize};

fn default_database_url() -> String {
    "sqlite://features.sqlite?mode=rwc".into()
}

fn default_database_pool_size() -> u32 {
    5
}

fn default_feature_cycle_interval() -> u32 {
    60 * 60 * 24 // 1 day
}

#[derive(Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_database_url")]
    pub database_url: String,
    #[serde(default = "default_database_pool_size")]
    pub database_pool_size: u32,
    #[serde(default = "default_feature_cycle_interval")]
    pub feature_cycle_interval: u32,
    #[serde(default)]
    pub spreadsheet_id: Option<String>,
    #[serde(default)]
    pub google_credentials_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database_url: default_database_url(),
            database_pool_size: default_database_pool_size(),
            feature_cycle_interval: default_feature_cycle_interval(),
            spreadsheet_id: None,
            google_credentials_path: None,
        }
    }
}
