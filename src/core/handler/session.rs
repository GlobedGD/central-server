use server_shared::SessionId;

use crate::{core::handler::LevelEntry, users::UsersModule};

use super::{ConnectionHandler, util::*};

impl ConnectionHandler {
    pub async fn handle_join_session(
        &self,
        client: &ClientStateHandle,
        session_id: u64,
        author_id: i32,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let session_id = SessionId::from(session_id);

        // do some validation

        if client.get_room_id().is_none_or(|x| x != session_id.room_id()) {
            return self.on_join_failed(client, data::JoinSessionFailedReason::InvalidRoom);
        }

        if !self.game_server_manager.has_server(session_id.server_id()) {
            return self.on_join_failed(client, data::JoinSessionFailedReason::InvalidServer);
        }

        let prev_id = client.set_session_id(session_id.as_u64());
        self.handle_session_change(client, SessionId::from(prev_id), session_id, Some(author_id))
            .await?;

        Ok(())
    }

    fn on_join_failed(
        &self,
        client: &ClientStateHandle,
        reason: data::JoinSessionFailedReason,
    ) -> HandlerResult<()> {
        let buf = data::encode_message!(self, 40, msg => {
            let mut join_failed = msg.reborrow().init_join_failed();
            join_failed.set_reason(reason);
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }

    pub async fn handle_leave_session(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        let prev_id = client.set_session_id(0);
        self.handle_session_change(client, SessionId::from(prev_id), SessionId(0), None).await?;

        Ok(())
    }

    // internal, called when the session ID changes to update player counts in rooms and stuff
    #[allow(clippy::await_holding_lock)]
    async fn handle_session_change(
        &self,
        client: &ClientStateHandle,
        prev_session: SessionId,
        new_session: SessionId,
        new_author: Option<i32>,
    ) -> HandlerResult<()> {
        #[cfg(debug_assertions)]
        tracing::trace!(
            "[{}] session change: {} -> {}",
            client.account_id(),
            prev_session.as_u64(),
            new_session.as_u64()
        );

        let users = self.module::<UsersModule>();

        if !prev_session.is_zero() {
            debug_assert!(self.all_levels.contains_key(&prev_session.as_u64()));

            self.all_levels.remove_if_mut(&prev_session.as_u64(), |_, entry| {
                entry.player_count -= 1;
                entry.player_count == 0
            });
        }

        if !new_session.is_zero() {
            let is_blacklisted = users.is_level_blacklisted(new_session.level_id())
                || new_author.is_some_and(|x| users.is_author_blacklisted(x));

            let mut ent = self.all_levels.entry(new_session.as_u64()).or_insert(LevelEntry {
                player_count: 0,
                is_hidden: is_blacklisted,
            });
            ent.player_count += 1;

            let users = self.module::<UsersModule>();
            let can_use_qc = client.active_mute.lock().is_none();
            let can_use_voice =
                can_use_qc && (!users.vc_requires_discord || client.is_discord_linked());

            // notify the appropriate game server

            if let Err(e) = self
                .game_server_manager
                .notify_user_data(
                    new_session.server_id(),
                    client.account_id(),
                    can_use_qc,
                    can_use_voice,
                )
                .await
            {
                warn!("Failed to send NotifyUserData to game server: {e}");
            }
        }

        // if this is a follower room and the owner changed the level, warp all other players

        let room = client.lock_room(); // this is held across .await but it's fine because it's local to the user
        let Some(room) = room.as_ref() else {
            return Ok(());
        };

        let is_owner = room.owner() == client.account_id();
        let do_warp = is_owner && room.is_follower();
        let do_update_pinned = is_owner && !room.settings.lock().manual_pinning;

        if do_warp {
            let buf = data::encode_message!(self, 64, msg => {
                let mut warp = msg.reborrow().init_room_warp();
                warp.set_session(new_session.as_u64());
            })?;

            room.send_to_all_sync(buf);
        }

        if do_update_pinned {
            room.set_pinned_level(new_session);
            self.notify_pinned_level_updated(room)?;
        }

        Ok(())
    }
}
