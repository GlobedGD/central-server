use std::sync::Arc;
use std::time::Duration;

use crate::{
    auth::argon_client::ArgonError,
    core::{
        handler::ConnectionHandler,
        module::{ConfigurableModule, ModuleInitResult, ServerModule},
    },
    users::DiscordUserData,
};

use config::Config;
use server_shared::{MultiColor, data::ClientAccountData, token_issuer::*};
use tracing::{debug, warn};

mod account_data;
mod argon_client;
mod config;

use crate::core::data::LoginFailedReason;
pub use account_data::LoginKind;
pub use argon_client::ArgonClient;

pub struct AuthModule {
    token_issuer: TokenIssuer,
    argon_client: Option<ArgonClient>,
    trust_token_key: String,
}

pub enum AuthVerdict {
    Success {
        account_data: ClientAccountData,
        discord_data: Option<DiscordUserData>,
    },
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

    pub fn trust_token_key(&self) -> &str {
        self.trust_token_key.as_ref()
    }

    pub fn validate_user_token(
        &self,
        token: &str,
        account_id: i32,
    ) -> Result<TokenData, TokenValidationError> {
        self.token_issuer.validate_match(token, account_id)
    }

    pub fn generate_user_token(
        &self,
        account_id: i32,
        user_id: i32,
        username: &str,
        roles_str: &str,
        name_color: Option<&MultiColor>,
    ) -> String {
        self.token_issuer.generate(account_id, user_id, username, roles_str, name_color)
    }

    /// Generates a new token, keeping some of the data: account/user ID, expiry date.
    pub fn refresh_user_token(
        &self,
        old_token: &str,
        username: &str,
        roles_str: &str,
        name_color: Option<&MultiColor>,
    ) -> Result<String, TokenValidationError> {
        self.token_issuer.refresh(old_token, username, roles_str, name_color)
    }

    pub async fn handle_login(&self, kind: LoginKind<'_>) -> AuthVerdict {
        match kind {
            LoginKind::Plain(data) => {
                if self.verification_enabled() {
                    AuthVerdict::LoginRequired
                } else {
                    AuthVerdict::Success {
                        account_data: data,
                        discord_data: None,
                    }
                }
            }

            LoginKind::UserToken(account_id, token) => {
                let token_data = match self.validate_user_token(token, account_id) {
                    Ok(data) => data,
                    Err(e) => {
                        debug!("[{}] failed to validate user token: {}", account_id, e);
                        return AuthVerdict::Failed(LoginFailedReason::InvalidUserToken);
                    }
                };

                AuthVerdict::Success {
                    account_data: ClientAccountData {
                        account_id: token_data.account_id,
                        user_id: token_data.user_id,
                        username: token_data.username,
                    },
                    discord_data: None,
                }
            }

            LoginKind::Argon(account_id, token) => self.handle_argon_login(account_id, token).await,

            LoginKind::ArgonV2(account_id, token) => {
                self.handle_argon_login(account_id, token).await
            }
        }
    }

    async fn handle_argon_login(&self, account_id: i32, token: &str) -> AuthVerdict {
        let Some(argon) = self.argon_client() else {
            return AuthVerdict::Failed(LoginFailedReason::ArgonNotSupported);
        };

        let response = match argon.check_token(token).await {
            Ok(r) => r,

            Err(e) => {
                warn!("failed to validate token for {account_id}: {e}");
                return AuthVerdict::Failed(match e {
                    ArgonError::Request(_) => LoginFailedReason::ArgonUnreachable,
                    ArgonError::Unknown(_, _) => LoginFailedReason::ArgonInternalError,
                    ArgonError::Argon(_) => LoginFailedReason::ArgonInternalError,
                });
            }
        };

        let account_data = response.account_data;
        if !response.valid || account_data.as_ref().is_none_or(|ad| ad.account_id != account_id) {
            return AuthVerdict::Failed(LoginFailedReason::InvalidArgonToken);
        }

        AuthVerdict::Success {
            account_data: account_data.unwrap(),
            discord_data: response.discord_user,
        }
    }
}

impl ServerModule for AuthModule {
    async fn new(config: Arc<Config>, handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        let token_issuer =
            TokenIssuer::new(&config.secret_key, Duration::from_secs(config.token_expiry as u64))?;

        let argon_client = config.enable_argon.then(|| {
            ArgonClient::new(
                handler.http_client(),
                config.argon_url.clone(),
                config.argon_token.clone(),
            )
        });

        Ok(Self {
            token_issuer,
            argon_client,
            trust_token_key: config.trust_token_key.clone(),
        })
    }

    fn id() -> &'static str {
        "auth"
    }

    fn name() -> &'static str {
        "Authentication"
    }
}
impl ConfigurableModule for AuthModule {
    type Config = Config;
}
