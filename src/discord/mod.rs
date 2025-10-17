use std::{sync::Arc, time::Duration};

use poise::serenity_prelude as serenity;
use qunet::server::ServerHandle;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tracing::error;

use crate::{
    core::{
        handler::ConnectionHandler,
        module::{ConfigurableModule, ModuleInitResult, ServerModule},
    },
    discord::{bot::DiscordBot, state::BotState},
};

pub use message::*;
pub use state::BotError;

mod bot;
mod commands;
mod event_handler;
mod message;
mod state;

pub struct DiscordUserData {
    pub id: u64,
    pub avatar_url: String,
    pub username: String,
}

impl DiscordUserData {
    pub fn from_discord(user: &serenity::User) -> Self {
        Self {
            id: user.id.get(),
            avatar_url: user.avatar_url().unwrap_or_default(),
            username: user.name.clone(),
        }
    }
}

pub struct DiscordModule {
    handle: JoinHandle<()>,
    state: Arc<BotState>,
    alert_channel: u64,
}

impl DiscordModule {
    pub async fn send_message(
        &self,
        channel_id: u64,
        msg: DiscordMessage<'_>,
    ) -> Result<(), BotError> {
        self.state.send_message(channel_id, msg).await
    }

    pub async fn send_alert(&self, msg: DiscordMessage<'_>) -> Result<(), BotError> {
        if self.alert_channel == 0 {
            return Ok(());
        }

        self.state.send_message(self.alert_channel, msg).await
    }

    pub async fn get_user_data(&self, account_id: u64) -> Result<DiscordUserData, BotError> {
        self.state.get_user_data(account_id).await
    }

    pub fn finish_link_attempt(&self, gd_account: i32, id: u64, accepted: bool) {
        self.state.finish_link_attempt(gd_account, id, accepted)
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
    #[cfg(feature = "discord")]
    #[serde(default)]
    pub alert_channel: u64,
}

impl ServerModule for DiscordModule {
    async fn new(config: &Config, _handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        let state = Arc::new(BotState::new());

        let mut bot = DiscordBot::new(&config.token, state.clone()).await?;

        let handle = tokio::spawn(async move {
            if let Err(e) = bot.start().await {
                error!("Failed to start discord bot: {e}");
            }
        });

        Ok(Self {
            handle,
            state,
            alert_channel: config.alert_channel,
        })
    }

    fn id() -> &'static str {
        "discord"
    }

    fn name() -> &'static str {
        "Discord"
    }

    fn on_launch(&self, server: &ServerHandle<ConnectionHandler>) {
        self.state.set_server(server);

        server.schedule(Duration::from_hours(1), async |server| {
            server.handler().module::<Self>().state.cleanup_link_attempts();
        });
    }
}

impl ConfigurableModule for DiscordModule {
    type Config = Config;
}

pub const fn hex_color_to_decimal(color: &'static str) -> u32 {
    if color.as_bytes().first() == Some(&b'#') {
        return hex_color_to_decimal(&color[1..]);
    }

    u32::from_str_radix(color, 16).unwrap_or_default()
}
