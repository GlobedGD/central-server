use std::{
    net::SocketAddr,
    path::Path,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use dashmap::DashMap;
use parking_lot::Mutex;
use serde::Serialize;
use server_shared::{
    SessionId,
    events::EventStringCache,
    qunet::{
        message::{BufferKind, MsgData},
        server::{
            Server as QunetServer, ServerHandle as QunetServerHandle, WeakServerHandle,
            app_handler::{AppHandler, AppResult},
            stat_tracker::{FinishedConnection, OverallStats},
        },
    },
};
use server_shared::{TypeMap, data::GameServerData};

use crate::{
    auth::{ArgonConnectionState, AuthModule},
    core::{
        client_data::ClientData,
        config::Config,
        data::{self},
        event_worker::EventWorker,
        game_server::{GameServerHandler, GameServerManager, StoredGameServer},
        handler::client_store::{ClientStore, normalize_username},
        module::{ConfigurableModule, ServerModule},
    },
    rooms::RoomModule,
    users::UsersModule,
};

mod admin;
mod client_store;
#[cfg(feature = "featured-levels")]
mod featured;
mod login;
mod message_handling;
mod misc;
mod rooms;
mod session;
mod util;
pub use message_handling::LoginData;
use util::*;
pub use util::{ClientState, ClientStateHandle, WeakClientStateHandle};

struct LevelEntry {
    player_count: u32,
    is_hidden: bool,
}

type ModuleReloadFn = Box<dyn Fn(&QunetServerHandle<ConnectionHandler>, &Config) + Send + Sync>;

pub struct ConnectionHandler {
    modules: TypeMap,
    module_list: Mutex<Vec<Arc<dyn ServerModule>>>,
    module_reload_fns: Mutex<Vec<ModuleReloadFn>>,

    // we use a weak handle here to avoid ref cycles, which will make it impossible to drop the server
    server: OnceLock<WeakServerHandle<Self>>,
    game_server_manager: GameServerManager,
    http_client: reqwest::Client,
    config: Config,
    launched_at: Instant,

    clients: ClientStore,
    all_levels: DashMap<u64, LevelEntry>,
    refuse_connections: AtomicBool,

    event_string_cache: EventStringCache,
    event_worker: EventWorker,
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
            let h = server.handler();

            server.print_server_status();
            info!(" - Authorized clients: {}", h.clients.count());
            info!(
                " - Active game sessions: {} (total players: {})",
                h.all_levels.len(),
                h.all_levels.iter().map(|mref| mref.value().player_count).sum::<u32>()
            );

            let rooms = h.module::<RoomModule>();
            info!(" - Room count: {}", rooms.get_room_count());

            // vacuum invalid clients
            let removed_clients = h.clients.vacuum();
            if removed_clients > 0 {
                warn!(
                    "Removed {} invalid clients from client store, this is likely a bug!",
                    removed_clients
                );
            }
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

        self.event_worker.set_server(server);

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
            self.clients.remove_if_same(account_id, client);

            let _ = self.handle_leave_session(client).await;
        }
    }

    async fn post_shutdown(&self, _server: &QunetServer<Self>) -> AppResult<()> {
        // by this point all connections have been dropped, we should clean up any resources
        info!("Cleaning up resources");

        let rooms = self.module::<RoomModule>();
        rooms.cleanup_everything();

        self.event_worker.abort();

        info!("Post-shutdown cleanup complete");

        Ok(())
    }
    async fn on_client_data(
        &self,
        server: &QunetServer<Self>,
        client: &ClientStateHandle,
        data: MsgData<'_>,
    ) {
        self.handle_client_data(server, client, data).await;
    }

    async fn on_sigusr1(&self, _server: &QunetServer<Self>) {
        self.dump_all_connections().await;
    }

    async fn on_sigusr2(&self, _server: &QunetServer<Self>) {
        self.reload_config().await;
    }
}

impl ConnectionHandler {
    pub fn new(config: Config) -> Self {
        static APP_USER_AGENT: &str =
            concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

        Self {
            modules: TypeMap::new(),
            module_list: Mutex::new(Vec::new()),
            module_reload_fns: Mutex::new(Vec::new()),
            server: OnceLock::new(),
            game_server_manager: GameServerManager::new(),
            config,
            http_client: reqwest::Client::builder()
                .user_agent(APP_USER_AGENT)
                .connect_timeout(Duration::from_secs(5))
                .build()
                .expect("failed to create an http client"),
            launched_at: Instant::now(),
            clients: ClientStore::new(),
            all_levels: DashMap::new(),
            refuse_connections: AtomicBool::new(false),

            event_string_cache: EventStringCache::new(),
            event_worker: EventWorker::new(),
        }
    }

    pub fn insert_module<T: ServerModule + ConfigurableModule + Sized>(&self, module: T) {
        self.modules.insert(module);
        let module = self.opt_module_owned::<T>().unwrap();
        let weak = Arc::downgrade(&module);

        self.module_list.lock().push(module);

        self.module_reload_fns.lock().push(Box::new(move |server, config| {
            let Some(module) = weak.upgrade() else {
                return;
            };

            match config.reload_module::<T>() {
                Ok(cfg) => module.reload(server, cfg),

                Err(e) => {
                    error!("Failed to reload config for module {}: {}", T::id(), e);
                }
            }
        }));
    }

    /// Get a module by type. Panics if the module is not found.
    pub fn module<T: ServerModule>(&self) -> &T {
        self.opt_module().expect("non-existent module getter called")
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

    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
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

    pub async fn reload_config(&self) {
        if let Err(e) = self.config().reload_core() {
            error!("Failed to reload core config: {e}");
        }

        for func in self.module_reload_fns.lock().iter() {
            func(&self.server(), self.config());
        }

        if let Err(e) = self.game_server_manager.notify_reload_config().await {
            error!("Failed to notify game servers about config reload: {e}");
        }

        info!("Reloaded server configuration!");
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

    pub fn increment_level_players(&self, session: impl Into<SessionId>, is_hidden: bool) {
        let mut ent = self
            .all_levels
            .entry(session.into().as_u64())
            .or_insert(LevelEntry { player_count: 0, is_hidden });

        ent.player_count += 1;
    }

    pub fn decrement_level_players(&self, session: impl Into<SessionId>) {
        let session = session.into().as_u64();
        debug_assert!(self.all_levels.contains_key(&session));

        self.all_levels.remove_if_mut(&session, |_, entry| {
            entry.player_count -= 1;
            entry.player_count == 0
        });
    }

    pub fn override_level_hidden(&self, session: u64, hidden: bool) -> bool {
        if let Some(mut ent) = self.all_levels.get_mut(&session) {
            ent.is_hidden = hidden;
            true
        } else {
            false
        }
    }

    pub fn http_client(&self) -> reqwest::Client {
        self.http_client.clone()
    }

    pub fn get_server_health(&self) -> ServerHealth {
        let auth = self.module::<AuthModule>();

        ServerHealth {
            uptime: self.launched_at.elapsed().as_secs_f64(),
            argon_state: match auth.argon_state() {
                ArgonConnectionState::Disabled => "disabled",
                ArgonConnectionState::Connected => "up",
                ArgonConnectionState::Disconnected => "down",
            },
            argon_connected_for: auth.argon_connected_for().map(|x| x.as_secs_f64()),
            clients: self.server().client_count(),
            rooms: self.module::<RoomModule>().get_room_count(),
            levels: self.level_count(),

            game_servers: self
                .game_server_manager
                .servers()
                .iter()
                .map(|s| GameServerHealth {
                    id: s.data.string_id.as_str().to_owned(),
                    uptime: s.uptime().as_secs_f64(),
                    load: s.status_data().server_load,
                })
                .collect(),
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
        let Some(srv) = self.game_server_manager.remove_server(&client) else {
            error!(
                "[{} @ {}] unknown game server disconnected!",
                client.connection_id, client.address
            );
            return;
        };

        warn!(
            "[{}] Game server '{}' disconnected, was connected for {:?}",
            client.address,
            srv.data.string_id,
            srv.uptime()
        );

        // close all rooms that are hosted on this server
        let module = self.module::<RoomModule>();
        module.close_all_rooms_on_server(srv.data.id, &self.game_server_manager).await;

        // notify clients about the disconnect
        self.notify_servers_changed().await;

        // log on discord
        #[cfg(feature = "discord")]
        {
            use crate::discord::{DiscordMessage, DiscordModule};

            if let Some(discord) = self.opt_module::<DiscordModule>() {
                discord.send_server_alert(DiscordMessage::new().content(format!(
                    "⚠️ Game server '{}' disconnected, was connected for {:?}",
                    srv.data.string_id,
                    srv.uptime()
                )));
            }
        }
    }

    pub async fn notify_servers_changed(&self) {
        let servers = self.game_server_manager.servers();

        let buf = data::encode_message_dyn!(self, msg => {
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
                let targets = self.clients.collect_all_authorized();

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

    pub fn set_refuse_connections(&self, refuse: bool) {
        self.refuse_connections.store(refuse, Ordering::Relaxed);
    }

    pub fn get_all_clients(&self) -> Vec<ClientStateHandle> {
        self.clients.collect_all()
    }

    pub fn get_n_clients_matching(&self, query: &str, limit: usize) -> Vec<ClientStateHandle> {
        let query = normalize_username(query);
        self.clients.collect_name_pred(|name| name.contains(query.as_str()), limit)
    }

    pub fn get_all_authorized_clients(&self) -> Vec<ClientStateHandle> {
        self.clients.collect_all_authorized()
    }

    pub async fn notify_user_linked(&self, handle: &ClientStateHandle) {
        handle.set_discord_linked(true);

        // if the user is in a session, notify the appropriate game server
        let session = handle.session_id();
        if !session.is_zero() {
            let users = self.module::<UsersModule>();
            let data = users.gather_user_data(handle);

            let _ = self.game_server_manager.notify_user_data(session.server_id(), data).await;
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

    pub fn client_count(&self) -> usize {
        self.clients.count()
    }

    pub fn find_client(&self, account_id: i32) -> Option<ClientStateHandle> {
        self.clients.find(account_id)
    }

    pub fn find_client_by_name(&self, username: &str) -> Option<ClientStateHandle> {
        self.clients.find_by_name(username)
    }

    pub fn find_client_by_id_or_name(&self, query: &str) -> Option<ClientStateHandle> {
        if let Ok(account_id) = query.parse::<i32>() {
            self.find_client(account_id).or_else(|| self.find_client_by_name(query))
        } else {
            self.find_client_by_name(query)
        }
    }

    #[cfg(feature = "word-filter")]
    async fn has_bad_word(&self, string: &str) -> Option<String> {
        use crate::word_filter::WordFilterModule;

        let module = self.opt_module::<WordFilterModule>();
        if let Some(module) = module {
            module.has_bad_word(string).await
        } else {
            None
        }
    }

    #[cfg(not(feature = "word-filter"))]
    async fn has_bad_word(&self, _string: &str) -> Option<String> {
        None
    }
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

#[derive(Serialize)]
pub struct ServerHealth {
    /// Uptime in seconds
    pub uptime: f64,
    /// Argon state: disabled / down / up
    pub argon_state: &'static str,
    /// How long we've been connected to the Argon server for in seconds, or null
    pub argon_connected_for: Option<f64>,
    /// How many clients in total are connected to the server
    pub clients: usize,
    /// How many rooms are currently active
    pub rooms: usize,
    /// How many levels are currently active across all rooms
    pub levels: usize,

    /// Statuses of connected game servers
    pub game_servers: Vec<GameServerHealth>,
}

#[derive(Serialize)]
pub struct GameServerHealth {
    pub id: String,
    /// Uptime in seconds, how long the game server has been connected to the central server
    pub uptime: f64,
    /// Server load, typically from 0 to 1 but can exceed 100%
    pub load: f32,
}
