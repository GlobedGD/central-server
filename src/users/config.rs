use serde::{Deserialize, Serialize};

fn default_database_url() -> String {
    "sqlite://db.sqlite?mode=rwc".into()
}

fn default_database_pool_size() -> u32 {
    5
}

fn default_roles() -> Vec<Role> {
    vec![]
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Role {
    pub id: String,
    pub priority: i32,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub name_color: String,

    #[serde(default)]
    pub can_kick: Option<bool>,
    #[serde(default)]
    pub can_mute: Option<bool>,
    #[serde(default)]
    pub can_ban: Option<bool>,
}

#[derive(Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_database_url")]
    pub database_url: String,
    #[serde(default = "default_database_pool_size")]
    pub database_pool_size: u32,
    #[serde(default = "default_roles")]
    pub roles: Vec<Role>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database_url: default_database_url(),
            database_pool_size: default_database_pool_size(),
            roles: default_roles(),
        }
    }
}
