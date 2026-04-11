use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(FeaturedLevel::Table)
                    .col(pk_auto(FeaturedLevel::Id))
                    .col(integer(FeaturedLevel::LevelId).unique_key())
                    .col(text(FeaturedLevel::Name))
                    .col(integer(FeaturedLevel::Author))
                    .col(text(FeaturedLevel::AuthorName))
                    .col(big_integer(FeaturedLevel::FeaturedAt))
                    .col(integer(FeaturedLevel::RateTier))
                    .col(integer_null(FeaturedLevel::FeatureDuration))
                    .take(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(QueuedLevel::Table)
                    .col(integer(QueuedLevel::Id).primary_key())
                    .col(integer(QueuedLevel::Priority))
                    .col(text(QueuedLevel::Name))
                    .col(integer(QueuedLevel::Author))
                    .col(text(QueuedLevel::AuthorName))
                    .col(integer(QueuedLevel::RateTier))
                    .col(integer_null(QueuedLevel::FeatureDuration))
                    .take(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SentLevel::Table)
                    .col(pk_auto(SentLevel::Id))
                    .col(integer(SentLevel::LevelId))
                    .col(text(SentLevel::Name))
                    .col(integer(SentLevel::Author))
                    .col(text(SentLevel::AuthorName))
                    .col(integer(SentLevel::RateTier))
                    .col(integer(SentLevel::SentBy))
                    .col(text(SentLevel::Note))
                    .take(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let mut td = Table::drop();
        td.table(FeaturedLevel::Table).table(QueuedLevel::Table).table(SentLevel::Table);
        manager.drop_table(td).await?;

        Ok(())
    }
}

#[derive(Iden)]
enum FeaturedLevel {
    Table,
    Id,
    LevelId,
    Name,
    Author,
    AuthorName,
    FeaturedAt,
    RateTier,
    FeatureDuration,
}

#[derive(Iden)]
enum QueuedLevel {
    Table,
    Id,
    Priority,
    Name,
    Author,
    AuthorName,
    RateTier,
    FeatureDuration,
}

#[derive(Iden)]
enum SentLevel {
    Table,
    Id,
    LevelId,
    Name,
    Author,
    AuthorName,
    RateTier,
    SentBy,
    Note,
}
