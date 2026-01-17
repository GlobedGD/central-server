use std::{num::NonZeroI64, sync::Arc};

use rand::seq::IteratorRandom;
use server_shared::qunet::buffers::ByteWriter;

use crate::{
    auth::ClientAccountData,
    rooms::{Room, RoomCreationError, RoomModule, RoomSettings},
    users::UsersModule,
};

use super::{ConnectionHandler, util::*};

impl ConnectionHandler {
    pub async fn handle_create_room(
        &self,
        client: &ClientStateHandle,
        mut name: &str,
        passcode: u32,
        settings: RoomSettings,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        name = name.trim();
        if !name.is_ascii() || name.is_empty() {
            return self.send_room_create_failed(client, data::RoomCreateFailedReason::InvalidName);
        }

        if let Some(p) = client.active_room_ban.lock().as_ref() {
            // user is room banned, don't allow creating rooms
            return self.send_room_banned(client, &p.reason, p.expires_at);
        }

        let users = self.module::<UsersModule>();
        let rooms = self.module::<RoomModule>();
        let server_id = settings.server_id;

        // check if the requested server is valid
        if !self.game_server_manager.has_server(server_id) {
            return self
                .send_room_create_failed(client, data::RoomCreateFailedReason::InvalidServer);
        }

        let default_name;
        // if the user is not allowed to name rooms, override the name with a default one
        if users.disallow_room_names && client.role().as_ref().is_none_or(|r| !r.can_name_rooms) {
            default_name = format!("{}'s Room", client.username());
            name = &default_name;
        }

        // check if the name is a-ok

        if self.is_disallowed(name).await {
            return self
                .send_room_create_failed(client, data::RoomCreateFailedReason::InappropriateName);
        }

        let new_room = match rooms
            .create_room_and_join(name, passcode, settings, client, &self.game_server_manager)
            .await
        {
            Ok(new_room) => new_room,

            Err(RoomCreationError::NameTooLong) => {
                return self
                    .send_room_create_failed(client, data::RoomCreateFailedReason::InvalidName);
            }
        };

        // notify the game server about the new room being created and wait for the response
        match self
            .game_server_manager
            .notify_room_created(server_id, new_room.id, passcode, client.account_id())
            .await
        {
            Ok(()) => {
                self.send_room_data(client, &new_room).await?;
            }

            Err(e) => {
                // failed :(
                warn!(
                    "[{}] failed to create room on game server {}: {}",
                    client.address, server_id, e
                );

                // leave back to the global room
                return self.handle_leave_room(client).await;
            }
        }

        Ok(())
    }

    fn send_room_create_failed(
        &self,
        client: &ClientStateHandle,
        reason: data::RoomCreateFailedReason,
    ) -> HandlerResult<()> {
        let buf = data::encode_message!(self, 40, msg => {
            let mut create_failed = msg.reborrow().init_room_create_failed();
            create_failed.set_reason(reason);
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }

    fn send_room_banned(
        &self,
        client: &ClientStateHandle,
        reason: &str,
        expires_at: Option<NonZeroI64>,
    ) -> HandlerResult<()> {
        let cap = 56 + reason.len();
        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut room_banned = msg.reborrow().init_room_banned();
            room_banned.set_reason(reason);
            room_banned.set_expires_at(expires_at.map_or(0, |x| x.get()));
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }

    pub async fn handle_join_room(
        &self,
        client: &ClientStateHandle,
        id: u32,
        passcode: u32,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let rooms = self.module::<RoomModule>();
        match rooms.join_room_by_id(client, &self.game_server_manager, id, passcode).await {
            Ok(new_room) => self.send_room_data(client, &new_room).await,
            Err(reason) => self.send_room_join_failed(client, reason),
        }
    }

    pub async fn handle_join_room_by_token(
        &self,
        client: &ClientStateHandle,
        token: u64,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let rooms = self.module::<RoomModule>();
        match rooms.join_room_by_invite_token(client, &self.game_server_manager, token).await {
            Ok(new_room) => self.send_room_data(client, &new_room).await,
            Err(reason) => self.send_room_join_failed(client, reason),
        }
    }

    pub async fn handle_leave_room(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        // Leaving a room is the same as joining the global room
        self.handle_join_room(client, 0, 0).await
    }

    fn send_room_join_failed(
        &self,
        client: &ClientStateHandle,
        reason: data::RoomJoinFailedReason,
    ) -> HandlerResult<()> {
        let buf = data::encode_message!(self, 40, msg => {
            let mut join_failed = msg.reborrow().init_room_join_failed();
            join_failed.set_reason(reason);
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }

    pub(crate) fn encode_account_data(
        player: &ClientAccountData,
        mut builder: data::player_account_data::Builder<'_>,
    ) {
        builder.set_account_id(player.account_id);
        builder.set_user_id(player.user_id);
        builder.set_username(&player.username);
    }

    pub(crate) fn encode_room_player(
        is_mod: bool,
        player: &ClientStateHandle,
        mut builder: data::room_player::Builder<'_>,
    ) {
        let icons = player.icons();

        Self::encode_account_data(
            player.account_data().unwrap(),
            builder.reborrow().init_account_data(),
        );

        builder.set_cube(icons.cube);
        builder.set_color1(icons.color1);
        builder.set_color2(icons.color2);
        builder.set_glow_color(icons.glow_color);
        builder.reborrow().set_session(player.session_id_u64());
        builder.set_team_id(player.team_id());

        let should_send_roles = is_mod || !player.settings().hide_roles;

        if should_send_roles
            && let Some(role) = &*player.role()
            && role.is_special()
        {
            let mut sdata = builder.reborrow().init_special_data();

            if let Some(nc) = role.name_color.as_ref() {
                let mut buf = [0u8; 256];
                let mut writer = ByteWriter::new(&mut buf);
                nc.encode(&mut writer);
                sdata.set_name_color(writer.written());
            }

            let _ = sdata.set_roles(&*role.roles);
        }
    }

    pub(crate) fn encode_minimal_room_player(
        player: &ClientStateHandle,
        mut builder: data::minimal_room_player::Builder<'_>,
    ) {
        let icons = player.icons();

        builder.set_cube(icons.cube);
        builder.set_color1(icons.color1);
        builder.set_color2(icons.color2);
        builder.set_glow_color(icons.glow_color);

        Self::encode_account_data(
            player.account_data().unwrap(),
            builder.reborrow().init_account_data(),
        );
    }

    async fn send_room_data(&self, client: &ClientStateHandle, room: &Room) -> HandlerResult<()> {
        self.send_room_players_filtered(client, room, true, false, |_| true).await
    }

    async fn send_room_players_filtered(
        &self,
        client: &ClientStateHandle,
        room: &Room,
        full_room_check: bool,
        minimal: bool,
        filter: impl Fn(&ClientStateHandle) -> bool,
    ) -> HandlerResult<()> {
        let players = self.pick_players_to_send(client, room, filter).await;
        let total_player_count = room.player_count();

        let players_cap = if minimal && !full_room_check {
            players.iter().map(|_| 64).sum::<usize>()
        } else {
            players.iter().map(bytes_for_room_player).sum::<usize>()
        };

        let is_mod = client.can_moderate();

        let buf = if full_room_check {
            let team_count = room.team_count();
            let cap = 128 + room.name.len() + players_cap + 4 * team_count;

            data::encode_message_heap!(self, cap, msg => {
                let mut room_state = msg.reborrow().init_room_state();
                room_state.set_room_id(room.id);
                room_state.set_room_owner(room.owner());
                room_state.set_room_name(&room.name);
                room_state.set_passcode(room.passcode);
                room_state.set_player_count(total_player_count as u32);
                room_state.set_pinned_level(room.pinned_level().as_u64());

                room.settings.lock().encode(room_state.reborrow().init_settings());

                let mut players_ser = room_state.reborrow().init_players(players.len() as u32);

                for (i, player) in players.iter().enumerate() {
                    let mut player_ser = players_ser.reborrow().get(i as u32);
                    Self::encode_room_player(is_mod, player, player_ser.reborrow());
                }

                // encode teams
                if team_count > 0 {
                    room.with_teams(|count, teams| {
                        let mut teams_ser = room_state.reborrow().init_teams(count as u32);
                        for (i, team) in teams.enumerate() {
                            teams_ser.reborrow().set(i as u32, team.color);
                        }
                    });
                }
            })?
        } else if !minimal {
            let cap = 56 + players_cap;

            data::encode_message_heap!(self, cap, msg => {
                let mut room_players = msg.reborrow().init_room_players();

                let mut players_ser = room_players.reborrow().init_players(players.len() as u32);

                for (i, player) in players.iter().enumerate() {
                    let mut player_ser = players_ser.reborrow().get(i as u32);
                    Self::encode_room_player(is_mod, player, player_ser.reborrow());
                }
            })?
        } else {
            let cap = 48 + players_cap;

            data::encode_message_heap!(self, cap, msg => {
                let mut room_players = msg.reborrow().init_global_players();

                let mut players_ser = room_players.reborrow().init_players(players.len() as u32);

                for (i, player) in players.iter().enumerate() {
                    let mut player_ser = players_ser.reborrow().get(i as u32);
                    Self::encode_minimal_room_player(player, player_ser.reborrow());
                }
            })?
        };

        client.send_data_bufkind(buf);

        Ok(())
    }

    async fn pick_players_to_send(
        &self,
        client: &ClientStateHandle,
        room: &Room,
        filter: impl Fn(&ClientStateHandle) -> bool,
    ) -> Vec<ClientStateHandle> {
        const PLAYER_CAP: usize = 100;

        let player_count = if room.is_global() {
            room.player_count().min(PLAYER_CAP)
        } else {
            room.player_count()
        };

        let mut out = Vec::with_capacity(player_count + 2); // +2 to decrease the chance of reallocation
        let mut friend_ids = Vec::new();
        let account_id = client.account_id();

        // always push friends first
        {
            let friend_list = client.friend_list.lock();
            for friend in friend_list.iter() {
                if *friend != account_id
                    && let Some(friend) = self.find_client(*friend)
                    && friend.get_room_id().unwrap_or(0) == room.id
                    && filter(&friend)
                {
                    friend_ids.push(friend.account_id());
                    out.push(friend);

                    if out.len() == player_count {
                        break;
                    }
                }
            }
        }

        debug_assert!(out.len() <= player_count);

        let new_filter = |p: &ClientStateHandle| {
            let id = p.account_id();
            if id == account_id || friend_ids.contains(&id) || !filter(p) {
                return false;
            }

            // check user settings, if the user chose to be hidden then don't send them unless we are a moderator
            if p.settings().hide_in_menus && !client.can_moderate() {
                return false;
            }

            true
        };

        let begin = out.len();

        // put a bunch of dummy values into the vec, as `choose_multiple_fill` requires a mutable slice of initialized Arcs
        out.resize(player_count, client.clone());

        let written = room
            .with_players(|_, players| {
                players
                    .map(|x| x.1.handle.clone())
                    .filter(new_filter)
                    .choose_multiple_fill(&mut rand::rng(), &mut out[begin..])
            })
            .await;

        out.truncate(begin + written);

        out
    }

    pub async fn handle_check_room_state(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        if let Some(room) = client.get_room() {
            self.send_room_data(client, &room).await?;
        }

        Ok(())
    }

    pub async fn handle_request_room_players(
        &self,
        client: &ClientStateHandle,
        name_filter: &str,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        if let Some(room) = client.get_room() {
            self.send_room_players(client, &room, name_filter, false).await?;
        }

        Ok(())
    }

    pub async fn handle_request_global_player_list(
        &self,
        client: &ClientStateHandle,
        name_filter: &str,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let room = self.module::<RoomModule>().global_room();
        self.send_room_players(client, &room, name_filter, true).await
    }

    async fn send_room_players(
        &self,
        client: &ClientStateHandle,
        room: &Room,
        name_filter: &str,
        minimal: bool,
    ) -> HandlerResult<()> {
        if name_filter.is_empty() {
            self.send_room_players_filtered(client, room, false, minimal, |_| true).await?;
        } else {
            self.send_room_players_filtered(client, room, false, minimal, |p| {
                username_match(p.username(), name_filter)
            })
            .await?;
        }

        Ok(())
    }

    pub fn handle_request_room_list(
        &self,
        client: &ClientStateHandle,
        name_filter: &str,
        page: u32,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let filter = if name_filter.is_empty() { None } else { Some(name_filter) };
        let rooms = self.module::<RoomModule>();

        let (sorted, total) = rooms.get_top_rooms(page as usize * 100, 100, |r| {
            filter.is_none_or(|n| username_match(&r.name, n))
        });
        self.send_room_list(client, &sorted, page, total as u32)?;

        Ok(())
    }

    pub fn handle_assign_team(
        &self,
        client: &ClientStateHandle,
        mut player_id: i32,
        team_id: u16,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let room = get_custom_room(client)?;
        let is_owner = client.account_id() == room.owner();

        if player_id == 0 {
            player_id = client.account_id();
        } else {
            // only room owner can assign other players
            if !is_owner {
                return Ok(());
            }
        }

        if !is_owner && room.settings.lock().locked_teams {
            // disallow players moving freely between teams if locked teams is enabled
            return Ok(());
        }

        if !room.assign_team_to_player(team_id, player_id) {
            return self.send_warn(
                client,
                format!("failed to assign player {player_id} to team {team_id}"),
            );
        }

        // notify that player
        if let Some(player) = self.find_client(player_id) {
            let buf = data::encode_message!(self, 40, msg => {
                let mut changed = msg.reborrow().init_team_changed();
                changed.set_team_id(team_id);
            })?;

            player.set_team_id(team_id);
            player.send_data_bufkind(buf);
        }

        Ok(())
    }

    pub fn handle_create_team(&self, client: &ClientStateHandle, color: u32) -> HandlerResult<()> {
        must_auth(client)?;

        let room = get_custom_room_as_owner(client)?;

        let (success, team_count) = match room.create_team(color) {
            Ok(count) => (true, count),
            Err(e) => {
                debug!("team creation failed in room {}: {e}", room.id);
                (false, room.team_count())
            }
        };

        let buf = data::encode_message!(self, 40, msg => {
            let mut result = msg.init_team_creation_result();
            result.set_success(success);
            result.set_team_count(team_count as u16);
        })?;

        client.send_data_bufkind(buf);

        self.notify_teams_updated(&room)?;

        Ok(())
    }

    pub fn handle_delete_team(
        &self,
        client: &ClientStateHandle,
        team_id: u16,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let room = get_custom_room_as_owner(client)?;

        let Ok(players) = room.delete_team(team_id) else {
            return Ok(());
        };

        for player in players {
            player.handle.set_team_id(player.team_id);
            player.handle.send_data_bufkind(data::encode_message!(self, 48, msg => {
                let mut changed = msg.reborrow().init_team_changed();
                changed.set_team_id(player.team_id);
            })?);
        }

        self.notify_teams_updated(&room)?;

        Ok(())
    }

    pub fn handle_update_team(
        &self,
        client: &ClientStateHandle,
        team_id: u16,
        color: u32,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let room = client.get_room();

        if room.as_ref().is_none_or(|r| r.is_global() || r.owner() != client.account_id()) {
            // cannot do this in a global room or if not the room owner
            return Ok(());
        }

        let room = room.as_ref().unwrap();
        room.set_team_color(team_id, color);

        self.notify_teams_updated(room)?;

        Ok(())
    }

    pub fn handle_get_team_members(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        let room = get_custom_room(client)?;

        let buf = room.with_players_sync(|count, players| {
            let cap = 64 + 5 * count;

            data::encode_message_heap!(self, cap, msg => {
                let mut members = msg.init_team_members();
                members.reborrow().init_members(count as u32);
                members.reborrow().init_team_ids(count as u32);

                for (i, (_, player)) in players.enumerate() {
                    members.reborrow().get_members().unwrap().set(i as u32, player.handle.account_id());
                    members.reborrow().get_team_ids().unwrap().set(i as u32, player.team_id as u8);
                }
            })
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    pub async fn handle_room_owner_action(
        &self,
        client: &ClientStateHandle,
        r#type: data::RoomOwnerActionType,
        target: i32,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let room = get_custom_room_as_owner(client)?;

        match r#type {
            data::RoomOwnerActionType::BanUser => {
                // try to locate the user
                if let Some(target_arc) = self.find_client(target)
                    && can_kick_from_room(&target_arc)
                {
                    room.ban_player(target);
                    // just leave for them lol
                    self.handle_leave_room(&target_arc).await?;
                }
            }

            data::RoomOwnerActionType::KickUser => {
                if let Some(target_arc) = self.find_client(target)
                    && can_kick_from_room(&target_arc)
                {
                    self.handle_leave_room(&target_arc).await?;
                }
            }

            data::RoomOwnerActionType::CloseRoom => {
                self.close_room_by_id(room.id).await?;
            }
        }

        Ok(())
    }

    pub async fn close_room_by_id(&self, room_id: u32) -> HandlerResult<()> {
        let rooms = self.module::<RoomModule>();

        if let Some(users) = rooms.close_room(room_id, &self.game_server_manager).await {
            for user in users {
                self.send_room_data(&user, &rooms.global_room()).await?;
            }
        }

        Ok(())
    }

    pub async fn handle_invite_player(
        &self,
        client: &ClientStateHandle,
        player: i32,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let room = get_custom_room(client)?;

        if room.private_invites() && room.owner() != client.account_id() {
            return Ok(());
        }

        debug!("{} is creating invite for {} (room {})", client.account_id(), player, room.id);

        // if player is 0, create the invite token and send back to the same person
        if player == 0 {
            let token = room.create_invite_token();

            let buf = data::encode_message!(self, 56, msg => {
                let mut created = msg.init_invite_token_created();
                created.set_token(token.get());
            })?;

            client.send_data_bufkind(buf);
        } else if let Some(target) = self.find_client(player) {
            let token = room.create_invite_token();

            let buf = data::encode_message!(self, 104, msg => {
                let mut invited = msg.init_invited();
                invited.set_token(token.get());

                Self::encode_account_data(client.account_data().unwrap(), invited.init_invited_by());
            })?;

            target.send_data_bufkind(buf);
        };

        Ok(())
    }

    pub async fn handle_update_room_settings(
        &self,
        client: &ClientStateHandle,
        settings: RoomSettings,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let room = get_custom_room_as_owner(client)?;
        room.set_settings(settings);

        self.notify_settings_updated(&room)?;

        Ok(())
    }

    pub async fn handle_update_pinned_level(
        &self,
        client: &ClientStateHandle,
        id: u64,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let room = get_custom_room_as_owner(client)?;
        room.set_pinned_level(id);

        self.notify_pinned_level_updated(&room)?;

        Ok(())
    }

    fn notify_teams_updated(&self, room: &Room) -> HandlerResult<()> {
        let buf = room.with_teams(|team_count, teams| {
            let cap = 40 + 4 * team_count;

            data::encode_message_heap!(self, cap, msg => {
                let mut teams_ser = msg.reborrow().init_teams_updated().init_teams(team_count as u32);

                for (i, team) in teams.enumerate() {
                    teams_ser.reborrow().set(i as u32, team.color);
                }
            })
        })?;
        room.send_to_all_sync(buf);

        Ok(())
    }

    fn notify_settings_updated(&self, room: &Room) -> HandlerResult<()> {
        room.send_to_all_sync(data::encode_message!(self, 128, msg => {
            let mut ser = msg.reborrow().init_room_settings_updated();
            room.settings.lock().encode(ser.reborrow().init_settings());
        })?);

        Ok(())
    }

    pub fn notify_pinned_level_updated(&self, room: &Room) -> HandlerResult<()> {
        room.send_to_all_sync(data::encode_message!(self, 48, msg => {
            let mut ser = msg.reborrow().init_pinned_level_updated();
            ser.set_id(room.pinned_level().as_u64());
        })?);

        Ok(())
    }

    fn send_room_list(
        &self,
        client: &ClientStateHandle,
        rooms: &[Arc<Room>],
        page: u32,
        total_rooms: u32,
    ) -> HandlerResult<()> {
        let cap = 64
            + rooms
                .iter()
                .map(|x| {
                    72 + x.name.len()
                        + self
                            .find_client(x.owner())
                            .map_or(64, |owner| bytes_for_room_player(&owner))
                })
                .sum::<usize>();

        let is_mod = client.can_moderate();

        debug!("encoding {} rooms, cap: {}", rooms.len(), cap);

        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut room_list = msg.reborrow().init_room_list();
            room_list.set_page(page as u16);
            room_list.set_total(total_rooms);
            let mut enc_rooms = room_list.init_rooms(rooms.len() as u32);

            for (i, room) in rooms.iter().enumerate() {
                let mut room_ser = enc_rooms.reborrow().get(i as u32);
                room_ser.set_room_id(room.id);
                room_ser.set_room_name(&room.name);
                room_ser.set_player_count(room.player_count() as u32);
                room_ser.set_has_password(room.has_password());
                room_ser.set_original_owner_id(room.original_owner);
                room.settings.lock().encode(room_ser.reborrow().init_settings());

                if let Some(owner) = self.find_client(room.owner()) {
                    let mut owner_ser = room_ser.reborrow().init_room_owner();
                    Self::encode_room_player(is_mod, &owner, owner_ser.reborrow());
                }
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }
}

fn username_match(username: &str, filter: &str) -> bool {
    username
        .as_bytes()
        .windows(filter.len())
        .any(|window| window.eq_ignore_ascii_case(filter.as_bytes()))
}

fn bytes_for_room_player(client: &ClientStateHandle) -> usize {
    80 + if let Some(role) = &*client.role()
        && role.is_special()
    {
        40 + role.roles.len() + role.name_color.as_ref().map(|x| x.encoded_len()).unwrap_or(0)
    } else {
        0
    }
}

fn can_kick_from_room(client: &ClientStateHandle) -> bool {
    !client.authorized_mod()
}

#[allow(unused)]
fn get_room(c: &ClientStateHandle) -> HandlerResult<Arc<Room>> {
    match c.get_room() {
        Some(r) => Ok(r),
        _ => Err(HandlerError::Unauthorized),
    }
}

fn get_custom_room(c: &ClientStateHandle) -> HandlerResult<Arc<Room>> {
    match c.get_room() {
        Some(r) if !r.is_global() => Ok(r),
        _ => Err(HandlerError::NotInCustomRoom),
    }
}

fn get_custom_room_as_owner(c: &ClientStateHandle) -> HandlerResult<Arc<Room>> {
    match c.get_room() {
        Some(r) if !r.is_global() => {
            if r.owner() == c.account_id() {
                Ok(r)
            } else {
                Err(HandlerError::NotRoomOwner)
            }
        }

        _ => Err(HandlerError::NotInCustomRoom),
    }
}
