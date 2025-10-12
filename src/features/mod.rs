#[cfg(feature = "discord")]
use std::sync::Arc;
use std::{
    collections::{HashMap, hash_map::Entry},
    error::Error,
    sync::atomic::{AtomicI32, AtomicU8, AtomicU32, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use qunet::server::ServerHandle;
use tracing::{debug, error, info};

use crate::{
    core::{
        gd_api::GDDifficulty,
        handler::ConnectionHandler,
        module::{ConfigurableModule, ModuleInitResult, ServerModule},
    },
    features::{
        database::{DatabaseResult, Db, FeaturedLevelModel},
        sheets_client::SheetsClient,
    },
    users::UsersModule,
};
#[cfg(feature = "discord")]
use {
    crate::{
        core::gd_api::GDApiClient,
        discord::{DiscordMessage, DiscordModule, hex_color_to_decimal},
    },
    poise::serenity_prelude::{CreateEmbed, CreateEmbedAuthor},
    tracing::warn,
};

mod config;
mod database;
mod sheets_client;

pub use database::PartialFeaturedLevelId;

#[derive(thiserror::Error, Debug)]
pub enum FeaturesError {
    #[error("{0}")]
    Db(#[from] database::DatabaseError),
}

pub struct FeaturesModule {
    db: Db,
    active_level: AtomicI32,
    active_level_tier: AtomicU8,
    active_level_edition: AtomicU32,
    feature_cycle_interval: Duration,
    sheets: Option<SheetsClient>,
    #[cfg(feature = "discord")]
    discord: Option<Arc<DiscordModule>>,
    users_module: Arc<UsersModule>,
    exhaust_notif_channel: u64,
    exhaust_notif_message: Option<String>,
    feature_notif_channel: u64,
    feature_notif_message: Option<String>,
}

pub struct FeaturedLevelMeta {
    pub id: i32,
    pub rate_tier: u8,
    pub edition: u32,
}

impl FeaturesModule {
    pub fn get_featured_level_meta(&self) -> FeaturedLevelMeta {
        let id = self.active_level.load(Ordering::Relaxed);
        let rate_tier = self.active_level_tier.load(Ordering::Relaxed);
        let edition = self.active_level_edition.load(Ordering::Relaxed);

        FeaturedLevelMeta { id, rate_tier, edition }
    }

    pub async fn get_featured_levels_page(
        &self,
        page: u32,
    ) -> Result<Vec<database::PartialFeaturedLevelId>, FeaturesError> {
        Ok(self.db.get_featured_level_ids_page(page).await?)
    }

    pub async fn get_featured_levels_total_pages(&self) -> Result<u32, FeaturesError> {
        Ok(self.db.get_featured_level_pages().await?)
    }

    pub async fn send_level(
        &self,
        sender_id: i32,
        level_id: i32,
        level_name: &str,
        author_id: i32,
        author_name: &str,
        rate_tier: u8,
        note: &str,
        queue: bool,
    ) -> Result<(), FeaturesError> {
        self.db
            .add_sent_level(
                sender_id,
                level_id,
                level_name,
                author_id,
                author_name,
                rate_tier,
                note,
                queue,
            )
            .await?;

        self.update_spreadsheet(false, queue, true).await;

        Ok(())
    }

    pub async fn set_feature_duration(
        &self,
        level_id: i32,
        duration: Duration,
    ) -> DatabaseResult<()> {
        self.db.set_feature_duration(level_id, duration.as_secs() as i32).await?;
        self.update_spreadsheet(true, true, false).await;

        Ok(())
    }

    pub async fn set_feature_priority(&self, level_id: i32, priority: i32) -> DatabaseResult<()> {
        self.db.set_feature_priority(level_id, priority).await?;
        self.update_spreadsheet(false, true, false).await;

        Ok(())
    }

    fn set_active_from(&self, level: &FeaturedLevelModel) {
        self.active_level.store(level.level_id, Ordering::Relaxed);
        self.active_level_edition.store(level.id as u32, Ordering::Relaxed);
        self.active_level_tier.store(level.rate_tier as u8, Ordering::Relaxed);
    }

    async fn reload_featured_level(&self) -> DatabaseResult<Option<FeaturedLevelModel>> {
        match self.db.get_featured_level().await? {
            Some(l) => {
                self.set_active_from(&l);
                Ok(Some(l))
            }

            None => {
                self.active_level.store(0, Ordering::Relaxed);
                self.active_level_edition.store(0, Ordering::Relaxed);
                self.active_level_tier.store(0, Ordering::Relaxed);
                Ok(None)
            }
        }
    }

    async fn update_featured_level(&self) {
        let level = match self.reload_featured_level().await {
            Ok(l) => l,

            Err(e) => {
                error!("failed to reload featured level: {e}");
                return;
            }
        };

        // don't cycle if interval is 0
        if self.feature_cycle_interval.is_zero() {
            return;
        }

        let expired = match &level {
            Some(level) => {
                let dur = level
                    .feature_duration
                    .map_or(self.feature_cycle_interval, |d| Duration::from_secs(d as u64));

                let until = (UNIX_EPOCH + Duration::from_secs(level.featured_at as u64) + dur)
                    .duration_since(SystemTime::now())
                    .unwrap_or_default();

                debug!(
                    "Featured level {} (edition {}) expires in {:?}",
                    level.level_id, level.id, until
                );

                until.is_zero()
            }

            None => true,
        };

        if expired {
            info!("Cycling featured level, current: {level:?}");

            match self.cycle_level().await {
                Ok(true) => {}
                Ok(false) => {
                    debug!("No queued levels to feature");

                    #[cfg(feature = "discord")]
                    if let Err(e) = self.notify_features_exhausted_discord().await {
                        warn!("failed to send discord msg: {e}");
                    }
                }
                Err(e) => {
                    error!("failed to cycle featured level: {e}")
                }
            }
        }
    }

    pub async fn cycle_level(&self) -> DatabaseResult<bool> {
        match self.db.cycle_next_queued_level().await {
            Ok(Some(level)) => {
                info!(
                    "Featured new level #{}: {} ({}) by {} ({})",
                    level.id, level.name, level.level_id, level.author_name, level.author
                );
                self.set_active_from(&level);
                self.update_spreadsheet(true, true, false).await;

                #[cfg(feature = "discord")]
                if let Err(e) = self.notify_new_featured_discord(&level).await {
                    warn!("failed to send new featured level notification: {e}");
                }

                Ok(true)
            }

            Ok(None) => Ok(false),

            Err(e) => Err(e),
        }
    }

    pub async fn update_spreadsheet(&self, featured: bool, queued: bool, sent: bool) {
        if let Err(e) = self.update_spreadsheet_inner(featured, queued, sent).await {
            error!("failed to update spreadsheet: {e}");
        }
    }

    async fn update_spreadsheet_inner(
        &self,
        featured: bool,
        queued: bool,
        sent: bool,
    ) -> Result<(), Box<dyn Error>> {
        let Some(sheets) = &self.sheets else {
            return Ok(());
        };

        if featured {
            let featured = self.db.get_all_featured_levels().await?;
            sheets.update_featured_sheet(featured).await?;
        }

        if queued {
            let queued = self.db.get_all_queued_levels().await?;
            sheets.update_queued_sheet(queued).await?;
        }

        if sent {
            let mut username_map = HashMap::new();
            let sent = self.db.get_all_sent_levels().await?;

            // build a map of all usernames .. lol
            for level in &sent {
                if let Entry::Vacant(e) = username_map.entry(level.sent_by) {
                    if let Some(user) = self.users_module.get_user(level.sent_by).await? {
                        e.insert(
                            user.username
                                .as_deref()
                                .map(|x| x.try_into().unwrap())
                                .unwrap_or_default(),
                        );
                    }
                }
            }

            sheets.update_sent_sheet(sent, username_map).await?;
        }

        Ok(())
    }

    #[cfg(feature = "discord")]
    async fn notify_new_featured_discord(&self, level: &FeaturedLevelModel) -> anyhow::Result<()> {
        let Some(discord) = &self.discord else {
            return Ok(());
        };

        if self.feature_notif_channel == 0 {
            return Ok(());
        }

        let difficulty = GDApiClient::new()
            .fetch_level(level.level_id)
            .await?
            .map_or(GDDifficulty::NA, |l| l.difficulty);

        discord
            .send_message(
                self.feature_notif_channel,
                DiscordMessage::new()
                    .content(self.feature_notif_message.as_deref().unwrap_or_default())
                    .add_embed(
                        CreateEmbed::new()
                            .author(CreateEmbedAuthor::new("New Featured Level"))
                            .title(format!("{} by {}", level.name, level.author_name))
                            .field("Level ID", level.level_id.to_string(), true)
                            .thumbnail(rate_tier_to_image(difficulty, level.rate_tier))
                            .color(hex_color_to_decimal("#4dace8")),
                    ),
            )
            .await?;

        Ok(())
    }

    #[cfg(feature = "discord")]
    async fn notify_features_exhausted_discord(&self) -> anyhow::Result<()> {
        let Some(discord) = &self.discord else {
            return Ok(());
        };

        if self.exhaust_notif_channel == 0 {
            return Ok(());
        }

        discord
            .send_message(
                self.exhaust_notif_channel,
                DiscordMessage::new()
                    .content(self.exhaust_notif_message.as_deref().unwrap_or_default()),
            )
            .await?;

        Ok(())
    }
}

impl ServerModule for FeaturesModule {
    async fn new(config: &config::Config, handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        let db = Db::new(&config.database_url, config.database_pool_size).await?;
        db.run_migrations().await?;

        let sheets = if config.google_credentials_path.is_some() && config.spreadsheet_id.is_some()
        {
            let creds = std::fs::read_to_string(config.google_credentials_path.as_ref().unwrap())?;

            Some(SheetsClient::new(&creds, config.spreadsheet_id.clone().unwrap()).await)
        } else {
            None
        };

        #[cfg(feature = "discord")]
        let discord = handler.opt_module_owned::<DiscordModule>();

        let out = Self {
            db,
            active_level: AtomicI32::new(0),
            active_level_tier: AtomicU8::new(0),
            active_level_edition: AtomicU32::new(0),
            feature_cycle_interval: Duration::from_secs(config.feature_cycle_interval as u64),
            sheets,
            #[cfg(feature = "discord")]
            discord,
            users_module: handler.opt_module_owned::<UsersModule>().unwrap(),
            exhaust_notif_channel: config.exhaust_notif_channel,
            exhaust_notif_message: config.exhaust_notif_message.clone(),
            feature_notif_channel: config.feature_notif_channel,
            feature_notif_message: config.feature_notif_message.clone(),
        };

        out.update_featured_level().await;

        Ok(out)
    }

    fn id() -> &'static str {
        "featured-levels"
    }

    fn name() -> &'static str {
        "Featured Levels"
    }

    fn on_launch(&self, server: &ServerHandle<ConnectionHandler>) {
        server.schedule(Duration::from_mins(15), async |server| {
            server.handler().module::<Self>().update_featured_level().await;
        });

        server.schedule(Duration::from_hours(12), async |server| {
            server.handler().module::<Self>().update_spreadsheet(true, true, true).await;
        });
    }
}

impl ConfigurableModule for FeaturesModule {
    type Config = config::Config;
}

fn rate_tier_to_image(difficulty: GDDifficulty, tier: i32) -> String {
    let diffname: &str = match difficulty {
        GDDifficulty::Easy => "easy",
        GDDifficulty::Normal => "normal",
        GDDifficulty::Hard => "hard",
        GDDifficulty::Harder => "harder",
        GDDifficulty::Insane => "insane",
        GDDifficulty::Demon
        | GDDifficulty::DemonEasy
        | GDDifficulty::DemonMedium
        | GDDifficulty::DemonInsane
        | GDDifficulty::DemonExtreme => "harddemon",
        _ => "na",
    };

    let ratename: &str = match tier {
        1 => "epic",
        2 => "outstanding",
        _ => "featured",
    };

    format!(
        // TODO (release): replace this when we merge globed3
        // "https://raw.githubusercontent.com/GlobedGD/globed2/main/resources/_raw/globed-faces/{diffname}/{diffname}-{ratename}.png"
        "https://raw.githubusercontent.com/GlobedGD/globed2/main/assets/globed-faces/{diffname}/{diffname}-{ratename}.png"
    )
}
