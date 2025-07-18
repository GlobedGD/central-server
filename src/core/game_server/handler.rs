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
use crate::core::{data::heapless_str_from_reader, handler::ConnectionHandler};

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

        // successful login! tell the main server to add this game server
        if let Err(e) =
            self.main_server().handler().handle_game_server_connect(client.clone(), data).await
        {
            warn!("[{}] failed to handle game server connect: {e}", client.address);
            return self.send_login_failed(client, &format!("internal error: {e}")).await;
        }

        let buf = data::encode_message!(self, 128, msg => {
            msg.reborrow().init_login_ok();
        })
        .expect("failed to encode login success message");

        client.send_data_bufkind(buf);

        info!("[{}] New game server connected!", client.address);
        client.set_authorized(true);

        Ok(())
    }

    /// Invoked when an authorized client disconnects.
    async fn handle_logout(&self, client: &ClientStateHandle) {
        debug_assert!(client.authorized());

        warn!("[{}] Game server disconnected", client.address);

        self.main_server().handler().handle_game_server_disconnect(client.clone()).await;
    }
}

impl AppHandler for GameServerHandler {
    type ClientData = GameServerClientData;

    async fn on_launch(&self, server: QunetServerHandle<Self>) -> AppResult<()> {
        let _ = self.server.set(server.make_weak());

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
        let result = data::decode_message_match!(self, data,{
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
