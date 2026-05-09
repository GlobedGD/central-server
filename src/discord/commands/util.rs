use std::{sync::Arc, time::Duration};

use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use itertools::Itertools;
use poise::{
    CreateReply, ReplyHandle,
    serenity_prelude::{self as serenity, AutocompleteChoice},
};
use server_shared::qunet::server::Server;
use thiserror::Error;

use crate::{
    core::handler::{ClientStateHandle, ConnectionHandler},
    discord::{BotError, state::BotState},
    users::{ComputedRole, DbUser, UserPunishmentType, UsersModule},
};

pub type Context<'a> = poise::Context<'a, Arc<BotState>, BotError>;

pub async fn edit_message(
    ctx: Context<'_>,
    msg: ReplyHandle<'_>,
    content: impl Into<String>,
) -> Result<(), serenity::Error> {
    msg.edit(ctx, CreateReply::default().content(content)).await
}

// pub async fn is_discord_admin(ctx: Context<'_>) -> Result<bool, BotError> {
//     Ok(ctx.author_member().await.is_some_and(|x| x.permissions.is_some_and(|x| x.administrator())))
// }

// pub async fn is_discord_moderator(ctx: Context<'_>) -> Result<bool, BotError> {
//     Ok(ctx
//         .author_member()
//         .await
//         .is_some_and(|x| x.permissions.is_some_and(|x| x.ban_members() || x.manage_roles())))
// }

pub async fn check_admin(ctx: Context<'_>) -> Result<DbUser, BotError> {
    check_linked_and_roles(ctx, |r| r.can_set_password).await
}

pub async fn check_moderator(ctx: Context<'_>) -> Result<DbUser, BotError> {
    check_linked_and_roles(ctx, |r| r.can_moderate()).await
}

pub async fn check_linked_and(
    ctx: Context<'_>,
    f: impl FnOnce(&DbUser) -> bool,
) -> Result<DbUser, BotError> {
    let state = ctx.data();
    let server = state.server()?;

    match get_linked_gd_user(ctx, &server).await? {
        Some(user) => f(&user).then_some(user).ok_or(BotError::NoPermission),
        None => Err(BotError::NoPermission),
    }
}

pub async fn check_linked_and_roles(
    ctx: Context<'_>,
    f: impl FnOnce(&ComputedRole) -> bool,
) -> Result<DbUser, BotError> {
    let state = ctx.data();
    let server = state.server()?;

    let users = server.handler().module::<UsersModule>();

    check_linked_and(ctx, |u| f(&users.compute_from_user(u))).await
}

pub async fn check_linked_and_can_punish(
    ctx: Context<'_>,
    pun_type: UserPunishmentType,
) -> Result<DbUser, BotError> {
    check_linked_and_roles(ctx, |u| match pun_type {
        UserPunishmentType::Ban => u.can_ban,
        UserPunishmentType::Mute => u.can_mute,
        UserPunishmentType::RoomBan => u.can_ban,
    })
    .await
}

pub async fn get_linked_gd_user(
    ctx: Context<'_>,
    server: &Server<ConnectionHandler>,
) -> Result<Option<DbUser>, BotError> {
    let author = ctx.author();
    let users = server.handler().module::<UsersModule>();

    // check if we're not linked
    match users.get_linked_discord_inverse(author.id.get()).await? {
        Some(user) => Ok(Some(user)),
        None => {
            ctx.reply(":x: Not linked to a GD account! Please use the /link command and the Discord Linking option in game to link.").await?;
            Ok(None)
        }
    }
}

#[derive(Debug, Error)]
#[error("Failed to parse duration string")]
pub struct ParseDurationError;

pub fn parse_duration_str(s: &str) -> Result<Duration, ParseDurationError> {
    if s.starts_with("perma") || s.starts_with("Perma") || s.eq_ignore_ascii_case("forever") {
        return Ok(Duration::from_secs(0));
    }

    if !s.contains(' ') {
        return Err(ParseDurationError);
    }

    let mut split = s.split(' ');
    let number = split.next().and_then(|x| x.parse::<u64>().ok()).ok_or(ParseDurationError)?;

    let modifier: u64 = match split.next().unwrap() {
        "second" => 1,
        "seconds" => 1,
        "minute" => 60,
        "minutes" => 60,
        "hour" => 3600,
        "hours" => 3600,
        "day" => 3600 * 24,
        "days" => 3600 * 24,
        "month" => 3600 * 24 * 30,
        "months" => 3600 * 24 * 30,
        "year" => 3600 * 24 * 30 * 12,
        "years" => 3600 * 24 * 30 * 12,
        _ => 0,
    };

    Ok(Duration::from_secs(number * modifier))
}

fn fuzzy_match(target: &str, candidate: &str) -> i64 {
    let matcher = SkimMatcherV2::default();
    matcher.fuzzy_match(target, candidate).unwrap_or(-1)
}

fn wrap_user_autocomplete<'a>(
    query: &str,
    iter: impl Iterator<Item = (&'a str, i32)>,
) -> Vec<AutocompleteChoice> {
    let query_id = query.parse::<i32>().ok();

    let mut choices = iter
        .sorted_by(|a, b| {
            let (a_name, a_id) = a;
            let (b_name, b_id) = b;

            let mut a_score = 0;
            let mut b_score = 0;

            if let Some(qid) = query_id {
                if *a_id == qid {
                    a_score = i64::MAX - 1;
                }
                if *b_id == qid {
                    b_score = i64::MAX - 1;
                }
            }

            // check for exact username match
            if a_name.eq_ignore_ascii_case(query) {
                a_score = i64::MAX - 2;
            }
            if b_name.eq_ignore_ascii_case(query) {
                b_score = i64::MAX - 2;
            }

            // fuzzy match on the username
            if a_score == 0 {
                a_score = fuzzy_match(query, a_name);
            }
            if b_score == 0 {
                b_score = fuzzy_match(query, b_name);
            }

            if a_score != b_score {
                b_score.cmp(&a_score)
            } else {
                // score by id as tiebreaker
                a_id.cmp(b_id)
            }
        })
        .collect::<Vec<_>>();

    choices.dedup();

    choices
        .into_iter()
        .map(|(username, id)| AutocompleteChoice::new(username.to_owned(), id.to_string()))
        .take(10)
        .collect()
}

fn get_online_users_matching(ctx: Context<'_>, partial: &str) -> Vec<ClientStateHandle> {
    let server = ctx.data().server().unwrap();
    let mut clients = server.handler().get_n_clients_matching(partial, 25);

    if let Ok(query_id) = partial.parse::<i32>() {
        if let Some(client) = server.handler().find_client(query_id) {
            clients.push(client);
        }
    }

    clients
}

async fn get_db_users_matching(ctx: Context<'_>, partial: &str) -> Vec<DbUser> {
    let server = ctx.data().server().unwrap();
    let users = server.handler().module::<UsersModule>();

    users.query_matching_users(partial, 50).await.unwrap_or_default()
}

pub async fn online_user_autocomplete(ctx: Context<'_>, partial: &str) -> Vec<AutocompleteChoice> {
    let clients = get_online_users_matching(ctx, partial);
    wrap_user_autocomplete(partial, clients.iter().map(|c| (c.username(), c.account_id())))
}

pub async fn db_user_autocomplete(ctx: Context<'_>, partial: &str) -> Vec<AutocompleteChoice> {
    let users = get_db_users_matching(ctx, partial).await;
    wrap_user_autocomplete(partial, users.iter().map(|u| (u.username(), u.account_id)))
}

pub async fn online_and_db_user_autocomplete(
    ctx: Context<'_>,
    partial: &str,
) -> Vec<AutocompleteChoice> {
    let mut vec = Vec::new();

    let first = get_online_users_matching(ctx, partial);
    vec.extend(first.iter().map(|c| (c.username(), c.account_id())));

    let second = get_db_users_matching(ctx, partial).await;
    vec.extend(second.iter().map(|u| (u.username(), u.account_id)));

    wrap_user_autocomplete(partial, vec.into_iter())
}
