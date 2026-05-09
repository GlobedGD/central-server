use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Uident::Table)
                    .add_column(boolean(Uident::Whitelisted).default(false))
                    .take(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter().table(Uident::Table).drop_column(Uident::Whitelisted).take(),
            )
            .await
    }
}

#[derive(Iden)]
enum Uident {
    Table,
    Whitelisted,
}
