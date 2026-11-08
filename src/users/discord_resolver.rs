use std::{sync::Arc, time::Duration};

use crate::{
    auth::{ArgonClient, ArgonError, DiscordLookupAllResponse, DiscordLookupResponse},
    users::Error,
};

pub struct DiscordResolver {
    client: Arc<ArgonClient>,
}

impl DiscordResolver {
    pub fn new(client: Arc<ArgonClient>) -> Self {
        Self { client }
    }

    pub async fn resolve_by_account_id(
        &self,
        account_id: i32,
    ) -> Result<Option<DiscordLookupResponse>, Error> {
        match self.with_timeout(self.client.lookup_discord_by_gd(account_id)).await {
            Ok(u) => Ok(Some(u)),
            Err(Error::Argon(ArgonError::NotFound)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn resolve_by_discord_id(
        &self,
        discord_id: u64,
    ) -> Result<Option<DiscordLookupResponse>, Error> {
        match self.with_timeout(self.client.lookup_gd_by_discord(discord_id)).await {
            Ok(u) => Ok(Some(u)),
            Err(Error::Argon(ArgonError::NotFound)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn get_all_linked(&self) -> Result<DiscordLookupAllResponse, Error> {
        Ok(self.client.lookup_all_linked_accounts().await?)
    }

    pub async fn with_timeout<R>(
        &self,
        fut: impl Future<Output = Result<R, ArgonError>>,
    ) -> Result<R, Error> {
        match tokio::time::timeout(Duration::from_secs(1), fut).await {
            Ok(result) => Ok(result?),
            Err(_) => Err(Error::Argon(ArgonError::Timeout)),
        }
    }
}
