use std::{any::Any, fmt::Display};
use thiserror::Error;

pub use server_shared::{encoding::*, schema::main::*};

#[derive(Debug, Error)]
pub enum DataDecodeError {
    #[error("capnp error: {0}")]
    Capnp(#[from] capnp::Error),
    #[error("invalid enum/union discriminant")]
    InvalidDiscriminant,
    #[error("invalid utf-8 string: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("username too long")]
    UsernameTooLong,
    #[error("no message handler for the incoming message type")]
    NoMessageHandler,
}

macro_rules! decode_message_match {
    ($data:expr, {$($variant:ident($msg_var:ident) => {  $($t:tt)* }),* $(,)?}) => {{
        let _res: Result<_, $crate::core::data::DataDecodeError> = try {
            let mut data = $data;
            let reader = capnp::serialize::read_message_from_flat_slice_no_alloc(
                &mut data,
                capnp::message::ReaderOptions::new(),
            )?;

            let message = reader
                .get_root::<$crate::core::data::message::Reader>()
                .map_err(|_| $crate::core::data::DataDecodeError::InvalidDiscriminant)?;

            match message.which().map_err(|_| $crate::core::data::DataDecodeError::InvalidDiscriminant)? {
                $($crate::core::data::message::Which::$variant(msg) => {
                    let $msg_var = msg?;
                    $($t)*
                })*

                _ => Err($crate::core::data::DataDecodeError::NoMessageHandler)?,
            }
        };

        _res
    }};
}

#[derive(Debug)]
pub struct EncodeMessageError {
    pub payload: Box<dyn Any + Send + 'static>,
    pub file: &'static str,
    pub line: u32,
}

impl std::error::Error for EncodeMessageError {}

impl Display for EncodeMessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(err) = self.payload.downcast_ref::<String>() {
            write!(f, "error: {} ({}:{})", err, self.file, self.line)
        } else if let Some(err) = self.payload.downcast_ref::<&str>() {
            write!(f, "error: {} ({}:{})", err, self.file, self.line)
        } else {
            write!(
                f,
                "unknown error type: {:?} ({}:{})",
                (*self.payload).type_id(),
                self.file,
                self.line
            )
        }
    }
}

/// Encodes a message into a buffer allocated by the qunet server, using the provided closure.
/// You are required to pass in the estimated maximum message size in bytes, if it proves to be too small,
/// a panic will occur and subsequently be caught and returned as an error.
macro_rules! encode_message_unsafe {
    ($this:expr, $estcap:expr, $msg:ident => $code:expr) => {{
        let _res: Result<qunet::message::BufferKind, $crate::core::data::EncodeMessageError> = try {
            let mut builder = $crate::core::data::CapnpAlloc::<$estcap>::new().into_builder();

            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut $msg = builder.init_root::<$crate::core::data::message::Builder>();
                $code
            }))
            .map_err(|e| $crate::core::data::EncodeMessageError {
                payload: e,
                file: file!(),
                line: line!(),
            })?;

            let ser_size = capnp::serialize::compute_serialized_size_in_words(&builder) * 8;

            #[cfg(debug_assertions)]
            tracing::debug!("serialized size: {ser_size}");

            let mut buf = $this.server().request_buffer(ser_size).await;

            // this must never fail at this point
            capnp::serialize::write_message(&mut buf, &builder).expect("capnp write failed");

            buf
        };

        _res
    }};
}

/// Like `encode_message_unsafe!`, but uses heap buffers from server's bufferpool.
/// You are required to pass in the estimated maximum message size in bytes, if it proves to be too small,
/// a panic will occur and subsequently be caught and returned as an error.
macro_rules! encode_message_heap {
    ($this:expr, $estcap:expr, $msg:ident => $code:expr) => {{
        let _res: Result<qunet::message::BufferKind, $crate::core::data::EncodeMessageError> = try {
            let mut buffer = $this.server().request_buffer($estcap).await;

            let mut builder =
                $crate::core::data::CapnpBorrowAlloc::new(&mut buffer[..]).into_builder();

            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut $msg = builder.init_root::<$crate::core::data::message::Builder>();
                $code
            }))
            .map_err(|e| $crate::core::data::EncodeMessageError {
                payload: e,
                file: file!(),
                line: line!(),
            })?;

            let ser_size = capnp::serialize::compute_serialized_size_in_words(&builder) * 8;

            #[cfg(debug_assertions)]
            tracing::debug!("serialized size: {ser_size}");

            let mut buf = $this.server().request_buffer(ser_size).await;

            // this must never fail at this point
            capnp::serialize::write_message(&mut buf, &builder).expect("capnp write failed");

            buf
        };

        _res
    }};
}

pub(crate) use decode_message_match;
// pub(crate) use encode_safe;
pub(crate) use encode_message_heap;
pub(crate) use encode_message_unsafe;
