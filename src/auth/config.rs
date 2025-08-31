use serde::{Deserialize, Serialize};

fn default_secret_key() -> String {
    // generate a random 32-byte key
    let secret_key = rand::random::<[u8; 32]>();
    hex::encode(secret_key)
}

fn default_token_expiry() -> i64 {
    60 * 60 * 24 * 7 // 7 days
}

fn default_enable_argon() -> bool {
    false
}

fn default_argon_url() -> String {
    "https://argon.globed.dev".into()
}

fn default_argon_token() -> String {
    "".into()
}

fn default_argon_ping_interval() -> u64 {
    30
}

fn default_argon_disconnect_timeout() -> u64 {
    45
}

#[derive(Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_secret_key")]
    pub secret_key: String,
    #[serde(default = "default_token_expiry")]
    pub token_expiry: i64,
    #[serde(default = "default_enable_argon")]
    pub enable_argon: bool,
    #[serde(default = "default_argon_url")]
    pub argon_url: String,
    #[serde(default = "default_argon_token")]
    pub argon_token: String,
    #[serde(default = "default_argon_ping_interval")]
    pub argon_ping_interval: u64,
    #[serde(default = "default_argon_disconnect_timeout")]
    pub argon_disconnect_timeout: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            secret_key: default_secret_key(),
            token_expiry: default_token_expiry(),
            enable_argon: default_enable_argon(),
            argon_url: default_argon_url(),
            argon_token: default_argon_token(),
            argon_ping_interval: default_argon_ping_interval(),
            argon_disconnect_timeout: default_argon_disconnect_timeout(),
        }
    }
}
