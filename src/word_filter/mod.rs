use std::{path::PathBuf, time::Duration};

use async_watcher::{AsyncDebouncer, notify::RecursiveMode};
use filter::WordFilter;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use server_shared::qunet::server::ServerHandle;
use tracing::{info, warn};

use crate::core::{
    handler::ConnectionHandler,
    module::{ConfigurableModule, ModuleInitResult, ServerModule},
};

mod filter;

pub struct WordFilterModule {
    path: PathBuf,
    filter: Mutex<Option<WordFilter>>,
}

impl ServerModule for WordFilterModule {
    async fn new(config: &Config, _handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        let path = config.file_path.clone().unwrap_or_else(|| "config/word-filter.txt".into());

        let filter = if path.exists() {
            let filter = WordFilter::new_from_path(&path).expect("Failed to create word filter");
            info!("Loaded word filter with {} words", filter.word_count());
            Some(filter)
        } else {
            None
        };

        Ok(Self {
            path,
            filter: Mutex::new(filter),
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
        if !wpath.exists() {
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

            while let Some(_event) = file_events.recv().await {
                if let Some(filter) = &mut *this.filter.lock() {
                    match filter.reload_from_file(&wpath) {
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
    pub fn is_allowed(&self, content: &str) -> bool {
        self.filter.lock().as_ref().is_none_or(|wf| !wf.is_bad(content))
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    file_path: Option<PathBuf>,
}
