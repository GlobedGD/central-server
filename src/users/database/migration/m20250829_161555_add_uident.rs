use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Uident::Table)
                    .col(pk_auto(Uident::Id))
                    .col(integer(Uident::AccountId))
                    .col(text(Uident::Ident))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let mut td = Table::drop();
        td.table(Uident::Table);
        manager.drop_table(td).await?;

        Ok(())
    }
}

#[derive(Iden)]
enum Uident {
    Table,
    Id,
    AccountId,
    Ident,
}
