use serde::{Serialize, de::DeserializeOwned};

pub trait ServerModule: Send + Sync + 'static {
    type Config: DeserializeOwned + Serialize + Default + Send + Sync + 'static;

    fn new(config: &Self::Config) -> Result<Self, Box<dyn std::error::Error + Send + Sync>>
    where
        Self: Sized;

    /// Returns the ID of the module. This should be a kebab-case string,
    /// it will be used to identify the configuration file for the module, and other things.
    fn id() -> &'static str;

    /// Returns the name of the module. This should be a human-readable string.
    fn name() -> &'static str;
}
