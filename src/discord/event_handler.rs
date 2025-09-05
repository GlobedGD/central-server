use serenity::{all::Ready, async_trait, model::channel::Message, prelude::*};
use tracing::info;

use crate::discord::bot::BotStateType;

pub struct Handler {}

impl Handler {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        info!("Discord bot is running, user: {} ({})", ready.user.display_name(), ready.user.id);

        let state = ctx
            .data
            .read()
            .await
            .get::<BotStateType>()
            .cloned()
            .expect("discord bot state not set");

        state.set_ctx(ctx).await;
    }
}
