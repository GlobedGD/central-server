use std::{num::NonZeroI64, sync::Arc};

use rand::seq::IteratorRandom;

use crate::rooms::{Room, RoomCreationError, RoomModule, RoomSettings};

use super::{ConnectionHandler, util::*};

const BYTES_PER_PLAYER: usize = 72; // TODO (high)

impl ConnectionHandler {
    pub async fn handle_create_room(
        &self,
        client: &ClientStateHandle,
        name: &str,
        passcode: u32,
        settings: RoomSettings,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        if let Some(p) = client.active_room_ban.lock().as_ref() {
            // user is room banned, don't allow creating rooms
            return self.send_room_banned(client, &p.reason, p.expires_at);
        }

        let rooms = self.module::<RoomModule>();
        let server_id = settings.server_id;

        // check if the requested server is valid
        if !self.game_server_manager.has_server(server_id) {
            return self
                .send_room_create_failed(client, data::RoomCreateFailedReason::InvalidServer);
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
        match self.game_server_manager.notify_room_created(server_id, new_room.id, passcode).await {
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
        let buf = data::encode_message!(self, 40, msg => {
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

    fn encode_room_player(player: &ClientStateHandle, mut builder: data::room_player::Builder<'_>) {
        builder.set_cube(player.icons().cube);
        builder.reborrow().set_session(player.session_id());

        let mut accdata = builder.reborrow().init_account_data();
        let account = player.account_data().expect("client must have account data");
        accdata.set_account_id(account.account_id);
        accdata.set_user_id(account.user_id);
        accdata.set_username(&account.username);

        if let Some(room) = player.lock_room().as_ref() {
            builder.set_team_id(room.team_id());
        }
    }

    async fn send_room_data(&self, client: &ClientStateHandle, room: &Room) -> HandlerResult<()> {
        let players = self.pick_players_to_send(client, room).await;

        // TODO (high): that number is uncertain
        let cap = 128 + BYTES_PER_PLAYER * players.len();

        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut room_state = msg.reborrow().init_room_state();
            room_state.set_room_id(room.id);
            room_state.set_room_owner(room.owner);
            room_state.set_room_name(&room.name);
            room.settings.encode(room_state.reborrow().init_settings());

            let mut players_ser = room_state.init_players(players.len() as u32);

            for (i, player) in players.iter().enumerate() {
                let mut player_ser = players_ser.reborrow().get(i as u32);
                Self::encode_room_player(player, player_ser.reborrow());
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    async fn pick_players_to_send(
        &self,
        client: &ClientStateHandle,
        room: &Room,
    ) -> Vec<ClientStateHandle> {
        const PLAYER_CAP: usize = 100;

        let player_count = if room.is_global() {
            room.player_count().min(PLAYER_CAP)
        } else {
            room.player_count()
        };

        let mut out = Vec::with_capacity(player_count + 2); // +2 to decrease the chance of reallocation

        // always push friends first
        {
            let friend_list = client.friend_list.lock();
            for friend in friend_list.iter() {
                if let Some(friend) = self.find_client(*friend)
                    && let Some(room_id) = friend.get_room_id()
                    && room_id == room.id
                {
                    out.push(friend);
                }

                if out.len() == player_count {
                    break;
                }
            }
        }

        debug_assert!(out.len() <= player_count);

        let begin = out.len();

        // put a bunch of dummy values into the vec, as `choose_multiple_fill` requires a mutable slice of initialized Arcs
        out.resize(player_count, client.clone());
        let account_id = client.account_id();

        let written = room
            .with_players(|_, players| {
                players
                    .map(|x| x.1.handle.clone())
                    .filter(|x| x.account_id() != account_id)
                    .choose_multiple_fill(&mut rand::rng(), &mut out[begin..])
            })
            .await;

        out.truncate(begin + written);

        out
    }

    #[allow(clippy::await_holding_lock)]
    pub async fn handle_check_room_state(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        if let Some(room) = &*client.lock_room() {
            self.send_room_data(client, room).await?;
        }

        Ok(())
    }

    pub fn handle_request_room_list(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        let rooms = self.module::<RoomModule>();

        // TODO: filtering
        // TODO: pagination

        let sorted = rooms.get_top_rooms(0, 100);
        self.send_room_list(client, &sorted)?;

        Ok(())
    }

    pub fn handle_assign_team(
        &self,
        client: &ClientStateHandle,
        mut player_id: i32,
        team_id: u16,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let room = client.lock_room();

        if room.as_ref().is_none_or(|r| r.is_global()) {
            // cannot do this in a global room
            return Ok(());
        }

        let room = room.as_ref().unwrap();

        if player_id == 0 {
            player_id = client.account_id();
        } else {
            // only room owner can assign other players
            if client.account_id() != room.owner {
                return Ok(());
            }
        }

        if !room.assign_team_to_player(team_id, player_id) {
            return self.send_warn(
                client,
                format!("failed to assign player {player_id} to team {team_id}"),
            );
        }

        // notify that player
        if let Some(player) = self.find_client(player_id) {
            let buf = data::encode_message!(self, 48, msg => {
                let mut changed = msg.reborrow().init_team_changed();
                changed.set_team_id(team_id);
            })?;

            player.send_data_bufkind(buf);
        }

        Ok(())
    }

    pub fn handle_create_team(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        let room = client.lock_room();

        if room.as_ref().is_none_or(|r| r.is_global() || r.owner != client.account_id()) {
            // cannot do this in a global room or if not the room owner
            return Ok(());
        }

        let room = room.as_ref().unwrap();

        let (success, team_count) = match room.create_team() {
            Ok(count) => (true, count),
            Err(e) => {
                debug!("team creation failed in room {}: {e}", room.id);
                (false, room.team_count())
            }
        };

        let buf = data::encode_message!(self, 56, msg => {
            let mut result = msg.init_team_creation_result();
            result.set_success(success);
            result.set_team_count(team_count as u16);
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    pub fn handle_delete_team(
        &self,
        client: &ClientStateHandle,
        team_id: u16,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let room = client.lock_room();

        if room.as_ref().is_none_or(|r| r.is_global() || r.owner != client.account_id()) {
            // cannot do this in a global room or if not the room owner
            return Ok(());
        }

        let room = room.as_ref().unwrap();

        let Ok(players) = room.delete_team(team_id) else {
            return Ok(());
        };

        for player in players {
            player.handle.send_data_bufkind(data::encode_message!(self, 48, msg => {
                let mut changed = msg.reborrow().init_team_changed();
                changed.set_team_id(player.team_id);
            })?);
        }

        Ok(())
    }

    pub fn handle_get_team_members(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        let room = client.lock_room();

        if room.as_ref().is_none_or(|r| r.is_global()) {
            // cannot do this in a global room
            return Ok(());
        }

        let room = room.as_ref().unwrap();
        let team_id = room.team_id();

        let Ok(players) = room.get_players_on_team(team_id) else {
            return self
                .send_warn(client, format!("failed to find team {team_id} in the current room"));
        };

        let cap = 48 + BYTES_PER_PLAYER * players.len();
        let buf = data::encode_message_heap!(self, cap, msg => {
            let members = msg.init_team_members();
            let mut players_ser = members.init_members(players.len() as u32);

            for (i, player) in players.iter().enumerate() {
                players_ser.reborrow().set(i as u32, player.handle.account_id());
                // let mut player_ser = players_ser.reborrow().get(i as u32);
                // Self::encode_room_player(&player.handle, player_ser.reborrow());
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    fn send_room_list(&self, client: &ClientStateHandle, rooms: &[Arc<Room>]) -> HandlerResult<()> {
        const BYTES_PER_ROOM: usize = 112; // TODO (high)

        // TODO:
        let cap = 48 + BYTES_PER_ROOM * rooms.len();

        debug!("encoding {} rooms, cap: {}", rooms.len(), cap);

        let buf = data::encode_message_heap!(self, cap, msg => {
            let room_list = msg.reborrow().init_room_list();
            let mut enc_rooms = room_list.init_rooms(rooms.len() as u32);

            for (i, room) in rooms.iter().enumerate() {
                let mut room_ser = enc_rooms.reborrow().get(i as u32);
                room_ser.set_room_id(room.id);
                room_ser.set_room_name(&room.name);
                room_ser.set_player_count(room.player_count() as u32);
                room_ser.set_has_password(room.has_password());
                room.settings.encode(room_ser.reborrow().init_settings());

                if let Some(owner) = self.find_client(room.owner) {
                    let mut owner_ser = room_ser.reborrow().init_room_owner();
                    Self::encode_room_player(&owner, owner_ser.reborrow());
                }
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }
}
