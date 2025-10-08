use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{Multipart, Path as AxumPath, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::CookieJar;
use sanitize_filename::sanitize;
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use tokio::{fs as tokio_fs, time::sleep};
use tracing::error;
use uuid::Uuid;

mod admin;

use crate::{
    AppState, SESSION_COOKIE, escape_html,
    llm::{AttachmentKind, ChatMessage, FileAttachment, LlmClient, LlmRequest, MessageRole},
    render_footer,
    usage::{self, MODULE_REVIEWER},
};

const STORAGE_ROOT: &str = "storage/reviewer";
const STATUS_PENDING: &str = "pending";
const STATUS_PROCESSING: &str = "processing";
const STATUS_COMPLETED: &str = "completed";
const STATUS_FAILED: &str = "failed";

const ROUND1_RETRIES: usize = 3;
const ROUND1_MIN_SUCCESSES: usize = 4;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tools/reviewer", get(reviewer_page))
        .route("/tools/reviewer/jobs", post(create_job))
        .route("/api/reviewer/jobs/:id", get(job_status))
        .route(
            "/api/reviewer/jobs/:job_id/round/:round/review/:idx/download",
            get(download_review),
        )
        .route("/dashboard/modules/reviewer", get(admin::settings_page))
        .route(
            "/dashboard/modules/reviewer/models",
            post(admin::save_models),
        )
        .route(
            "/dashboard/modules/reviewer/prompts",
            post(admin::save_prompts),
        )
}

#[derive(sqlx::FromRow, Clone)]
struct SessionUser {
    id: Uuid,
    username: String,
    is_admin: bool,
}

#[derive(sqlx::FromRow)]
struct JobRow {
    user_id: Uuid,
    status: String,
    status_detail: Option<String>,
}

#[derive(Serialize)]
struct JobStatusResponse {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    round1_reviews: Option<Vec<ReviewInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    round2_review: Option<ReviewInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    round3_review: Option<ReviewInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct ReviewInfo {
    model: String,
    available: bool,
    download_url: Option<String>,
}

async fn require_user(state: &AppState, jar: &CookieJar) -> Result<SessionUser, Response> {
    let token_cookie = jar
        .get(SESSION_COOKIE)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Not authenticated").into_response())?;

    let token = Uuid::parse_str(token_cookie.value())
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid session").into_response())?;

    sqlx::query_as::<_, SessionUser>(
        "SELECT users.id, users.username, users.is_admin
         FROM sessions
         INNER JOIN users ON users.id = sessions.user_id
         WHERE sessions.id = $1 AND sessions.expires_at > NOW()",
    )
    .bind(token)
    .fetch_optional(state.pool_ref())
    .await
    .map_err(|e| {
        error!("Database error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
    })?
    .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Session expired").into_response())
}

async fn reviewer_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Html<String>, Response> {
    let user = require_user(&state, &jar).await?;

    let footer = render_footer();
    let username = escape_html(&user.username);
    let admin_link = if user.is_admin {
        r#"<a class="admin-link" href="/dashboard/modules/reviewer">模块管理</a>"#.to_string()
    } else {
        String::new()
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>审稿助手 | Zhang Group AI Toolkit</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="robots" content="noindex,nofollow">
    <style>
        :root {{ color-scheme: light; }}
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; margin: 0; background: #f8fafc; color: #0f172a; }}
        header {{ background: #ffffff; padding: 2rem 1.5rem; border-bottom: 1px solid #e2e8f0; }}
        .header-bar {{ display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 1rem; }}
        .back-link {{ display: inline-flex; align-items: center; gap: 0.4rem; color: #1d4ed8; text-decoration: none; font-weight: 600; background: #e0f2fe; padding: 0.5rem 0.95rem; border-radius: 999px; border: 1px solid #bfdbfe; transition: background 0.15s ease, border 0.15s ease; }}
        .back-link:hover {{ background: #bfdbfe; border-color: #93c5fd; }}
        .admin-link {{ display: inline-flex; align-items: center; gap: 0.35rem; color: #0f172a; background: #fee2e2; border: 1px solid #fecaca; padding: 0.45rem 0.9rem; border-radius: 999px; text-decoration: none; font-weight: 600; }}
        .admin-link:hover {{ background: #fecaca; border-color: #fca5a5; }}
        main {{ padding: 2rem 1.5rem; max-width: 960px; margin: 0 auto; box-sizing: border-box; }}
        section {{ margin-bottom: 2.5rem; }}
        .panel {{ background: #ffffff; border-radius: 12px; border: 1px solid #e2e8f0; padding: 1.5rem; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); }}
        .panel h2 {{ margin-top: 0; }}
        .note {{ color: #475569; font-size: 0.95rem; }}
        label {{ display: block; margin-bottom: 0.5rem; font-weight: 600; color: #0f172a; }}
        input[type="file"], select {{ width: 100%; padding: 0.75rem; border-radius: 8px; border: 1px solid #cbd5f5; background: #f8fafc; color: #0f172a; box-sizing: border-box; }}
        input[type="file"]:focus, select:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        button {{ padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        button:disabled {{ opacity: 0.6; cursor: not-allowed; }}
        .drop-zone {{ border: 2px dashed #cbd5f5; border-radius: 12px; padding: 2rem; text-align: center; background: #f8fafc; transition: border-color 0.2s ease, background 0.2s ease; cursor: pointer; margin-bottom: 1rem; color: #475569; }}
        .drop-zone.dragover {{ border-color: #2563eb; background: #e0f2fe; }}
        .drop-zone strong {{ color: #1d4ed8; }}
        .drop-zone input[type="file"] {{ display: none; }}
        .browse-link {{ color: #2563eb; text-decoration: underline; cursor: pointer; }}
        .status-alert {{ margin-top: 1rem; font-size: 0.95rem; }}
        .status-alert.success {{ color: #166534; }}
        .status-alert.error {{ color: #b91c1c; }}
        .status-card {{ margin-top: 1rem; padding: 1.25rem; border-radius: 12px; border: 1px solid #e2e8f0; background: #f8fafc; transition: border-color 0.2s ease, background 0.2s ease; line-height: 1.7; }}
        .status-card.processing {{ border-color: #fbbf24; background: #fffbeb; }}
        .status-card.completed {{ border-color: #bbf7d0; background: #ecfdf3; }}
        .status-card.failed {{ border-color: #fecaca; background: #fef2f2; }}
        .downloads {{ display: grid; gap: 0.5rem; margin-top: 0.75rem; }}
        .download-link {{ display: inline-flex; align-items: center; gap: 0.35rem; color: #2563eb; background: #e0f2fe; border: 1px solid #bfdbfe; padding: 0.45rem 0.9rem; border-radius: 999px; text-decoration: none; font-weight: 600; width: fit-content; }}
        .download-link:hover {{ background: #bfdbfe; border-color: #93c5fd; }}
        ul.process {{ padding-left: 1.25rem; margin: 0.5rem 0 0; color: #475569; }}
        @media (max-width: 768px) {{
            header {{ padding: 1.5rem 1rem; }}
            main {{ padding: 1.5rem 1rem; }}
            .header-bar {{ flex-direction: column; align-items: flex-start; }}
        }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>审稿助手</h1>
            <div style="display:flex; gap:0.75rem; align-items:center; flex-wrap:wrap;">
                <a class="back-link" href="/">← 返回首页</a>
                {admin_link}
            </div>
        </div>
        <p class="note">当前登录：<strong>{username}</strong>。上传 PDF 或 DOCX 手稿，系统将执行三轮专家审稿流程。</p>
    </header>
    <main>
        <section class="panel">
            <h2>提交新任务</h2>
            <p class="note">完整工作流程包含 8 个并行初审、1 次元审稿与 1 次事实核查。所有产出均可下载为 DOCX 文件。</p>
            <ul class="process">
                <li>第一轮：8 个模型并行独立审稿（每个最多重试 3 次，至少 4 个成功）</li>
                <li>第二轮：综合所有第一轮结果生成元审稿</li>
                <li>第三轮：针对原稿的事实核查与修订建议</li>
            </ul>
            <form id="reviewer-form" enctype="multipart/form-data">
                <label for="file">上传稿件（PDF 或 DOCX）</label>
                <div id="drop-area" class="drop-zone">
                    <p><strong>拖拽文件</strong>到此处，或<span class="browse-link">点击选择</span>文件。</p>
                    <p class="note">一次仅支持上传 1 个文件，以便执行完整审稿流程。</p>
                    <input type="file" id="file" name="file" accept=".pdf,.docx" required>
                </div>
                <div id="selection-status" class="note"></div>
                <label for="language">审稿语言</label>
                <select id="language" name="language" required>
                    <option value="english">English</option>
                    <option value="chinese">中文</option>
                </select>
                <button type="submit" id="submit-btn">开始审稿</button>
            </form>
            <div id="submission-status" class="status-alert"></div>
        </section>
        <section class="panel">
            <h2>任务进度</h2>
            <div id="job-status" class="status-card">
                <p class="note">提交任务后，这里将显示实时进度和下载链接。</p>
            </div>
        </section>
        {footer}
    </main>
    <script>
        const form = document.getElementById('reviewer-form');
        const statusBox = document.getElementById('submission-status');
        const jobStatus = document.getElementById('job-status');
        const submitBtn = document.getElementById('submit-btn');
        const dropArea = document.getElementById('drop-area');
        const fileInput = document.getElementById('file');
        const selectionStatus = document.getElementById('selection-status');
        let pollTimer = null;

        const statusLabels = {{
            pending: '排队中',
            processing: '处理中',
            completed: '已完成',
            failed: '失败'
        }};

        const updateSelectionStatus = () => {{
            if (fileInput.files.length > 0) {{
                selectionStatus.textContent = `已选择 ${{fileInput.files[0].name}}`;
            }} else {{
                selectionStatus.textContent = '';
            }}
        }};

        const handleFiles = (list) => {{
            if (!list || list.length === 0) {{
                return;
            }}
            const dt = new DataTransfer();
            dt.items.add(list[0]);
            fileInput.files = dt.files;
            updateSelectionStatus();
        }};

        fileInput.addEventListener('change', updateSelectionStatus);
        dropArea.addEventListener('click', () => fileInput.click());
        dropArea.addEventListener('dragenter', (event) => {{
            event.preventDefault();
            dropArea.classList.add('dragover');
        }});
        dropArea.addEventListener('dragover', (event) => event.preventDefault());
        dropArea.addEventListener('dragleave', (event) => {{
            event.preventDefault();
            const related = event.relatedTarget;
            if (!related || !dropArea.contains(related)) {{
                dropArea.classList.remove('dragover');
            }}
        }});
        dropArea.addEventListener('drop', (event) => {{
            event.preventDefault();
            dropArea.classList.remove('dragover');
            handleFiles(event.dataTransfer.files);
        }});

        updateSelectionStatus();

        form.addEventListener('submit', async (event) => {{
            event.preventDefault();

            if (!fileInput.files.length) {{
                statusBox.textContent = '请先选择待审稿的文件。';
                statusBox.className = 'status-alert error';
                return;
            }}

            const formData = new FormData(form);
            submitBtn.disabled = true;
            statusBox.textContent = '正在提交任务...';
            statusBox.className = 'status-alert';

            try {{
                const response = await fetch('/tools/reviewer/jobs', {{
                    method: 'POST',
                    body: formData
                }});

                if (!response.ok) {{
                    const errorText = await response.text();
                    throw new Error(errorText || '上传失败');
                }}

                const data = await response.json();
                statusBox.textContent = '任务已提交，正在排队处理。';
                statusBox.className = 'status-alert success';
                setProcessingState();
                pollJobStatus(data.job_id);
            }} catch (err) {{
                statusBox.textContent = `错误：${{err.message}}`;
                statusBox.className = 'status-alert error';
                submitBtn.disabled = false;
            }}
        }});

        function setProcessingState() {{
            jobStatus.className = 'status-card processing';
            jobStatus.innerHTML = '<p><strong>状态：</strong>任务已启动，正在等待各轮结果。</p>';
        }}

        function pollJobStatus(jobId) {{
            if (pollTimer) {{
                clearInterval(pollTimer);
            }}

            const fetchStatus = async () => {{
                try {{
                    const response = await fetch(`/api/reviewer/jobs/${{jobId}}`);
                    if (!response.ok) {{
                        throw new Error('无法获取任务状态');
                    }}
                    const data = await response.json();
                    renderJobStatus(data);

                    if (data.status === 'completed') {{
                        statusBox.textContent = '审稿已完成，可下载各轮报告。';
                        statusBox.className = 'status-alert success';
                        submitBtn.disabled = false;
                        clearInterval(pollTimer);
                    }} else if (data.status === 'failed') {{
                        statusBox.textContent = data.error ? `任务失败：${{data.error}}` : '任务失败，请稍后重试。';
                        statusBox.className = 'status-alert error';
                        submitBtn.disabled = false;
                        clearInterval(pollTimer);
                    }}
                }} catch (error) {{
                    console.error('Polling error:', error);
                }}
            }};

            fetchStatus();
            pollTimer = setInterval(fetchStatus, 2000);
        }}

        function renderJobStatus(data) {{
            const statusClass = getStatusClass(data.status);
            jobStatus.className = `status-card ${{statusClass}}`;

            let html = `<p><strong>状态：</strong>${{statusLabels[data.status] || data.status}}</p>`;
            if (data.status_detail) {{
                html += `<p class="note">${{data.status_detail}}</p>`;
            }}

            if (Array.isArray(data.round1_reviews)) {{
                const available = data.round1_reviews.filter(r => r.available && r.download_url);
                html += '<h3>第一轮审稿</h3>';
                if (available.length) {{
                    html += '<div class="downloads">';
                    available.forEach((review, index) => {{
                        html += `<a class="download-link" href="${{review.download_url}}">评审 ${{index + 1}}（${{review.model}}）</a>`;
                    }});
                    html += '</div>';
                }} else {{
                    html += '<p class="note">首轮评审正在生成中……</p>';
                }}
            }}

            if (data.round2_review && data.round2_review.available && data.round2_review.download_url) {{
                html += '<h3>第二轮元审稿</h3>';
                html += `<div class="downloads"><a class="download-link" href="${{data.round2_review.download_url}}">下载元审稿报告</a></div>`;
            }}

            if (data.round3_review && data.round3_review.available && data.round3_review.download_url) {{
                html += '<h3>第三轮事实核查</h3>';
                html += `<div class="downloads"><a class="download-link" href="${{data.round3_review.download_url}}">下载最终报告</a></div>`;
            }}

            if (data.error && data.status !== 'completed') {{
                html += `<p class="note" style="color:#b91c1c;">错误信息：${{data.error}}</p>`;
            }}

            jobStatus.innerHTML = html;
        }}

        function getStatusClass(status) {{
            if (status === 'completed') return 'completed';
            if (status === 'failed') return 'failed';
            return 'processing';
        }}
    </script>
</body>
</html>"#,
        admin_link = admin_link,
        username = username,
        footer = footer
    );

    Ok(Html(html))
}
async fn create_job(
    State(state): State<AppState>,
    jar: CookieJar,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, Response> {
    let user = require_user(&state, &jar).await?;

    // Check usage limits
    if let Err(e) = usage::ensure_within_limits(state.pool_ref(), user.id, MODULE_REVIEWER, 1).await
    {
        return Err((StatusCode::TOO_MANY_REQUESTS, e.message()).into_response());
    }

    let mut filename = String::new();
    let mut language = String::from("english");
    let mut file_data: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Multipart error: {e}")).into_response())?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                filename = field.file_name().unwrap_or("manuscript.pdf").to_string();
                file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| {
                            (StatusCode::BAD_REQUEST, format!("Failed to read file: {e}"))
                                .into_response()
                        })?
                        .to_vec(),
                );
            }
            "language" => {
                language = field.text().await.map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Failed to read language: {e}"),
                    )
                        .into_response()
                })?;
            }
            _ => {}
        }
    }

    let file_bytes =
        file_data.ok_or_else(|| (StatusCode::BAD_REQUEST, "No file provided").into_response())?;

    if !language.eq("english") && !language.eq("chinese") {
        return Err((StatusCode::BAD_REQUEST, "Invalid language").into_response());
    }

    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    if ext != "pdf" && ext != "docx" {
        return Err((
            StatusCode::BAD_REQUEST,
            "Only PDF and DOCX files are accepted",
        )
            .into_response());
    }

    // Create job in database
    let job_id: i32 = sqlx::query_scalar(
        "INSERT INTO reviewer_jobs (user_id, filename, language, status)
         VALUES ($1, $2, $3, $4) RETURNING job_id",
    )
    .bind(user.id)
    .bind(&filename)
    .bind(&language)
    .bind(STATUS_PENDING)
    .fetch_one(state.pool_ref())
    .await
    .map_err(|e| {
        error!("Failed to create job: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, "Failed to create job").into_response()
    })?;

    // Save uploaded file
    let job_dir = PathBuf::from(STORAGE_ROOT).join(job_id.to_string());
    tokio_fs::create_dir_all(&job_dir).await.map_err(|e| {
        error!("Failed to create job directory: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to create job directory",
        )
            .into_response()
    })?;

    let sanitized_filename = sanitize(&filename);
    let manuscript_path = job_dir.join(&sanitized_filename);
    tokio_fs::write(&manuscript_path, &file_bytes)
        .await
        .map_err(|e| {
            error!("Failed to write manuscript file: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to save file").into_response()
        })?;

    // Spawn background processing
    let pool = state.pool().clone();
    let llm_client = state.llm_client().clone();
    let reviewer_settings = state.reviewer_settings().await.ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Reviewer settings not configured",
        )
            .into_response()
    })?;

    tokio::spawn(async move {
        if let Err(e) = process_reviewer_job(
            pool.clone(),
            llm_client,
            job_id,
            user.id,
            manuscript_path,
            &language,
            &ext,
            reviewer_settings,
        )
        .await
        {
            error!("Job {job_id} failed: {e}");
            let _ = sqlx::query(
                "UPDATE reviewer_jobs SET status = $1, status_detail = $2, updated_at = NOW()
                 WHERE job_id = $3",
            )
            .bind(STATUS_FAILED)
            .bind(format!("Error: {e}"))
            .bind(job_id)
            .execute(&pool)
            .await;
        }
    });

    Ok(Json(json!({ "job_id": job_id })))
}

async fn job_status(
    State(state): State<AppState>,
    jar: CookieJar,
    AxumPath(job_id): AxumPath<i32>,
) -> Result<Json<JobStatusResponse>, Response> {
    let user = require_user(&state, &jar).await?;

    let job = sqlx::query_as::<_, JobRow>(
        "SELECT user_id, status, status_detail
         FROM reviewer_jobs WHERE job_id = $1",
    )
    .bind(job_id)
    .fetch_optional(state.pool_ref())
    .await
    .map_err(|e| {
        error!("Database error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
    })?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "Job not found").into_response())?;

    if job.user_id != user.id && !user.is_admin {
        return Err((StatusCode::FORBIDDEN, "Access denied").into_response());
    }

    // Fetch review documents
    #[derive(sqlx::FromRow)]
    struct DocRow {
        round: i32,
        review_index: Option<i32>,
        model_name: String,
        file_path: Option<String>,
        status: String,
    }

    let docs = sqlx::query_as::<_, DocRow>(
        "SELECT round, review_index, model_name, file_path, status
         FROM reviewer_documents WHERE job_id = $1 ORDER BY round, review_index",
    )
    .bind(job_id)
    .fetch_all(state.pool_ref())
    .await
    .map_err(|e| {
        error!("Database error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
    })?;

    let mut round1_reviews = Vec::new();
    let mut round2_review = None;
    let mut round3_review = None;

    for doc in docs {
        let DocRow {
            round,
            review_index,
            model_name,
            file_path,
            status,
        } = doc;

        let is_completed = status == STATUS_COMPLETED;
        let has_file = is_completed
            && file_path
                .as_ref()
                .map(|path| !path.is_empty())
                .unwrap_or(false);

        match round {
            1 => {
                let idx = review_index.unwrap_or(0);
                round1_reviews.push(ReviewInfo {
                    model: model_name,
                    available: has_file,
                    download_url: if has_file {
                        Some(format!(
                            "/api/reviewer/jobs/{job_id}/round/1/review/{idx}/download"
                        ))
                    } else {
                        None
                    },
                });
            }
            2 => {
                round2_review = Some(ReviewInfo {
                    model: model_name,
                    available: has_file,
                    download_url: if has_file {
                        Some(format!(
                            "/api/reviewer/jobs/{job_id}/round/2/review/0/download"
                        ))
                    } else {
                        None
                    },
                });
            }
            3 => {
                round3_review = Some(ReviewInfo {
                    model: model_name,
                    available: has_file,
                    download_url: if has_file {
                        Some(format!(
                            "/api/reviewer/jobs/{job_id}/round/3/review/0/download"
                        ))
                    } else {
                        None
                    },
                });
            }
            _ => {}
        }
    }

    Ok(Json(JobStatusResponse {
        status: job.status,
        status_detail: job.status_detail,
        round1_reviews: if !round1_reviews.is_empty() {
            Some(round1_reviews)
        } else {
            None
        },
        round2_review,
        round3_review,
        error: None,
    }))
}

async fn download_review(
    State(state): State<AppState>,
    jar: CookieJar,
    AxumPath((job_id, round, idx)): AxumPath<(i32, i32, i32)>,
) -> Result<Response, Response> {
    let user = require_user(&state, &jar).await?;

    let job = sqlx::query_as::<_, JobRow>(
        "SELECT user_id, status, status_detail
         FROM reviewer_jobs WHERE job_id = $1",
    )
    .bind(job_id)
    .fetch_optional(state.pool_ref())
    .await
    .map_err(|e| {
        error!("Database error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
    })?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "Job not found").into_response())?;

    if job.user_id != user.id && !user.is_admin {
        return Err((StatusCode::FORBIDDEN, "Access denied").into_response());
    }

    #[derive(sqlx::FromRow)]
    struct DocPath {
        file_path: Option<String>,
    }

    let doc = sqlx::query_as::<_, DocPath>(
        "SELECT file_path FROM reviewer_documents
         WHERE job_id = $1 AND round = $2 AND (review_index = $3 OR (review_index IS NULL AND $3 = 0))"
    )
    .bind(job_id)
    .bind(round)
    .bind(idx)
    .fetch_optional(state.pool_ref())
    .await
    .map_err(|e| {
        error!("Database error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
    })?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "Review not found").into_response())?;

    let file_path = doc
        .file_path
        .ok_or_else(|| (StatusCode::NOT_FOUND, "File not available").into_response())?;

    let path_buf = PathBuf::from(&file_path);
    let filename = path_buf
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("review.docx");

    let bytes = tokio_fs::read(&file_path).await.map_err(|e| {
        error!("Failed to read file {file_path}: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read file").into_response()
    })?;

    Ok((
        [
            (
                "Content-Type",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            (
                "Content-Disposition",
                &format!("attachment; filename=\"{}\"", filename),
            ),
        ],
        bytes,
    )
        .into_response())
}

// Background processing function
async fn process_reviewer_job(
    pool: PgPool,
    llm_client: LlmClient,
    job_id: i32,
    user_id: Uuid,
    manuscript_path: PathBuf,
    language: &str,
    ext: &str,
    settings: crate::config::ReviewerSettings,
) -> Result<()> {
    // Update status to processing
    sqlx::query(
        "UPDATE reviewer_jobs SET status = $1, status_detail = $2, updated_at = NOW()
         WHERE job_id = $3",
    )
    .bind(STATUS_PROCESSING)
    .bind("Starting review process...")
    .bind(job_id)
    .execute(&pool)
    .await?;

    // Convert DOCX to PDF if needed
    let pdf_path = if ext == "docx" {
        convert_docx_to_pdf(&manuscript_path).await?
    } else {
        manuscript_path.clone()
    };

    // Round 1: 8 parallel reviews with retry
    sqlx::query(
        "UPDATE reviewer_jobs SET status_detail = $1, updated_at = NOW() WHERE job_id = $2",
    )
    .bind("Round 1: Running 8 parallel reviews...")
    .bind(job_id)
    .execute(&pool)
    .await?;

    let round1_models = vec![
        settings.models.round1_model_1.clone(),
        settings.models.round1_model_2.clone(),
        settings.models.round1_model_3.clone(),
        settings.models.round1_model_4.clone(),
        settings.models.round1_model_5.clone(),
        settings.models.round1_model_6.clone(),
        settings.models.round1_model_7.clone(),
        settings.models.round1_model_8.clone(),
    ];

    let round1_prompt = if language == "chinese" {
        &settings.prompts.initial_prompt_zh
    } else {
        &settings.prompts.initial_prompt
    };

    let mut round1_results = Vec::new();
    let mut round1_futures = Vec::new();

    for (idx, model) in round1_models.iter().enumerate() {
        let pool_clone = pool.clone();
        let llm_clone = llm_client.clone();
        let pdf_path_clone = pdf_path.clone();
        let prompt_clone = round1_prompt.clone();
        let model_clone = model.clone();

        round1_futures.push(tokio::spawn(async move {
            run_round1_review(
                pool_clone,
                llm_clone,
                job_id,
                idx as i32,
                &pdf_path_clone,
                &prompt_clone,
                &model_clone,
            )
            .await
        }));
    }

    for (idx, future) in round1_futures.into_iter().enumerate() {
        match future.await {
            Ok(Ok(review_text)) => {
                round1_results.push((idx, review_text));
            }
            Ok(Err(e)) => {
                error!("Round 1 review {idx} failed: {e}");
            }
            Err(e) => {
                error!("Round 1 review {idx} task panicked: {e}");
            }
        }
    }

    if round1_results.len() < ROUND1_MIN_SUCCESSES {
        return Err(anyhow!(
            "Round 1 failed: only {} out of 8 reviews succeeded (minimum {})",
            round1_results.len(),
            ROUND1_MIN_SUCCESSES
        ));
    }

    sqlx::query(
        "UPDATE reviewer_jobs SET status_detail = $1, updated_at = NOW() WHERE job_id = $2",
    )
    .bind(format!(
        "Round 1 completed: {}/{} reviews succeeded",
        round1_results.len(),
        8
    ))
    .bind(job_id)
    .execute(&pool)
    .await?;

    // Convert Round 1 reviews to DOCX and save
    for (idx, review_text) in &round1_results {
        let docx_path = PathBuf::from(STORAGE_ROOT)
            .join(job_id.to_string())
            .join(format!("round1_review_{}.docx", idx + 1));

        text_to_docx(&review_text, &docx_path).await?;

        sqlx::query(
            "UPDATE reviewer_documents SET file_path = $1, updated_at = NOW()
             WHERE job_id = $2 AND round = 1 AND review_index = $3",
        )
        .bind(docx_path.to_string_lossy().to_string())
        .bind(job_id)
        .bind(*idx as i32)
        .execute(&pool)
        .await?;
    }

    // Round 2: Meta-review
    sqlx::query(
        "UPDATE reviewer_jobs SET status_detail = $1, updated_at = NOW() WHERE job_id = $2",
    )
    .bind("Round 2: Generating meta-review...")
    .bind(job_id)
    .execute(&pool)
    .await?;

    let round2_prompt = if language == "chinese" {
        &settings.prompts.secondary_prompt_zh
    } else {
        &settings.prompts.secondary_prompt
    };

    let combined_reviews = round1_results
        .iter()
        .map(|(idx, text)| format!("=== Review {} ===\n\n{}\n\n", idx + 1, text))
        .collect::<Vec<_>>()
        .join("\n");

    let round2_text = run_round2_review(
        &pool,
        &llm_client,
        job_id,
        &pdf_path,
        round2_prompt,
        &combined_reviews,
        &settings.models.round2_model,
    )
    .await?;

    let round2_docx = PathBuf::from(STORAGE_ROOT)
        .join(job_id.to_string())
        .join("round2_meta_review.docx");
    text_to_docx(&round2_text, &round2_docx).await?;

    sqlx::query(
        "UPDATE reviewer_documents SET file_path = $1, updated_at = NOW()
         WHERE job_id = $2 AND round = 2",
    )
    .bind(round2_docx.to_string_lossy().to_string())
    .bind(job_id)
    .execute(&pool)
    .await?;

    // Round 3: Fact-checking
    sqlx::query(
        "UPDATE reviewer_jobs SET status_detail = $1, updated_at = NOW() WHERE job_id = $2",
    )
    .bind("Round 3: Fact-checking...")
    .bind(job_id)
    .execute(&pool)
    .await?;

    let round3_prompt = if language == "chinese" {
        &settings.prompts.final_prompt_zh
    } else {
        &settings.prompts.final_prompt
    };

    let round3_text = run_round3_review(
        &pool,
        &llm_client,
        job_id,
        &pdf_path,
        round3_prompt,
        &round2_text,
        &settings.models.round3_model,
    )
    .await?;

    let round3_docx = PathBuf::from(STORAGE_ROOT)
        .join(job_id.to_string())
        .join("round3_final_report.docx");
    text_to_docx(&round3_text, &round3_docx).await?;

    sqlx::query(
        "UPDATE reviewer_documents SET file_path = $1, updated_at = NOW()
         WHERE job_id = $2 AND round = 3",
    )
    .bind(round3_docx.to_string_lossy().to_string())
    .bind(job_id)
    .execute(&pool)
    .await?;

    // Record usage (tokens are not tracked for reviewer module)
    usage::record_usage(&pool, user_id, MODULE_REVIEWER, 0, 1).await?;

    // Mark job as completed
    sqlx::query(
        "UPDATE reviewer_jobs SET status = $1, status_detail = $2, updated_at = NOW()
         WHERE job_id = $3",
    )
    .bind(STATUS_COMPLETED)
    .bind("All rounds completed successfully")
    .bind(job_id)
    .execute(&pool)
    .await?;

    Ok(())
}

async fn convert_docx_to_pdf(docx_path: &Path) -> Result<PathBuf> {
    let output_dir = docx_path
        .parent()
        .ok_or_else(|| anyhow!("Invalid path: no parent directory"))?;

    let output = Command::new("libreoffice")
        .args(&[
            "--headless",
            "--convert-to",
            "pdf:writer_pdf_Export",
            "--outdir",
            &output_dir.to_string_lossy(),
            &docx_path.to_string_lossy(),
        ])
        .output()
        .context("Failed to execute libreoffice command")?;

    if !output.status.success() {
        return Err(anyhow!(
            "LibreOffice conversion failed with exit code {:?}\nStderr: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let pdf_filename = docx_path
        .file_stem()
        .ok_or_else(|| anyhow!("Invalid DOCX filename"))?
        .to_string_lossy()
        .to_string()
        + ".pdf";

    let pdf_path = output_dir.join(pdf_filename);

    if !pdf_path.exists() {
        return Err(anyhow!(
            "PDF file was not created at expected path: {}",
            pdf_path.display()
        ));
    }

    Ok(pdf_path)
}

async fn run_round1_review(
    pool: PgPool,
    llm_client: LlmClient,
    job_id: i32,
    idx: i32,
    pdf_path: &Path,
    prompt: &str,
    model: &str,
) -> Result<String> {
    // Create document record
    sqlx::query(
        "INSERT INTO reviewer_documents (job_id, round, review_index, model_name, status)
         VALUES ($1, 1, $2, $3, $4)",
    )
    .bind(job_id)
    .bind(idx)
    .bind(model)
    .bind(STATUS_PROCESSING)
    .execute(&pool)
    .await?;

    let mut last_error = None;
    for attempt in 0..ROUND1_RETRIES {
        match call_llm(&llm_client, model, prompt, pdf_path).await {
            Ok(text) => {
                sqlx::query(
                    "UPDATE reviewer_documents SET review_text = $1, status = $2, updated_at = NOW()
                     WHERE job_id = $3 AND round = 1 AND review_index = $4"
                )
                .bind(&text)
                .bind(STATUS_COMPLETED)
                .bind(job_id)
                .bind(idx)
                .execute(&pool)
                .await?;
                return Ok(text);
            }
            Err(e) => {
                last_error = Some(e);
                if attempt < ROUND1_RETRIES - 1 {
                    sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }
    }

    let error_msg = last_error.unwrap().to_string();
    sqlx::query(
        "UPDATE reviewer_documents SET status = $1, error = $2, updated_at = NOW()
         WHERE job_id = $3 AND round = 1 AND review_index = $4",
    )
    .bind(STATUS_FAILED)
    .bind(&error_msg)
    .bind(job_id)
    .bind(idx)
    .execute(&pool)
    .await?;

    Err(anyhow!(
        "Round 1 review {idx} failed after {ROUND1_RETRIES} attempts: {error_msg}"
    ))
}

async fn run_round2_review(
    pool: &PgPool,
    llm_client: &LlmClient,
    job_id: i32,
    pdf_path: &Path,
    prompt: &str,
    combined_reviews: &str,
    model: &str,
) -> Result<String> {
    sqlx::query(
        "INSERT INTO reviewer_documents (job_id, round, review_index, model_name, status)
         VALUES ($1, 2, NULL, $2, $3)",
    )
    .bind(job_id)
    .bind(model)
    .bind(STATUS_PROCESSING)
    .execute(pool)
    .await?;

    let full_prompt = format!("{}\n\n{}", prompt, combined_reviews);
    let text = call_llm(llm_client, model, &full_prompt, pdf_path).await?;

    sqlx::query(
        "UPDATE reviewer_documents SET review_text = $1, status = $2, updated_at = NOW()
         WHERE job_id = $3 AND round = 2",
    )
    .bind(&text)
    .bind(STATUS_COMPLETED)
    .bind(job_id)
    .execute(pool)
    .await?;

    Ok(text)
}

async fn run_round3_review(
    pool: &PgPool,
    llm_client: &LlmClient,
    job_id: i32,
    pdf_path: &Path,
    prompt: &str,
    round2_text: &str,
    model: &str,
) -> Result<String> {
    sqlx::query(
        "INSERT INTO reviewer_documents (job_id, round, review_index, model_name, status)
         VALUES ($1, 3, NULL, $2, $3)",
    )
    .bind(job_id)
    .bind(model)
    .bind(STATUS_PROCESSING)
    .execute(pool)
    .await?;

    let full_prompt = format!("{}\n\n=== Review Report ===\n\n{}", prompt, round2_text);
    let text = call_llm(llm_client, model, &full_prompt, pdf_path).await?;

    sqlx::query(
        "UPDATE reviewer_documents SET review_text = $1, status = $2, updated_at = NOW()
         WHERE job_id = $3 AND round = 3",
    )
    .bind(&text)
    .bind(STATUS_COMPLETED)
    .bind(job_id)
    .execute(pool)
    .await?;

    Ok(text)
}

async fn call_llm(
    llm_client: &LlmClient,
    model: &str,
    prompt: &str,
    pdf_path: &Path,
) -> Result<String> {
    let pdf_bytes = fs::read(pdf_path)?;
    let attachment = FileAttachment::new(
        "manuscript.pdf",
        "application/pdf",
        AttachmentKind::Pdf,
        pdf_bytes,
    );

    let request = LlmRequest::new(
        model.to_string(),
        vec![ChatMessage::new(MessageRole::User, prompt)],
    )
    .with_attachments(vec![attachment]);

    let response = llm_client.execute(request).await?;
    Ok(response.text)
}

async fn text_to_docx(text: &str, output_path: &Path) -> Result<()> {
    use docx_rs::*;

    let mut doc = Docx::new();
    for paragraph_text in text.split("\n\n") {
        let para = Paragraph::new().add_run(Run::new().add_text(paragraph_text));
        doc = doc.add_paragraph(para);
    }

    let file = fs::File::create(output_path)
        .with_context(|| format!("failed to create DOCX at {}", output_path.display()))?;
    doc.build()
        .pack(file)
        .with_context(|| format!("failed to pack DOCX to {}", output_path.display()))?;

    Ok(())
}
