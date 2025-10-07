use sqlx::PgPool;

use super::models::{GlossaryTermRow, JournalReferenceRow, JournalTopicRow, JournalTopicScoreRow};

pub async fn fetch_glossary_terms(pool: &PgPool) -> sqlx::Result<Vec<GlossaryTermRow>> {
    sqlx::query_as::<_, GlossaryTermRow>(
        "SELECT id, source_term, target_term, notes, created_at, updated_at FROM glossary_terms ORDER BY source_term",
    )
    .fetch_all(pool)
    .await
}

pub async fn fetch_journal_topics(pool: &PgPool) -> sqlx::Result<Vec<JournalTopicRow>> {
    sqlx::query_as::<_, JournalTopicRow>(
        "SELECT id, name, description, created_at FROM journal_topics ORDER BY name",
    )
    .fetch_all(pool)
    .await
}

pub async fn fetch_journal_references(pool: &PgPool) -> sqlx::Result<Vec<JournalReferenceRow>> {
    sqlx::query_as::<_, JournalReferenceRow>(
        "SELECT id, journal_name, reference_mark, low_bound, notes, created_at, updated_at FROM journal_reference_entries ORDER BY journal_name",
    )
    .fetch_all(pool)
    .await
}

pub async fn fetch_journal_topic_scores(pool: &PgPool) -> sqlx::Result<Vec<JournalTopicScoreRow>> {
    sqlx::query_as::<_, JournalTopicScoreRow>(
        "SELECT journal_id, topic_id, score FROM journal_topic_scores",
    )
    .fetch_all(pool)
    .await
}
