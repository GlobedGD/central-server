use std::{
    fmt::Write,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::util::*;
use crate::{
    discord::{BotError, hex_color_to_decimal},
    users::{UserPunishmentType, UsersModule, database::AuditLogModel},
};

use poise::{
    CreateReply,
    serenity_prelude::{self as serenity, CreateEmbed, EmbedField},
};
use tracing::info;

async fn do_punish(
    ctx: Context<'_>,
    pun_type: UserPunishmentType,
    target_user: &str,
    reason: &str,
    duration_str: &str,
) -> Result<(), BotError> {
    let user = check_linked_and_can_punish(ctx, pun_type).await?;

    let server = ctx.data().server()?;
    let users = server.handler().module::<UsersModule>();

    let target = users.query_or_create_user(target_user).await?;
    let Some(target) = target else {
        ctx.reply(":x: Failed to find the user by the given name").await?;
        return Ok(());
    };

    let duration = parse_duration_str(duration_str)?;
    let expires_at = if duration.is_zero() {
        0
    } else {
        (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + duration).as_secs() as i64
    };

    let ban_result = server
        .handler()
        .do_punish_user(user.account_id, target.account_id, reason, expires_at, pun_type)
        .await;

    if let Err(reason) = ban_result {
        ctx.reply(format!(":x: Failed to issue punishment: {reason}")).await?;
    } else {
        ctx.reply(format!(":white_check_mark: Successfully punished {target}")).await?;
    }

    Ok(())
}

macro_rules! punish_command {
    ($fn_name:ident, $type:expr, $action:literal) => {
        #[poise::command(slash_command, guild_only = true)]
        #[doc = concat!($action, " the provided user in-game")]
        pub async fn $fn_name(
            ctx: Context<'_>,

            #[autocomplete = "online_and_db_user_autocomplete"]
            #[description = "Geometry Dash username or ID"]
            target_user: String,

            #[description = "Punishment reason"]
            reason: String,

            #[rename = "duration"]
            #[description = "Punishment duration (i.e. \"1 year\", \"2 days\"); use \"permanent\" or \"perma\" for permanent punishments."]
            duration_str: String,
        ) -> Result<(), BotError> {
            do_punish(ctx, $type, &target_user, &reason, &duration_str).await
        }
    };
}

punish_command!(ban, UserPunishmentType::Ban, "Bans");
punish_command!(roomban, UserPunishmentType::RoomBan, "Room bans");
punish_command!(mute, UserPunishmentType::Mute, "Mutes");

async fn do_unpunish(
    ctx: Context<'_>,
    pun_type: UserPunishmentType,
    target_user: &str,
) -> Result<(), BotError> {
    let user = check_linked_and_can_punish(ctx, pun_type).await?;

    let server = ctx.data().server()?;
    let users = server.handler().module::<UsersModule>();

    let target = users.query_user(target_user).await?;
    let Some(target) = target else {
        ctx.reply(":x: Failed to find the user by the given name").await?;
        return Ok(());
    };

    let unpunish_result =
        server.handler().do_unpunish_user(user.account_id, target.account_id, pun_type).await;

    if let Err(reason) = unpunish_result {
        ctx.reply(format!(":x: Failed to remove punishment: `{reason}`")).await?;
    } else {
        ctx.reply(format!(":white_check_mark: Successfully removed punishment for {target}"))
            .await?;
    }

    Ok(())
}

macro_rules! unpunish_command {
    ($fn_name:ident, $type:expr, $action:literal) => {
        #[poise::command(slash_command, guild_only = true)]
        #[doc = concat!($action, " the provided user in-game")]
        pub async fn $fn_name(
            ctx: Context<'_>,

            #[autocomplete = "online_and_db_user_autocomplete"]
            #[description = "Geometry Dash username or ID"]
            target_user: String,
        ) -> Result<(), BotError> {
            do_unpunish(ctx, $type, &target_user).await
        }
    };
}

unpunish_command!(unban, UserPunishmentType::Ban, "Removes a ban from");
unpunish_command!(unmute, UserPunishmentType::Mute, "Removes a mute from");
unpunish_command!(unroomban, UserPunishmentType::RoomBan, "Removes a room ban from");

#[allow(clippy::format_in_format_args)]
async fn audit_log_embed(
    logs: Vec<AuditLogModel>,
    users: &UsersModule,
    num: u64,
) -> serenity::Embed {
    let mut res = serenity::Embed::default();

    res.title = Some(format!("Audit Log (page {})", num + 1));

    for log in logs {
        let target_user = users.get_user(log.target_account_id.unwrap_or(0) as i32).await;
        let Ok(Some(target_user)) = target_user else {
            return res;
        };

        let issuer_user = users.get_user(log.account_id as i32).await;
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
                    "editban" | "editmute" | "editroomban" => "pencil",
                    _ => "man_shrugging",
                },
                target_user.username.unwrap_or("`unable to retrieve username`".to_string()),
                log.target_account_id.unwrap_or(0),
                log.r#type
            ),
            format!(
                "**Issued by `{}` on <t:{}>**{}\n{}",
                issuer_user.username.unwrap_or("`unable to retrieve username`".to_string()),
                log.timestamp,
                format!(
                    "\n**Reason**: \"{}\"",
                    log.message.as_deref().unwrap_or("No reason provided")
                ),
                format!(
                    "**Expires at**: {}",
                    log.expires_at
                        .as_ref()
                        .map_or_else(|| "Permanent".to_owned(), |ts| format!("<t:{}>", ts))
                )
            ),
            false,
        ));
    }

    res
}

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
pub async fn audit_log(ctx: Context<'_>, issuer: Option<String>) -> Result<(), BotError> {
    const PAGE_SIZE: u64 = 10;

    let user = check_moderator(ctx).await?;

    let server = ctx.data().server()?;
    let users = server.handler().module::<UsersModule>();

    let issuer_id = if let Some(issuer) = issuer {
        let target = users.query_user(&issuer).await?;
        let Some(target) = target else {
            ctx.reply(":x: Failed to find the issuer").await?;
            return Ok(());
        };
        target.account_id
    } else {
        user.account_id
    };

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
                    users.admin_fetch_logs(issuer_id, 0, "", 0, 0, 0, PAGE_SIZE).await?.0,
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
    let mut current_page = 0u64;
    while let Some(press) = serenity::collector::ComponentInteractionCollector::new(ctx)
        // We defined our button IDs to start with `ctx_id`. If they don't, some other command's
        // button was pressed
        .filter(move |press| press.data.custom_id.starts_with(&ctx_id.to_string()))
        // Timeout when no navigation button has been pressed for 5 minutes
        .timeout(Duration::from_mins(5))
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
        let logs = users.admin_fetch_logs(issuer_id, 0, "", 0, 0, current_page, PAGE_SIZE).await?.0;
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

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
pub async fn check_actions(
    ctx: Context<'_>,
    user: Option<serenity::Member>,
    #[description = "Period of time to check (e.g. \"1 day\", \"2 weeks\"), default is 1 month"]
    period: Option<String>,
) -> Result<(), BotError> {
    check_moderator(ctx).await?;

    let server = ctx.data().server()?;
    let users = server.handler().module::<UsersModule>();

    let query_user = user.as_ref().map_or(ctx.author(), |u| &u.user);

    // map
    let period = match period {
        Some(p) => match parse_duration_str(&p)? {
            d if d.is_zero() => None,
            d => Some(d),
        },
        None => Some(Duration::from_days(30)),
    };

    let actions = users.check_actions_over_period_discord(query_user.id.get(), period).await?;

    let embed = CreateEmbed::default()
        .color(hex_color_to_decimal("#629fd9"))
        .title(format!("{}'s actions", query_user.display_name()))
        .description(format!(
            ":hammer: **Bans:** `{}`\n:mute: **Mutes:** `{}`\n:door: **Room Bans:** `{}`\n:bar_chart: **Total Actions:** `{}`",
            actions.bans, actions.mutes, actions.roombans, actions.total
        ));

    ctx.send(ctx.reply_builder(CreateReply::default().embed(embed))).await?;

    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
pub async fn check_alts(
    ctx: Context<'_>,
    #[autocomplete = "db_user_autocomplete"]
    #[description = "GD username or account ID of the target user"]
    user: String,
) -> Result<(), BotError> {
    check_moderator(ctx).await?;

    let server = ctx.data().server()?;
    let users = server.handler().module::<UsersModule>();

    let uident = match users.query_user(&user).await? {
        Some(u) => users.get_user_uident(u.account_id).await?,
        None => None,
    };

    let alts = match uident {
        Some(uid) => users.get_accounts_for_uident(&uid, true).await?,
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

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
pub async fn kick(
    ctx: Context<'_>,
    #[autocomplete = "online_user_autocomplete"]
    #[description = "GD username or account ID of the target user"]
    target: String,
    #[description = "Kick reason"] reason: String,
) -> Result<(), BotError> {
    let user = check_linked_and_roles(ctx, |p| p.can_kick).await?;

    let server = ctx.data().server()?;

    let target = server.handler().find_client_by_id_or_name(&target);
    let Some(target) = target else {
        ctx.reply(":x: Failed to find the target user.").await?;
        return Ok(());
    };

    server.handler().do_kick_user(user.account_id, &target, &reason, true).await;
    ctx.reply(format!(":white_check_mark: Successfully kicked {}", target.username())).await?;

    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
pub async fn kick_all(
    ctx: Context<'_>,
    #[description = "Kick reason"] reason: String,
) -> Result<(), BotError> {
    let user = check_admin(ctx).await?;

    info!("{} ran /kick_all \"{}\"", user.username(), reason);

    let server = ctx.data().server()?;

    let clients = server.handler().get_all_clients();
    for client in &clients {
        server.handler().do_kick_user(0, client, &reason, false).await;
    }

    ctx.reply(format!(":white_check_mark: Successfully kicked all {} users.", clients.len()))
        .await?;

    Ok(())
}
