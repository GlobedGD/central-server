use std::{
    net::SocketAddr,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use qunet::{
    message::MsgData,
    server::{
        Server as QunetServer, ServerHandle as QunetServerHandle, WeakServerHandle,
        app_handler::{AppHandler, AppResult},
        client::ClientState,
    },
};
use server_shared::{data::GameServerData, encoding::EncodeMessageError};
use thiserror::Error;
use tracing::{debug, error, info, warn};

use super::data;
use crate::{
    auth::AuthModule,
    core::{data::heapless_str_from_reader, handler::ConnectionHandler},
    users::UsersModule,
};

pub struct GameServerHandler {
    password: String,
    server: OnceLock<WeakServerHandle<Self>>,
    main_server: WeakServerHandle<ConnectionHandler>,
}

pub type ClientStateHandle = Arc<ClientState<GameServerHandler>>;

#[derive(Debug, Error)]
enum HandlerError {
    #[error("failed to encode message: {0}")]
    Encoder(#[from] EncodeMessageError),
    #[error("unauthorized client")]
    Unauthorized,
}

type HandlerResult<T> = Result<T, HandlerError>;

pub struct GameServerClientData {
    authorized: AtomicBool,
}

impl GameServerClientData {
    pub fn new() -> Self {
        Self {
            authorized: AtomicBool::new(false),
        }
    }

    pub fn authorized(&self) -> bool {
        self.authorized.load(Ordering::Relaxed)
    }

    pub fn set_authorized(&self, value: bool) {
        self.authorized.store(value, Ordering::Relaxed);
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
        let buf = data::encode_message!(self, 512, msg => {
            let mut login_failed = msg.reborrow().init_login_failed();
            login_failed.set_reason(reason);
        })?;

        client.send_data_bufkind(buf);

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

        let buf = data::encode_message!(self, 512, msg => {
            let mut login_ok = msg.reborrow().init_login_ok();

            login_ok.set_token_key(secret_key);
            login_ok.set_script_key(script_key);
            login_ok.set_token_expiry(token_expiry);
            let mut roles_ser = login_ok.init_roles(roles.len() as u32);

            for (i, role) in roles.iter().enumerate() {
                assert!(i < 256, "too many roles, must be below 256");

                let mut role_ser = roles_ser.reborrow().get(i as u32);
                role_ser.set_id(i as u8);
                role_ser.set_string_id(&role.id);
            }
        })
        .expect("failed to encode login success message");

        client.send_data_bufkind(buf);

        client.set_authorized(true);

        Ok(())
    }

    /// Invoked when an authorized client disconnects.
    async fn handle_logout(&self, client: &ClientStateHandle) {
        debug_assert!(client.authorized());

        warn!("[{}] Game server disconnected", client.address);

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
}

impl AppHandler for GameServerHandler {
    type ClientData = GameServerClientData;

    async fn on_launch(&self, server: QunetServerHandle<Self>) -> AppResult<()> {
        let _ = self.server.set(server.make_weak());
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
        let result = data::decode_message_match!(self, data, _unpacked_data, {
            LoginSrv(message) => {
                let password = message.get_password()?.to_str()?;
                let data = message.get_data()?;

                let data = GameServerData {
                    id: 0,
                    address: heapless_str_from_reader(data.get_address()?)?,
                    name: heapless_str_from_reader(data.get_name()?)?,
                    string_id: heapless_str_from_reader(data.get_string_id()?)?,
                    region: heapless_str_from_reader(data.get_region()?)?,
                };

                self.handle_login(client, password, data).await
            },

            RoomCreatedAck(message) => {
                let room_id = message.get_room_id();

                self.handle_room_created_ack(client, room_id).await
            }
        });

        match result {
            Ok(Ok(_)) => {}

            Ok(Err(e)) => {
                error!("[{}] failed to handle game server message: {e}", client.address);
            }

            Err(e) => {
                error!("[{}] failed to decode game server message: {e}", client.address);
            }
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
