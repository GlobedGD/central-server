pub enum LogAction<'a> {
    Kick {
        account_id: i32,
        reason: &'a str,
    },

    Notice {
        account_id: i32,
        message: &'a str,
    },

    Mute {
        account_id: i32,
        reason: &'a str,
        expires_at: i64,
    },

    EditMute {
        account_id: i32,
        reason: &'a str,
        expires_at: i64,
    },

    Unmute {
        account_id: i32,
    },

    Ban {
        account_id: i32,
        reason: &'a str,
        expires_at: i64,
    },

    EditBan {
        account_id: i32,
        reason: &'a str,
        expires_at: i64,
    },

    Unban {
        account_id: i32,
    },

    RoomBan {
        account_id: i32,
        reason: &'a str,
        expires_at: i64,
    },

    EditRoomBan {
        account_id: i32,
        reason: &'a str,
        expires_at: i64,
    },

    RoomUnban {
        account_id: i32,
    },

    EditRoles {
        account_id: i32,
        rolediff: &'a str,
    },

    EditPassword {
        account_id: i32,
    },
}

impl LogAction<'_> {
    pub fn type_str(&self) -> &'static str {
        match self {
            LogAction::Kick { .. } => "kick",
            LogAction::Notice { .. } => "notice",
            LogAction::Mute { .. } => "mute",
            LogAction::EditMute { .. } => "editmute",
            LogAction::Unmute { .. } => "unmute",
            LogAction::Ban { .. } => "ban",
            LogAction::EditBan { .. } => "editban",
            LogAction::Unban { .. } => "unban",
            LogAction::RoomBan { .. } => "roomban",
            LogAction::EditRoomBan { .. } => "editroomban",
            LogAction::RoomUnban { .. } => "roomunban",
            LogAction::EditRoles { .. } => "editroles",
            LogAction::EditPassword { .. } => "editpassword",
        }
    }

    pub fn account_id(&self) -> i32 {
        match self {
            LogAction::Kick { account_id, .. } => *account_id,
            LogAction::Notice { account_id, .. } => *account_id,
            LogAction::Mute { account_id, .. } => *account_id,
            LogAction::EditMute { account_id, .. } => *account_id,
            LogAction::Unmute { account_id } => *account_id,
            LogAction::Ban { account_id, .. } => *account_id,
            LogAction::EditBan { account_id, .. } => *account_id,
            LogAction::Unban { account_id } => *account_id,
            LogAction::RoomBan { account_id, .. } => *account_id,
            LogAction::EditRoomBan { account_id, .. } => *account_id,
            LogAction::RoomUnban { account_id } => *account_id,
            LogAction::EditRoles { account_id, .. } => *account_id,
            LogAction::EditPassword { account_id, .. } => *account_id,
        }
    }
}
