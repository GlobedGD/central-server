use std::{
    collections::BTreeSet,
    ops::Deref,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use dashmap::DashMap;
use parking_lot::{Mutex, RawRwLock, RwLock, lock_api::RwLockReadGuard};
use slab::Slab;
use smallvec::SmallVec;
use thiserror::Error;
use tracing::{debug, error};

use crate::{
    core::{data::RoomJoinFailedReason, handler::ClientStateHandle},
    rooms::RoomSettings,
};

pub const MAX_TEAM_COUNT: usize = 100;

#[derive(Clone)]
pub struct RoomPlayer {
    pub handle: ClientStateHandle,
    pub team_id: u16,
}

impl RoomPlayer {
    pub fn new(handle: ClientStateHandle) -> Self {
        Self { handle, team_id: 0 }
    }
}

enum RoomPlayerStore {
    Sync(RwLock<Slab<RoomPlayer>>),
    Async(tokio::sync::RwLock<Slab<RoomPlayer>>),
}

#[derive(Default, Clone)]
pub struct RoomTeam {
    pub color: u32,
}

impl RoomTeam {
    pub fn new(color: u32) -> Self {
        Self { color }
    }
}

#[derive(Error, Debug)]
pub enum TeamCreationFailed {
    #[error("Too many teams")]
    TooManyTeams,
}

#[derive(Error, Debug)]
#[error("Team not found")]
pub struct TeamNotFound;

pub struct Room {
    pub id: u32,
    pub name: heapless::String<64>,
    pub passcode: u32,
    pub owner: AtomicI32,
    pub original_owner: i32,
    pub settings: Mutex<RoomSettings>,
    teams: RwLock<SmallVec<[RoomTeam; 8]>>,
    banned: RwLock<SmallVec<[i32; 8]>>,
    created_at: Instant,

    players: RoomPlayerStore,
    player_count: AtomicUsize,
    key_player_count: AtomicUsize,
    joinable: AtomicBool,
}

impl Room {
    fn new(
        id: u32,
        owner: i32,
        name: heapless::String<64>,
        passcode: u32,
        settings: RoomSettings,
    ) -> Self {
        Self {
            id,
            owner: AtomicI32::new(owner),
            original_owner: owner,
            name,
            settings: Mutex::new(settings),
            passcode,
            teams: RwLock::new(SmallVec::from_elem(RoomTeam::new(0xffffffff), 1)),
            banned: RwLock::new(SmallVec::new()),
            created_at: Instant::now(),

            // global room use async locks because there is way more contention
            players: if id == 0 {
                RoomPlayerStore::Async(tokio::sync::RwLock::new(Slab::new()))
            } else {
                RoomPlayerStore::Sync(RwLock::new(Slab::new()))
            },

            player_count: AtomicUsize::new(0),
            key_player_count: AtomicUsize::new(0),
            joinable: AtomicBool::new(true),
        }
    }

    #[inline]
    async fn run_write_action<R>(&self, action: impl FnOnce(&mut Slab<RoomPlayer>) -> R) -> R {
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
    fn run_sync_write_action<R>(&self, action: impl FnOnce(&mut Slab<RoomPlayer>) -> R) -> R {
        match &self.players {
            RoomPlayerStore::Sync(lock) => {
                let mut players = lock.write();
                action(&mut players)
            }

            RoomPlayerStore::Async(_) => {
                panic!("run_sync_write_action called on global room");
            }
        }
    }

    #[inline]
    async fn run_read_action<R>(&self, action: impl FnOnce(&Slab<RoomPlayer>) -> R) -> R {
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

    #[inline]
    fn run_sync_read_action<R>(&self, action: impl FnOnce(&Slab<RoomPlayer>) -> R) -> R {
        match &self.players {
            RoomPlayerStore::Sync(lock) => {
                let mut players = lock.read();
                action(&mut players)
            }

            RoomPlayerStore::Async(_) => {
                panic!("run_sync_read_action called on global room");
            }
        }
    }

    pub fn set_settings(&self, settings: RoomSettings) {
        *self.settings.lock() = settings;
    }

    async fn remove_player(&self, key: usize) {
        self.run_write_action(|players| {
            if players.contains(key) {
                self.player_count.store(players.len() - 1, Ordering::Relaxed);
                let plr = players.remove(key);

                if self.owner() == plr.handle.account_id() {
                    self.rotate_owner(players);
                }
            }
        })
        .await;
    }

    fn rotate_owner(&self, players: &mut Slab<RoomPlayer>) {
        if let Some((_, player)) = players.iter().next() {
            let id = player.handle.account_id();
            let prev_id = self.owner.swap(id, Ordering::Relaxed);

            debug!("rotating owner from {} to {} for room {}", prev_id, id, self.id);
        }
    }

    fn make_handle(self: &Arc<Self>, key: usize) -> ClientRoomHandle {
        ClientRoomHandle {
            room: self.clone(),
            room_key: key,
            #[cfg(debug_assertions)]
            disposed: false,
        }
    }

    fn maybe_restore_owner(&self, player: &ClientStateHandle) {
        if player.account_id() == self.original_owner {
            self.owner.store(self.original_owner, Ordering::Relaxed);
        }
    }

    pub(super) async fn force_add_player(
        self: Arc<Room>,
        player: ClientStateHandle,
    ) -> ClientRoomHandle {
        self.maybe_restore_owner(&player);

        let key = self
            .run_write_action(|players| {
                self.player_count.store(players.len() + 1, Ordering::Relaxed);
                players.insert(RoomPlayer::new(player))
            })
            .await;

        self.make_handle(key)
    }

    pub(super) async fn add_player(
        self: Arc<Room>,
        player: ClientStateHandle,
        passcode: u32,
    ) -> Result<ClientRoomHandle, RoomJoinFailedReason> {
        if !self.joinable.load(Ordering::Relaxed) {
            return Err(RoomJoinFailedReason::NotFound);
        }

        if self.passcode != 0 && self.passcode != passcode {
            return Err(RoomJoinFailedReason::InvalidPasscode);
        }

        let player_id = player.account_id();
        if self.is_banned(player_id) {
            return Err(RoomJoinFailedReason::Banned);
        }

        let player_limit = self.settings.lock().player_limit as usize;

        if player_limit != 0 {
            // check if the room is full
            let mut player_count = self.player_count.load(Ordering::Relaxed);

            loop {
                if player_count >= player_limit {
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
        }

        self.maybe_restore_owner(&player);

        let key = self
            .run_write_action(|players| {
                // re-update the player count, as it may have changed after the check (and the check is only done if there is a limit anyway)
                self.player_count.store(players.len() + 1, Ordering::Relaxed);

                players.insert(RoomPlayer::new(player))
            })
            .await;

        Ok(self.make_handle(key))
    }

    pub fn make_unjoinable(&self) {
        self.joinable.store(false, Ordering::Relaxed);
    }

    pub fn has_player(&self, player: &ClientStateHandle) -> bool {
        player.get_room_id().is_some_and(|id| id == self.id)
    }

    pub fn since_creation(&self) -> Duration {
        self.created_at.elapsed()
    }

    pub fn owner(&self) -> i32 {
        self.owner.load(Ordering::Relaxed)
    }

    pub fn team_id_for_player(&self, key: usize) -> u16 {
        if self.is_global() {
            return 0;
        }

        self.run_sync_read_action(|players| match players.get(key) {
            Some(p) => p.team_id,
            None => {
                error!("team_id_for_player called on non-existent player!");
                error!("key: {}, room id: {}, players: {}", key, self.id, self.player_count());

                0
            }
        })
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
        self.settings.lock().is_follower
    }

    pub fn is_global(&self) -> bool {
        self.id == 0
    }

    pub fn has_password(&self) -> bool {
        self.passcode != 0
    }

    pub fn ban_player(&self, id: i32) {
        let mut players = self.banned.write();
        if players.len() > 256 {
            return;
        }

        match players.binary_search(&id) {
            Ok(_) => {
                // already banned, do nothing
            }

            Err(pos) => {
                // insert into that index to maintain sorted order
                players.insert(pos, id);
            }
        }
    }

    pub fn is_banned(&self, id: i32) -> bool {
        self.banned.read().binary_search(&id).is_ok()
    }

    pub async fn with_players<F, R>(&self, f: F) -> R
    where
        F: FnOnce(usize, slab::Iter<'_, RoomPlayer>) -> R,
    {
        self.run_read_action(|players| f(players.len(), players.iter())).await
    }

    pub fn with_players_sync<F, R>(&self, f: F) -> R
    where
        F: FnOnce(usize, slab::Iter<'_, RoomPlayer>) -> R,
    {
        self.run_sync_read_action(|players| f(players.len(), players.iter()))
    }

    // Team management

    /// Attempts to create a new team in this room, returns the count of teams on success
    pub fn create_team<T: Into<Option<u32>>>(&self, color: T) -> Result<usize, TeamCreationFailed> {
        let mut teams = self.teams.write();

        if teams.len() >= MAX_TEAM_COUNT {
            return Err(TeamCreationFailed::TooManyTeams);
        }

        teams.push(RoomTeam::new(color.into().unwrap_or(0xffffffff)));

        Ok(teams.len())
    }

    pub fn set_team_color(&self, team_id: u16, color: u32) -> bool {
        let mut teams = self.teams.write();

        if let Some(team) = teams.get_mut(team_id as usize) {
            team.color = color;
            true
        } else {
            false
        }
    }

    pub fn team_count(&self) -> usize {
        if self.is_global() { 0 } else { self.teams.read().len() }
    }

    /// Deletes a team from the room. If the team removed is not the last team, team indices will be shifted for the last team.
    /// Team IDs are shifted for every person in the team that was removed.
    /// Returns a list of players that were modified, and whom should be notified about that
    pub fn delete_team(&self, team_id: u16) -> Result<Vec<RoomPlayer>, TeamNotFound> {
        let mut modified = Vec::new();
        let mut teams = self.teams.write();

        // disallow deleting invalid teams as well as the last remaining team
        if team_id as usize >= teams.len() || teams.len() == 1 {
            return Err(TeamNotFound);
        };

        self.run_sync_write_action(|players| {
            // remove all players that were in this team
            for (_, player) in players.iter_mut() {
                if player.team_id == team_id {
                    player.team_id = 0;
                    modified.push(player.clone());
                }
            }

            teams.remove(team_id as usize);

            // if this was not the last team, all the teams afterwards should be notified about their team index being changed
            for (_, player) in players.iter_mut() {
                if player.team_id > team_id {
                    player.team_id -= 1;
                    modified.push(player.clone());
                }
            }
        });

        Ok(modified)
    }

    /// Attempts to assign a player to a specific team, fails and returns `false`
    /// if the team id or player id are invalid
    pub fn assign_team_to_player(&self, team_id: u16, player_id: i32) -> bool {
        if team_id as usize >= self.teams.read().len() {
            return false;
        }

        self.run_sync_write_action(|players| {
            if let Some((_, player)) =
                players.iter_mut().find(|p| p.1.handle.account_id() == player_id)
            {
                player.team_id = team_id;
                true
            } else {
                false
            }
        })
    }

    pub fn get_players_on_team(&self, team_id: u16) -> Result<Vec<RoomPlayer>, TeamNotFound> {
        let teams = self.teams.read();

        if (team_id as usize) < teams.len() {
            Ok(self.run_sync_read_action(|players| {
                let mut out = Vec::new();

                for (_, player) in players.iter() {
                    out.push(player.clone());
                }

                out
            }))
        } else {
            Err(TeamNotFound)
        }
    }

    pub fn with_teams<F, R>(&self, f: F) -> R
    where
        F: FnOnce(usize, std::slice::Iter<'_, RoomTeam>) -> R,
    {
        let teams = self.teams.read();
        f(teams.len(), teams.iter())
    }
}

pub struct ClientRoomHandle {
    pub(super) room: Arc<Room>,
    room_key: usize,
    #[cfg(debug_assertions)]
    disposed: bool,
}

impl ClientRoomHandle {
    pub async fn dispose(&mut self) -> Arc<Room> {
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

        self.room.clone()
    }

    pub fn team_id(&self) -> u16 {
        self.room.team_id_for_player(self.room_key)
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
            rooms: DashMap::new(),
            global_room,
            rooms_sorted: RwLock::new(BTreeSet::new()),
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
