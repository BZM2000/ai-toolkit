use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, Redirect},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use serde::Deserialize;
use tracing::error;

use crate::history;
use crate::web::{
    ApiMessage, AppState, JobStatus,
    auth::{self, JsonAuthError},
    history_ui, json_error,
};
use crate::{escape_html, render_footer};

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
    modules: Vec<history::ApiModuleDescriptor>,
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

    let modules = history::modules()
        .iter()
        .map(history::ApiModuleDescriptor::from)
        .collect::<Vec<_>>();

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
        modules,
        jobs,
        retention_seconds: history::retention_interval().as_secs(),
        generated_at: Utc::now().to_rfc3339(),
    };

    Ok(Json(response))
}

pub async fn jobs_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Html<String>, Redirect> {
    let user = auth::require_user_redirect(&state, &jar).await?;
    let username = escape_html(&user.username);
    let footer = render_footer();

    let panels = history::modules()
        .iter()
        .map(|module| history_ui::render_history_panel(module.key))
        .collect::<String>();

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>任务历史 | 张圆教授课题组 AI 工具箱</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="robots" content="noindex,nofollow">
    <style>
        :root {{ color-scheme: light; }}
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; margin: 0; background: #f8fafc; color: #0f172a; min-height: 100vh; display: flex; flex-direction: column; }}
        header {{ background: #ffffff; padding: clamp(1.8rem, 4vw, 2.5rem) clamp(1.5rem, 6vw, 3rem); border-bottom: 1px solid #e2e8f0; display: flex; flex-direction: column; gap: 0.6rem; }}
        header h1 {{ margin: 0; font-size: clamp(1.75rem, 3vw, 2.2rem); }}
        header p {{ margin: 0; color: #64748b; }}
        .header-meta {{ display: flex; gap: 1rem; flex-wrap: wrap; align-items: center; color: #475569; font-size: 0.95rem; }}
        .header-meta a {{ color: #2563eb; text-decoration: none; font-weight: 600; }}
        .header-meta a:hover {{ text-decoration: underline; }}
        main {{ flex: 1; padding: clamp(1.5rem, 5vw, 3rem); max-width: 1100px; margin: 0 auto; width: 100%; box-sizing: border-box; display: flex; flex-direction: column; gap: 1.5rem; }}
        .history-grid {{ display: grid; gap: 1.5rem; grid-template-columns: repeat(auto-fit, minmax(320px, 1fr)); }}
        .history-grid .panel {{ height: 100%; }}
        .app-footer {{ margin-top: 2rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
        {history_styles}
    </style>
</head>
<body>
    <header>
        <h1>任务历史</h1>
        <p>查看各模块最近 24 小时内的任务进度与下载链接。</p>
        <div class="header-meta">
            <span>当前登录：<strong>{username}</strong></span>
            <a href="/">返回首页</a>
        </div>
    </header>
    <main>
        <div class="history-grid">
            {panels}
        </div>
        {footer}
    </main>
    <script>
{history_script}
    </script>
</body>
</html>"#,
        username = username,
        panels = panels,
        footer = footer,
        history_styles = history_ui::HISTORY_STYLES,
        history_script = history_ui::HISTORY_SCRIPT,
    );

    Ok(Html(html))
}
