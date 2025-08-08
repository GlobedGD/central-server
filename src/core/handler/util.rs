use std::sync::{Arc, Weak};

pub use crate::core::data;
pub use qunet::server::client::ClientState;
pub use tracing::{debug, error, info, trace, warn};

use super::ConnectionHandler;
use server_shared::encoding::EncodeMessageError;
use thiserror::Error;

pub type ClientStateHandle = Arc<ClientState<ConnectionHandler>>;
pub type WeakClientStateHandle = Weak<ClientState<ConnectionHandler>>;

#[derive(Debug, Error)]
pub enum HandlerError {
    #[error("failed to encode message: {0}")]
    Encoder(#[from] EncodeMessageError),
    #[error("cannot handle this message while unauthorized")]
    Unauthorized,
    #[error("sensitive message received from a non-moderator")]
    NotAdmin,
}

pub type HandlerResult<T> = Result<T, HandlerError>;

pub fn must_auth(client: &ClientState<ConnectionHandler>) -> HandlerResult<()> {
    if client.data().authorized() {
        Ok(())
    } else {
        Err(HandlerError::Unauthorized)
    }
}

pub fn must_admin_auth(client: &ClientState<ConnectionHandler>) -> HandlerResult<()> {
    if client.data().authorized_admin() {
        Ok(())
    } else {
        Err(HandlerError::NotAdmin)
    }
}
