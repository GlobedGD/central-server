use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};
use server_shared::{TypeMap, config::env_replace};
use thiserror::Error;
use tracing::error;

trait ConfigTrait: Send + Sync + Default + DeserializeOwned + Serialize + 'static {}

impl<T> ConfigTrait for T where T: Send + Sync + Default + DeserializeOwned + Serialize + 'static {}

mod core;
pub use core::*;

use crate::core::module::{ConfigurableModule, ServerModule};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Error parsing configuration: {0}")]
    Parse(#[from] toml::de::Error),
}

pub struct Config {
    core_config: CoreConfig,
    mod_config: TypeMap,
    root_dir: PathBuf,
}

impl Config {
    pub fn new() -> Result<Self, ConfigError> {
        let mut root_dir =
            std::env::current_dir().expect("Failed to get current directory").join("config");

        env_replace("GLOBED_ROOT_CONFIG_DIR", &mut root_dir);

        if !root_dir.exists() {
            std::fs::create_dir_all(&root_dir).expect("Failed to create config directory");
        }

        Self::new_with_root_dir(root_dir)
    }

    pub fn new_with_root_dir(root_dir: PathBuf) -> Result<Self, ConfigError> {
        let mut core_config = Self::_init_core(&root_dir)?;
        core_config.replace_with_env();

        Ok(Self {
            mod_config: TypeMap::new(),
            root_dir,
            core_config,
        })
    }

    pub fn freeze(&mut self) {
        self.mod_config.freeze();
    }

    pub fn module<T: ConfigurableModule>(&self) -> &T::Config {
        self.custom::<T::Config>()
    }

    pub fn custom<T: DeserializeOwned + Send + Sync + 'static>(&self) -> &T {
        self.mod_config.get::<T>().expect("config not initialized for module")
    }

    pub fn core(&self) -> &CoreConfig {
        &self.core_config
    }

    pub fn init_module<T: ServerModule + ConfigurableModule>(&self) -> Result<(), ConfigError> {
        self.init_custom::<T::Config>(T::id())
    }

    fn init_custom<T: ConfigTrait>(&self, id: &str) -> Result<(), ConfigError> {
        let config = Self::_init_from_path::<T>(&self.root_dir, id)?;
        self.mod_config.insert(config);
        Ok(())
    }

    fn _init_core(root_dir: &Path) -> Result<CoreConfig, ConfigError> {
        Self::_init_from_path::<CoreConfig>(root_dir, "core")
    }

    fn _init_from_path<T: ConfigTrait>(root_dir: &Path, name: &str) -> Result<T, ConfigError> {
        let path = root_dir.join(format!("{name}.toml"));

        if path.exists() {
            let data = std::fs::read_to_string(&path)?;
            let config: T = toml::from_str(&data)?;
            Ok(config)
        } else {
            let config = T::default();
            std::fs::write(
                &path,
                toml::to_string_pretty(&config).expect("config serialization failed"),
            )?;
            Ok(config)
        }
    }
}
