use std::path::Path;

use anyhow::{Context, Result};
use axum::Json;
use axum::{
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use tracing::error;
use uuid::Uuid;

use crate::web::{ApiMessage, AuthUser, json_error};

/// Ensure the module-specific storage directory exists.
pub async fn ensure_storage_root(path: &str) -> Result<()> {
    tokio::fs::create_dir_all(path)
        .await
        .with_context(|| format!("failed to ensure storage root at {}", path))
}

/// Trait implemented by job rows that expose ownership and retention data.
pub trait JobAccess {
    fn user_id(&self) -> Uuid;
    fn files_purged_at(&self) -> Option<chrono::DateTime<chrono::Utc>>;
}

pub struct AccessMessages<'a> {
    pub not_found: &'a str,
    pub forbidden: &'a str,
    pub purged: &'a str,
}

/// Validate job access for the current user, enforcing ownership and purge status.
pub async fn verify_job_access<T, F, Fut>(
    fetch: F,
    requester: &AuthUser,
    messages: AccessMessages<'_>,
) -> Result<T, (StatusCode, Json<ApiMessage>)>
where
    T: JobAccess,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = sqlx::Result<Option<T>>>,
{
    let record = fetch()
        .await
        .map_err(|err| {
            error!(?err, "failed to load job access record");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "服务器内部错误。")
        })?
        .ok_or_else(|| json_error(StatusCode::NOT_FOUND, messages.not_found))?;

    if record.user_id() != requester.id && !requester.is_admin {
        return Err(json_error(StatusCode::FORBIDDEN, messages.forbidden));
    }

    if record.files_purged_at().is_some() {
        return Err(json_error(StatusCode::GONE, messages.purged));
    }

    Ok(record)
}

/// Ensure an optional path exists, returning a consistent JSON error otherwise.
pub fn require_path(
    path: Option<String>,
    message: impl Into<String>,
) -> Result<String, (StatusCode, Json<ApiMessage>)> {
    path.ok_or_else(|| json_error(StatusCode::NOT_FOUND, message))
}

/// Stream a file with a standard attachment disposition.
pub async fn stream_file(
    path: &Path,
    filename: &str,
    content_type: &str,
) -> Result<Response, (StatusCode, Json<ApiMessage>)> {
    let bytes = tokio::fs::read(path).await.map_err(|err| {
        error!(?err, file = %path.display(), "failed to read download file");
        json_error(StatusCode::INTERNAL_SERVER_ERROR, "文件读取失败。")
    })?;

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).unwrap(),
    );
    let disposition = format!("attachment; filename=\"{}\"", filename);
    let disposition = HeaderValue::from_str(&disposition)
        .map_err(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "下载头信息无效。"))?;
    headers.insert(axum::http::header::CONTENT_DISPOSITION, disposition);

    Ok((headers, bytes).into_response())
}

// Blanket implementation for tuples returned from SQL queries.
impl JobAccess for (Uuid, Option<chrono::DateTime<chrono::Utc>>) {
    fn user_id(&self) -> Uuid {
        self.0
    }

    fn files_purged_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.1
    }
}
