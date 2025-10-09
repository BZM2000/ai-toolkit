use std::{collections::HashMap, time::Duration as StdDuration};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Row};
use tracing::warn;
use uuid::Uuid;

use crate::usage;

pub const HISTORY_RETENTION_HOURS: i64 = 24;
const HISTORY_LIMIT: i64 = 50;
const POLL_WINDOW: Duration = Duration::hours(HISTORY_RETENTION_HOURS);

#[derive(Debug, Clone)]
pub struct ModuleMetadata {
    pub key: &'static str,
    pub label: &'static str,
    pub tool_path: &'static str,
    pub status_path_prefix: &'static str,
    pub supports_downloads: bool,
}

const MODULES: &[ModuleMetadata] = &[
    ModuleMetadata {
        key: usage::MODULE_SUMMARIZER,
        label: "摘要与翻译",
        tool_path: "/tools/summarizer",
        status_path_prefix: "/api/summarizer/jobs/",
        supports_downloads: true,
    },
    ModuleMetadata {
        key: usage::MODULE_INFO_EXTRACT,
        label: "信息提取",
        tool_path: "/tools/infoextract",
        status_path_prefix: "/api/infoextract/jobs/",
        supports_downloads: true,
    },
    ModuleMetadata {
        key: usage::MODULE_TRANSLATE_DOCX,
        label: "DOCX 翻译",
        tool_path: "/tools/translatedocx",
        status_path_prefix: "/api/translatedocx/jobs/",
        supports_downloads: true,
    },
    ModuleMetadata {
        key: usage::MODULE_GRADER,
        label: "稿件评估",
        tool_path: "/tools/grader",
        status_path_prefix: "/api/grader/jobs/",
        supports_downloads: false,
    },
    ModuleMetadata {
        key: usage::MODULE_REVIEWER,
        label: "审稿助手",
        tool_path: "/tools/reviewer",
        status_path_prefix: "/api/reviewer/jobs/",
        supports_downloads: true,
    },
];

pub fn module_metadata(key: &str) -> Option<&'static ModuleMetadata> {
    MODULES.iter().find(|meta| meta.key == key)
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub module: String,
    pub job_key: String,
    pub created_at: DateTime<Utc>,
    pub status: Option<String>,
    pub status_detail: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
    pub files_purged: bool,
}

#[derive(Debug)]
struct StatusSnapshot {
    status: String,
    status_detail: Option<String>,
    updated_at: DateTime<Utc>,
    files_purged_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct HistoryRow {
    module: String,
    job_key: String,
    created_at: DateTime<Utc>,
}

pub async fn record_job_start(
    pool: &PgPool,
    module: &str,
    user_id: Uuid,
    job_key: impl Into<String>,
) -> Result<()> {
    let job_key = job_key.into();

    sqlx::query(
        "INSERT INTO user_job_history (user_id, module, job_key) VALUES ($1, $2, $3)
         ON CONFLICT (module, job_key) DO UPDATE SET user_id = EXCLUDED.user_id, created_at = NOW()",
    )
    .bind(user_id)
    .bind(module)
    .bind(&job_key)
    .execute(pool)
    .await
    .with_context(|| format!("failed to upsert history record for module {module}"))?;

    sqlx::query(
        "DELETE FROM user_job_history
         WHERE id IN (
             SELECT id FROM user_job_history
             WHERE user_id = $1 AND module = $2
             ORDER BY created_at DESC, id DESC
             OFFSET $3
         )",
    )
    .bind(user_id)
    .bind(module)
    .bind(HISTORY_LIMIT)
    .execute(pool)
    .await
    .with_context(|| format!("failed to prune excess history rows for module {module}"))?;

    Ok(())
}

pub async fn fetch_recent_jobs(
    pool: &PgPool,
    user_id: Uuid,
    module_filter: Option<&str>,
    limit: i64,
) -> Result<Vec<HistoryEntry>> {
    let limit = limit.clamp(1, HISTORY_LIMIT);
    let cutoff = Utc::now() - POLL_WINDOW;

    let rows = if let Some(module) = module_filter {
        sqlx::query_as::<_, HistoryRow>(
            "SELECT module, job_key, created_at
             FROM user_job_history
             WHERE user_id = $1 AND module = $2 AND created_at >= $3
             ORDER BY created_at DESC
             LIMIT $4",
        )
        .bind(user_id)
        .bind(module)
        .bind(cutoff)
        .bind(limit)
        .fetch_all(pool)
        .await
        .with_context(|| format!("failed to load history rows for module {module}"))?
    } else {
        sqlx::query_as::<_, HistoryRow>(
            "SELECT module, job_key, created_at
             FROM user_job_history
             WHERE user_id = $1 AND created_at >= $2
             ORDER BY created_at DESC
             LIMIT $3",
        )
        .bind(user_id)
        .bind(cutoff)
        .bind(limit)
        .fetch_all(pool)
        .await
        .context("failed to load history rows")?
    };

    let mut entries: Vec<HistoryEntry> = rows
        .into_iter()
        .map(|row| HistoryEntry {
            module: row.module,
            job_key: row.job_key,
            created_at: row.created_at,
            status: None,
            status_detail: None,
            updated_at: None,
            files_purged: false,
        })
        .collect();

    if entries.is_empty() {
        return Ok(entries);
    }

    let mut module_indices: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        module_indices
            .entry(entry.module.clone())
            .or_default()
            .push(idx);
    }

    for (module_key, indices) in module_indices {
        match module_key.as_str() {
            usage::MODULE_SUMMARIZER => {
                hydrate_uuid_entries(pool, "summary_jobs", "id", &mut entries, &indices).await?;
            }
            usage::MODULE_TRANSLATE_DOCX => {
                hydrate_uuid_entries(pool, "docx_jobs", "id", &mut entries, &indices).await?;
            }
            usage::MODULE_GRADER => {
                hydrate_uuid_entries(pool, "grader_jobs", "id", &mut entries, &indices).await?;
            }
            usage::MODULE_INFO_EXTRACT => {
                hydrate_uuid_entries(pool, "info_extract_jobs", "id", &mut entries, &indices)
                    .await?;
            }
            usage::MODULE_REVIEWER => {
                hydrate_int_entries(pool, "reviewer_jobs", "job_id", &mut entries, &indices)
                    .await?;
            }
            other => {
                warn!(module = other, "unknown module in history table");
            }
        }
    }

    Ok(entries)
}

async fn hydrate_uuid_entries(
    pool: &PgPool,
    table: &str,
    id_column: &str,
    entries: &mut [HistoryEntry],
    indices: &[usize],
) -> Result<()> {
    let mut uuid_keys = Vec::with_capacity(indices.len());
    let mut key_to_indices: HashMap<String, Vec<usize>> = HashMap::new();

    for &idx in indices {
        match Uuid::parse_str(&entries[idx].job_key) {
            Ok(uuid) => {
                uuid_keys.push(uuid);
                key_to_indices
                    .entry(entries[idx].job_key.clone())
                    .or_default()
                    .push(idx);
            }
            Err(err) => {
                warn!(
                    job_key = %entries[idx].job_key,
                    ?err,
                    "invalid UUID stored for history entry"
                );
            }
        }
    }

    if uuid_keys.is_empty() {
        return Ok(());
    }

    let snapshots = fetch_uuid_snapshots(pool, table, id_column, &uuid_keys).await?;

    for (job_key, entry_indices) in key_to_indices {
        if let Some(snapshot) = snapshots.get(&job_key) {
            for idx in entry_indices {
                entries[idx].status = Some(snapshot.status.clone());
                entries[idx].status_detail = snapshot.status_detail.clone();
                entries[idx].updated_at = Some(snapshot.updated_at);
                entries[idx].files_purged = snapshot.files_purged_at.is_some();
            }
        }
    }

    Ok(())
}

async fn hydrate_int_entries(
    pool: &PgPool,
    table: &str,
    id_column: &str,
    entries: &mut [HistoryEntry],
    indices: &[usize],
) -> Result<()> {
    let mut int_keys = Vec::with_capacity(indices.len());
    let mut key_to_indices: HashMap<String, Vec<usize>> = HashMap::new();

    for &idx in indices {
        match entries[idx].job_key.parse::<i32>() {
            Ok(id) => {
                int_keys.push(id);
                key_to_indices
                    .entry(entries[idx].job_key.clone())
                    .or_default()
                    .push(idx);
            }
            Err(err) => {
                warn!(
                    job_key = %entries[idx].job_key,
                    ?err,
                    "invalid integer stored for history entry"
                );
            }
        }
    }

    if int_keys.is_empty() {
        return Ok(());
    }

    let snapshots = fetch_int_snapshots(pool, table, id_column, &int_keys).await?;

    for (job_key, entry_indices) in key_to_indices {
        if let Some(snapshot) = snapshots.get(&job_key) {
            for idx in entry_indices {
                entries[idx].status = Some(snapshot.status.clone());
                entries[idx].status_detail = snapshot.status_detail.clone();
                entries[idx].updated_at = Some(snapshot.updated_at);
                entries[idx].files_purged = snapshot.files_purged_at.is_some();
            }
        }
    }

    Ok(())
}

async fn fetch_uuid_snapshots(
    pool: &PgPool,
    table: &str,
    id_column: &str,
    ids: &[Uuid],
) -> Result<HashMap<String, StatusSnapshot>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let sql = format!(
        "SELECT {id_column} AS job_id, status, status_detail, updated_at, files_purged_at
         FROM {table}
         WHERE {id_column} = ANY($1)",
    );

    let rows = sqlx::query(&sql)
        .bind(ids)
        .fetch_all(pool)
        .await
        .with_context(|| format!("failed to load job statuses from {table}"))?;

    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        let job_id: Uuid = row.try_get("job_id")?;
        let status: String = row.try_get("status")?;
        let status_detail: Option<String> = row.try_get("status_detail")?;
        let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
        let files_purged_at: Option<DateTime<Utc>> = row.try_get("files_purged_at")?;
        map.insert(
            job_id.to_string(),
            StatusSnapshot {
                status,
                status_detail,
                updated_at,
                files_purged_at,
            },
        );
    }

    Ok(map)
}

async fn fetch_int_snapshots(
    pool: &PgPool,
    table: &str,
    id_column: &str,
    ids: &[i32],
) -> Result<HashMap<String, StatusSnapshot>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let sql = format!(
        "SELECT {id_column} AS job_id, status, status_detail, updated_at, files_purged_at
         FROM {table}
         WHERE {id_column} = ANY($1)",
    );

    let rows = sqlx::query(&sql)
        .bind(ids)
        .fetch_all(pool)
        .await
        .with_context(|| format!("failed to load job statuses from {table}"))?;

    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        let job_id: i32 = row.try_get("job_id")?;
        let status: String = row.try_get("status")?;
        let status_detail: Option<String> = row.try_get("status_detail")?;
        let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
        let files_purged_at: Option<DateTime<Utc>> = row.try_get("files_purged_at")?;
        map.insert(
            job_id.to_string(),
            StatusSnapshot {
                status,
                status_detail,
                updated_at,
                files_purged_at,
            },
        );
    }

    Ok(map)
}

pub async fn purge_stale_history(pool: &PgPool) -> Result<u64> {
    let cutoff = Utc::now() - POLL_WINDOW;
    let result = sqlx::query("DELETE FROM user_job_history WHERE created_at < $1")
        .bind(cutoff)
        .execute(pool)
        .await
        .context("failed to delete old history rows")?;

    Ok(result.rows_affected())
}

pub fn retention_interval() -> StdDuration {
    StdDuration::from_secs((HISTORY_RETENTION_HOURS * 3600) as u64)
}
