use std::fmt::Display;

#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug, PartialOrd, Ord)]
pub struct InviteToken(pub u64);

impl InviteToken {
    pub fn new_random(room_id: u32) -> Self {
        let rval = rand::random::<u64>() & 0x0000_00ff_ffff_ffff;
        let room_id = room_id & 0x00ff_ffff;
        let value = rval | ((room_id as u64) << 40);

        Self::from(value)
    }

    pub fn get(&self) -> u64 {
        self.0
    }

    pub fn room_id(&self) -> u32 {
        ((self.0 & 0xffff_ff00_0000_0000) >> 40) as u32
    }

    pub fn entropy(&self) -> u64 {
        self.0 & 0x0000_00ff_ffff_ffff
    }
}

impl From<u64> for InviteToken {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl Display for InviteToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
