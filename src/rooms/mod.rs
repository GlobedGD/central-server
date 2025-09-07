use std::sync::Arc;

use crate::core::{
    data,
    game_server::GameServerManager,
    handler::{ClientStateHandle, ConnectionHandler},
    module::{ConfigurableModule, ModuleInitResult, ServerModule},
};

mod manager;
mod settings;
pub use manager::{ClientRoomHandle, Room, RoomCreationError, RoomManager};
use serde::{Deserialize, Serialize};
pub use server_shared::SessionId;
pub use settings::RoomSettings;

pub struct RoomModule {
    manager: RoomManager,
}

impl RoomModule {
    pub fn get_room(&self, id: u32) -> Option<Arc<Room>> {
        self.manager.get(id)
    }

    pub fn get_room_or_global(&self, id: u32) -> Arc<Room> {
        self.manager.get_or_global(id)
    }

    pub fn global_room(&self) -> Arc<Room> {
        self.manager.global()
    }

    pub async fn cleanup_everything(&self) {
        self.manager.clear().await;
    }

    pub fn get_room_count(&self) -> usize {
        self.manager.room_count()
    }

    pub fn create_room(
        &self,
        name: &str,
        passcode: u32,
        owner: i32,
        settings: RoomSettings,
    ) -> Result<Arc<Room>, RoomCreationError> {
        self.manager.create_room(name, passcode, owner, settings)
    }

    pub async fn create_room_and_join(
        &self,
        name: &str,
        passcode: u32,
        settings: RoomSettings,
        client: &ClientStateHandle,
        gsm: &GameServerManager,
    ) -> Result<Arc<Room>, RoomCreationError> {
        debug_assert!(client.authorized());

        let room = self.create_room(name, passcode, client.account_id(), settings)?;
        self.force_join_room(client, gsm, room.clone()).await;
        Ok(room)
    }

    pub async fn join_room_by_id(
        &self,
        client: &ClientStateHandle,
        gsm: &GameServerManager,
        room_id: u32,
        passcode: u32,
    ) -> Result<Arc<Room>, data::RoomJoinFailedReason> {
        let room = if room_id == 0 {
            let room = self.global_room();
            self.force_join_room(client, gsm, room.clone()).await;
            room
        } else {
            let room = self.get_room(room_id).ok_or(data::RoomJoinFailedReason::NotFound)?;
            self.join_room(client, gsm, room.clone(), passcode).await?;
            room
        };

        Ok(room)
    }

    /// clears the client's current room and sets it to the given room,
    /// verifying if the passcode is correct and if the room is not full
    pub async fn join_room(
        &self,
        client: &ClientStateHandle,
        gsm: &GameServerManager,
        room: Arc<Room>,
        passcode: u32,
    ) -> Result<(), data::RoomJoinFailedReason> {
        if room.has_player(client) {
            return Ok(());
        }

        let handle = room.add_player(client.clone(), passcode).await?;
        self.clear_client_room(client, gsm).await; // leave after adding to the new room, since it can fail
        self.set_client_room(client, handle).await;

        Ok(())
    }

    /// clears the client's current room and sets it to the given room,
    /// does not validate if the room is full or if the passcode is invalid unlike `join_room`
    pub async fn force_join_room(
        &self,
        client: &ClientStateHandle,
        gsm: &GameServerManager,
        room: Arc<Room>,
    ) {
        self.clear_client_room(client, gsm).await; // leave before adding to the new room, since it cannot fail
        let handle = room.force_add_player(client.clone()).await;
        self.set_client_room(client, handle).await;
    }

    pub async fn close_room(
        &self,
        id: u32,
        gsm: &GameServerManager,
    ) -> Option<Vec<ClientStateHandle>> {
        let room = self.get_room(id)?;

        let mut out = Vec::new();

        // room is guaranteed not a global room, so sync variant is ok here
        room.with_players_sync(|count, iter| {
            out.reserve_exact(count);

            for (_, player) in iter {
                out.push(player.handle.clone());
            }
        });

        for handle in out.iter() {
            self.force_join_room(handle, gsm, self.global_room()).await;
        }

        Some(out)
    }

    pub fn get_top_rooms(&self, skip: usize, count: usize) -> Vec<Arc<Room>> {
        let sorted = self.manager.lock_sorted();
        sorted.iter().rev().skip(skip).take(count).map(|x| x.1.clone()).collect()
    }

    pub async fn cleanup_player(&self, client: &ClientStateHandle, gsm: &GameServerManager) {
        self.clear_client_room(client, gsm).await;
    }

    /// clears the client's room, does nothing if room is None
    async fn clear_client_room(&self, client: &ClientStateHandle, gsm: &GameServerManager) {
        debug_assert!(client.authorized());

        if let Some(room) = client.clear_room().await {
            // if the room has no more players, remove it
            if !room.is_global() {
                let player_count = room.player_count();

                if player_count == 0 {
                    self.manager.remove_room(room.id);
                    let server_id = room.settings.lock().server_id;
                    let _ = gsm.notify_room_deleted(server_id, room.id).await;
                } else {
                    self.manager.update_room_set(&room);
                }
            }
        }
    }

    /// sets the client's room, does not handle leaving the previous room
    async fn set_client_room(&self, client: &ClientStateHandle, handle: ClientRoomHandle) {
        debug_assert!(client.authorized());

        let room = handle.room.clone();
        client.set_room(handle);

        if !room.is_global() {
            self.manager.update_room_set(&room);
        }
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    _unused: bool,
}

impl ServerModule for RoomModule {
    async fn new(_config: &Config, _handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        Ok(Self { manager: RoomManager::new() })
    }

    fn id() -> &'static str {
        "rooms"
    }

    fn name() -> &'static str {
        "Rooms"
    }
}

impl ConfigurableModule for RoomModule {
    type Config = Config;
}
