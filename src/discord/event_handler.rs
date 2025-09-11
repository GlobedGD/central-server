use std::sync::Arc;

use super::serenity::*;
use tracing::warn;

use crate::discord::{BotError, state::BotState};

pub async fn event_handler(
    _ctx: &Context,
    _event: &FullEvent,
    _framework: poise::FrameworkContext<'_, Arc<BotState>, BotError>,
    _state: &Arc<BotState>,
) -> Result<(), BotError> {
    Ok(())
}

pub async fn on_error(error: poise::FrameworkError<'_, Arc<BotState>, BotError>) {
    match error {
        poise::FrameworkError::Setup { error, .. } => warn!("Failed to start bot: {:?}", error),
        poise::FrameworkError::Command { error, ctx, .. } => {
            warn!("Command '{}' errored: {error}", ctx.command().name);
            let _ = ctx.reply(format!(":x: Command failed due to internal error. Please report this to the developer.\n\nError: {error}")).await;
        }

        error => {
            if let Err(e) = poise::builtins::on_error(error).await {
                warn!("Error while handling error: {}", e)
            }
        }
    }
}
