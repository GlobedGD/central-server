use nohash_hasher::IntMap;
use rustc_hash::FxHashSet;
use server_shared::data::PlayerIconData;

use crate::{
    credits::CreditsModule,
    rooms::Room,
    users::{LinkedDiscordAccount, UsersModule},
};

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
        let cap = 56 + out_vals.len() * 12;

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

        let cap = 56 + levels.len() * 12;
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

    pub fn handle_fetch_credits(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        let credits_arc = self.module::<CreditsModule>().get_credits();

        let Some(credits) = credits_arc.as_ref() else {
            let buf = data::encode_message_heap!(self, 56, msg => {
                let mut cred = msg.init_credits();
                cred.set_unavailable(true);
            })?;

            client.send_data_bufkind(buf);

            return Ok(());
        };

        let cap =
            56 + credits.iter().map(|c| c.name.len() + 24 + c.users.len() * 96).sum::<usize>();

        let buf = data::encode_message_heap!(self, cap, msg => {
            let cred = msg.init_credits();

            let mut cats = cred.init_categories(credits.len() as u32);

            for (i, cat) in credits.iter().enumerate() {
                let mut out_cat = cats.reborrow().get(i as u32);
                out_cat.set_name(&cat.name);

                let mut users = out_cat.init_users(cat.users.len() as u32);
                for (j, user) in cat.users.iter().enumerate() {
                    let mut out_user = users.reborrow().get(j as u32);
                    out_user.set_account_id(user.account_id);
                    out_user.set_user_id(user.user_id);
                    out_user.set_username(&user.username);
                    out_user.set_display_name(&user.display_name);
                    out_user.set_cube(user.cube);
                    out_user.set_color1(user.color1);
                    out_user.set_color2(user.color2);
                    out_user.set_glow_color(user.glow_color);
                }
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

    pub async fn handle_get_discord_link_state(
        &self,
        client: &ClientStateHandle,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let users = self.module::<UsersModule>();

        let user = match users.get_linked_discord(client.account_id()).await {
            Ok(u) => u,
            Err(e) => {
                warn!("Failed to fetch linked discord ID: {e}");
                return self.send_discord_link_state(client, LinkedDiscordAccount::default());
            }
        };

        self.send_discord_link_state(client, user.unwrap_or_default())
    }

    fn send_discord_link_state(
        &self,
        client: &ClientStateHandle,
        account: LinkedDiscordAccount,
    ) -> HandlerResult<()> {
        let buf = data::encode_message!(self, 512, msg => {
            let mut state = msg.init_discord_link_state();
            state.set_id(account.id);
            state.set_username(account.username);
            state.set_avatar_url(account.avatar_url);
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    pub fn handle_set_discord_pairing_state(
        &self,
        client: &ClientStateHandle,
        state: bool,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        client.set_discord_pairing(state);

        Ok(())
    }

    pub fn handle_discord_link_confirm(
        &self,
        client: &ClientStateHandle,
        id: u64,
        accept: bool,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        #[cfg(feature = "discord")]
        {
            use crate::discord::DiscordModule;

            let discord = self.module::<DiscordModule>();
            discord.finish_link_attempt(client.account_id(), id, accept);
        }

        Ok(())
    }

    // Used in the discord module
    pub fn send_discord_link_attempt(
        &self,
        client: &ClientStateHandle,
        id: u64,
        username: &str,
        avatar_url: &str,
    ) -> HandlerResult<()> {
        let buf = data::encode_message!(self, 512, msg => {
            let mut att = msg.init_discord_link_attempt();
            att.set_id(id);
            att.set_username(username);
            att.set_avatar_url(avatar_url);
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }
}
