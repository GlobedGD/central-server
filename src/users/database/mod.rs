use sea_orm::{ConnectOptions, Database, DatabaseConnection, EntityTrait};
use sea_orm_migration::MigratorTrait;
use thiserror::Error;

mod entities;
mod migrations;

pub use entities::prelude::*;
use entities::*;
use migrations::Migrator;

#[derive(Error, Debug)]
pub enum DatabaseError {
    #[error("Database error: {0}")]
    Db(#[from] sea_orm::DbErr),
}

pub type DatabaseResult<T> = Result<T, DatabaseError>;

pub struct UsersDb {
    // slightly misleading name but this is a connection pool, not a single connection
    conn: DatabaseConnection,
}

impl UsersDb {
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

    pub async fn get_user(&self, account_id: i32) -> DatabaseResult<Option<user::Model>> {
        let user = User::find_by_id(account_id).one(&self.conn).await?;
        Ok(user)
    }

    pub async fn run<R, F>(&self, f: F) -> DatabaseResult<R>
    where
        F: AsyncFnOnce(&DatabaseConnection) -> DatabaseResult<R>,
    {
        f(&self.conn).await
    }
}
