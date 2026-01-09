use serde::{Deserialize, Serialize};

fn default_credits_cache_timeout() -> u32 {
    86400 // 1 day
}

fn default_credits_req_interval() -> u32 {
    2
}

fn default_credits_categories() -> Vec<CreditsCategory> {
    vec![]
}

#[derive(Clone, Deserialize, Serialize)]
pub struct CreditsUser {
    pub id: i32,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct CreditsCategory {
    pub name: String,
    #[serde(default)]
    pub sync_with_role: Option<String>,
    #[serde(default)]
    pub users: Vec<CreditsUser>,

    /// Which account IDs to ignore and not send to the client (e.g. test / alt accounts)
    /// This option applies only to users synced using roles, not to manually specified users.
    #[serde(default)]
    pub ignored: Vec<i32>,
}

#[derive(Deserialize, Serialize)]
pub struct Config {
    /// How long credits cache lasts in seconds
    #[serde(default = "default_credits_cache_timeout")]
    pub credits_cache_timeout: u32,
    /// Interval of requests to gd server in seconds
    #[serde(default = "default_credits_req_interval")]
    pub credits_req_interval: u32,
    /// Credits categories
    #[serde(default = "default_credits_categories")]
    pub credits_categories: Vec<CreditsCategory>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            credits_cache_timeout: default_credits_cache_timeout(),
            credits_req_interval: default_credits_req_interval(),
            credits_categories: default_credits_categories(),
        }
    }
}
