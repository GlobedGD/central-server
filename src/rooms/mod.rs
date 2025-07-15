use std::sync::Arc;

use crate::core::module::ServerModule;

mod manager;
pub use manager::{Room, RoomCreationError, RoomManager};

pub struct RoomModule {
    manager: RoomManager,
}

impl RoomModule {
    pub fn get_room(&self, id: u32) -> Option<Arc<Room>> {
        self.manager.get(id)
    }

    pub fn global_room(&self) -> Arc<Room> {
        self.manager.global()
    }

    pub fn create_room(&self, name: &str) -> Result<Arc<Room>, RoomCreationError> {
        self.manager.create_room(name)
    }
}

impl ServerModule for RoomModule {
    type Config = ();

    fn new(_config: &Self::Config) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self {
            manager: RoomManager::new(),
        })
    }

    fn id() -> &'static str {
        "rooms"
    }

    fn name() -> &'static str {
        "Rooms"
    }
}
