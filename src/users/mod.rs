use std::cmp::Reverse;

use crate::{
    core::module::{ModuleInitResult, ServerModule},
    users::config::Role,
};

mod config;
mod database;

pub use config::Config;
use database::UsersDb;
pub use database::{DatabaseError, DatabaseResult, DbUser, UserPunishment, UserPunishmentType};

#[derive(Default)]
pub struct ComputedRole {
    pub priority: i32,
    pub roles: heapless::Vec<u8, 32>,
    pub name_color: String,

    pub can_kick: bool,
    pub can_mute: bool,
    pub can_ban: bool,
}

pub struct UsersModule {
    db: UsersDb,
    roles: Vec<Role>, // index = numeric role ID
}

impl UsersModule {
    pub async fn get_user(&self, account_id: i32) -> DatabaseResult<Option<DbUser>> {
        self.db.get_user(account_id).await
    }

    pub async fn update_username(&self, account_id: i32, new_username: &str) -> DatabaseResult<()> {
        self.db.update_username(account_id, new_username).await
    }

    pub fn get_role(&self, id: u8) -> Option<&Role> {
        self.roles.get(id as usize)
    }

    pub fn get_role_by_str_id(&self, id: &str) -> Option<(usize, &Role)> {
        self.roles.iter().enumerate().find(|(_, role)| role.id == id)
    }

    pub fn get_roles(&self) -> &[Role] {
        &self.roles
    }

    pub fn compute_from_roles<'a>(
        &'a self,
        mut iter: impl Iterator<Item = &'a str>,
    ) -> ComputedRole {
        // start with a baseline user role with minimum priority and no permissions
        let mut out_role = ComputedRole {
            priority: i32::MIN,
            ..Default::default()
        };

        let mut can_mute = None;
        let mut can_kick = None;
        let mut can_ban = None;

        while let Some((role_index, role)) = iter.next().and_then(|s| self.get_role_by_str_id(s)) {
            // determine if this role is stronger than the current strongest role
            let is_weaker = role.priority < out_role.priority;

            let apply_permission = |out: &mut Option<bool>, r#in: Option<bool>| {
                if let Some(val) = r#in {
                    // assign only if the current value is None or if the new role is stronger
                    if out.is_none() || !is_weaker {
                        *out = Some(val);
                    }
                }
            };

            apply_permission(&mut can_mute, role.can_mute);
            apply_permission(&mut can_kick, role.can_kick);
            apply_permission(&mut can_ban, role.can_ban);

            out_role.priority = role.priority;
            let _ = out_role.roles.push(role_index as u8);

            if !is_weaker {
                out_role.name_color.clone_from(&role.name_color);
            }
        }

        out_role.can_mute = can_mute.unwrap_or(false);
        out_role.can_kick = can_kick.unwrap_or(false);
        out_role.can_ban = can_ban.unwrap_or(false);

        out_role
    }

    /// Converts a slice of role IDs into a comma-separated string of string IDs
    pub fn make_role_string(&self, roles: &[u8]) -> String {
        if roles.is_empty() {
            return String::new();
        }

        itertools::join(roles.iter().filter_map(|id| self.get_role(*id).map(|role| &role.id)), ",")
    }
}

impl ServerModule for UsersModule {
    type Config = Config;

    async fn new(config: &Self::Config) -> ModuleInitResult<Self> {
        let db = UsersDb::new(&config.database_url, config.database_pool_size).await?;
        db.run_migrations().await?;

        let mut roles = Vec::new();
        for role in config.roles.iter() {
            roles.push(role.clone());
        }

        // sort roles by priority descending
        roles.sort_by_key(|role| Reverse(role.priority));

        Ok(Self { db, roles })
    }

    fn id() -> &'static str {
        "users"
    }

    fn name() -> &'static str {
        "User management"
    }
}
