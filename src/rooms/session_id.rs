/// Structure of a session ID:
/// top 8 bits: server ID
/// next 32 bits: level ID
/// last 24 bits: arbitrary but unique number
pub struct SessionId(pub u64);

impl SessionId {
    /// Creates a new `SessionId` from the given server ID, level ID, and unique number.
    pub fn from_parts(server_id: u8, level_id: i32, uniq: u32) -> SessionId {
        let server_id = u64::from(server_id) << 56;
        let level_id = u64::from(level_id as u32) << 24;
        let uniq = u64::from(uniq) & 0x00ffffff;
        SessionId(server_id | level_id | uniq)
    }

    /// Generates a new `SessionId` with a random unique number.
    pub fn generate(server_id: u8, level_id: i32) -> SessionId {
        let uniq = rand::random::<u32>() & 0x00ffffff;
        SessionId::from_parts(server_id, level_id, uniq)
    }

    pub fn server_id(&self) -> u8 {
        (self.0 >> 56 & 0xff) as u8
    }

    pub fn level_id(&self) -> i32 {
        ((self.0 >> 24) & 0xffffffff) as i32
    }

    pub fn uniq(&self) -> u32 {
        (self.0 & 0x00ffffff) as u32
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}
