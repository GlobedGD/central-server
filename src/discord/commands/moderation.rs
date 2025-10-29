use std::{
    fmt::Write,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::util::*;
use crate::{
    discord::BotError,
    users::{UsersModule, database::AuditLogModel},
};

use poise::serenity_prelude::{self as serenity, EmbedField};

async fn punish_autocomplete(
    _ctx: Context<'_>,
    _partial: &str,
) -> impl Iterator<Item = poise::serenity_prelude::AutocompleteChoice> {
    ["Ban", "Mute", "Room Ban"]
        .iter()
        .map(|&n| poise::serenity_prelude::AutocompleteChoice::new(n, n))
}

#[poise::command(slash_command, guild_only = true)]
/// punishes the provided target
pub async fn punish(
    ctx: Context<'_>,
    #[autocomplete = "punish_autocomplete"]
    #[description = "Punishment type"]
    punishment_type: String,
    #[description = "Geometry Dash username or ID"] target_user: String,
    #[description = "Ban reason"] reason: String,
    #[rename = "duration"]
    #[description = "Punishment duration (i.e. \"1 year\", \"2 days\"); use \"permanent\" or \"perma\" for permanent punishments."]
    duration_str: String,
) -> Result<(), BotError> {
    let user = check_moderator(ctx).await?;

    let server = ctx.data().server()?;
    let users = server.handler().module::<UsersModule>();

    let target = users.query_or_create_user(&target_user).await?;

    let Some(target) = target else {
        ctx.reply(":x: Failed to find the user by the given name").await?;
        return Ok(());
    };

    let Ok(duration) = parse_duration_str(&duration_str) else {
        ctx.reply(":x: Invalid duration!").await?;
        return Ok(());
    };

    let ban_result = users
        .admin_punish_user(
            user.account_id,
            target.account_id,
            &reason,
            if duration.is_zero() {
                0
            } else {
                (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + duration).as_secs() as i64
            },
            match punishment_type.as_str() {
                "Ban" => crate::users::UserPunishmentType::Ban,
                "Mute" => crate::users::UserPunishmentType::Mute,
                "Room Ban" => crate::users::UserPunishmentType::RoomBan,
                _ => crate::users::UserPunishmentType::Mute, // just assume they wanna mute (although this shouldn't be reached anyways)
            },
        )
        .await;
    if ban_result.is_err() {
        ctx.reply(format!(":x: Failed to issue punishment: {}", ban_result.unwrap_err())).await?;
    } else {
        ctx.reply(format!(
            ":white_check_mark: Sucessfully punished `{}`",
            target.username.unwrap_or("Could not find username".to_string())
        ))
        .await?;
    }

    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
/// unpunishes the provided target
pub async fn unpunish(
    ctx: Context<'_>,
    #[autocomplete = "punish_autocomplete"]
    #[description = "Punishment type"]
    punishment_type: String,
    #[description = "Geometry Dash username or ID"] target_user: String,
) -> Result<(), BotError> {
    let user = check_moderator(ctx).await?;

    let server = ctx.data().server()?;
    let users = server.handler().module::<UsersModule>();

    let target = users.query_user(&target_user).await?;
    let Some(target) = target else {
        ctx.reply(":x: Failed to find the user by the given name").await?;
        return Ok(());
    };

    let unpunish_result = users
        .admin_unpunish_user(
            user.account_id,
            target.account_id,
            match punishment_type.as_str() {
                "Ban" => crate::users::UserPunishmentType::Ban,
                "Mute" => crate::users::UserPunishmentType::Mute,
                "Room Ban" => crate::users::UserPunishmentType::RoomBan,
                _ => crate::users::UserPunishmentType::Mute, // just assume they wanna mute (although this shouldn't be reached anyways)
            },
        )
        .await;
    if unpunish_result.is_err() {
        ctx.reply(format!(":x: Failed to remove punishment: `{}`", unpunish_result.unwrap_err()))
            .await?;
    } else {
        ctx.reply(format!(
            ":white_check_mark: Sucessfully unpunished `{}`",
            target.username.unwrap_or("Could not find username".to_string())
        ))
        .await?;
    }

    Ok(())
}

async fn audit_log_embed(
    logs: Vec<AuditLogModel>,
    users: &UsersModule,
    num: u32,
) -> serenity::Embed {
    let mut res = serenity::Embed::default();

    res.title = Some(format!("Audit Log (page {})", num + 1));

    for log in logs {
        let target_user = users.get_user(log.target_account_id.unwrap_or(0)).await;
        let Ok(Some(target_user)) = target_user else {
            return res;
        };

        let issuer_user = users.get_user(log.account_id).await;
        let Ok(Some(issuer_user)) = issuer_user else {
            return res;
        };

        res.fields.push(EmbedField::new(
            format!(
                ":{}: ({} [`{}`]) {}",
                match log.r#type.as_str() {
                    "ban" => "x",
                    "unban" => "white_check_mark",
                    "mute" => "mute",
                    "unmute" => "sound",
                    "roomban" => "door",
                    _ => "man_shrugging",
                },
                target_user.username.unwrap_or("`unable to retrieve username`".to_string()),
                log.target_account_id.unwrap_or(0),
                log.r#type
            ),
            format!(
                "**Issued by `{}` on <t:{}>**{}\n**{}**",
                issuer_user.username.unwrap_or("`unable to retrieve username`".to_string()),
                log.timestamp,
                if log.message.is_none() {
                    "".to_string()
                } else {
                    format!("\n**Reason**: \"{}\"", log.message.unwrap())
                },
                if log.expires_at.is_none() {
                    "Automated action".to_string()
                } else {
                    format!("Expires at: <t:{}>", log.expires_at.unwrap())
                }
            ),
            false,
        ));
    }

    res
}

#[poise::command(slash_command, guild_only = true)]
pub async fn audit_log(ctx: Context<'_>) -> Result<(), BotError> {
    let user = check_moderator(ctx).await?;

    let server = ctx.data().server()?;
    let users = server.handler().module::<UsersModule>();

    // Define some unique identifiers for the navigation buttons
    let ctx_id = ctx.id();
    let prev_button_id = format!("{}prev", ctx_id);
    let next_button_id = format!("{}next", ctx_id);

    // Send the embed with the first page as content
    let reply = {
        let components = serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(&prev_button_id).emoji('◀'),
            serenity::CreateButton::new(&next_button_id).emoji('▶'),
        ]);

        poise::CreateReply::default()
            .embed(
                audit_log_embed(
                    users.admin_fetch_logs(user.account_id, 0, "", 0, 0, 0).await?.0,
                    users,
                    0,
                )
                .await
                .into(),
            )
            .components(vec![components])
    };

    ctx.send(reply).await?;

    // Loop through incoming interactions with the navigation buttons
    let mut current_page = 0u32;
    while let Some(press) = serenity::collector::ComponentInteractionCollector::new(ctx)
        // We defined our button IDs to start with `ctx_id`. If they don't, some other command's
        // button was pressed
        .filter(move |press| press.data.custom_id.starts_with(&ctx_id.to_string()))
        // Timeout when no navigation button has been pressed for 24 hours
        .timeout(Duration::from_secs(3600 * 24))
        .await
    {
        // Depending on which button was pressed, go to next or previous page
        if press.data.custom_id == next_button_id {
            current_page += 1;
        } else if press.data.custom_id == prev_button_id {
            current_page = current_page.saturating_sub(1);
        } else {
            // This is an unrelated button interaction
            continue;
        }

        // Update the message with the new page contents
        let logs = users.admin_fetch_logs(user.account_id, 0, "", 0, 0, current_page).await?.0;
        press
            .create_response(
                ctx.serenity_context(),
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .embed(audit_log_embed(logs, users, current_page).await.into()),
                ),
            )
            .await?;
    }

    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
pub async fn check_alts(
    ctx: Context<'_>,
    #[description = "GD username or account ID of the target user"] user: String,
) -> Result<(), BotError> {
    check_moderator(ctx).await?;

    let server = ctx.data().server()?;
    let users = server.handler().module::<UsersModule>();

    let uident = match users.query_user(&user).await? {
        Some(u) => users.get_user_uident(u.account_id).await?,
        None => None,
    };

    let alts = match uident {
        Some(uid) => users.get_accounts_for_uident(&uid).await?,
        None => {
            ctx.reply(":x: Failed to find the user or their uident. This means the user likely hasn't tried logging in since their punishment.").await?;
            return Ok(());
        }
    };

    let mut out_str = format!("Found {} accounts:\n", alts.len());

    for id in alts {
        let acc = users.get_user(id).await?;

        let username = acc.as_ref().and_then(|u| u.username.as_deref()).unwrap_or("Unknown");

        writeln!(out_str, "* {} ({})", username, id).unwrap();
    }

    ctx.reply(out_str).await?;

    Ok(())
}
