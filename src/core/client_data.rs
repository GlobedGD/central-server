use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
};

use parking_lot::{Mutex, MutexGuard};
use rustc_hash::FxHashSet;
use server_shared::data::PlayerIconData;

use crate::{
    auth::ClientAccountData,
    rooms::{ClientRoomHandle, Room},
    users::{ComputedRole, UserPunishment},
};

#[derive(Default)]
pub struct ClientData {
    account_data: OnceLock<ClientAccountData>,
    account_id: AtomicI32, // redundant, for faster access
    icons: Mutex<PlayerIconData>,
    pub friend_list: Mutex<FxHashSet<i32>>,

    room: Mutex<Option<ClientRoomHandle>>,
    session_id: AtomicU64,
    deauthorized: AtomicBool,

    pub active_mute: Mutex<Option<UserPunishment>>,
    pub active_room_ban: Mutex<Option<UserPunishment>>,
    admin_password_hash: Mutex<Option<String>>,
    role: OnceLock<ComputedRole>,
}

impl ClientData {
    pub fn account_data(&self) -> Option<&ClientAccountData> {
        if self.deauthorized.load(Ordering::Relaxed) {
            return None;
        }

        self.account_data.get()
    }

    pub fn set_account_data(&self, data: ClientAccountData) -> bool {
        let account_id = data.account_id;

        if self.account_data.set(data).is_ok() {
            self.account_id.store(account_id, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn authorized(&self) -> bool {
        self.account_data().is_some()
    }

    /// Returns the account ID if the client is authorized, otherwise returns 0.
    pub fn account_id(&self) -> i32 {
        self.account_id.load(Ordering::Relaxed)
    }

    /// Returns the account ID if the client is authorized, otherwise returns 0.
    pub fn user_id(&self) -> i32 {
        self.account_data().map(|x| x.user_id).unwrap_or(0)
    }

    /// Returns the username if the client is authorized, otherwise returns an empty string.
    pub fn username(&self) -> &str {
        self.account_data().map_or("", |x| x.username.as_str())
    }

    /// Deauthorizes the client, clearing the room
    pub async fn deauthorize(&self) {
        self.deauthorized.store(true, Ordering::Relaxed);
        self.clear_room().await;
    }

    /// Returns the room the client is in, or None if not in a room.
    pub fn get_room_id(&self) -> Option<u32> {
        self.room.lock().as_ref().map(|r| r.id)
    }

    pub fn lock_room(&self) -> MutexGuard<'_, Option<ClientRoomHandle>> {
        self.room.lock()
    }

    /// Returns whether the client is connected to the given room
    pub fn is_in_room(&self, room: &Room) -> bool {
        self.room.lock().as_ref().is_some_and(|r| r.id == room.id)
    }

    /// Sets the room the client is in.
    pub fn set_room(&self, room: ClientRoomHandle) {
        let mut lock = self.room.lock();
        *lock = Some(room);
    }

    /// Clears the room the client is in, removing them from it and returning the room.
    /// Note: this puts a client into an invalid state, you should immediately call `set_room` with another room afterwards.
    pub async fn clear_room(&self) -> Option<Arc<Room>> {
        let handle = self.room.lock().take();

        if let Some(mut handle) = handle {
            Some(handle.dispose().await)
        } else {
            None
        }
    }

    /// Returns client's current session (aka which level they are in)
    pub fn session_id(&self) -> u64 {
        self.session_id.load(Ordering::Relaxed)
    }

    /// Sets the client's session ID, returning the previous session ID.
    pub fn set_session_id(&self, session_id: u64) -> u64 {
        self.session_id.swap(session_id, Ordering::Relaxed)
    }

    pub fn set_icons(&self, icons: PlayerIconData) {
        let mut lock = self.icons.lock();
        *lock = icons;
    }

    pub fn icons(&self) -> PlayerIconData {
        *self.icons.lock()
    }

    pub fn set_friends(&self, friends: FxHashSet<i32>) {
        let mut lock = self.friend_list.lock();
        *lock = friends;
    }

    pub fn set_active_punishments(
        &self,
        mute: Option<UserPunishment>,
        room_ban: Option<UserPunishment>,
    ) {
        let mut active_mute = self.active_mute.lock();
        let mut active_room_ban = self.active_room_ban.lock();

        *active_mute = mute;
        *active_room_ban = room_ban;
    }

    pub fn set_admin_password_hash(&self, hash: Option<String>) {
        let mut lock = self.admin_password_hash.lock();
        *lock = hash;
    }

    pub fn role(&self) -> Option<&ComputedRole> {
        self.role.get()
    }

    pub fn set_role(&self, role: ComputedRole) {
        let _ = self.role.set(role);
    }
}
