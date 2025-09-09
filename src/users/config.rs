use serde::{Deserialize, Serialize};

use server_shared::MultiColor;

fn default_database_url() -> String {
    "sqlite://db.sqlite?mode=rwc".into()
}

fn default_database_pool_size() -> u32 {
    5
}

fn default_roles() -> Vec<Role> {
    vec![]
}

fn default_super_admins() -> Vec<i32> {
    vec![]
}

fn default_script_sign_key() -> String {
    // generate a random 32-byte key
    let secret_key = rand::random::<[u8; 32]>();
    hex::encode(secret_key)
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Role {
    pub id: String,
    pub priority: i32,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub name_color: MultiColor,

    #[serde(default)]
    pub can_kick: Option<bool>,
    #[serde(default)]
    pub can_mute: Option<bool>,
    #[serde(default)]
    pub can_ban: Option<bool>,
    #[serde(default)]
    pub can_set_password: Option<bool>,
    #[serde(default)]
    pub can_notice_everyone: Option<bool>,
}

#[derive(Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_database_url")]
    pub database_url: String,
    #[serde(default = "default_database_pool_size")]
    pub database_pool_size: u32,
    #[serde(default = "default_roles")]
    pub roles: Vec<Role>,
    #[serde(default = "default_super_admins")]
    pub super_admins: Vec<i32>,
    #[serde(default = "default_script_sign_key")]
    pub script_sign_key: String,

    /// Where logs are sent on Discord, requires `discord` feature and module to be enabled.
    #[serde(default)]
    pub mod_log_channel: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database_url: default_database_url(),
            database_pool_size: default_database_pool_size(),
            roles: default_roles(),
            super_admins: default_super_admins(),
            script_sign_key: default_script_sign_key(),
            mod_log_channel: Default::default(),
        }
    }
}
