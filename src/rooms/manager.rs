use std::{
    collections::BTreeSet,
    str::FromStr,
    sync::{Arc, atomic::Ordering},
};

use dashmap::DashMap;
use nohash_hasher::BuildNoHashHasher;
use parking_lot::{RawRwLock, RwLock, lock_api::RwLockReadGuard};
use thiserror::Error;
use tracing::error;

use crate::rooms::{RoomSettings, room::Room};

#[derive(Debug, Error)]
pub enum RoomCreationError {
    #[error("room name is too long")]
    NameTooLong,
}

pub struct RoomManager {
    rooms: DashMap<u32, Arc<Room>, BuildNoHashHasher<u32>>,
    rooms_sorted: RwLock<BTreeSet<(usize, Arc<Room>)>>,
    global_room: Arc<Room>,
}

impl RoomManager {
    pub(super) fn new() -> Self {
        let global_room = Arc::new(Room::new(
            0,
            0,
            "Global Room".try_into().unwrap(),
            0,
            RoomSettings::default(),
        ));

        Self {
            rooms: DashMap::default(),
            rooms_sorted: RwLock::new(BTreeSet::new()),
            global_room,
        }
    }

    pub(super) fn room_count(&self) -> usize {
        self.rooms.len()
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

    pub(super) fn lock_sorted(
        &self,
    ) -> RwLockReadGuard<'_, RawRwLock, BTreeSet<(usize, Arc<Room>)>> {
        self.rooms_sorted.read()
    }

    pub(super) fn create_room(
        &self,
        name: &str,
        passcode: u32,
        owner: i32,
        settings: RoomSettings,
    ) -> Result<Arc<Room>, RoomCreationError> {
        let name = heapless::String::from_str(name).map_err(|_| RoomCreationError::NameTooLong)?;

        loop {
            let id: u32 = rand::random_range(100000..1000000);

            match self.rooms.entry(id) {
                dashmap::Entry::Vacant(entry) => {
                    let room = Arc::new(Room::new(id, owner, name, passcode, settings));

                    entry.insert(room.clone());
                    self.rooms_sorted.write().insert((0, room.clone()));

                    break Ok(room);
                }

                _ => {
                    continue; // id already exists, try again
                }
            }
        }
    }

    pub(super) fn remove_room(&self, id: u32) -> Option<Arc<Room>> {
        if let Some(room) = self.rooms.remove(&id).map(|entry| entry.1) {
            self.do_remove_from_sorted(&room, &mut self.rooms_sorted.write());
            Some(room)
        } else {
            None
        }
    }

    /// Updates the room set, re-adjusting this room's position in the sorted set based on the current player count.
    pub(super) fn update_room_set(&self, room: &Arc<Room>) {
        let mut sorted = self.rooms_sorted.write();

        self.do_remove_from_sorted(room, &mut sorted);
        let count = room.player_count();
        room.key_player_count.store(count, Ordering::Release);
        sorted.insert((count, room.clone()));
    }

    /// Deletes all rooms from the manager. The global room remains intact, but all players are removed from it.
    pub(super) async fn clear(&self) {
        for room in self.rooms.iter() {
            room.clear().await;
        }

        self.rooms.clear();
        self.rooms_sorted.write().clear();

        self.global_room.clear().await;
    }

    pub(super) fn routine_cleanup(&self) {
        for room in self.rooms.iter() {
            room.cleanup_invites();
        }
    }

    #[allow(clippy::mutable_key_type)] // this is okay, because we use room ID as the secondary key, which is immutable
    fn do_remove_from_sorted(&self, room: &Arc<Room>, sorted: &mut BTreeSet<(usize, Arc<Room>)>) {
        let kpc = room.key_player_count.load(Ordering::Acquire);

        if !sorted.remove(&(kpc, room.clone())) {
            error!(
                "internal inconsistency: key ({}, {}) couldn't be found in the sorted rooms set",
                kpc, room.id
            );
        }
    }
}

impl PartialEq for Room {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl PartialOrd for Room {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for Room {}

impl Ord for Room {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
    }
}
