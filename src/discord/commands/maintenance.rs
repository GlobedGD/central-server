use std::fmt::{Display, Write};

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

    writeln!(text, "## Performance").unwrap();
    writeln!(text, "* Buffer pool: {}", ByteCount(bpool.total_heap_usage)).unwrap();

    #[cfg(not(target_env = "msvc"))]
    {
        // jemalloc stats!
        use tikv_jemalloc_ctl::{epoch, stats};
        epoch::advance().unwrap();

        let allocated = stats::allocated::read().unwrap();
        let active = stats::active::read().unwrap();
        let resident = stats::resident::read().unwrap();

        writeln!(
            text,
            "* Jemalloc stats: {} allocated, {} active, {} resident",
            ByteCount(allocated),
            ByteCount(active),
            ByteCount(resident)
        )
        .unwrap();
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
