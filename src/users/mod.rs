use std::{cmp::Reverse, collections::HashSet, fmt::Write, num::NonZeroI64};

#[cfg(feature = "database")]
use crate::{auth::ClientAccountData, users::database::AuditLogModel};
use crate::{
    core::module::{ModuleInitResult, ServerModule},
    users::{config::Role, database::LogAction},
};

mod config;
mod database;
mod pwhash;

pub use config::Config;
use database::UsersDb;
pub use database::{DatabaseError, DatabaseResult, DbUser, UserPunishment, UserPunishmentType};
use thiserror::Error;
use tracing::warn;

#[derive(Error, Debug)]
pub enum PunishUserError {
    #[error("{0}")]
    Database(#[from] DatabaseError),
    #[error("User not found")]
    NotFound,
    #[error("Insufficient permissions")]
    Permissions,
}

#[derive(Default)]
pub struct ComputedRole {
    pub priority: i32,
    pub roles: heapless::Vec<u8, 32>,
    pub name_color: String,

    pub can_kick: bool,
    pub can_mute: bool,
    pub can_ban: bool,
    pub can_set_password: bool,
    pub can_notice_everyone: bool,
}

impl ComputedRole {
    pub fn can_moderate(&self) -> bool {
        self.can_kick
            || self.can_mute
            || self.can_ban
            || self.can_set_password
            || self.can_notice_everyone
    }
}

pub struct UsersModule {
    db: UsersDb,
    roles: Vec<Role>,       // index = numeric role ID
    super_admins: Vec<i32>, // account IDs of super admins
}

impl UsersModule {
    pub async fn get_user(&self, account_id: i32) -> DatabaseResult<Option<DbUser>> {
        self.db.get_user(account_id).await
    }

    pub async fn update_username(&self, account_id: i32, new_username: &str) -> DatabaseResult<()> {
        self.db.update_username(account_id, new_username).await
    }

    pub fn get_role(&self, id: u8) -> Option<&Role> {
        self.roles.get(id as usize)
    }

    pub fn get_role_by_str_id(&self, id: &str) -> Option<(usize, &Role)> {
        self.roles.iter().enumerate().find(|(_, role)| role.id == id)
    }

    pub fn get_roles(&self) -> &[Role] {
        &self.roles
    }

    /// Converts a comma-separated string of string role IDs into a vector of numeric IDs
    pub fn role_str_to_ids(&self, roles: &str) -> Vec<u8> {
        roles
            .split(',')
            .filter(|s| !s.is_empty())
            .filter_map(|id| self.get_role_by_str_id(id))
            .map(|(index, _)| index as u8)
            .collect()
    }

    pub fn compute_from_roles<'a>(
        &'a self,
        account_id: i32,
        mut iter: impl Iterator<Item = &'a str>,
    ) -> ComputedRole {
        // super admin has the highest possible priority and perms
        if self.super_admins.contains(&account_id) {
            return ComputedRole {
                priority: i32::MAX,
                can_kick: true,
                can_mute: true,
                can_ban: true,
                can_set_password: true,
                can_notice_everyone: true,
                ..Default::default()
            };
        }

        // start with a baseline user role with minimum priority and no permissions
        let mut out_role = ComputedRole {
            priority: i32::MIN,
            ..Default::default()
        };

        let mut can_mute = None;
        let mut can_kick = None;
        let mut can_ban = None;
        let mut can_set_password = None;
        let mut can_notice_everyone = None;

        while let Some((role_index, role)) = iter.next().and_then(|s| self.get_role_by_str_id(s)) {
            // determine if this role is stronger than the current strongest role
            let is_weaker = role.priority < out_role.priority;

            let apply_permission = |out: &mut Option<bool>, r#in: Option<bool>| {
                if let Some(val) = r#in {
                    // assign only if the current value is None or if the new role is stronger
                    if out.is_none() || !is_weaker {
                        *out = Some(val);
                    }
                }
            };

            apply_permission(&mut can_mute, role.can_mute);
            apply_permission(&mut can_kick, role.can_kick);
            apply_permission(&mut can_ban, role.can_ban);
            apply_permission(&mut can_set_password, role.can_set_password);
            apply_permission(&mut can_notice_everyone, role.can_notice_everyone);

            out_role.priority = role.priority;
            let _ = out_role.roles.push(role_index as u8);

            if !is_weaker {
                out_role.name_color.clone_from(&role.name_color);
            }
        }

        out_role.can_mute = can_mute.unwrap_or(false);
        out_role.can_kick = can_kick.unwrap_or(false);
        out_role.can_ban = can_ban.unwrap_or(false);
        out_role.can_set_password = can_set_password.unwrap_or(false);
        out_role.can_notice_everyone = can_notice_everyone.unwrap_or(false);

        // sort roles by priority descending
        out_role.roles.sort_unstable_by_key(|&id| {
            Reverse(self.get_role(id).map_or(i32::MIN, |r| r.priority))
        });

        out_role
    }

    pub fn compute_from_user(&self, user: &DbUser) -> ComputedRole {
        self.compute_from_roles(
            user.account_id,
            user.roles.as_deref().unwrap_or("").split(',').filter(|x| !x.is_empty()),
        )
    }

    /// Converts a slice of role IDs into a comma-separated string of string IDs
    pub fn make_role_string(&self, roles: &[u8]) -> String {
        if roles.is_empty() {
            return String::new();
        }

        itertools::join(roles.iter().filter_map(|id| self.get_role(*id).map(|role| &role.id)), ",")
    }

    // Moderation utilities

    pub async fn admin_login(&self, account_id: i32, password: &str) -> DatabaseResult<bool> {
        // super admins can log in without a password
        if self.super_admins.contains(&account_id) {
            return Ok(true);
        }

        let hash = self.db.get_admin_password_hash(account_id).await?;

        Ok(hash.map(|hash| pwhash::verify(password, &hash)).unwrap_or(false))
    }

    pub async fn admin_edit_roles(
        &self,
        issuer_id: i32,
        account_id: i32,
        new_roles: &[u8],
    ) -> DatabaseResult<()> {
        // get the previous roles to compute the diff
        let prev_roles = match self.get_user(account_id).await? {
            Some(user) => user.roles.unwrap_or_default(),
            None => String::new(),
        };

        let prev_roles = self.role_str_to_ids(&prev_roles);

        let prev_set: HashSet<u8> = prev_roles.iter().copied().collect();
        let new_set: HashSet<u8> = new_roles.iter().copied().collect();
        let mut rolediff = String::new();

        for role in new_set.difference(&prev_set) {
            write!(rolediff, "+{},", role).unwrap();
        }

        for role in prev_set.difference(&new_set) {
            write!(rolediff, "-{},", role).unwrap();
        }

        // remove the last comma if it exists
        if rolediff.ends_with(',') {
            rolediff.pop();
        }

        // construct the new role string
        let new_role_string = self.make_role_string(new_roles);

        // update the user
        self.db.update_roles(account_id, &new_role_string).await?;

        // log
        self.db
            .log_action(
                issuer_id,
                LogAction::EditRoles {
                    account_id,
                    rolediff: &rolediff,
                },
            )
            .await?;

        Ok(())
    }

    pub async fn admin_set_password(
        &self,
        issuer_id: i32,
        account_id: i32,
        password: &str,
    ) -> DatabaseResult<()> {
        self.db.set_admin_password_hash(account_id, &pwhash::hash(password)).await?;
        self.db.log_action(issuer_id, LogAction::EditPassword { account_id }).await?;

        Ok(())
    }

    pub async fn admin_update_user(&self, account_id: i32, username: &str) -> DatabaseResult<()> {
        self.update_username(account_id, username).await
    }

    #[cfg(feature = "database")]
    async fn punishment_preconditions(
        &self,
        issuer_id: i32,
        account_id: i32,
    ) -> Result<(), PunishUserError> {
        let issuer = self.get_user(issuer_id).await?;
        let target = self.get_user(account_id).await?;

        if issuer.is_none() {
            warn!("could not find the moderator in the database ({issuer_id})");
            return Err(PunishUserError::Permissions);
        }

        if target.is_none() {
            return Err(PunishUserError::NotFound);
        }

        let issuer = issuer.unwrap();
        let target = target.unwrap();

        if !self.has_stronger_role(&issuer, &target) {
            return Err(PunishUserError::Permissions);
        }

        Ok(())
    }

    #[cfg(feature = "database")]
    pub async fn admin_punish_user(
        &self,
        issuer_id: i32,
        account_id: i32,
        reason: &str,
        expires_at: i64,
        r#type: UserPunishmentType,
    ) -> Result<(), PunishUserError> {
        self.punishment_preconditions(issuer_id, account_id).await?;

        let exp = NonZeroI64::new(expires_at);
        match self.db.punish_user(issuer_id, account_id, r#type, reason, exp).await? {
            Some(edit) => {
                self.db
                    .log_action(
                        issuer_id,
                        self.log_for_punish(account_id, reason, expires_at, r#type, edit),
                    )
                    .await?;
            }

            None => {
                warn!("failed to ban user, did not find the target in the database ({account_id})");
                return Err(PunishUserError::NotFound);
            }
        }

        Ok(())
    }

    #[cfg(feature = "database")]
    pub async fn admin_unpunish_user(
        &self,
        issuer_id: i32,
        account_id: i32,
        r#type: UserPunishmentType,
    ) -> Result<(), PunishUserError> {
        self.db.unpunish_user(account_id, r#type).await?;
        self.db.log_action(issuer_id, self.log_for_unpunish(account_id, r#type)).await?;

        Ok(())
    }

    #[cfg(feature = "database")]
    pub async fn admin_fetch_logs(
        &self,
        issuer: i32,
        target: i32,
        r#type: &str,
        before: i64,
        after: i64,
    ) -> DatabaseResult<(Vec<AuditLogModel>, Vec<ClientAccountData>)> {
        let logs = self.db.fetch_logs(issuer, target, r#type, before, after).await?;

        // build the account data vec, so that the user knows which account ids correspond to which person
        let mut datas: Vec<ClientAccountData> = Vec::new();

        let mut push_user = async |account_id: i32| -> Result<(), DatabaseError> {
            if !datas.iter().any(|c| c.account_id == account_id) {
                if let Some(user) = self.get_user(account_id).await? {
                    datas.push(ClientAccountData {
                        account_id,
                        user_id: 0,
                        username: user
                            .username
                            .and_then(|x| x.as_str().try_into().ok())
                            .unwrap_or_default(),
                    });
                }
            }

            Ok(())
        };

        for model in logs.iter() {
            push_user(model.account_id);

            if let Some(target_id) = model.target_account_id {
                push_user(target_id);
            }
        }

        Ok((logs, datas))
    }

    pub async fn log_kick(
        &self,
        issuer_id: i32,
        account_id: i32,
        reason: &str,
    ) -> DatabaseResult<()> {
        self.db.log_action(issuer_id, LogAction::Kick { account_id, reason }).await
    }

    pub async fn log_notice(
        &self,
        issuer_id: i32,
        account_id: i32,
        message: &str,
    ) -> DatabaseResult<()> {
        self.db.log_action(issuer_id, LogAction::Notice { account_id, message }).await
    }

    fn log_for_punish<'a>(
        &self,
        account_id: i32,
        reason: &'a str,
        expires_at: i64,
        r#type: UserPunishmentType,
        edit: bool,
    ) -> LogAction<'a> {
        if edit {
            match r#type {
                UserPunishmentType::Ban => LogAction::EditBan { account_id, reason, expires_at },
                UserPunishmentType::Mute => LogAction::EditMute { account_id, reason, expires_at },
                UserPunishmentType::RoomBan => {
                    LogAction::EditRoomBan { account_id, reason, expires_at }
                }
            }
        } else {
            match r#type {
                UserPunishmentType::Ban => LogAction::Ban { account_id, reason, expires_at },
                UserPunishmentType::Mute => LogAction::Mute { account_id, reason, expires_at },
                UserPunishmentType::RoomBan => {
                    LogAction::RoomBan { account_id, reason, expires_at }
                }
            }
        }
    }

    fn log_for_unpunish<'a>(&self, account_id: i32, r#type: UserPunishmentType) -> LogAction<'a> {
        match r#type {
            UserPunishmentType::Ban => LogAction::Unban { account_id },
            UserPunishmentType::Mute => LogAction::Unmute { account_id },
            UserPunishmentType::RoomBan => LogAction::RoomUnban { account_id },
        }
    }

    fn has_stronger_role(&self, issuer: &DbUser, target: &DbUser) -> bool {
        let issuer_role = self.compute_from_user(issuer);
        let target_role = self.compute_from_user(target);

        issuer_role.priority > target_role.priority
    }
}

impl ServerModule for UsersModule {
    type Config = Config;

    async fn new(config: &Self::Config) -> ModuleInitResult<Self> {
        let db = UsersDb::new(&config.database_url, config.database_pool_size).await?;
        db.run_migrations().await?;

        let mut roles = Vec::new();
        for role in config.roles.iter() {
            roles.push(role.clone());
        }

        // sort roles by priority descending
        roles.sort_by_key(|role| Reverse(role.priority));

        Ok(Self {
            db,
            roles,
            super_admins: config.super_admins.clone(),
        })
    }

    fn id() -> &'static str {
        "users"
    }

    fn name() -> &'static str {
        "User management"
    }
}
