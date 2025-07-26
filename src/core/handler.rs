use std::{
    borrow::Cow,
    net::SocketAddr,
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
use tracing::{debug, error, info, warn};

use crate::{
    auth::{AuthModule, AuthVerdict, ClientAccountData, LoginKind},
    core::{
        client_data::ClientData,
        config::Config,
        data::{self, EncodeMessageError, decode_message_match},
        game_server::{GameServerHandler, GameServerManager},
        module::ServerModule,
    },
    rooms::{Room, RoomModule, RoomSettings, SessionId},
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

        if account_id != 0 {
            self.all_clients.remove(&account_id);
            let _ = self.handle_leave_session(client).await;
        }
    }

    async fn post_shutdown(&self, _server: &QunetServer<Self>) -> AppResult<()> {
        // by this point all connections have been dropped, we should clean up any resources
        info!("Cleaning up resources");
        let rooms = self.module::<RoomModule>();
        rooms.cleanup_everything();

        Ok(())
    }

    async fn on_client_data(
        &self,
        _server: &QunetServer<Self>,
        client: &ClientStateHandle,
        data: MsgData<'_>,
    ) {
        info!("Received {} bytes from client {}", data.len(), client.address);

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

                self.handle_login_attempt(client, LoginKind::Plain(ClientAccountData {
                    account_id, user_id, username
                }), icons).await
            },

            UpdateOwnData(message) => {
                if message.has_icons() {
                    let icons = PlayerIconData::from_reader(message.get_icons()?)?;
                    client.set_icons(icons);
                }

                if message.has_friend_list() {
                    let mut fl = FxHashSet::default();

                    let friend_list = message.get_friend_list()?;
                    for friend in friend_list.iter().take(500) { // limit to 500 friends to prevent evil stuff
                        fl.insert(friend);
                    }

                    client.set_friends(fl);
                }

                Ok(())
            },

            RequestPlayerCounts(message) => {
                let levels = message.get_levels()?;
                let mut out_levels = heapless::Vec::<u64, 128>::new();

                for level in levels.iter().take(out_levels.capacity()) {
                    let _ = out_levels.push(level);
                }

                unpacked_data.reset(); // free up memory

                self.handle_request_player_counts(client, &out_levels).await
            },

            CreateRoom(message) => {
                let name = message.get_name()?.to_str()?;
                let settings = RoomSettings::from_reader(message.get_settings()?)?;

                self.handle_create_room(client, name, settings).await
            },

            JoinRoom(message) => {
                let id = message.get_room_id();
                unpacked_data.reset(); // free up memory

                self.handle_join_room(client, id).await
            },

            LeaveRoom(_message) => {
                unpacked_data.reset(); // free up memory

                self.handle_leave_room(client).await
            },

            CheckRoomState(_message) => {
                unpacked_data.reset(); // free up memory

                self.handle_check_room_state(client).await
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
            // TODO: reset active session of clients that were connected to this server
        } else {
            error!(
                "[{} @ {}] unknown game server disconnected!",
                client.connection_id, client.address
            );
        }
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
                self.on_login_success(client, data).await?;
                client.set_icons(icons);
            }

            AuthVerdict::Failed(reason) => {
                self.on_login_failed(client, reason).await?;
            }

            AuthVerdict::LoginRequired => {
                let buf = data::encode_message!(self, 128, msg => {
                    let mut login_req = msg.reborrow().init_login_required();
                    login_req.set_argon_url(auth.argon_url().unwrap());
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
    ) -> HandlerResult<()> {
        // refresh the user's user token (or generate a new one)
        let auth = self.module::<AuthModule>();
        let rooms = self.module::<RoomModule>();

        info!("[{}] {} ({}) logged in", client.address, data.username, data.account_id);

        let token = auth.generate_user_token(data.account_id, data.user_id, data.username.clone());

        if let Some(old_client) = self.all_clients.insert(data.account_id, Arc::downgrade(client)) {
            // there already was a client with this account ID, disconnect them
            if let Some(old_client) = old_client.upgrade() {
                old_client.disconnect(Cow::Borrowed("Duplicate login detected, the same account logged in from a different location"));
            }
        }

        client.set_account_data(data);

        // put the user in the global room
        rooms.join_room(client, rooms.global_room());

        // send login success message with all servers
        let servers = self.game_server_manager.servers();

        let buf = data::encode_message!(self, 1024, msg => {
            let mut login_ok = msg.reborrow().init_login_ok();
            login_ok.set_new_token(&token);

            let mut srvs = login_ok.reborrow().init_servers(servers.len() as u32);

            for (i, srv) in servers.iter().enumerate() {
                let server = srvs.reborrow().get(i as u32);
                self.encode_game_server(&srv.data, server);
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    #[inline]
    async fn on_login_failed(
        &self,
        client: &ClientState<Self>,
        reason: data::LoginFailedReason,
    ) -> HandlerResult<()> {
        let buf = data::encode_message!(self, 128, msg => {
            let mut login_failed = msg.reborrow().init_login_failed();
            login_failed.set_reason(reason);
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }

    async fn handle_request_player_counts(
        &self,
        client: &ClientStateHandle,
        sessions: &[u64],
    ) -> HandlerResult<()> {
        let mut out_vals = heapless::Vec::<(u64, u16), 128>::new();
        debug_assert!(sessions.len() <= out_vals.capacity());

        for &sess in sessions {
            if let Some(count) = self.player_counts.get(&sess) {
                let _ = out_vals.push((sess, *count as u16));
                // TODO: maybe do a zero optimization?
            }
        }

        // TODO: benchmark size properly
        let cap = 32 + out_vals.len() * 12;

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
        settings: RoomSettings,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let rooms = self.module::<RoomModule>();

        match rooms.create_room_and_join(name, settings, client) {
            Ok(new_room) => {
                self.send_room_data(client, &new_room).await?;
            }

            // TODO: send error to the user
            Err(e) => warn!("failed to create room: {e}"),
        }

        Ok(())
    }

    async fn handle_join_room(&self, client: &ClientStateHandle, id: u32) -> HandlerResult<()> {
        must_auth(client)?;

        let rooms = self.module::<RoomModule>();
        let new_room = rooms.join_room_by_id(client, id);

        self.send_room_data(client, &new_room).await
    }

    async fn handle_leave_room(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        // Leaving a room is the same as joining the global room
        self.handle_join_room(client, 0).await
    }

    async fn send_room_data(&self, client: &ClientStateHandle, room: &Room) -> HandlerResult<()> {
        const BYTES_PER_PLAYER: usize = 64; // TODO

        let players = self.pick_players_to_send(client, room);

        // TODO: that 64 is uncertain
        let cap = 64 + BYTES_PER_PLAYER * players.len();

        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut room_state = msg.reborrow().init_room_state();
            room_state.set_room_id(room.id);
            room_state.set_room_name(&room.name);

            // TODO: like globed, we should prioritize friends, and when the list is greater than the cap, show random players
            let mut players_ser = room_state.init_players(players.len() as u32);

            for (i, player) in players.iter().enumerate() {
                let mut player_ser = players_ser.reborrow().get(i as u32);
                player_ser.set_cube(player.icons().cube);
                player_ser.reborrow().set_session(player.session_id());

                let mut accdata = player_ser.reborrow().init_account_data();
                let account = player.account_data().expect("client must have account data");
                accdata.set_account_id(account.account_id);
                accdata.set_user_id(account.user_id);
                accdata.set_username(&account.username);
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    fn pick_players_to_send(
        &self,
        client: &ClientStateHandle,
        room: &Room,
    ) -> Vec<ClientStateHandle> {
        const PLAYER_CAP: usize = 250;

        let players = room.get_players();

        let mut out = Vec::with_capacity(players.len().min(PLAYER_CAP));

        // always push friends first
        let friend_list = client.friend_list.lock();
        for friend in friend_list.iter() {
            if let Some(friend) = self.all_clients.get(friend).and_then(|x| x.upgrade())
                && let Some(room_id) = friend.get_room_id()
                && room_id == room.id
            {
                out.push(friend);
            }

            if out.len() == PLAYER_CAP {
                break;
            }
        }

        debug_assert!(out.len() <= PLAYER_CAP);

        // put a bunch of dummy values into the vec, as `choose_multiple_fill` requires a mutable slice of initialized Arcs
        out.resize(out.capacity(), client.clone());
        let begin = out.len();
        let written =
            players.iter().map(|x| x.1.clone()).choose_multiple_fill(&mut rng(), &mut out[begin..]);

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
            return self.on_join_failed(client, data::JoinSessionFailedReason::InvalidRoom).await;
        }

        if !self.game_server_manager.has_server(session_id.server_id()) {
            return self.on_join_failed(client, data::JoinSessionFailedReason::InvalidServer).await;
        }

        let prev_id = client.set_session_id(session_id.as_u64());
        self.handle_session_change(client, SessionId::from(prev_id), session_id).await?;

        Ok(())
    }

    async fn on_join_failed(
        &self,
        client: &ClientStateHandle,
        reason: data::JoinSessionFailedReason,
    ) -> HandlerResult<()> {
        let buf = data::encode_message!(self, 128, msg => {
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

    // internal, called when the session ID changes to update player counts in rooms and stuff
    async fn handle_session_change(
        &self,
        _client: &ClientStateHandle,
        prev_session: SessionId,
        new_session: SessionId,
    ) -> HandlerResult<()> {
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

        Ok(())
    }
}
