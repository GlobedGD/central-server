use std::{
    net::SocketAddr,
    str::FromStr,
    sync::{Arc, OnceLock},
    time::Duration,
};

use qunet::server::{
    Server as QunetServer, ServerHandle as QunetServerHandle, WeakServerHandle,
    app_handler::{AppHandler, AppResult, MsgData},
    client::ClientState,
};
use state::TypeMap;
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::{
    auth::AuthModule,
    core::{
        client_data::{ClientAccountData, ClientData},
        data::{
            self, DataDecodeError, EncodeMessageError, decode_message_match, encode_message_heap,
            encode_message_unsafe,
        },
        module::ServerModule,
    },
    rooms::{Room, RoomModule},
};

#[derive(Default)]
pub struct ConnectionHandler {
    modules: TypeMap![Send + Sync],
    // we use a weak handle here to avoid ref cycles, which will make it impossible to drop the server
    server: OnceLock<WeakServerHandle<Self>>,
}

pub type ClientStateHandle = Arc<ClientState<ConnectionHandler>>;

enum LoginKind<'a> {
    UserToken(i32, &'a str),
    Argon(i32, &'a str),
    Plain(ClientAccountData),
}

#[derive(Debug, Error)]
enum HandlerError {
    #[error("failed to encode message: {0}")]
    Encoder(#[from] EncodeMessageError),
    #[error("cannot handle this message while unauthorized")]
    Unauthorized,
}

type HandlerResult<T> = Result<T, HandlerError>;

impl ConnectionHandler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_module<T: ServerModule>(&self, module: T) {
        self.modules.set(module);
    }

    /// Get a module by type. Panics if the module is not found.
    pub fn module<T: ServerModule>(&self) -> &T {
        self.modules.get()
    }

    /// Get a module by type, returning `None` if the module is not found.
    pub fn opt_module<T: ServerModule>(&self) -> Option<&T> {
        self.modules.try_get()
    }

    pub fn freeze(&mut self) {
        self.modules.freeze();
    }

    /// Obtain a reference to the server. This must not be called before the server is launched and `on_launch` is called,
    /// otherwise it causes undefined behavior in Release.
    fn server(&self) -> QunetServerHandle<Self> {
        self.server
            .get()
            .expect("Server not initialized yet")
            .upgrade()
            .expect("Server has shut down")
    }

    async fn handle_login_attempt(
        &self,
        client: &Arc<ClientState<Self>>,
        kind: LoginKind<'_>,
    ) -> HandlerResult<()> {
        let auth = self.module::<AuthModule>();

        if client.data().authorized() {
            // if the client is already authorized, ignore the login attempt
            debug!("[{}] ignoring repeated login attempt", client.address);
            return Ok(());
        }

        // TODO: clean this up, delegate stuff here to AuthModule
        match kind {
            LoginKind::Plain(data) => {
                if auth.verification_enabled() {
                    // if verification is enabled, plain login is not allowed
                    let buf = encode_message_unsafe!(self, 128, msg => {
                        let mut login_req = msg.reborrow().init_login_required();
                        login_req.set_argon_url(auth.argon_url().unwrap());
                    })?;

                    client.send_data_bufkind(buf);
                } else {
                    // otherwise, perform no verification
                    self.on_login_success(client, data).await?;
                }
            }

            LoginKind::UserToken(account_id, token) => {
                let token_data = match auth.validate_user_token(token) {
                    Ok(d) => d,
                    Err(e) => {
                        warn!(
                            "[{} @ {}] failed to validate user token: {}",
                            account_id, client.address, e
                        );

                        self.on_login_failed(client, data::LoginFailedReason::InvalidUserToken)
                            .await?;

                        return Ok(());
                    }
                };

                if token_data.account_id != account_id {
                    warn!(
                        "[{} @ {}] user token validation failed: account ID mismatch",
                        account_id, client.address
                    );

                    self.on_login_failed(client, data::LoginFailedReason::InvalidUserToken).await?;

                    return Ok(());
                }

                self.on_login_success(
                    client,
                    ClientAccountData {
                        account_id,
                        user_id: token_data.user_id,
                        username: token_data.username,
                    },
                )
                .await?;
            }

            LoginKind::Argon(account_id, token) => {
                if let Some(argon) = auth.argon_client() {
                    let handle = match argon.validate(account_id, token) {
                        Ok(handle) => handle,
                        Err(e) => {
                            warn!(
                                "[{} @ {}] failed to request token validation: {}",
                                account_id, client.address, e
                            );
                            self.on_login_failed(client, data::LoginFailedReason::ArgonUnreachable)
                                .await?;
                            return Ok(());
                        }
                    };

                    let response = match handle.wait().await {
                        Ok(resp) => resp,
                        Err(_) => {
                            warn!(
                                "[{} @ {}] token validation attempt was dropped",
                                account_id, client.address
                            );

                            self.on_login_failed(
                                client,
                                data::LoginFailedReason::ArgonInternalError,
                            )
                            .await?;

                            return Ok(());
                        }
                    };

                    match response.into_inner() {
                        Ok(data) => {
                            self.on_login_success(client, data).await?;
                        }

                        Err(err) => {
                            debug!(
                                "[{} @ {}] token validation failed: {}",
                                account_id, client.address, err
                            );

                            self.on_login_failed(
                                client,
                                data::LoginFailedReason::InvalidArgonToken,
                            )
                            .await?;
                        }
                    }
                } else {
                    self.on_login_failed(client, data::LoginFailedReason::ArgonNotSupported)
                        .await?;
                }
            }
        }

        Ok(())
    }

    async fn on_login_success(
        &self,
        client: &ClientStateHandle,
        data: ClientAccountData,
    ) -> HandlerResult<()> {
        // refresh the user's user token (or generate a new one)
        let auth = self.module::<AuthModule>();
        let rooms = self.module::<RoomModule>();

        let token = auth.generate_user_token(data.account_id, data.user_id, data.username.clone());

        client.set_account_data(data);

        // put the user in the global room
        rooms.join_room(client, rooms.global_room());

        // send login success message

        let buf = encode_message_unsafe!(self, 128, msg => {
            let mut login_ok = msg.reborrow().init_login_ok();
            login_ok.set_new_token(&token);
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    #[inline]
    async fn on_login_failed(
        &self,
        client: &ClientState<Self>,
        reason: data::LoginFailedReason,
    ) -> HandlerResult<()> {
        let buf = encode_message_unsafe!(self, 128, msg => {
            let mut login_failed = msg.reborrow().init_login_failed();
            login_failed.set_reason(reason);
        })?;

        client.send_data_bufkind(buf);
        Ok(())
    }

    async fn handle_create_room(
        &self,
        client: &ClientStateHandle,
        name: &str,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let rooms = self.module::<RoomModule>();

        match rooms.create_room_and_join(name, client) {
            Ok(new_room) => {
                self.send_room_data(client, &new_room).await?;
            }

            // TODO: send error to the user
            Err(e) => warn!("failed to create room: {e}"),
        }

        Ok(())
    }

    async fn handle_join_room(&self, client: &ClientStateHandle, id: u32) -> HandlerResult<()> {
        must_auth(client)?;

        let rooms = self.module::<RoomModule>();
        let new_room = rooms.join_room_by_id(client, id);

        self.send_room_data(client, &new_room).await
    }

    async fn handle_leave_room(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        // Leaving a room is the same as joining the global room
        self.handle_join_room(client, 0).await
    }

    async fn send_room_data(&self, client: &ClientStateHandle, room: &Room) -> HandlerResult<()> {
        let player_count = room.player_count();

        // choose appropriate buffer size based on player count
        let cap = if player_count <= 25 {
            1500
        } else if player_count <= 65 {
            4096
        } else {
            65536
        };

        const PLAYER_CAP: usize = 250;

        let buf = encode_message_heap!(self, cap, msg => {
            let mut room_state = msg.reborrow().init_room_state();
            room_state.set_room_id(room.id);
            room_state.set_name(&room.name);

            // we could optimize this to not allocate an extra vec if there is a small amount of people in the room,
            // but i decided it's not worth it
            let players = room.get_players();
            let player_count = players.len().min(PLAYER_CAP);

            // TODO: like globed, we should prioritize friends, and when the list is greater than the cap, show random players
            let mut players_ser = room_state.init_players(player_count as u32);

            for (i, player) in players.iter().take(player_count).enumerate() {
                let mut player_ser = players_ser.reborrow().get(i as u32);
                player_ser.set_cube(0); // TODO: use player's cube

                let mut level = player_ser.reborrow().init_level();
                level.set_session_id(0); // TODO: use player's session ID

                let mut accdata = player_ser.reborrow().init_account_data();
                let account = player.account_data().expect("client must have account data");
                accdata.set_account_id(account.account_id);
                accdata.set_user_id(account.user_id);
                accdata.set_username(&account.username);
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }
}

impl AppHandler for ConnectionHandler {
    type ClientData = ClientData;

    async fn on_launch(&self, server: QunetServerHandle<Self>) -> AppResult<()> {
        let _ = self.server.set(server.make_weak());

        info!("Globed central server is running!");

        let status_intv = if cfg!(debug_assertions) {
            Duration::from_mins(15)
        } else {
            Duration::from_mins(60)
        };

        server
            .schedule(status_intv, |server| async move {
                server.print_server_status();
                // TODO: shrink server buffer pool here to reclaim memory?
            })
            .await;

        Ok(())
    }

    async fn on_client_connect(
        &self,
        _server: &QunetServer<Self>,
        connection_id: u64,
        address: SocketAddr,
        kind: &str,
    ) -> AppResult<Self::ClientData> {
        if self.server.get().is_none() {
            return Err("server not initialized yet".into());
        }

        info!(
            "Client connected: connection_id={}, address={}, kind={}",
            connection_id, address, kind
        );

        Ok(ClientData::default())
    }

    async fn post_shutdown(&self, _server: &QunetServer<Self>) -> AppResult<()> {
        // by this point all connections have been dropped, we should clean up any resources
        let rooms = self.module::<RoomModule>();
        rooms.cleanup_everything();

        Ok(())
    }

    async fn on_client_data(
        &self,
        _server: &QunetServer<Self>,
        client: &ClientStateHandle,
        data: MsgData<'_>,
    ) {
        info!("Received {} bytes from client {}", data.len(), client.address);

        let result = decode_message_match!(data.as_bytes(), {
            LoginUToken(message) => {
                let account_id = message.get_account_id();
                let token = message.get_token()?.to_str()?;
                self.handle_login_attempt(client, LoginKind::UserToken(account_id, token)).await
            },

            LoginArgon(message) => {
                let account_id = message.get_account_id();
                let token = message.get_token()?.to_str()?;
                self.handle_login_attempt(client, LoginKind::Argon(account_id, token)).await
            },

            LoginPlain(message) => {
                let data = message.get_data()?;
                let account_id = data.get_account_id();
                let user_id = data.get_user_id();
                let username = data.get_username()?.to_str()?;

                let username = heapless::String::from_str(username)
                        .map_err(|_| DataDecodeError::UsernameTooLong)?;

                self.handle_login_attempt(client, LoginKind::Plain(ClientAccountData {
                    account_id, user_id, username
                })).await
            },

            UpdateOwnData(message) => {
                let icons = message.get_icons()?;

                if_auth(client, || {
                    // TODO
                    Ok(())
                })
            },

            CreateRoom(message) => {
                let name = message.get_name()?.to_str()?;
                self.handle_create_room(client, name).await
            },

            JoinRoom(message) => {
                let id = message.get_room_id();
                self.handle_join_room(client, id).await
            },

            LeaveRoom(_message) => {
                self.handle_leave_room(client).await
            },
        });

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                warn!("[{}] handler error: {}", client.address, e);
            }

            Err(e) => {
                warn!("[{}] failed to decode message: {}", client.address, e);
            }
        }
    }
}

fn must_auth(client: &ClientState<ConnectionHandler>) -> HandlerResult<()> {
    if client.data().authorized() {
        Ok(())
    } else {
        Err(HandlerError::Unauthorized)
    }
}

fn if_auth<R, F: FnOnce() -> Result<R, HandlerError>>(
    client: &ClientState<ConnectionHandler>,
    f: F,
) -> Result<R, HandlerError> {
    if client.data().authorized() {
        f()
    } else {
        Err(HandlerError::Unauthorized)
    }
}
