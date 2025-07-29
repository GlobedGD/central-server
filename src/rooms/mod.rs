use std::sync::Arc;

use crate::core::{data, handler::ClientStateHandle, module::ServerModule};

mod manager;
mod session_id;
mod settings;
pub use manager::{ClientRoomHandle, Room, RoomCreationError, RoomManager};
use serde::{Deserialize, Serialize};
pub use session_id::SessionId;
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
    ) -> Result<Arc<Room>, RoomCreationError> {
        debug_assert!(client.authorized());

        let room = self.create_room(name, passcode, client.account_id(), settings)?;
        self.force_join_room(client, room.clone()).await;
        Ok(room)
    }

    pub async fn join_room_by_id(
        &self,
        client: &ClientStateHandle,
        room_id: u32,
        passcode: u32,
    ) -> Result<Arc<Room>, data::RoomJoinFailedReason> {
        let room = if room_id == 0 {
            let room = self.global_room();
            self.force_join_room(client, room.clone()).await;
            room
        } else {
            let room = self.get_room(room_id).ok_or(data::RoomJoinFailedReason::NotFound)?;
            self.join_room(client, room.clone(), passcode).await?;
            room
        };

        Ok(room)
    }

    /// clears the client's current room and sets it to the given room,
    /// verifying if the passcode is correct and if the room is not full
    pub async fn join_room(
        &self,
        client: &ClientStateHandle,
        room: Arc<Room>,
        passcode: u32,
    ) -> Result<(), data::RoomJoinFailedReason> {
        debug_assert!(client.authorized());

        if room.has_player(client) {
            return Ok(());
        }

        let handle = room.add_player(client.clone(), passcode).await?;
        self.clear_client_room(client).await;
        client.set_room(handle);

        Ok(())
    }

    /// clears the client's current room and sets it to the given room,
    /// does not validate if the room is full or if the passcode is invalid unlike `join_room`
    pub async fn force_join_room(&self, client: &ClientStateHandle, room: Arc<Room>) {
        debug_assert!(client.authorized());

        self.clear_client_room(client).await;
        self.set_client_room(client, room).await;
    }

    /// clears the client's room, does nothing if room is None
    async fn clear_client_room(&self, client: &ClientStateHandle) {
        debug_assert!(client.authorized());

        if let Some(room) = client.clear_room().await {
            // if the room has no more players, remove it
            if room.player_count() == 0 {
                self.manager.remove_room(room.id);
            }
        }
    }

    /// sets the client's room, does not handle leaving the previous room
    async fn set_client_room(&self, client: &ClientStateHandle, room: Arc<Room>) {
        debug_assert!(client.authorized());

        let handle = room.force_add_player(client.clone()).await;
        client.set_room(handle);
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    _unused: bool,
}

impl ServerModule for RoomModule {
    type Config = Config;

    fn new(_config: &Self::Config) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self { manager: RoomManager::new() })
    }

    fn id() -> &'static str {
        "rooms"
    }

    fn name() -> &'static str {
        "Rooms"
    }
}
