use std::str::FromStr;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as b64e};
use qunet::buffers::{
    byte_reader::{ByteReader, ByteReaderError},
    byte_writer::ByteWriter,
};
use thiserror::Error;

pub struct TokenIssuer {
    secret_key: [u8; 32],
}

pub struct TokenData {
    pub account_id: i32,
    pub user_id: i32,
    pub username: heapless::String<16>,
}

#[derive(Debug, Error)]
pub enum TokenValidationError {
    #[error("Invalid token format")]
    InvalidFormat,
    #[error("Could not decode base64 in token: {0}")]
    InvalidBase64(#[from] base64::DecodeSliceError),
    #[error("Invalid binary token structure: {0}")]
    InvalidBinary(#[from] ByteReaderError),
    #[error("Unsupported token version: {0}")]
    UnsupportedVersion(u8),
    #[error("Username too long")]
    UsernameTooLong,
    #[error("Invalid signature")]
    InvalidSignature,
}

impl TokenIssuer {
    pub fn new(secret_key: &str) -> Result<Self, &'static str> {
        let mut authtoken_secret_key = [0u8; 32];
        hex::decode_to_slice(secret_key, &mut authtoken_secret_key)
            .map_err(|_| "invalid secret key format, expected a 256-bit hex string")?;

        Ok(Self {
            secret_key: authtoken_secret_key,
        })
    }

    pub fn validate(&self, token: &str) -> Result<TokenData, TokenValidationError> {
        let (data, sig) = token
            .split_once('.')
            .ok_or(TokenValidationError::InvalidFormat)?;

        let mut data_buf = [0u8; 64];
        let data_len = b64e.decode_slice(data, &mut data_buf)?;
        let data = &data_buf[..data_len];

        // validate signature
        let mut sig_buf = [0u8; 32];
        if b64e.decode_slice(sig, &mut sig_buf)? != 32 {
            return Err(TokenValidationError::InvalidSignature);
        }

        let valid = blake3::keyed_hash(&self.secret_key, data) == blake3::Hash::from_bytes(sig_buf);
        if !valid {
            return Err(TokenValidationError::InvalidSignature);
        }

        // decode the data
        let mut reader = ByteReader::new(data);
        let version = reader.read_u8()?;

        if version != 1 {
            return Err(TokenValidationError::UnsupportedVersion(version));
        }

        let account_id = reader.read_i32()?;
        let user_id = reader.read_i32()?;
        let username = reader.read_string_u8()?;
        let username = heapless::String::from_str(username)
            .map_err(|_| TokenValidationError::UsernameTooLong)?;

        Ok(TokenData {
            account_id,
            user_id,
            username,
        })
    }

    pub fn generate(&self, data: &TokenData) -> String {
        let mut buf = [0u8; 64];
        let mut writer = ByteWriter::new(&mut buf);

        writer.write_u8(1); // version
        writer.write_i32(data.account_id);
        writer.write_i32(data.user_id);
        writer.write_string_u8(&data.username);

        let data = writer.written();

        // sign the token
        let mut sig_buf = [0u8; 42]; // 32 / 3 * 4 + (32 % 3)
        let sig_len = b64e
            .encode_slice(
                blake3::keyed_hash(&self.secret_key, data).as_bytes(),
                &mut sig_buf,
            )
            .expect("b64 encoded signature must be exactly 42 bytes long");

        assert_eq!(
            sig_len, 42,
            "b64 encoded signature must be exactly 42 bytes long"
        );

        let mut data_buf = [0u8; 128];
        let data_len = b64e
            .encode_slice(data, &mut data_buf)
            .expect("b64 encoded data must fit in 128 bytes");

        format!(
            "{}.{}",
            str::from_utf8(&sig_buf).expect("signature must be valid UTF-8"),
            str::from_utf8(&data_buf[..data_len]).expect("data must be valid UTF-8")
        )
    }
}
