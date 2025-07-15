use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

use crate::rooms::Room;

#[derive(Default, Debug)]
pub struct ClientAccountData {
    pub account_id: i32,
    pub user_id: i32,
    pub username: heapless::String<16>,
}

#[derive(Default)]
pub struct ClientData {
    account_data: OnceLock<ClientAccountData>,
    room: Mutex<Option<Arc<Room>>>,
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

    /// Returns the room the client is in, if any.
    pub fn room(&self) -> Option<Arc<Room>> {
        self.room.lock().clone()
    }

    /// Sets the room the client is in.
    pub fn set_room(&self, room: Arc<Room>) {
        let mut lock = self.room.lock();
        *lock = Some(room);
    }
}
