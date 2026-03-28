use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PlayerCountLog::Table)
                    .col(big_integer(PlayerCountLog::Timestamp).primary_key())
                    .col(integer(PlayerCountLog::Count))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let mut td = Table::drop();
        td.table(PlayerCountLog::Table);
        manager.drop_table(td).await?;

        Ok(())
    }
}

#[derive(Iden)]
enum PlayerCountLog {
    Table,
    Timestamp,
    Count,
}
