#[allow(warnings)]
#[path = "../../../schema/generated/main_capnp.rs"]
pub mod main_capnp;
use std::{any::Any, fmt::Display};

pub use main_capnp::*;

use capnp::message::Allocator;
use thiserror::Error;

pub use capnp::message::Builder;

#[repr(align(8))]
pub struct CapnpAlloc<const N: usize> {
    buf: [u8; N],
    called: bool,
}

unsafe impl<const N: usize> Allocator for CapnpAlloc<N> {
    #[inline]
    fn allocate_segment(&mut self, size: u32) -> (*mut u8, u32) {
        if self.called {
            panic!("CapnpAlloc::allocate_segment called multiple times");
        }

        self.called = true;

        if size as usize > N {
            panic!("Not enough space in CapnpAlloc");
        }

        (self.buf.as_mut_ptr(), size)
    }

    #[inline]
    unsafe fn deallocate_segment(&mut self, _ptr: *mut u8, _word_size: u32, _words_used: u32) {}
}

impl<const N: usize> CapnpAlloc<N> {
    pub const fn new() -> Self {
        Self {
            buf: [0; N],
            called: false,
        }
    }

    pub fn into_builder(self) -> Builder<Self> {
        Builder::new(self)
    }
}

impl<const N: usize> Default for CapnpAlloc<N> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct CapnpBorrowAlloc<'a> {
    buf: &'a mut [u8],
    called: bool,
}

unsafe impl<'a> Allocator for CapnpBorrowAlloc<'a> {
    #[inline]
    fn allocate_segment(&mut self, size: u32) -> (*mut u8, u32) {
        if self.called {
            panic!("CapnpAlloc::allocate_segment called multiple times");
        }

        self.called = true;

        if size as usize > self.buf.len() {
            panic!("Not enough space in CapnpAlloc");
        }

        (self.buf.as_mut_ptr(), size)
    }

    #[inline]
    unsafe fn deallocate_segment(&mut self, _ptr: *mut u8, _word_size: u32, _words_used: u32) {}
}

impl<'a> CapnpBorrowAlloc<'a> {
    pub const fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, called: false }
    }

    pub fn into_builder(self) -> Builder<Self> {
        Builder::new(self)
    }
}

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
}

macro_rules! decode_message_match {
    ($data:expr, {$($variant:ident($msg_var:ident) => $block:expr),* $(,)?}) => {{
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
                    $block
                })*

                _ => Err($crate::core::data::DataDecodeError::InvalidDiscriminant)?,
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

pub(crate) use decode_message_match;
// pub(crate) use encode_safe;
pub(crate) use encode_message_unsafe;
