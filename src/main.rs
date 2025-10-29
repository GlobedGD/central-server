#![feature(
    const_index,
    const_cmp,
    const_trait_impl,
    const_result_trait_fn,
    try_blocks,
    iter_array_chunks,
    if_let_guard,
    string_remove_matches
)]
#![allow(clippy::new_without_default, clippy::collapsible_if, clippy::too_many_arguments)]

use std::sync::Arc;

use server_shared::qunet::server::{
    Server as QunetServer, ServerOutcome,
    builder::{BufferPoolOpts, MemoryUsageOptions, UdpDiscoveryMode},
};

use server_shared::config::parse_addr;
use server_shared::logging::WorkerGuard;
use tracing::{debug, error};

use crate::{
    auth::AuthModule,
    core::{
        config::{Config, CoreConfig},
        game_server::GameServerHandler,
        gd_api::GDApiClient,
        handler::ConnectionHandler,
        module::{ConfigurableModule, ServerModule},
    },
    credits::CreditsModule,
    rooms::RoomModule,
    users::UsersModule,
};

#[cfg(all(not(target_env = "msvc"), not(debug_assertions)))]
use tikv_jemallocator::Jemalloc;

#[cfg(all(not(target_env = "msvc"), not(debug_assertions)))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

pub mod auth;
pub mod core;
pub mod credits;
pub mod rooms;
pub mod users;

#[cfg(feature = "discord")]
pub mod discord;
#[cfg(feature = "featured-levels")]
pub mod features;
#[cfg(feature = "word-filter")]
pub mod word_filter;

fn setup_logger(config: &CoreConfig) -> (WorkerGuard, WorkerGuard) {
    server_shared::logging::setup_logger(
        config.log_rolling,
        &config.log_directory,
        &config.log_filename,
        &config.log_level,
        &config.log_level,
        config.log_file_enabled,
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load config and setup logger
    let config = match Config::new() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("Failed to load configuration: {e}");
            std::process::exit(1);
        }
    };

    let _guard = setup_logger(config.core());

    // this is needed for tokio tungstenite :/
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install default crypto provider");

    // set the globals for GD api requests
    if let Some(url) = config.core().gd_api_base_url.clone() {
        GDApiClient::set_global_base_url(url);
    }
    if let Some(token) = config.core().gd_api_auth_token.clone() {
        GDApiClient::set_global_auth_token(token);
    }

    let mut handler = ConnectionHandler::new(config);

    // Add optional modules that have dependents
    #[cfg(feature = "discord")]
    {
        let _discord =
            init_optional_module::<discord::DiscordModule>(&handler, |c| c.enabled).await;

        // if let Some(_) = discord {
        //     // init modules that depend on discord
        // }
    }

    // Add necessary modules
    init_module::<AuthModule>(&handler).await;
    init_module::<RoomModule>(&handler).await;
    init_module::<UsersModule>(&handler).await;
    init_module::<CreditsModule>(&handler).await;

    // Add more optional modules
    #[cfg(feature = "featured-levels")]
    init_module::<features::FeaturesModule>(&handler).await;

    #[cfg(feature = "word-filter")]
    init_module::<word_filter::WordFilterModule>(&handler).await;

    // Freeze handler, this disallows adding new modules and module configs,
    // but improves performance by removing the need for locks.
    handler.freeze();

    // Initialize the qunet server
    let core = handler.config().core().clone();

    let mut builder = QunetServer::builder()
        .with_memory_options(make_memory_limits(core.memory_usage))
        .with_max_messages_per_second(10) // allow 10 messages, client does not really need more than this
        .with_app_handler(handler);

    #[cfg(feature = "quic")]
    {
        if core.enable_quic {
            builder = builder.with_quic(
                parse_addr(&core.quic_address, "quic_address"),
                &core.quic_tls_cert,
                &core.quic_tls_key,
            );
        }
    }

    if core.enable_tcp {
        builder = builder.with_tcp(parse_addr(&core.tcp_address, "tcp_address"));
    }

    if core.enable_udp {
        builder = builder.with_udp(
            parse_addr(&core.udp_address, "udp_address"),
            if core.udp_ping_only {
                UdpDiscoveryMode::Discovery
            } else {
                UdpDiscoveryMode::Both
            },
        );
    }

    if let Some(path) = &core.qdb_path
        && path.exists()
    {
        builder = builder.with_qdb_file(path);
    }

    // Build the server
    let server = match builder.build().await {
        Ok(srv) => srv,
        Err(e) => {
            error!("Failed to setup server: {e}");
            return Ok(());
        }
    };

    // Run the server
    let server_clone = server.clone();
    let mut srv_join_handle = tokio::spawn(async move {
        match server_clone.run().await {
            ServerOutcome::GracefulShutdown => {}
            e => {
                error!("Critical server error: {}", e);
            }
        }
    });

    // .. Build the listener for game servers ..

    let handler = GameServerHandler::new(server.make_weak(), core.gs_password.clone());

    let mut builder =
        QunetServer::builder().with_memory_options(make_memory_limits(3)).with_app_handler(handler);

    if let Some(addr) = &core.gs_tcp_address
        && !addr.is_empty()
    {
        builder = builder.with_tcp(parse_addr(addr, "gs_tcp_address"));
    }

    #[cfg(feature = "quic")]
    {
        if let Some(addr) = &core.gs_quic_address
            && !addr.is_empty()
        {
            builder = builder.with_quic(
                parse_addr(addr, "gs_quic_address"),
                &core.quic_tls_cert,
                &core.quic_tls_key,
            );
        }
    }

    // TODO: qdb

    let gs_server = match builder.build().await {
        Ok(srv) => srv,
        Err(e) => {
            error!("Failed to setup game server listener: {e}");
            return Ok(());
        }
    };

    // Run the game server listener
    let gs_server_clone = gs_server.clone();
    let mut gs_srv_join_handle = tokio::spawn(async move {
        match gs_server_clone.run().await {
            ServerOutcome::GracefulShutdown => {}

            e => {
                error!("Critical game server listener error: {}", e);
            }
        }
    });

    // Poll both of the servers

    tokio::select! {
        _ = &mut srv_join_handle => {
            debug!("Main server has stopped, shutting down");
            gs_server.shutdown();

            if let Err(e) = gs_srv_join_handle.await {
                error!("Failed to join game server listener: {e}");
            }
        }

        _ = &mut gs_srv_join_handle => {
            debug!("Game server listener has stopped, shutting down");
            server.shutdown();

            if let Err(e) = srv_join_handle.await {
                error!("Failed to join main server: {e}");
            }
        }
    }

    Ok(())
}

async fn init_module<T: ServerModule + ConfigurableModule>(handler: &ConnectionHandler) -> Arc<T> {
    init_optional_module(handler, |_| true).await.expect("module initialization failed")
}

async fn init_optional_module<T: ServerModule + ConfigurableModule>(
    handler: &ConnectionHandler,
    should_enable: impl FnOnce(&T::Config) -> bool,
) -> Option<Arc<T>> {
    let config = handler.config();

    if let Err(e) = config.init_module::<T>() {
        error!("Failed to initialize config for module {} ({}): {e}", T::name(), T::id());
        return None;
    }

    let conf = config.module::<T>();

    if !should_enable(conf) {
        return None;
    }

    let module = match T::new(conf, handler).await {
        Ok(m) => m,
        Err(e) => {
            error!("Failed to initialize module {} ({}): {e}", T::name(), T::id());
            return None;
        }
    };

    handler.insert_module(module);

    Some(handler.opt_module_owned().unwrap())
}

fn make_memory_limits(usage: u32) -> MemoryUsageOptions {
    let (initial_mem, max_mem, rcvbuf, sndbuf) = server_shared::config::make_memory_limits(usage);

    MemoryUsageOptions {
        initial_mem,
        max_mem,
        udp_listener_buffer_pool: BufferPoolOpts::new(1500, 16, 512),
        udp_recv_buffer_size: rcvbuf,
        udp_send_buffer_size: sndbuf,
    }
}
