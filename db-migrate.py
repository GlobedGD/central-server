# Migrates globed2 central server database to globed3 database
import sqlite3
import time
import os
import requests
from pathlib import Path
from dataclasses import dataclass

# replace paths with yours
roles_path = "/home/dankpc/Downloads/globed-roles.sqlite" # put None if irrelevant
old_db_path = "/home/dankpc/Downloads/globed-prod.sqlite"
new_db_path = "./db.sqlite"
feat_db_path = "./features.sqlite"

roles_conn = sqlite3.connect(roles_path) if roles_path else None
old_conn = sqlite3.connect(old_db_path)
new_conn = sqlite3.connect(new_db_path)
feat_conn = sqlite3.connect(feat_db_path)

roles_cur = roles_conn.cursor() if roles_conn else None
old_cur = old_conn.cursor()
new_cur = new_conn.cursor()
feat_cur = feat_conn.cursor()

@dataclass
class OldDiscordRole:
    id: str
    discord_id: int

@dataclass
class OldDiscordLink:
    discord_id: int
    gd_id: int

@dataclass
class OldUser:
    id: int
    username: str
    name_color: str
    whitelisted: bool
    roles: list[str]
    admin_password_hash: str
    # dont migrate punishments at this stage

@dataclass
class NewUser:
    id: int
    cube: int
    color1: int
    color2: int
    glow_color: int
    username: str
    name_color: str
    whitelisted: bool
    admin_password_hash: str
    roles: list[str]
    active_mute: int
    active_ban: int
    active_room_ban: int
    discord_id: int | None

@dataclass
class OldPunishment:
    id: int
    account_id: int
    type: str
    reason: str
    expires_at: int
    issued_by: int
    issued_at: int
    type2: str | None

@dataclass
class NewPunishment:
    id: int
    account_id: int
    type: str
    reason: str
    expires_at: int
    issued_by: int
    issued_at: int

@dataclass
class AuditLog:
    account_id: int
    type: str
    timestamp: int
    target_account_id: int | None
    message: str | None
    expires_at: int | None

@dataclass
class OldFeaturedLevel:
    id: int
    level_id: int
    picked_at: int
    picked_by: int
    is_active: int
    rate_tier: int

@dataclass
class NewFeaturedLevel:
    level_id: int
    name: str
    author: int
    author_name: str
    featured_at: int
    rate_tier: int
    feature_duration: None

# Fetches level data, returns level name, author id and author name
def fetch_level_data(level_id: int) -> tuple[str, int, str]:
    token = os.environ.get("GDPROXY_TOKEN", "")
    if not token:
        return ("", 0, "")

    r = requests.post(
        f"https://gdproxy.globed.dev/database/getGJLevels21.php",
        data={
            "secret": "Wmfd2893gb7",
            "str": str(level_id),
            "type": 0,
        },
        headers={
            "Authorization": f"{token}",
            "Content-Type": "application/x-www-form-urlencoded"
        }
    )
    r.raise_for_status()

    outer_parts = r.text.split("#")
    data, extra = outer_parts[:2]

    parts = {}
    ckey = None
    for part in data.split(":"):
        if ckey is None:
            ckey = part
        else:
            parts[ckey] = part
            ckey = None

    player_id = int(parts["6"])

    # extra should be formatted like userid:username:accountid
    extra_parts = extra.split(":")
    assert len(extra_parts) >= 3
    assert int(extra_parts[0]) == player_id

    _, username, account_id = extra_parts[:3]

    return (parts["2"], int(account_id), username)

def migrate_users() -> dict[int, NewUser]:
    old_users: dict[int, OldUser] = {}
    new_users: dict[int, NewUser] = {}

    # fetch old users
    for uid, uname, ncolor, whitelisted, roles_str, admin_phash in old_cur.execute("SELECT account_id, user_name, name_color, is_whitelisted, user_roles, admin_password_hash FROM users").fetchall():
        roles = roles_str.split(",") if roles_str else []
        old_users[uid] = OldUser(
            id=uid,
            username=uname,
            name_color=ncolor,
            whitelisted=bool(whitelisted),
            roles=roles,
            admin_password_hash=admin_phash
        )

    # convert to new users
    for user in old_users.values():
        new_users[user.id] = NewUser(
            id=user.id,
            cube=0,
            color1=0,
            color2=0,
            glow_color=0,
            username=user.username,
            name_color=user.name_color,
            whitelisted=user.whitelisted,
            admin_password_hash=user.admin_password_hash,
            roles=user.roles,
            active_mute=0,
            active_ban=0,
            active_room_ban=0,
            discord_id=None
        )

    return new_users

def migrate_punishments(users: dict[int, NewUser]) -> tuple[list[NewPunishment], list[AuditLog]]:
    punishments: list[NewPunishment] = []
    logs: list[AuditLog] = []

    for pid, aid, ptype, reason, expires_at, issued_by, issued_at, type2 in old_cur.execute("SELECT punishment_id, account_id, type, reason, expires_at, issued_by, issued_at, type2 FROM punishments").fetchall():
        type = type2 or ptype

        punishments.append(NewPunishment(
            id=pid,
            account_id=aid,
            type=type,
            reason=reason,
            expires_at=expires_at,
            issued_by=issued_by,
            issued_at=issued_at,
        ))

    # set active puns
    now = int(time.time())
    migrated = 0

    for p in punishments:
        # check if it's expired
        if p.expires_at != 0 and p.expires_at < now:
            continue

        if p.account_id in users:
            migrated += 1
            user = users[p.account_id]
            if p.type == "mute":
                user.active_mute = p.id
            elif p.type == "ban":
                user.active_ban = p.id
            elif p.type == "roomban":
                user.active_room_ban = p.id
            else:
                raise ValueError(f"Unknown punishment type: {p.type}")
        else:
            print(f"Warning: Punishment ID {p.id} for non-existent user ID {p.account_id}")

    print(f"Migrated {migrated} active punishments")

    # create audit logs for all past punishments
    for p in punishments:
        logs.append(
            AuditLog(
                account_id=p.issued_by or 0,
                type=p.type,
                timestamp=p.issued_at or 0,
                target_account_id=p.account_id,
                message=p.reason,
                expires_at=p.expires_at
            )
        )

    print(f"Migrated {len(logs)} audit logs")

    return punishments, logs

def migrate_discord_links(users: dict[int, NewUser]) -> None:
    if not roles_cur:
        return

    old_roles: list[OldDiscordRole] = []
    old_links: list[OldDiscordLink] = []

    for strid, did in roles_cur.execute("SELECT * from roles").fetchall():
        old_roles.append(OldDiscordRole(id=strid, discord_id=did))

    for did, gdid in roles_cur.execute("SELECT * from linked_users").fetchall():
        old_links.append(OldDiscordLink(discord_id=did, gd_id=gdid))

    for link in old_links:
        if link.gd_id in new_users:
            # print(f"Linking GD ID {link.gd_id} to Discord ID {link.discord_id}")
            users[link.gd_id].discord_id = link.discord_id
        else:
            print(f"Warning: Discord link for GD ID {link.gd_id} ({link.discord_id}) has no matching user")

def migrate_features() -> list[NewFeaturedLevel]:
    old_features: list[OldFeaturedLevel] = []
    new_features: list[NewFeaturedLevel] = []

    for (id, level_id, picked_at, picked_by, is_active, rate_tier) in old_cur.execute("SELECT * from featured_levels").fetchall():
        old_features.append(OldFeaturedLevel(
            id=id,
            level_id=level_id,
            picked_at=picked_at,
            picked_by=picked_by,
            is_active=is_active,
            rate_tier=rate_tier
        ))

    print(f"Collected {len(old_features)} featured levels, starting to fetch data..")

    for level in old_features:
        level_name, author, author_name = fetch_level_data(level.level_id)
        new_features.append(NewFeaturedLevel(
            level_id=level.level_id,
            name=level_name,
            author=author,
            author_name=author_name,
            featured_at=level.picked_at,
            rate_tier=level.rate_tier,
            feature_duration=None
        ))
        print(f"\r[{len(new_features)}/{len(old_features)}] Fetched {level_name} ({level.level_id}) by {author_name} ({author})" + " " * 20, end="")
        time.sleep(0.2)

    print()

    return new_features

def push_new_users(users: dict[int, NewUser], puns: list[NewPunishment], logs: list[AuditLog]) -> None:
    # remove all data in the new database
    new_cur.execute("DELETE FROM user")
    new_cur.execute("DELETE FROM punishment")
    new_cur.execute("DELETE FROM audit_log")

    for user in users.values():
        roles_str = ",".join(user.roles)
        res = new_cur.execute(
            "INSERT INTO user (account_id, cube, color1, color2, glow_color, username, name_color, is_whitelisted, admin_password_hash, roles, active_mute, active_ban, active_room_ban, discord_id)" \
            "VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?)",
            (user.id, user.cube, user.color1, user.color2, user.glow_color, user.username, int(user.whitelisted), user.admin_password_hash, roles_str, user.active_mute, user.active_ban, user.active_room_ban, user.discord_id)
        )

    for pun in puns:
        new_cur.execute(
            "INSERT INTO punishment (id, account_id, type, reason, expires_at, issued_by, issued_at)" \
            "VALUES (?, ?, ?, ?, ?, ?, ?)",
            (pun.id, pun.account_id, pun.type, pun.reason, pun.expires_at, pun.issued_by or 0, pun.issued_at)
        )

    for log in logs:
        new_cur.execute(
            "INSERT INTO audit_log (account_id, type, timestamp, target_account_id, message, expires_at)" \
            "VALUES (?, ?, ?, ?, ?, ?)",
            (log.account_id, log.type, log.timestamp, log.target_account_id, log.message, log.expires_at)
        )

    new_conn.commit()

def push_features(features: list[NewFeaturedLevel]) -> None:
    if not feat_cur:
        return

    feat_cur.execute("DELETE FROM featured_level")

    for feat in features:
        print(f"Inserting {feat}")
        feat_cur.execute(
            "INSERT INTO featured_level (level_id, name, author, author_name, featured_at, rate_tier, feature_duration)" \
            "VALUES (?, ?, ?, ?, ?, ?, ?)",
            (feat.level_id, feat.name, feat.author, feat.author_name, feat.featured_at, feat.rate_tier, feat.feature_duration)
        )

    feat_conn.commit()

new_users = migrate_users()
new_puns, new_logs = migrate_punishments(new_users)
migrate_discord_links(new_users)
push_new_users(new_users, new_puns, new_logs)

features = migrate_features()
push_features(features)