use std::{collections::HashSet, sync::Arc, time::Duration};

use parking_lot::Mutex;
use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};
use server_shared::qunet::server::ServerHandle;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

#[cfg(feature = "web")]
use {
    crate::web::{WebModule, WebState},
    axum::{
        extract::{Query, State},
        response::{Html, IntoResponse, Redirect},
    },
    tracing::debug,
};

use crate::{
    core::{
        handler::{ClientStateHandle, ConnectionHandler},
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
    ticket_ping_channel: u64,
    sent_alerts: Mutex<HashSet<i32>>,
}

impl DiscordModule {
    pub fn send_message(&self, channel_id: u64, msg: DiscordMessage<'_>) {
        let msg = msg.into_owned();
        let state = self.state.clone();

        tokio::spawn(async move {
            match state.send_message(channel_id, msg).await {
                Ok(()) => {}
                Err(e) => warn!("Failed to send message ({channel_id}): {e}"),
            }
        });
    }

    pub fn send_alert(&self, msg: DiscordMessage<'_>) {
        if self.alert_channel == 0 {
            return;
        }

        self.send_message(self.alert_channel, msg)
    }

    pub fn send_ticket_ping(&self, ticket_channel: u64, moderator_id: u64) {
        if self.ticket_ping_channel == 0 {
            return;
        }

        info!("Sending ticket ping for channel {ticket_channel} to moderator {moderator_id}");

        self.send_message(
            self.ticket_ping_channel,
            DiscordMessage::new()
                .content(format!("<@{moderator_id}> New ticket to handle: <#{ticket_channel}>")),
        )
    }

    pub fn send_username_alert(&self, username: &str, id: i32) {
        // don't repeat alerts
        let new_alert = self.sent_alerts.lock().insert(id);

        if new_alert {
            self.send_alert(
                DiscordMessage::new()
                    .content(format!("⚠️ Potentially bad username: {username} ({id})")),
            );
        }
    }

    pub async fn get_user_data(&self, account_id: u64) -> Result<DiscordUserData, BotError> {
        self.state.get_user_data(account_id).await
    }

    pub fn finish_link_attempt(&self, gd_account: i32, id: u64, accepted: bool) {
        self.state.finish_link_attempt(gd_account, id, accepted)
    }

    /// Begins oauth2 flow and returns a URL that the user must open
    pub fn begin_oauth_flow(&self, client: &ClientStateHandle, gd_account: i32) -> String {
        self.state.begin_oauth_flow(Arc::downgrade(client), gd_account)
    }

    pub fn finish_oauth_flow(&self, code: String, state: String) -> anyhow::Result<()> {
        self.state.finish_oauth_flow(code, state)
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

#[derive(Deserialize, Serialize, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct OauthOptions {
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub redirect_uri: String,
}

#[derive(Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub main_guild_id: u64,
    #[serde(default)]
    pub alert_channel: u64,
    #[serde(default)]
    pub ticket_ping_channel: u64,
    #[serde(default)]
    pub oauth: OauthOptions,
}

impl ServerModule for DiscordModule {
    async fn new(config: &Config, handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        let state = Arc::new(BotState::new(handler.http_client(), config));

        let mut bot = DiscordBot::new(&config.token, state.clone()).await?;

        let handle = tokio::spawn(async move {
            if let Err(e) = bot.start().await {
                error!("Failed to start discord bot: {e}");
            }
        });

        #[cfg(feature = "web")]
        {
            let web = handler.module::<WebModule>();
            web.add_route("/discord-oauth/callback", axum::routing::get(oauth_handler)).await;
            web.add_route("/discord-oauth/success", axum::routing::get(oauth_success_handler))
                .await;
            web.add_route("/discord-oauth/failure", axum::routing::get(oauth_failure_handler))
                .await;
        }

        Ok(Self {
            handle,
            state,
            alert_channel: config.alert_channel,
            ticket_ping_channel: config.ticket_ping_channel,
            sent_alerts: Mutex::new(HashSet::new()),
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
            server.handler().module::<Self>().state.cleanup();
        });

        server.schedule(Duration::from_hours(24), async |server| {
            if let Err(e) = server.handler().module::<Self>().state.slow_sync_all().await {
                error!("Failed to run Discord sync-all: {e}");
            }
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

#[derive(Deserialize)]
struct OauthQuery {
    code: String,
    state: String,
}

#[cfg(feature = "web")]
async fn oauth_handler(
    Query(OauthQuery { code, state }): Query<OauthQuery>,
    State(wstate): State<Arc<WebState>>,
) -> impl IntoResponse {
    debug!("Received OAuth callback with code {code}");
    let server = wstate.server();
    let module = server.handler().module::<DiscordModule>();

    match module.finish_oauth_flow(code, state) {
        Ok(()) => Redirect::to("/discord-oauth/success").into_response(),
        Err(e) => {
            error!("Failed to finish OAuth flow: {e}");
            Redirect::to("/discord-oauth/failure").into_response()
        }
    }
}

#[cfg(feature = "web")]
async fn oauth_success_handler() -> impl IntoResponse {
    Html(include_str!("oauth_success.html")).into_response()
}

#[cfg(feature = "web")]
async fn oauth_failure_handler() -> impl IntoResponse {
    Html(include_str!("oauth_failure.html")).into_response()
}
