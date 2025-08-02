use sea_orm_migration::prelude::*;

// generate using `sea-orm-cli migrate generate <name>`
mod m20250802_000001_initial;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20250802_000001_initial::Migration)]
    }
}
