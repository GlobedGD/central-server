use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicI32, AtomicU16, AtomicU32, AtomicU64, Ordering},
};

use nohash_hasher::IntSet;
use parking_lot::{Mutex, MutexGuard};
use rustc_hash::FxHashSet;
use server_shared::{UserSettings, data::PlayerIconData};

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
    room_id: AtomicU32, // also redundant
    session_id: AtomicU64,
    authorized_admin: AtomicBool,
    deauthorized: AtomicBool,
    team_id: AtomicU16,
    discord_pairing_on: AtomicBool,
    discord_linked: AtomicBool,
    awaiting_notice_reply_from: Mutex<IntSet<i32>>,

    pub active_mute: Mutex<Option<UserPunishment>>,
    pub active_room_ban: Mutex<Option<UserPunishment>>,
    admin_password_hash: Mutex<Option<String>>,
    role: Mutex<Option<ComputedRole>>,
    uident: OnceLock<[u8; 32]>,
    settings: Mutex<UserSettings>,
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
        Some(self.room_id.load(Ordering::Relaxed))
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
        self.room_id.store(room.id, Ordering::Relaxed);
        *self.room.lock() = Some(room);
    }

    /// Clears the room the client is in, removing them from it and returning the room.
    /// Note: this puts a client into an invalid state, you should immediately call `set_room` with another room afterwards.
    pub async fn clear_room(&self) -> Option<Arc<Room>> {
        self.set_team_id(0);
        self.room_id.store(0, Ordering::Relaxed);

        let handle = self.room.lock().take();

        if let Some(mut handle) = handle {
            Some(handle.dispose().await)
        } else {
            None
        }
    }

    /// Returns team ID
    pub fn team_id(&self) -> u16 {
        self.team_id.load(Ordering::Relaxed)
    }

    pub fn set_team_id(&self, value: u16) {
        self.team_id.store(value, Ordering::Relaxed);
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

    pub fn role(&self) -> MutexGuard<'_, Option<ComputedRole>> {
        self.role.lock()
    }

    pub fn set_role(&self, role: ComputedRole) {
        *self.role.lock() = Some(role);
    }

    pub fn can_moderate(&self) -> bool {
        self.role().as_ref().is_some_and(|x| x.can_moderate())
    }

    pub fn authorized_mod(&self) -> bool {
        self.authorized_admin.load(Ordering::Relaxed)
    }

    pub fn set_authorized_mod(&self) {
        self.authorized_admin.store(true, Ordering::Relaxed);
    }

    pub fn set_uident(&self, uident: [u8; 32]) {
        let _ = self.uident.set(uident);
    }

    pub fn uident(&self) -> Option<&[u8; 32]> {
        self.uident.get()
    }

    pub fn set_settings(&self, settings: UserSettings) {
        *self.settings.lock() = settings;
    }

    pub fn settings(&self) -> UserSettings {
        *self.settings.lock()
    }

    pub fn set_discord_pairing(&self, enabled: bool) {
        self.discord_pairing_on.store(enabled, Ordering::Relaxed);
    }

    pub fn discord_pairing(&self) -> bool {
        self.discord_pairing_on.load(Ordering::Relaxed)
    }

    pub fn set_discord_linked(&self, linked: bool) {
        self.discord_linked.store(linked, Ordering::Relaxed);
    }

    pub fn is_discord_linked(&self) -> bool {
        self.discord_linked.load(Ordering::Relaxed)
    }

    pub fn take_awaiting_notice_reply(&self, user_id: i32) -> bool {
        self.awaiting_notice_reply_from.lock().remove(&user_id)
    }

    pub fn add_awaiting_notice_reply(&self, user_id: i32) {
        self.awaiting_notice_reply_from.lock().insert(user_id);
    }
}
