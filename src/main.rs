#![feature(try_blocks, duration_constructors_lite)]
#![allow(clippy::new_without_default)]

use std::net::SocketAddr;

use qunet::server::{
    Server as QunetServer, ServerOutcome,
    builder::{BufferPoolOpts, MemoryUsageOptions, UdpDiscoveryMode},
};

use tracing::{error, info, level_filters::LevelFilter};
use tracing_appender::non_blocking::{NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::{Layer as _, Registry, fmt::Layer, layer::SubscriberExt};

use crate::{
    auth::AuthModule,
    core::{
        config::{Config, CoreConfig},
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
    let appender = if config.log_rolling {
        tracing_appender::rolling::daily(&config.log_directory, &config.log_filename)
    } else {
        tracing_appender::rolling::never(&config.log_directory, &config.log_filename)
    };

    let (nb, guard) = NonBlockingBuilder::default()
        .lossy(true)
        .thread_name("Log writer thread")
        .buffered_lines_limit(8192)
        .finish(appender);

    let log_level = match config.log_level.as_str() {
        "error" => LevelFilter::ERROR,
        "warn" => LevelFilter::WARN,
        "info" => LevelFilter::INFO,
        "debug" => LevelFilter::DEBUG,
        "trace" => LevelFilter::TRACE,
        _ => LevelFilter::INFO,
    };

    let stdout_layer = Layer::default().with_writer(std::io::stdout).with_filter(log_level);

    let subscriber = Registry::default().with(stdout_layer);

    if config.log_file_enabled {
        let subscriber = subscriber
            .with(Layer::default().with_writer(nb).with_ansi(false).with_filter(log_level));
        tracing::subscriber::set_global_default(subscriber)
    } else {
        tracing::subscriber::set_global_default(subscriber)
    }
    .expect("failed to set global subscriber");

    guard
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
        builder = builder.with_udp_multiple(
            parse_addr(&core.udp_address, "udp_address"),
            if core.udp_ping_only {
                UdpDiscoveryMode::Discovery
            } else {
                UdpDiscoveryMode::Both
            },
            core.udp_binds as usize,
        );
    }

    if let Some(path) = &core.qdb_path
        && path.exists()
    {
        builder = builder.with_qdb_file(path);
    }

    // Actually run the server
    let outcome = builder.run().await;

    match outcome {
        ServerOutcome::GracefulShutdown => {
            info!("Server shutdown gracefully.");
        }

        e => {
            error!("Critical server error: {}", e);
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

fn make_memory_limits(mut usage: u32) -> MemoryUsageOptions {
    usage = usage.clamp(1, 11);

    let (buf_min_mult, buf_max_mult, rcvbuf, sndbuf) = match usage {
        1 => (1, 1, None, None),
        2 => (2, 2, None, None),
        3 => (3, 5, None, None),
        4 => (4, 8, None, None),
        5 => (8, 16, None, None),
        6 => (12, 32, None, None),
        7 => (16, 64, None, Some(524288)),
        8 => (32, 128, None, Some(1048576)),
        9 => (64, 256, Some(524288), Some(2097152)),
        10 => (128, 512, Some(1048576), Some(4194304)),
        11 => (256, 1024, Some(2097152), Some(8388608)),
        _ => unreachable!(),
    };

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

fn parse_addr(addr: &str, name: &str) -> SocketAddr {
    match addr.parse() {
        Ok(x) => x,
        Err(e) => {
            error!("failed to parse option '{name}': {e}");
            error!(
                "note: it must be a valid IPv4/IPv6 socket address, for example \"0.0.0.0:4340\" or \"[::]:4340\""
            );

            std::process::exit(1);
        }
    }
}
