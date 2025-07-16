use std::sync::Arc;

use crate::core::{handler::ClientStateHandle, module::ServerModule};

mod manager;
mod session_id;
pub use manager::{ClientRoomHandle, Room, RoomCreationError, RoomManager};
use serde::{Deserialize, Serialize};
pub use session_id::SessionId;

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

    pub fn cleanup_everything(&self) {
        self.manager.clear();
    }

    pub fn create_room(&self, name: &str) -> Result<Arc<Room>, RoomCreationError> {
        self.manager.create_room(name)
    }

    pub fn create_room_and_join(
        &self,
        name: &str,
        client: &ClientStateHandle,
    ) -> Result<Arc<Room>, RoomCreationError> {
        debug_assert!(client.authorized());

        let room = self.create_room(name)?;
        Ok(self.join_room(client, room))
    }

    /// clears the client's current room and sets it to the given room (or global if not found)
    pub fn join_room_by_id(&self, client: &ClientStateHandle, room_id: u32) -> Arc<Room> {
        let room = self.get_room_or_global(room_id);
        self.join_room(client, room)
    }

    /// clears the client's current room and sets it to the given room
    pub fn join_room(&self, client: &ClientStateHandle, room: Arc<Room>) -> Arc<Room> {
        debug_assert!(client.authorized());

        self.clear_client_room(client);
        self.set_client_room(client, room.clone());

        room
    }

    /// clears the client's room, does nothing if room is None
    fn clear_client_room(&self, client: &ClientStateHandle) {
        debug_assert!(client.authorized());

        // clear the room, this returns the room handle which will remove the player from the room when dropped
        client.clear_room();
    }

    /// sets the client's room, does not handle leaving the previous room
    fn set_client_room(&self, client: &ClientStateHandle, room: Arc<Room>) {
        debug_assert!(client.authorized());

        let handle = room.add_player(client.clone());
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
