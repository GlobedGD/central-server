use std::sync::Arc;

use crate::discord::{BotError, state::BotState};

#[cfg(feature = "featured-levels")]
mod features;
mod link;
mod maintenance;
mod moderation;
mod util;

pub fn all() -> Vec<poise::Command<Arc<BotState>, BotError>> {
    vec![
        link::link(),
        link::unlink(),
        moderation::punish(),
        moderation::unpunish(),
        moderation::audit_log(),
        #[cfg(feature = "featured-levels")]
        features::feature(),
        maintenance::status(),
    ]
}
