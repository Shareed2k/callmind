use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend};

/// Run a statement that has no portable builder form.
async fn exec(manager: &SchemaManager<'_>, sql: &str) -> Result<(), DbErr> {
    manager
        .get_connection()
        .execute_unprepared(sql)
        .await
        .map(|_| ())
}

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Timestamps are RFC 3339 strings rather than a native timestamp type. That is
/// what the repositories read and write, and it behaves identically on both
/// backends.
fn timestamp(name: impl IntoIden) -> ColumnDef {
    ColumnDef::new(name).text().not_null().take()
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Organizations::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Organizations::Id).text().primary_key())
                    .col(ColumnDef::new(Organizations::Name).text().not_null())
                    .col(timestamp(Organizations::CreatedAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Calls::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Calls::Id).text().primary_key())
                    .col(ColumnDef::new(Calls::OrganizationId).text().not_null())
                    .col(ColumnDef::new(Calls::ExternalId).text())
                    .col(ColumnDef::new(Calls::Direction).text().not_null())
                    .col(ColumnDef::new(Calls::PhoneFrom).text())
                    .col(ColumnDef::new(Calls::PhoneTo).text())
                    .col(ColumnDef::new(Calls::StartedAt).text())
                    .col(ColumnDef::new(Calls::EndedAt).text())
                    .col(ColumnDef::new(Calls::DurationMs).big_integer())
                    .col(ColumnDef::new(Calls::ProcessingStatus).text().not_null())
                    .col(
                        ColumnDef::new(Calls::IsFavorite)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(Calls::Tags).text().not_null().default("[]"))
                    .col(timestamp(Calls::CreatedAt))
                    .col(timestamp(Calls::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Calls::Table, Calls::OrganizationId)
                            .to(Organizations::Table, Organizations::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_calls_org_created")
                    .table(Calls::Table)
                    .col(Calls::OrganizationId)
                    .col((Calls::CreatedAt, IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_calls_status")
                    .table(Calls::Table)
                    .col(Calls::ProcessingStatus)
                    .to_owned(),
            )
            .await?;
        // Import is resumable because a re-import of the same filename is
        // rejected here rather than silently duplicated.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_calls_external_id_unique")
                    .table(Calls::Table)
                    .col(Calls::OrganizationId)
                    .col(Calls::ExternalId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_calls_favorite")
                    .table(Calls::Table)
                    .col(Calls::IsFavorite)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(CallRecordings::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(CallRecordings::Id).text().primary_key())
                    .col(
                        ColumnDef::new(CallRecordings::CallId)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(CallRecordings::StorageKey).text().not_null())
                    .col(ColumnDef::new(CallRecordings::MimeType).text().not_null())
                    .col(
                        ColumnDef::new(CallRecordings::FileSizeBytes)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(CallRecordings::Sha256).text().not_null())
                    .col(ColumnDef::new(CallRecordings::DurationMs).big_integer())
                    .col(ColumnDef::new(CallRecordings::Channels).integer())
                    .col(ColumnDef::new(CallRecordings::SampleRate).integer())
                    .col(timestamp(CallRecordings::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(CallRecordings::Table, CallRecordings::CallId)
                            .to(Calls::Table, Calls::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_recordings_sha256")
                    .table(CallRecordings::Table)
                    .col(CallRecordings::Sha256)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Jobs::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Jobs::Id).text().primary_key())
                    .col(ColumnDef::new(Jobs::CallId).text())
                    .col(ColumnDef::new(Jobs::Kind).text().not_null())
                    .col(ColumnDef::new(Jobs::Payload).text().not_null())
                    .col(ColumnDef::new(Jobs::Status).text().not_null())
                    .col(
                        ColumnDef::new(Jobs::Priority)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Jobs::Attempt)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Jobs::MaxAttempts)
                            .integer()
                            .not_null()
                            .default(3),
                    )
                    .col(timestamp(Jobs::RunAfter))
                    .col(ColumnDef::new(Jobs::LockedAt).text())
                    .col(ColumnDef::new(Jobs::LockedBy).text())
                    .col(ColumnDef::new(Jobs::LastError).text())
                    .col(timestamp(Jobs::CreatedAt))
                    .col(ColumnDef::new(Jobs::CompletedAt).text())
                    .foreign_key(
                        ForeignKey::create()
                            .from(Jobs::Table, Jobs::CallId)
                            .to(Calls::Table, Calls::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // Covers the leasing query: status, then due time, then priority.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_jobs_fetch")
                    .table(Jobs::Table)
                    .col(Jobs::Status)
                    .col(Jobs::RunAfter)
                    .col((Jobs::Priority, IndexOrder::Desc))
                    .col((Jobs::CreatedAt, IndexOrder::Asc))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_jobs_call_id")
                    .table(Jobs::Table)
                    .col(Jobs::CallId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(CallTranscripts::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(CallTranscripts::CallId).text().primary_key())
                    .col(
                        ColumnDef::new(CallTranscripts::TranscriptJson)
                            .text()
                            .not_null(),
                    )
                    .col(timestamp(CallTranscripts::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(CallTranscripts::Table, CallTranscripts::CallId)
                            .to(Calls::Table, Calls::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(CallAnalyses::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(CallAnalyses::Id).text().primary_key())
                    .col(
                        ColumnDef::new(CallAnalyses::CallId)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(CallAnalyses::Title).text().not_null())
                    .col(ColumnDef::new(CallAnalyses::Summary).text().not_null())
                    .col(ColumnDef::new(CallAnalyses::Reason).text())
                    .col(ColumnDef::new(CallAnalyses::Resolution).text())
                    .col(
                        ColumnDef::new(CallAnalyses::Resolved)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(ColumnDef::new(CallAnalyses::CustomerIntent).text())
                    .col(
                        ColumnDef::new(CallAnalyses::SentimentScore)
                            .double()
                            .not_null()
                            .default(0.0),
                    )
                    .col(ColumnDef::new(CallAnalyses::MetricsJson).text().not_null())
                    .col(
                        ColumnDef::new(CallAnalyses::FullAnalysisJson)
                            .text()
                            .not_null(),
                    )
                    .col(timestamp(CallAnalyses::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(CallAnalyses::Table, CallAnalyses::CallId)
                            .to(Calls::Table, Calls::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        for (name, col) in [
            ("idx_analyses_sentiment", CallAnalyses::SentimentScore),
            ("idx_analyses_resolved", CallAnalyses::Resolved),
        ] {
            manager
                .create_index(
                    Index::create()
                        .if_not_exists()
                        .name(name)
                        .table(CallAnalyses::Table)
                        .col(col)
                        .to_owned(),
                )
                .await?;
        }

        // One generic surface for every plugin, so adding one needs no migration.
        manager
            .create_table(
                Table::create()
                    .table(CallPluginResults::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(CallPluginResults::CallId).text().not_null())
                    .col(ColumnDef::new(CallPluginResults::Plugin).text().not_null())
                    .col(
                        ColumnDef::new(CallPluginResults::PayloadJson)
                            .text()
                            .not_null(),
                    )
                    .col(timestamp(CallPluginResults::CreatedAt))
                    .primary_key(
                        Index::create()
                            .col(CallPluginResults::CallId)
                            .col(CallPluginResults::Plugin),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(CallPluginResults::Table, CallPluginResults::CallId)
                            .to(Calls::Table, Calls::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        self.create_backend_specific(manager).await?;
        self.seed_default_organization(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // FTS objects first: they reference the tables below.
        if manager.get_database_backend() == DbBackend::Sqlite {
            exec(manager, "DROP TABLE IF EXISTS fts_calls").await?;
        }
        for table in [
            "call_plugin_results",
            "call_analyses",
            "call_transcripts",
            "jobs",
            "call_recordings",
            "calls",
            "organizations",
        ] {
            exec(manager, &format!("DROP TABLE IF EXISTS {table}")).await?;
        }
        Ok(())
    }
}

impl Migration {
    /// The two pieces of schema that have no portable form.
    async fn create_backend_specific(&self, manager: &SchemaManager<'_>) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();

        match backend {
            DbBackend::Sqlite => {
                // FTS5 is a virtual table; there is no builder for it.
                exec(
                    manager,
                    r#"
                    CREATE VIRTUAL TABLE IF NOT EXISTS fts_calls USING fts5(
                        call_id UNINDEXED,
                        organization_id UNINDEXED,
                        title, summary, transcript, topics, entities, reason, resolution,
                        tokenize = 'unicode61 remove_diacritics 2'
                    )
                    "#,
                )
                .await?;

                // SQLite's ALTER TABLE accepts only VIRTUAL generated columns.
                exec(
                    manager,
                    r#"
                    ALTER TABLE call_transcripts
                      ADD COLUMN primary_language TEXT
                      GENERATED ALWAYS AS (
                        lower(json_extract(transcript_json, '$.languages[0].language'))
                      ) VIRTUAL
                    "#,
                )
                .await?;
            }
            _ => {
                // Postgres: a tsvector column with a GIN index plays the role of
                // the FTS5 table. Kept as its own table so the row shape matches
                // what the SQLite path stores.
                exec(
                    manager,
                    r#"
                    CREATE TABLE IF NOT EXISTS fts_calls (
                        call_id TEXT PRIMARY KEY,
                        organization_id TEXT NOT NULL,
                        title TEXT NOT NULL DEFAULT '',
                        summary TEXT NOT NULL DEFAULT '',
                        transcript TEXT NOT NULL DEFAULT '',
                        topics TEXT NOT NULL DEFAULT '',
                        entities TEXT NOT NULL DEFAULT '',
                        reason TEXT NOT NULL DEFAULT '',
                        resolution TEXT NOT NULL DEFAULT '',
                        document tsvector GENERATED ALWAYS AS (
                            to_tsvector('simple',
                                coalesce(title,'') || ' ' || coalesce(summary,'') || ' ' ||
                                coalesce(transcript,'') || ' ' || coalesce(topics,'') || ' ' ||
                                coalesce(entities,'') || ' ' || coalesce(reason,'') || ' ' ||
                                coalesce(resolution,''))
                        ) STORED
                    )
                    "#,
                )
                .await?;
                exec(manager, "CREATE INDEX IF NOT EXISTS idx_fts_calls_document ON fts_calls USING GIN (document)"
                )
                .await?;

                // Postgres supports only STORED generated columns.
                exec(
                    manager,
                    r#"
                    ALTER TABLE call_transcripts
                      ADD COLUMN IF NOT EXISTS primary_language TEXT
                      GENERATED ALWAYS AS (
                        lower(transcript_json::jsonb -> 'languages' -> 0 ->> 'language')
                      ) STORED
                    "#,
                )
                .await?;
            }
        }

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_transcripts_language")
                    .table(CallTranscripts::Table)
                    .col(Alias::new("primary_language"))
                    .to_owned(),
            )
            .await
    }

    /// The single organization every default deployment uses.
    async fn seed_default_organization(&self, manager: &SchemaManager<'_>) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let sql = match backend {
            DbBackend::Sqlite => {
                "INSERT OR IGNORE INTO organizations (id, name, created_at) VALUES \
                 ('00000000-0000-0000-0000-000000000001', 'Default Organization', \
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))"
            }
            _ => {
                "INSERT INTO organizations (id, name, created_at) VALUES \
                 ('00000000-0000-0000-0000-000000000001', 'Default Organization', \
                 to_char(now() at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')) \
                 ON CONFLICT (id) DO NOTHING"
            }
        };
        exec(manager, sql).await
    }
}

#[derive(DeriveIden)]
enum Organizations {
    Table,
    Id,
    Name,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Calls {
    Table,
    Id,
    OrganizationId,
    ExternalId,
    Direction,
    PhoneFrom,
    PhoneTo,
    StartedAt,
    EndedAt,
    DurationMs,
    ProcessingStatus,
    IsFavorite,
    Tags,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum CallRecordings {
    Table,
    Id,
    CallId,
    StorageKey,
    MimeType,
    FileSizeBytes,
    Sha256,
    DurationMs,
    Channels,
    SampleRate,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Jobs {
    Table,
    Id,
    CallId,
    Kind,
    Payload,
    Status,
    Priority,
    Attempt,
    MaxAttempts,
    RunAfter,
    LockedAt,
    LockedBy,
    LastError,
    CreatedAt,
    CompletedAt,
}

#[derive(DeriveIden)]
enum CallTranscripts {
    Table,
    CallId,
    TranscriptJson,
    CreatedAt,
}

#[derive(DeriveIden)]
enum CallAnalyses {
    Table,
    Id,
    CallId,
    Title,
    Summary,
    Reason,
    Resolution,
    Resolved,
    CustomerIntent,
    SentimentScore,
    MetricsJson,
    FullAnalysisJson,
    CreatedAt,
}

#[derive(DeriveIden)]
enum CallPluginResults {
    Table,
    CallId,
    Plugin,
    PayloadJson,
    CreatedAt,
}
