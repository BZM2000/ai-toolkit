use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Clone, FromRow)]
pub struct GlossaryTermRow {
    pub id: Uuid,
    pub source_term: String,
    pub target_term: String,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, FromRow)]
pub struct JournalTopicRow {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, FromRow)]
pub struct JournalReferenceRow {
    pub id: Uuid,
    pub journal_name: String,
    pub reference_mark: Option<String>,
    pub low_bound: f64,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, FromRow)]
pub struct JournalTopicScoreRow {
    pub journal_id: Uuid,
    pub topic_id: Uuid,
    pub score: i16,
}
