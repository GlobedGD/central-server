use nohash_hasher::IntMap;
use rustc_hash::FxHashSet;
use server_shared::{
    UserSettings,
    data::PlayerIconData,
    events::{EventOptions, OwnedEvent},
};

use crate::{
    credits::CreditsModule,
    rooms::{Room, RoomModule},
    users::{LinkedDiscordAccount, UsersModule},
};

use super::{ConnectionHandler, util::*};

pub enum HandleEventError {
    RateLimit,
    UnscopedGlobalEvent,
}

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

    pub fn handle_update_user_settings(
        &self,
        client: &ClientStateHandle,
        settings: UserSettings,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        client.set_settings(settings);

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
            if let Some(ent) = self.all_levels.get(&sess)
                && !ent.is_hidden
                && ent.player_count > 0
            {
                let _ = out_vals.push((sess, ent.player_count as u16));
            }
        }

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

    pub async fn handle_request_level_list(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        let Some(room) = client.get_room() else {
            return Ok(());
        };

        let levels = self.gather_levels_in_room(&room);

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
            let buf = data::encode_message!(self, 64, msg => {
                let mut cred = msg.init_credits();
                cred.set_unavailable(true);
            })?;

            client.send_data_bufkind(buf);

            return Ok(());
        };

        let buf = data::encode_message_dyn!(self, msg => {
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

    fn gather_levels_in_room(&self, room: &Room) -> IntMap<u64, u16> {
        room.with_players(|_, iter| {
            let mut map = IntMap::default();

            for (_, p) in iter {
                let session = p.handle.session_id_u64();

                if self.all_levels.get(&session).is_some_and(|e| e.is_hidden) {
                    continue;
                }

                *map.entry(session).or_insert(0u16) += 1;
            }

            map
        })
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

        let _ = id;
        let _ = accept;

        Ok(())
    }

    pub fn handle_discord_get_oauth(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        #[cfg(feature = "discord")]
        {
            use crate::discord::DiscordModule;

            let discord = self.module::<DiscordModule>();
            let url = discord.begin_oauth_flow(client, client.account_id());

            info!("[{} ({})] generated oauth url: {url}", client.username(), client.account_id());

            let buf = data::encode_message_dyn!(self, msg => {
                let mut oauth = msg.init_discord_oauth_url();
                oauth.set_url(&url);
            })?;

            client.send_data_bufkind(buf);
        }

        Ok(())
    }

    pub async fn handle_discord_unlink(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        #[cfg(feature = "discord")]
        {
            let users = self.module::<UsersModule>();

            let res = users.try_unlink_discord(client.account_id()).await;
            if res.is_ok() {
                client.set_discord_linked(false);
            }

            let buf = data::encode_message_dyn!(self, msg => {
                let mut r = msg.init_discord_unlink_result();
                r.set_success(res.is_ok());
                if let Err(e) = res {
                    r.set_error(e.to_string());
                }
            })?;

            client.send_data_bufkind(buf);
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

    pub async fn handle_notice_reply(
        &self,
        client: &ClientStateHandle,
        target_user: i32,
        message: &str,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let res = self.notice_reply_inner(client, target_user, message).await;

        let buf = data::encode_message!(self, 512, msg => {
            let mut m = msg.init_notice_reply_result();
            m.set_success(res.is_ok());
            if let Err(e) = res {
                m.set_error(e);
            }
        });
        client.send_data_bufkind(buf?);

        Ok(())
    }

    async fn notice_reply_inner(
        &self,
        client: &ClientStateHandle,
        target_user: i32,
        message: &str,
    ) -> Result<(), &'static str> {
        let Some(target) = self.find_client(target_user) else {
            debug!("{} could not reply to {target_user}, target not found", client.account_id());
            return Err("user went offline");
        };

        if target.take_awaiting_notice_reply(client.account_id()) {
            let users = self.module::<UsersModule>();
            let _ = users
                .log_notice_reply(client.account_id(), client.username(), target_user, message)
                .await;

            target.send_data_bufkind(
                self.make_notice_buf(Some(client), message, false, true)
                    .map_err(|_| "Failed to encode notice reply")?,
            );

            return Ok(());
        }

        debug!("{} could not reply to {target_user}, reply likely expired", client.account_id());
        Err("reply expired or user went offline")
    }

    pub async fn handle_get_user_state(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        // encode as anonymized, don't show the issuer to the user
        let buf = data::encode_message_dyn!(self, msg => {
            let mut state = msg.init_user_state();
            if let Some(mute) = &*client.active_mute.lock() {
                mute.encode_anonymized(&mut state.reborrow().init_active_mute());
            }
            if let Some(room_ban) = &*client.active_room_ban.lock() {
                room_ban.encode_anonymized(&mut state.init_active_room_ban());
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    pub async fn handle_fetch_user(
        &self,
        client: &ClientStateHandle,
        account_id: i32,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let users = self.module::<UsersModule>();
        let user = users.get_user(account_id).await.ok().flatten();

        let buf = data::encode_message_dyn!(self, msg => {
            let mut out = msg.init_fetch_user_response();

            if let Some(u) = user {
                out.set_found(true);
                out.set_account_id(u.account_id);
                let role_ids = users.role_str_to_ids(u.roles.as_deref().unwrap_or_default());
                let _ = out.set_roles(&role_ids[..]);
            } else {
                out.set_found(false);
            }
        })?;
        client.send_data_bufkind(buf);
        Ok(())
    }

    pub async fn handle_events(
        &self,
        client: &ClientStateHandle,
        events: Vec<OwnedEvent>,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let rooms = self.module::<RoomModule>();
        let room = client.get_room().unwrap_or_else(|| rooms.global_room());

        for event in events {
            match self.handle_event(client, event, &room).await {
                Ok(()) => {}

                Err(HandleEventError::RateLimit) => {
                    warn!(
                        "[{} @ {}] event rate limit exceeded, disconnecting client",
                        client.account_id(),
                        client.address
                    );

                    client.disconnect("Event rate limit exceeded");
                    return Ok(());
                }

                Err(HandleEventError::UnscopedGlobalEvent) => {
                    warn!(
                        "[{} @ {}] rejecting unscoped event in global room!",
                        client.account_id(),
                        client.address
                    );

                    self.send_warn(client, "rejecting unscoped event in the global room")?;
                }
            }
        }

        Ok(())
    }

    async fn handle_event(
        &self,
        client: &ClientStateHandle,
        event: OwnedEvent,
        room: &Room,
    ) -> Result<(), HandleEventError> {
        debug!(
            "[{} @ {}] handling event {} ({} bytes)",
            client.account_id(),
            client.address,
            event.id,
            event.data.len()
        );

        let out_event = OwnedEvent {
            id: event.id,
            data: event.data,
            options: EventOptions {
                target_players: Vec::new(),
                sent_by_player: client.account_id_nz(),
                send_back: false,
                ..event.options
            },
        };

        // calculate how many targets in total there are, to check the rate limits
        let targets = if event.options.target_players.is_empty() {
            if room.is_global() {
                // disallow sending events to everybody when in the global room
                return Err(HandleEventError::UnscopedGlobalEvent);
            }

            room.player_count()
        } else {
            event.options.target_players.len()
        };

        if !client.try_event(targets, out_event.data.len(), out_event.options.reliable) {
            return Err(HandleEventError::RateLimit);
        }

        let targets: Vec<_> = if event.options.target_players.is_empty() {
            room.get_players_filtered(|p| {
                if event.options.send_back {
                    true
                } else {
                    p.handle.account_id() != client.account_id()
                }
            })
            .into_iter()
            .map(|p| p.handle)
            .collect()
        } else {
            event
                .options
                .target_players
                .iter()
                .filter_map(|id| self.find_client(*id))
                .filter(|target| target.is_in_room(room))
                .collect()
        };

        debug!("dispatching event to {} targets", targets.len());

        for target in targets {
            let account_id = target.account_id();
            if !self.event_worker.enqueue(out_event.clone(), target).await {
                debug!("could not enqueue event for {}", account_id);
            }
        }

        Ok(())
    }
}
