use server_shared::SessionId;

use super::{ConnectionHandler, util::*};

impl ConnectionHandler {
    pub async fn handle_join_session(
        &self,
        client: &ClientStateHandle,
        session_id: u64,
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
        self.handle_session_change(client, SessionId::from(prev_id), session_id).await?;

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
        self.handle_session_change(client, SessionId::from(prev_id), SessionId(0)).await?;

        Ok(())
    }

    // internal, called when the session ID changes to update player counts in rooms and stuff
    #[allow(clippy::await_holding_lock)]
    async fn handle_session_change(
        &self,
        client: &ClientStateHandle,
        prev_session: SessionId,
        new_session: SessionId,
    ) -> HandlerResult<()> {
        #[cfg(debug_assertions)]
        trace!(
            "[{}] session change: {} -> {}",
            client.account_id(),
            prev_session.as_u64(),
            new_session.as_u64()
        );

        if !prev_session.is_zero() {
            debug_assert!(self.player_counts.contains_key(&prev_session.as_u64()));

            self.player_counts.remove_if_mut(&prev_session.as_u64(), |_, count| {
                *count -= 1;
                *count == 0
            });
        }

        if !new_session.is_zero() {
            let mut ent = self.player_counts.entry(new_session.as_u64()).or_insert(0);
            *ent += 1;
        }

        // if this is a follower room and the owner changed the level, warp all other players
        let room = client.lock_room(); // this is held across .await but it's fine because it's local to the user

        let do_warp =
            room.as_ref().is_some_and(|x| x.is_follower() && x.owner() == client.account_id());

        if do_warp {
            room.as_ref()
                .unwrap()
                .with_players(|_, players| {
                    let buf = data::encode_message!(self, 64, msg => {
                        let mut warp = msg.reborrow().init_warp_player();
                        warp.set_session(new_session.as_u64());
                    })
                    .expect("failed to encode warp message");

                    for (_, p) in players {
                        p.handle.send_data_bufkind(buf.clone_into_small());
                    }
                })
                .await;
        }

        Ok(())
    }
}
