use std::{cmp::Reverse, collections::HashSet, fmt::Write, num::NonZeroI64};

#[cfg(feature = "discord")]
use {
    crate::{
        core::handler::ClientStateHandle,
        discord::{DiscordMessage, DiscordModule, hex_color_to_decimal},
    },
    poise::serenity_prelude::{CreateEmbed, CreateEmbedAuthor},
    std::{collections::HashMap, sync::Arc},
};

use crate::{
    auth::ClientAccountData,
    core::{
        gd_api::{GDApiClient, GDApiFetchError},
        handler::ConnectionHandler,
        module::{ConfigurableModule, ModuleInitResult, ServerModule},
    },
    users::{
        config::PunishReasons,
        database::{AuditLogModel, LogAction},
    },
};

use server_shared::MultiColor;

mod config;
pub mod database;
mod pwhash;

pub use config::Config;
pub use config::Role;
use database::UsersDb;
pub use database::{DatabaseError, DatabaseResult, DbUser, UserPunishment, UserPunishmentType};
use smallvec::SmallVec;
use thiserror::Error;
use tracing::{debug, info, warn};

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
    #[error("Insufficient permissions")]
    Permissions,
    #[error("Failed to fetch user from GD api: {0}")]
    Fetch(#[from] GDApiFetchError),

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
    #[cfg(feature = "discord")]
    discord_role_map: HashMap<u64, u8>,
    #[cfg(feature = "discord")]
    log_channel: u64,
    whitelist: bool,
    pub vc_requires_discord: bool,

    punish_reasons: PunishReasons,
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
        _account_id: i32,
    ) -> DatabaseResult<Option<LinkedDiscordAccount>> {
        Ok(None)
    }

    #[cfg(feature = "discord")]
    pub async fn link_discord_account_online(
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

        self.db.link_discord_account(account_id, discord_id).await?;

        handle.set_discord_linked(true);

        Ok(())
    }

    #[cfg(feature = "discord")]
    pub async fn link_discord_account_offline(
        &self,
        account_id: i32,
        discord_id: u64,
    ) -> DatabaseResult<()> {
        self.db.link_discord_account(account_id, discord_id).await
    }

    #[cfg(feature = "discord")]
    pub async fn get_linked_discord_inverse(
        &self,
        discord_id: u64,
    ) -> DatabaseResult<Option<DbUser>> {
        self.db.get_linked_discord_inverse(discord_id).await
    }

    #[cfg(feature = "discord")]
    pub async fn unlink_discord_inverse(&self, discord_id: u64) -> DatabaseResult<()> {
        self.db.unlink_discord_inverse(discord_id).await
    }

    pub async fn query_user(&self, query: &str) -> DatabaseResult<Option<DbUser>> {
        self.db.query_user(query).await
    }

    pub async fn query_or_create_user(&self, query: &str) -> Result<Option<DbUser>, Error> {
        if let Some(user) = self.db.query_user(query).await? {
            return Ok(Some(user));
        }

        let Some(user) = GDApiClient::new().fetch_user_by_username(query).await? else {
            return Ok(None);
        };

        self.admin_update_user(
            user.account_id,
            &user.username,
            user.cube,
            user.color1,
            user.color2,
            user.glow_color,
        )
        .await?;

        Ok(self.db.get_user(user.account_id).await?)
    }

    pub async fn update_username(&self, account_id: i32, new_username: &str) -> DatabaseResult<()> {
        self.db.update_username(account_id, new_username).await
    }

    pub async fn insert_uident(&self, account_id: i32, ident: &str) -> DatabaseResult<bool> {
        info!("Inserting uident association: {account_id} - {ident}");
        self.db.insert_uident(account_id, ident).await
    }

    pub async fn get_accounts_for_uident(&self, ident: &str) -> DatabaseResult<SmallVec<[i32; 8]>> {
        if self.db.get_account_count_for_uident(ident).await? == 0 {
            Ok(SmallVec::new())
        } else {
            self.db.get_accounts_for_uident(ident).await
        }
    }

    pub async fn get_user_uident(&self, account_id: i32) -> DatabaseResult<Option<String>> {
        self.db.get_user_uident(account_id).await
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

    pub fn whitelist(&self) -> bool {
        self.whitelist
    }

    pub fn get_punishment_reasons(&self) -> &PunishReasons {
        &self.punish_reasons
    }

    pub async fn is_whitelisted(&self, account_id: i32) -> bool {
        self.get_user(account_id).await.ok().flatten().is_some_and(|x| x.is_whitelisted)
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

    pub fn compute_from_role_ids(
        &self,
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
        self.punishment_preconditions(issuer_id, account_id).await?;

        // disallow adding roles higher than your highest
        let highest_p = self.get_user_highest_priority(issuer_id).await?;

        debug!(
            "User {issuer_id} editing roles for {account_id}, new roles: {new_roles:?}, highest priority: {highest_p}"
        );

        if new_roles.iter().any(|id| self.get_role(*id).is_some_and(|r| r.priority >= highest_p)) {
            return Err(Error::Permissions);
        }

        let rolediff = self.compute_role_diff(account_id, new_roles).await?;
        self.system_set_roles(account_id, new_roles).await?;

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

    pub async fn system_set_roles(&self, account_id: i32, new_roles: &[u8]) -> Result<(), Error> {
        // construct the new role string
        let new_role_string = self.make_role_string(new_roles);

        // update the user
        if !self.db.update_roles(account_id, &new_role_string).await? {
            return Err(Error::NotFound);
        }

        Ok(())
    }

    async fn compute_role_diff(&self, account_id: i32, new_roles: &[u8]) -> DatabaseResult<String> {
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

        Ok(rolediff)
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

    pub async fn admin_set_whitelisted(
        &self,
        _issuer_id: i32,
        account_id: i32,
        whitelisted: bool,
    ) -> DatabaseResult<()> {
        self.db.set_whitelisted(account_id, whitelisted).await
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

    async fn get_user_highest_priority(&self, account_id: i32) -> DatabaseResult<i32> {
        if self.super_admins.contains(&account_id) {
            return Ok(i32::MAX);
        }

        let user = match self.get_user(account_id).await? {
            Some(u) => u,
            None => return Ok(0),
        };

        Ok(self.compute_from_user(&user).priority)
    }

    async fn punishment_preconditions(
        &self,
        issuer_id: i32,
        account_id: i32,
    ) -> Result<(), PunishUserError> {
        // Check that the user has ability to punish (meaning they have a higher role)

        if issuer_id == account_id {
            return Ok(());
        }

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
                self.perform_log(
                    issuer_id,
                    self.log_for_punish(account_id, reason, expires_at, r#type, edit),
                )
                .await;
            }

            None => {
                warn!("failed to ban user, did not find the target in the database ({account_id})");
                return Err(PunishUserError::NotFound);
            }
        }

        Ok(())
    }

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

    pub async fn log_kick(&self, issuer_id: i32, account_id: i32, username: &str, reason: &str) {
        self.perform_log(issuer_id, LogAction::Kick { account_id, username, reason }).await
    }

    pub async fn log_notice(&self, issuer_id: i32, account_id: i32, message: &str) {
        self.perform_log(issuer_id, LogAction::Notice { account_id, message }).await
    }

    pub async fn log_notice_group(&self, issuer_id: i32, message: &str, count: u32) {
        self.perform_log(issuer_id, LogAction::NoticeGroup { message, count }).await
    }

    pub async fn log_notice_everyone(&self, issuer_id: i32, message: &str, count: u32) {
        self.perform_log(issuer_id, LogAction::NoticeEveryone { message, count }).await
    }

    pub async fn log_notice_reply(
        &self,
        issuer_id: i32,
        issuer_name: &str,
        account_id: i32,
        message: &str,
    ) {
        self.perform_log(
            issuer_id,
            LogAction::NoticeReply {
                username: issuer_name,
                reply_to: account_id,
                message,
            },
        )
        .await
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
            if let Some(d) = &self.discord
                && self.log_channel != 0
            {
                match self.convert_to_discord_log(log, issuer_id).await {
                    Ok(msg) => {
                        if msg.content.is_some() || !msg.embeds.is_empty() {
                            d.send_message(self.log_channel, msg);
                        }
                    }

                    Err(e) => {
                        warn!("Failed to convert log to discord message: {e}");
                    }
                }
            }
        }
    }

    #[cfg(feature = "discord")]
    async fn convert_to_discord_log(
        &self,
        log: LogAction<'_>,
        issuer_id: i32,
    ) -> anyhow::Result<DiscordMessage<'_>> {
        let mut msg = DiscordMessage::new();

        let issuer = self.get_user(issuer_id).await?;

        let target = if log.account_id() != 0 {
            self.get_user(log.account_id()).await?
        } else {
            None
        };

        let mut issuer_name =
            issuer.as_ref().and_then(|u| u.username.as_deref()).unwrap_or("Unknown");
        let target_name = target.as_ref().and_then(|u| u.username.as_deref()).unwrap_or("Unknown");

        if issuer_name == "Unknown" {
            // try to extract the name from the log if possible
            match log {
                LogAction::Kick { username, .. } => {
                    issuer_name = username;
                }

                LogAction::NoticeReply { username, .. } => {
                    issuer_name = username;
                }

                _ => {}
            }
        }

        let issuer_combo = format!("{} ({})", issuer_name, issuer_id);
        let target_combo = format!("{} ({})", target_name, log.account_id());

        match log {
            // Notice to everyone or group
            LogAction::NoticeEveryone { message, count }
            | LogAction::NoticeGroup { message, count } => {
                msg = msg.add_embed(
                    CreateEmbed::new()
                        .title(format!("Notice to {} people", count))
                        .color(hex_color_to_decimal("#4dace8"))
                        .description(message)
                        .field("Performed by", issuer_combo, true),
                )
            }

            // Notice to user
            LogAction::Notice { message, .. } => {
                msg = msg.add_embed(
                    CreateEmbed::new()
                        .title(format!("Notice to {}", target_combo))
                        .color(hex_color_to_decimal("#4dace8"))
                        .description(message)
                        .field("Performed by", issuer_combo, true),
                )
            }

            LogAction::NoticeReply { message, username, .. } => {
                msg = msg.add_embed(
                    CreateEmbed::new()
                        .title(format!("Notice reply from {}", username))
                        .color(hex_color_to_decimal("#55d9ed"))
                        .description(message)
                        .field("Sent to", target_combo, true),
                )
            }

            LogAction::Kick { reason, .. } => {
                msg = msg.add_embed(
                    CreateEmbed::new()
                        .title("User kicked")
                        .color(hex_color_to_decimal("#e8d34d"))
                        .description(reason)
                        .author(CreateEmbedAuthor::new(target_combo))
                        .field("Performed by", issuer_combo, true),
                )
            }

            LogAction::Ban { reason, expires_at, .. }
            | LogAction::Mute { reason, expires_at, .. }
            | LogAction::RoomBan { reason, expires_at, .. }
            | LogAction::EditBan { reason, expires_at, .. }
            | LogAction::EditMute { reason, expires_at, .. }
            | LogAction::EditRoomBan { reason, expires_at, .. } => {
                let (title, color) = match log {
                    LogAction::Ban { .. } => ("User banned", "#de3023"),
                    LogAction::Mute { .. } => ("User muted", "#ded823"),
                    LogAction::RoomBan { .. } => ("User room banned", "#d2a126"),
                    LogAction::EditBan { .. } => ("User ban changed", "#de7a23"),
                    LogAction::EditMute { .. } => ("User mute changed", "#de7a23"),
                    LogAction::EditRoomBan { .. } => ("User room ban changed", "#de7a23"),
                    _ => unreachable!(),
                };

                msg = msg.add_embed(
                    CreateEmbed::new()
                        .title(title)
                        .color(hex_color_to_decimal(color))
                        .description(if reason.is_empty() { "No reason provided" } else { reason })
                        .author(CreateEmbedAuthor::new(target_combo))
                        .field("Performed by", issuer_combo, true)
                        .field("Expires", format_expiry(expires_at), true),
                )
            }

            LogAction::Unban { .. } | LogAction::Unmute { .. } | LogAction::RoomUnban { .. } => {
                let (title, color) = match log {
                    LogAction::Unban { .. } => ("User unbanned", "#31bd31"),
                    LogAction::Unmute { .. } => ("User unmuted", "#79bd31"),
                    LogAction::RoomUnban { .. } => ("User room unbanned", "#388e3c"),
                    _ => unreachable!(),
                };

                msg = msg.add_embed(
                    CreateEmbed::new()
                        .title(title)
                        .color(hex_color_to_decimal(color))
                        .author(CreateEmbedAuthor::new(target_combo))
                        .field("Performed by", issuer_combo, true),
                )
            }

            LogAction::EditRoles { rolediff, .. } => {
                let mut added = Vec::new();
                let mut removed = Vec::new();

                for part in rolediff.split(',') {
                    if let Some(part) = part.strip_prefix('+') {
                        added.push(part);
                    } else if let Some(part) = part.strip_prefix('-') {
                        removed.push(part);
                    }
                }

                msg = msg.add_embed(
                    CreateEmbed::new()
                        .title("Role change")
                        .color(hex_color_to_decimal("#8b4de8"))
                        .author(CreateEmbedAuthor::new(target_combo))
                        .field("Performed by", issuer_combo, true)
                        .field("Added roles", itertools::join(added.iter(), ", "), true)
                        .field("Removed roles", itertools::join(removed.iter(), ", "), true),
                )
            }

            LogAction::EditPassword { .. } => {
                // not logged
            }
        }

        Ok(msg)
    }

    fn has_stronger_role(&self, issuer: &DbUser, target: &DbUser) -> bool {
        let issuer_role = self.compute_from_user(issuer);
        let target_role = self.compute_from_user(target);

        issuer_role.priority > target_role.priority
    }

    #[cfg(feature = "discord")]
    pub fn get_role_by_discord_id(&self, id: u64) -> Option<&Role> {
        self.get_role(self.get_role_id_by_discord_id(id)?)
    }

    #[cfg(feature = "discord")]
    pub fn get_role_id_by_discord_id(&self, id: u64) -> Option<u8> {
        self.discord_role_map.get(&id).copied()
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

        #[cfg(feature = "discord")]
        let discord_role_map = {
            let mut map = HashMap::new();

            for (id, role) in roles.iter().enumerate() {
                if role.discord_id != 0 {
                    map.insert(role.discord_id, id as u8);
                }
            }

            map
        };

        let _ = handler;

        Ok(Self {
            db,
            roles,
            super_admins: config.super_admins.clone(),
            #[cfg(feature = "discord")]
            discord,
            #[cfg(feature = "discord")]
            discord_role_map,
            #[cfg(feature = "discord")]
            log_channel: config.mod_log_channel,
            whitelist: config.whitelist,
            vc_requires_discord: config.vc_requires_discord_link,
            punish_reasons: config.punishment_reasons.clone(),
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

#[cfg(feature = "discord")]
fn format_expiry(expires_at: i64) -> String {
    if expires_at == 0 {
        "Never".to_string()
    } else {
        format!("<t:{0}:f> (<t:{0}:R>)", expires_at)
    }
}
