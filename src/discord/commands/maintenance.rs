use std::{
    fmt::Display,
    io::Cursor,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::anyhow;
use build_time::build_time_utc;
use plotters::{
    chart::ChartBuilder,
    prelude::{BitMapBackend, IntoDrawingArea},
    series::LineSeries,
    style::{Color, RED, RGBColor},
};
use poise::{
    CreateReply,
    serenity_prelude::{
        ButtonStyle, CreateActionRow, CreateAttachment, CreateButton, CreateEmbed,
        CreateInteractionResponse,
    },
};
use server_shared::qunet::server::{ServerHandle, stat_tracker::OverallStats};
use tracing::{info, warn};

use super::util::*;
use crate::{
    core::handler::ConnectionHandler,
    discord::{BotError, hex_color_to_decimal},
    rooms::RoomModule,
    users::{PlayerCountHistoryEntry, UsersModule},
};

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
/// Refresh internal blacklist cache
pub async fn refresh_blacklist_cache(ctx: Context<'_>) -> Result<(), BotError> {
    check_admin(ctx).await?;

    let state = ctx.data();
    let server = state.server()?;
    server.handler().module::<UsersModule>().refresh_blacklist_cache().await?;

    ctx.reply("✅ Cache refreshed.").await?;

    Ok(())
}

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
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

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
/// Cleanly shutdown the server, with optional message
pub async fn shutdown_server(ctx: Context<'_>, message: Option<String>) -> Result<(), BotError> {
    check_admin(ctx).await?;

    // prompt for confirmation
    let msg = ctx
        .reply_builder(
            CreateReply::default()
                .content(":warning: Are you sure you want to shutdown the server?"),
        )
        .components(vec![CreateActionRow::Buttons(vec![
            CreateButton::new("confirm_shutdown")
                .style(ButtonStyle::Success)
                .label("Yes, shutdown"),
            CreateButton::new("cancel_shutdown").style(ButtonStyle::Danger).label("No, cancel"),
        ])]);
    let msg = ctx.send(msg).await?;

    // wait for response
    let interaction = msg
        .message()
        .await?
        .await_component_interaction(ctx)
        .author_id(ctx.author().id)
        .timeout(Duration::from_secs(30))
        .await;

    msg.delete(ctx).await?;

    match interaction {
        Some(interaction) if interaction.data.custom_id == "confirm_shutdown" => {
            info!("Shutdown initiated by {} ({})", ctx.author().name, ctx.author().id);

            interaction.create_response(&ctx, CreateInteractionResponse::Acknowledge).await?;

            let state = ctx.data();
            let server = state.server()?;

            // send a message to all users on the server, if requested
            if let Some(msg) = message {
                let _ = server.handler().send_notice_all(None, &msg, false, false);

                // wait a bit, it might take some time for the message to send successfully
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            ctx.say("Goodbye!").await?;

            server.shutdown();
        }

        _ => {
            ctx.say("Action cancelled.").await?;
        }
    }

    Ok(())
}

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
/// Disables or enables global maintenance mode, disallowing connections from non mods
pub async fn disallow_joins(ctx: Context<'_>, enable: bool) -> Result<(), BotError> {
    check_admin(ctx).await?;

    let state = ctx.data();
    let server = state.server()?;
    server.handler().set_refuse_connections(enable);

    if enable {
        ctx.reply("✅ Maintenance mode enabled: new connections will be refused.").await?;
    } else {
        ctx.reply("✅ Maintenance mode disabled.").await?;
    }

    Ok(())
}

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
/// Show server status
pub async fn status(ctx: Context<'_>) -> Result<(), BotError> {
    check_moderator(ctx).await?;

    let state = ctx.data();
    let server = state.server()?;

    let mut msg = ctx
        .reply_builder(CreateReply::default())
        .embed(collect_clients_stats(&server))
        .embed(collect_perf_stats(&server));

    for embed in collect_gs_stats(&server) {
        msg = msg.embed(embed);
    }

    ctx.send(msg).await?;

    Ok(())
}

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
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

fn make_chart(entries: &[PlayerCountHistoryEntry]) -> anyhow::Result<CreateAttachment> {
    let width = 800;
    let height = 350;
    let mut buffer = vec![0u8; width * height * 3];

    let bg_color = RGBColor(240, 250, 217);
    let main_color = RED.mix(0.7);
    let text_color = RGBColor(0, 0, 0);

    {
        let root = BitMapBackend::with_buffer(&mut buffer, (width as u32, height as u32))
            .into_drawing_area();
        root.fill(&bg_color)?;

        let min_time = entries.first().map(|e| e.timestamp).unwrap_or(0);
        let max_time = entries.last().map(|e| e.timestamp).unwrap_or(0);
        let max_count = (entries.iter().map(|e| e.count).max().unwrap_or(50) as f32 * 1.2) as u32;

        let show_full_time = (max_time - min_time) > 3600 * 32; // 32 hours

        let mut chart = ChartBuilder::on(&root)
            .caption("Player Count", ("sans-serif", 32))
            .margin(20)
            .x_label_area_size(40)
            .y_label_area_size(50)
            .build_cartesian_2d(min_time..max_time, 0..max_count)?;

        chart
            .configure_mesh()
            .disable_x_mesh()
            .disable_y_mesh()
            .axis_style(text_color.mix(0.2))
            .label_style(("sans-serif", 15, &text_color))
            .x_label_formatter(&|x| format_timestamp(*x, show_full_time))
            .draw()?;

        chart.draw_series(LineSeries::new(
            entries.iter().map(|e| (e.timestamp, e.count)),
            main_color.stroke_width(3),
        ))?;

        // chart.draw_series(PointSeries::of_element(
        //     entries.iter().map(|e| (e.timestamp, e.count)),
        //     3,
        //     main_color.filled(),
        //     &|c, s, st| EmptyElement::at(c) + Circle::new((0, 0), s, st),
        // ))?;

        root.present()?;
    }

    let mut png_buf = Vec::new();
    let mut cursor = Cursor::new(&mut png_buf);
    image::write_buffer_with_format(
        &mut cursor,
        &buffer,
        width as u32,
        height as u32,
        image::ColorType::Rgb8,
        image::ImageFormat::Png,
    )?;

    Ok(CreateAttachment::bytes(png_buf, "player_count.png"))
}

fn format_timestamp(ts: i64, full: bool) -> String {
    if full {
        time_format::strftime_utc("%Y-%m-%d %H:%M", ts).unwrap()
    } else {
        time_format::strftime_utc("%H:%M", ts).unwrap()
    }
}

#[poise::command(slash_command, ephemeral = true, guild_only = true)]
/// Show the player count graph for a given period of time
pub async fn player_count(
    ctx: Context<'_>,
    #[description = "The time period (e.g. 1 day, 2 weeks), by default is 24 hours"] period: Option<
        String,
    >,
) -> Result<(), BotError> {
    check_moderator(ctx).await?;

    let state = ctx.data();
    let server = state.server()?;
    let users = server.handler().module::<UsersModule>();

    let period =
        period.map(|p| parse_duration_str(&p)).transpose()?.unwrap_or(Duration::from_days(1));

    let counts = users.get_player_counts_cached(period).await?;
    if counts.is_empty() {
        ctx.reply(":x: No player count data available.").await?;
        return Ok(());
    }

    let max_count = counts.iter().map(|e| e.count).max().unwrap_or(0);
    let mean_count = counts.iter().map(|e| e.count as u64).sum::<u64>() / counts.len() as u64;
    let current = counts.last().map_or(0, |e| e.count);

    let result = tokio::task::spawn_blocking(move || make_chart(&counts))
        .await
        .map_err(|e| anyhow!("task failed: {e}"))
        .flatten();

    match result {
        Ok(attachment) => {
            ctx.send(
                CreateReply::default().attachment(attachment).embed(
                    CreateEmbed::new()
                        .title("Player count graph")
                        .field("Max players", max_count.to_string(), true)
                        .field("Average players", mean_count.to_string(), true)
                        .field("Current players", current.to_string(), true)
                        .image("attachment://player_count.png")
                        .color(0x00ff00),
                ),
            )
            .await?;
        }

        Err(e) => {
            warn!("Failed to generate player count chart: {e}");
            ctx.reply(format!(":x: Failed to generate chart: {e}")).await?;
        }
    };

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

fn collect_gs_stats(server: &ServerHandle<ConnectionHandler>) -> Vec<CreateEmbed> {
    let mut embeds: Vec<CreateEmbed> = server
        .handler()
        .get_game_servers()
        .iter()
        .map(|server| {
            let sdata = server.status_data();
            CreateEmbed::default()
                .title(format!(
                    "{} ({} / nID {})",
                    server.data.name, server.data.string_id, server.data.id
                ))
                .color(hex_color_to_decimal("#ffd700"))
                .field(
                    "Clients",
                    format!("{} ({} authorized)", sdata.clients, sdata.auth_clients),
                    true,
                )
                .field("Rooms", sdata.rooms.to_string(), true)
                .field("Sessions", sdata.sessions.to_string(), true)
                .field("Connected for", Uptime(server.uptime()).to_string(), true)
                .field("Total connections", sdata.total_connections.to_string(), true)
                .field("Total data messages", sdata.total_data_messages.to_string(), true)
        })
        .collect();

    if embeds.is_empty() {
        embeds.push(CreateEmbed::default().title("No game servers connected"));
    }

    embeds
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
