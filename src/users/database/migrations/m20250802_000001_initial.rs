use sea_orm_migration::{
    prelude::*,
    schema::{boolean, date_time_null, integer, integer_null, pk_auto, string, string_null},
};

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20250802_000001_initial"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Punishment::Table)
                    .col(pk_auto(Punishment::Id))
                    .col(integer(Punishment::AccountId))
                    .col(string(Punishment::Type))
                    .col(string(Punishment::Reason))
                    .col(date_time_null(Punishment::ExpiresAt).null())
                    .col(integer(Punishment::IssuedBy))
                    .col(date_time_null(Punishment::IssuedAt).null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(User::Table)
                    .col(pk_auto(User::AccountId))
                    .col(string_null(User::Username))
                    .col(string_null(User::NameColor))
                    .col(boolean(User::IsWhitelisted).default(false))
                    .col(string_null(User::AdminPasswordHash))
                    .col(string_null(User::UserRoles))
                    .col(integer_null(User::ActiveMute))
                    .col(integer_null(User::ActiveBan))
                    .col(integer_null(User::ActiveRoomBan))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let mut td = Table::drop();
        td.table(Punishment::Table);
        manager.drop_table(td).await?;

        let mut td = Table::drop();
        td.table(User::Table);
        manager.drop_table(td).await?;

        Ok(())
    }
}

#[derive(Iden)]
pub enum Punishment {
    Table,
    Id,
    AccountId,
    Type,
    Reason,
    ExpiresAt,
    IssuedBy,
    IssuedAt,
}

#[derive(Iden)]
pub enum User {
    Table,
    AccountId,
    Username,
    NameColor,
    IsWhitelisted,
    AdminPasswordHash,
    UserRoles,
    ActiveMute,
    ActiveBan,
    ActiveRoomBan,
}
