pub const MAX_USERNAME_LENGTH: usize = 24;
pub const MAX_ROOM_NAME_LENGTH: usize = 32;

pub type UsernameString = heapless::String<MAX_USERNAME_LENGTH>;
pub type RoomNameString = heapless::String<MAX_ROOM_NAME_LENGTH>;
