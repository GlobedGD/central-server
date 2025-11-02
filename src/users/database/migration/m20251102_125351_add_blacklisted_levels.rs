use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(BlacklistedLevel::Table)
                    .col(integer(BlacklistedLevel::Id).primary_key())
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(BlacklistedAuthor::Table)
                    .col(integer(BlacklistedAuthor::Id).primary_key())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let mut td = Table::drop();
        td.table(BlacklistedAuthor::Table).table(BlacklistedLevel::Table);
        manager.drop_table(td).await?;

        Ok(())
    }
}

#[derive(Iden)]
enum BlacklistedLevel {
    Table,
    Id,
}

#[derive(Iden)]
enum BlacklistedAuthor {
    Table,
    Id,
}
