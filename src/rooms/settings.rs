use server_shared::encoding::DataDecodeError;

use crate::core::data::room_settings;

// XXX: when adding new fields, make sure that the defualt of 0 or false is correct,
// otherwise manually implement Default
#[derive(Default, Debug)]
pub struct RoomSettings {
    pub server_id: u8,
    pub player_limit: u16,
    pub faster_reset: bool,
    pub hidden: bool,
    pub private_invites: bool,
    pub is_follower: bool,
    pub level_integrity: bool,
    pub teams: bool,
    pub locked_teams: bool,
    pub manual_pinning: bool,

    pub collision: bool,
    pub two_player_mode: bool,
    pub deathlink: bool,
}

impl RoomSettings {
    pub fn from_reader(reader: room_settings::Reader<'_>) -> Result<Self, DataDecodeError> {
        Ok(Self {
            server_id: reader.get_server_id(),
            player_limit: reader.get_player_limit(),
            faster_reset: reader.get_faster_reset(),
            hidden: reader.get_hidden(),
            private_invites: reader.get_private_invites(),
            is_follower: reader.get_is_follower(),
            level_integrity: reader.get_level_integrity(),
            teams: reader.get_teams(),
            locked_teams: reader.get_locked_teams(),
            manual_pinning: reader.get_manual_pinning(),

            collision: reader.get_collision(),
            two_player_mode: reader.get_two_player_mode(),
            deathlink: reader.get_deathlink(),
        })
    }

    pub fn encode(&self, mut writer: room_settings::Builder<'_>) {
        writer.set_server_id(self.server_id);
        writer.set_player_limit(self.player_limit);
        writer.set_faster_reset(self.faster_reset);
        writer.set_hidden(self.hidden);
        writer.set_private_invites(self.private_invites);
        writer.set_is_follower(self.is_follower);
        writer.set_level_integrity(self.level_integrity);
        writer.set_teams(self.teams);
        writer.set_locked_teams(self.locked_teams);
        writer.set_manual_pinning(self.manual_pinning);

        writer.set_collision(self.collision);
        writer.set_two_player_mode(self.two_player_mode);
        writer.set_deathlink(self.deathlink);
    }
}
