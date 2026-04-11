use sea_orm::{EnumIter, Iterable};
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Punishment::Table)
                    .col(pk_auto(Punishment::Id))
                    .col(integer(Punishment::AccountId))
                    .col(enumeration_null(
                        Punishment::Type,
                        Alias::new("type"),
                        PunishmentType::iter(),
                    ))
                    .col(string(Punishment::Reason))
                    .col(big_integer_null(Punishment::ExpiresAt))
                    .col(integer(Punishment::IssuedBy))
                    .col(big_integer_null(Punishment::IssuedAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(User::Table)
                    .col(pk_auto(User::AccountId))
                    .col(integer(User::Cube))
                    .col(integer(User::Color1))
                    .col(integer(User::Color2))
                    .col(integer(User::GlowColor))
                    .col(string_null(User::Username))
                    .col(string_null(User::NameColor))
                    .col(boolean(User::IsWhitelisted).default(false))
                    .col(string_null(User::AdminPasswordHash))
                    .col(string_null(User::Roles))
                    .col(integer_null(User::ActiveMute))
                    .col(integer_null(User::ActiveBan))
                    .col(integer_null(User::ActiveRoomBan))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(AuditLog::Table)
                    .col(pk_auto(AuditLog::Id))
                    .col(integer(AuditLog::AccountId))
                    .col(enumeration(AuditLog::Type, Alias::new("type"), AuditLogType::iter()))
                    .col(big_integer(AuditLog::Timestamp))
                    .col(integer_null(AuditLog::TargetAccountId))
                    .col(string_null(AuditLog::Message))
                    .col(big_integer_null(AuditLog::ExpiresAt))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let mut td = Table::drop();
        td.table(Punishment::Table).table(User::Table).table(AuditLog::Table);
        manager.drop_table(td).await?;

        Ok(())
    }
}

#[derive(Iden, EnumIter)]
pub enum PunishmentType {
    #[iden = "mute"]
    Mute,
    #[iden = "ban"]
    Ban,
    #[iden = "roomban"]
    RoomBan,
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
    Cube,
    Color1,
    Color2,
    GlowColor,
    Username,
    NameColor,
    IsWhitelisted,
    AdminPasswordHash,
    Roles,
    ActiveMute,
    ActiveBan,
    ActiveRoomBan,
}

#[derive(Iden, EnumIter)]
pub enum AuditLogType {
    #[iden = "kick"]
    Kick,
    #[iden = "notice"]
    Notice,
    #[iden = "mute"]
    Mute,
    #[iden = "editmute"]
    EditMute,
    #[iden = "unmute"]
    Unmute,
    #[iden = "ban"]
    Ban,
    #[iden = "editban"]
    EditBan,
    #[iden = "unban"]
    Unban,
    #[iden = "roomban"]
    RoomBan,
    #[iden = "editroomban"]
    EditRoomBan,
    #[iden = "roomunban"]
    RoomUnban,
    #[iden = "editroles"]
    EditRoles,
    #[iden = "editpassword"]
    EditPassword,
}

#[derive(Iden)]
pub enum AuditLog {
    Table,
    Id,
    AccountId, // the account that performed the action, 0 if system action
    Type,
    Timestamp,
    TargetAccountId, // applies to all, target of the punishment/action
    Message, // for notices/kicks it's the message, for punishments it's the reason, for editroles it's the rolediff (string in format "+role1,-role2")
    ExpiresAt, // applies to mutes/bans/roombans
}
