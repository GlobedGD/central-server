use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(WhitelistedName::Table)
                    .col(string(WhitelistedName::Name).primary_key())
                    .extra("WITHOUT ROWID")
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let mut td = Table::drop();
        td.table(WhitelistedName::Table);
        manager.drop_table(td).await
    }
}

#[derive(Iden)]
enum WhitelistedName {
    Table,
    Name,
}
