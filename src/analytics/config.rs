use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Default)]
pub struct Config {
    /// URL of the clickhouse instance
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub database: String,
}
