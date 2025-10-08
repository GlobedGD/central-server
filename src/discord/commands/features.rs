use super::util::*;
use crate::{
    core::gd_api::GDApiClient, discord::BotError, features::FeaturesModule, users::UsersModule,
};

#[poise::command(
    slash_command,
    subcommands(
        "send",
        "queue",
        "update_spreadsheet",
        "set_duration",
        "set_priority",
        "force_cycle"
    )
)]
pub async fn feature(_ctx: Context<'_>) -> Result<(), BotError> {
    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
/// Update featured levels spreadsheet
pub async fn update_spreadsheet(ctx: Context<'_>) -> Result<(), BotError> {
    let state = ctx.data();
    let Some(server) = state.server() else {
        return Err(BotError::custom("Server handle not initialized"));
    };

    if !is_admin(ctx).await? {
        ctx.reply(":x: Must be administrator to use this command.").await?;
        return Ok(());
    }

    let features = server.handler().module::<FeaturesModule>();

    features.update_spreadsheet(true, true, true).await;

    ctx.reply("✅ Requested spreadsheet update. It may take a few minutes to update.").await?;

    Ok(())
}
async fn send_autocomplete(
    _ctx: Context<'_>,
    _partial: &str,
) -> impl Iterator<Item = poise::serenity_prelude::AutocompleteChoice> {
    ["Normal", "Featured", "Outstanding"]
        .iter()
        .map(|&n| poise::serenity_prelude::AutocompleteChoice::new(n, n))
}

#[poise::command(slash_command, guild_only = true)]
/// Send a level to be featured
pub async fn send(
    ctx: Context<'_>,
    level_id: i32,
    #[autocomplete = "send_autocomplete"]
    #[description = "Rate tier"]
    rate_tier: String,
    note: String,
) -> Result<(), BotError> {
    send_inner(ctx, level_id, rate_tier, note, false).await
}

#[poise::command(slash_command, guild_only = true)]
/// Queue a level to be featured
pub async fn queue(
    ctx: Context<'_>,
    level_id: i32,
    #[autocomplete = "send_autocomplete"]
    #[description = "Rate tier"]
    rate_tier: String,
    note: String,
) -> Result<(), BotError> {
    send_inner(ctx, level_id, rate_tier, note, true).await
}

async fn send_inner(
    ctx: Context<'_>,
    level_id: i32,
    rate_tier: String,
    note: String,
    queue: bool,
) -> Result<(), BotError> {
    let state = ctx.data();
    let Some(server) = state.server() else {
        return Err(BotError::custom("Server handle not initialized"));
    };

    let Some(user) = get_linked_gd_user(ctx, &server).await? else {
        return Ok(());
    };
    let role = server.handler().module::<UsersModule>().compute_from_user(&user);
    let needed_perm = if queue { role.can_rate_features } else { role.can_send_features };

    if !needed_perm {
        ctx.reply(":x: You do not have permission to use this command.").await?;
        return Ok(());
    }

    let rate_tier = match rate_tier.as_str() {
        "Normal" => 1,
        "Featured" => 2,
        "Outstanding" => 3,
        _ => {
            ctx.reply(":x: Invalid rate tier.").await?;
            return Ok(());
        }
    };

    let features = server.handler().module::<FeaturesModule>();

    let level = match GDApiClient::new().fetch_level(level_id).await {
        Ok(Some(level)) => level,
        Ok(None) => {
            ctx.reply(":x: Level not found. Make sure the ID is correct.").await?;
            return Ok(());
        }

        Err(e) => {
            ctx.reply(format!(":x: Failed to fetch level from GD servers: {e}")).await?;
            return Ok(());
        }
    };

    if let Err(e) = features
        .send_level(
            user.account_id,
            level.id,
            &level.name,
            level.author_id,
            &level.author_name,
            rate_tier,
            &note,
            queue,
        )
        .await
    {
        ctx.reply(format!(":x: Failed to add level to database: {e}")).await?;
        return Ok(());
    }

    ctx.reply(format!("✅ Successfully sent {} by {}!", level.name, level.author_name)).await?;

    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
/// Set the feature duration for a level
pub async fn set_duration(
    ctx: Context<'_>,
    level_id: i32,
    #[rename = "duration"]
    #[description = "Punishment duration (i.e. \"1 year\", \"2 days\"); use \"permanent\" for permanent punishments."]
    duration_str: String,
) -> Result<(), BotError> {
    let state = ctx.data();
    let Some(server) = state.server() else {
        return Err(BotError::custom("Server handle not initialized"));
    };

    let Some(user) = get_linked_gd_user(ctx, &server).await? else {
        return Ok(());
    };
    let role = server.handler().module::<UsersModule>().compute_from_user(&user);

    if !role.can_rate_features {
        ctx.reply(":x: You do not have permission to use this command.").await?;
        return Ok(());
    }

    let Ok(dur) = parse_duration_str(&duration_str) else {
        ctx.reply(":x: Invalid duration!").await?;
        return Ok(());
    };

    let features = server.handler().module::<FeaturesModule>();
    if let Err(e) = features.set_feature_duration(level_id, dur).await {
        ctx.reply(format!(":x: Failed to set feature duration: {e}")).await?;
        return Ok(());
    }

    ctx.reply("✅ Feature duration updated successfully!").await?;
    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
/// Set the feature priority for a level
pub async fn set_priority(ctx: Context<'_>, level_id: i32, priority: i32) -> Result<(), BotError> {
    let state = ctx.data();
    let Some(server) = state.server() else {
        return Err(BotError::custom("Server handle not initialized"));
    };

    let Some(role) = get_linked_gd_role(ctx, &server).await? else {
        return Ok(());
    };

    if !role.can_rate_features {
        ctx.reply(":x: You do not have permission to use this command.").await?;
        return Ok(());
    }

    let features = server.handler().module::<FeaturesModule>();
    if let Err(e) = features.set_feature_priority(level_id, priority).await {
        ctx.reply(format!(":x: Failed to set feature priority: {e}")).await?;
        return Ok(());
    }

    ctx.reply("✅ Feature priority updated successfully!").await?;
    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
/// Set the feature priority for a level
pub async fn force_cycle(ctx: Context<'_>) -> Result<(), BotError> {
    let state = ctx.data();
    let Some(server) = state.server() else {
        return Err(BotError::custom("Server handle not initialized"));
    };

    let Some(role) = get_linked_gd_role(ctx, &server).await? else {
        return Ok(());
    };

    if !role.can_rate_features {
        ctx.reply(":x: You do not have permission to use this command.").await?;
        return Ok(());
    }

    let features = server.handler().module::<FeaturesModule>();
    match features.cycle_level().await {
        Ok(true) => {
            ctx.reply("✅ Feature priority updated successfully!").await?;
        }

        Ok(false) => {
            ctx.reply("⚠️ No queued levels to feature.").await?;
        }

        Err(e) => {
            ctx.reply(format!(":x: Failed to cycle featured level: {e}")).await?;
        }
    }

    Ok(())
}
