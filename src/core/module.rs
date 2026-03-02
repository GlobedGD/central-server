use serde::{Serialize, de::DeserializeOwned};
use server_shared::qunet::server::ServerHandle;

use crate::core::handler::ConnectionHandler;

pub type ModuleInitResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub trait ServerModule: Send + Sync + 'static {
    fn new(
        config: &Self::Config,
        handler: &ConnectionHandler,
    ) -> impl Future<Output = ModuleInitResult<Self>> + Send
    where
        Self: Sized + ConfigurableModule;

    /// Returns the ID of the module. This should be a kebab-case string,
    /// it will be used to identify the configuration file for the module, and other things.
    fn id() -> &'static str
    where
        Self: Sized;

    /// Returns the name of the module. This should be a human-readable string.
    fn name() -> &'static str
    where
        Self: Sized;

    fn on_launch(&self, _server: &ServerHandle<ConnectionHandler>) {}
}

pub trait ConfigurableModule {
    type Config: Send + Sync + Default + DeserializeOwned + Serialize + 'static;
}
