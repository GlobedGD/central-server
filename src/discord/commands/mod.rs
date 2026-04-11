use std::sync::Arc;

use crate::discord::{BotError, state::BotState};

#[cfg(feature = "featured-levels")]
mod features;
mod link;
mod maintenance;
mod misc;
mod moderation;
pub mod util;

pub fn all() -> Vec<poise::Command<Arc<BotState>, BotError>> {
    vec![
        link::link(),
        link::adminlink(),
        link::unlink(),
        link::sync(),
        link::syncall(),
        link::linkinfo(),
        moderation::punish(),
        moderation::unpunish(),
        moderation::audit_log(),
        moderation::check_actions(),
        moderation::check_alts(),
        moderation::kick(),
        moderation::kick_all(),
        #[cfg(feature = "featured-levels")]
        features::feature(),
        maintenance::refresh_blacklist_cache(),
        maintenance::set_level_blacklisted(),
        maintenance::shutdown_server(),
        maintenance::disallow_joins(),
        maintenance::status(),
        maintenance::reload_config(),
        maintenance::conn_stats(),
        maintenance::player_count(),
        misc::say(),
    ]
}
