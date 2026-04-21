use std::{
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use super::serenity::{self, ChannelId, Context, CreateMessage, UserId};
use anyhow::{anyhow, bail};
use arc_swap::ArcSwap;
use dashmap::DashMap;
use poise::serenity_prelude::{GetMessages, GuildChannel, GuildId, Member, RoleId, User};
use serde::{Deserialize, de::DeserializeOwned};
use server_shared::qunet::server::{ServerHandle, WeakServerHandle};
use thiserror::Error;
use tokio::{
    sync::{RwLock, oneshot},
    time::MissedTickBehavior,
};
use tracing::{debug, error, info, warn};

use crate::{
    core::handler::{ClientStateHandle, ConnectionHandler, WeakClientStateHandle},
    discord::{DiscordMessage, DiscordModule, DiscordUserData, commands::util::ParseDurationError},
    users::{DatabaseError, DbUser, Error as UsersError, UsersModule},
};

struct LinkAttempt {
    started_at: Instant,
    gd_account: i32,
    tx: oneshot::Sender<bool>,
}

impl LinkAttempt {
    pub fn new(tx: oneshot::Sender<bool>, gd_account: i32) -> Self {
        Self {
            started_at: Instant::now(),
            gd_account,
            tx,
        }
    }
}

struct OauthAttempt {
    started_at: Instant,
    client: WeakClientStateHandle,
    secret: u64,
}

impl OauthAttempt {
    pub fn new(client: WeakClientStateHandle, secret: u64) -> Self {
        Self {
            started_at: Instant::now(),
            client,
            secret,
        }
    }
}

pub struct DiscordMemberData {
    #[allow(unused)]
    id: UserId,
    username: String,
    roles: Vec<RoleId>,
}

impl DiscordMemberData {
    pub fn from_discord(m: &Member) -> Self {
        Self {
            id: m.user.id,
            username: m.user.name.clone(),
            roles: m.roles.clone(),
        }
    }
}

pub struct BotState {
    config: ArcSwap<super::Config>,
    ctx: RwLock<Option<Context>>,
    server: OnceLock<WeakServerHandle<ConnectionHandler>>,
    http_client: reqwest::Client,

    link_attempts: DashMap<u64, LinkAttempt>,
    oauth_attempts: DashMap<i32, OauthAttempt>,
}

#[derive(Error, Debug)]
pub enum BotError {
    #[error("Bot context not yet available")]
    NoContext,
    #[error("Invalid channel ID given")]
    InvalidChannel,
    #[error("No permission")]
    NoPermission,
    #[error("Invalid duration: {0}")]
    InvalidDuration(#[from] ParseDurationError),
    #[error("{0}")]
    Serenity(#[from] Box<serenity::Error>),
    #[error("Database error: {0}")]
    Database(#[from] DatabaseError),
    #[error("{0}")]
    Custom(String),
}

impl From<serenity::Error> for BotError {
    fn from(e: serenity::Error) -> Self {
        BotError::Serenity(Box::new(e))
    }
}

impl From<UsersError> for BotError {
    fn from(e: UsersError) -> Self {
        match e {
            UsersError::Database(e) => BotError::Database(e),
            _ => BotError::custom(e.to_string()),
        }
    }
}

impl BotError {
    pub fn custom(s: impl Into<String>) -> Self {
        Self::Custom(s.into())
    }
}

impl BotState {
    pub fn new(http_client: reqwest::Client, config: Arc<super::Config>) -> Self {
        Self {
            config: ArcSwap::new(config),
            ctx: RwLock::new(None),
            server: OnceLock::new(),
            http_client,
            link_attempts: DashMap::new(),
            oauth_attempts: DashMap::new(),
        }
    }

    pub fn main_guild_id(&self) -> u64 {
        self.config.load().main_guild_id
    }

    pub fn reload_config(&self, config: Arc<super::Config>) {
        self.config.store(config);
    }

    pub fn reset_ctx(&self) {
        *self.ctx.blocking_write() = None;
    }

    pub async fn set_ctx(&self, ctx: Context) {
        *self.ctx.write().await = Some(ctx);
    }

    pub fn set_server(&self, handle: &ServerHandle<ConnectionHandler>) {
        let _ = self.server.set(handle.make_weak());
    }

    pub fn server(&self) -> Result<ServerHandle<ConnectionHandler>, BotError> {
        self.server
            .get()
            .and_then(|x| x.upgrade())
            .ok_or_else(|| BotError::custom("Server handle not initialized"))
    }

    pub fn get_from_server(handle: &ServerHandle<ConnectionHandler>) -> Arc<Self> {
        handle.handler().module::<DiscordModule>().state.clone()
    }

    pub fn create_link_attempt(&self, id: u64, gd_account: i32) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.link_attempts.insert(id, LinkAttempt::new(tx, gd_account));

        rx
    }

    pub fn has_link_attempt(&self, id: u64) -> bool {
        self.link_attempts.contains_key(&id)
    }

    pub fn finish_link_attempt(&self, gd_account: i32, id: u64, accepted: bool) {
        if let Some((_, la)) = self.link_attempts.remove(&id) {
            if la.gd_account != gd_account {
                // id mismatch
                debug!(
                    "ID mismatch when finishing link attempt: expected {}, got {}",
                    la.gd_account, gd_account
                );

                self.link_attempts.insert(id, la);
            } else {
                let _ = la.tx.send(accepted);
            }
        }
    }

    pub fn remove_link_attempt(&self, id: u64) {
        self.link_attempts.remove(&id);
    }

    pub fn begin_oauth_flow(&self, client: WeakClientStateHandle, gd_account: i32) -> String {
        let secret = rand::random::<u64>();
        self.oauth_attempts.insert(gd_account, OauthAttempt::new(client, secret));

        let config = self.config.load();
        format!(
            "https://discord.com/api/oauth2/authorize?client_id={}&redirect_uri={}&response_type=code&scope=identify&state={}-{}",
            config.oauth.client_id, config.oauth.redirect_uri, gd_account, secret
        )
    }

    pub fn finish_oauth_flow(&self, code: String, state: String) -> anyhow::Result<()> {
        let Some((id_str, secret_str)) = state.split_once('-') else {
            bail!("invalid OAuth state: '{state}'");
        };

        let id = id_str.parse::<i32>()?;
        let secret = secret_str.parse::<u64>()?;

        if let Some((_, attempt)) = self.oauth_attempts.remove(&id) {
            if attempt.secret == secret {
                // valid OAuth flow
                debug!("Finished OAuth flow for user {id}");

                let server = self.server().unwrap();
                tokio::spawn(async move {
                    let Some(client) = attempt.client.upgrade() else {
                        return;
                    };

                    if let Err(e) = Self::finish_oauth_link(server, client, code).await {
                        warn!("Failed to finish OAuth for user {id}: {e}");
                    }
                });
            } else {
                bail!("invalid OAuth state: secret mismatch for user {id}");
            }
        } else {
            bail!("Received OAuth state for unknown user {id}");
        }

        Ok(())
    }

    async fn finish_oauth_link(
        server: ServerHandle<ConnectionHandler>,
        client: ClientStateHandle,
        code: String,
    ) -> anyhow::Result<()> {
        let this = server.handler().module::<DiscordModule>().state.clone();
        let config = this.config.load();

        let response = json_request::<DiscordOAuthAuthorizeResponse>(
            this.http_client.post("https://discord.com/api/v10/oauth2/token").form(&[
                ("client_id", config.oauth.client_id.as_str()),
                ("client_secret", config.oauth.client_secret.as_str()),
                ("grant_type", "authorization_code"),
                ("redirect_uri", config.oauth.redirect_uri.as_str()),
                ("code", code.as_str()),
            ]),
            "oauth2/token",
        )
        .await?;

        let response = json_request::<DiscordOAuthUserResponse>(
            this.http_client
                .get("https://discord.com/api/v10/users/@me")
                .header("Authorization", format!("Bearer {}", response.access_token).as_str()),
            "users/@me",
        )
        .await?;

        let user_id = response
            .id
            .parse::<u64>()
            .map_err(|e| anyhow!("failed to parse discord user id: {e}"))?;

        info!("Received Discord OAuth data for user {}", response.id);
        let users = server.handler().module::<UsersModule>();

        users.link_discord_account_online(&client, user_id).await?;
        this.sync_user_roles_by_id(user_id).await?;

        Ok(())
    }

    /// Sync all linked users' roles. This will be slow and block for a while.
    pub async fn slow_sync_all(&self) -> anyhow::Result<()> {
        let users = self.server()?.handler().module::<UsersModule>().get_all_linked_users().await?;

        info!("Beginning to sync roles for all ({}) linked users", users.len());

        // limit to 5 requests per second
        let mut interval = tokio::time::interval(Duration::from_millis(200));
        interval.set_missed_tick_behavior(MissedTickBehavior::Burst);

        for user in users {
            interval.tick().await;

            let discord_id = user.discord_id.expect("returned user didn't have discord id");

            match self.get_member_data(discord_id.get()).await {
                Ok(Some(u)) => {
                    // sync their roles
                    if let Err(e) = self.sync_user_roles_for_dbuser(&u, &user).await {
                        warn!("Failed to sync roles for {} ({}): {e}", discord_id, user.account_id);
                    }
                }

                Ok(None) => {
                    // remove all roles that require linking, user left the server
                    info!(
                        "Clearing roles for user {} ({}), they left the server",
                        user.account_id, discord_id
                    );
                    self.clear_roles_for_dbuser(&user).await?;
                    continue;
                }

                Err(e) => {
                    warn!("failed to fetch discord user {discord_id}: {e}");
                    // don't do anything if we fail to fetch the user
                    continue;
                }
            };
        }

        Ok(())
    }

    pub async fn with_ctx<R, E: Into<BotError>>(
        &self,
        f: impl AsyncFnOnce(&Context) -> Result<R, E>,
    ) -> Result<R, BotError> {
        let ctx = self.ctx.read().await;

        match &*ctx {
            None => Err(BotError::NoContext),
            Some(ctx) => f(ctx).await.map_err(Into::into),
        }
    }

    pub async fn send_message(
        &self,
        channel_id: u64,
        msg: DiscordMessage<'_>,
    ) -> Result<(), BotError> {
        if channel_id == 0 {
            return Err(BotError::InvalidChannel);
        }

        let channel = ChannelId::new(channel_id);

        if msg.embeds.is_empty() {
            self.with_ctx(async |c| channel.say(c, msg.content.unwrap_or_default()).await).await?;
            return Ok(());
        }

        let mut message = CreateMessage::new();

        if let Some(c) = msg.content {
            message = message.content(c);
        }

        message = message.embeds(msg.embeds);

        self.with_ctx(async |c| channel.send_message(c, message).await).await?;

        Ok(())
    }

    pub async fn get_user_data(&self, id: u64) -> Result<DiscordUserData, BotError> {
        let id = UserId::new(id);

        self.with_ctx::<_, BotError>(async |c| {
            if let Some(user) = c.cache.user(id) {
                return Ok(DiscordUserData::from_discord(&user));
            }

            let user = c.http.get_user(id).await?;
            Ok(DiscordUserData::from_discord(&user))
        })
        .await
    }

    /// Note: panics if self.main_guild_id == 0
    pub async fn get_member_data(&self, id: u64) -> Result<Option<DiscordMemberData>, BotError> {
        let id = UserId::new(id);
        let guild_id = GuildId::new(self.main_guild_id());

        self.with_ctx::<_, BotError>(async |c| {
            // scope has to exist here to satisfy Sync of the cache ref
            {
                let cached_guild = c.cache.guild(guild_id);
                let cached = cached_guild.as_ref().and_then(|r| r.members.get(&id));

                if let Some(c) = cached {
                    return Ok(Some(DiscordMemberData::from_discord(c)));
                }
            }

            match c.http.get_member(guild_id, id).await {
                Ok(member) => Ok(Some(DiscordMemberData::from_discord(&member))),

                Err(serenity::Error::Http(he))
                    if he.status_code().is_some_and(|c| c.as_u16() == 404) =>
                {
                    Ok(None)
                }

                Err(e) => Err(e.into()),
            }
        })
        .await
    }

    pub(super) async fn on_member_updated(
        &self,
        old: Option<&Member>,
        new: &Member,
    ) -> Result<(), BotError> {
        if old.is_some_and(|o| o.roles == new.roles) {
            return Ok(());
        }

        // ignore errors
        let _ = self.sync_user_roles(new).await;

        Ok(())
    }

    pub(super) async fn on_member_leave(&self, user: &User) -> Result<(), BotError> {
        // ignore errors
        let _ = self.sync_user_roles_by_id(user.id.get()).await;

        Ok(())
    }

    // wow this function sucks
    pub(super) async fn on_ticket_channel_created(
        &self,
        channel: GuildChannel,
    ) -> Result<(), BotError> {
        let server = self.server()?;
        tokio::spawn(async move {
            let this = Self::get_from_server(&server);

            info!("Detected new ticket channel: {} ({})", channel.name(), channel.id);

            let mut iters = 0;
            let message = loop {
                let result = this
                    .with_ctx(async |ctx| channel.messages(&ctx.http, GetMessages::new()).await)
                    .await;

                match result {
                    Ok(mut messages) if !messages.is_empty() => {
                        break Some(messages.remove(0));
                    }

                    Ok(_) => {
                        info!("No messages available in channel {} yet...", channel.id);
                    }

                    Err(e) => {
                        warn!("Failed to fetch messages for channel {}: {e}", channel.id);
                    }
                }

                iters += 1;
                if iters >= 100 {
                    warn!("Giving up on channel {} after 10 attempts", channel.id);
                    break None;
                }

                tokio::time::sleep(Duration::from_secs(5)).await;
            };

            let Some(message) = message else {
                return;
            };

            let mut username = String::new();
            for embed in message.embeds {
                let Some(desc) = embed.description else {
                    continue;
                };

                for [question, answer] in desc.split('\n').array_chunks::<2>() {
                    if question.contains("in-game name") {
                        username = answer.trim_matches('`').trim().to_owned();
                    }
                }
            }

            let users = server.handler().module::<UsersModule>();

            let Some(user) = users.query_user(&username).await.ok().flatten() else {
                warn!(
                    "Failed to find user with username '{}' for channel {}",
                    username, channel.id
                );
                return;
            };

            let Some(pun_id) =
                user.active_ban.or(user.active_mute).or(user.active_room_ban).map(|p| p.id)
            else {
                warn!(
                    "User {} ({}) doesn't have an active punishment, skipping lookup for channel {}",
                    username, user.account_id, channel.id
                );
                return;
            };

            let Some(pun) = users.get_punishment(pun_id).await.ok().flatten() else {
                error!(
                    cid = channel.id.get(),
                    "Punishment lookup failed for punishment id {pun_id}"
                );
                return;
            };

            let Some(issuer) = users.get_user(pun.issued_by).await.ok().flatten() else {
                error!(
                    cid = channel.id.get(),
                    "Punishment issuer lookup failed for user id {}", pun.issued_by
                );
                return;
            };

            let issuer_discord = issuer.discord_id.map_or(0, |d| d.get());

            let discord = server.handler().module::<DiscordModule>();
            discord.send_ticket_ping(channel.id.get(), issuer_discord);
        });

        Ok(())
    }

    async fn dbuser_from_discord_id(&self, discord_id: u64) -> Result<DbUser, BotError> {
        let server = self.server()?;
        let users = server.handler().module::<UsersModule>();

        let Some(db_user) = users.get_linked_discord_inverse(discord_id).await? else {
            return Err(BotError::custom("User is not linked to any GD account"));
        };

        Ok(db_user)
    }

    pub(super) async fn sync_user_roles(&self, user: &Member) -> Result<Vec<String>, BotError> {
        let db_user = self.dbuser_from_discord_id(user.user.id.get()).await?;
        self.sync_user_roles_for_dbuser(&DiscordMemberData::from_discord(user), &db_user).await
    }

    async fn sync_user_roles_by_id(&self, discord_id: u64) -> Result<Vec<String>, BotError> {
        let db_user = self.dbuser_from_discord_id(discord_id).await?;

        if let Some(member) = self.get_member_data(discord_id).await? {
            self.sync_user_roles_for_dbuser(&member, &db_user).await
        } else {
            self.clear_roles_for_dbuser(&db_user).await?;
            Ok(vec![])
        }
    }

    async fn sync_user_roles_for_dbuser(
        &self,
        user: &DiscordMemberData,
        db_user: &DbUser,
    ) -> Result<Vec<String>, BotError> {
        let server = self.server().unwrap();
        let users = server.handler().module::<UsersModule>();

        let mut new_roles = Vec::new();
        let mut new_roles_idx = Vec::new();

        for role in &user.roles {
            if let Some(role_id) = users.get_role_id_by_discord_id(role.get())
                && let Some(role) = users.get_role(role_id)
            {
                new_roles.push(role.id.clone());
                new_roles_idx.push(role_id);
            }
        }

        info!("Syncing roles for {} ({}): {:?}", user.username, db_user.account_id, new_roles);

        users.system_set_roles(db_user.account_id, &new_roles_idx).await?;
        Ok(new_roles)
    }

    /// Clears all roles that require linking
    async fn clear_roles_for_dbuser(&self, db_user: &DbUser) -> Result<(), BotError> {
        // TODO: for now we just remove all roles
        let server = self.server().unwrap();
        let users = server.handler().module::<UsersModule>();

        info!("Syncing roles for {} ({}): <none>", db_user.username(), db_user.account_id);

        users.system_set_roles(db_user.account_id, &[]).await?;
        Ok(())
    }

    fn cleanup_link_attempts(&self) {
        self.link_attempts.retain(|_, la| la.started_at.elapsed() < Duration::from_mins(1));
    }

    fn cleanup_oauth_flows(&self) {
        self.oauth_attempts.retain(|_, oa| oa.started_at.elapsed() < Duration::from_mins(10));
    }

    pub fn cleanup(&self) {
        self.cleanup_link_attempts();
        self.cleanup_oauth_flows();
    }
}

#[derive(Deserialize)]
struct DiscordOAuthAuthorizeResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct DiscordOAuthUserResponse {
    id: String,
}

async fn json_request<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    name: &str,
) -> Result<T, anyhow::Error> {
    let result = request
        .send()
        .await
        .map_err(|e| anyhow!("request {name} failed to send: {e}"))?
        .error_for_status()
        .map_err(|e| anyhow!("request {name} returned error: {e}"))?;

    let text =
        result.text().await.map_err(|e| anyhow!("request {name} failed to read text: {e}"))?;

    let data: T = serde_json::from_str(&text)
        .map_err(|e| anyhow!("request {name} failed to parse json: {e}, text was: {text}"))?;

    Ok(data)
}
