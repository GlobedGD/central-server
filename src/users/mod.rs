use std::{cmp::Reverse, collections::HashSet, fmt::Write, num::NonZeroI64};

#[cfg(feature = "discord")]
use {crate::discord::DiscordModule, std::sync::Arc};

#[cfg(feature = "discord")]
use crate::core::handler::ClientStateHandle;
#[cfg(feature = "database")]
use crate::{auth::ClientAccountData, users::database::AuditLogModel};

#[cfg(all(feature = "discord", feature = "database"))]
use crate::discord::DiscordMessage;

use crate::{
    core::{
        handler::ConnectionHandler,
        module::{ConfigurableModule, ModuleInitResult, ServerModule},
    },
    users::{config::Role, database::LogAction},
};

use server_shared::MultiColor;

mod config;
mod database;
mod pwhash;

pub use config::Config;
use database::UsersDb;
pub use database::{DatabaseError, DatabaseResult, DbUser, UserPunishment, UserPunishmentType};
use smallvec::SmallVec;
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

#[derive(Error, Debug)]
pub enum Error {
    #[error("Failed to punish user: {0}")]
    Punish(#[from] PunishUserError),
    #[error("Database error: {0}")]
    Database(#[from] DatabaseError),
    #[error("Failed to find user in the database")]
    NotFound,

    #[cfg(feature = "discord")]
    #[error("Failed to log action via discord bot: {0}")]
    Discord(#[from] crate::discord::BotError),
}

pub struct FetchedMod {
    pub account_id: i32,
    pub username: String,
    pub cube: i16,
    pub color1: u16,
    pub color2: u16,
    pub glow_color: u16,
}

#[derive(Default, Clone)]
pub struct ComputedRole {
    pub priority: i32,
    pub roles: heapless::Vec<u8, 32>,
    pub name_color: Option<MultiColor>,

    pub can_kick: bool,
    pub can_mute: bool,
    pub can_ban: bool,
    pub can_set_password: bool,
    pub can_notice_everyone: bool,
    pub can_edit_roles: bool,
    pub can_send_features: bool,
    pub can_rate_features: bool,
}

impl ComputedRole {
    pub fn can_moderate(&self) -> bool {
        self.can_kick
            || self.can_mute
            || self.can_ban
            || self.can_set_password
            || self.can_notice_everyone
    }

    pub fn is_special(&self) -> bool {
        !self.roles.is_empty() || self.name_color.is_some()
    }
}

#[derive(Clone, Debug, Default)]
pub struct LinkedDiscordAccount {
    pub id: u64,
    pub username: String,
    pub avatar_url: String,
}

impl LinkedDiscordAccount {
    pub fn new(id: u64, username: String, avatar_url: String) -> Self {
        Self { id, username, avatar_url }
    }
}

pub struct UsersModule {
    db: UsersDb,
    roles: Vec<Role>,       // index = numeric role ID
    super_admins: Vec<i32>, // account IDs of super admins
    #[cfg(feature = "discord")]
    discord: Option<Arc<DiscordModule>>,
    log_channel: u64,
}

impl UsersModule {
    pub async fn get_user(&self, account_id: i32) -> DatabaseResult<Option<DbUser>> {
        self.db.get_user(account_id).await
    }

    #[cfg(feature = "discord")]
    pub async fn get_linked_discord(
        &self,
        account_id: i32,
    ) -> DatabaseResult<Option<LinkedDiscordAccount>> {
        let Some(discord_id) = self.db.get_linked_discord(account_id).await? else {
            return Ok(None);
        };

        let mut acc = LinkedDiscordAccount::new(discord_id, String::new(), String::new());

        // try to get the avatar url
        if let Some(discord) = self.discord.as_ref() {
            match discord.get_user_data(discord_id).await {
                Ok(data) => {
                    acc.username = data.username;
                    acc.avatar_url = data.avatar_url;
                }

                Err(e) => warn!("Failed to get user data for user {discord_id}: {e}"),
            }
        }

        Ok(Some(acc))
    }

    #[cfg(not(feature = "discord"))]
    pub async fn get_linked_discord(
        &self,
        account_id: i32,
    ) -> DatabaseResult<Option<LinkedDiscordAccount>> {
        Ok(None)
    }

    #[cfg(feature = "discord")]
    pub async fn link_discord_account(
        &self,
        handle: &ClientStateHandle,
        discord_id: u64,
    ) -> DatabaseResult<()> {
        let account_id = handle.account_id();
        let username = handle.username();
        let icons = handle.icons();

        self.db
            .update_user(
                account_id,
                username,
                icons.cube,
                icons.color1,
                icons.color2,
                icons.glow_color,
            )
            .await?;

        self.db.link_discord_account(account_id, discord_id).await
    }

    #[cfg(feature = "discord")]
    pub async fn get_linked_discord_inverse(
        &self,
        discord_id: u64,
    ) -> DatabaseResult<Option<DbUser>> {
        self.db.get_linked_discord_inverse(discord_id).await
    }

    pub async fn query_user(&self, query: &str) -> DatabaseResult<Option<DbUser>> {
        self.db.query_user(query).await
    }

    pub async fn update_username(&self, account_id: i32, new_username: &str) -> DatabaseResult<()> {
        self.db.update_username(account_id, new_username).await
    }

    pub async fn insert_uident(&self, account_id: i32, ident: &str) -> DatabaseResult<()> {
        self.db.insert_uident(account_id, ident).await
    }

    pub async fn get_accounts_for_uident(&self, ident: &str) -> DatabaseResult<SmallVec<[i32; 8]>> {
        if self.db.get_account_count_for_uident(ident).await? == 0 {
            Ok(SmallVec::new())
        } else {
            self.db.get_accounts_for_uident(ident).await
        }
    }

    pub async fn get_punishment_count(&self, account_id: i32) -> DatabaseResult<u32> {
        self.db.get_punishment_count(account_id).await
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
        let mut ids: Vec<u8> = roles
            .split(',')
            .filter(|s| !s.is_empty())
            .filter_map(|id| self.get_role_by_str_id(id))
            .map(|(index, _)| index as u8)
            .collect();

        ids.sort_by_key(|k| Reverse(self.get_role(*k).unwrap().priority));

        ids
    }

    pub fn compute_from_role_ids<'a>(
        &'a self,
        account_id: i32,
        iter: impl Iterator<Item = u8>,
    ) -> ComputedRole {
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
        let mut can_edit_roles = None;
        let mut can_send_features = None;
        let mut can_rate_features = None;

        let iter = iter.filter_map(|id| self.get_role(id).map(|role| (id, role)));

        for (role_id, role) in iter {
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
            apply_permission(&mut can_edit_roles, role.can_edit_roles);
            apply_permission(&mut can_send_features, role.can_send_features);
            apply_permission(&mut can_rate_features, role.can_rate_features);

            out_role.priority = role.priority;
            let _ = out_role.roles.push(role_id);

            if !is_weaker {
                out_role.name_color = Some(role.name_color.clone());
            }
        }

        out_role.can_mute = can_mute.unwrap_or(false);
        out_role.can_kick = can_kick.unwrap_or(false);
        out_role.can_ban = can_ban.unwrap_or(false);
        out_role.can_set_password = can_set_password.unwrap_or(false);
        out_role.can_notice_everyone = can_notice_everyone.unwrap_or(false);
        out_role.can_edit_roles = can_edit_roles.unwrap_or(false);
        out_role.can_send_features = can_send_features.unwrap_or(false);
        out_role.can_rate_features = can_rate_features.unwrap_or(false);

        // sort roles by priority descending
        out_role.roles.sort_unstable_by_key(|&id| {
            Reverse(self.get_role(id).map_or(i32::MIN, |r| r.priority))
        });

        // super admin has the highest possible priority and perms
        if self.super_admins.contains(&account_id) {
            out_role.priority = i32::MAX;
            out_role.can_kick = true;
            out_role.can_mute = true;
            out_role.can_ban = true;
            out_role.can_set_password = true;
            out_role.can_notice_everyone = true;
            out_role.can_edit_roles = true;
            out_role.can_send_features = true;
            out_role.can_rate_features = true;
        }

        out_role
    }

    pub fn compute_from_roles<'a>(
        &'a self,
        account_id: i32,
        iter: impl Iterator<Item = &'a str>,
    ) -> ComputedRole {
        self.compute_from_role_ids(
            account_id,
            iter.filter_map(|x| self.get_role_by_str_id(x).map(|(idx, _)| idx as u8)),
        )
    }

    pub fn compute_from_rolestr(&self, account_id: i32, rolestr: &str) -> ComputedRole {
        self.compute_from_roles(account_id, rolestr.split(',').filter(|x| !x.is_empty()))
    }

    pub fn compute_from_user(&self, user: &DbUser) -> ComputedRole {
        self.compute_from_rolestr(user.account_id, user.roles.as_deref().unwrap_or(""))
    }

    /// Converts a slice of role IDs into a comma-separated string of string IDs
    pub fn make_role_string(&self, roles: &[u8]) -> String {
        if roles.is_empty() {
            return String::new();
        }

        itertools::join(roles.iter().filter_map(|id| self.get_role(*id).map(|role| &role.id)), ",")
    }

    pub async fn get_all_users_with_role(&self, role_id: &str) -> DatabaseResult<Vec<DbUser>> {
        self.db.query_user_with_role(role_id).await
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
    ) -> Result<(), Error> {
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
            if let Some(role) = self.get_role(*role) {
                write!(rolediff, "+{},", role.id).unwrap();
            } else {
                warn!("Unknown role ID: {role}");
            }
        }

        for role in prev_set.difference(&new_set) {
            if let Some(role) = self.get_role(*role) {
                write!(rolediff, "-{},", role.id).unwrap();
            } else {
                warn!("Unknown role ID: {role}");
            }
        }

        // remove the last comma if it exists
        if rolediff.ends_with(',') {
            rolediff.pop();
        }

        // construct the new role string
        let new_role_string = self.make_role_string(new_roles);

        // update the user
        if !self.db.update_roles(account_id, &new_role_string).await? {
            return Err(Error::NotFound);
        }

        // log to db and discord
        self.perform_log(
            issuer_id,
            LogAction::EditRoles {
                account_id,
                rolediff: &rolediff,
            },
        )
        .await;

        Ok(())
    }

    pub async fn admin_set_password(
        &self,
        issuer_id: i32,
        account_id: i32,
        password: &str,
    ) -> DatabaseResult<()> {
        self.db.set_admin_password_hash(account_id, &pwhash::hash(password)).await?;
        self.perform_log(issuer_id, LogAction::EditPassword { account_id }).await;

        Ok(())
    }

    pub async fn admin_update_user(
        &self,
        account_id: i32,
        username: &str,
        cube: i16,
        color1: u16,
        color2: u16,
        glow_color: u16,
    ) -> DatabaseResult<()> {
        self.db.update_user(account_id, username, cube, color1, color2, glow_color).await
    }

    pub async fn fetch_moderators(&self) -> DatabaseResult<Vec<FetchedMod>> {
        // TODO: this function is not very fast

        let mut out = Vec::new();

        let mut users = self.db.fetch_all_with_roles().await?;

        users.retain(|user| {
            let role =
                self.compute_from_rolestr(user.account_id, user.roles.as_deref().unwrap_or(""));
            role.can_moderate()
        });

        for user in users {
            out.push(FetchedMod {
                account_id: user.account_id,
                username: user.username.unwrap_or_else(|| "Unknown".to_owned()),
                cube: user.cube.try_into().unwrap_or(0),
                color1: user.color1.try_into().unwrap_or(0),
                color2: user.color2.try_into().unwrap_or(0),
                glow_color: user.glow_color.try_into().unwrap_or(0),
            });
        }

        // sort by account id
        out.sort_by_key(|u| u.account_id);

        Ok(out)
    }

    #[cfg(feature = "database")]
    async fn punishment_preconditions(
        &self,
        issuer_id: i32,
        account_id: i32,
    ) -> Result<(), PunishUserError> {
        // Check that the user has ability to punish (meaning they have a higher role)

        // super admins can do anything
        if self.super_admins.contains(&issuer_id) {
            return Ok(());
        }

        // compare roles, if the target doesn't exist then assume them as a regular user

        let issuer = self.get_user(issuer_id).await?;
        let target = self.get_user(account_id).await?;

        if issuer.is_none() {
            warn!("punishment failed: could not find the moderator in the database ({issuer_id})");
            return Err(PunishUserError::Permissions);
        }

        if target.is_none() {
            return Ok(());
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
        self.perform_log(issuer_id, self.log_for_unpunish(account_id, r#type)).await;

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
        page: u32,
    ) -> DatabaseResult<(Vec<AuditLogModel>, Vec<ClientAccountData>)> {
        let logs = self.db.fetch_logs(issuer, target, r#type, before, after, page).await?;

        // build the account data vec, so that the user knows which account ids correspond to which person
        let mut datas: Vec<ClientAccountData> = Vec::new();

        let mut push_user = async |account_id: i32| -> Result<(), DatabaseError> {
            if !datas.iter().any(|c| c.account_id == account_id)
                && let Some(user) = self.get_user(account_id).await?
            {
                datas.push(ClientAccountData {
                    account_id,
                    user_id: 0,
                    username: user
                        .username
                        .and_then(|x| x.as_str().try_into().ok())
                        .unwrap_or_default(),
                });
            }

            Ok(())
        };

        for model in logs.iter() {
            push_user(model.account_id).await?;

            if let Some(target_id) = model.target_account_id {
                push_user(target_id).await?;
            }
        }

        Ok((logs, datas))
    }

    pub async fn log_kick(&self, issuer_id: i32, account_id: i32, reason: &str) {
        self.perform_log(issuer_id, LogAction::Kick { account_id, reason }).await
    }

    pub async fn log_notice(&self, issuer_id: i32, account_id: i32, message: &str) {
        self.perform_log(issuer_id, LogAction::Notice { account_id, message }).await
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

    async fn perform_log(&self, issuer_id: i32, log: LogAction<'_>) {
        if let Err(e) = self.db.log_action(issuer_id, log).await {
            warn!("Failed to log punishment in database: {e}");
        }

        #[cfg(feature = "discord")]
        {
            if let Some(d) = &self.discord {
                let msg = Self::convert_to_discord_log(log);

                if let Err(e) = d.send_message(self.log_channel, msg).await {
                    warn!("Failed to log punishment on discord: {e}");
                }
            }
        }
    }

    #[cfg(all(feature = "discord", feature = "database"))]
    fn convert_to_discord_log(log: LogAction<'_>) -> DiscordMessage<'_> {
        // TODO: convert the log actions to embeds
        DiscordMessage::new().content(format!("{log:?}"))
    }

    fn has_stronger_role(&self, issuer: &DbUser, target: &DbUser) -> bool {
        let issuer_role = self.compute_from_user(issuer);
        let target_role = self.compute_from_user(target);

        issuer_role.priority > target_role.priority
    }
}

impl ServerModule for UsersModule {
    async fn new(config: &Config, handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        let db = UsersDb::new(&config.database_url, config.database_pool_size).await?;
        db.run_migrations().await?;

        let mut roles = Vec::new();
        for role in config.roles.iter() {
            roles.push(role.clone());
        }

        // sort roles by priority descending
        roles.sort_by_key(|role| Reverse(role.priority));

        #[cfg(feature = "discord")]
        let discord = handler.opt_module_owned::<DiscordModule>();

        Ok(Self {
            db,
            roles,
            super_admins: config.super_admins.clone(),
            #[cfg(feature = "discord")]
            discord,
            log_channel: config.mod_log_channel,
        })
    }

    fn id() -> &'static str {
        "users"
    }

    fn name() -> &'static str {
        "User management"
    }
}

impl ConfigurableModule for UsersModule {
    type Config = Config;
}
