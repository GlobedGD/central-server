use serde::Deserialize;
use serde_json::json;
use server_shared::data::ClientAccountData;
use thiserror::Error;
use tracing::warn;

use crate::users::DiscordUserData;

#[derive(Deserialize)]
pub struct ArgonCheckResponse {
    pub valid: bool,
    // this struct just so happens to have the same layout as argon's user response
    pub account_data: Option<ClientAccountData>,
    pub discord_user: Option<DiscordUserData>,
}

#[derive(Debug, Error)]
pub enum ArgonError {
    #[error("failed to send a request to Argon: {0}")]
    Request(#[from] reqwest::Error),
    #[error("argon server returned unknown error, code: {0}, body: {1}")]
    Unknown(u16, String),
    #[error("argon server returned an error: {0}")]
    Argon(String),
}

pub struct ArgonClient {
    http_client: reqwest::Client,
    url: String,
    api_key: String,
}

impl ArgonClient {
    pub fn new(http_client: reqwest::Client, url: String, api_key: String) -> Self {
        Self { http_client, url, api_key }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn check_token(&self, token: &str) -> Result<ArgonCheckResponse, ArgonError> {
        let mut request = self.http_client.post(format!("{}/v2/token/validate", self.url));
        if !self.api_key.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let response = request
            .json(&json!({
                "token": token,
                "include_discord": true
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(self.extract_error(response).await);
        }

        Ok(response.json().await?)
    }

    async fn extract_error(&self, response: reqwest::Response) -> ArgonError {
        let code = response.status();
        let text = match response.text().await {
            Ok(t) => t,
            Err(e) => format!("failed to read response body: {}", e),
        };

        #[derive(Deserialize)]
        struct ErrorBody {
            error: String,
            code: String,
        }

        match serde_json::from_str::<ErrorBody>(&text) {
            Ok(body) => {
                warn!("got error {} from argon!", body.code);
                ArgonError::Argon(body.error)
            }

            Err(e) => {
                warn!("failed to parse argon error response: {e}");
                ArgonError::Unknown(code.as_u16(), text)
            }
        }
    }
}
