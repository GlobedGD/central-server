use std::{
    ops::Deref,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use dashmap::DashMap;
use slab::Slab;
use thiserror::Error;

use crate::{
    core::{data::RoomJoinFailedReason, handler::ClientStateHandle},
    rooms::RoomSettings,
};

enum RoomPlayerStore {
    Sync(parking_lot::RwLock<Slab<ClientStateHandle>>),
    Async(tokio::sync::RwLock<Slab<ClientStateHandle>>),
}

pub struct Room {
    pub id: u32,
    pub name: heapless::String<64>,
    pub owner: i32,
    pub settings: RoomSettings,
    players: RoomPlayerStore,
    player_count: AtomicUsize,
}

impl Room {
    fn new(id: u32, owner: i32, name: heapless::String<64>, settings: RoomSettings) -> Self {
        Self {
            id,
            owner,
            name,
            settings,
            // global room use async locks because there is way more contention
            players: if id == 0 {
                RoomPlayerStore::Async(tokio::sync::RwLock::new(Slab::new()))
            } else {
                RoomPlayerStore::Sync(parking_lot::RwLock::new(Slab::new()))
            },

            player_count: AtomicUsize::new(0),
        }
    }

    #[inline]
    async fn run_write_action<R>(
        &self,
        action: impl FnOnce(&mut Slab<ClientStateHandle>) -> R,
    ) -> R {
        match &self.players {
            RoomPlayerStore::Sync(lock) => {
                let mut players = lock.write();
                action(&mut players)
            }

            RoomPlayerStore::Async(lock) => {
                let mut players = lock.write().await;
                action(&mut players)
            }
        }
    }

    #[inline]
    async fn run_read_action<R>(&self, action: impl FnOnce(&Slab<ClientStateHandle>) -> R) -> R {
        match &self.players {
            RoomPlayerStore::Sync(lock) => {
                let players = lock.read();
                action(&players)
            }

            RoomPlayerStore::Async(lock) => {
                let players = lock.read().await;
                action(&players)
            }
        }
    }

    async fn remove_player(&self, key: usize) {
        self.run_write_action(|players| {
            if players.contains(key) {
                players.remove(key);
                self.player_count.store(players.len(), Ordering::Relaxed);
            }
        })
        .await;
    }

    fn make_handle(self: &Arc<Self>, key: usize) -> ClientRoomHandle {
        ClientRoomHandle {
            room: self.clone(),
            room_key: key,
            #[cfg(debug_assertions)]
            disposed: false,
        }
    }

    pub(super) async fn force_add_player(
        self: Arc<Room>,
        player: ClientStateHandle,
    ) -> ClientRoomHandle {
        let key = self.run_write_action(|players| players.insert(player)).await;

        self.player_count.fetch_add(1, Ordering::Relaxed);

        self.make_handle(key)
    }

    pub(super) async fn add_player(
        self: Arc<Room>,
        player: ClientStateHandle,
        passcode: u32,
    ) -> Result<ClientRoomHandle, RoomJoinFailedReason> {
        // TODO: passcode

        if self.settings.player_limit != 0 {
            // check if the room is full
            let mut player_count = self.player_count.load(Ordering::Relaxed);

            loop {
                if player_count >= self.settings.player_limit as usize {
                    return Err(RoomJoinFailedReason::Full);
                }

                match self.player_count.compare_exchange_weak(
                    player_count,
                    player_count + 1,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(current_count) => {
                        player_count = current_count;
                        continue; // retry
                    }
                }
            }
        } else {
            // no limit, just increment the count
            self.player_count.fetch_add(1, Ordering::Relaxed);
        }

        let key = self.run_write_action(|players| players.insert(player)).await;

        Ok(self.make_handle(key))
    }

    pub fn has_player(&self, player: &ClientStateHandle) -> bool {
        player.get_room_id().is_some_and(|id| id == self.id)
    }

    async fn clear(&self) {
        self.run_write_action(|players| {
            *players = Slab::new();
        })
        .await;

        self.player_count.store(0, Ordering::Relaxed);
    }

    pub fn player_count(&self) -> usize {
        self.player_count.load(Ordering::Relaxed)
    }

    pub fn is_follower(&self) -> bool {
        self.settings.is_follower
    }

    pub fn is_global(&self) -> bool {
        self.id == 0
    }

    pub async fn with_players<F, R>(&self, f: F) -> R
    where
        F: FnOnce(usize, slab::Iter<'_, ClientStateHandle>) -> R,
    {
        self.run_read_action(|players| f(players.len(), players.iter())).await
    }
}

pub struct ClientRoomHandle {
    room: Arc<Room>,
    room_key: usize,
    #[cfg(debug_assertions)]
    disposed: bool,
}

impl ClientRoomHandle {
    pub async fn dispose(&mut self) {
        self.room.remove_player(self.room_key).await;

        #[cfg(debug_assertions)]
        {
            if self.disposed {
                tracing::error!(
                    "ClientRoomHandle::dispose() called multiple times for the same handle (room = {}, key = {})",
                    self.room.id,
                    self.room_key
                );
            }
            self.disposed = true;
        }
    }
}

#[cfg(debug_assertions)]
impl Drop for ClientRoomHandle {
    fn drop(&mut self) {
        if !self.disposed {
            tracing::error!(
                "ClientRoomHandle dropped without calling dispose() (room = {}, key = {})",
                self.room.id,
                self.room_key
            );
        }
    }
}

impl Deref for ClientRoomHandle {
    type Target = Room;

    fn deref(&self) -> &Self::Target {
        &self.room
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
        let global_room =
            Arc::new(Room::new(0, 0, "Global Room".try_into().unwrap(), RoomSettings::default()));

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

    pub(super) fn create_room(
        &self,
        name: &str,
        owner: i32,
        settings: RoomSettings,
    ) -> Result<Arc<Room>, RoomCreationError> {
        let name = heapless::String::from_str(name).map_err(|_| RoomCreationError::NameTooLong)?;

        loop {
            let id: u32 = rand::random_range(100000..1000000);

            match self.rooms.entry(id) {
                dashmap::Entry::Vacant(entry) => {
                    let room = Arc::new(Room::new(id, owner, name, settings));
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
    pub(super) async fn clear(&self) {
        for room in self.rooms.iter() {
            room.clear().await;
        }

        self.rooms.clear();

        self.global_room.clear().await;
    }
}
