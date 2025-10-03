use std::{
    num::NonZeroI64,
    time::{SystemTime, UNIX_EPOCH},
};

use sea_orm::{ActiveValue::NotSet, FromQueryResult, QueryOrder, QuerySelect};
use thiserror::Error;
use {
    sea_orm::{
        ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectOptions, Database,
        DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, prelude::*,
    },
    sea_orm_migration::MigratorTrait,
};

use migration::Migrator;

mod entities;
mod migration;

pub use entities::prelude::*;
use entities::*;

pub type FeaturedLevelModel = featured_level::Model;
pub type QueuedLevelModel = queued_level::Model;
pub type SentLevelModel = sent_level::Model;

const FEATURE_PAGE_SIZE: u64 = 25;

#[derive(DerivePartialModel, FromQueryResult)]
#[sea_orm(entity = "FeaturedLevel")]
#[repr(transparent)]
pub struct PartialFeaturedLevelId {
    #[sea_orm(from_col = "id")]
    pub id: i32,
}

#[derive(Error, Debug)]
pub enum DatabaseError {
    #[cfg(feature = "database")]
    #[error("Database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("Level already was featured")]
    AlreadyFeatured,
    #[error("Level already was queued")]
    AlreadyQueued,
    #[error("Level not found")]
    NotFound,
}

pub type DatabaseResult<T> = Result<T, DatabaseError>;

pub struct Db {
    conn: DatabaseConnection,
}

fn timestamp() -> NonZeroI64 {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    NonZeroI64::new(now).unwrap()
}

impl Db {
    pub async fn new(url: &str, pool_size: u32) -> DatabaseResult<Self> {
        let mut opt = ConnectOptions::new(url);
        opt.max_connections(pool_size).min_connections(1);

        let db = Database::connect(opt).await?;

        Ok(Self { conn: db })
    }

    pub async fn run_migrations(&self) -> DatabaseResult<()> {
        Migrator::up(&self.conn, None).await?;
        Ok(())
    }

    // Featured levels

    pub async fn get_featured_level_id(&self) -> DatabaseResult<Option<i32>> {
        // find the last featured level
        let level = FeaturedLevel::find()
            .order_by_desc(featured_level::Column::FeaturedAt)
            .one(&self.conn)
            .await?;

        Ok(level.map(|x| x.id))
    }

    pub async fn get_featured_level(&self) -> DatabaseResult<Option<featured_level::Model>> {
        // find the last featured level
        Ok(FeaturedLevel::find()
            .order_by_desc(featured_level::Column::FeaturedAt)
            .one(&self.conn)
            .await?)
    }

    pub async fn get_all_featured_levels(&self) -> DatabaseResult<Vec<featured_level::Model>> {
        Ok(FeaturedLevel::find()
            .order_by_asc(featured_level::Column::FeaturedAt)
            .all(&self.conn)
            .await?)
    }

    pub async fn get_all_queued_levels(&self) -> DatabaseResult<Vec<queued_level::Model>> {
        Ok(QueuedLevel::find()
            .order_by_desc(queued_level::Column::Priority)
            .all(&self.conn)
            .await?)
    }

    pub async fn get_all_sent_levels(&self) -> DatabaseResult<Vec<sent_level::Model>> {
        Ok(SentLevel::find().order_by_asc(sent_level::Column::Id).all(&self.conn).await?)
    }

    pub async fn get_featured_level_ids_page(
        &self,
        page: u32,
    ) -> DatabaseResult<Vec<PartialFeaturedLevelId>> {
        let levels = FeaturedLevel::find()
            .order_by_desc(featured_level::Column::FeaturedAt)
            .limit(FEATURE_PAGE_SIZE)
            .offset(page as u64 * FEATURE_PAGE_SIZE)
            .into_partial_model::<PartialFeaturedLevelId>()
            .all(&self.conn)
            .await?;

        Ok(levels)
    }

    pub async fn get_featured_level_pages(&self) -> DatabaseResult<u32> {
        let count = FeaturedLevel::find().count(&self.conn).await?;

        Ok((count as f32 / FEATURE_PAGE_SIZE as f32).ceil() as u32)
    }

    pub async fn cycle_next_queued_level(&self) -> DatabaseResult<Option<featured_level::Model>> {
        // pick the level with highest priority, using id as tiebreaker
        let queued = QueuedLevel::find()
            .order_by_desc(queued_level::Column::Priority)
            .order_by_asc(queued_level::Column::Id)
            .one(&self.conn)
            .await?;

        let Some(queued) = queued else {
            return Ok(None);
        };

        // delete from queue
        QueuedLevel::delete_by_id(queued.id).exec(&self.conn).await?;

        Ok(Some(self.add_featured_level_from_queued(queued).await?))
    }

    async fn add_featured_level_from_queued(
        &self,
        level: queued_level::Model,
    ) -> DatabaseResult<featured_level::Model> {
        let new = featured_level::ActiveModel {
            id: Set(level.id),
            name: Set(level.name),
            author: Set(level.author),
            author_name: Set(level.author_name),
            featured_at: Set(timestamp().get()),
            rate_tier: Set(level.rate_tier),
            feature_duration: Set(level.feature_duration),
        };

        Ok(new.insert(&self.conn).await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_sent_level(
        &self,
        sender_id: i32,
        level_id: i32,
        level_name: &str,
        author_id: i32,
        author_name: &str,
        rate_tier: u8,
        note: &str,
        queue: bool,
    ) -> DatabaseResult<()> {
        // check if already featured
        if self.was_featured(level_id).await? {
            return Err(DatabaseError::AlreadyFeatured);
        }

        let model = sent_level::ActiveModel {
            id: NotSet,
            level_id: Set(level_id),
            name: Set(level_name.to_string()),
            author: Set(author_id),
            author_name: Set(author_name.to_string()),
            note: Set(note.to_string()),
            rate_tier: Set(rate_tier as i32),
            sent_by: Set(sender_id),
        };

        model.insert(&self.conn).await?;

        if queue {
            // check if this level has already been queued
            if self.was_queued(level_id).await? {
                return Err(DatabaseError::AlreadyQueued);
            }

            let queued = queued_level::ActiveModel {
                id: Set(level_id),
                priority: Set(0),
                name: Set(level_name.to_string()),
                author: Set(author_id),
                author_name: Set(author_name.to_string()),
                rate_tier: Set(rate_tier as i32),
                feature_duration: Set(None),
            };

            queued.insert(&self.conn).await?;

            self.remove_sends_for(level_id).await?;
        }

        Ok(())
    }

    pub async fn set_feature_duration(&self, level_id: i32, duration: i32) -> DatabaseResult<()> {
        if let Some(level) = FeaturedLevel::find_by_id(level_id).one(&self.conn).await? {
            let mut model = level.into_active_model();
            model.feature_duration = Set(Some(duration));
            model.update(&self.conn).await?;
            Ok(())
        } else if let Some(level) = QueuedLevel::find_by_id(level_id).one(&self.conn).await? {
            let mut model = level.into_active_model();
            model.feature_duration = Set(Some(duration));
            model.update(&self.conn).await?;
            Ok(())
        } else {
            Err(DatabaseError::NotFound)
        }
    }

    pub async fn set_feature_priority(&self, level_id: i32, priority: i32) -> DatabaseResult<()> {
        if let Some(level) = QueuedLevel::find_by_id(level_id).one(&self.conn).await? {
            let mut model = level.into_active_model();
            model.priority = Set(priority);
            model.update(&self.conn).await?;
            Ok(())
        } else {
            Err(DatabaseError::NotFound)
        }
    }

    pub async fn was_featured(&self, level_id: i32) -> DatabaseResult<bool> {
        Ok(FeaturedLevel::find_by_id(level_id).one(&self.conn).await?.is_some())
    }

    pub async fn was_queued(&self, level_id: i32) -> DatabaseResult<bool> {
        Ok(QueuedLevel::find_by_id(level_id).one(&self.conn).await?.is_some())
    }

    pub async fn remove_sends_for(&self, level_id: i32) -> DatabaseResult<()> {
        SentLevel::delete_many()
            .filter(sent_level::Column::LevelId.eq(level_id))
            .exec(&self.conn)
            .await?;

        Ok(())
    }
}
