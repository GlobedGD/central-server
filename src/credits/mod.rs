use std::{
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use parking_lot::Mutex;
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
    next_refresh: Mutex<Instant>,
    req_interval: Duration,

    config_categories: Vec<config::CreditsCategory>,
    cache: ArcSwap<Option<CategoryVec>>,
    server: OnceLock<WeakServerHandle<ConnectionHandler>>,
    client: GDApiClient,
}

impl CreditsModule {
    /// Queues a reload of the credits as soon as possible, may take some time to actually happen
    pub fn queue_reload(&self) {
        *self.next_refresh.lock() = Instant::now();
    }

    async fn reload_cache(&self) {
        info!("Reloading credits cache");

        let server =
            self.server.get().expect("server must be set").upgrade().expect("server must be alive");
        let users = server.handler().module::<UsersModule>();

        let mut out_credits = CategoryVec::new();

        let mut interval = tokio::time::interval(self.req_interval);

        for cat in &self.config_categories {
            let mut ids = Vec::new();
            let filter = |id: &i32| !cat.ignored.contains(id);

            if let Some(role) = &cat.sync_with_role {
                match users.get_all_users_with_role(role).await {
                    Ok(users) => {
                        for id in users.iter().map(|u| u.account_id).filter(filter) {
                            ids.push((id, None));
                        }
                    }

                    Err(e) => {
                        error!("Failed to fetch users with role '{role}': {e}");
                    }
                }
            }

            for user in &cat.users {
                ids.push((user.id, user.display_name.clone().filter(|x| !x.is_empty())));
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
        *self.next_refresh.lock() = Instant::now() + self.interval;
    }

    async fn reload_if_needed(&self) {
        let next = *self.next_refresh.lock();

        if Instant::now() >= next {
            self.reload_cache().await;
        }
    }

    pub fn get_credits(&self) -> Arc<Option<CategoryVec>> {
        self.cache.load_full()
    }
}

impl ServerModule for CreditsModule {
    async fn new(config: &config::Config, _handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        Ok(Self {
            interval: Duration::from_secs(config.credits_cache_timeout as u64),
            next_refresh: Mutex::new(Instant::now()),
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

        // re-check every 15 minutes
        server.schedule(Duration::from_mins(15), async |s| {
            s.handler().module::<CreditsModule>().reload_if_needed().await;
        });

        // run reload right now as well

        let server = server.clone();
        tokio::spawn(async move {
            server.handler().module::<CreditsModule>().reload_if_needed().await;
        });
    }
}

impl ConfigurableModule for CreditsModule {
    type Config = config::Config;
}
