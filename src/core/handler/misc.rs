use nohash_hasher::IntMap;
use rustc_hash::FxHashSet;
use server_shared::data::PlayerIconData;

use crate::rooms::Room;

use super::{ConnectionHandler, util::*};

impl ConnectionHandler {
    pub fn handle_update_own_data(
        &self,
        client: &ClientStateHandle,
        icons: Option<PlayerIconData>,
        friends: Option<FxHashSet<i32>>,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        if let Some(icons) = icons {
            client.set_icons(icons);
        };

        if let Some(friends) = friends {
            client.set_friends(friends);
        };

        Ok(())
    }

    pub fn handle_request_player_counts(
        &self,
        client: &ClientStateHandle,
        sessions: &[u64],
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let mut out_vals = heapless::Vec::<(u64, u16), 128>::new();
        debug_assert!(sessions.len() <= out_vals.capacity());

        for &sess in sessions {
            if let Some(count) = self.player_counts.get(&sess) {
                let _ = out_vals.push((sess, *count as u16));
                // TODO: maybe do a zero optimization?
            }
        }

        // TODO: benchmark size properly
        let cap = 40 + out_vals.len() * 12;

        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut player_counts = msg.reborrow().init_player_counts();

            let mut level_ids = player_counts.reborrow().init_level_ids(out_vals.len() as u32);
            for (n, (level_id, _)) in out_vals.iter().enumerate() {
                level_ids.set(n as u32, *level_id);
            }

            let mut counts = player_counts.reborrow().init_counts(out_vals.len() as u32);
            for (n, (_, count)) in out_vals.iter().enumerate() {
                counts.set(n as u32, *count);
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    #[allow(clippy::await_holding_lock)]
    pub async fn handle_request_level_list(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        let room_lock = client.lock_room();

        let Some(room) = &*room_lock else {
            return Ok(());
        };

        let levels = self.gather_levels_in_room(room).await;

        let cap = 40 + levels.len() * 12;
        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut list = msg.reborrow().init_level_list();

            let mut level_ids = list.reborrow().init_level_ids(levels.len() as u32);
            for (n, (level_id, _)) in levels.iter().enumerate() {
                level_ids.set(n as u32, *level_id);
            }

            let mut counts = list.reborrow().init_player_counts(levels.len() as u32);
            for (n, (_, count)) in levels.iter().enumerate() {
                counts.set(n as u32, *count);
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    async fn gather_levels_in_room(&self, room: &Room) -> IntMap<u64, u16> {
        room.with_players(|_, iter| {
            let mut map = IntMap::default();

            for (_, p) in iter {
                let session = p.handle.session_id();

                *map.entry(session).or_insert(0u16) += 1;
            }

            map
        })
        .await
    }
}
