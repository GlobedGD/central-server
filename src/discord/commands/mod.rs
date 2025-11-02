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
        link::adminlink(),
        link::unlink(),
        link::sync(),
        link::syncall(),
        moderation::punish(),
        moderation::unpunish(),
        moderation::audit_log(),
        moderation::check_alts(),
        #[cfg(feature = "featured-levels")]
        features::feature(),
        maintenance::refresh_blacklist_cache(),
        maintenance::set_level_blacklisted(),
        maintenance::status(),
    ]
}
