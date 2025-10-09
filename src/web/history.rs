use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use serde::Deserialize;
use tracing::error;

use crate::history;
use crate::web::{
    ApiMessage, AppState, JobStatus,
    auth::{self, JsonAuthError},
    json_error,
};

#[derive(Deserialize)]
pub struct HistoryQuery {
    #[serde(default)]
    module: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(serde::Serialize)]
pub(crate) struct HistoryItem {
    module: String,
    module_label: String,
    tool_path: String,
    status_path: String,
    job_key: String,
    created_at: String,
    updated_at: Option<String>,
    status: Option<String>,
    status_label: Option<String>,
    status_detail: Option<String>,
    files_purged: bool,
    supports_downloads: bool,
}

#[derive(serde::Serialize)]
pub(crate) struct HistoryResponse {
    jobs: Vec<HistoryItem>,
    retention_seconds: u64,
    generated_at: String,
}

pub async fn recent_history(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<HistoryResponse>, (StatusCode, Json<ApiMessage>)> {
    let user = auth::current_user_or_json_error(&state, &jar)
        .await
        .map_err(|JsonAuthError { status, message }| json_error(status, message))?;

    if let Some(ref module) = query.module {
        if history::module_metadata(module).is_none() {
            return Err(json_error(StatusCode::BAD_REQUEST, "未知模块标识。"));
        }
    }

    let limit = query.limit.unwrap_or(20);

    let entries =
        history::fetch_recent_jobs(&state.pool(), user.id, query.module.as_deref(), limit)
            .await
            .map_err(|err| {
                error!(?err, user_id = %user.id, "failed to load history entries");
                json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "无法读取历史记录，请稍后再试。",
                )
            })?;

    let jobs = entries
        .into_iter()
        .filter_map(|entry| {
            let meta = history::module_metadata(&entry.module)?;
            Some(HistoryItem {
                module: entry.module.clone(),
                module_label: meta.label.to_string(),
                tool_path: meta.tool_path.to_string(),
                status_path: format!("{}{}", meta.status_path_prefix, entry.job_key),
                job_key: entry.job_key,
                created_at: entry.created_at.to_rfc3339(),
                updated_at: entry.updated_at.map(|ts| ts.to_rfc3339()),
                status_label: entry
                    .status
                    .as_deref()
                    .map(|status| JobStatus::from_str(status).label_zh().to_string()),
                status: entry.status,
                status_detail: entry.status_detail,
                files_purged: entry.files_purged,
                supports_downloads: meta.supports_downloads,
            })
        })
        .collect::<Vec<_>>();

    let response = HistoryResponse {
        jobs,
        retention_seconds: history::retention_interval().as_secs(),
        generated_at: Utc::now().to_rfc3339(),
    };

    Ok(Json(response))
}
