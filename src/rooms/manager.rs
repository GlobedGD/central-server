use std::{
    ops::Deref,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use arc_swap::ArcSwap;
use dashmap::DashMap;
use slab::Slab;
use thiserror::Error;

use crate::core::handler::ClientStateHandle;

pub struct Room {
    pub id: u32,
    pub name: heapless::String<64>,
    players: ArcSwap<Slab<ClientStateHandle>>,
    player_count: AtomicUsize,
}

impl Room {
    fn new(id: u32, name: heapless::String<64>) -> Self {
        Self {
            id,
            name,
            players: ArcSwap::new(Arc::new(Slab::new())),
            player_count: AtomicUsize::new(0),
        }
    }

    fn remove_player(&self, key: usize) {
        self.players.rcu(|players| {
            let mut players = (**players).clone();

            // sometimes, this function can run already after the server has shut down and the room has been cleared
            // this would result in a panic inside a dtor, which is quite bad, so let's check just to be sure
            if players.contains(key) {
                players.remove(key);
            }

            players
        });

        self.player_count.fetch_sub(1, Ordering::Relaxed);
    }

    pub(super) fn add_player(self: Arc<Room>, player: ClientStateHandle) -> ClientRoomHandle {
        let mut key = 0;
        self.players.rcu(|players| {
            let mut players = (**players).clone();
            key = players.insert(player.clone());
            players
        });

        self.player_count.fetch_add(1, Ordering::Relaxed);

        ClientRoomHandle {
            room: self.clone(),
            room_key: key,
        }
    }

    fn clear(&self) {
        self.players.store(Arc::new(Slab::new()));
        self.player_count.store(0, Ordering::Relaxed);
    }

    pub fn get_players(&self) -> Arc<Slab<ClientStateHandle>> {
        self.players.load_full()
    }

    pub fn player_count(&self) -> usize {
        self.player_count.load(Ordering::Relaxed)
    }

    pub fn with_players<F, R>(&self, f: F) -> R
    where
        F: FnOnce(usize, slab::Iter<'_, ClientStateHandle>) -> R,
    {
        let players = self.players.load_full();
        f(players.len(), players.iter())
    }
}

pub struct ClientRoomHandle {
    room: Arc<Room>,
    room_key: usize,
}

impl Deref for ClientRoomHandle {
    type Target = Room;

    fn deref(&self) -> &Self::Target {
        &self.room
    }
}

// Remove the player from the player list inside the room once the handle is dropped
impl Drop for ClientRoomHandle {
    fn drop(&mut self) {
        self.room.remove_player(self.room_key);
    }
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
    pub(super) fn new() -> Self {
        let global_room = Arc::new(Room::new(0, "Global".try_into().unwrap()));

        Self {
            rooms: DashMap::new(),
            global_room,
        }
    }

    pub(super) fn get(&self, id: u32) -> Option<Arc<Room>> {
        self.rooms.get(&id).map(|r| r.clone())
    }

    pub(super) fn global(&self) -> Arc<Room> {
        self.global_room.clone()
    }

    pub(super) fn get_or_global(&self, id: u32) -> Arc<Room> {
        self.get(id).unwrap_or_else(|| self.global().clone())
    }

    pub(super) fn create_room(&self, name: &str) -> Result<Arc<Room>, RoomCreationError> {
        let name = heapless::String::from_str(name).map_err(|_| RoomCreationError::NameTooLong)?;

        loop {
            let id: u32 = rand::random_range(100000..1000000);

            match self.rooms.entry(id) {
                dashmap::Entry::Vacant(entry) => {
                    let room = Arc::new(Room::new(id, name));
                    entry.insert(room.clone());

                    break Ok(room);
                }

                _ => {
                    continue; // id already exists, try again
                }
            }
        }
    }

    /// Deletes all rooms from the manager. The global room remains intact, but all players are removed from it.
    pub(super) fn clear(&self) {
        for room in self.rooms.iter() {
            room.clear();
        }

        self.rooms.clear();

        self.global_room.clear();
    }
}
