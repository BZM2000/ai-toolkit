use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{Multipart, Path as AxumPath, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use tokio::{fs as tokio_fs, time::sleep};
use tracing::error;
use uuid::Uuid;

mod admin;

use crate::web::history_ui;
use crate::web::{
    FileFieldConfig, FileNaming, ToolAdminLink, ToolPageLayout, UPLOAD_WIDGET_SCRIPT,
    UPLOAD_WIDGET_STYLES, UploadWidgetConfig, process_upload_form, render_tool_page,
    render_upload_widget,
};
use crate::{
    AppState, escape_html, history,
    llm::{AttachmentKind, ChatMessage, FileAttachment, LlmClient, LlmRequest, MessageRole},
    render_footer,
    usage::{self, MODULE_REVIEWER},
    utils::docx_to_pdf::convert_docx_to_pdf,
    web::{
        auth::{self, JsonAuthError},
        json_error,
    },
};

const STORAGE_ROOT: &str = "storage/reviewer";
const STATUS_PENDING: &str = "pending";
const STATUS_PROCESSING: &str = "processing";
const STATUS_COMPLETED: &str = "completed";
const STATUS_FAILED: &str = "failed";

const ROUND1_RETRIES: usize = 3;
const ROUND1_MIN_SUCCESSES: usize = 4;

fn json_response(status: StatusCode, message: impl Into<String>) -> Response {
    json_error(status, message).into_response()
}

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

#[derive(sqlx::FromRow)]
struct JobRow {
    user_id: Uuid,
    status: String,
    status_detail: Option<String>,
    files_purged_at: Option<chrono::DateTime<Utc>>,
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

async fn reviewer_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Html<String>, Redirect> {
    let user = auth::require_user_redirect(&state, &jar).await?;

    let username = escape_html(&user.username);
    let note_html = format!(
        "当前登录：<strong>{username}</strong>。上传稿件后，系统将自动执行三轮审稿，生成可下载的 DOCX 报告。",
        username = username,
    );
    let admin_link = if user.is_admin {
        Some(ToolAdminLink {
            href: "/dashboard/modules/reviewer",
            label: "模块管理",
        })
    } else {
        None
    };
    let upload_widget = render_upload_widget(
        &UploadWidgetConfig::new("reviewer-upload", "reviewer-file", "file", "稿件文件")
            .with_description("支持上传 PDF 或 DOCX。DOCX 将自动转换为 PDF 参与审稿。")
            .with_accept(".pdf,.docx"),
    );
    let history_panel = history_ui::render_history_panel(MODULE_REVIEWER);
    let new_tab_html = format!(
        r#"                <section class="panel">
                    <h2>提交稿件</h2>
                    <form id="reviewer-form">
                        {upload_widget}
                        <label for="language">审稿语言</label>
                        <select id="language" name="language">
                            <option value="english">英文</option>
                            <option value="chinese">中文</option>
                        </select>
                        <button type="submit">开始审稿</button>
                    </form>
                    <div id="submission-status" class="status-box">等待上传。</div>
                </section>
                <section class="panel">
                    <h2>任务进度</h2>
                    <div id="job-status"></div>
                </section>
"#,
        upload_widget = upload_widget,
    );

    let reviewer_script = r#"const form = document.getElementById('reviewer-form');
const statusBox = document.getElementById('submission-status');
const jobStatus = document.getElementById('job-status');
const fileInput = document.getElementById('reviewer-file');
const languageSelect = document.getElementById('language');
let pollTimer = null;

const setStatus = (message, type = null) => {
    statusBox.textContent = message;
    statusBox.classList.remove('error', 'success');
    if (type) {
        statusBox.classList.add(type);
    }
};

const stopPolling = () => {
    if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = null;
    }
};

const renderReviewCard = (title, review) => {
    const status = review.available ? 'completed' : 'processing';
    const tag = `<span class="status-tag ${status}">${status}</span>`;
    const download = review.download_url
        ? `<p class="downloads"><a href="${review.download_url}">下载 DOCX</a></p>`
        : '';
    return `
        <div class="review-card">
            <h3>${title} ${tag}</h3>
            <p class="note">模型：${review.model}</p>
            ${download}
        </div>
    `;
};

const renderJobStatus = (payload) => {
    if (!payload) {
        jobStatus.innerHTML = '<p class="note">暂无任务记录。</p>';
        return;
    }

    const reviews = [];
    if (payload.round1_reviews && payload.round1_reviews.length) {
        payload.round1_reviews.forEach((review, idx) => {
            reviews.push(renderReviewCard(`第一轮评审 ${idx + 1}`, review));
        });
    }
    if (payload.round2_review) {
        reviews.push(renderReviewCard('第二轮元审稿', payload.round2_review));
    }
    if (payload.round3_review) {
        reviews.push(renderReviewCard('第三轮事实核查', payload.round3_review));
    }

    const cards = reviews.length ? reviews.join('') : '<p class="note">评审结果准备中...</p>';
    const detail = payload.status_detail ? `<p class="note">${payload.status_detail}</p>` : '';

    jobStatus.innerHTML = `
        <div class="status">
            <p><strong>任务状态：</strong> ${payload.status}</p>
            ${detail}
            <div class="reviews">${cards}</div>
        </div>
    `;
};

const fetchStatus = async (jobId) => {
    try {
        const response = await fetch(`/api/reviewer/jobs/${jobId}`, { headers: { 'Accept': 'application/json' } });
        if (!response.ok) {
            throw new Error('状态查询失败');
        }
        const payload = await response.json();
        renderJobStatus(payload);

        if (payload.status === 'completed' || payload.status === 'failed') {
            stopPolling();
            if (payload.status === 'completed') {
                setStatus('审稿完成，可查看下方下载链接。', 'success');
            } else {
                setStatus('任务失败，请查看状态信息。', 'error');
            }
        }
    } catch (error) {
        stopPolling();
        setStatus('轮询失败：' + error.message, 'error');
    }
};

form.addEventListener('submit', async (event) => {
    event.preventDefault();
    if (!fileInput || fileInput.files.length === 0) {
        setStatus('请先选择稿件文件。', 'error');
        return;
    }

    stopPolling();
    setStatus('正在上传稿件...', null);

    const formData = new FormData(form);

    try {
        const response = await fetch('/tools/reviewer/jobs', {
            method: 'POST',
            body: formData,
        });

        if (!response.ok) {
            const payload = await response.json().catch(() => ({ message: '提交失败。' }));
            setStatus(payload.message || '提交失败。', 'error');
            return;
        }

        const payload = await response.json();
        setStatus('任务已创建，正在执行审稿流程...', 'success');
        renderJobStatus(null);
        fetchStatus(payload.job_id);
        pollTimer = setInterval(() => fetchStatus(payload.job_id), 5000);
        form.reset();
        if (fileInput) {
            fileInput.value = '';
            fileInput.dispatchEvent(new Event('change'));
        }
    } catch (error) {
        setStatus('提交失败：' + error.message, 'error');
    }
});
"#;

    let html = render_tool_page(ToolPageLayout {
        meta_title: "审稿助手 | Zhang Group AI Toolkit",
        page_heading: "审稿助手",
        username: &username,
        note_html: Cow::Owned(note_html),
        tab_group: "reviewer",
        new_tab_label: "新任务",
        new_tab_html: Cow::Owned(new_tab_html),
        history_tab_label: "历史记录",
        history_panel_html: Cow::Owned(history_panel),
        admin_link,
        footer_html: Cow::Owned(render_footer()),
        extra_style_blocks: vec![
            Cow::Borrowed(history_ui::HISTORY_STYLES),
            Cow::Borrowed(UPLOAD_WIDGET_STYLES),
        ],
        body_scripts: vec![
            Cow::Borrowed(UPLOAD_WIDGET_SCRIPT),
            Cow::Owned(format!(
                "<script>
{}
</script>",
                reviewer_script
            )),
            Cow::Owned(format!(
                "<script>
{}
</script>",
                history_ui::HISTORY_SCRIPT
            )),
        ],
    });

    Ok(Html(html))
}

async fn create_job(
    State(state): State<AppState>,
    jar: CookieJar,
    multipart: Multipart,
) -> Result<Json<serde_json::Value>, Response> {
    let user = auth::current_user_or_json_error(&state, &jar)
        .await
        .map_err(|JsonAuthError { status, message }| json_response(status, message))?;

    if let Err(e) = usage::ensure_within_limits(state.pool_ref(), user.id, MODULE_REVIEWER, 1).await
    {
        return Err(json_response(StatusCode::TOO_MANY_REQUESTS, e.message()));
    }

    let temp_dir = PathBuf::from(STORAGE_ROOT).join(format!("tmp_{}", Uuid::new_v4()));
    let file_config = FileFieldConfig::new(
        "file",
        &["pdf", "docx"],
        1,
        FileNaming::PrefixOnly {
            prefix: "manuscript_",
        },
    )
    .with_min_files(1);

    let upload = match process_upload_form(multipart, &temp_dir, &[file_config]).await {
        Ok(outcome) => outcome,
        Err(err) => {
            let _ = tokio_fs::remove_dir_all(&temp_dir).await;
            return Err(json_response(
                StatusCode::BAD_REQUEST,
                err.message().to_string(),
            ));
        }
    };

    let language = upload
        .first_text("language")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "english".to_string());

    if language != "english" && language != "chinese" {
        let _ = tokio_fs::remove_dir_all(&temp_dir).await;
        return Err(json_response(StatusCode::BAD_REQUEST, "Invalid language"));
    }

    let file = match upload.first_file_for("file").cloned() {
        Some(file) => file,
        None => {
            let _ = tokio_fs::remove_dir_all(&temp_dir).await;
            return Err(json_response(StatusCode::BAD_REQUEST, "No file provided"));
        }
    };

    let ext = file
        .original_name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();
    if ext != "pdf" && ext != "docx" {
        let _ = tokio_fs::remove_dir_all(&temp_dir).await;
        return Err(json_response(
            StatusCode::BAD_REQUEST,
            "Only PDF and DOCX files are accepted",
        ));
    }

    let job_id: i32 = match sqlx::query_scalar(
        "INSERT INTO reviewer_jobs (user_id, filename, language, status)
         VALUES ($1, $2, $3, $4) RETURNING job_id",
    )
    .bind(user.id)
    .bind(&file.original_name)
    .bind(&language)
    .bind(STATUS_PENDING)
    .fetch_one(state.pool_ref())
    .await
    {
        Ok(id) => id,
        Err(e) => {
            let _ = tokio_fs::remove_dir_all(&temp_dir).await;
            error!("Failed to create job: {e}");
            return Err(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to create job",
            ));
        }
    };

    let final_dir = PathBuf::from(STORAGE_ROOT).join(job_id.to_string());
    if let Err(e) = tokio_fs::create_dir_all(&final_dir).await {
        let _ = tokio_fs::remove_dir_all(&temp_dir).await;
        error!("Failed to create job directory: {e}");
        return Err(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to create job directory",
        ));
    }

    let manuscript_path = final_dir.join(&file.stored_name);
    if let Err(e) = tokio_fs::rename(&file.stored_path, &manuscript_path).await {
        let _ = tokio_fs::remove_dir_all(&temp_dir).await;
        error!("Failed to persist manuscript: {e}");
        return Err(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to save file",
        ));
    }
    let _ = tokio_fs::remove_dir_all(&temp_dir).await;

    let pool = state.pool().clone();
    let llm_client = state.llm_client().clone();
    let reviewer_settings = state.reviewer_settings().await.ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Reviewer settings not configured",
        )
            .into_response()
    })?;

    if let Err(err) =
        history::record_job_start(&pool, MODULE_REVIEWER, user.id, job_id.to_string()).await
    {
        error!(?err, job_id, "failed to record reviewer job history");
    }

    let language_clone = language.clone();
    let ext_clone = ext.clone();
    tokio::spawn(async move {
        if let Err(e) = process_reviewer_job(
            pool.clone(),
            llm_client,
            job_id,
            user.id,
            manuscript_path.clone(),
            &language_clone,
            &ext_clone,
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
    let user = auth::current_user_or_json_error(&state, &jar)
        .await
        .map_err(|JsonAuthError { status, message }| {
            (status, Json(json!({ "message": message }))).into_response()
        })?;

    let job = sqlx::query_as::<_, JobRow>(
        "SELECT user_id, status, status_detail, files_purged_at
         FROM reviewer_jobs WHERE job_id = $1",
    )
    .bind(job_id)
    .fetch_optional(state.pool_ref())
    .await
    .map_err(|e| {
        error!("Database error: {e}");
        json_response(StatusCode::INTERNAL_SERVER_ERROR, "Database error")
    })?
    .ok_or_else(|| json_response(StatusCode::NOT_FOUND, "Job not found"))?;

    if job.user_id != user.id && !user.is_admin {
        return Err(json_response(StatusCode::FORBIDDEN, "Access denied"));
    }

    if job.files_purged_at.is_some() {
        return Err(json_response(StatusCode::GONE, "审稿文件已过期并被清除。"));
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
        json_response(StatusCode::INTERNAL_SERVER_ERROR, "Database error")
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
    let user = auth::current_user_or_json_error(&state, &jar)
        .await
        .map_err(|JsonAuthError { status, message }| {
            (status, Json(json!({ "message": message }))).into_response()
        })?;

    let job = sqlx::query_as::<_, JobRow>(
        "SELECT user_id, status, status_detail, files_purged_at
         FROM reviewer_jobs WHERE job_id = $1",
    )
    .bind(job_id)
    .fetch_optional(state.pool_ref())
    .await
    .map_err(|e| {
        error!("Database error: {e}");
        json_response(StatusCode::INTERNAL_SERVER_ERROR, "Database error")
    })?
    .ok_or_else(|| json_response(StatusCode::NOT_FOUND, "Job not found"))?;

    if job.user_id != user.id && !user.is_admin {
        return Err(json_response(StatusCode::FORBIDDEN, "Access denied"));
    }

    if job.files_purged_at.is_some() {
        return Err(json_response(StatusCode::GONE, "审稿文件已过期并被清除。"));
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
        json_response(StatusCode::INTERNAL_SERVER_ERROR, "Database error")
    })?
    .ok_or_else(|| json_response(StatusCode::NOT_FOUND, "Review not found"))?;

    let file_path = doc
        .file_path
        .ok_or_else(|| json_response(StatusCode::NOT_FOUND, "File not available"))?;

    let path_buf = PathBuf::from(&file_path);
    let filename = path_buf
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("review.docx");

    let bytes = tokio_fs::read(&file_path).await.map_err(|e| {
        error!("Failed to read file {file_path}: {e}");
        json_response(StatusCode::INTERNAL_SERVER_ERROR, "Failed to read file")
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
