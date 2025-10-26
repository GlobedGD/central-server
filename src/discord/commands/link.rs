use std::time::Duration;

use poise::serenity_prelude as serenity;

use super::util::*;
use crate::{discord::BotError, users::UsersModule};

#[poise::command(slash_command, guild_only = true)]
/// Link your Discord account to your GD account
pub async fn link(
    ctx: Context<'_>,
    #[description = "Geometry Dash username or ID"] user: String,
) -> Result<(), BotError> {
    let state = ctx.data();
    let server = state.server()?;

    // check if user is already linked
    let author = ctx.author();
    let member = ctx.author_member().await.unwrap();
    let users = server.handler().module::<UsersModule>();

    if let Some(user) = users.get_linked_discord_inverse(author.id.get()).await? {
        ctx.reply(format!(
            ":x: Already linked to an account: {} ({})",
            user.username.as_deref().unwrap_or("Unknown"),
            user.account_id
        ))
        .await?;

        return Ok(());
    }

    if state.has_link_attempt(author.id.get()) {
        ctx.reply(":x: Already attempting to link to an account. Please wait and try again.")
            .await?;
        return Ok(());
    }

    let target = if let Ok(id) = user.parse::<i32>() {
        server.handler().find_client(id).or_else(|| server.handler().find_client_by_name(&user))
    } else {
        server.handler().find_client_by_name(&user)
    };

    let Some(target) = target else {
        ctx.reply(
            ":x: Failed to find the user by the given name. Make sure you are currently online on Globed and try again.",
        ).await?;
        return Ok(());
    };

    if !target.discord_pairing() {
        ctx.reply(":x: Linking is not currently enabled for this account. Please go to Globed settings, click \"Link\" on Discord Linking, and enable linking.").await?;
        return Ok(());
    }

    // initiate link attempt
    match server.handler().send_discord_link_attempt(
        &target,
        author.id.get(),
        &author.name,
        &author.avatar_url().unwrap_or_default(),
    ) {
        Ok(()) => {}

        Err(e) => {
            ctx.reply(format!(":x: Failed to send a link request message: {e}")).await?;
            return Ok(());
        }
    }

    let msg_handle = ctx.reply("✅ Request was sent! Now open the game and confirm it...").await?;

    // create a link attempt and wait up to 30s for a response
    let attempt = state.create_link_attempt(author.id.get(), target.account_id());
    let result = tokio::time::timeout(Duration::from_secs(30), attempt).await;

    // always delete link attempt
    state.remove_link_attempt(author.id.get());

    match result {
        Ok(Ok(accepted)) => {
            if accepted {
                users.link_discord_account_online(&target, author.id.get()).await?;
                state.sync_user_roles(&member).await?;

                edit_message(
                    ctx,
                    msg_handle,
                    format!(
                        "✅ Linked <@{}> to GD account {} ({})",
                        author.id,
                        target.username(),
                        target.account_id()
                    ),
                )
                .await?;
            } else {
                edit_message(ctx, msg_handle, ":x: Player declined the link attempt.".to_owned())
                    .await?;
            }
        }

        Ok(Err(e)) => return Err(BotError::custom(format!("Link failed due to RecvError: {e}"))),

        Err(_) => {
            edit_message(
                ctx,
                msg_handle,
                ":x: Player did not accept link request in 30 seconds. Please try again.",
            )
            .await?;
        }
    }

    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
/// Link someone's Discord account to a GD account
pub async fn adminlink(
    ctx: Context<'_>,
    user: serenity::Member,
    #[description = "Geometry Dash username"] gd_user: String,
) -> Result<(), BotError> {
    check_moderator(ctx).await?;

    let state = ctx.data();
    let server = state.server()?;
    let users = server.handler().module::<UsersModule>();

    // unlink any existing link
    let _ = users.unlink_discord_inverse(user.user.id.get()).await;

    let Some(target) = users.query_or_create_user(&gd_user).await? else {
        ctx.reply(":x: Failed to find the user by the given name").await?;
        return Ok(());
    };

    users.link_discord_account_offline(target.account_id, user.user.id.get()).await?;
    state.sync_user_roles(&user).await?;

    ctx.reply(format!(
        "✅ Linked <@{}> to GD account {} ({})",
        user.user.id,
        target.username.as_deref().unwrap_or("Unknown"),
        target.account_id
    ))
    .await?;

    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
/// Unlink a GD account, admin only command
pub async fn unlink(ctx: Context<'_>, user: serenity::Member) -> Result<(), BotError> {
    check_moderator(ctx).await?;

    let state = ctx.data();
    let server = state.server()?;
    let users = server.handler().module::<UsersModule>();

    let linked_acc = users.get_linked_discord_inverse(user.user.id.get()).await?;
    if linked_acc.is_none() {
        ctx.reply(":x: User is not linked to any GD account.").await?;
        return Ok(());
    }

    let linked_acc = linked_acc.unwrap();

    users.unlink_discord_inverse(user.user.id.get()).await?;
    users.system_set_roles(linked_acc.account_id, &[]).await?; // clear all roles

    ctx.reply(format!(
        "✅ Successfully unlinked. Previously linked account: {} ({})",
        linked_acc.username.as_deref().unwrap_or("Unknown"),
        linked_acc.account_id
    ))
    .await?;

    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
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

    // let state = ctx.data();
    // let server = state.server()?;

    // TODO

    Ok(())
}
