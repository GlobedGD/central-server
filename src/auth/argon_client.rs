use reqwest::Method;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
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

#[derive(Deserialize)]
pub struct DiscordLookupResponse {
    pub account_id: i32,
    pub discord_user: DiscordUserData,
}

#[derive(Deserialize)]
pub struct DiscordLookupAllResponse {
    pub users: Vec<DiscordLookupResponse>,
}

#[derive(Debug, Error)]
pub enum ArgonError {
    #[error("failed to send a request to Argon: {0}")]
    Request(#[from] reqwest::Error),
    #[error("argon server returned unknown error, code: {0}, body: {1}")]
    Unknown(u16, String),
    #[error("argon server returned an error: {0}")]
    Argon(String),
    #[error("resource not found")]
    NotFound,
    #[error("response timed out")]
    Timeout,
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
        self.send_request(
            "v2/token/validate",
            Method::POST,
            &json!({
                "token": token,
                "include_discord": true
            }),
        )
        .await
    }

    pub async fn lookup_discord_by_gd(
        &self,
        account_id: i32,
    ) -> Result<DiscordLookupResponse, ArgonError> {
        self.send_request(
            "v2/discord/lookup",
            Method::POST,
            &json!({
                "account_id": account_id,
            }),
        )
        .await
    }

    pub async fn lookup_gd_by_discord(
        &self,
        discord_id: u64,
    ) -> Result<DiscordLookupResponse, ArgonError> {
        self.send_request(
            "v2/discord/lookup",
            Method::POST,
            &json!({
                "discord_id": discord_id,
            }),
        )
        .await
    }

    pub async fn lookup_all_linked_accounts(&self) -> Result<DiscordLookupAllResponse, ArgonError> {
        self.send_request("v2/discord/lookup-all", Method::GET, &()).await
    }

    async fn send_request<T: DeserializeOwned>(
        &self,
        suffix: &str,
        method: Method,
        payload: &impl Serialize,
    ) -> Result<T, ArgonError> {
        let url = format!("{}/{}", self.url, suffix);

        let mut req = self.http_client.request(method.clone(), &url);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }

        if method != Method::GET && method != Method::HEAD {
            req = req.json(payload);
        }

        let response = req.send().await?;

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
                match body.code.as_str() {
                    "not_found" => ArgonError::NotFound,
                    _ => ArgonError::Argon(body.error),
                }
            }

            Err(e) => {
                warn!("failed to parse argon error response: {e}");
                ArgonError::Unknown(code.as_u16(), text)
            }
        }
    }
}
