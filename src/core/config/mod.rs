use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};
use server_shared::config::env_replace;
use state::TypeMap;
use thiserror::Error;
use tracing::error;

use crate::core::module::ServerModule;

mod core;
pub use core::*;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Error parsing configuration: {0}")]
    Parse(#[from] toml::de::Error),
}

pub struct Config {
    core_config: CoreConfig,
    mod_config: TypeMap![Send + Sync],
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
            mod_config: <TypeMap![Send + Sync]>::new(),
            root_dir,
            core_config,
        })
    }

    pub fn freeze(&mut self) {
        self.mod_config.freeze();
    }

    pub fn module<T: ServerModule>(&self) -> &T::Config {
        self.mod_config.get::<T::Config>()
    }

    pub fn core(&self) -> &CoreConfig {
        &self.core_config
    }

    pub fn init_module<T: ServerModule>(&self) -> Result<(), ConfigError> {
        match self._init_module::<T>() {
            Ok(config) => {
                self.mod_config.set(config);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn _init_module<T: ServerModule>(&self) -> Result<T::Config, ConfigError> {
        Self::_init_from_path::<T::Config>(&self.root_dir, T::id())
    }

    fn _init_core(root_dir: &Path) -> Result<CoreConfig, ConfigError> {
        Self::_init_from_path::<CoreConfig>(root_dir, "core")
    }

    fn _init_from_path<T: Send + Sync + Default + DeserializeOwned + Serialize>(
        root_dir: &Path,
        name: &str,
    ) -> Result<T, ConfigError> {
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
            Ok(T::default())
        }
    }
}
