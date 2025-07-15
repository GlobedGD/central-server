use serde::{Deserialize, Serialize};

fn default_secret_key() -> String {
    // generate a random 32-byte key
    let secret_key = rand::random::<[u8; 32]>();
    hex::encode(secret_key)
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

#[derive(Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_secret_key")]
    pub secret_key: String,
    #[serde(default = "default_enable_argon")]
    pub enable_argon: bool,
    #[serde(default = "default_argon_url")]
    pub argon_url: String,
    #[serde(default = "default_argon_token")]
    pub argon_token: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            secret_key: default_secret_key(),
            enable_argon: default_enable_argon(),
            argon_url: default_argon_url(),
            argon_token: default_argon_token(),
        }
    }
}
