use std::{borrow::Cow, num::NonZeroI64};

use rustc_hash::FxHashSet;
use server_shared::{
    UserSettings,
    data::{PlayerIconData, SRVC_MAGIC},
    encoding::{DataDecodeError, heapless_str_from_reader},
    qunet::{buffers::ByteWriter, message::MsgData, server::Server as QunetServer},
    schema::main::Platform,
};
use tracing::{debug, warn};

use crate::{
    auth::{ClientAccountData, LoginKind},
    core::{
        data::{self, decode_message_match},
        handler::{ClientStateHandle, ConnectionHandler, util::HandlerResult},
    },
    rooms::RoomSettings,
    users::{ComputedRole, UsersModule},
};

impl ConnectionHandler {
    pub async fn handle_client_data(
        &self,
        _server: &QunetServer<Self>,
        client: &ClientStateHandle,
        data: MsgData<'_>,
    ) {
        // cheap check since we already errored, sometimes people put in the wrong address
        // and try to connect their game server here instead of the game server handler
        if data.len() >= 8 {
            let magic = u64::from_le_bytes(data[..8].try_into().unwrap());
            if magic == SRVC_MAGIC {
                client.disconnect("This port accepts client connections, not game server connections. You likely input the wrong port as part of the 'central_server_url' option in your game server configuration.");
                debug!(
                    "[{} @ {}] disconnected client that sent srvc magic",
                    client.connection_id, client.address
                );
                return;
            }
        }

        let result = decode_message_match!(self, data, unpacked_data, {
            Login(message) => {
                let data = decode_login_data(message)?;

                self.handle_login_attempt(client, data).await
            },

            UpdateOwnData(message) => {
                let icons = if message.has_icons() {
                    Some(PlayerIconData::from_reader(message.get_icons()?)?)
                } else {
                    None
                };

                let fl = if message.has_friend_list() {
                    let mut fl = FxHashSet::default();
                    let friend_list = message.get_friend_list()?;
                    for friend in friend_list.iter().take(500) { // limit to 500 friends to prevent evil stuff
                        fl.insert(friend);
                    }

                    Some(fl)
                } else {
                    None
                };

                self.handle_update_own_data(client, icons, fl)
            },

            UpdateUserSettings(message) => {
                let settings = UserSettings::from_reader(message.get_settings()?);
                unpacked_data.reset(); // free up memory

                self.handle_update_user_settings(client, settings)
            },

            RequestPlayerCounts(message) => {
                let levels = message.get_levels()?;
                let mut out_levels = heapless::Vec::<u64, 128>::new();

                for level in levels.iter().take(out_levels.capacity()) {
                    let _ = out_levels.push(level);
                }

                unpacked_data.reset(); // free up memory

                self.handle_request_player_counts(client, &out_levels)
            },

            RequestLevelList(_msg) => {
                unpacked_data.reset(); // free up memory

                self.handle_request_level_list(client).await
            },

            RequestGlobalPlayerList(msg) => {
                let name_filter = heapless_str_from_reader::<32>(msg.get_name_filter()?)?;
                unpacked_data.reset(); // free up memory

                self.handle_request_global_player_list(client, &name_filter).await
            },

            CreateRoom(message) => {
                let name = heapless_str_from_reader::<32>(message.get_name()?)?;
                let settings = RoomSettings::from_reader(message.get_settings()?)?;
                let passcode = message.get_passcode();

                unpacked_data.reset(); // free up memory

                self.handle_create_room(client, &name, passcode, settings).await
            },

            JoinRoom(message) => {
                let id = message.get_room_id();
                let passcode = message.get_passcode();

                unpacked_data.reset(); // free up memory

                self.handle_join_room(client, id, passcode).await
            },

            JoinRoomByToken(message) => {
                let token = message.get_token();
                unpacked_data.reset(); // free up memory

                self.handle_join_room_by_token(client, token).await
            },

            LeaveRoom(_message) => {
                unpacked_data.reset(); // free up memory

                self.handle_leave_room(client).await
            },

            CheckRoomState(_message) => {
                unpacked_data.reset(); // free up memory

                self.handle_check_room_state(client).await
            },

            RequestRoomPlayers(msg) => {
                let name_filter = heapless_str_from_reader::<32>(msg.get_name_filter()?)?;

                unpacked_data.reset(); // free up memory

                self.handle_request_room_players(client, &name_filter).await
            },

            RequestRoomList(msg) => {
                let name_filter = heapless_str_from_reader::<32>(msg.get_name_filter()?)?;
                let page = msg.get_page();

                unpacked_data.reset(); // free up memory

                self.handle_request_room_list(client, &name_filter, page)
            },

            AssignTeam(message) => {
                let account_id = message.get_account_id();
                let team_id = message.get_team_id();

                unpacked_data.reset(); // free up memory

                self.handle_assign_team(client, account_id, team_id)
            },

            CreateTeam(message) => {
                let color = message.get_color();
                unpacked_data.reset(); // free up memory

                self.handle_create_team(client, color)
            },

            DeleteTeam(message) => {
                let team_id = message.get_team_id();

                unpacked_data.reset(); // free up memory

                self.handle_delete_team(client, team_id)
            },

            UpdateTeam(message) => {
                let team_id = message.get_team_id();
                let color = message.get_color();

                unpacked_data.reset(); // free up memory

                self.handle_update_team(client, team_id, color)
            },

            GetTeamMembers(_message) => {
                unpacked_data.reset(); // free up memory

                self.handle_get_team_members(client)
            },

            RoomOwnerAction(message) => {
                let r#type = message.get_type()?;
                let target = message.get_target();

                unpacked_data.reset(); // free up memory

                self.handle_room_owner_action(client, r#type, target).await
            },

            UpdateRoomSettings(message) => {
                let settings = RoomSettings::from_reader(message.get_settings()?)?;
                unpacked_data.reset(); // free up memory

                self.handle_update_room_settings(client, settings).await
            },

            InvitePlayer(message) => {
                let player = message.get_player();
                unpacked_data.reset(); // free up memory

                self.handle_invite_player(client, player).await
            },

            UpdatePinnedLevel(message) => {
                let id = message.get_id();
                unpacked_data.reset(); // free up memory

                self.handle_update_pinned_level(client, id).await
            },

            //

            JoinSession(message) => {
                let id = message.get_session_id();
                let author_id = message.get_author_id();
                unpacked_data.reset(); // free up memory

                self.handle_join_session(client, id, author_id).await
            },

            LeaveSession(_message) => {
                unpacked_data.reset(); // free up memory

                self.handle_leave_session(client).await
            },

            //

            FetchCredits(_message) => {
                unpacked_data.reset(); // free up memory
                self.handle_fetch_credits(client)
            },

            GetUserState(_message) => {
                unpacked_data.reset(); // free up memory
                self.handle_get_user_state(client).await
            },

            GetDiscordLinkState(_message) => {
                unpacked_data.reset(); // free up memory
                self.handle_get_discord_link_state(client).await
            },

            SetDiscordPairingState(message) => {
                let state = message.get_state();
                unpacked_data.reset(); // free up memory

                self.handle_set_discord_pairing_state(client, state)
            },

            DiscordLinkConfirm(message) => {
                let id = message.get_id();
                let accept = message.get_accept();
                unpacked_data.reset(); // free up memory

                self.handle_discord_link_confirm(client, id, accept)
            },

            RequestDiscordOauth(_message) => {
                unpacked_data.reset(); // free up memory
                self.handle_discord_get_oauth(client)
            },

            RequestDiscordUnlink(_message) => {
                unpacked_data.reset(); // free up memory
                self.handle_discord_unlink(client).await
            },

            //

            AdminLogin(message) => {
                let password = message.get_password()?.to_str()?;

                self.handle_admin_login(client, password).await
            },

            AdminKick(message) => {
                let account_id = message.get_account_id();
                let reason = message.get_message()?.to_str()?;

                self.handle_admin_kick(client, account_id, reason).await
            },

            AdminNotice(message) => {
                let target_user = message.get_target_user()?.to_str()?;
                let room_id = message.get_room_id();
                let level_id = message.get_level_id();
                let can_reply = message.get_can_reply();
                let show_sender = message.get_show_sender();
                let message = message.get_message()?.to_str()?;

                self.handle_admin_notice(client, target_user, room_id, level_id, message, can_reply, show_sender).await
            },

            AdminNoticeEveryone(message) => {
                let message = message.get_message()?.to_str()?;
                self.handle_admin_notice_everyone(client, message).await
            },

            AdminFetchUser(message) => {
                let query = message.get_query()?.to_str()?;
                let query_num = message.get_query_num();

                self.handle_admin_fetch_user(client, query, query_num).await
            },

            AdminFetchLogs(message) => {
                let issuer = message.get_issuer();
                let target = message.get_target();
                let r#type = message.get_type()?.to_str()?;
                let before = message.get_before();
                let after = message.get_after();
                let page = message.get_page();

                self.handle_admin_fetch_logs(client, issuer, target, r#type, before, after, page).await
            },

            AdminBan(message) => {
                let account_id = message.get_account_id();
                let reason = message.get_reason()?.to_str()?;
                let expires_at = message.get_expires_at();

                self.handle_admin_ban(client, account_id, reason, expires_at).await
            },

            AdminUnban(message) => {
                let account_id = message.get_account_id();

                self.handle_admin_unban(client, account_id).await
            },

            AdminRoomBan(message) => {
                let account_id = message.get_account_id();
                let reason = message.get_reason()?.to_str()?;
                let expires_at = message.get_expires_at();

                self.handle_admin_room_ban(client, account_id, reason, expires_at).await
            },

            AdminRoomUnban(message) => {
                let account_id = message.get_account_id();

                self.handle_admin_room_unban(client, account_id).await
            },

            AdminMute(message) => {
                let account_id = message.get_account_id();
                let reason = message.get_reason()?.to_str()?;
                let expires_at = message.get_expires_at();

                self.handle_admin_mute(client, account_id, reason, expires_at).await
            },

            AdminUnmute(message) => {
                let account_id = message.get_account_id();

                self.handle_admin_unmute(client, account_id).await
            },

            AdminEditRoles(message) => {
                let account_id = message.get_account_id();
                let mut roles = heapless::Vec::<u8, 64>::new();
                message.get_roles()?.iter().for_each(|x| {
                    let _ = roles.push(x);
                });

                self.handle_admin_edit_roles(client, account_id, &roles).await
            },

            AdminSetPassword(message) => {
                let account_id = message.get_account_id();
                let password = message.get_new_password()?.to_str()?;

                self.handle_admin_set_password(client, account_id, password).await
            },

            AdminUpdateUser(message) => {
                let account_id = message.get_account_id();
                let username = message.get_username()?.to_str()?;
                let cube = message.get_cube();
                let color1 = message.get_color1();
                let color2 = message.get_color2();
                let glow_color = message.get_glow_color();

                self.handle_admin_update_user(client, account_id, username, cube, color1, color2, glow_color).await
            },

            AdminFetchMods(_message) => {
                unpacked_data.reset();

                self.handle_admin_fetch_mods(client).await
            },

            AdminSetWhitelisted(message) => {
                let account_id = message.get_account_id();
                let whitelisted = message.get_whitelisted();

                unpacked_data.reset();

                self.handle_admin_set_whitelisted(client, account_id, whitelisted).await
            },

            AdminCloseRoom(message) => {
                let room_id = message.get_room_id();

                unpacked_data.reset();

                self.handle_admin_close_room(client, room_id).await
            },

            GetFeaturedLevel(_message) => {
                unpacked_data.reset();

                #[cfg(feature = "featured-levels")]
                let res = self.handle_get_featured_level(client);
                #[cfg(not(feature = "featured-levels"))]
                let res = Ok(());

                res
            },

            GetFeaturedList(message) => {
                #[allow(unused)]
                let page = message.get_page();

                unpacked_data.reset();

                #[cfg(feature = "featured-levels")]
                let res = self.handle_get_featured_list(client, page).await;
                #[cfg(not(feature = "featured-levels"))]
                let res = Ok(());

                res
            },

            SendFeaturedLevel(message) => {
                #[cfg(feature = "featured-levels")]
                let res = {
                    let level_id = message.get_level_id();
                    let level_name = message.get_level_name()?.to_str()?;
                    let author_id = message.get_author_id();
                    let author_name = message.get_author_name()?.to_str()?;
                    let rate_tier = message.get_rate_tier();
                    let note = message.get_note()?.to_str()?;
                    let queue = message.get_queue();

                    self.handle_send_featured_level(client, level_id, level_name, author_id, author_name, rate_tier, note, queue).await
                };

                #[cfg(not(feature = "featured-levels"))]
                let res = {
                    let _ = message;
                    Ok(())
                };

                res
            },

            NoticeReply(message) => {
                let target_user = message.get_receiver_id();
                let message = message.get_message()?.to_str()?;

                self.handle_notice_reply(client, target_user, message).await
            },

            FetchUser(message) => {
                let account_id = message.get_account_id();
                self.handle_fetch_user(client, account_id).await
            },

            Events(message) => {
                let Some(encoder) = client.event_encoder() else {
                    return Ok(Ok(()));
                };

                try {
                    let events = encoder.decode_events_owned(message).map_err(|e| e.into())?;
                    self.handle_events(client, events).await?;
                }
            }
        });

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                warn!("[{}] handler error: {}", client.address, e);
            }

            Err(e) => {
                warn!("[{}] failed to decode message: {}", client.address, e);
            }
        }
    }

    pub fn send_banned(
        &self,
        client: &ClientStateHandle,
        reason: &str,
        expires_at: Option<NonZeroI64>,
    ) -> HandlerResult<()> {
        let buf = data::encode_message_heap!(self, 64 + reason.len(), msg => {
            let mut banned = msg.reborrow().init_banned();
            banned.set_reason(reason);
            banned.set_expires_at(expires_at.map_or(0, |x| x.get()));
        })?;

        client.send_data_bufkind(buf);
        client.disconnect(Cow::Borrowed("user is banned"));

        Ok(())
    }

    pub fn send_muted(
        &self,
        client: &ClientStateHandle,
        reason: &str,
        expires_at: Option<NonZeroI64>,
    ) -> HandlerResult<()> {
        let buf = data::encode_message_heap!(self, 64 + reason.len(), msg => {
            let mut banned = msg.reborrow().init_muted();
            banned.set_reason(reason);
            banned.set_expires_at(expires_at.map_or(0, |x| x.get()));
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    pub fn send_warn(
        &self,
        client: &ClientStateHandle,
        message: impl AsRef<str>,
    ) -> HandlerResult<()> {
        let buf = data::encode_message_heap!(self, 48 + message.as_ref().len(), msg => {
            let mut warn = msg.reborrow().init_warn();
            warn.set_message(message.as_ref());
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    pub fn encode_ext_user_data(
        &self,
        role: &ComputedRole,
        token: &str,
        mut builder: data::extended_user_data::Builder<'_>,
    ) {
        let users = self.module::<UsersModule>();
        let can_name_rooms = role.can_name_rooms || !users.disallow_room_names();

        if let Err(e) = builder.set_roles(role.roles.as_slice()) {
            warn!("failed to encode user roles: {e}, roles: {:?}", role.roles);
        }

        builder.set_is_moderator(role.can_moderate());
        builder.set_can_mute(role.can_mute);
        builder.set_can_ban(role.can_ban);
        builder.set_can_set_password(role.can_set_password);
        builder.set_can_edit_roles(role.can_edit_roles);
        builder.set_can_send_features(role.can_send_features);
        builder.set_can_rate_features(role.can_rate_features);
        builder.set_can_name_rooms(can_name_rooms);
        builder.set_new_token(token);

        if let Some(nc) = role.name_color.as_ref() {
            let mut buf = [0u8; 512];
            let mut writer = ByteWriter::new(&mut buf);
            nc.encode(&mut writer);

            builder.set_name_color(writer.written());
        }
    }
}

#[allow(unused)]
pub struct LoginData<'a> {
    pub kind: LoginKind<'a>,
    pub icons: PlayerIconData,
    pub uident: Option<&'a [u8]>,
    pub settings: UserSettings,
    pub globed_version: &'a str,
    pub geode_version: &'a str,
    pub platform: &'a str,
    pub platform_desc: Option<&'a str>,
    pub event_dict: Option<&'a [u8]>,
}

fn decode_login_data<'a>(
    message: server_shared::schema::main::login_message::Reader<'a>,
) -> Result<LoginData<'a>, DataDecodeError> {
    use server_shared::schema::main::login_message::Which;

    let account_id = message.get_account_id();
    let icons = PlayerIconData::from_reader(message.get_icons()?)?;
    let uident = if message.has_uident() { Some(message.get_uident()?) } else { None };
    let settings = UserSettings::from_reader(message.get_settings()?);

    let kind = match message.which().map_err(|_| DataDecodeError::InvalidDiscriminant)? {
        Which::Utoken(m) => LoginKind::UserToken(account_id, m?.to_str()?),

        Which::Argon(m) => LoginKind::Argon(account_id, m?.to_str()?),

        Which::Plain(m) => {
            let data = m?;
            let username = heapless_str_from_reader(data.get_username()?)?;
            let user_id = data.get_user_id();

            LoginKind::Plain(ClientAccountData { account_id, user_id, username })
        }
    };

    let globed_version = message.get_globed_version()?.to_str()?;
    let geode_version = message.get_geode_version()?.to_str()?;
    let platform = match message.get_platform()? {
        Platform::Unknown => "unknown",
        Platform::Windows => "windows",
        Platform::Wine => "wine",
        Platform::MacArm => "macarm",
        Platform::MacIntel => "macintel",
        Platform::Android32 => "android32",
        Platform::Android64 => "android64",
        Platform::Ios => "ios",
    };

    let platform_desc = if message.has_platform_desc() {
        Some(message.get_platform_desc()?.to_str()?)
    } else {
        None
    };

    let event_dict = if message.has_event_dictionary() {
        Some(message.get_event_dictionary()?)
    } else {
        None
    };

    Ok(LoginData {
        kind,
        icons,
        uident,
        settings,
        globed_version,
        geode_version,
        platform,
        platform_desc,
        event_dict,
    })
}
