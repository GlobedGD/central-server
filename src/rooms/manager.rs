use std::{str::FromStr, sync::Arc};

use dashmap::DashMap;
use thiserror::Error;

pub struct Room {
    pub id: u32,
    pub name: heapless::String<64>,
}

#[derive(Debug, Error)]
pub enum RoomCreationError {
    #[error("room name is too long")]
    NameTooLong,
}

pub struct RoomManager {
    rooms: DashMap<u32, Arc<Room>>,
    global_room: Arc<Room>,
}

impl RoomManager {
    pub fn new() -> Self {
        let global_room = Arc::new(Room {
            id: 0,
            name: "Global".try_into().unwrap(),
        });

        let rooms = DashMap::new();
        rooms.insert(global_room.id, global_room.clone());

        Self { rooms, global_room }
    }

    pub fn get(&self, id: u32) -> Option<Arc<Room>> {
        self.rooms.get(&id).map(|r| r.clone())
    }

    pub fn global(&self) -> Arc<Room> {
        self.global_room.clone()
    }

    pub fn get_or_global(&self, id: u32) -> Arc<Room> {
        self.get(id).unwrap_or_else(|| self.global().clone())
    }

    pub fn create_room(&self, name: &str) -> Result<Arc<Room>, RoomCreationError> {
        let name = heapless::String::from_str(name).map_err(|_| RoomCreationError::NameTooLong)?;

        loop {
            let id: u32 = rand::random_range(100000..1000000);

            match self.rooms.entry(id) {
                dashmap::Entry::Vacant(entry) => {
                    let room = Arc::new(Room { id, name });
                    entry.insert(room.clone());

                    break Ok(room);
                }

                _ => {
                    continue; // id already exists, try again
                }
            }
        }
    }
}
