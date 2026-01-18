use std::borrow::Cow;

use crypto_secretbox::{KeyInit, aead::AeadMutInPlace};
use server_shared::qunet::buffers::{ByteReader, ByteReaderError, ByteWriter};
use server_shared::{UserSettings, data::PlayerIconData, schema::main::LoginFailedReason};
use thiserror::Error;

use crate::{
    auth::{AuthModule, AuthVerdict, ClientAccountData, LoginKind},
    rooms::RoomModule,
    users::UsersModule,
};

#[cfg(feature = "featured-levels")]
use crate::features::FeaturesModule;

#[cfg(feature = "discord")]
use crate::discord::{DiscordMessage, DiscordModule};

use super::{ConnectionHandler, util::*};

#[derive(Debug, Error)]
enum UidentDecodeError {
    #[error("Invalid key")]
    InvalidKey,
    #[error("Not enough data")]
    NotEnoughData,
    #[error("Invalid data: {0}")]
    InvalidData(#[from] ByteReaderError),
    #[error("Mismatch: expected account {0} but in token {1} was found")]
    AccountMismatch(i32, i32),
    #[error("Decryption failed: {0}")]
    Decryption(crypto_secretbox::Error),
}

impl ConnectionHandler {
    pub async fn handle_login_attempt(
        &self,
        client: &ClientStateHandle,
        kind: LoginKind<'_>,
        icons: PlayerIconData,
        uident: Option<&[u8]>,
        settings: UserSettings,
    ) -> HandlerResult<()> {
        let auth = self.module::<AuthModule>();
        let users = self.module::<UsersModule>();

        if client.authorized() {
            // if the client is already authorized, ignore the login attempt
            debug!("[{}] ignoring repeated login attempt", client.address);
            return Ok(());
        }

        let ttkey = auth.trust_token_key();

        let uident = if ttkey.is_empty() {
            None
        } else {
            match &kind {
                &LoginKind::Argon(accid, _) | &LoginKind::UserToken(accid, _) => {
                    match uident.and_then(|x| {
                        self.decrypt_uident(accid, x, ttkey)
                            .inspect_err(|e| warn!("Failed to decode uident from user: {e}"))
                            .ok()
                    }) {
                        Some(x) => Some(x),
                        None => {
                            debug!(
                                "[{} @ {}] rejecting login due to missing trust token",
                                client.address, accid
                            );
                            return Ok(());
                        }
                    }
                }

                LoginKind::Plain(_) => None,
            }
        };

        match auth.handle_login(kind).await {
            AuthVerdict::Success(data) => {
                // verify that the data is absoultely valid
                if data.account_id != 0
                    && data.user_id != 0
                    && data.username.is_ascii()
                    && !data.username.is_empty()
                {
                    // verify that the user is whitelisted if whitelist is enabled
                    if users.whitelist() && !users.is_whitelisted(data.account_id).await {
                        self.on_login_failed(client, LoginFailedReason::NotWhitelisted)?;
                    } else {
                        // success!
                        self.on_login_success(client, data, icons, uident, settings).await?;
                    }
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
        settings: UserSettings,
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

        client.set_settings(settings);

        let uident = uident.map(hex::encode);

        if let Some(user) = user {
            // do some checks

            if let Some(username) = &user.username
                && username.as_str() != data.username.as_str()
            {
                // update the username in the database
                let _ = users.update_username(data.account_id, &data.username).await;
            }

            if let Some(uident) = uident.as_ref() {
                if user.active_ban.is_some()
                    || user.active_mute.is_some()
                    || user.active_room_ban.is_some()
                {
                    if let Err(e) = users.insert_uident(data.account_id, uident).await {
                        warn!(
                            "[{}] failed to insert ident ({}, {}): {e}",
                            client.address, data.account_id, uident
                        );
                    }
                }
            }

            if let Some(ban) = &user.active_ban {
                // user is banned
                return self.send_banned(client, &ban.reason, ban.expires_at);
            }

            // update various stuff

            client.set_role(users.compute_from_user(&user));

            client.set_active_punishments(user.active_mute, user.active_room_ban);
            client.set_admin_password_hash(user.admin_password_hash);
            client.set_discord_linked(user.discord_id.is_some());
        } else {
            client.set_role(users.compute_from_roles(data.account_id, std::iter::empty()));
        }

        // check potential alt account
        if let Some(uident) = uident.as_ref() {
            let accounts = match users.get_accounts_for_uident(uident).await {
                Ok(x) => x,
                Err(e) => {
                    warn!("[{}] failed to get alt accounts: {}", client.address, e);
                    return self.on_login_failed(client, data::LoginFailedReason::InternalDbError);
                }
            };

            if accounts.iter().any(|&id| id != data.account_id) {
                match users.insert_uident(data.account_id, uident).await {
                    Ok(true) => {
                        // notify on discord
                        #[cfg(feature = "discord")]
                        self.module::<DiscordModule>().send_alert(DiscordMessage::new().content(
                            format!(
                                "⚠️ Potential alt account logged in: {} ({}), accounts: {:?}. Uident: {}",
                                data.username, data.account_id, accounts, uident
                            ),
                        ));

                        // put the user into the db
                        let _ = users.query_or_create_user(&format!("{}", data.account_id)).await;
                    }

                    Ok(false) => {}

                    Err(e) => warn!(
                        "[{}] failed to insert ident ({}, {}): {e}",
                        client.address, data.account_id, uident
                    ),
                }
            }
        }

        info!("[{}] {} ({}) logged in", client.address, data.username, data.account_id);
        client.set_icons(icons);

        if let Some(old_client) = self.clients.insert(data.account_id, &data.username, client) {
            // there already was a client with this account ID, disconnect them
            old_client.disconnect(Cow::Borrowed(
                "Duplicate login detected, the same account logged in from a different location",
            ));
        }

        // if the username has disallowed words, send a discord notification
        #[cfg(feature = "discord")]
        if self.is_disallowed(&data.username).await {
            self.module::<DiscordModule>().send_alert(DiscordMessage::new().content(format!(
                "⚠️ User logged in with disallowed terms in username: {} ({})",
                data.username, data.account_id
            )));
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
        let cap = 140 + token.len() + servers.len() * 256 + all_roles.len() * 128;

        let mut color_buf = [0u8; 256];

        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut login_ok = msg.reborrow().init_login_ok();

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
                role_ser.set_hide(role.hide);

                let mut role_buf = ByteWriter::new(&mut color_buf);
                role.name_color.encode(&mut role_buf);
                role_ser.set_name_color(role_buf.written());
            }

            // encode featured level
            #[cfg(feature = "featured-levels")]
            {
                let level = self.module::<FeaturesModule>().get_featured_level_meta();
                login_ok.set_featured_level(level.id);
                login_ok.set_featured_level_tier(level.rate_tier);
                login_ok.set_featured_level_edition(level.edition);
            }

            // encode user data
            self.encode_ext_user_data(client_role, &token, login_ok.reborrow().init_user_data());
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

    fn decrypt_uident(
        &self,
        account_id: i32,
        data: &[u8],
        key_str: &str,
    ) -> Result<[u8; 32], UidentDecodeError> {
        use crypto_secretbox::aead::heapless::Vec;

        if key_str.len() != 64 {
            return Err(UidentDecodeError::InvalidKey);
        }

        if data.len() < 40 || data.len() > 128 {
            return Err(UidentDecodeError::NotEnoughData);
        }

        let mut key = [0u8; 32];
        let _ = hex::decode_to_slice(key_str, &mut key);

        let mut cipher = crypto_secretbox::XSalsa20Poly1305::new((&key).into());

        let nonce = &data[..24];
        let ciphertext = &data[24..];

        let mut decrypt_buffer = Vec::<u8, 128>::new();
        let _ = decrypt_buffer.extend_from_slice(ciphertext);
        cipher
            .decrypt_in_place(nonce.into(), b"", &mut decrypt_buffer)
            .map_err(UidentDecodeError::Decryption)?;

        let mut reader = ByteReader::new(&decrypt_buffer);
        let rid = reader.read_i32()?;
        if rid != account_id {
            return Err(UidentDecodeError::AccountMismatch(account_id, rid));
        }

        let mut uident_out = [0u8; 32];
        reader.read_bytes(&mut uident_out)?;

        Ok(uident_out)
    }
}
