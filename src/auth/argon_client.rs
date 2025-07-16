use futures_util::{SinkExt, StreamExt};
use qunet::buffers::byte_reader::ByteReaderError;
use qunet::buffers::{byte_reader::ByteReader, byte_writer::ByteWriter};
use qunet::message::channel;
use std::{
    collections::VecDeque,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Bytes, protocol::Message},
};
use tracing::{error, info, warn};

use crate::core::client_data::ClientAccountData;

pub struct ArgonClient {
    inner: Arc<InnerState>,
    handle: JoinHandle<()>,
}

/// Converts an http(s) URL to an argon ws(s) URL.
/// Example: 'https://argon.globed.dev' -> 'wss://argon.globed.dev/v1/ws'
fn to_ws_url(url: &str) -> String {
    if let Some(url) = url.strip_prefix("http://") {
        format!("ws://{url}/v1/ws")
    } else if let Some(url) = url.strip_prefix("https://") {
        format!("wss://{url}/v1/ws")
    } else {
        panic!("Invalid argon URL: {url}");
    }
}

pub struct ArgonValidateResponse {
    result: Result<ClientAccountData, String>,
}

impl ArgonValidateResponse {
    pub fn is_valid(&self) -> bool {
        self.result.is_ok()
    }

    pub fn data(&self) -> Option<&ClientAccountData> {
        self.result.as_ref().ok()
    }

    pub fn into_inner(self) -> Result<ClientAccountData, String> {
        self.result
    }

    pub fn error(&self) -> Option<&str> {
        self.result.as_ref().err().map(String::as_str)
    }
}

pub struct ValidationAwaitToken {
    rx: channel::Receiver<ArgonValidateResponse>,
}

struct ArgonValidateRequest {
    account_id: i32,
    token: String,
    tx: channel::Sender<ArgonValidateResponse>,
}

#[derive(Debug)]
pub struct TokenValidationDropped;

impl ValidationAwaitToken {
    #[inline]
    pub async fn wait(self) -> Result<ArgonValidateResponse, TokenValidationDropped> {
        self.rx.recv().await.ok_or(TokenValidationDropped)
    }
}

impl ArgonClient {
    pub fn new(url: String, api_token: String) -> Self {
        let inner = Arc::new(InnerState::new(url, api_token));
        let handle = inner.clone().run();

        Self { inner, handle }
    }

    pub fn url(&self) -> &str {
        &self.inner.url
    }

    pub fn validate(
        &self,
        account_id: i32,
        token: &str,
    ) -> Result<ValidationAwaitToken, &'static str> {
        if !self.inner.connected.load(Ordering::Acquire) {
            return Err("argon client is not connected");
        }

        let (tx, rx) = channel::new_channel(1);

        let req = ArgonValidateRequest {
            account_id,
            token: token.to_string(),
            tx,
        };

        if self.inner.req_tx.send(req) {
            Ok(ValidationAwaitToken { rx })
        } else {
            Err("argon request queue is full")
        }
    }
}

impl Drop for ArgonClient {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[derive(Debug, Error)]
pub enum ArgonClientError {
    #[error("Failed to connect to argon server: {0}")]
    ConnectionError(#[from] Box<tokio_tungstenite::tungstenite::Error>),
    #[error("Auth attempt timed out")]
    AuthTimeout,
    #[error("Server sent an invalid message")]
    InvalidMessage,
    #[error("Unexpected account ID in response")]
    UnexpectedAccountId,
    #[error("Failed to decode server response: {0}")]
    DecodeError(#[from] ByteReaderError),
    #[error("{0}")]
    Other(String),
}

impl From<tokio_tungstenite::tungstenite::Error> for ArgonClientError {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        ArgonClientError::ConnectionError(Box::new(err))
    }
}

#[allow(unused)]
enum ArgonMessageType {
    Auth = 1,
    AuthAck = 2,
    FatalError = 3,
    Error = 4,
    Status = 5,
    StatusResponse = 6,
    Validate = 7,
    ValidateResponse = 8,
    ValidateStrong = 9,
    ValidateStrongResponse = 10,
    ValidateCheckDataMany = 13,
    ValidateCheckDataManyResponse = 14,
}

struct InnerState {
    url: String,
    api_token: String,
    connected: AtomicBool,

    req_tx: channel::Sender<ArgonValidateRequest>,
    req_rx: channel::Receiver<ArgonValidateRequest>,
}

impl InnerState {
    pub fn new(url: String, api_token: String) -> Self {
        let (req_tx, req_rx) = channel::new_channel(128);

        Self {
            url,
            api_token,
            connected: AtomicBool::new(false),
            req_tx,
            req_rx,
        }
    }

    pub fn run(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            // websocket thread will keep trying to connect to the argon server, sleeping on failure
            loop {
                match self._try_connect().await {
                    Ok(conn) => match self._conn_loop(conn).await {
                        Ok(()) => {}
                        Err(e) => {
                            warn!("argon connection loop failed: {e}");
                        }
                    },
                    Err(e) => {
                        warn!("connection to argon server failed: {e}");
                    }
                }

                // do cleanup
                self.connected.store(false, Ordering::Release);
                self.req_rx.drain();

                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        })
    }

    async fn _try_connect(
        &self,
    ) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, ArgonClientError> {
        let (mut socket, _) = tokio_tungstenite::connect_async(to_ws_url(&self.url)).await?;

        // Send auth message
        let msg = Message::text(format!(r#"{{"token":"{}","proto":"binary-v1"}}"#, self.api_token));

        socket.send(msg).await?;

        // wait for ack
        let msg = match tokio::time::timeout(Duration::from_secs(5), socket.next()).await {
            Ok(Some(msg)) => msg?,
            Ok(None) => {
                error!("ws connection closed before receiving auth ack");
                return Err(ArgonClientError::AuthTimeout);
            }

            Err(_) => {
                error!("argon auth attempt timed out");
                return Err(ArgonClientError::AuthTimeout);
            }
        };

        match msg {
            Message::Text(text) => {
                self._extract_if_ack_successful(text.as_str())?;
            }

            _ => {
                error!("argon server sent an invalid message: {:?}", msg);
                return Err(ArgonClientError::InvalidMessage);
            }
        }

        Ok(socket)
    }

    fn _extract_if_ack_successful(&self, msg: &str) -> Result<(), ArgonClientError> {
        let obj = serde_json::Value::from_str(msg).map_err(|_| ArgonClientError::InvalidMessage)?;

        let Some(ty) = obj["type"].as_str() else {
            error!("invalid message sent by argon server: {msg}");
            return Err(ArgonClientError::InvalidMessage);
        };

        if ty == "AuthAck" {
            Ok(())
        } else if ty == "FatalError" {
            error!("argon server sent a fatal error: {}", obj["data"]["error"]);
            Err(ArgonClientError::Other(
                obj["data"]["error"].as_str().unwrap_or("<unknown fatal error>").to_owned(),
            ))
        } else {
            error!("expected AuthAck from argon server, got message: {}", msg);
            Err(ArgonClientError::InvalidMessage)
        }
    }

    async fn _conn_loop(
        &self,
        mut socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> Result<(), ArgonClientError> {
        // TODO: this function is hell please refactor it

        self.req_rx.drain();
        self.connected.store(true, Ordering::SeqCst);

        info!("Argon client successfully connected to {}", self.url);

        let mut in_flight = VecDeque::new();

        struct InFlightReq {
            tx: channel::Sender<ArgonValidateResponse>,
            account_id: i32,
        }

        let mut data_buf = [0u8; 64];

        loop {
            tokio::select! {
                msg = self.req_rx.recv() => match msg {
                    Some(msg) => {
                        let mut writer = ByteWriter::new(&mut data_buf);
                        writer.write_u8(ArgonMessageType::ValidateCheckDataMany as u8);
                        writer.write_u16(1); // number of accounts
                        writer.write_i32(msg.account_id);
                        writer.write_string_u16(&msg.token);

                        // send a ws message
                        socket.send(Message::Binary(Bytes::copy_from_slice(writer.written()))).await?;

                        // add to in-flight queue
                        in_flight.push_back(InFlightReq {
                            tx: msg.tx,
                            account_id: msg.account_id,
                        });
                    },

                    None => panic!("Argon request channel closed unexpectedly"),
                },

                msg = socket.next() => match msg {
                    Some(Ok(msg)) => {
                        if !msg.is_binary() {
                            // holy shit non binary
                            error!("argon server sent a non-binary message: {:?}", msg);
                            return Err(ArgonClientError::InvalidMessage);
                        }

                        let Message::Binary(data) = msg else { unreachable!() };

                        let mut reader = ByteReader::new(data.as_ref());
                        let msg = reader.read_u8()?;

                        if msg != ArgonMessageType::ValidateCheckDataManyResponse as u8 {
                            if msg == ArgonMessageType::Error as u8 {
                                let err = reader.read_string_u16()?;
                                error!("argon server sent an Error message: {err}");
                                continue;
                            } else {
                                error!("argon server sent unexpected message: {msg}");
                                return Err(ArgonClientError::InvalidMessage);
                            }
                        }

                        let num_accounts = reader.read_u16()?;
                        if num_accounts != 1 {
                            error!("argon server sent unexpected number of accounts: {num_accounts}");
                            return Err(ArgonClientError::InvalidMessage);
                        }

                        let account_id = reader.read_i32()?;
                        let valid = reader.read_bool()?;

                        let resp = if valid {
                            let user_id = reader.read_i32()?;
                            let username = reader.read_string_u16()?;

                            ArgonValidateResponse {
                                result: Ok(ClientAccountData {
                                    account_id,
                                    user_id,
                                    username: heapless::String::from_str(username).map_err(|_| ArgonClientError::InvalidMessage)?,
                                }),
                            }
                        } else {
                            let cause = reader.read_string_u16()?;

                            ArgonValidateResponse {
                                result: Err(cause.to_owned()),
                            }
                        };

                        match in_flight.pop_front() {
                            Some(InFlightReq { tx, account_id: expected_id }) => {
                                // this should never really happen
                                if account_id != expected_id {
                                    error!("argon server sent response for unexpected account ID: {account_id}, expected: {expected_id}");
                                    return Err(ArgonClientError::UnexpectedAccountId);
                                }

                                if !tx.send(resp) {
                                    warn!("argon validation response channel closed, dropping response");
                                }
                            },

                            None => {
                                error!("argon server sent response for an unknown request");
                                return Err(ArgonClientError::InvalidMessage);
                            },
                        }
                    },

                    Some(Err(e)) => {
                        error!("argon server connection error: {e}");
                        return Err(ArgonClientError::ConnectionError(Box::new(e)));
                    },

                    None => {
                        warn!("argon server connection closed");
                        return Ok(());
                    },
                }
            };
        }
    }
}
