use serde::{Deserialize, Serialize};

fn default_port() -> u16 {
    8080
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self { port: default_port() }
    }
}
