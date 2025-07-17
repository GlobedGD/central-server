use crate::core::module::ServerModule;

mod argon_client;
mod config;

pub use argon_client::ArgonClient;
use config::Config;
use server_shared::token_issuer::*;

pub struct AuthModule {
    token_issuer: TokenIssuer,
    argon_client: Option<ArgonClient>,
}

impl AuthModule {
    pub fn verification_enabled(&self) -> bool {
        self.argon_client.is_some()
    }

    pub fn argon_url(&self) -> Option<&str> {
        self.argon_client.as_ref().map(|client| client.url())
    }

    pub fn argon_client(&self) -> Option<&ArgonClient> {
        self.argon_client.as_ref()
    }

    pub fn validate_user_token(&self, token: &str) -> Result<TokenData, TokenValidationError> {
        self.token_issuer.validate(token)
    }

    pub fn generate_user_token(
        &self,
        account_id: i32,
        user_id: i32,
        username: heapless::String<16>,
    ) -> String {
        self.token_issuer.generate(&TokenData { account_id, user_id, username })
    }
}

impl ServerModule for AuthModule {
    type Config = Config;

    fn new(config: &Self::Config) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let token_issuer = TokenIssuer::new(&config.secret_key)?;

        let argon_client = config
            .enable_argon
            .then(|| ArgonClient::new(config.argon_url.clone(), config.argon_token.clone()));

        Ok(Self { token_issuer, argon_client })
    }

    fn id() -> &'static str {
        "auth"
    }

    fn name() -> &'static str {
        "Authentication"
    }
}
