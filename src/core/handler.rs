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
        client::ClientState,
    },
};
use rand::{rng, seq::IteratorRandom};
use rustc_hash::FxHashSet;
use server_shared::{
    data::{GameServerData, PlayerIconData},
    encoding::heapless_str_from_reader,
};
use state::TypeMap;
use thiserror::Error;
use tracing::{debug, error, info, trace, warn};

use crate::{
    auth::{AuthModule, AuthVerdict, ClientAccountData, LoginKind},
    core::{
        client_data::ClientData,
        config::Config,
        data::{self, EncodeMessageError, decode_message_match},
        game_server::{GameServerHandler, GameServerManager},
        module::ServerModule,
    },
    rooms::{Room, RoomCreationError, RoomModule, RoomSettings, SessionId},
    users::UsersModule,
};

pub struct ConnectionHandler {
    modules: TypeMap![Send + Sync],
    // we use a weak handle here to avoid ref cycles, which will make it impossible to drop the server
    server: OnceLock<WeakServerHandle<Self>>,
    game_server_manager: GameServerManager,
    config: Config,

    all_clients: DashMap<i32, WeakClientStateHandle>,
    player_counts: DashMap<u64, usize>,
}

pub type ClientStateHandle = Arc<ClientState<ConnectionHandler>>;
pub type WeakClientStateHandle = Weak<ClientState<ConnectionHandler>>;

#[derive(Debug, Error)]
pub enum HandlerError {
    #[error("failed to encode message: {0}")]
    Encoder(#[from] EncodeMessageError),
    #[error("cannot handle this message while unauthorized")]
    Unauthorized,
}

type HandlerResult<T> = Result<T, HandlerError>;

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

fn must_auth(client: &ClientState<ConnectionHandler>) -> HandlerResult<()> {
    if client.data().authorized() {
        Ok(())
    } else {
        Err(HandlerError::Unauthorized)
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

    async fn handle_login_attempt(
        &self,
        client: &Arc<ClientState<Self>>,
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

    async fn handle_create_room(
        &self,
        client: &ClientStateHandle,
        name: &str,
        passcode: u32,
        settings: RoomSettings,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        if let Some(p) = client.active_room_ban.lock().as_ref() {
            // user is room banned, don't allow creating rooms
            return self.send_room_banned(client, &p.reason, p.expires_at);
        }

        let rooms = self.module::<RoomModule>();
        let server_id = settings.server_id;

        // check if the requested server is valid
        if !self.game_server_manager.has_server(server_id) {
            return self
                .send_room_create_failed(client, data::RoomCreateFailedReason::InvalidServer);
        }

        let new_room = match rooms
            .create_room_and_join(name, passcode, settings, client, &self.game_server_manager)
            .await
        {
            Ok(new_room) => new_room,

            Err(RoomCreationError::NameTooLong) => {
                return self
                    .send_room_create_failed(client, data::RoomCreateFailedReason::InvalidName);
            }
        };

        // notify the game server about the new room being created and wait for the response
        match self.game_server_manager.notify_room_created(server_id, new_room.id, passcode).await {
            Ok(()) => {
                self.send_room_data(client, &new_room).await?;
            }

            Err(e) => {
                // failed :(
                warn!(
                    "[{}] failed to create room on game server {}: {}",
                    client.address, server_id, e
                );

                // leave back to the global room
                return self.handle_leave_room(client).await;
            }
        }

        Ok(())
    }

    fn send_room_create_failed(
        &self,
        client: &ClientStateHandle,
        reason: data::RoomCreateFailedReason,
    ) -> HandlerResult<()> {
        let buf = data::encode_message!(self, 40, msg => {
            let mut create_failed = msg.reborrow().init_room_create_failed();
            create_failed.set_reason(reason);
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }

    fn send_room_banned(
        &self,
        client: &ClientStateHandle,
        reason: &str,
        expires_at: Option<NonZeroI64>,
    ) -> HandlerResult<()> {
        let buf = data::encode_message!(self, 40, msg => {
            let mut room_banned = msg.reborrow().init_room_banned();
            room_banned.set_reason(reason);
            room_banned.set_expires_at(expires_at.map_or(0, |x| x.get()));
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }

    async fn handle_join_room(
        &self,
        client: &ClientStateHandle,
        id: u32,
        passcode: u32,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let rooms = self.module::<RoomModule>();
        match rooms.join_room_by_id(client, &self.game_server_manager, id, passcode).await {
            Ok(new_room) => self.send_room_data(client, &new_room).await,
            Err(reason) => self.send_room_join_failed(client, reason),
        }
    }

    async fn handle_leave_room(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        // Leaving a room is the same as joining the global room
        self.handle_join_room(client, 0, 0).await
    }

    fn send_room_join_failed(
        &self,
        client: &ClientStateHandle,
        reason: data::RoomJoinFailedReason,
    ) -> HandlerResult<()> {
        let buf = data::encode_message!(self, 40, msg => {
            let mut join_failed = msg.reborrow().init_room_join_failed();
            join_failed.set_reason(reason);
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }

    fn encode_room_player(player: &ClientStateHandle, mut builder: data::room_player::Builder<'_>) {
        builder.set_cube(player.icons().cube);
        builder.reborrow().set_session(player.session_id());

        let mut accdata = builder.reborrow().init_account_data();
        let account = player.account_data().expect("client must have account data");
        accdata.set_account_id(account.account_id);
        accdata.set_user_id(account.user_id);
        accdata.set_username(&account.username);
    }

    async fn send_room_data(&self, client: &ClientStateHandle, room: &Room) -> HandlerResult<()> {
        const BYTES_PER_PLAYER: usize = 64; // TODO (high)

        let players = self.pick_players_to_send(client, room).await;

        // TODO (high): that number is uncertain
        let cap = 128 + BYTES_PER_PLAYER * players.len();

        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut room_state = msg.reborrow().init_room_state();
            room_state.set_room_id(room.id);
            room_state.set_room_owner(room.owner);
            room_state.set_room_name(&room.name);
            room.settings.encode(room_state.reborrow().init_settings());

            let mut players_ser = room_state.init_players(players.len() as u32);

            for (i, player) in players.iter().enumerate() {
                let mut player_ser = players_ser.reborrow().get(i as u32);
                Self::encode_room_player(player, player_ser.reborrow());
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    async fn pick_players_to_send(
        &self,
        client: &ClientStateHandle,
        room: &Room,
    ) -> Vec<ClientStateHandle> {
        const PLAYER_CAP: usize = 100;

        let player_count = if room.is_global() {
            room.player_count().min(PLAYER_CAP)
        } else {
            room.player_count()
        };

        let mut out = Vec::with_capacity(player_count + 2); // +2 to decrease the chance of reallocation

        // always push friends first
        {
            let friend_list = client.friend_list.lock();
            for friend in friend_list.iter() {
                if let Some(friend) = self.all_clients.get(friend).and_then(|x| x.upgrade())
                    && let Some(room_id) = friend.get_room_id()
                    && room_id == room.id
                {
                    out.push(friend);
                }

                if out.len() == player_count {
                    break;
                }
            }
        }

        debug_assert!(out.len() <= player_count);

        let begin = out.len();

        // put a bunch of dummy values into the vec, as `choose_multiple_fill` requires a mutable slice of initialized Arcs
        out.resize(player_count, client.clone());
        let account_id = client.account_id();

        let written = room
            .with_players(|_, players| {
                players
                    .map(|x| x.1.clone())
                    .filter(|x| x.account_id() != account_id)
                    .choose_multiple_fill(&mut rng(), &mut out[begin..])
            })
            .await;

        out.truncate(begin + written);

        out
    }

    async fn handle_join_session(
        &self,
        client: &ClientStateHandle,
        session_id: u64,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let session_id = SessionId::from(session_id);

        // do some validation

        if client.get_room_id().is_none_or(|x| x != session_id.room_id()) {
            return self.on_join_failed(client, data::JoinSessionFailedReason::InvalidRoom);
        }

        if !self.game_server_manager.has_server(session_id.server_id()) {
            return self.on_join_failed(client, data::JoinSessionFailedReason::InvalidServer);
        }

        let prev_id = client.set_session_id(session_id.as_u64());
        self.handle_session_change(client, SessionId::from(prev_id), session_id).await?;

        Ok(())
    }

    fn on_join_failed(
        &self,
        client: &ClientStateHandle,
        reason: data::JoinSessionFailedReason,
    ) -> HandlerResult<()> {
        let buf = data::encode_message!(self, 40, msg => {
            let mut join_failed = msg.reborrow().init_join_failed();
            join_failed.set_reason(reason);
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }

    async fn handle_leave_session(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        let prev_id = client.set_session_id(0);
        self.handle_session_change(client, SessionId::from(prev_id), SessionId(0)).await?;

        Ok(())
    }

    #[allow(clippy::await_holding_lock)]
    async fn handle_check_room_state(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        if let Some(room) = &*client.lock_room() {
            self.send_room_data(client, room).await?;
        }

        Ok(())
    }

    fn handle_request_room_list(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        let rooms = self.module::<RoomModule>();

        // TODO: filtering
        // TODO: pagination

        let sorted = rooms.get_top_rooms(0, 100);
        self.send_room_list(client, &sorted)?;

        Ok(())
    }

    fn send_room_list(&self, client: &ClientStateHandle, rooms: &[Arc<Room>]) -> HandlerResult<()> {
        const BYTES_PER_ROOM: usize = 112; // TODO (high)

        // TODO:
        let cap = 48 + BYTES_PER_ROOM * rooms.len();

        debug!("encoding {} rooms, cap: {}", rooms.len(), cap);

        let buf = data::encode_message_heap!(self, cap, msg => {
            let room_list = msg.reborrow().init_room_list();
            let mut enc_rooms = room_list.init_rooms(rooms.len() as u32);

            for (i, room) in rooms.iter().enumerate() {
                let mut room_ser = enc_rooms.reborrow().get(i as u32);
                room_ser.set_room_id(room.id);
                room_ser.set_room_name(&room.name);
                room_ser.set_player_count(room.player_count() as u32);
                room_ser.set_has_password(room.has_password());
                room.settings.encode(room_ser.reborrow().init_settings());

                let owner = self.all_clients.get(&room.owner).and_then(|x| x.upgrade());
                if let Some(owner) = owner {
                    let mut owner_ser = room_ser.reborrow().init_room_owner();
                    Self::encode_room_player(&owner, owner_ser.reborrow());
                }
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    // internal, called when the session ID changes to update player counts in rooms and stuff
    #[allow(clippy::await_holding_lock)]
    async fn handle_session_change(
        &self,
        client: &ClientStateHandle,
        prev_session: SessionId,
        new_session: SessionId,
    ) -> HandlerResult<()> {
        #[cfg(debug_assertions)]
        trace!(
            "[{}] session change: {} -> {}",
            client.account_id(),
            prev_session.as_u64(),
            new_session.as_u64()
        );

        if !prev_session.is_zero() {
            debug_assert!(self.player_counts.contains_key(&prev_session.as_u64()));

            self.player_counts.remove_if_mut(&prev_session.as_u64(), |_, count| {
                *count -= 1;
                *count == 0
            });
        }

        if !new_session.is_zero() {
            let mut ent = self.player_counts.entry(new_session.as_u64()).or_insert(0);
            *ent += 1;
        }

        // if this is a follower room and the owner changed the level, warp all other players
        let room = client.lock_room(); // this is held across .await but it's fine because it's local to the user

        let do_warp =
            room.as_ref().is_some_and(|x| x.is_follower() && x.owner == client.account_id());

        if do_warp {
            room.as_ref()
                .unwrap()
                .with_players(|_, players| {
                    let buf = data::encode_message!(self, 64, msg => {
                        let mut warp = msg.reborrow().init_warp_player();
                        warp.set_session(new_session.as_u64());
                    })
                    .expect("failed to encode warp message");

                    for (_, p) in players {
                        p.send_data_bufkind(buf.clone_into_small());
                    }
                })
                .await;
        }

        Ok(())
    }
}
