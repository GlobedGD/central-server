use std::sync::Arc;

use tracing::info;

use super::serenity::{self, Client, GatewayIntents};

use crate::discord::state::BotState;

pub struct DiscordBot {
    client: Client,
}

impl DiscordBot {
    pub async fn new(token: &str, state: Arc<BotState>) -> serenity::Result<Self> {
        let intents = GatewayIntents::non_privileged() | GatewayIntents::GUILD_MEMBERS;

        let framework = poise::Framework::builder()
            .options(poise::FrameworkOptions {
                commands: super::commands::all(),
                on_error: |error| Box::pin(super::event_handler::on_error(error)),
                // command_check: Some(|_ctx| {
                //     Box::pin(async move {
                //         // allow from a specific guild?
                //         Ok(true)
                //     })
                // }),
                event_handler: |ctx, event, framework, data| {
                    Box::pin(super::event_handler::event_handler(ctx, event, framework, data))
                },
                ..Default::default()
            })
            .setup(move |ctx, ready, framework| {
                Box::pin(async move {
                    info!(
                        "Discord bot is running, user: {} ({})",
                        ready.user.display_name(),
                        ready.user.id
                    );

                    state.set_ctx(ctx.clone()).await;

                    // register commands
                    poise::builtins::register_globally(ctx, &framework.options().commands).await?;

                    Ok(state)
                })
            })
            .build();

        let client = Client::builder(token, intents).framework(framework).await?;

        Ok(Self { client })
    }

    pub async fn start(&mut self) -> serenity::Result<()> {
        self.client.start().await
    }
}
