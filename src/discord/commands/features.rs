use super::util::*;
use crate::{
    core::gd_api::GDApiClient, discord::BotError, features::FeaturesModule, users::ComputedRole,
};

#[poise::command(
    slash_command,
    subcommands(
        "send",
        "queue",
        "unsend",
        "update_spreadsheet",
        "set_duration",
        "set_priority",
        "set_tier",
        "force_cycle"
    )
)]
pub async fn feature(_ctx: Context<'_>) -> Result<(), BotError> {
    Ok(())
}

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
/// Update featured levels spreadsheet
pub async fn update_spreadsheet(ctx: Context<'_>) -> Result<(), BotError> {
    check_admin(ctx).await?;

    let server = ctx.data().server()?;
    let features = server.handler().module::<FeaturesModule>();

    features.update_spreadsheet(true, true, true).await;

    ctx.reply("✅ Requested spreadsheet update. It may take a few minutes to update.").await?;

    Ok(())
}

async fn send_autocomplete(
    _ctx: Context<'_>,
    _partial: &str,
) -> impl Iterator<Item = poise::serenity_prelude::AutocompleteChoice> {
    ["Featured", "Epic", "Outstanding"]
        .iter()
        .map(|&n| poise::serenity_prelude::AutocompleteChoice::new(n, n))
}

fn tier_str_to_int(tier: &str) -> Option<u8> {
    match tier {
        "Featured" => Some(0),
        "Epic" => Some(1),
        "Outstanding" => Some(2),
        _ => None,
    }
}

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
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

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
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
    let has_perm = |r: &ComputedRole| if queue { r.can_rate_features } else { r.can_send_features };
    let user = check_linked_and_roles(ctx, has_perm).await?;

    let server = ctx.data().server()?;

    let Some(rate_tier) = tier_str_to_int(&rate_tier) else {
        ctx.reply(":x: Invalid rate tier.").await?;
        return Ok(());
    };

    let features = server.handler().module::<FeaturesModule>();
    let client = GDApiClient::new(server.handler().http_client());

    let level = match client.fetch_level(level_id).await {
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

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
/// Remove all instances of a level from the sent / queued table
pub async fn unsend(ctx: Context<'_>, level_id: i32) -> Result<(), BotError> {
    let user = check_linked_and_roles(ctx, |r| r.can_rate_features).await?;

    let server = ctx.data().server()?;
    let features = server.handler().module::<FeaturesModule>();

    match features.unsend_level(level_id).await {
        Ok(()) => {
            ctx.reply("✅ Successfully removed level from sent/queued list.").await?;
        }

        Err(e) => {
            ctx.reply(format!(":x: Failed to remove level from database: {e}")).await?;
        }
    }

    Ok(())
}

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
/// Set the feature duration for a level
pub async fn set_duration(
    ctx: Context<'_>,
    level_id: i32,
    #[rename = "duration"]
    #[description = "Feature duration (i.e. \"1 year\", \"2 days\")"]
    duration_str: String,
) -> Result<(), BotError> {
    check_admin(ctx).await?;

    let server = ctx.data().server()?;

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

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
/// Set the feature priority for a level
pub async fn set_priority(ctx: Context<'_>, level_id: i32, priority: i32) -> Result<(), BotError> {
    check_admin(ctx).await?;

    let server = ctx.data().server()?;

    let features = server.handler().module::<FeaturesModule>();
    if let Err(e) = features.set_feature_priority(level_id, priority).await {
        ctx.reply(format!(":x: Failed to set feature priority: {e}")).await?;
        return Ok(());
    }

    ctx.reply("✅ Feature priority updated successfully!").await?;
    Ok(())
}

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
/// Set the feature tier for a queued or featured level
pub async fn set_tier(
    ctx: Context<'_>,
    level_id: i32,
    #[autocomplete = "send_autocomplete"]
    #[description = "Rate tier"]
    rate_tier: String,
) -> Result<(), BotError> {
    check_admin(ctx).await?;

    let server = ctx.data().server()?;
    let Some(rate_tier) = tier_str_to_int(&rate_tier) else {
        ctx.reply(":x: Invalid rate tier.").await?;
        return Ok(());
    };

    let features = server.handler().module::<FeaturesModule>();
    if let Err(e) = features.set_feature_tier(level_id, rate_tier).await {
        ctx.reply(format!(":x: Failed to set feature tier: {e}")).await?;
        return Ok(());
    }

    ctx.reply("✅ Feature tier updated successfully!").await?;

    Ok(())
}

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
/// Force cycle to the next featured level
pub async fn force_cycle(ctx: Context<'_>) -> Result<(), BotError> {
    check_admin(ctx).await?;

    let server = ctx.data().server()?;
    let features = server.handler().module::<FeaturesModule>();

    match features.cycle_level().await {
        Ok(Some(level)) => {
            ctx.reply(format!(
                "✅ Successfully featured next level: {} ({}) by {}!",
                level.name, level.id, level.author_name
            ))
            .await?;
        }

        Ok(None) => {
            ctx.reply("⚠️ No queued levels to feature.").await?;
        }

        Err(e) => {
            ctx.reply(format!(":x: Failed to cycle featured level: {e}")).await?;
        }
    }

    Ok(())
}
