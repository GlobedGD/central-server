use std::sync::Arc;

use serenity::{Client, all::GatewayIntents, prelude::TypeMapKey};

use crate::discord::{event_handler::Handler, state::BotState};

pub struct DiscordBot {
    client: Client,
}

pub struct BotStateType;

impl TypeMapKey for BotStateType {
    type Value = Arc<BotState>;
}

impl DiscordBot {
    pub async fn new(token: &str, state: Arc<BotState>) -> serenity::Result<Self> {
        let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

        let client = Client::builder(token, intents).event_handler(Handler::new()).await?;
        client.data.write().await.insert::<BotStateType>(state);

        Ok(Self { client })
    }

    pub async fn start(&mut self) -> serenity::Result<()> {
        self.client.start().await
    }
}
