use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(BlacklistedFeatureAuthor::Table)
                    .col(integer(BlacklistedFeatureAuthor::Id).primary_key().not_null())
                    .take(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(BlacklistedFeatureAuthor::Table).to_owned()).await
    }
}

#[derive(Iden)]
enum BlacklistedFeatureAuthor {
    Table,
    Id,
}
