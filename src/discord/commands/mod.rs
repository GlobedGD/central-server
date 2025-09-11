use std::sync::Arc;

use crate::discord::{BotError, state::BotState};

mod link;
mod util;

pub fn all() -> Vec<poise::Command<Arc<BotState>, BotError>> {
    vec![link::link()]
}
