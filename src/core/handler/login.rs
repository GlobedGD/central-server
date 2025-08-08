use std::{borrow::Cow, sync::Arc};

use server_shared::data::PlayerIconData;

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
    ) -> HandlerResult<()> {
        let auth = self.module::<AuthModule>();

        if client.authorized() {
            // if the client is already authorized, ignore the login attempt
            debug!("[{}] ignoring repeated login attempt", client.address);
            return Ok(());
        }

        match auth.handle_login(kind).await {
            AuthVerdict::Success(data) => {
                self.on_login_success(client, data, icons).await?;
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

        if let Some(user) = user {
            // do some checks

            if let Some(username) = user.username
                && username.as_str() != data.username.as_str()
            {
                // update the username in the database
                let _ = users.update_username(data.account_id, &data.username).await;
            }

            if let Some(ban) = user.active_ban {
                // user is banned
                return self.send_banned(client, &ban.reason, ban.expires_at);
            }

            // update various stuff
            client.set_active_punishments(user.active_mute, user.active_room_ban);
            client.set_admin_password_hash(user.admin_password_hash);

            let computed_role = users.compute_from_roles(
                user.roles.as_deref().unwrap_or("").split(",").filter(|s| !s.is_empty()),
            );

            client.set_role(computed_role);
        } else {
            client.set_role(users.compute_from_roles(std::iter::empty()));
        }

        info!("[{}] {} ({}) logged in", client.address, data.username, data.account_id);
        client.set_icons(icons);

        // refresh the user's user token (or generate a new one)
        let client_roles = &client.role().unwrap().roles;
        let roles_str = users.make_role_string(client_roles);
        let token =
            auth.generate_user_token(data.account_id, data.user_id, &data.username, &roles_str);

        if let Some(old_client) = self.all_clients.insert(data.account_id, Arc::downgrade(client)) {
            // there already was a client with this account ID, disconnect them
            if let Some(old_client) = old_client.upgrade() {
                old_client.disconnect(Cow::Borrowed("Duplicate login detected, the same account logged in from a different location"));
            }
        }

        client.set_account_data(data);

        // put the user in the global room
        rooms.force_join_room(client, &self.game_server_manager, rooms.global_room()).await;

        // send login success message with all servers
        let servers = self.game_server_manager.servers();
        let all_roles = users.get_roles();

        // roughly estimate how many bytes will it take to encode the response
        let cap = 80 + token.len() + servers.len() * 256 + all_roles.len() * 128;

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
                role_ser.set_name_color(&role.name_color);
            }

            // encode user's roles
            if let Err(e) = login_ok.reborrow().set_user_roles(client_roles.as_slice()) {
                warn!("[{}] failed to encode user roles: {}", client.address, e);
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
