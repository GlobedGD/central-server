use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use arc_swap::ArcSwap;
use qunet::{
    message::channel,
    server::{ServerHandle, WeakServerHandle, client::ClientState},
};
use rustc_hash::FxHashMap;
use server_shared::{data::GameServerData, encoding::EncodeMessageError};
use thiserror::Error;

use super::data;
use crate::core::game_server::GameServerHandler;

#[derive(Clone)]
pub struct StoredGameServer {
    qclient: Arc<ClientState<GameServerHandler>>,
    pub data: GameServerData,
}

#[derive(Default)]
pub struct GameServerManager {
    servers: ArcSwap<Vec<StoredGameServer>>,
    create_reqs: parking_lot::Mutex<FxHashMap<u32, RoomCreateRequest>>,
    server_handle: OnceLock<WeakServerHandle<GameServerHandler>>,
}

#[derive(Error, Debug)]
pub enum GameServerError {
    #[error("Server not found")]
    ServerNotFound,
    #[error("Failed to encode message: {0}")]
    EncodeError(#[from] EncodeMessageError),
    #[error("Internal failure talking to the game server")]
    InternalFailure,
    #[error("Timed out waiting for the game server to respond")]
    Timeout,
}

struct RoomCreateRequest {
    tx: channel::Sender<()>,
}

impl GameServerManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Note: this function is unrelated to game servers, it sets the handle to the server of game servers
    pub fn set_server(&self, server: WeakServerHandle<GameServerHandler>) {
        if self.server_handle.set(server).is_err() {
            panic!("game server server handle already set");
        }
    }

    fn server(&self) -> ServerHandle<GameServerHandler> {
        self.server_handle
            .get()
            .expect("Server not initialized yet")
            .upgrade()
            .expect("Server has shut down")
    }

    pub fn add_server(
        &self,
        server: Arc<ClientState<GameServerHandler>>,
        mut data: GameServerData,
    ) {
        self.servers.rcu(|servers| {
            let mut servers = (**servers).clone();

            // find the next available ID
            data.id = 0;
            while servers.iter().any(|s| s.data.id == data.id) {
                data.id = data
                    .id
                    .checked_add(1)
                    .expect("More than 255 servers connected, this is unsupported!");
            }

            servers.push(StoredGameServer {
                qclient: server.clone(),
                data: data.clone(),
            });
            servers
        });
    }

    pub fn remove_server(
        &self,
        server: &ClientState<GameServerHandler>,
    ) -> Option<StoredGameServer> {
        let mut ret = None;

        self.servers.rcu(|servers| {
            let mut servers = (**servers).clone();

            ret = servers
                .iter()
                .position(|s| s.qclient.connection_id == server.connection_id)
                .map(|pos| servers.remove(pos));

            servers
        });

        ret
    }

    pub fn servers(&self) -> Arc<Vec<StoredGameServer>> {
        self.servers.load_full()
    }

    pub fn has_server(&self, id: u8) -> bool {
        self.servers.load().iter().any(|s| s.data.id == id)
    }

    pub async fn notify_room_created(
        &self,
        server_id: u8,
        room_id: u32,
        passcode: u32,
    ) -> Result<(), GameServerError> {
        let servers = self.servers.load();
        let server = servers
            .iter()
            .find(|s| s.data.id == server_id)
            .ok_or(GameServerError::ServerNotFound)?;

        let buf = data::encode_message_unsafe!(self, 32, msg => {
            let mut room_created = msg.init_notify_room_created();
            room_created.set_room_id(room_id);
            room_created.set_passcode(passcode);
        })?;

        server.qclient.send_data_bufkind(buf);

        // wait up to 5 seconds for a response from the game server

        let (tx, rx) = channel::new_channel(1);
        self.create_reqs.lock().insert(room_id, RoomCreateRequest { tx });

        let res = match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(())) => Ok(()),
            Ok(None) => Err(GameServerError::InternalFailure),
            Err(_) => Err(GameServerError::Timeout),
        };

        // make sure to remove the request from the map, because on failures it does not get removed
        self.create_reqs.lock().remove(&room_id);

        res
    }

    pub async fn ack_room_created(&self, room_id: u32) {
        if let Some(req) = self.create_reqs.lock().remove(&room_id) {
            req.tx.send(());
        }
    }
}
