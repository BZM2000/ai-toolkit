use std::{io::ErrorKind, path::PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Row};
use tokio::time::{Duration as TokioDuration, sleep};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{AppState, history};

const CLEANUP_INTERVAL_MINUTES: u64 = 15;
const SUMMARIZER_STORAGE: &str = "storage/summarizer";
const DOCX_STORAGE: &str = "storage/translatedocx";
const GRADER_STORAGE: &str = "storage/grader";
const INFO_EXTRACT_STORAGE: &str = "storage/infoextract";
const REVIEWER_STORAGE: &str = "storage/reviewer";

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let interval = TokioDuration::from_secs(CLEANUP_INTERVAL_MINUTES * 60);
        loop {
            if let Err(err) = run_cleanup_cycle(&state).await {
                error!(?err, "retention cleanup cycle failed");
            }
            sleep(interval).await;
        }
    });
}

async fn run_cleanup_cycle(state: &AppState) -> Result<()> {
    let pool = state.pool();
    let cutoff = Utc::now() - Duration::hours(history::HISTORY_RETENTION_HOURS);

    let mut purged_jobs = 0_u64;

    purged_jobs += purge_summarizer(&pool, cutoff).await?;
    purged_jobs += purge_docx(&pool, cutoff).await?;
    purged_jobs += purge_grader(&pool, cutoff).await?;
    purged_jobs += purge_info_extract(&pool, cutoff).await?;
    purged_jobs += purge_reviewer(&pool, cutoff).await?;

    let history_removed = history::purge_stale_history(&pool).await?;

    if purged_jobs > 0 || history_removed > 0 {
        info!(purged_jobs, history_removed, "retention cleanup completed");
    }

    Ok(())
}

async fn purge_summarizer(pool: &PgPool, cutoff: DateTime<Utc>) -> Result<u64> {
    let rows = sqlx::query(
        "SELECT id FROM summary_jobs WHERE files_purged_at IS NULL AND updated_at < $1",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
    .context("failed to fetch summarizer jobs pending cleanup")?;

    let mut purged = 0_u64;

    for row in rows {
        let job_id: Uuid = row.try_get("id")?;
        let job_id_str = job_id.to_string();

        if !remove_job_directory(SUMMARIZER_STORAGE, &job_id_str).await {
            continue;
        }

        sqlx::query(
            "UPDATE summary_documents
             SET summary_path = NULL, translation_path = NULL, updated_at = NOW()
             WHERE job_id = $1",
        )
        .bind(job_id)
        .execute(pool)
        .await
        .context("failed to null summarizer document outputs during cleanup")?;

        sqlx::query(
            "UPDATE summary_jobs
             SET combined_summary_path = NULL,
                 combined_translation_path = NULL,
                 files_purged_at = NOW(),
                 updated_at = NOW()
             WHERE id = $1",
        )
        .bind(job_id)
        .execute(pool)
        .await
        .context("failed to update summarizer job after cleanup")?;

        purged += 1;
    }

    Ok(purged)
}

async fn purge_docx(pool: &PgPool, cutoff: DateTime<Utc>) -> Result<u64> {
    let rows =
        sqlx::query("SELECT id FROM docx_jobs WHERE files_purged_at IS NULL AND updated_at < $1")
            .bind(cutoff)
            .fetch_all(pool)
            .await
            .context("failed to fetch DOCX translator jobs pending cleanup")?;

    let mut purged = 0_u64;

    for row in rows {
        let job_id: Uuid = row.try_get("id")?;
        let job_id_str = job_id.to_string();

        if !remove_job_directory(DOCX_STORAGE, &job_id_str).await {
            continue;
        }

        sqlx::query(
            "UPDATE docx_documents
             SET translated_path = NULL, updated_at = NOW()
             WHERE job_id = $1",
        )
        .bind(job_id)
        .execute(pool)
        .await
        .context("failed to null DOCX translator outputs during cleanup")?;

        sqlx::query(
            "UPDATE docx_jobs
             SET files_purged_at = NOW(), updated_at = NOW()
             WHERE id = $1",
        )
        .bind(job_id)
        .execute(pool)
        .await
        .context("failed to update DOCX translator job after cleanup")?;

        purged += 1;
    }

    Ok(purged)
}

async fn purge_grader(pool: &PgPool, cutoff: DateTime<Utc>) -> Result<u64> {
    let rows =
        sqlx::query("SELECT id FROM grader_jobs WHERE files_purged_at IS NULL AND updated_at < $1")
            .bind(cutoff)
            .fetch_all(pool)
            .await
            .context("failed to fetch grader jobs pending cleanup")?;

    let mut purged = 0_u64;

    for row in rows {
        let job_id: Uuid = row.try_get("id")?;
        let job_id_str = job_id.to_string();

        if !remove_job_directory(GRADER_STORAGE, &job_id_str).await {
            continue;
        }

        sqlx::query(
            "UPDATE grader_jobs
             SET files_purged_at = NOW(), updated_at = NOW()
             WHERE id = $1",
        )
        .bind(job_id)
        .execute(pool)
        .await
        .context("failed to update grader job after cleanup")?;

        purged += 1;
    }

    Ok(purged)
}

async fn purge_info_extract(pool: &PgPool, cutoff: DateTime<Utc>) -> Result<u64> {
    let rows = sqlx::query(
        "SELECT id FROM info_extract_jobs WHERE files_purged_at IS NULL AND updated_at < $1",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
    .context("failed to fetch info extract jobs pending cleanup")?;

    let mut purged = 0_u64;

    for row in rows {
        let job_id: Uuid = row.try_get("id")?;
        let job_id_str = job_id.to_string();

        if !remove_job_directory(INFO_EXTRACT_STORAGE, &job_id_str).await {
            continue;
        }

        sqlx::query(
            "UPDATE info_extract_jobs
             SET result_path = NULL, files_purged_at = NOW(), updated_at = NOW()
             WHERE id = $1",
        )
        .bind(job_id)
        .execute(pool)
        .await
        .context("failed to update info extract job after cleanup")?;

        purged += 1;
    }

    Ok(purged)
}

async fn purge_reviewer(pool: &PgPool, cutoff: DateTime<Utc>) -> Result<u64> {
    let rows = sqlx::query(
        "SELECT job_id FROM reviewer_jobs WHERE files_purged_at IS NULL AND updated_at < $1",
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await
    .context("failed to fetch reviewer jobs pending cleanup")?;

    let mut purged = 0_u64;

    for row in rows {
        let job_id: i32 = row.try_get("job_id")?;
        let job_id_str = job_id.to_string();

        if !remove_job_directory(REVIEWER_STORAGE, &job_id_str).await {
            continue;
        }

        sqlx::query(
            "UPDATE reviewer_documents
             SET file_path = NULL, updated_at = NOW()
             WHERE job_id = $1",
        )
        .bind(job_id)
        .execute(pool)
        .await
        .context("failed to null reviewer document outputs during cleanup")?;

        sqlx::query(
            "UPDATE reviewer_jobs
             SET files_purged_at = NOW(), updated_at = NOW()
             WHERE job_id = $1",
        )
        .bind(job_id)
        .execute(pool)
        .await
        .context("failed to update reviewer job after cleanup")?;

        purged += 1;
    }

    Ok(purged)
}

async fn remove_job_directory(root: &str, name: &str) -> bool {
    let path = PathBuf::from(root).join(name);
    match tokio::fs::remove_dir_all(&path).await {
        Ok(_) => true,
        Err(err) if err.kind() == ErrorKind::NotFound => true,
        Err(err) => {
            warn!(?err, path = %path.display(), "failed to remove job directory");
            false
        }
    }
}
