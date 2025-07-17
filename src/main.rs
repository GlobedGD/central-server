#![feature(try_blocks, duration_constructors_lite)]
#![allow(clippy::new_without_default)]

use qunet::server::{
    Server as QunetServer, ServerOutcome,
    builder::{BufferPoolOpts, MemoryUsageOptions, UdpDiscoveryMode},
};

use server_shared::config::parse_addr;
use tracing::{debug, error};
use tracing_appender::non_blocking::WorkerGuard;

use crate::{
    auth::AuthModule,
    core::{
        config::{Config, CoreConfig},
        game_server::GameServerHandler,
        handler::ConnectionHandler,
        module::ServerModule,
    },
    rooms::RoomModule,
};

#[cfg(all(not(target_env = "msvc"), not(debug_assertions)))]
use tikv_jemallocator::Jemalloc;

#[cfg(all(not(target_env = "msvc"), not(debug_assertions)))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

pub mod auth;
pub mod core;
pub mod rooms;

fn setup_logger(config: &CoreConfig) -> WorkerGuard {
    server_shared::logging::setup_logger(
        config.log_rolling,
        &config.log_directory,
        &config.log_filename,
        &config.log_level,
        config.log_file_enabled,
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load config and setup logger
    let mut config = match Config::new() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("Failed to load configuration: {e}");
            std::process::exit(1);
        }
    };

    let _guard = setup_logger(config.core());

    let mut handler = ConnectionHandler::new();

    // Add necessary modules
    init_module::<AuthModule>(&config, &handler);
    init_module::<RoomModule>(&config, &handler);

    // Add optional modules
    // todo

    // Freeze handler and config, this disallows adding new modules,
    // but improves performance by removing the need for locks.
    config.freeze();
    handler.freeze();

    // Initialize the qunet server
    let core = config.core();

    let mut builder = QunetServer::builder()
        .with_memory_options(make_memory_limits(core.memory_usage))
        .with_app_handler(handler);

    if core.enable_quic {
        builder = builder.with_quic(
            parse_addr(&core.quic_address, "quic_address"),
            &core.quic_tls_cert,
            &core.quic_tls_key,
        );
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
            std::process::exit(1);
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

    if let Some(addr) = &core.gs_tcp_address {
        builder = builder.with_tcp(parse_addr(addr, "gs_tcp_address"));
    }

    if let Some(addr) = &core.gs_quic_address {
        builder = builder.with_quic(
            parse_addr(addr, "gs_quic_address"),
            &core.quic_tls_cert,
            &core.quic_tls_key,
        );
    }

    // TODO: qdb

    let gs_server = match builder.build().await {
        Ok(srv) => srv,
        Err(e) => {
            error!("Failed to setup game server listener: {e}");
            std::process::exit(1);
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

fn init_module<'a, T: ServerModule>(config: &Config, handler: &'a ConnectionHandler) -> &'a T {
    if let Err(e) = config.init_module::<T>() {
        error!("Failed to initialize config for module {} ({}): {e}", T::name(), T::id());
        std::process::exit(1);
    }

    let conf = config.module::<T>();

    let module = match T::new(conf) {
        Ok(m) => m,
        Err(e) => {
            error!("Failed to initialize module {} ({}): {e}", T::name(), T::id());
            std::process::exit(1);
        }
    };

    handler.insert_module(module);

    handler.module()
}

fn make_memory_limits(usage: u32) -> MemoryUsageOptions {
    let (buf_min_mult, buf_max_mult, rcvbuf, sndbuf) =
        server_shared::config::make_memory_limits(usage);

    MemoryUsageOptions {
        buffer_pools: vec![
            BufferPoolOpts::new(1500, 16 * buf_min_mult, 64 * buf_max_mult), // buffers around mtu size for udp
            BufferPoolOpts::new(4096, 8 * buf_min_mult, 32 * buf_max_mult),  // small buffers
            BufferPoolOpts::new(65536, buf_min_mult, 4 * buf_max_mult),      // large buffers
        ],
        udp_listener_buffer_pool: BufferPoolOpts::new(1500, 8 * buf_min_mult, 32 * buf_max_mult),
        udp_recv_buffer_size: rcvbuf,
        udp_send_buffer_size: sndbuf,
    }
}
