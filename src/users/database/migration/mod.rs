use sea_orm_migration::prelude::*;

// generate using `sea-orm-cli migrate generate <name>`
mod m20250802_000001_initial;
mod m20250829_161555_add_uident;
mod m20250910_214142_add_discord_id;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20250802_000001_initial::Migration),
            Box::new(m20250829_161555_add_uident::Migration),
            Box::new(m20250910_214142_add_discord_id::Migration),
        ]
    }
}
