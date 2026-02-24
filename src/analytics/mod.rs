use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};
use parking_lot::Mutex;
use server_shared::qunet::{
    message::channel,
    server::{ServerHandle, WeakServerHandle},
};
use tracing::{debug, error};

use crate::core::{
    handler::ConnectionHandler,
    module::{ConfigurableModule, ModuleInitResult, ServerModule},
};

mod config;
mod migrations;
mod models;
use config::Config;
pub use models::LoginEvent;

#[cfg(debug_assertions)]
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(not(debug_assertions))]
const FLUSH_INTERVAL: Duration = Duration::from_secs(45);

pub enum Event {
    Login(LoginEvent),
}

pub struct AnalyticsModule {
    client: Option<clickhouse::Client>,
    server: OnceLock<WeakServerHandle<ConnectionHandler>>,
    tx: channel::Sender<Event>,
    rx: Mutex<Option<channel::Receiver<Event>>>,
}

impl AnalyticsModule {
    pub async fn run(&self) -> Result<()> {
        let client = self.client.as_ref().expect("client must be initialized");
        let rx = self.rx.lock().take().expect("receiver must be initialized");

        // perform migrations
        migrations::run(client).await.map_err(|e| anyhow!("Failed to run migrations: {e}"))?;

        let mut last_flush = Instant::now();
        let mut pending_logins = Vec::new();

        loop {
            let deadline = last_flush + FLUSH_INTERVAL;
            if let Ok(ev) = tokio::time::timeout_at(deadline.into(), rx.recv()).await {
                match ev {
                    Some(Event::Login(event)) => {
                        pending_logins.push(event);
                    }

                    None => break,
                }
            }

            // flush either when the interval has passed or when we have too many pending events
            let should_flush = last_flush.elapsed() > FLUSH_INTERVAL || pending_logins.len() > 250;

            if should_flush {
                last_flush = Instant::now();

                if let Err(e) = self.flush(client, &mut pending_logins).await {
                    error!("{e}");
                }
            }
        }

        Ok(())
    }

    async fn flush(&self, client: &clickhouse::Client, logins: &mut Vec<LoginEvent>) -> Result<()> {
        if !logins.is_empty() {
            self.flush_pending_logins(client, logins)
                .await
                .map_err(|e| anyhow!("failed to flush login events: {e}"))?;
            logins.clear();
        }

        Ok(())
    }

    async fn flush_pending_logins(
        &self,
        client: &clickhouse::Client,
        logins: &mut Vec<LoginEvent>,
    ) -> Result<()> {
        debug!("Writing {} login events", logins.len());
        let mut insert = client.insert::<LoginEvent>("login_events").await?;
        for login in logins.drain(..) {
            insert.write(&login).await?;
        }
        insert.end().await?;

        Ok(())
    }

    pub fn log_event(&self, event: Event) {
        if self.client.is_some() {
            self.tx.send(event);
        }
    }

    pub fn log_login_event(&self, event: LoginEvent) {
        self.log_event(Event::Login(event));
    }
}

fn create_client(config: &Config) -> Result<Option<clickhouse::Client>> {
    if config.url.is_empty() {
        Ok(None)
    } else {
        if config.username.is_empty() || config.password.is_empty() || config.database.is_empty() {
            bail!(
                "Clickhouse URL is set in config/clickhouse.toml, but username, password or database are missing. Please provide all fields or leave the URL empty to disable analytics."
            );
        }

        let client = clickhouse::Client::default()
            .with_url(&config.url)
            .with_user(&config.username)
            .with_password(&config.password)
            .with_database(&config.database);

        Ok(Some(client))
    }
}

impl ServerModule for AnalyticsModule {
    async fn new(config: &Config, _handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        let (tx, rx) = channel::new_channel(1024);

        Ok(Self {
            client: create_client(config)?,
            server: OnceLock::new(),
            tx,
            rx: Mutex::new(Some(rx)),
        })
    }

    fn id() -> &'static str {
        "analytics"
    }

    fn name() -> &'static str {
        "Analytics"
    }

    fn on_launch(&self, server: &ServerHandle<ConnectionHandler>) {
        let _ = self.server.set(server.make_weak());

        if self.client.is_some() {
            let server = server.clone();
            tokio::spawn(async move {
                if let Err(e) = server.handler().module::<Self>().run().await {
                    error!("Analytics module failed: {e}");
                }
            });
        }
    }
}

impl ConfigurableModule for AnalyticsModule {
    type Config = Config;
}
