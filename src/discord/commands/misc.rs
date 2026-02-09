use super::util::*;
use crate::discord::BotError;

#[poise::command(slash_command, guild_only = true)]
/// Say a message (for testing)
pub async fn say(ctx: Context<'_>, what: String) -> Result<(), BotError> {
    // check_linked_and_roles(ctx, |role| role.can_moderate() || role.priority > 0).await?;
    check_admin(ctx).await?;

    ctx.reply(format!("`@{}`: {}", ctx.author().name, what)).await?;

    Ok(())
}
