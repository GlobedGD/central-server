use serde::{Deserialize, Serialize};

fn default_database_url() -> String {
    "sqlite://db.sqlite?mode=rwc".into()
}

fn default_database_pool_size() -> u32 {
    5
}

#[derive(Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_database_url")]
    pub database_url: String,
    #[serde(default = "default_database_pool_size")]
    pub database_pool_size: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database_url: default_database_url(),
            database_pool_size: default_database_pool_size(),
        }
    }
}
