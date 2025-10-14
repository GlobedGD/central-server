use std::{sync::Arc, time::Duration};

use poise::{CreateReply, ReplyHandle, serenity_prelude as serenity};
use qunet::server::Server;
use thiserror::Error;

use crate::{
    core::handler::ConnectionHandler,
    discord::{BotError, state::BotState},
    users::{ComputedRole, DbUser, UsersModule},
};

pub type Context<'a> = poise::Context<'a, Arc<BotState>, BotError>;

pub async fn edit_message(
    ctx: Context<'_>,
    msg: ReplyHandle<'_>,
    content: impl Into<String>,
) -> Result<(), serenity::Error> {
    msg.edit(ctx, CreateReply::default().content(content)).await
}

pub async fn is_admin(ctx: Context<'_>) -> Result<bool, BotError> {
    Ok(ctx.author_member().await.is_some_and(|x| x.permissions.is_some_and(|x| x.administrator())))
}

pub async fn is_moderator(ctx: Context<'_>) -> Result<bool, BotError> {
    Ok(ctx
        .author_member()
        .await
        .is_some_and(|x| x.permissions.is_some_and(|x| x.ban_members() || x.manage_roles())))
}

pub async fn get_linked_gd_user(
    ctx: Context<'_>,
    server: &Server<ConnectionHandler>,
) -> Result<Option<DbUser>, BotError> {
    let author = ctx.author();
    let users = server.handler().module::<UsersModule>();

    // check if we're not linked
    match users.get_linked_discord_inverse(author.id.get()).await? {
        Some(user) => Ok(Some(user)),
        None => {
            ctx.reply(":x: Not linked to a GD account! Please use the /link command and the Discord Linking option in game to link.").await?;
            Ok(None)
        }
    }
}

pub async fn get_linked_gd_role(
    ctx: Context<'_>,
    server: &Server<ConnectionHandler>,
) -> Result<Option<ComputedRole>, BotError> {
    Ok(get_linked_gd_user(ctx, server)
        .await?
        .map(|u| server.handler().module::<UsersModule>().compute_from_user(&u)))
}

#[derive(Debug, Error)]
#[error("Failed to parse duration string")]
pub struct ParseDurationError;

pub fn parse_duration_str(s: &str) -> Result<Duration, ParseDurationError> {
    if s.starts_with("perma") || s.starts_with("Perma") || s.eq_ignore_ascii_case("forever") {
        return Ok(Duration::from_secs(0));
    }

    if !s.contains(' ') {
        return Err(ParseDurationError);
    }

    let mut split = s.split(' ');
    let number = split.next().and_then(|x| x.parse::<u64>().ok()).ok_or(ParseDurationError)?;

    let modifier: u64 = match split.next().unwrap() {
        "second" => 1,
        "seconds" => 1,
        "minute" => 60,
        "minutes" => 60,
        "hour" => 3600,
        "hours" => 3600,
        "day" => 3600 * 24,
        "days" => 3600 * 24,
        "month" => 3600 * 24 * 30,
        "months" => 3600 * 24 * 30,
        "year" => 3600 * 24 * 30 * 12,
        "years" => 3600 * 24 * 30 * 12,
        _ => 0,
    };

    Ok(Duration::from_secs(number * modifier))
}
