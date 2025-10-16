use std::{
    fmt::{Display, Write},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use build_time::build_time_utc;

use super::util::*;
use crate::{discord::BotError, rooms::RoomModule};

#[poise::command(slash_command, guild_only = true)]
/// Show server status
pub async fn status(ctx: Context<'_>) -> Result<(), BotError> {
    if !is_admin(ctx).await? {
        ctx.reply(":x: You do not have permission to use this command.").await?;
        return Ok(());
    }

    let state = ctx.data();
    let Some(server) = state.server() else {
        return Err(BotError::custom("Server handle not initialized"));
    };

    let rooms = server.handler().module::<RoomModule>();

    let mut text = String::new();

    let bpool = server.get_buffer_pool().stats();

    writeln!(
        text,
        "Running globed central server v{} (built at {})",
        env!("CARGO_PKG_VERSION"),
        build_time_utc!("%Y-%m-%d %H:%M:%S")
    )
    .unwrap();

    writeln!(text, "## Clients").unwrap();
    writeln!(text, "* Authorized: {}", server.handler().client_count()).unwrap();
    writeln!(
        text,
        "* Total: {} ({} suspended, {} udp routes)",
        server.client_count(),
        server.suspended_client_count(),
        server.udp_route_count()
    )
    .unwrap();
    writeln!(text, "* Rooms: {}", rooms.get_room_count()).unwrap();

    let metrics = metrics_process::collector::collect();

    let uptime = Uptime::from(
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
            - metrics.start_time_seconds.unwrap_or(0),
    );

    writeln!(text, "## Performance").unwrap();
    writeln!(text, "* Uptime: {}, thread count: {}", uptime, metrics.threads.unwrap_or(0)).unwrap();

    if let Some(fd) = metrics.open_fds
        && let Some(mfd) = metrics.max_fds
    {
        writeln!(text, "* File descriptors: {fd}/{mfd}").unwrap();
    }

    writeln!(text, "* Buffer pool: {}", ByteCount(bpool.total_heap_usage)).unwrap();

    #[cfg(not(target_env = "msvc"))]
    {
        // jemalloc stats!
        use tikv_jemalloc_ctl::{epoch, stats};
        let _ = epoch::advance();

        let allocated = stats::allocated::read().unwrap_or(0);
        let active = stats::active::read().unwrap_or(0);
        let resident = stats::resident::read().unwrap_or(0);

        writeln!(
            text,
            "* Jemalloc stats: {} allocated, {} active, {} resident",
            ByteCount(allocated),
            ByteCount(active),
            ByteCount(resident)
        )
        .unwrap();
    }

    writeln!(text, "## Game servers").unwrap();
    let servers = server.handler().get_game_servers();

    for server in servers {
        writeln!(
            text,
            "* {} ({} / {}) - connected for {}",
            server.data.name,
            server.data.string_id,
            server.data.id,
            Uptime(server.uptime())
        );
    }

    // TODO: qunet stat tracker :p

    ctx.reply(text).await?;

    Ok(())
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
