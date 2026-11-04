use std::{
    net::SocketAddr,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::anyhow;
use parking_lot::Mutex;
use server_shared::data::{
    GameServerData, SRVC_PROTOCOL_VERSION, SrvEncodeError, SrvMessage, SrvRole,
};
use server_shared::{
    data::{SRVC_MAGIC, SrvStatusData},
    qunet::{
        buffers::ByteReader,
        message::MsgData,
        server::{
            Server as QunetServer, ServerHandle as QunetServerHandle, WeakServerHandle,
            app_handler::{AppHandler, AppResult},
            client::ClientState,
        },
    },
};
use thiserror::Error;
use tracing::{debug, error, info, warn};

use crate::{auth::AuthModule, core::handler::ConnectionHandler, users::UsersModule};

pub struct GameServerHandler {
    password: String,
    server: OnceLock<WeakServerHandle<Self>>,
    main_server: WeakServerHandle<ConnectionHandler>,
}

pub type ClientStateHandle = Arc<ClientState<GameServerHandler>>;

#[derive(Debug, Error)]
enum HandlerError {
    #[error("failed to encode message: {0}")]
    Encoder(#[from] SrvEncodeError),
    #[error("unauthorized client")]
    Unauthorized,
}

type HandlerResult<T> = Result<T, HandlerError>;

pub struct GameServerClientData {
    authorized: AtomicBool,
    srvc_handshaked: AtomicBool,
    status_data: Mutex<SrvStatusData>,
}

impl GameServerClientData {
    pub fn new() -> Self {
        Self {
            authorized: AtomicBool::new(false),
            srvc_handshaked: AtomicBool::new(false),
            status_data: Mutex::new(SrvStatusData::default()),
        }
    }

    pub fn authorized(&self) -> bool {
        self.authorized.load(Ordering::Relaxed)
    }

    pub fn set_authorized(&self, value: bool) {
        self.authorized.store(value, Ordering::Relaxed);
    }

    pub fn srvc_handshaked(&self) -> bool {
        self.srvc_handshaked.load(Ordering::Relaxed)
    }

    pub fn set_srvc_handshaked(&self) {
        self.srvc_handshaked.store(true, Ordering::Relaxed);
    }

    pub fn update_status_data(&self, data: SrvStatusData) {
        *self.status_data.lock() = data;
    }

    pub fn status_data(&self) -> SrvStatusData {
        self.status_data.lock().clone()
    }
}

impl GameServerHandler {
    pub fn new(main_server: WeakServerHandle<ConnectionHandler>, password: String) -> Self {
        Self {
            password,
            server: OnceLock::new(),
            main_server,
        }
    }

    fn server(&self) -> QunetServerHandle<Self> {
        self.server
            .get()
            .expect("server not initialized")
            .upgrade()
            .expect("server already shut down")
    }

    fn main_server(&self) -> QunetServerHandle<ConnectionHandler> {
        self.main_server.upgrade().expect("main server already shut down")
    }

    async fn send_login_failed(
        &self,
        client: &ClientStateHandle,
        reason: &str,
    ) -> HandlerResult<()> {
        let msg = SrvMessage::LoginFailed { reason: reason.to_owned() };
        client.send_data(&msg.encode()?);

        Ok(())
    }

    async fn handle_login(
        &self,
        client: &ClientStateHandle,
        password: &str,
        data: GameServerData,
    ) -> HandlerResult<()> {
        // ignore duplicate login attempts
        if client.authorized() {
            return self.send_login_failed(client, "already logged in").await;
        }

        if !constant_time_eq(password, &self.password) {
            return self.send_login_failed(client, "invalid password").await;
        }

        let server = self.main_server();

        // successful login! tell the main server to add this game server
        info!("[{}] New game server connected! ({})", client.address, data.string_id);
        if let Err(e) = server.handler().handle_game_server_connect(client.clone(), data).await {
            warn!("[{}] failed to handle game server connect: {e}", client.address);
            return self.send_login_failed(client, &format!("internal error: {e}")).await;
        }

        let roles = server.handler().module::<UsersModule>().get_roles();

        let auth_config = server.handler().config().module::<AuthModule>();
        let secret_key = &auth_config.secret_key;
        let token_expiry = auth_config.token_expiry as u64;
        let script_key = &server.handler().config().module::<UsersModule>().script_sign_key;

        let msg = SrvMessage::LoginOk {
            token_key: secret_key.clone(),
            script_key: script_key.clone(),
            token_expiry,
            roles: roles
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    assert!(i < 256, "too many roles, must be below 256");
                    SrvRole::new(i as u8, &r.id, r.can_moderate())
                })
                .collect(),
        };

        client.send_data(&msg.encode()?);

        client.set_authorized(true);

        Ok(())
    }

    /// Invoked when an authorized client disconnects.
    async fn handle_logout(&self, client: &ClientStateHandle) {
        debug_assert!(client.authorized());

        self.main_server().handler().handle_game_server_disconnect(client.clone()).await;
    }

    async fn handle_room_created_ack(
        &self,
        client: &ClientStateHandle,
        room_id: u32,
    ) -> HandlerResult<()> {
        if !client.authorized() {
            return Err(HandlerError::Unauthorized);
        }

        self.main_server().handler().handle_game_server_room_created(room_id).await;

        Ok(())
    }

    async fn handle_server_status(
        &self,
        client: &ClientStateHandle,
        data: SrvStatusData,
    ) -> HandlerResult<()> {
        client.update_status_data(data);
        Ok(())
    }

    fn handle_srvc_handshake(
        &self,
        _client: &ClientStateHandle,
        data: &[u8],
    ) -> anyhow::Result<()> {
        // expect srvc handshake as the first message
        let mut reader = ByteReader::new(data);

        if reader.read_u64()? != SRVC_MAGIC {
            return Err(anyhow!("invalid srvc magic"));
        }

        let version = reader.read_u32()?;
        if version != SRVC_PROTOCOL_VERSION {
            return Err(anyhow!(
                "incompatible server versions, this central server uses protocol {}, while the game server uses {}",
                SRVC_PROTOCOL_VERSION,
                version
            ));
        }

        Ok(())
    }
}

impl AppHandler for GameServerHandler {
    type ClientData = GameServerClientData;

    async fn on_launch(&self, server: QunetServerHandle<Self>) -> AppResult<()> {
        let _ = self.server.set(server.downgrade());
        self.main_server().handler().notify_game_server_handler_started(server).await;

        debug!("Game server handler launched");

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

        info!("[{connection_id} @ {address} ({kind})] Game server connection attempt");

        Ok(GameServerClientData::new())
    }

    async fn on_client_disconnect(&self, _server: &QunetServer<Self>, client: &ClientStateHandle) {
        if client.authorized() {
            self.handle_logout(client).await;
        }
    }

    async fn on_client_data(
        &self,
        _server: &QunetServer<Self>,
        client: &ClientStateHandle,
        data: MsgData<'_>,
    ) {
        if !client.srvc_handshaked() {
            match self.handle_srvc_handshake(client, data.as_bytes()) {
                Ok(()) => {
                    client.set_srvc_handshaked();
                }

                Err(e) => {
                    debug!("[{}] client failed srvc handshake: {e}", client.address);
                    client.disconnect(format!("handshake failed: {e}"));
                }
            }

            return;
        }

        let message = match SrvMessage::decode(&data) {
            Ok(m) => m,
            Err(e) => {
                debug!("[{}] failed to decode game server message: {e}", client.address);
                return;
            }
        };

        let result = match message {
            SrvMessage::Login { password, data } => {
                self.handle_login(client, &password, data).await
            }

            SrvMessage::RoomCreatedAck { room_id } => {
                self.handle_room_created_ack(client, room_id).await
            }

            SrvMessage::Status(data) => self.handle_server_status(client, data).await,

            _ => {
                debug!(
                    "[{}] received unexpected game server message: {:?}",
                    client.address, message
                );
                Ok(())
            }
        };

        if let Err(e) = result {
            error!("[{}] failed to handle game server message: {e}", client.address);
        }
    }
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut result = 0u8;

    for (a_byte, b_byte) in a.bytes().zip(b.bytes()) {
        result |= a_byte ^ b_byte;
    }

    result == 0
}
