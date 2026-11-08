use poise::serenity_prelude as serenity;
use tracing::warn;

use super::util::*;
use crate::{
    discord::BotError,
    users::{DbUser, DiscordUserData, UsersModule},
};

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
/// Sync your roles with your GD account
pub async fn sync(ctx: Context<'_>) -> Result<(), BotError> {
    match ctx.data().sync_user_roles(&ctx.author_member().await.unwrap()).await {
        Ok(roles) => {
            ctx.reply(format!("✅ Successfully synced roles: {}", itertools::join(&roles, ", ")))
                .await?;
            Ok(())
        }

        Err(BotError::Custom(e)) => {
            ctx.reply(format!(":x: Failed to sync roles: {e}")).await?;
            Ok(())
        }

        Err(e) => Err(e),
    }
}

#[poise::command(slash_command, guild_only = true)]
/// Sync all users' roles with their GD accounts (admin only)
pub async fn syncall(ctx: Context<'_>) -> Result<(), BotError> {
    check_admin(ctx).await?;

    let state = ctx.data().clone();
    tokio::spawn(async move {
        if let Err(e) = state.slow_sync_all().await {
            warn!("Failed to sync all users' roles: {e}");
        }
    });

    ctx.reply("✅ Started syncing all users' roles in the background. This may take a while.")
        .await?;

    Ok(())
}

#[poise::command(slash_command, guild_only = true, ephemeral = true)]
/// Check if a discord user is linked to a GD account and vice versa
pub async fn linkinfo(
    ctx: Context<'_>,
    discord_user: Option<serenity::Member>,
    gd_user: Option<String>,
) -> Result<(), BotError> {
    let result = get_linked_user(ctx, discord_user.as_ref(), gd_user.as_deref()).await?;
    let is_same_user =
        result.as_ref().is_some_and(|(_, linked)| linked.id == ctx.author().id.get());

    if !is_same_user {
        check_moderator(ctx).await?;
    }

    match result {
        Some((dbuser, linked)) => {
            ctx.reply(format!(
                "✅ `@{}` (`{}`) is linked to GD account `{}` (`{}`)",
                linked.username,
                linked.id,
                dbuser.username(),
                dbuser.account_id
            ))
            .await?;
        }

        None => match (discord_user, gd_user) {
            (Some(u), None) => {
                ctx.reply(format!(":x: `@{}` is not linked to any GD account.", u.user.name))
                    .await?;
            }

            (None, None) => {
                ctx.reply(":x: Please provide either a Discord user or a GD username/ID.").await?;
            }

            _ => {
                ctx.reply(
                    ":x: Could not find the account in the database or the user is not linked.",
                )
                .await?;
            }
        },
    }

    Ok(())
}

async fn get_linked_user(
    ctx: Context<'_>,
    discord_user: Option<&serenity::Member>,
    gd_user: Option<&str>,
) -> Result<Option<(DbUser, DiscordUserData)>, BotError> {
    let server = ctx.data().server()?;
    let users = server.handler().module::<UsersModule>();

    if let Some(u) = discord_user {
        let dbu = users.get_linked_discord_inverse(u.user.id.get()).await?;
        let data = DiscordUserData::from_discord_member(u);

        return Ok(dbu.map(|u| (u, data)));
    }

    if let Some(gdu) = gd_user {
        let db_user = users.query_user(gdu).await?;
        let linked = match &db_user {
            Some(x) => users.get_linked_discord(x.account_id).await?,
            None => None,
        };

        return Ok(match (db_user, linked) {
            (Some(dbu), Some(link)) => Some((dbu, link)),
            _ => None,
        });
    };

    // nothing supplied, try to fetch this user
    let Some(actor) = ctx.author_member().await else {
        return Ok(None);
    };

    let dbu = users.get_linked_discord_inverse(actor.user.id.get()).await?;
    let data = DiscordUserData::from_discord_member(&actor);

    Ok(dbu.map(|u| (u, data)))
}
