use std::{
    num::NonZeroI64,
    time::{SystemTime, UNIX_EPOCH},
};

use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectOptions, Database, DatabaseConnection,
    EntityTrait, IntoActiveModel, QueryFilter, prelude::*,
};
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
    #[error("Invalid punishment type in the database")]
    InvalidPunishmentType,
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

    pub async fn get_user(&self, account_id: i32) -> DatabaseResult<Option<DbUser>> {
        let user = User::find_by_id(account_id).one(&self.conn).await?;

        let Some(model) = user else {
            return Ok(None);
        };

        let mut user = DbUser {
            account_id: model.account_id,
            username: model.username.clone(),
            name_color: model.name_color.clone(),
            is_whitelisted: model.is_whitelisted,
            admin_password_hash: model.admin_password_hash.clone(),
            roles: model.roles.clone(),
            active_mute: None,
            active_ban: None,
            active_room_ban: None,
        };

        if let Some(id) = model.active_mute {
            user.active_mute = self.get_punishment(id).await?;
        }

        if let Some(id) = model.active_ban {
            user.active_ban = self.get_punishment(id).await?;
        }

        if let Some(id) = model.active_room_ban {
            user.active_room_ban = self.get_punishment(id).await?;
        }

        if self.expire_punishments(&mut user) {
            let mut active = model.into_active_model();

            active.active_mute = Set(user.active_mute.as_ref().map(|x| x.id));
            active.active_ban = Set(user.active_ban.as_ref().map(|x| x.id));
            active.active_room_ban = Set(user.active_room_ban.as_ref().map(|x| x.id));

            active.update(&self.conn).await?;
        }

        Ok(Some(user))
    }

    pub async fn get_punishment(&self, id: i32) -> DatabaseResult<Option<UserPunishment>> {
        let punishment = Punishment::find_by_id(id).one(&self.conn).await?;

        Ok(match punishment {
            None => None,
            Some(p) => Some(UserPunishment {
                id: p.id,
                account_id: p.account_id,
                r#type: match p.r#type.as_str() {
                    "mute" => UserPunishmentType::Mute,
                    "ban" => UserPunishmentType::Ban,
                    "roomban" => UserPunishmentType::RoomBan,
                    _ => return Err(DatabaseError::InvalidPunishmentType),
                },
                reason: p.reason,
                expires_at: NonZeroI64::new(p.expires_at.unwrap_or_default()),
                issued_by: p.issued_by,
                issued_at: NonZeroI64::new(p.issued_at.unwrap_or_default()),
            }),
        })
    }

    pub async fn update_username(&self, account_id: i32, new_username: &str) -> DatabaseResult<()> {
        User::update_many()
            .filter(user::Column::AccountId.eq(account_id))
            .col_expr(user::Column::Username, Expr::value(new_username))
            .exec(&self.conn)
            .await?;

        Ok(())
    }

    /// Returns whether the user was modified
    fn expire_punishments(&self, user: &mut DbUser) -> bool {
        let mut modified = false;

        let punishments = [&mut user.active_mute, &mut user.active_ban, &mut user.active_room_ban];
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;

        for pun in punishments {
            if let Some(p) = pun
                && let Some(exp) = p.expires_at
                && exp.get() <= timestamp
            {
                modified = true;
                *pun = None;
            }
        }

        modified
    }

    pub async fn run<R, F>(&self, f: F) -> DatabaseResult<R>
    where
        F: AsyncFnOnce(&DatabaseConnection) -> DatabaseResult<R>,
    {
        f(&self.conn).await
    }
}

pub enum UserPunishmentType {
    Mute,
    Ban,
    RoomBan,
}

pub struct UserPunishment {
    pub id: i32,
    pub account_id: i32,
    pub r#type: UserPunishmentType,
    pub reason: String,
    pub expires_at: Option<NonZeroI64>,
    pub issued_by: i32,
    pub issued_at: Option<NonZeroI64>,
}

pub struct DbUser {
    pub account_id: i32,
    pub username: Option<String>,
    pub name_color: Option<String>,
    pub is_whitelisted: bool,
    pub admin_password_hash: Option<String>,
    pub roles: Option<String>,
    pub active_mute: Option<UserPunishment>,
    pub active_ban: Option<UserPunishment>,
    pub active_room_ban: Option<UserPunishment>,
}

impl DbUser {
    pub fn is_muted(&self) -> bool {
        self.active_mute.is_some()
    }

    pub fn is_banned(&self) -> bool {
        self.active_ban.is_some()
    }

    pub fn is_room_banned(&self) -> bool {
        self.active_room_ban.is_some()
    }
}
