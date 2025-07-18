use std::{marker::PhantomData, sync::Arc};

use arc_swap::ArcSwap;
use qunet::server::client::ClientState;

use crate::core::game_server::GameServerHandler;

#[derive(Clone, Debug)]
pub struct GameServerData {
    pub id: u8,
    pub address: heapless::String<64>,
    pub string_id: heapless::String<32>,
    pub name: heapless::String<64>,
    pub region: heapless::String<32>,
}

#[derive(Clone)]
pub struct StoredGameServer {
    qclient: Arc<ClientState<GameServerHandler>>,
    pub data: GameServerData,
}

#[derive(Default)]
pub struct GameServerManager {
    servers: ArcSwap<Vec<StoredGameServer>>,
}

impl GameServerManager {
    pub fn new() -> Self {
        Self::default()
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

    pub fn remove_server(&self, server: &ClientState<GameServerHandler>) {
        self.servers.rcu(|servers| {
            let mut servers = (**servers).clone();
            servers.retain(|s| s.qclient.connection_id != server.connection_id);
            servers
        });
    }

    pub fn servers(&self) -> Arc<Vec<StoredGameServer>> {
        self.servers.load_full()
    }
}
