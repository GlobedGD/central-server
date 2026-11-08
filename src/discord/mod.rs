use std::{
    collections::HashSet,
    fmt::Write,
    sync::Arc,
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use plotters::style::FontStyle;
use poise::serenity_prelude::{self as serenity, ButtonStyle, CreateActionRow, CreateButton};
use serde::{Deserialize, Serialize};
use server_shared::{UsernameString, qunet::server::ServerHandle};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::{
    core::{
        handler::ConnectionHandler,
        module::{ConfigurableModule, ModuleInitResult, ServerModule},
    },
    discord::{
        bot::DiscordBot,
        state::{AltAlertInteraction, BotState, NameAlertInteraction},
    },
    users::{DiscordUserData, UsersModule},
};

pub use message::*;
pub use state::BotError;

pub const ROBOTO_TTF: &[u8] = include_bytes!("Roboto-Regular.ttf");

mod bot;
mod commands;
mod event_handler;
mod message;
mod state;

pub struct DiscordModule {
    config: ArcSwap<Config>,
    handle: JoinHandle<()>,
    state: Arc<BotState>,
    sent_alerts: Mutex<HashSet<i32>>,
}

impl DiscordModule {
    pub fn send_message(&self, channel_id: u64, msg: DiscordMessage<'_>) {
        let msg = msg.into_owned();
        let state = self.state.clone();

        tokio::spawn(async move {
            match state.send_message(channel_id, msg).await {
                Ok(_) => {}
                Err(e) => warn!("Failed to send message ({channel_id}): {e}"),
            }
        });
    }

    pub fn send_alert(&self, msg: DiscordMessage<'_>) {
        let config = self.config.load();
        if config.alert_channel == 0 {
            return;
        }

        self.send_message(config.alert_channel, msg)
    }

    pub fn send_server_alert(&self, msg: DiscordMessage<'_>) {
        let config = self.config.load();
        if config.server_alert_channel == 0 {
            return;
        }

        self.send_message(config.server_alert_channel, msg)
    }

    pub fn send_ticket_ping(&self, ticket_channel: u64, moderator_id: u64) {
        let config = self.config.load();
        if config.ticket_ping_channel == 0 {
            return;
        }

        info!("Sending ticket ping for channel {ticket_channel} to moderator {moderator_id}");

        let msg = if moderator_id != 0 {
            format!("<@{moderator_id}> New ticket to handle: <#{ticket_channel}>")
        } else {
            format!("New ticket to handle: <#{ticket_channel}>")
        };

        self.send_message(config.ticket_ping_channel, DiscordMessage::new().content(msg))
    }

    pub async fn send_alt_alert(
        &self,
        username: &str,
        account_id: i32,
        alts: &[i32],
        uident: &str,
        users: &UsersModule,
    ) {
        let config = self.config.load();
        if config.alert_channel == 0 {
            return;
        }

        let mut alert_str = format!(
            "⚠️ Potential alt account logged in: {} ({}), uident: {}. Other (suspected) accounts:\n",
            username,
            account_id,
            &uident[..8]
        );

        for acc_id in alts {
            match users.get_user(*acc_id).await.ok().flatten() {
                Some(user) => {
                    let mut pun_strs = Vec::new();
                    if user.is_banned() {
                        pun_strs.push("banned");
                    }
                    if user.is_muted() {
                        pun_strs.push("muted");
                    }
                    if user.is_room_banned() {
                        pun_strs.push("room banned");
                    }

                    writeln!(
                        alert_str,
                        "- {} ({}){}",
                        user.username(),
                        acc_id,
                        if pun_strs.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", pun_strs.join(", "))
                        }
                    )
                    .unwrap();
                }

                None => {
                    writeln!(alert_str, "- {acc_id} (failed to fetch details)").unwrap();
                }
            }
        }

        alert_str.push_str("\n**Note: this can be a false positive. Only ban after confirming all accounts are actual alts.**");

        // interactions handled in event_handler.rs
        let buttons = vec![
            CreateButton::new("altalrt_ban").style(ButtonStyle::Danger).label("Ban all accounts"),
            CreateButton::new("altalrt_dismiss").style(ButtonStyle::Secondary).label("Dismiss"),
            CreateButton::new("altalrt_keep").style(ButtonStyle::Secondary).label("Keep alert"),
            CreateButton::new("altalrt_wl").style(ButtonStyle::Success).label("Whitelist uident"),
        ];

        let msg = DiscordMessage::new()
            .content(alert_str)
            .add_component(CreateActionRow::Buttons(buttons))
            .into_owned();

        let state = self.state.clone();
        let channel = config.alert_channel;

        let mut alts = alts.to_vec();
        if !alts.contains(&account_id) {
            alts.push(account_id);
        }

        tokio::spawn(async move {
            let msg = match state.send_message(channel, msg).await {
                Ok(msg) => msg,
                Err(e) => {
                    warn!("Failed to send alt alert message: {e}");
                    return;
                }
            };

            state.add_pending_alt_alert_interaction(AltAlertInteraction {
                message_id: msg.id.get(),
                accounts: alts,
                began_at: Instant::now(),
            })
        });
    }

    pub fn send_username_alert(&self, username: &str, id: i32, bad_term: &str) {
        // don't repeat alerts
        let config = self.config.load();
        let new_alert = self.sent_alerts.lock().insert(id);

        if !new_alert || config.alert_channel == 0 {
            return;
        }

        // interactions handled in event_handler.rs
        let buttons = vec![
            CreateButton::new("ualert_ban_user").style(ButtonStyle::Danger).label("Ban user"),
            CreateButton::new("ualert_dismiss").style(ButtonStyle::Secondary).label("Dismiss"),
        ];

        let msg = DiscordMessage::new()
            .content(format!(
                "⚠️ Potentially bad username: {username} ({id}), contains term '{bad_term}'"
            ))
            .add_component(CreateActionRow::Buttons(buttons))
            .into_owned();

        let state = self.state.clone();
        let channel = config.alert_channel;
        let username = UsernameString::try_from(username).unwrap();

        tokio::spawn(async move {
            let msg = match state.send_message(channel, msg).await {
                Ok(msg) => msg,
                Err(e) => {
                    warn!("Failed to send username alert message: {e}");
                    return;
                }
            };

            state.add_pending_username_alert_interaction(NameAlertInteraction {
                message_id: msg.id.get(),
                account_id: id,
                username,
                began_at: Instant::now(),
            })
        });
    }

    pub async fn get_user_data(&self, account_id: u64) -> Result<DiscordUserData, BotError> {
        self.state.get_user_data(account_id).await
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
    pub server_alert_channel: u64,
    #[serde(default)]
    pub ticket_ping_channel: u64,
}

impl ServerModule for DiscordModule {
    async fn new(config: Arc<Config>, _handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        // laod the roboto font
        let _ = tokio::task::spawn_blocking(|| {
            if plotters::style::register_font("sans-serif", FontStyle::Normal, ROBOTO_TTF).is_err()
            {
                warn!("Failed to load Roboto font");
            }
        })
        .await;

        let state = Arc::new(BotState::new(config.clone()));

        let mut bot = DiscordBot::new(&config.token, state.clone()).await?;

        let handle = tokio::spawn(async move {
            if let Err(e) = bot.start().await {
                error!("Failed to start discord bot: {e}");
            }
        });

        Ok(Self {
            handle,
            state,
            config: ArcSwap::new(config),
            sent_alerts: Mutex::new(HashSet::new()),
        })
    }

    fn reload(&self, _server: &ServerHandle<ConnectionHandler>, config: Arc<Config>) {
        self.config.store(config.clone());
        self.state.reload_config(config);
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
            let module = server.handler().module::<Self>();

            if let Err(e) = module.state.slow_sync_all().await {
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
