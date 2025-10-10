use sea_orm_migration::prelude::*;

// generate using `sea-orm-cli migrate generate <name>` in database/ folder
mod m20250928_144510_add_featured;
mod m20251010_160043_add_blacklisted;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20250928_144510_add_featured::Migration),
            Box::new(m20251010_160043_add_blacklisted::Migration),
        ]
    }
}
