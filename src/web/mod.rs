use std::sync::{Arc, OnceLock};

use anyhow::anyhow;
use arc_swap::ArcSwap;
use axum::routing::MethodRouter;
use server_shared::qunet::server::{ServerHandle, WeakServerHandle};
use tokio::{net::TcpListener, sync::Mutex};
use tracing::info;

use crate::core::{
    handler::ConnectionHandler,
    module::{ConfigurableModule, ModuleInitResult, ServerModule},
};
use config::Config;

mod config;

pub struct WebState {
    server: OnceLock<WeakServerHandle<ConnectionHandler>>,
}

impl WebState {
    pub fn server(&self) -> ServerHandle<ConnectionHandler> {
        self.server
            .get()
            .expect("web server handle was not set")
            .upgrade()
            .expect("web server handle was dropped")
    }
}

pub struct WebModule {
    #[allow(dead_code)]
    config: ArcSwap<Config>,
    router: Mutex<Option<axum::Router<Arc<WebState>>>>,
    listener: Mutex<Option<TcpListener>>,
    state: Arc<WebState>,
}

impl WebModule {
    pub async fn add_router_merge<R: Into<axum::Router<Arc<WebState>>>>(&self, input_r: R) {
        let mut router = self.router.lock().await;
        let r = router.take().expect("called add_router_merge after the app was launched");
        *router = Some(r.merge(input_r));
    }

    pub async fn add_route(&self, route: &str, handler: MethodRouter<Arc<WebState>>) {
        let mut router = self.router.lock().await;
        let r = router.take().expect("called add_route after the app was launched");
        *router = Some(r.route(route, handler));
    }
}

impl ServerModule for WebModule {
    async fn new(config: Arc<Config>, _handler: &ConnectionHandler) -> ModuleInitResult<Self> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", config.port))
            .await
            .map_err(|e| anyhow!("failed to bind web server port: {e}"))?;

        let state = Arc::new(WebState { server: OnceLock::new() });
        let router = axum::Router::new();

        Ok(Self {
            config: ArcSwap::new(config),
            router: Mutex::new(Some(router)),
            listener: Mutex::new(Some(listener)),
            state,
        })
    }

    fn id() -> &'static str {
        "web"
    }

    fn name() -> &'static str {
        "Web Server"
    }

    fn on_launch(&self, server: &ServerHandle<ConnectionHandler>) {
        let this = server.handler().opt_module_owned::<Self>().unwrap();
        this.state.server.set(server.make_weak()).ok();

        tokio::spawn(async move {
            let listener = this.listener.lock().await.take().unwrap();
            let router = this.router.lock().await.take().unwrap().with_state(this.state.clone());

            info!("Web server listening on http://{}", listener.local_addr().unwrap());
            axum::serve(listener, router).await.unwrap()
        });
    }
}

impl ConfigurableModule for WebModule {
    type Config = Config;
}
