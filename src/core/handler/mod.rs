use std::{
    borrow::Cow,
    net::SocketAddr,
    num::NonZeroI64,
    path::Path,
    sync::{Arc, OnceLock, Weak},
    time::{Duration, SystemTime},
};

use dashmap::DashMap;
use parking_lot::Mutex;
use rustc_hash::FxHashSet;
use server_shared::qunet::{
    buffers::{BufPool, ByteWriter},
    message::{BufferKind, MsgData},
    server::{
        Server as QunetServer, ServerHandle as QunetServerHandle, WeakServerHandle,
        app_handler::{AppHandler, AppResult},
        stat_tracker::{FinishedConnection, OverallStats},
    },
};
use server_shared::{
    TypeMap, UserSettings,
    data::{GameServerData, PlayerIconData},
    encoding::{DataDecodeError, heapless_str_from_reader},
};

use crate::{
    auth::{ClientAccountData, LoginKind},
    core::{
        client_data::ClientData,
        config::Config,
        data::{self, decode_message_match},
        game_server::{GameServerHandler, GameServerManager, StoredGameServer},
        module::ServerModule,
    },
    rooms::{RoomModule, RoomSettings},
    users::{ComputedRole, UsersModule},
};

mod admin;
#[cfg(feature = "featured-levels")]
mod featured;
mod login;
mod misc;
mod rooms;
mod session;
mod util;
use util::*;
pub use util::{ClientState, ClientStateHandle, WeakClientStateHandle};

struct LevelEntry {
    player_count: u32,
    is_hidden: bool,
}

pub struct ConnectionHandler {
    modules: TypeMap,
    module_list: Mutex<Vec<Arc<dyn ServerModule>>>,
    // we use a weak handle here to avoid ref cycles, which will make it impossible to drop the server
    server: OnceLock<WeakServerHandle<Self>>,
    game_server_manager: GameServerManager,
    config: Config,

    all_clients: DashMap<i32, WeakClientStateHandle>,
    all_levels: DashMap<u64, LevelEntry>,
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

        server.schedule(status_intv, |server| async move {
            server.print_server_status();
            info!(" - Authorized clients: {}", server.handler().all_clients.len());
            info!(
                " - Active game sessions: {} (total players: {})",
                server.handler().all_levels.len(),
                server
                    .handler()
                    .all_levels
                    .iter()
                    .map(|mref| mref.value().player_count)
                    .sum::<u32>()
            );

            let rooms = server.handler().module::<RoomModule>();
            info!(" - Room count: {}", rooms.get_room_count());
        });

        // TODO: determine if this is really worth it?
        server.schedule(Duration::from_hours(12), |server| async move {
            let pool = server.get_buffer_pool();
            let prev_usage = pool.stats().total_heap_usage;
            pool.shrink();
            let new_usage = pool.stats().total_heap_usage;

            info!("Shrinking buffer pool to reclaim memory: {} -> {} bytes", prev_usage, new_usage);
        });

        // periodically clean up stat tracker stuff if enabled
        if server.stat_tracker().is_some() {
            server.schedule(Duration::from_mins(30), |server| async move {
                if let Some(t) = server.stat_tracker() {
                    info!("Cleaning up stale stat tracker data");
                    t.clear_past_older_than(Duration::from_hours(6));
                }
            });
        }

        for module in self.module_list.lock().iter() {
            module.on_launch(&server);
        }

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
            Login(message) => {
                let LoginData { kind, icons, uident, settings } = decode_login_data(message)?;

                self.handle_login_attempt(client, kind, icons, uident, settings).await
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

                self.handle_admin_fetch_user(client, query).await
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

    async fn on_sigusr1(&self, _server: &QunetServer<Self>) {
        self.dump_all_connections().await;
    }
}

impl ConnectionHandler {
    pub fn new(config: Config) -> Self {
        Self {
            modules: TypeMap::new(),
            module_list: Mutex::new(Vec::new()),
            server: OnceLock::new(),
            game_server_manager: GameServerManager::new(),
            config,
            all_clients: DashMap::new(),
            all_levels: DashMap::new(),
        }
    }

    pub fn insert_module<T: ServerModule>(&self, module: T) {
        self.modules.insert(module);
        let module: Arc<dyn ServerModule> = self.opt_module_owned::<T>().unwrap();
        self.module_list.lock().push(module);
    }

    /// Get a module by type. Panics if the module is not found.
    pub fn module<T: ServerModule>(&self) -> &T {
        self.opt_module().expect("non-existend module getter called")
    }

    /// Get a module by type, returning `None` if the module is not found.
    pub fn opt_module<T: ServerModule>(&self) -> Option<&T> {
        self.modules.get()
    }

    /// Get a module by type, returning `None` if the module is not found.
    pub fn opt_module_owned<T: ServerModule>(&self) -> Option<Arc<T>> {
        self.modules.get_owned()
    }

    pub fn freeze(&mut self) {
        self.modules.freeze();
        self.config.freeze();
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub async fn dump_all_connections(&self) -> Option<OverallStats> {
        let server = self.server();
        let st = server.stat_tracker()?;

        let conns = st.take_all_past();
        let overall = st.get_overall_stats();

        info!("== Overall connection stats ==");
        info!("Bytes sent: {}, received: {}", overall.bytes_tx, overall.bytes_rx);
        info!("Packets sent: {}, received: {}", overall.pkt_tx, overall.pkt_rx);
        info!("Total connections made: {}", overall.total_conns);
        info!(
            "Connections suspended: {}, resumed: {}",
            overall.total_suspends, overall.total_resumes
        );
        info!("Total keepalives exchanged: {}", overall.total_keepalives);

        let base_dir = std::env::current_dir().unwrap().join("conn-dumps");
        info!("Dumping {} connections to {base_dir:?}", conns.len());

        for conn in conns {
            // dump connection data
            let time_str = format_systime(conn.creation);
            let dir = base_dir.join(format!("{}-{}", time_str, conn.id));

            match dump_connection_data(&conn, &dir).await {
                Ok(()) => {
                    info!("Dumped connection {} to {:?}", conn.id, dir);
                }

                Err(e) => {
                    error!("Failed to dump connection {}: {}", conn.id, e);
                }
            }
        }

        Some(overall)
    }

    /// Obtain a reference to the server. This must not be called before the server is launched and `on_launch` is called.
    fn server(&self) -> QunetServerHandle<Self> {
        self.server
            .get()
            .expect("Server not initialized yet")
            .upgrade()
            .expect("Server has shut down")
    }

    pub fn level_count(&self) -> usize {
        self.all_levels.len()
    }

    pub fn override_level_hidden(&self, session: u64, hidden: bool) -> bool {
        if let Some(mut ent) = self.all_levels.get_mut(&session) {
            ent.is_hidden = hidden;
            true
        } else {
            false
        }
    }

    // Handling of game servers.

    pub fn get_game_servers(&self) -> Arc<Vec<StoredGameServer>> {
        self.game_server_manager.servers()
    }

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
        self.notify_servers_changed().await;

        Ok(())
    }

    pub async fn handle_game_server_disconnect(&self, client: Arc<ClientState<GameServerHandler>>) {
        if let Some(_srv) = self.game_server_manager.remove_server(&client) {
            // TODO: reset active session of clients that were connected to this server ?
            self.notify_servers_changed().await;
        } else {
            error!(
                "[{} @ {}] unknown game server disconnected!",
                client.connection_id, client.address
            );
        }
    }

    pub async fn notify_servers_changed(&self) {
        let servers = self.game_server_manager.servers();

        // roughly estimate how many bytes will it take to encode the response
        let cap = 48 + servers.len() * 256;

        let buf = data::encode_message_heap!(self, cap, msg => {
            let changed = msg.init_servers_changed();
            let mut srvs = changed.init_servers(servers.len() as u32);

            for (i, srv) in servers.iter().enumerate() {
                let server = srvs.reborrow().get(i as u32);
                self.encode_game_server(&srv.data, server);
            }
        })
        .map(Arc::new);

        match buf {
            Ok(buf) => {
                let targets: Vec<_> =
                    self.all_clients.iter().filter_map(|x| x.value().upgrade()).collect();

                info!("Notifying {} clients about server change!", targets.len());

                for target in targets {
                    target.send_data_bufkind(BufferKind::Reference(Arc::clone(&buf)));
                }
            }

            Err(err) => {
                error!("Failed to send ServersChangedMessage, encoding failed: {err}");
            }
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

    pub fn client_count(&self) -> usize {
        self.all_clients.len()
    }

    pub fn find_client(&self, account_id: i32) -> Option<ClientStateHandle> {
        self.all_clients.get(&account_id).and_then(|x| x.upgrade())
    }

    /// TODO: this function is not fast
    pub fn find_client_by_name(&self, username: &str) -> Option<ClientStateHandle> {
        self.all_clients
            .iter()
            .filter_map(|r| match r.value().upgrade() {
                Some(c) if c.username().eq_ignore_ascii_case(username) => Some(c),
                Some(_) => None,
                None => None,
            })
            .next()
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

    fn send_muted(
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

    fn send_warn(&self, client: &ClientStateHandle, message: impl AsRef<str>) -> HandlerResult<()> {
        let buf = data::encode_message_heap!(self, 48 + message.as_ref().len(), msg => {
            let mut warn = msg.reborrow().init_warn();
            warn.set_message(message.as_ref());
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    fn encode_ext_user_data(
        &self,
        role: &ComputedRole,
        token: &str,
        mut builder: data::extended_user_data::Builder<'_>,
    ) {
        let users = self.module::<UsersModule>();
        let can_name_rooms = role.can_name_rooms || !users.disallow_room_names;

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

    #[cfg(feature = "word-filter")]
    async fn is_disallowed(&self, string: &str) -> bool {
        use crate::word_filter::WordFilterModule;

        let module = self.opt_module::<WordFilterModule>();
        if let Some(module) = module {
            !module.is_allowed(string).await
        } else {
            false
        }
    }

    #[cfg(not(feature = "word-filter"))]
    async fn is_disallowed(&self, _string: &str) -> bool {
        false
    }
}

struct LoginData<'a> {
    kind: LoginKind<'a>,
    icons: PlayerIconData,
    uident: Option<&'a [u8]>,
    settings: UserSettings,
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

    Ok(LoginData { kind, icons, uident, settings })
}

fn format_systime(s: SystemTime) -> String {
    time_format::strftime_utc(
        "%Y-%m-%dT%H.%M.%S",
        time_format::from_system_time(s).unwrap_or_default(),
    )
    .unwrap_or_else(|_| "unknown".to_string())
}

fn format_dur(d: Duration) -> String {
    format!("{:.3}s", d.as_secs_f64())
}

async fn dump_connection_data(conn: &FinishedConnection, dir: &Path) -> std::io::Result<()> {
    use tokio::{fs, io::AsyncWriteExt};

    fs::create_dir_all(dir).await?;
    let mut info_file = fs::File::create(dir.join("info.txt")).await?;

    let up_p = conn.packets.iter().filter(|x| x.up).count();
    let down_p = conn.packets.iter().filter(|x| !x.up).count();

    info_file.write_all(format!(
        "Connection ID: {}\nAddress: {}\nConnected at: {} (UTC)\nLasted: {:?}\nPackets transferred: {} ({} up, {} down)\n",
        conn.id,
        conn.address,
        format_systime(conn.creation),
        conn.whole_time,
        up_p + down_p,
        up_p,
        down_p,
    ).as_bytes()).await?;

    // Dump all packets as separate files

    for (i, pkt) in conn.packets.iter().enumerate() {
        // format example:
        // pkt-0-0.001s-up.bin
        // pkt-1-0.002s-down.bin
        // this way index is prioritized (e.g. in sorting) but timestamp is also known
        let filename = format!(
            "pkt-{}-{}-{}",
            i,
            format_dur(pkt.timestamp),
            if pkt.up { "up" } else { "down" }
        );

        fs::File::create(dir.join(filename)).await?.write_all(&pkt.data).await?;
    }

    Ok(())
}
