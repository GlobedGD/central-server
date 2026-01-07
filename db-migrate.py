# Migrates globed2 central server database to globed3 database
import sqlite3
import time
from pathlib import Path
from dataclasses import dataclass

# replace paths with yours
roles_path = "/home/dankpc/Downloads/globed-roles.sqlite" # put None if irrelevant
old_db_path = "/home/dankpc/Downloads/globed-prod.sqlite"
new_db_path = "./db.sqlite"

roles_conn = sqlite3.connect(roles_path) if roles_path else None
old_conn = sqlite3.connect(old_db_path)
new_conn = sqlite3.connect(new_db_path)

roles_cur = roles_conn.cursor() if roles_conn else None
old_cur = old_conn.cursor()
new_cur = new_conn.cursor()

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

def migrate_punishments(users: dict[int, NewUser]) -> list[NewPunishment]:
    punishments: list[NewPunishment] = []

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

    return punishments

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

def push_new_users(users: dict[int, NewUser], puns: list[NewPunishment]) -> None:
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

    new_conn.commit()

new_users = migrate_users()
new_puns = migrate_punishments(new_users)
migrate_discord_links(new_users)
push_new_users(new_users, new_puns)
