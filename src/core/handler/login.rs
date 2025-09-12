use std::{borrow::Cow, sync::Arc};

use qunet::buffers::ByteWriter;
use server_shared::{data::PlayerIconData, schema::main::LoginFailedReason};

use crate::{
    auth::{AuthModule, AuthVerdict, ClientAccountData, LoginKind},
    rooms::RoomModule,
    users::UsersModule,
};

use super::{ConnectionHandler, util::*};

impl ConnectionHandler {
    pub async fn handle_login_attempt(
        &self,
        client: &ClientStateHandle,
        kind: LoginKind<'_>,
        icons: PlayerIconData,
        uident: Option<&[u8]>,
    ) -> HandlerResult<()> {
        let auth = self.module::<AuthModule>();

        if client.authorized() {
            // if the client is already authorized, ignore the login attempt
            debug!("[{}] ignoring repeated login attempt", client.address);
            return Ok(());
        }

        let uident = match &kind {
            LoginKind::Argon(_, _) | LoginKind::UserToken(_, _) => {
                match uident.and_then(|x| x.try_into().ok()) {
                    Some(x) => Some(x),
                    None => {
                        debug!("[{}] received login with no uident", client.address);
                        return Ok(());
                    }
                }
            }

            LoginKind::Plain(_) => None,
        };

        match auth.handle_login(kind).await {
            AuthVerdict::Success(data) => {
                // verify that the data is absoultely valid
                if data.account_id != 0
                    && data.user_id != 0
                    && data.username.is_ascii()
                    && !data.username.is_empty()
                {
                    self.on_login_success(client, data, icons, uident).await?;
                } else {
                    self.on_login_failed(client, LoginFailedReason::InvalidAccountData)?;
                }
            }

            AuthVerdict::Failed(reason) => {
                self.on_login_failed(client, reason)?;
            }

            AuthVerdict::LoginRequired => {
                let argon_url = auth.argon_url().unwrap();

                let buf = data::encode_message_heap!(self, 48 + argon_url.len(), msg => {
                    let mut login_req = msg.reborrow().init_login_required();
                    login_req.set_argon_url(argon_url);
                })?;

                client.send_data_bufkind(buf);
            }
        }

        Ok(())
    }

    async fn on_login_success(
        &self,
        client: &ClientStateHandle,
        data: ClientAccountData,
        icons: PlayerIconData,
        uident: Option<[u8; 32]>,
    ) -> HandlerResult<()> {
        let auth = self.module::<AuthModule>();
        let rooms = self.module::<RoomModule>();
        let users = self.module::<UsersModule>();

        // query the database to check the user's data
        let user = match users.get_user(data.account_id).await {
            Ok(user) => user,
            Err(e) => {
                warn!("[{}] failed to get user data: {}", client.address, e);
                return self.on_login_failed(client, data::LoginFailedReason::InternalDbError);
            }
        };

        if let Some(uident) = uident {
            client.set_uident(uident);
        }

        if let Some(user) = user {
            // do some checks

            if let Some(username) = &user.username
                && username.as_str() != data.username.as_str()
            {
                // update the username in the database
                let _ = users.update_username(data.account_id, &data.username).await;
            }

            if let Some(uident) = uident {
                let uident = hex::encode(uident);

                if user.active_ban.is_some()
                    || user.active_mute.is_some()
                    || user.active_room_ban.is_some()
                {
                    if let Err(e) = users.insert_uident(data.account_id, &uident).await {
                        warn!(
                            "[{}] failed to insert ident ({}, {}): {e}",
                            client.address, data.account_id, uident
                        );
                    }
                }

                let accounts = match users.get_accounts_for_uident(&uident).await {
                    Ok(x) => x,
                    Err(e) => {
                        warn!("[{}] failed to get alt accounts: {}", client.address, e);
                        return self
                            .on_login_failed(client, data::LoginFailedReason::InternalDbError);
                    }
                };

                // TODO: flag account in some way??
                _ = accounts;
            }

            if let Some(ban) = &user.active_ban {
                // user is banned
                return self.send_banned(client, &ban.reason, ban.expires_at);
            }

            // update various stuff

            client.set_role(users.compute_from_user(&user));

            client.set_active_punishments(user.active_mute, user.active_room_ban);
            client.set_admin_password_hash(user.admin_password_hash);
        } else {
            client.set_role(users.compute_from_roles(data.account_id, std::iter::empty()));
        }

        info!("[{}] {} ({}) logged in", client.address, data.username, data.account_id);
        client.set_icons(icons);

        if let Some(old_client) = self.all_clients.insert(data.account_id, Arc::downgrade(client)) {
            // there already was a client with this account ID, disconnect them
            if let Some(old_client) = old_client.upgrade() {
                old_client.disconnect(Cow::Borrowed("Duplicate login detected, the same account logged in from a different location"));
            }
        }

        client.set_account_data(data.clone());

        // put the user in the global room
        rooms.force_join_room(client, &self.game_server_manager, rooms.global_room()).await;

        // refresh the user's user token (or generate a new one)
        let client_role_lock = client.role();
        let client_role = client_role_lock.as_ref().unwrap();
        let roles_str = users.make_role_string(&client_role.roles);
        let token = auth.generate_user_token(
            data.account_id,
            data.user_id,
            &data.username,
            &roles_str,
            client_role.name_color.as_ref(),
        );

        // send login success message with all servers
        let servers = self.game_server_manager.servers();
        let all_roles = users.get_roles();

        // roughly estimate how many bytes will it take to encode the response
        let cap = 128 + token.len() + servers.len() * 256 + all_roles.len() * 128;

        let mut color_buf = [0u8; 256];

        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut login_ok = msg.reborrow().init_login_ok();
            login_ok.set_new_token(&token);

            let mut srvs = login_ok.reborrow().init_servers(servers.len() as u32);

            for (i, srv) in servers.iter().enumerate() {
                let server = srvs.reborrow().get(i as u32);
                self.encode_game_server(&srv.data, server);
            }

            // encode all roles
            let mut all_roles_ser = login_ok.reborrow().init_all_roles(all_roles.len() as u32);

            for (i, role) in all_roles.iter().enumerate() {
                let mut role_ser = all_roles_ser.reborrow().get(i as u32);
                role_ser.set_string_id(&role.id);
                role_ser.set_icon(&role.icon);

                let mut role_buf = ByteWriter::new(&mut color_buf);
                role.name_color.encode(&mut role_buf);
                role_ser.set_name_color(role_buf.written());
            }

            // encode user's roles
            if let Err(e) = login_ok.reborrow().set_user_roles(client_role.roles.as_slice()) {
                warn!("[{}] failed to encode user roles: {}", client.address, e);
            }

            login_ok.set_is_moderator(client_role.can_moderate());
            login_ok.set_can_mute(client_role.can_mute);
            login_ok.set_can_ban(client_role.can_ban);
            login_ok.set_can_set_password(client_role.can_set_password);
            login_ok.set_can_edit_roles(client_role.can_edit_roles);
            login_ok.set_can_send_features(client_role.can_send_features);
            login_ok.set_can_rate_features(client_role.can_rate_features);

            if let Some(nc) = &client_role.name_color {
                let mut role_buf = ByteWriter::new(&mut color_buf);
                nc.encode(&mut role_buf);
                login_ok.set_name_color(role_buf.written());
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    #[inline]
    fn on_login_failed(
        &self,
        client: &ClientState<Self>,
        reason: data::LoginFailedReason,
    ) -> HandlerResult<()> {
        let buf = data::encode_message!(self, 40, msg => {
            let mut login_failed = msg.reborrow().init_login_failed();
            login_failed.set_reason(reason);
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }
}
