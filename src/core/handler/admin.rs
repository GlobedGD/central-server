use std::{borrow::Cow, fmt::Display};

use crate::users::{UserPunishmentType, UsersModule};

use super::{ConnectionHandler, util::*};

enum ActionType {
    Kick,
    Notice,
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
        let buf = data::encode_message!(self, 40, msg => {
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

    pub async fn handle_admin_notice(
        &self,
        client: &ClientStateHandle,
        account_id: Option<i32>,
        message: &str,
        can_reply: bool,
    ) -> HandlerResult<()> {
        must_be_able(client, ActionType::Notice)?;

        let users = self.module::<UsersModule>();

        if let Some(user) = account_id.and_then(|id| self.find_client(id)) {
            let _ = users.log_notice(client.account_id(), user.account_id(), message).await;
            self.send_notice(client, &user, message, can_reply, false)?; // TODO: show_sender
        } else {
            // send a notice to everyone!
            let _ = users.log_notice(client.account_id(), 0, message).await;

            for user in self.all_clients.iter().filter_map(|x| x.value().upgrade()) {
                self.send_notice(client, &user, message, can_reply, false)?; // TODO: show_sender
            }
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
        let buf = data::encode_message_heap!(self, 64 + message.len(), msg => {
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
}
