use std::sync::OnceLock;

use parking_lot::Mutex;

use crate::{auth::ClientAccountData, rooms::ClientRoomHandle};

#[derive(Default)]
pub struct ClientData {
    account_data: OnceLock<ClientAccountData>,
    room: Mutex<Option<ClientRoomHandle>>,
}

impl ClientData {
    pub fn account_data(&self) -> Option<&ClientAccountData> {
        self.account_data.get()
    }

    pub fn set_account_data(&self, data: ClientAccountData) -> bool {
        self.account_data.set(data).is_ok()
    }

    pub fn authorized(&self) -> bool {
        self.account_data().is_some()
    }

    /// Returns the account ID if the client is authorized, otherwise returns 0.
    pub fn account_id(&self) -> i32 {
        self.account_data().map(|x| x.account_id).unwrap_or(0)
    }

    /// Returns the account ID if the client is authorized, otherwise returns 0.
    pub fn user_id(&self) -> i32 {
        self.account_data().map(|x| x.user_id).unwrap_or(0)
    }

    /// Returns the username if the client is authorized, otherwise returns an empty string.
    pub fn username(&self) -> &str {
        self.account_data().map_or("", |x| x.username.as_str())
    }

    /// Sets the room the client is in.
    pub fn set_room(&self, room: ClientRoomHandle) {
        let mut lock = self.room.lock();
        *lock = Some(room);
    }

    /// Clears the room the client is in, returning the room.
    /// Note: this puts a client into an invalid state, you should immediately call `set_room` with another room afterwards.
    pub fn clear_room(&self) -> Option<ClientRoomHandle> {
        self.room.lock().take()
    }
}
