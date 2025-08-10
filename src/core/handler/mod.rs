use std::{
    borrow::Cow,
    net::SocketAddr,
    num::NonZeroI64,
    sync::{Arc, OnceLock, Weak},
    time::Duration,
};

use dashmap::DashMap;
use qunet::{
    message::MsgData,
    server::{
        Server as QunetServer, ServerHandle as QunetServerHandle, WeakServerHandle,
        app_handler::{AppHandler, AppResult},
    },
};
use rustc_hash::FxHashSet;
use server_shared::{
    data::{GameServerData, PlayerIconData},
    encoding::heapless_str_from_reader,
};
use state::TypeMap;

use crate::{
    auth::{ClientAccountData, LoginKind},
    core::{
        client_data::ClientData,
        config::Config,
        data::{self, decode_message_match},
        game_server::{GameServerHandler, GameServerManager},
        module::ServerModule,
    },
    rooms::{RoomModule, RoomSettings},
};

mod admin;
mod login;
mod rooms;
mod session;
mod util;
use util::*;
pub use util::{ClientState, ClientStateHandle, WeakClientStateHandle};

pub struct ConnectionHandler {
    modules: TypeMap![Send + Sync],
    // we use a weak handle here to avoid ref cycles, which will make it impossible to drop the server
    server: OnceLock<WeakServerHandle<Self>>,
    game_server_manager: GameServerManager,
    config: Config,

    all_clients: DashMap<i32, WeakClientStateHandle>,
    player_counts: DashMap<u64, usize>,
}

impl AppHandler for ConnectionHandler {
    type ClientData = ClientData;

    async fn on_launch(&self, server: QunetServerHandle<Self>) -> AppResult<()> {
        let _ = self.server.set(server.make_weak());

        info!("Globed central server is running!");

        let status_intv = if cfg!(debug_assertions) {
            Duration::from_mins(15)
        } else {
            Duration::from_mins(60)
        };

        server
            .schedule(status_intv, |server| async move {
                server.print_server_status();
                // TODO: shrink server buffer pool here to reclaim memory?
                info!(" - Authorized clients: {}", server.handler().all_clients.len());
                info!(
                    " - Active game sessions: {} (total players: {})",
                    server.handler().player_counts.len(),
                    server.handler().player_counts.iter().map(|mref| *mref.value()).sum::<usize>()
                );

                let rooms = server.handler().module::<RoomModule>();
                info!(" - Room count: {}", rooms.get_room_count());
            })
            .await;

        Ok(())
    }

    async fn on_client_connect(
        &self,
        _server: &QunetServer<Self>,
        connection_id: u64,
        address: SocketAddr,
        kind: &str,
    ) -> AppResult<Self::ClientData> {
        if self.server.get().is_none() {
            return Err("server not initialized yet".into());
        }

        info!(
            "Client connected: connection_id={}, address={}, kind={}",
            connection_id, address, kind
        );

        Ok(ClientData::default())
    }

    async fn on_client_disconnect(&self, _server: &QunetServer<Self>, client: &ClientStateHandle) {
        let account_id = client.account_id();

        debug!("[{} @ {}] client disconnected", account_id, client.address);

        if account_id != 0 {
            let rooms = self.module::<RoomModule>();
            rooms.cleanup_player(client, &self.game_server_manager).await;

            // remove only if the client has not been replaced by a newer login
            self.all_clients.remove_if(&account_id, |_, current_client| {
                Weak::ptr_eq(current_client, &Arc::downgrade(client))
            });

            let _ = self.handle_leave_session(client).await;
        }
    }

    async fn post_shutdown(&self, _server: &QunetServer<Self>) -> AppResult<()> {
        // by this point all connections have been dropped, we should clean up any resources
        info!("Cleaning up resources");
        let rooms = self.module::<RoomModule>();
        rooms.cleanup_everything().await;

        Ok(())
    }

    async fn on_client_data(
        &self,
        _server: &QunetServer<Self>,
        client: &ClientStateHandle,
        data: MsgData<'_>,
    ) {
        let result = decode_message_match!(self, data, unpacked_data, {
            LoginUToken(message) => {
                let account_id = message.get_account_id();
                let token = message.get_token()?.to_str()?;
                let icons = PlayerIconData::from_reader(message.get_icons()?)?;

                self.handle_login_attempt(client, LoginKind::UserToken(account_id, token), icons).await
            },

            LoginArgon(message) => {
                let account_id = message.get_account_id();
                let token = message.get_token()?.to_str()?;
                let icons = PlayerIconData::from_reader(message.get_icons()?)?;

                self.handle_login_attempt(client, LoginKind::Argon(account_id, token), icons).await
            },

            LoginPlain(message) => {
                let data = message.get_data()?;
                let account_id = data.get_account_id();
                let user_id = data.get_user_id();
                let username = heapless_str_from_reader(data.get_username()?)?;
                let icons = PlayerIconData::from_reader(message.get_icons()?)?;

                unpacked_data.reset(); // free up memory

                self.handle_login_attempt(client, LoginKind::Plain(ClientAccountData {
                    account_id, user_id, username
                }), icons).await
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

            RequestPlayerCounts(message) => {
                let levels = message.get_levels()?;
                let mut out_levels = heapless::Vec::<u64, 128>::new();

                for level in levels.iter().take(out_levels.capacity()) {
                    let _ = out_levels.push(level);
                }

                unpacked_data.reset(); // free up memory

                self.handle_request_player_counts(client, &out_levels)
            },

            CreateRoom(message) => {
                let name: heapless::String<64> = heapless_str_from_reader(message.get_name()?)?;
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

            LeaveRoom(_message) => {
                unpacked_data.reset(); // free up memory

                self.handle_leave_room(client).await
            },

            CheckRoomState(_message) => {
                unpacked_data.reset(); // free up memory

                self.handle_check_room_state(client).await
            },

            RequestRoomList(_message) => {
                unpacked_data.reset(); // free up memory

                self.handle_request_room_list(client)
            },

            JoinSession(message) => {
                let id = message.get_session_id();
                unpacked_data.reset(); // free up memory

                self.handle_join_session(client, id).await
            },

            LeaveSession(_message) => {
                unpacked_data.reset(); // free up memory

                self.handle_leave_session(client).await
            },

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
                let account_id = message.get_account_id();

                self.handle_admin_fetch_user(client, account_id).await
            },

            AdminFetchLogs(message) => {
                let issuer = message.get_issuer();
                let target = message.get_target();
                let r#type = message.get_type()?.to_str()?;
                let before = message.get_before();
                let after = message.get_after();

                self.handle_admin_fetch_logs(client, issuer, target, r#type, before, after).await
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

                self.handle_admin_update_user(client, account_id, username).await
            },
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
}

impl ConnectionHandler {
    pub fn new(config: Config) -> Self {
        Self {
            modules: <TypeMap![Send + Sync]>::new(),
            server: OnceLock::new(),
            game_server_manager: GameServerManager::new(),
            config,
            all_clients: DashMap::new(),
            player_counts: DashMap::new(),
        }
    }

    pub fn insert_module<T: ServerModule>(&self, module: T) {
        self.modules.set(module);
    }

    /// Get a module by type. Panics if the module is not found.
    pub fn module<T: ServerModule>(&self) -> &T {
        self.modules.get()
    }

    /// Get a module by type, returning `None` if the module is not found.
    pub fn opt_module<T: ServerModule>(&self) -> Option<&T> {
        self.modules.try_get()
    }

    pub fn freeze(&mut self) {
        self.modules.freeze();
        self.config.freeze();
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Obtain a reference to the server. This must not be called before the server is launched and `on_launch` is called.
    fn server(&self) -> QunetServerHandle<Self> {
        self.server
            .get()
            .expect("Server not initialized yet")
            .upgrade()
            .expect("Server has shut down")
    }

    // Handling of game servers.

    pub async fn notify_game_server_handler_started(
        &self,
        server: QunetServerHandle<GameServerHandler>,
    ) {
        self.game_server_manager.set_server(server.make_weak());
    }

    pub async fn handle_game_server_connect(
        &self,
        client: Arc<ClientState<GameServerHandler>>,
        data: GameServerData,
    ) -> HandlerResult<()> {
        self.game_server_manager.add_server(client, data);

        // TODO: notify all clients about the change
        Ok(())
    }

    pub async fn handle_game_server_disconnect(&self, client: Arc<ClientState<GameServerHandler>>) {
        if let Some(_srv) = self.game_server_manager.remove_server(&client) {
            // TODO: notify all clients about the change
            // TODO: reset active session of clients that were connected to this server ?
        } else {
            error!(
                "[{} @ {}] unknown game server disconnected!",
                client.connection_id, client.address
            );
        }
    }

    #[inline]
    pub async fn handle_game_server_room_created(&self, room_id: u32) {
        self.game_server_manager.ack_room_created(room_id).await;
    }

    // Misc encoding stuff

    fn encode_game_server(
        &self,
        srv: &GameServerData,
        mut server: server_shared::schema::shared::game_server::Builder<'_>,
    ) {
        server.set_id(srv.id);
        server.set_name(&srv.name);
        server.set_address(&srv.address);
        server.set_string_id(&srv.string_id);
        server.set_region(&srv.region);
    }

    // Handling of clients.

    fn find_client(&self, account_id: i32) -> Option<ClientStateHandle> {
        self.all_clients.get(&account_id).and_then(|x| x.upgrade())
    }

    fn send_banned(
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

    fn handle_update_own_data(
        &self,
        client: &ClientStateHandle,
        icons: Option<PlayerIconData>,
        friends: Option<FxHashSet<i32>>,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        if let Some(icons) = icons {
            client.set_icons(icons);
        };

        if let Some(friends) = friends {
            client.set_friends(friends);
        };

        Ok(())
    }

    fn handle_request_player_counts(
        &self,
        client: &ClientStateHandle,
        sessions: &[u64],
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let mut out_vals = heapless::Vec::<(u64, u16), 128>::new();
        debug_assert!(sessions.len() <= out_vals.capacity());

        for &sess in sessions {
            if let Some(count) = self.player_counts.get(&sess) {
                let _ = out_vals.push((sess, *count as u16));
                // TODO: maybe do a zero optimization?
            }
        }

        // TODO: benchmark size properly
        let cap = 40 + out_vals.len() * 12;

        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut player_counts = msg.reborrow().init_player_counts();

            let mut level_ids = player_counts.reborrow().init_level_ids(out_vals.len() as u32);
            for (n, (level_id, _)) in out_vals.iter().enumerate() {
                level_ids.set(n as u32, *level_id);
            }

            let mut counts = player_counts.reborrow().init_counts(out_vals.len() as u32);
            for (n, (_, count)) in out_vals.iter().enumerate() {
                counts.set(n as u32, *count);
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }
}
