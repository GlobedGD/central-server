use crate::core::module::{ModuleInitResult, ServerModule};

mod config;
mod database;

pub use config::Config;
use database::UsersDb;

pub struct UsersModule {
    db: UsersDb,
}

impl UsersModule {}

impl ServerModule for UsersModule {
    type Config = Config;

    async fn new(config: &Self::Config) -> ModuleInitResult<Self> {
        let db = UsersDb::new(&config.database_url, config.database_pool_size).await?;
        db.run_migrations().await?;

        Ok(Self { db })
    }

    fn id() -> &'static str {
        "users"
    }

    fn name() -> &'static str {
        "User management"
    }
}
