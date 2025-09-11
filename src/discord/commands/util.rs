use std::sync::Arc;

use poise::{CreateReply, ReplyHandle, serenity_prelude as serenity};

use crate::discord::{BotError, state::BotState};

pub type Context<'a> = poise::Context<'a, Arc<BotState>, BotError>;

pub async fn edit_message(
    ctx: Context<'_>,
    msg: ReplyHandle<'_>,
    content: impl Into<String>,
) -> Result<(), serenity::Error> {
    msg.edit(ctx, CreateReply::default().content(content)).await
}
