use std::num::NonZeroI64;

use sea_orm::{QueryOrder, QuerySelect};
use thiserror::Error;
#[cfg(feature = "database")]
use {
    sea_orm::{
        ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectOptions, Database,
        DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, prelude::*,
    },
    sea_orm_migration::MigratorTrait,
    std::time::{SystemTime, UNIX_EPOCH},
};

mod log_action;
pub use audit_log::Model as AuditLogModel;
pub use log_action::LogAction;

#[allow(warnings)]
#[cfg(feature = "database")]
mod entities;
#[cfg(feature = "database")]
mod migrations;

#[cfg(feature = "database")]
pub use entities::prelude::*;
#[cfg(feature = "database")]
use entities::*;
#[cfg(feature = "database")]
use migrations::Migrator;

#[derive(Error, Debug)]
pub enum DatabaseError {
    #[cfg(feature = "database")]
    #[error("Database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("Invalid punishment type in the database")]
    InvalidPunishmentType,
}

pub type DatabaseResult<T> = Result<T, DatabaseError>;

pub struct UsersDb {
    // slightly misleading name but this is a connection pool, not a single connection
    #[cfg(feature = "database")]
    conn: DatabaseConnection,
}

fn timestamp() -> NonZeroI64 {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    NonZeroI64::new(now).unwrap()
}

impl UsersDb {
    #[cfg(feature = "database")]
    pub async fn new(url: &str, pool_size: u32) -> DatabaseResult<Self> {
        let mut opt = ConnectOptions::new(url);
        opt.max_connections(pool_size).min_connections(1);

        let db = Database::connect(opt).await?;

        Ok(Self { conn: db })
    }

    #[cfg(not(feature = "database"))]
    pub async fn new(_url: &str, _pool_size: u32) -> DatabaseResult<Self> {
        Ok(Self {})
    }

    #[cfg(feature = "database")]
    pub async fn run_migrations(&self) -> DatabaseResult<()> {
        Migrator::up(&self.conn, None).await?;
        Ok(())
    }

    #[cfg(not(feature = "database"))]
    pub async fn run_migrations(&self) -> DatabaseResult<()> {
        Ok(())
    }

    #[cfg(feature = "database")]
    pub async fn get_user(&self, account_id: i32) -> DatabaseResult<Option<DbUser>> {
        let user = User::find_by_id(account_id).one(&self.conn).await?;

        let Some(model) = user else {
            return Ok(None);
        };

        Ok(Some(self.post_user_fetch(model).await?))
    }

    #[cfg(not(feature = "database"))]
    pub async fn get_user(&self, _account_id: i32) -> DatabaseResult<Option<DbUser>> {
        Ok(None)
    }

    #[cfg(feature = "database")]
    pub async fn query_user(&self, query: &str) -> DatabaseResult<Option<DbUser>> {
        // if it's an integer, try fetch by ID

        let mut user = None;
        if let Ok(id) = query.parse::<i32>() {
            user = User::find_by_id(id).one(&self.conn).await?;
        };

        // if that didn't work, try exact username match
        if user.is_none() {
            user = User::find().filter(user::Column::Username.eq(query)).one(&self.conn).await?;
        }

        // if that didn't work either, try a contains match
        if user.is_none() {
            user =
                User::find().filter(user::Column::Username.contains(query)).one(&self.conn).await?;
        }

        match user {
            Some(x) => Ok(Some(self.post_user_fetch(x).await?)),
            None => Ok(None),
        }
    }

    #[cfg(not(feature = "database"))]
    pub async fn query_user(&self, query: &str) -> DatabaseResult<Option<DbUser>> {
        Ok(None)
    }

    #[cfg(feature = "database")]
    pub async fn post_user_fetch(&self, model: user::Model) -> DatabaseResult<DbUser> {
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

        Ok(user)
    }

    #[cfg(feature = "database")]
    pub async fn get_punishment(&self, id: i32) -> DatabaseResult<Option<UserPunishment>> {
        let punishment = Punishment::find_by_id(id).one(&self.conn).await?;

        Ok(match punishment {
            None => None,
            Some(p) => Some(UserPunishment {
                id: p.id,
                account_id: p.account_id,
                r#type: match p.r#type.as_deref().unwrap_or_default() {
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

    #[cfg(not(feature = "database"))]
    pub async fn get_punishment(&self, _id: i32) -> DatabaseResult<Option<UserPunishment>> {
        Ok(None)
    }

    #[cfg(feature = "database")]
    pub async fn update_username(&self, account_id: i32, new_username: &str) -> DatabaseResult<()> {
        let result = User::update_many()
            .filter(user::Column::AccountId.eq(account_id))
            .col_expr(user::Column::Username, Expr::value(new_username))
            .exec(&self.conn)
            .await?;

        if result.rows_affected == 0 {
            // user does not exist, insert a new one
            let new_user = user::ActiveModel {
                account_id: Set(account_id),
                username: Set(Some(new_username.to_owned())),
                is_whitelisted: Set(false),
                ..Default::default()
            };

            new_user.insert(&self.conn).await?;
        }

        Ok(())
    }

    #[cfg(not(feature = "database"))]
    pub async fn update_username(&self, _: i32, _: &str) -> DatabaseResult<()> {
        Ok(())
    }

    #[cfg(feature = "database")]
    pub async fn update_icons(
        &self,
        account_id: i32,
        cube: i16,
        color1: u16,
        color2: u16,
        glow_color: u16,
    ) -> DatabaseResult<()> {
        User::update_many()
            .filter(user::Column::AccountId.eq(account_id))
            .col_expr(user::Column::Cube, Expr::value(cube))
            .col_expr(user::Column::Color1, Expr::value(color1))
            .col_expr(user::Column::Color2, Expr::value(color2))
            .col_expr(user::Column::GlowColor, Expr::value(glow_color))
            .exec(&self.conn)
            .await?;

        Ok(())
    }

    #[cfg(feature = "database")]
    pub async fn fetch_all_with_roles(&self) -> DatabaseResult<Vec<user::Model>> {
        Ok(User::find().filter(user::Column::Roles.is_not_null()).all(&self.conn).await?)
    }

    /// Returns whether the user was modified
    #[cfg(feature = "database")]
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

    #[cfg(feature = "database")]
    pub async fn get_admin_password_hash(&self, account_id: i32) -> DatabaseResult<Option<String>> {
        let user = User::find_by_id(account_id).one(&self.conn).await?;

        Ok(user.and_then(|u| u.admin_password_hash))
    }

    #[cfg(not(feature = "database"))]
    pub async fn get_admin_password_hash(&self, _: i32) -> DatabaseResult<Option<String>> {
        Ok(None)
    }

    #[cfg(feature = "database")]
    pub async fn set_admin_password_hash(&self, account_id: i32, hash: &str) -> DatabaseResult<()> {
        User::update_many()
            .filter(user::Column::AccountId.eq(account_id))
            .col_expr(user::Column::AdminPasswordHash, Expr::value(hash))
            .exec(&self.conn)
            .await?;

        Ok(())
    }

    #[cfg(not(feature = "database"))]
    pub async fn set_admin_password_hash(&self, _: i32, _: &str) -> DatabaseResult<()> {
        Ok(())
    }

    #[cfg(feature = "database")]
    pub async fn get_punishment_count(&self, account_id: i32) -> DatabaseResult<u32> {
        let count = Punishment::find()
            .filter(punishment::Column::AccountId.eq(account_id))
            .count(&self.conn)
            .await?;

        Ok(count as u32)
    }

    #[cfg(not(feature = "database"))]
    pub async fn get_punishment_count(&self, account_id: i32) -> DatabaseResult<u32> {
        Ok(0)
    }

    /// Punish a user, returns whether the user was already punished and the punishment was updated.
    /// If the user does not exist, it will return `Ok(None)`.
    pub async fn punish_user(
        &self,
        issuer_id: i32,
        account_id: i32,
        r#type: UserPunishmentType,
        reason: &str,
        expires_at: Option<NonZeroI64>,
    ) -> DatabaseResult<Option<bool>> {
        // check if the user exists and already has a punishment
        let Some(user) = self.get_user(account_id).await? else {
            return Ok(None);
        };

        let active_pun = match r#type {
            UserPunishmentType::Mute => user.active_mute,
            UserPunishmentType::Ban => user.active_ban,
            UserPunishmentType::RoomBan => user.active_room_ban,
        };

        let updating = active_pun.is_some();
        let mut punishment = active_pun.unwrap_or_else(|| UserPunishment {
            id: 0,
            account_id,
            r#type,
            reason: String::new(),
            expires_at: None,
            issued_by: 0,
            issued_at: None,
        });

        punishment.reason = reason.to_owned();
        punishment.expires_at = expires_at;
        punishment.issued_by = issuer_id;
        punishment.issued_at = Some(timestamp());

        self.insert_or_update_punishment(punishment, updating).await?;

        Ok(Some(updating))
    }

    #[cfg(feature = "database")]
    pub async fn unpunish_user(
        &self,
        account_id: i32,
        r#type: UserPunishmentType,
    ) -> DatabaseResult<()> {
        self.update_active_punishment(account_id, r#type, None).await
    }

    #[cfg(feature = "database")]
    async fn insert_or_update_punishment(
        &self,
        p: UserPunishment,
        updating: bool,
    ) -> DatabaseResult<()> {
        let pun = punishment::ActiveModel {
            id: if updating { Set(p.id) } else { Set(0) },
            account_id: Set(p.account_id),
            r#type: Set(Some(
                match p.r#type {
                    UserPunishmentType::Mute => "mute",
                    UserPunishmentType::Ban => "ban",
                    UserPunishmentType::RoomBan => "roomban",
                }
                .to_owned(),
            )),
            reason: Set(p.reason),
            expires_at: Set(p.expires_at.map(|x| x.get())),
            issued_by: Set(p.issued_by),
            issued_at: Set(p.issued_at.map(|x| x.get())),
        };

        let pun_id = if updating {
            pun.update(&self.conn).await?.id
        } else {
            pun.insert(&self.conn).await?.id
        };

        // update active mute / ban / room ban
        self.update_active_punishment(p.account_id, p.r#type, Some(pun_id)).await?;

        Ok(())
    }

    async fn update_active_punishment(
        &self,
        account_id: i32,
        punishment_type: UserPunishmentType,
        punishment_id: Option<i32>,
    ) -> DatabaseResult<()> {
        let stmt = User::update_many().filter(user::Column::AccountId.eq(account_id));

        let stmt = match punishment_type {
            UserPunishmentType::Mute => {
                stmt.col_expr(user::Column::ActiveMute, Expr::value(punishment_id))
            }

            UserPunishmentType::Ban => {
                stmt.col_expr(user::Column::ActiveBan, Expr::value(punishment_id))
            }

            UserPunishmentType::RoomBan => {
                stmt.col_expr(user::Column::ActiveRoomBan, Expr::value(punishment_id))
            }
        };

        stmt.exec(&self.conn).await?;

        Ok(())
    }

    pub async fn update_roles(&self, account_id: i32, roles: &str) -> DatabaseResult<()> {
        User::update_many()
            .filter(user::Column::AccountId.eq(account_id))
            .col_expr(user::Column::Roles, Expr::value(roles))
            .exec(&self.conn)
            .await?;

        Ok(())
    }

    pub async fn fetch_logs(
        &self,
        issuer: i32,
        target: i32,
        r#type: &str,
        before: i64,
        after: i64,
        page: u32,
    ) -> DatabaseResult<Vec<audit_log::Model>> {
        let mut stmt = AuditLog::find();

        if issuer != 0 {
            stmt = stmt.filter(audit_log::Column::AccountId.eq(issuer))
        }

        if target != 0 {
            stmt = stmt.filter(audit_log::Column::TargetAccountId.eq(target))
        }

        if !r#type.is_empty() {
            stmt = stmt.filter(audit_log::Column::Type.eq(r#type))
        }

        if before != 0 {
            stmt = stmt.filter(audit_log::Column::Timestamp.lt(before))
        }

        if after != 0 {
            stmt = stmt.filter(audit_log::Column::Timestamp.gte(after))
        }

        stmt = stmt.order_by_desc(audit_log::Column::Id).limit(50).offset(page as u64 * 50);

        let results: Vec<audit_log::Model> = stmt.all(&self.conn).await?;

        Ok(results)
    }

    #[cfg(feature = "database")]
    pub async fn log_action(&self, account_id: i32, action: LogAction<'_>) -> DatabaseResult<()> {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;

        let mut entry = audit_log::ActiveModel {
            account_id: Set(account_id),
            r#type: Set(action.type_str().to_owned()),
            timestamp: Set(timestamp),
            target_account_id: Set(Some(action.account_id())),
            ..Default::default()
        };

        match action {
            LogAction::Kick { reason, .. } => {
                entry.message = Set(Some(reason.to_owned()));
            }

            LogAction::Notice { message, .. } => {
                entry.message = Set(Some(message.to_owned()));
            }

            LogAction::Ban { reason, expires_at, .. }
            | LogAction::Mute { reason, expires_at, .. }
            | LogAction::EditBan { reason, expires_at, .. }
            | LogAction::EditMute { reason, expires_at, .. }
            | LogAction::RoomBan { reason, expires_at, .. }
            | LogAction::EditRoomBan { reason, expires_at, .. } => {
                entry.message = Set(Some(reason.to_owned()));
                entry.expires_at = Set(NonZeroI64::new(expires_at).map(|x| x.get()));
            }

            LogAction::Unban { .. } | LogAction::Unmute { .. } | LogAction::RoomUnban { .. } => {
                // no extra fields
            }

            LogAction::EditRoles { rolediff, .. } => {
                entry.message = Set(Some(rolediff.to_owned()));
            }

            LogAction::EditPassword { .. } => {
                // no extra fields
            }
        }

        entry.insert(&self.conn).await?;

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
