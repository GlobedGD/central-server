use serde::{Deserialize, Serialize};

fn default_address() -> String {
    "0.0.0.0".to_owned()
}

fn default_port() -> u16 {
    8080
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_address")]
    pub address: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            address: default_address(),
            port: default_port(),
        }
    }
}
