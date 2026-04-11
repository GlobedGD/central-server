use std::{path::PathBuf, sync::Arc};

use arc_swap::ArcSwap;
use filter::WordFilter;
use serde::{Deserialize, Serialize};
use server_shared::qunet::server::ServerHandle;
use tracing::{info, warn};

use crate::core::{
    handler::ConnectionHandler,
    module::{ConfigurableModule, ModuleInitResult, ServerModule},
};

mod filter;

pub struct WordFilterModule {
    filter: ArcSwap<Option<WordFilter>>,
}

impl ServerModule for WordFilterModule {
    async fn new(config: Arc<Config>, _handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        let this = Self {
            filter: ArcSwap::new(Arc::new(None)),
        };

        this.do_reload(&config).await;

        Ok(this)
    }

    fn id() -> &'static str {
        "word-filter"
    }

    fn name() -> &'static str {
        "Word Filter"
    }

    fn reload(&self, server: &ServerHandle<ConnectionHandler>, config: Arc<Config>) {
        let this = server.handler().opt_module_owned::<Self>().unwrap();

        tokio::spawn(async move {
            this.do_reload(&config).await;
        });
    }
}

impl ConfigurableModule for WordFilterModule {
    type Config = Config;
}

impl WordFilterModule {
    pub async fn is_allowed(&self, content: &str) -> bool {
        let filter = self.filter.load();

        (**filter).as_ref().is_none_or(|wf| !wf.is_bad(content))
    }

    pub async fn do_reload(&self, config: &Config) {
        let path = config.file_path.clone().unwrap_or_else(|| "config/word-filter.txt".into());

        let filter = if path.exists() {
            let filter =
                WordFilter::new_from_path(&path).await.expect("Failed to create word filter");
            info!("Loaded word filter from {path:?} with {} words", filter.word_count());
            Some(filter)
        } else {
            if config.file_path.is_some() {
                warn!(
                    "Failed to load the word filter from {:?}, file does not exist",
                    config.file_path
                );
            }

            None
        };

        self.filter.store(Arc::new(filter));
    }
}

#[derive(Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    file_path: Option<PathBuf>,
    /// Now unused
    #[serde(default)]
    watch: bool,
}
