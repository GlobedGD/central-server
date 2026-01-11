use std::{
    fmt::Display,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use build_time::build_time_utc;
use poise::{CreateReply, serenity_prelude::CreateEmbed};
use server_shared::qunet::server::{ServerHandle, stat_tracker::OverallStats};

use super::util::*;
use crate::{
    core::handler::ConnectionHandler,
    discord::{BotError, hex_color_to_decimal},
    rooms::RoomModule,
    users::UsersModule,
};

#[poise::command(slash_command, guild_only = true)]
/// Refresh internal blacklist cache
pub async fn refresh_blacklist_cache(ctx: Context<'_>) -> Result<(), BotError> {
    check_admin(ctx).await?;

    let state = ctx.data();
    let server = state.server()?;
    server.handler().module::<UsersModule>().refresh_blacklist_cache().await?;

    ctx.reply("✅ Cache refreshed.").await?;

    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
/// Change whether a level should be blacklisted at runtime
pub async fn set_level_blacklisted(
    ctx: Context<'_>,
    session: u64,
    blacklist: bool,
) -> Result<(), BotError> {
    check_admin(ctx).await?;

    let state = ctx.data();
    let server = state.server()?;
    let result = server.handler().override_level_hidden(session, blacklist);

    if result {
        ctx.reply("✅ Success").await?;
    } else {
        ctx.reply(":x: Session ID was not found").await?;
    }

    Ok(())
}

#[poise::command(slash_command, guild_only = true)]
/// Show server status
pub async fn status(ctx: Context<'_>) -> Result<(), BotError> {
    check_admin(ctx).await?;

    let state = ctx.data();
    let server = state.server()?;

    let msg = ctx
        .reply_builder(CreateReply::default())
        .embed(collect_clients_stats(&server))
        .embed(collect_perf_stats(&server))
        .embed(collect_gs_stats(&server));

    ctx.send(msg).await?;

    Ok(())
}

#[cfg(feature = "stat-tracking")]
#[poise::command(slash_command, guild_only = true)]
/// Dump and show connection stats
pub async fn conn_stats(ctx: Context<'_>) -> Result<(), BotError> {
    check_admin(ctx).await?;

    let state = ctx.data();
    let server = state.server()?;
    ctx.defer().await?;

    if let Some(data) = server.handler().dump_all_connections().await {
        let msg = ctx.reply_builder(CreateReply::default()).embed(collect_connection_stats(&data));
        ctx.send(msg).await?;
    } else {
        ctx.reply(":x: Stat tracking is not enabled on this server.").await?;
    }

    Ok(())
}

fn collect_clients_stats(server: &ServerHandle<ConnectionHandler>) -> CreateEmbed {
    let rooms = server.handler().module::<RoomModule>();
    CreateEmbed::default()
        .title("Clients")
        .color(hex_color_to_decimal("#00bfff"))
        .field("Authorized", server.handler().client_count().to_string(), false)
        .field(
            "Total",
            format!(
                "{} ({} suspended, {} udp routes)",
                server.client_count(),
                server.suspended_client_count(),
                server.udp_route_count()
            ),
            false,
        )
        .field("Rooms", rooms.get_room_count().to_string(), true)
        .field("Levels", server.handler().level_count().to_string(), true)
}

fn collect_perf_stats(server: &ServerHandle<ConnectionHandler>) -> CreateEmbed {
    let metrics = metrics_process::collector::collect();
    let bpool = server.get_buffer_pool().stats();

    let uptime = Uptime::from(
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
            - metrics.start_time_seconds.unwrap_or(0),
    );

    let mut embed =
        CreateEmbed::default().title("Performance").color(hex_color_to_decimal("#32cd32"));

    embed = embed.description(format!(
        "Running Globed central server v{} (built at {})",
        env!("CARGO_PKG_VERSION"),
        build_time_utc!("%Y-%m-%d %H:%M:%S")
    ));

    embed = embed.field("Uptime", format!("{}", uptime), true);

    if let Some(thr) = metrics.threads {
        embed = embed.field("Threads", thr.to_string(), true);
    }

    if let Some(fd) = metrics.open_fds
        && let Some(mfd) = metrics.max_fds
    {
        embed = embed.field("File descriptors", format!("{}/{}", fd, mfd), true);
    }

    embed = embed.field("Buffer pool", format!("{}", ByteCount(bpool.total_heap_usage)), true);

    embed
}

fn collect_connection_stats(data: &OverallStats) -> CreateEmbed {
    CreateEmbed::default()
        .title("Connection stats")
        .color(hex_color_to_decimal("#ff69b4"))
        .field("Total connections", data.total_conns.to_string(), true)
        .field(
            "Suspends / Resumes",
            format!("{} / {}", data.total_suspends, data.total_resumes),
            true,
        )
        .field("Packets sent", data.pkt_tx.to_string(), true)
        .field("Packets received", data.pkt_rx.to_string(), true)
        .field("Bytes sent", ByteCount(data.bytes_tx).to_string(), true)
        .field("Bytes received", ByteCount(data.bytes_rx).to_string(), true)
}

fn collect_gs_stats(server: &ServerHandle<ConnectionHandler>) -> CreateEmbed {
    let servers = server.handler().get_game_servers();

    let mut embed =
        CreateEmbed::default().title("Game servers").color(hex_color_to_decimal("#ffd700"));

    for server in &*servers {
        embed = embed.field(
            format!("{} ({} / nID {})", server.data.name, server.data.string_id, server.data.id),
            format!("Connected for {}", Uptime(server.uptime())),
            false,
        );
    }

    if servers.is_empty() {
        embed = embed.description("No connected game servers.");
    }

    embed
}

struct ByteCount(pub usize);

impl Display for ByteCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
        let mut size = self.0 as f64;
        let mut unit = 0;

        while size >= 1024.0 && unit < UNITS.len() - 1 {
            size /= 1024.0;
            unit += 1;
        }

        if unit == 0 {
            write!(f, "{} {}", size as usize, UNITS[unit])
        } else {
            write!(f, "{:.1} {}", size, UNITS[unit])
        }
    }
}

struct Uptime(Duration);

impl From<u64> for Uptime {
    fn from(secs: u64) -> Self {
        Uptime(Duration::from_secs(secs))
    }
}

impl Display for Uptime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let total_secs = self.0.as_secs();
        let days = total_secs / 86400;
        let hours = (total_secs % 86400) / 3600;
        let minutes = (total_secs % 3600) / 60;
        let seconds = total_secs % 60;

        if days > 0 {
            write!(f, "{days}d {:02}h {:02}m {:02}s", hours, minutes, seconds)
        } else if hours > 0 {
            write!(f, "{hours}h {:02}m {:02}s", minutes, seconds)
        } else if minutes > 0 {
            write!(f, "{minutes}m {:02}s", seconds)
        } else {
            write!(f, "{seconds}s")
        }
    }
}
