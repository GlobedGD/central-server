use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use arc_swap::ArcSwap;
use server_shared::qunet::server::{ServerHandle, WeakServerHandle};
use smallvec::SmallVec;
use tracing::{debug, error, info, warn};

use crate::{
    core::{
        gd_api::{GDApiClient, GDUser},
        handler::ConnectionHandler,
        module::{ConfigurableModule, ModuleInitResult, ServerModule},
    },
    users::UsersModule,
};

mod config;

#[derive(Clone, Debug)]
pub struct CreditsCategory {
    pub name: String,
    pub users: Vec<GDUser>,
}

pub type CategoryVec = SmallVec<[CreditsCategory; 8]>;

pub struct CreditsModule {
    interval: Duration,
    req_interval: Duration,
    config_categories: Vec<config::CreditsCategory>,
    cache: ArcSwap<Option<CategoryVec>>,
    server: OnceLock<WeakServerHandle<ConnectionHandler>>,
    client: GDApiClient,
}

impl CreditsModule {
    async fn reload_cache(&self) {
        info!("Reloading credits cache");

        let server =
            self.server.get().expect("server must be set").upgrade().expect("server must be alive");
        let users = server.handler().module::<UsersModule>();

        let mut out_credits = CategoryVec::new();

        let mut interval = tokio::time::interval(self.req_interval);

        for cat in &self.config_categories {
            let mut ids = Vec::new();

            if let Some(role) = &cat.sync_with_role {
                match users.get_all_users_with_role(role).await {
                    Ok(users) => {
                        for id in users.iter().map(|u| u.account_id) {
                            ids.push((id, None));
                        }
                    }

                    Err(e) => {
                        error!("Failed to fetch users with role '{role}': {e}");
                    }
                }
            }

            for user in &cat.users {
                if let Some(name) = &user.display_name
                    && !name.is_empty()
                {
                    ids.push((user.id, Some(name.clone())));
                }
            }

            let mut users = Vec::new();

            for (id, display_name) in ids {
                interval.tick().await;

                debug!("fetching profile of {id}");

                match self.client.fetch_user(id).await {
                    Ok(Some(mut user)) => {
                        user.display_name = display_name
                            .and_then(|x| x.as_str().try_into().ok())
                            .unwrap_or_else(|| user.username.clone());

                        users.push(user);
                    }

                    Ok(None) => {
                        warn!("Failed to fetch user data for {id}: user not found!");
                    }

                    Err(e) => {
                        warn!("Failed to fetch user data for {id}: {e}");
                    }
                }
            }

            out_credits.push(CreditsCategory { name: cat.name.clone(), users });
        }

        info!(
            "Credits reloaded! Total categories: {}, users: {}",
            out_credits.len(),
            out_credits.iter().map(|c| c.users.len()).sum::<usize>()
        );

        self.cache.store(Arc::new(Some(out_credits)));
    }

    pub fn get_credits(&self) -> Arc<Option<CategoryVec>> {
        self.cache.load_full()
    }
}

impl ServerModule for CreditsModule {
    async fn new(config: &config::Config, _handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        Ok(Self {
            interval: Duration::from_secs(config.credits_cache_timeout as u64),
            req_interval: Duration::from_secs(config.credits_req_interval as u64),
            config_categories: config.credits_categories.clone(),
            cache: ArcSwap::new(Arc::new(None)),
            server: OnceLock::new(),
            client: GDApiClient::default(),
        })
    }

    fn id() -> &'static str {
        "credits"
    }

    fn name() -> &'static str {
        "Credits"
    }

    fn on_launch(&self, server: &ServerHandle<ConnectionHandler>) {
        let _ = self.server.set(server.make_weak());

        server.schedule(self.interval, async |s| {
            s.handler().module::<CreditsModule>().reload_cache().await;
        });

        // run reload right now as well

        let server = server.clone();

        tokio::spawn(async move {
            server.handler().module::<Self>().reload_cache().await;
        });
    }
}

impl ConfigurableModule for CreditsModule {
    type Config = config::Config;
}
