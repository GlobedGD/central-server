use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

use super::serenity::{self, ChannelId, Context, CreateMessage, UserId};
use dashmap::DashMap;
use qunet::server::{ServerHandle, WeakServerHandle};
use thiserror::Error;
use tokio::sync::{RwLock, oneshot};
use tracing::debug;

use crate::{
    core::handler::ConnectionHandler,
    discord::{DiscordMessage, DiscordUserData},
    users::DatabaseError,
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

pub struct BotState {
    ctx: RwLock<Option<Context>>,
    server: OnceLock<WeakServerHandle<ConnectionHandler>>,
    link_attempts: DashMap<u64, LinkAttempt>,
}

#[derive(Error, Debug)]
pub enum BotError {
    #[error("Bot context not yet available")]
    NoContext,
    #[error("Invalid channel ID given")]
    InvalidChannel,
    #[error("{0}")]
    Serenity(#[from] serenity::Error),
    #[error("Database error: {0}")]
    Database(#[from] DatabaseError),
    #[error("{0}")]
    Custom(String),
}

impl BotError {
    pub fn custom(s: impl Into<String>) -> Self {
        Self::Custom(s.into())
    }
}

impl BotState {
    pub fn new() -> Self {
        Self {
            ctx: RwLock::new(None),
            server: OnceLock::new(),
            link_attempts: DashMap::new(),
        }
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

    pub fn server(&self) -> Option<ServerHandle<ConnectionHandler>> {
        self.server.get().and_then(|x| x.upgrade())
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

    pub fn cleanup_link_attempts(&self) {
        self.link_attempts.retain(|_, la| la.started_at.elapsed() < Duration::from_mins(1));
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
}
