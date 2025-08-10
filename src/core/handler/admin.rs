use std::{borrow::Cow, fmt::Display};

use server_shared::SessionId;

use crate::{
    rooms::RoomModule,
    users::{UserPunishmentType, UsersModule},
};

use super::{ConnectionHandler, util::*};

enum ActionType {
    Kick,
    Notice,
    NoticeEveryone,
    Ban,
    RoomBan,
    Mute,
    SetPassword,
    EditRoles,
}

fn must_be_able(client: &ClientStateHandle, action: ActionType) -> HandlerResult<()> {
    must_admin_auth(client)?;

    let Some(role) = client.role() else {
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
    };

    can.then_some(()).ok_or(HandlerError::NotAdmin)
}

#[derive(Default)]
struct FetchResponse<'a> {
    account_id: i32,
    found: bool,
    whitelisted: bool,
    roles: &'a [u8],
}

impl ConnectionHandler {
    fn send_admin_result<Fr: AsRef<str>>(
        &self,
        client: &ClientStateHandle,
        result: Result<(), Fr>,
    ) -> HandlerResult<()> {
        let cap = 48 + result.as_ref().err().map_or(0, |e| e.as_ref().len());

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

    fn send_admin_db_result<E: Display>(
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
            client.set_authorized_admin();
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
        must_be_able(client, ActionType::Kick)?;

        let users = self.module::<UsersModule>();

        let result = if let Some(client) = self.find_client(account_id) {
            // kick the person
            client.disconnect(Cow::Owned(reason.to_owned()));
            let _ = users.log_kick(client.account_id(), account_id, reason).await;
            Ok(())
        } else {
            Err("failed to find the target person")
        };

        self.send_admin_result(client, result)?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
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
        must_be_able(
            client,
            if room_id == 0 {
                ActionType::NoticeEveryone
            } else {
                ActionType::Notice
            },
        )?;

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
            let _ = users.log_notice(client.account_id(), 0, message).await;
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
        must_be_able(client, ActionType::NoticeEveryone)?;

        let users = self.module::<UsersModule>();
        let _ = users.log_notice(client.account_id(), 0, message).await;

        for user in self.all_clients.iter().filter_map(|x| x.value().upgrade()) {
            self.send_notice(client, &user, message, false, false)?;
        }

        Ok(())
    }

    fn send_notice(
        &self,
        sender: &ClientStateHandle,
        target: &ClientStateHandle,
        message: &str,
        can_reply: bool,
        show_sender: bool,
    ) -> HandlerResult<()> {
        let buf = data::encode_message_heap!(self, 80 + message.len(), msg => {
            let mut notice = msg.init_notice();
            notice.set_message(message);
            notice.set_can_reply(can_reply);

            if show_sender {
                let account_data = sender.account_data().expect("must have account data");

                notice.set_sender_id(account_data.account_id);
                notice.set_sender_name(&account_data.username);
            } else {
                notice.set_sender_id(0);
            }
        })?;

        info!(
            "[{} ({})] sent notice to {} ({}): \"{}\"",
            sender.username(),
            sender.account_id(),
            target.username(),
            target.account_id(),
            message
        );

        target.send_data_bufkind(buf);

        Ok(())
    }

    pub async fn handle_admin_fetch_user(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
    ) -> HandlerResult<()> {
        must_admin_auth(client)?;

        let users = self.module::<UsersModule>();

        match users.get_user(account_id).await {
            Ok(Some(user)) => {
                self.send_fetch_response(
                    client,
                    FetchResponse {
                        account_id,
                        found: true,
                        whitelisted: user.is_whitelisted,
                        roles: &users.role_str_to_ids(&user.roles.unwrap_or_default()),
                    },
                )?;
            }

            Ok(None) => {
                self.send_fetch_response(
                    client,
                    FetchResponse {
                        account_id,
                        ..Default::default()
                    },
                )?;
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
        let buf = data::encode_message_heap!(self, 80 + resp.roles.len(), msg => {
            let mut fetch = msg.init_admin_fetch_response();
            fetch.set_account_id(resp.account_id);
            fetch.set_found(resp.found);
            fetch.set_whitelisted(resp.whitelisted);
            let _ = fetch.set_roles(resp.roles);
        })?;

        client.send_data_bufkind(buf);
        Ok(())
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

    async fn wrap_punish(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        reason: &str,
        expires_at: i64,
        r#type: UserPunishmentType,
    ) -> HandlerResult<()> {
        must_be_able(
            client,
            match r#type {
                UserPunishmentType::Ban => ActionType::Ban,
                UserPunishmentType::Mute => ActionType::Mute,
                UserPunishmentType::RoomBan => ActionType::RoomBan,
            },
        )?;

        // TODO: make punishments live, if the user is online, they should be punished immediately

        let users = self.module::<UsersModule>();
        let result = users
            .admin_punish_user(client.account_id(), account_id, reason, expires_at, r#type)
            .await;

        self.send_admin_db_result(client, result)?;

        Ok(())
    }

    async fn wrap_unpunish(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        r#type: UserPunishmentType,
    ) -> HandlerResult<()> {
        must_be_able(
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

        Ok(())
    }

    pub async fn handle_admin_edit_roles(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        role_ids: &[u8],
    ) -> HandlerResult<()> {
        must_be_able(client, ActionType::EditRoles)?;

        let users = self.module::<UsersModule>();
        let result = users.admin_edit_roles(client.account_id(), account_id, role_ids).await;

        self.send_admin_db_result(client, result)?;

        Ok(())
    }

    pub async fn handle_admin_set_password(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
        password: &str,
    ) -> HandlerResult<()> {
        must_be_able(client, ActionType::SetPassword)?;

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
    ) -> HandlerResult<()> {
        must_admin_auth(client)?;

        let users = self.module::<UsersModule>();
        let result = users.admin_update_user(account_id, username).await;

        self.send_admin_db_result(client, result)?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
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

        let cap = 64 + logs.len() * 64 + users.len() * 40;

        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut msg = msg.reborrow().init_admin_logs_response();

            let mut accounts = msg.reborrow().init_accounts(users.len() as u32);
            for (i, user) in users.iter().enumerate() {
                let mut acc = accounts.reborrow().get(i as u32);
                acc.set_username(&user.username);
                acc.set_account_id(user.account_id);
                acc.set_user_id(user.user_id);
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
}
