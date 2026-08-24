//! Voice prints, so a speaker can be recognised in a later call.
//!
//! Its own migration rather than an edit to the initial schema: by the time this
//! was added there were live databases with calls in them, and a collapsed
//! history only stays honest while nothing has shipped.
//!
//! The embedding is stored as bytes rather than a native array type. SQLite has
//! no array, and a little-endian `f32` blob is the same code on both backends --
//! the alternative was a per-backend branch for no gain.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(CallSpeakers::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(CallSpeakers::CallId).text().not_null())
                    .col(ColumnDef::new(CallSpeakers::SpeakerId).integer().not_null())
                    .col(ColumnDef::new(CallSpeakers::Embedding).binary().not_null())
                    // Null until somebody names the voice; naming is what makes
                    // it an exemplar for later calls.
                    .col(ColumnDef::new(CallSpeakers::Name).text())
                    .col(ColumnDef::new(CallSpeakers::CreatedAt).text().not_null())
                    // One row per speaker per call: a call is reprocessed often
                    // and each pass should replace, not accumulate.
                    .primary_key(
                        Index::create()
                            .col(CallSpeakers::CallId)
                            .col(CallSpeakers::SpeakerId),
                    )
                    // Voice prints are biometric data: deleting a call must take
                    // them with it rather than leaving them orphaned.
                    .foreign_key(
                        ForeignKey::create()
                            .from(CallSpeakers::Table, CallSpeakers::CallId)
                            .to(Calls::Table, Calls::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Exemplar lookup reads only the named rows, and there are far fewer of
        // those than of calls.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_call_speakers_name")
                    .table(CallSpeakers::Table)
                    .col(CallSpeakers::Name)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(CallSpeakers::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum CallSpeakers {
    Table,
    CallId,
    SpeakerId,
    Embedding,
    Name,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Calls {
    Table,
    Id,
}
