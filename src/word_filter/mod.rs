use std::{path::PathBuf, sync::Arc, time::Duration};

use async_watcher::{AsyncDebouncer, notify::RecursiveMode};
use filter::WordFilter;
use serde::{Deserialize, Serialize};
use server_shared::qunet::server::ServerHandle;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::core::{
    handler::ConnectionHandler,
    module::{ConfigurableModule, ModuleInitResult, ServerModule},
};

mod filter;

pub struct WordFilterModule {
    path: PathBuf,
    watch: bool,
    filter: RwLock<Option<WordFilter>>,
}

impl ServerModule for WordFilterModule {
    async fn new(config: Arc<Config>, _handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        let path = config.file_path.clone().unwrap_or_else(|| "config/word-filter.txt".into());

        let filter = if path.exists() {
            let filter =
                WordFilter::new_from_path(&path).await.expect("Failed to create word filter");
            info!("Loaded word filter with {} words", filter.word_count());
            Some(filter)
        } else {
            None
        };

        Ok(Self {
            path,
            watch: config.watch,
            filter: RwLock::new(filter),
        })
    }

    fn id() -> &'static str {
        "word-filter"
    }

    fn name() -> &'static str {
        "Word Filter"
    }

    fn on_launch(&self, server: &ServerHandle<ConnectionHandler>) {
        // watch the word filter file for changes
        let wpath = self.path.clone();
        if !wpath.exists() || !self.watch {
            // don't watch :)
            return;
        }

        let this = server.handler().opt_module_owned::<Self>().unwrap();

        tokio::spawn(async move {
            let (mut debouncer, mut file_events) = AsyncDebouncer::new_with_channel(
                Duration::from_secs(1),
                Some(Duration::from_secs(1)),
            )
            .await
            .expect("Failed to create debouncer");

            if let Err(e) = debouncer.watcher().watch(&wpath, RecursiveMode::NonRecursive) {
                warn!("Failed to watch the word filter file ({wpath:?}): {e}");
                return;
            }

            while let Some(event) = file_events.recv().await {
                debug!("received file event: {event:?}");
                if let Some(filter) = &mut *this.filter.write().await {
                    match filter.reload_from_file(&wpath).await {
                        Ok(()) => {
                            info!(
                                "Successfully reloaded the word filter! Total words: {}",
                                filter.word_count()
                            );
                        }

                        Err(e) => {
                            warn!("Failed to reload the word filter: {e}");
                        }
                    }
                }
            }
        });
    }
}

impl ConfigurableModule for WordFilterModule {
    type Config = Config;
}

impl WordFilterModule {
    pub async fn is_allowed(&self, content: &str) -> bool {
        self.filter.read().await.as_ref().is_none_or(|wf| !wf.is_bad(content))
    }
}

#[derive(Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    file_path: Option<PathBuf>,
    #[serde(default)]
    watch: bool,
}
