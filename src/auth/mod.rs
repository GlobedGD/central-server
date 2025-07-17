use crate::core::module::ServerModule;

mod account_data;
mod argon_client;
mod config;

use crate::core::data::LoginFailedReason;
pub use account_data::{ClientAccountData, LoginKind};
pub use argon_client::ArgonClient;
use config::Config;
use server_shared::token_issuer::*;
use tracing::{debug, warn};

pub struct AuthModule {
    token_issuer: TokenIssuer,
    argon_client: Option<ArgonClient>,
}

pub enum AuthVerdict {
    Success(ClientAccountData),
    Failed(LoginFailedReason),
    LoginRequired,
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

    pub async fn handle_login(&self, kind: LoginKind<'_>) -> AuthVerdict {
        match kind {
            LoginKind::Plain(data) => {
                if self.verification_enabled() {
                    AuthVerdict::LoginRequired
                } else {
                    AuthVerdict::Success(data)
                }
            }

            LoginKind::UserToken(account_id, token) => {
                let token_data = match self.validate_user_token(token) {
                    Ok(data) => data,
                    Err(_) => return AuthVerdict::Failed(LoginFailedReason::InvalidUserToken),
                };

                if token_data.account_id != account_id {
                    return AuthVerdict::Failed(LoginFailedReason::InvalidUserToken);
                }

                AuthVerdict::Success(ClientAccountData {
                    account_id: token_data.account_id,
                    user_id: token_data.user_id,
                    username: token_data.username,
                })
            }

            LoginKind::Argon(account_id, token) => {
                if let Some(argon) = self.argon_client() {
                    let handle = match argon.validate(account_id, token) {
                        Ok(handle) => handle,
                        Err(e) => {
                            warn!("failed to request token validation: {e}");
                            return AuthVerdict::Failed(LoginFailedReason::ArgonUnreachable);
                        }
                    };

                    let response = match handle.wait().await {
                        Ok(resp) => resp,
                        Err(_) => {
                            warn!("[{}] token validation attempt was dropped", account_id);
                            return AuthVerdict::Failed(LoginFailedReason::ArgonInternalError);
                        }
                    };

                    match response.into_inner() {
                        Ok(data) => AuthVerdict::Success(data),

                        Err(e) => {
                            debug!("[{}] failed to validate argon token: {}", account_id, e);
                            AuthVerdict::Failed(LoginFailedReason::InvalidArgonToken)
                        }
                    }
                } else {
                    AuthVerdict::Failed(LoginFailedReason::ArgonNotSupported)
                }
            }
        }
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
