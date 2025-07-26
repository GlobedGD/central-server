/// Structure of a session ID:
/// top 8 bits: server ID
/// next 24 bits: room ID
/// last 32 bits: level ID
pub struct SessionId(pub u64);

impl SessionId {
    /// Creates a new `SessionId` from the given server ID, level ID, and room ID.
    pub fn from_parts(server_id: u8, level_id: i32, room_id: u32) -> SessionId {
        let server_id = u64::from(server_id) << 56;
        let room_id = u64::from(room_id & 0x00ffffff) << 32;
        let level_id = u64::from(level_id as u32);
        SessionId(server_id | room_id | level_id)
    }

    pub fn server_id(&self) -> u8 {
        (self.0 >> 56) as u8
    }

    pub fn level_id(&self) -> i32 {
        (self.0 & 0xffffffff) as i32
    }

    pub fn room_id(&self) -> u32 {
        ((self.0 >> 32) & 0x00ffffff) as u32
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl From<u64> for SessionId {
    fn from(value: u64) -> Self {
        SessionId(value)
    }
}
