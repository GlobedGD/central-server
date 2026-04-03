use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(QueuedLevel::Table)
                    .add_column(big_integer_null(QueuedLevel::QueuedAt))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(QueuedLevel::Table)
                    .drop_column(QueuedLevel::QueuedAt)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum QueuedLevel {
    Table,
    QueuedAt,
}
