use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tracing::error;

use crate::{
    core::{
        handler::ConnectionHandler,
        module::{ModuleInitResult, ServerModule},
    },
    discord::{bot::DiscordBot, state::BotState},
};

pub use message::*;
pub use state::BotError;

mod bot;
mod event_handler;
mod message;
mod state;

pub struct DiscordModule {
    handle: JoinHandle<()>,
    state: Arc<BotState>,
}

impl DiscordModule {
    pub async fn send_message(
        &self,
        channel_id: u64,
        msg: DiscordMessage<'_>,
    ) -> Result<(), BotError> {
        self.state.send_message(channel_id, msg).await
    }
}

impl Drop for DiscordModule {
    fn drop(&mut self) {
        let state = self.state.clone();

        tokio::task::block_in_place(move || {
            state.reset_ctx();
        });

        self.handle.abort();
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub token: String,
}

impl ServerModule for DiscordModule {
    type Config = Config;

    async fn new(config: &Self::Config, _handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        let state = Arc::new(BotState::new());

        let mut bot = DiscordBot::new(&config.token, state.clone()).await?;

        let handle = tokio::spawn(async move {
            if let Err(e) = bot.start().await {
                error!("Failed to start discord bot: {e}");
            }
        });

        Ok(Self { handle, state })
    }

    fn id() -> &'static str {
        "discord"
    }

    fn name() -> &'static str {
        "Discord"
    }
}
