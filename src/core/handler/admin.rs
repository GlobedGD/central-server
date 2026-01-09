use std::{fmt::Display, sync::Arc};

use server_shared::{
    SessionId,
    qunet::{buffers::ByteWriter, message::BufferKind},
};
use thiserror::Error;

use crate::{
    auth::AuthModule,
    credits::CreditsModule,
    rooms::RoomModule,
    users::{DatabaseError, UserPunishment, UserPunishmentType, UsersModule},
};

use super::{ConnectionHandler, util::*};

pub enum ActionType {
    Kick,
    Notice,
    NoticeEveryone,
    Ban,
    RoomBan,
    Mute,
    SetPassword,
    EditRoles,
    SendFeatures,
    RateFeatures,
}

#[derive(Default)]
struct FetchResponse<'a> {
    account_id: i32,
    found: bool,
    whitelisted: bool,
    roles: &'a [u8],
    active_ban: Option<&'a UserPunishment>,
    active_room_ban: Option<&'a UserPunishment>,
    active_mute: Option<&'a UserPunishment>,
    punishment_count: u32,
}

#[derive(Error, Debug)]
enum RefreshError {
    #[error("Database error: {0}")]
    Database(#[from] DatabaseError),
    #[error("Handler error: {0}")]
    Handler(#[from] HandlerError),
}

impl ConnectionHandler {
    pub fn must_be_able(
        &self,
        client: &ClientStateHandle,
        action: ActionType,
    ) -> HandlerResult<()> {
        must_admin_auth(client)?;

        let Some(role) = &*client.role() else {
            // dont send a message
            return Err(HandlerError::NotAdmin);
        };

        let can = match action {
            ActionType::Kick => role.can_kick,
            ActionType::Notice => true, // anyone can send notices
            ActionType::NoticeEveryone => role.can_notice_everyone,
            ActionType::Ban => role.can_ban,
            ActionType::RoomBan => role.can_ban,
            ActionType::Mute => role.can_mute,
            ActionType::SetPassword => role.can_set_password,
            ActionType::EditRoles => true,
            ActionType::SendFeatures => role.can_send_features,
            ActionType::RateFeatures => role.can_rate_features,
        };

        if can {
            Ok(())
        } else {
            self.send_admin_result(client, Err("insufficient permissions"))?;
            Err(HandlerError::NotAdmin)
        }
    }

    pub fn send_admin_result<Fr: AsRef<str>>(
        &self,
        client: &ClientStateHandle,
        result: Result<(), Fr>,
    ) -> HandlerResult<()> {
        let cap = 56 + result.as_ref().err().map_or(0, |e| e.as_ref().len());

        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut admin_result = msg.reborrow().init_admin_result();

            match result {
                Ok(()) => admin_result.set_success(true),
                Err(e) => {
                    admin_result.set_success(false);
                    admin_result.set_error(e.as_ref())
                }
            }
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }

    pub fn send_admin_db_result<E: Display>(
        &self,
        client: &ClientStateHandle,
        result: Result<(), E>,
    ) -> HandlerResult<()> {
        self.send_admin_result(client, result.map_err(|db| db.to_string()))
    }

    pub async fn handle_admin_login(
        &self,
        client: &ClientStateHandle,
        password: &str,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let users = self.module::<UsersModule>();

        let result = match users.admin_login(client.account_id(), password).await {
            Ok(true) => Ok(()),

            Ok(false) => Err("invalid credentials"),

            Err(e) => {
                warn!("[{} @ {}] admin login failed: {}", client.account_id(), client.address, e);
                Err("internal error")
            }
        };

        if result.is_ok() {
            client.set_authorized_mod();

            let reasons = users.get_punishment_reasons();
            let cap = 80
                + reasons.ban.iter().map(|r| r.len() + 16).sum::<usize>()
                + reasons.mute.iter().map(|r| r.len() + 16).sum::<usize>()
                + reasons.room_ban.iter().map(|r| r.len() + 16).sum::<usize>();

            let buf = data::encode_message_heap!(self, cap, msg => {
                let mut msg = msg.init_admin_punishment_reasons();
                let _ = msg.set_ban(&reasons.ban[..]);
                let _ = msg.set_mute(&reasons.mute[..]);
                let _ = msg.set_room_ban(&reasons.room_ban[..]);
            })?;
            client.send_data_bufkind(buf);
        }

        self.send_admin_result(client, result)?;

        Ok(())
    }

    pub async fn handle_admin_kick(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        reason: &str,
    ) -> HandlerResult<()> {
        self.must_be_able(client, ActionType::Kick)?;

        let users = self.module::<UsersModule>();

        let result = if let Some(target) = self.find_client(account_id) {
            // kick the person
            target.disconnect(format!("Kicked by moderator: {reason}"));
            users.log_kick(client.account_id(), account_id, target.username(), reason).await;

            Ok(())
        } else {
            Err("failed to find the target person")
        };

        self.send_admin_result(client, result)?;

        Ok(())
    }

    pub async fn handle_admin_notice(
        &self,
        client: &ClientStateHandle,
        target_user: &str,
        room_id: u32,
        level_id: i32,
        message: &str,
        can_reply: bool,
        show_sender: bool,
    ) -> HandlerResult<()> {
        let multi_notice = room_id != 0 || level_id != 0;

        // sending notices to multiple people in the global room requires NoticeEveryone permission
        self.must_be_able(
            client,
            if multi_notice && room_id == 0 {
                ActionType::NoticeEveryone
            } else {
                ActionType::Notice
            },
        )?;

        // to be able to reply, we must show the sender
        let show_sender = show_sender || can_reply;

        let users = self.module::<UsersModule>();

        let targets = if let Some(target) =
            target_user.parse::<i32>().ok().and_then(|id| self.find_client(id))
        {
            vec![target]
        } else if !target_user.is_empty() {
            self.all_clients
                .iter()
                .filter_map(|x| {
                    x.value().upgrade().and_then(|c| {
                        c.clone().account_data().and_then(|d| {
                            if d.username.eq_ignore_ascii_case(target_user) {
                                Some(c)
                            } else {
                                None
                            }
                        })
                    })
                })
                .collect()
        } else if room_id != 0 {
            let rooms = self.module::<RoomModule>();
            let Some(room) = rooms.get_room(room_id) else {
                self.send_admin_result(client, Err("failed to find the room"))?;
                return Ok(());
            };

            room.with_players(|_, players| {
                let mut out = Vec::new();

                if level_id == 0 {
                    out.extend(players.map(|(_, p)| p.handle.clone()));
                } else {
                    players.for_each(|(_, p)| {
                        if SessionId::from(p.handle.session_id()).level_id() == level_id {
                            out.push(p.handle.clone());
                        }
                    });
                }

                out
            })
            .await
        } else if level_id != 0 {
            self.all_clients
                .iter()
                .filter_map(|x| x.value().upgrade())
                .filter(|c| SessionId::from(c.session_id()).level_id() == level_id)
                .collect()
        } else {
            self.send_admin_result(client, Err("no target specified"))?;
            return Ok(());
        };

        if targets.is_empty() {
            self.send_admin_result(client, Err("failed to find any targets for the notice"))?;
            return Ok(());
        }

        if targets.len() == 1 {
            let _ = users.log_notice(client.account_id(), targets[0].account_id(), message).await;
        } else {
            let _ =
                users.log_notice_group(client.account_id(), message, targets.len() as u32).await;
        }

        for target in targets {
            self.send_notice(client, &target, message, can_reply, show_sender)?;
        }

        Ok(())
    }

    pub async fn handle_admin_notice_everyone(
        &self,
        client: &ClientStateHandle,
        message: &str,
    ) -> HandlerResult<()> {
        self.must_be_able(client, ActionType::NoticeEveryone)?;

        let users = self.module::<UsersModule>();
        let count = self.send_notice_all(client, message, false, false).unwrap_or(0);
        users.log_notice_everyone(client.account_id(), message, count as u32).await;

        Ok(())
    }

    pub fn make_notice_buf(
        &self,
        sender: &ClientStateHandle,
        message: &str,
        can_reply: bool,
        is_reply: bool,
        show_sender: bool,
    ) -> HandlerResult<BufferKind> {
        let buf = data::encode_message_heap!(self, 80 + message.len(), msg => {
            let mut notice = msg.init_notice();
            notice.set_message(message);
            notice.set_can_reply(can_reply);
            notice.set_is_reply(is_reply);

            if show_sender {
                let account_data = sender.account_data().expect("must have account data");

                notice.set_sender_id(account_data.account_id);
                notice.set_sender_name(&account_data.username);
            } else {
                notice.set_sender_id(0);
            }
        })?;

        Ok(buf)
    }

    fn send_notice(
        &self,
        sender: &ClientStateHandle,
        target: &ClientStateHandle,
        message: &str,
        can_reply: bool,
        show_sender: bool,
    ) -> HandlerResult<()> {
        info!(
            "[{} ({})] sent notice to {} ({}): \"{}\"",
            sender.username(),
            sender.account_id(),
            target.username(),
            target.account_id(),
            message
        );

        if can_reply {
            sender.add_awaiting_notice_reply(target.account_id());
        }

        target.send_data_bufkind(self.make_notice_buf(
            sender,
            message,
            can_reply,
            false,
            show_sender,
        )?);

        Ok(())
    }

    fn send_notice_all(
        &self,
        sender: &ClientStateHandle,
        message: &str,
        can_reply: bool,
        show_sender: bool,
    ) -> HandlerResult<usize> {
        info!(
            "[{} ({})] sent notice to EVERYONE!!: \"{}\"",
            sender.username(),
            sender.account_id(),
            message
        );

        let buf = Arc::new(self.make_notice_buf(sender, message, can_reply, false, show_sender)?);

        for target in self.all_clients.iter().filter_map(|x| x.value().upgrade()) {
            target.send_data_bufkind(BufferKind::Reference(buf.clone()));
        }

        Ok(self.all_clients.len())
    }

    pub async fn handle_admin_fetch_user(
        &self,
        client: &ClientStateHandle,
        query: &str,
    ) -> HandlerResult<()> {
        must_admin_auth(client)?;

        let users = self.module::<UsersModule>();

        match users.query_user(query).await {
            Ok(Some(user)) => {
                self.send_fetch_response(
                    client,
                    FetchResponse {
                        account_id: user.account_id,
                        found: true,
                        whitelisted: user.is_whitelisted,
                        roles: &users.role_str_to_ids(&user.roles.unwrap_or_default()),
                        active_ban: user.active_ban.as_ref(),
                        active_room_ban: user.active_room_ban.as_ref(),
                        active_mute: user.active_mute.as_ref(),
                        punishment_count: users
                            .get_punishment_count(user.account_id)
                            .await
                            .unwrap_or(0),
                    },
                )?;
            }

            Ok(None) => {
                self.send_fetch_response(client, FetchResponse::default())?;
            }

            Err(e) => self.send_admin_result(client, Err(e.to_string()))?,
        };

        Ok(())
    }

    fn send_fetch_response(
        &self,
        client: &ClientStateHandle,
        resp: FetchResponse<'_>,
    ) -> HandlerResult<()> {
        let cap = 108
            + resp.roles.len()
            + resp.active_ban.map_or(0, |p| 32 + p.reason.len())
            + resp.active_room_ban.map_or(0, |p| 32 + p.reason.len())
            + resp.active_mute.map_or(0, |p| 32 + p.reason.len());

        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut fetch = msg.init_admin_fetch_response();
            fetch.set_account_id(resp.account_id);
            fetch.set_found(resp.found);
            fetch.set_whitelisted(resp.whitelisted);
            fetch.set_punishment_count(resp.punishment_count);

            if let Some(ban) = resp.active_ban {
                Self::encode_punishment(ban, &mut fetch.reborrow().init_active_ban());
            }

            if let Some(room_ban) = resp.active_room_ban {
                Self::encode_punishment(room_ban, &mut fetch.reborrow().init_active_room_ban());
            }

            if let Some(mute) = resp.active_mute {
                Self::encode_punishment(mute, &mut fetch.reborrow().init_active_mute());
            }

            let _ = fetch.set_roles(resp.roles);
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }

    fn encode_punishment(
        punishment: &UserPunishment,
        out: &mut data::user_punishment::Builder<'_>,
    ) {
        out.set_issued_at(punishment.issued_at.map_or(0, |t| t.get()));
        out.set_issued_by(punishment.issued_by);
        out.set_expires_at(punishment.expires_at.map_or(0, |t| t.get()));
        out.set_reason(&punishment.reason);
    }

    pub async fn handle_admin_ban(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        reason: &str,
        expires_at: i64,
    ) -> HandlerResult<()> {
        self.wrap_punish(client, account_id, reason, expires_at, UserPunishmentType::Ban).await
    }

    pub async fn handle_admin_unban(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
    ) -> HandlerResult<()> {
        self.wrap_unpunish(client, account_id, UserPunishmentType::Ban).await
    }

    pub async fn handle_admin_room_ban(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        reason: &str,
        expires_at: i64,
    ) -> HandlerResult<()> {
        self.wrap_punish(client, account_id, reason, expires_at, UserPunishmentType::RoomBan).await
    }

    pub async fn handle_admin_room_unban(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
    ) -> HandlerResult<()> {
        self.wrap_unpunish(client, account_id, UserPunishmentType::RoomBan).await
    }

    pub async fn handle_admin_mute(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        reason: &str,
        expires_at: i64,
    ) -> HandlerResult<()> {
        self.wrap_punish(client, account_id, reason, expires_at, UserPunishmentType::Mute).await
    }

    pub async fn handle_admin_unmute(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
    ) -> HandlerResult<()> {
        self.wrap_unpunish(client, account_id, UserPunishmentType::Mute).await
    }

    async fn try_save_uident(&self, user: &ClientStateHandle) {
        let users = self.module::<UsersModule>();

        if let Some(uident) = user.uident() {
            let uident = hex::encode(uident);

            if let Err(e) = users.insert_uident(user.account_id(), &uident).await {
                warn!("failed to save ident for {} ({uident}): {e}", user.account_id());
            }
        }
    }

    async fn refresh_live_punishments(
        &self,
        client: &ClientStateHandle,
        r#type: impl Into<Option<UserPunishmentType>>,
    ) -> Result<(), RefreshError> {
        let users = self.module::<UsersModule>();
        let r#type = r#type.into();

        if let Some(user) = users.get_user(client.account_id()).await? {
            if let Some(ban) = user.active_ban {
                self.send_banned(client, &ban.reason, ban.expires_at)?;
            }

            if let Some(mute) = &user.active_mute
                && r#type == Some(UserPunishmentType::Mute)
            {
                self.send_muted(client, &mute.reason, mute.expires_at)?;
            }

            client.set_active_punishments(user.active_mute, user.active_room_ban);

            // tell all game servers about the ban/mute if the user is connected

            if r#type == Some(UserPunishmentType::Ban) {
                let _ = self.game_server_manager.notify_user_banned(client.account_id()).await;
            } else if r#type == Some(UserPunishmentType::Mute) {
                let _ = self.game_server_manager.notify_user_muted(client.account_id()).await;
            }
        }

        Ok(())
    }

    async fn wrap_punish(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        reason: &str,
        expires_at: i64,
        r#type: UserPunishmentType,
    ) -> HandlerResult<()> {
        self.must_be_able(
            client,
            match r#type {
                UserPunishmentType::Ban => ActionType::Ban,
                UserPunishmentType::Mute => ActionType::Mute,
                UserPunishmentType::RoomBan => ActionType::RoomBan,
            },
        )?;

        let users = self.module::<UsersModule>();

        let result = users
            .admin_punish_user(client.account_id(), account_id, reason, expires_at, r#type)
            .await;

        if let Some(user) = self.find_client(account_id) {
            self.try_save_uident(&user).await;

            if let Err(e) = self.refresh_live_punishments(&user, r#type).await {
                warn!("failed to apply punishments live to {}: {e}", user.account_id());
            }
        }

        self.send_admin_db_result(client, result)?;

        Ok(())
    }

    async fn wrap_unpunish(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        r#type: UserPunishmentType,
    ) -> HandlerResult<()> {
        self.must_be_able(
            client,
            match r#type {
                UserPunishmentType::Ban => ActionType::Ban,
                UserPunishmentType::Mute => ActionType::Mute,
                UserPunishmentType::RoomBan => ActionType::RoomBan,
            },
        )?;

        let users = self.module::<UsersModule>();
        let result = users.admin_unpunish_user(client.account_id(), account_id, r#type).await;

        self.send_admin_db_result(client, result)?;

        if let Some(user) = self.find_client(account_id) {
            // tell the user they are unbanned
            let _ = self.refresh_live_punishments(&user, None).await;
        }

        Ok(())
    }

    pub async fn handle_admin_edit_roles(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        role_ids: &[u8],
    ) -> HandlerResult<()> {
        self.must_be_able(client, ActionType::EditRoles)?;

        let users = self.module::<UsersModule>();
        let result = users.admin_edit_roles(client.account_id(), account_id, role_ids).await;

        if result.is_ok() {
            let _ = self.notify_user_data_changed(account_id, role_ids).await;

            // force a reload of credits
            self.module::<CreditsModule>().queue_reload();
        }

        self.send_admin_db_result(client, result)?;

        Ok(())
    }

    pub async fn handle_admin_set_password(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        password: &str,
    ) -> HandlerResult<()> {
        self.must_be_able(client, ActionType::SetPassword)?;

        let users = self.module::<UsersModule>();
        let result = users.admin_set_password(client.account_id(), account_id, password).await;

        self.send_admin_db_result(client, result)?;

        Ok(())
    }

    pub async fn handle_admin_update_user(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        username: &str,
        cube: i16,
        color1: u16,
        color2: u16,
        glow_color: u16,
    ) -> HandlerResult<()> {
        must_admin_auth(client)?;

        let users = self.module::<UsersModule>();
        let result =
            users.admin_update_user(account_id, username, cube, color1, color2, glow_color).await;

        self.send_admin_db_result(client, result)?;

        Ok(())
    }

    pub async fn handle_admin_fetch_logs(
        &self,
        client: &ClientStateHandle,
        issuer: i32,
        target: i32,
        r#type: &str,
        before: i64,
        after: i64,
        page: u32,
    ) -> HandlerResult<()> {
        must_admin_auth(client)?;

        let users = self.module::<UsersModule>();

        let (logs, users) =
            match users.admin_fetch_logs(issuer, target, r#type, before, after, page).await {
                Ok(x) => x,
                Err(e) => {
                    self.send_admin_db_result(client, Err(e))?;
                    return Ok(());
                }
            };

        let cap = 80
            + logs
                .iter()
                .map(|l| 64 + l.message.as_ref().map(|x| x.len()).unwrap_or(0))
                .sum::<usize>()
            + users.len() * 56;

        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut msg = msg.reborrow().init_admin_logs_response();

            let mut accounts = msg.reborrow().init_accounts(users.len() as u32);
            for (i, user) in users.iter().enumerate() {
                let acc = accounts.reborrow().get(i as u32);
                Self::encode_account_data(user, acc);
            }

            let mut out_logs = msg.init_logs(logs.len() as u32);
            for (i, log) in logs.iter().enumerate() {
                let mut out = out_logs.reborrow().get(i as u32);
                out.set_id(log.id);
                out.set_account_id(log.account_id);
                out.set_target_account_id(log.target_account_id.unwrap_or(0));
                out.set_type(&log.r#type);
                out.set_timestamp(log.timestamp);
                out.set_expires_at(log.expires_at.unwrap_or(0));
                out.set_message(log.message.as_deref().unwrap_or_default());
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    pub async fn handle_admin_fetch_mods(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_admin_auth(client)?;

        let users = self.module::<UsersModule>();

        let users = match users.fetch_moderators().await {
            Ok(x) => x,
            Err(e) => {
                self.send_admin_db_result(client, Err(e))?;
                return Ok(());
            }
        };

        let cap = 48 + 80 * users.len();
        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut resp = msg.init_admin_fetch_mods_response();
            let mut ser = resp.reborrow().init_users(users.len() as u32);

            for (i, user) in users.iter().enumerate() {
                let mut u = ser.reborrow().get(i as u32);
                u.set_account_id(user.account_id);
                u.set_username(&user.username);
                u.set_cube(user.cube);
                u.set_color1(user.color1);
                u.set_color2(user.color2);
                u.set_glow_color(user.glow_color);
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    pub async fn handle_admin_set_whitelisted(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        whitelisted: bool,
    ) -> HandlerResult<()> {
        self.must_be_able(client, ActionType::Ban)?;

        self.send_admin_db_result(
            client,
            self.module::<UsersModule>()
                .admin_set_whitelisted(client.account_id(), account_id, whitelisted)
                .await,
        )?;

        Ok(())
    }

    pub async fn handle_admin_close_room(
        &self,
        client: &ClientStateHandle,
        room_id: u32,
    ) -> HandlerResult<()> {
        self.must_be_able(client, ActionType::Kick)?;

        self.close_room_by_id(room_id).await
    }

    async fn notify_user_data_changed(
        &self,
        account_id: i32,
        new_roles: &[u8],
    ) -> HandlerResult<()> {
        let Some(client) = self.find_client(account_id) else {
            return Ok(());
        };

        let auth = self.module::<AuthModule>();
        let users = self.module::<UsersModule>();

        // generate new role and token to send to the user
        // new token is generated so the user can immediately connect to the game server with appropriate roles
        let new_role = users.compute_from_role_ids(account_id, new_roles.iter().cloned());
        let roles_str = users.make_role_string(new_roles);
        let token = auth.generate_user_token(
            account_id,
            client.user_id(),
            client.username(),
            &roles_str,
            new_role.name_color.as_ref(),
        );

        let buf = data::encode_message!(self, 1024, msg => {
            let mut changed = msg.init_user_data_changed();
            let _ = changed.set_roles(new_role.roles.as_slice());
            changed.set_is_moderator(new_role.can_moderate());
            changed.set_can_mute(new_role.can_mute);
            changed.set_can_ban(new_role.can_ban);
            changed.set_can_set_password(new_role.can_set_password);
            changed.set_can_edit_roles(new_role.can_edit_roles);
            changed.set_can_send_features(new_role.can_send_features);
            changed.set_can_rate_features(new_role.can_rate_features);
            changed.set_new_token(&token);

            if let Some(nc) = new_role.name_color.as_ref() {
                let mut buf = [0u8; 512];
                let mut writer = ByteWriter::new(&mut buf);
                nc.encode(&mut writer);

                changed.set_name_color(writer.written());
            }
        })?;

        client.set_role(new_role);
        client.send_data_bufkind(buf);

        Ok(())
    }
}
