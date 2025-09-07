use std::{
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use parking_lot::{Mutex, RwLock};
use slab::Slab;
use smallvec::SmallVec;
use thiserror::Error;
use tracing::{debug, error, warn};

use crate::{
    core::{data::RoomJoinFailedReason, handler::ClientStateHandle},
    rooms::{RoomSettings, invite_token::InviteToken},
};

pub const MAX_TEAM_COUNT: usize = 100;
pub const INVITE_LIFETIME: Duration = Duration::from_mins(15);

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

struct StoredInviteToken {
    token: InviteToken,
    created_at: Instant,
}

pub struct Room {
    pub id: u32,
    pub name: heapless::String<64>,
    pub passcode: u32,
    pub owner: AtomicI32,
    pub original_owner: i32,
    pub settings: Mutex<RoomSettings>,
    teams: RwLock<SmallVec<[RoomTeam; 8]>>,
    banned: RwLock<SmallVec<[i32; 8]>>,

    invite_tokens: Mutex<SmallVec<[StoredInviteToken; 8]>>,
    created_at: Instant,

    players: RoomPlayerStore,
    player_count: AtomicUsize,
    pub(super) key_player_count: AtomicUsize,
    joinable: AtomicBool,
}

impl Room {
    pub fn new(
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
            invite_tokens: Mutex::new(SmallVec::new()),
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

    pub async fn clear(&self) {
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

    pub fn private_invites(&self) -> bool {
        self.settings.lock().private_invites
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

    pub fn consume_invite_token(&self, token: InviteToken) -> bool {
        let mut tokens = self.invite_tokens.lock();

        match tokens.binary_search_by_key(&token, |t| t.token) {
            Ok(pos) => {
                tokens.remove(pos);
                true
            }

            Err(_) => false,
        }
    }

    pub fn create_invite_token(&self) -> InviteToken {
        let mut tokens = self.invite_tokens.lock();

        if tokens.len() >= 128 {
            let idx = tokens.len() - 1;

            warn!(
                "Invite overflow in room {} ({} invites), removing invite ID {}",
                self.id,
                tokens.len(),
                tokens[idx].token
            );

            tokens.remove(idx);
        }

        loop {
            let token = InviteToken::new_random(self.id);

            match tokens.binary_search_by_key(&token, |t| t.token) {
                Ok(_) => continue,
                Err(pos) => {
                    tokens.insert(
                        pos,
                        StoredInviteToken {
                            token,
                            created_at: Instant::now(),
                        },
                    );
                    break token;
                }
            }
        }
    }

    pub fn cleanup_invites(&self) {
        self.invite_tokens.lock().retain(|inv| inv.created_at.elapsed() < INVITE_LIFETIME);
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
