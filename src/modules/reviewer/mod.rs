use std::{
    fs,
    io::Read,
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
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use tokio::{fs as tokio_fs, time::sleep};
use tracing::error;
use uuid::Uuid;

mod admin;

use crate::{
    AppState, render_footer, SESSION_COOKIE,
    llm::{AttachmentKind, ChatMessage, FileAttachment, LlmClient, LlmRequest, MessageRole},
    usage::{self, MODULE_REVIEWER},
};

use quick_xml::{Reader as XmlReader, events::Event};
use zip::ZipArchive;

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
        .route("/api/reviewer/jobs/:job_id/round/:round/review/:idx/download", get(download_review))
        .route("/dashboard/modules/reviewer", get(admin::settings_page))
        .route("/dashboard/modules/reviewer/models", post(admin::save_models))
        .route("/dashboard/modules/reviewer/prompts", post(admin::save_prompts))
}

#[derive(sqlx::FromRow, Clone)]
struct SessionUser {
    id: Uuid,
    username: String,
    is_admin: bool,
}

#[derive(sqlx::FromRow)]
struct JobRow {
    job_id: i32,
    user_id: Uuid,
    filename: String,
    language: String,
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

async fn reviewer_page(jar: CookieJar) -> Result<Html<String>, Response> {
    let _session_id = jar
        .get(SESSION_COOKIE)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Not authenticated").into_response())?
        .value();

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>审稿助手 - Zhang Group AI Toolkit</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 900px;
            margin: 40px auto;
            padding: 0 20px;
            line-height: 1.6;
            color: #333;
        }}
        h1 {{
            color: #2c3e50;
            border-bottom: 3px solid #3498db;
            padding-bottom: 10px;
        }}
        .description {{
            background: #f8f9fa;
            padding: 15px;
            border-left: 4px solid #3498db;
            margin: 20px 0;
        }}
        .form-group {{
            margin: 20px 0;
        }}
        label {{
            display: block;
            font-weight: 600;
            margin-bottom: 5px;
            color: #555;
        }}
        input[type="file"], select {{
            width: 100%;
            padding: 10px;
            border: 2px solid #ddd;
            border-radius: 4px;
            font-size: 14px;
        }}
        input[type="file"]:focus, select:focus {{
            outline: none;
            border-color: #3498db;
        }}
        button {{
            background: #3498db;
            color: white;
            border: none;
            padding: 12px 30px;
            font-size: 16px;
            border-radius: 4px;
            cursor: pointer;
            margin-top: 10px;
        }}
        button:hover {{
            background: #2980b9;
        }}
        button:disabled {{
            background: #95a5a6;
            cursor: not-allowed;
        }}
        #status {{
            margin-top: 20px;
            padding: 15px;
            border-radius: 4px;
            display: none;
        }}
        .status-processing {{
            background: #fff3cd;
            border: 1px solid #ffc107;
        }}
        .status-completed {{
            background: #d4edda;
            border: 1px solid #28a745;
        }}
        .status-failed {{
            background: #f8d7da;
            border: 1px solid #dc3545;
        }}
        .review-link {{
            display: inline-block;
            margin: 5px 10px 5px 0;
            padding: 8px 15px;
            background: #e9ecef;
            border-radius: 4px;
            text-decoration: none;
            color: #495057;
        }}
        .review-link:hover {{
            background: #dee2e6;
        }}
    </style>
</head>
<body>
    <h1>审稿助手 (Academic Review Agent)</h1>

    <div class="description">
        <p><strong>功能说明：</strong>上传学术稿件（PDF或DOCX），系统将使用8个不同的LLM模型并行进行首轮审稿，然后生成元审稿报告，最后进行事实核查。所有报告均可下载为DOCX文件。</p>
        <p><strong>工作流程：</strong></p>
        <ul>
            <li>第一轮：8个模型并行审稿（每个最多重试3次，至少需要4个成功）</li>
            <li>第二轮：综合所有第一轮报告生成元审稿</li>
            <li>第三轮：基于原稿进行事实核查</li>
        </ul>
    </div>

    <form id="uploadForm" enctype="multipart/form-data">
        <div class="form-group">
            <label for="file">上传稿件（PDF或DOCX）：</label>
            <input type="file" id="file" name="file" accept=".pdf,.docx" required>
        </div>

        <div class="form-group">
            <label for="language">审稿语言：</label>
            <select id="language" name="language" required>
                <option value="english">English</option>
                <option value="chinese">中文</option>
            </select>
        </div>

        <button type="submit" id="submitBtn">开始审稿</button>
    </form>

    <div id="status"></div>

    <script>
        const form = document.getElementById('uploadForm');
        const statusDiv = document.getElementById('status');
        const submitBtn = document.getElementById('submitBtn');
        let pollInterval;

        form.addEventListener('submit', async (e) => {{
            e.preventDefault();

            const formData = new FormData(form);
            submitBtn.disabled = true;
            statusDiv.style.display = 'block';
            statusDiv.className = 'status-processing';
            statusDiv.innerHTML = '正在提交任务...';

            try {{
                const response = await fetch('/tools/reviewer/jobs', {{
                    method: 'POST',
                    body: formData
                }});

                if (!response.ok) {{
                    const error = await response.text();
                    throw new Error(error || 'Upload failed');
                }}

                const data = await response.json();
                pollJobStatus(data.job_id);
            }} catch (err) {{
                statusDiv.className = 'status-failed';
                statusDiv.innerHTML = `<strong>错误：</strong>${{err.message}}`;
                submitBtn.disabled = false;
            }}
        }});

        function pollJobStatus(jobId) {{
            pollInterval = setInterval(async () => {{
                try {{
                    const response = await fetch(`/api/reviewer/jobs/${{jobId}}`);
                    const data = await response.json();

                    let html = `<strong>状态：</strong>${{data.status}}`;
                    if (data.status_detail) {{
                        html += `<br>${{data.status_detail}}`;
                    }}

                    if (data.round1_reviews) {{
                        html += '<br><br><strong>第一轮审稿（8个独立评审）：</strong><br>';
                        data.round1_reviews.forEach((r, i) => {{
                            if (r.available && r.download_url) {{
                                html += `<a href="${{r.download_url}}" class="review-link">下载评审 ${{i + 1}} (${{r.model}})</a>`;
                            }}
                        }});
                    }}

                    if (data.round2_review && data.round2_review.available) {{
                        html += '<br><br><strong>第二轮综合审稿：</strong><br>';
                        html += `<a href="${{data.round2_review.download_url}}" class="review-link">下载元审稿报告</a>`;
                    }}

                    if (data.round3_review && data.round3_review.available) {{
                        html += '<br><br><strong>第三轮事实核查：</strong><br>';
                        html += `<a href="${{data.round3_review.download_url}}" class="review-link">下载最终报告</a>`;
                    }}

                    if (data.error) {{
                        html += `<br><br><strong>错误信息：</strong>${{data.error}}`;
                    }}

                    statusDiv.innerHTML = html;

                    if (data.status === 'completed') {{
                        statusDiv.className = 'status-completed';
                        clearInterval(pollInterval);
                        submitBtn.disabled = false;
                    }} else if (data.status === 'failed') {{
                        statusDiv.className = 'status-failed';
                        clearInterval(pollInterval);
                        submitBtn.disabled = false;
                    }}
                }} catch (err) {{
                    console.error('Polling error:', err);
                }}
            }}, 2000);
        }}
    </script>

    {footer}
</body>
</html>"#,
        footer = render_footer()
    );

    Ok(Html(html))
}

#[derive(Deserialize)]
struct CreateJobForm {
    language: String,
}

async fn create_job(
    State(state): State<AppState>,
    jar: CookieJar,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, Response> {
    let user = require_user(&state, &jar).await?;

    // Check usage limits
    if let Err(e) = usage::ensure_within_limits(state.pool_ref(), user.id, MODULE_REVIEWER, 1).await {
        return Err((StatusCode::TOO_MANY_REQUESTS, e.message()).into_response());
    }

    let mut filename = String::new();
    let mut language = String::from("english");
    let mut file_data: Option<Vec<u8>> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        (StatusCode::BAD_REQUEST, format!("Multipart error: {e}")).into_response()
    })? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                filename = field
                    .file_name()
                    .unwrap_or("manuscript.pdf")
                    .to_string();
                file_data = Some(field.bytes().await.map_err(|e| {
                    (StatusCode::BAD_REQUEST, format!("Failed to read file: {e}")).into_response()
                })?.to_vec());
            }
            "language" => {
                language = field.text().await.map_err(|e| {
                    (StatusCode::BAD_REQUEST, format!("Failed to read language: {e}")).into_response()
                })?;
            }
            _ => {}
        }
    }

    let file_bytes = file_data.ok_or_else(|| {
        (StatusCode::BAD_REQUEST, "No file provided").into_response()
    })?;

    if !language.eq("english") && !language.eq("chinese") {
        return Err((StatusCode::BAD_REQUEST, "Invalid language").into_response());
    }

    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    if ext != "pdf" && ext != "docx" {
        return Err((StatusCode::BAD_REQUEST, "Only PDF and DOCX files are accepted").into_response());
    }

    // Create job in database
    let job_id: i32 = sqlx::query_scalar(
        "INSERT INTO reviewer_jobs (user_id, filename, language, status)
         VALUES ($1, $2, $3, $4) RETURNING job_id"
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
        (StatusCode::INTERNAL_SERVER_ERROR, "Failed to create job directory").into_response()
    })?;

    let sanitized_filename = sanitize(&filename);
    let manuscript_path = job_dir.join(&sanitized_filename);
    tokio_fs::write(&manuscript_path, &file_bytes).await.map_err(|e| {
        error!("Failed to write manuscript file: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, "Failed to save file").into_response()
    })?;

    // Spawn background processing
    let pool = state.pool().clone();
    let llm_client = state.llm_client().clone();
    let reviewer_settings = state.reviewer_settings().await.ok_or_else(|| {
        (StatusCode::INTERNAL_SERVER_ERROR, "Reviewer settings not configured").into_response()
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
        ).await {
            error!("Job {job_id} failed: {e}");
            let _ = sqlx::query(
                "UPDATE reviewer_jobs SET status = $1, status_detail = $2, updated_at = NOW()
                 WHERE job_id = $3"
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
        "SELECT job_id, user_id, filename, language, status, status_detail
         FROM reviewer_jobs WHERE job_id = $1"
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
        review_text: Option<String>,
        file_path: Option<String>,
        status: String,
    }

    let docs = sqlx::query_as::<_, DocRow>(
        "SELECT round, review_index, model_name, review_text, file_path, status
         FROM reviewer_documents WHERE job_id = $1 ORDER BY round, review_index"
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
        match doc.round {
            1 => {
                let idx = doc.review_index.unwrap_or(0);
                round1_reviews.push(ReviewInfo {
                    model: doc.model_name,
                    available: doc.status == "completed",
                    download_url: if doc.status == "completed" {
                        Some(format!("/api/reviewer/jobs/{job_id}/round/1/review/{idx}/download"))
                    } else {
                        None
                    },
                });
            }
            2 => {
                round2_review = Some(ReviewInfo {
                    model: doc.model_name,
                    available: doc.status == "completed",
                    download_url: if doc.status == "completed" {
                        Some(format!("/api/reviewer/jobs/{job_id}/round/2/review/0/download"))
                    } else {
                        None
                    },
                });
            }
            3 => {
                round3_review = Some(ReviewInfo {
                    model: doc.model_name,
                    available: doc.status == "completed",
                    download_url: if doc.status == "completed" {
                        Some(format!("/api/reviewer/jobs/{job_id}/round/3/review/0/download"))
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
        round1_reviews: if !round1_reviews.is_empty() { Some(round1_reviews) } else { None },
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
        "SELECT job_id, user_id, filename, language, status, status_detail
         FROM reviewer_jobs WHERE job_id = $1"
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

    let file_path = doc.file_path.ok_or_else(|| {
        (StatusCode::NOT_FOUND, "File not available").into_response()
    })?;

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
            ("Content-Type", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
            ("Content-Disposition", &format!("attachment; filename=\"{}\"", filename)),
        ],
        bytes,
    ).into_response())
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
         WHERE job_id = $3"
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
        "UPDATE reviewer_jobs SET status_detail = $1, updated_at = NOW() WHERE job_id = $2"
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
            ).await
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
        "UPDATE reviewer_jobs SET status_detail = $1, updated_at = NOW() WHERE job_id = $2"
    )
    .bind(format!("Round 1 completed: {}/{} reviews succeeded", round1_results.len(), 8))
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
             WHERE job_id = $2 AND round = 1 AND review_index = $3"
        )
        .bind(docx_path.to_string_lossy().to_string())
        .bind(job_id)
        .bind(*idx as i32)
        .execute(&pool)
        .await?;
    }

    // Round 2: Meta-review
    sqlx::query(
        "UPDATE reviewer_jobs SET status_detail = $1, updated_at = NOW() WHERE job_id = $2"
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

    let combined_reviews = round1_results.iter()
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
    ).await?;

    let round2_docx = PathBuf::from(STORAGE_ROOT)
        .join(job_id.to_string())
        .join("round2_meta_review.docx");
    text_to_docx(&round2_text, &round2_docx).await?;

    sqlx::query(
        "UPDATE reviewer_documents SET file_path = $1, updated_at = NOW()
         WHERE job_id = $2 AND round = 2"
    )
    .bind(round2_docx.to_string_lossy().to_string())
    .bind(job_id)
    .execute(&pool)
    .await?;

    // Round 3: Fact-checking
    sqlx::query(
        "UPDATE reviewer_jobs SET status_detail = $1, updated_at = NOW() WHERE job_id = $2"
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
    ).await?;

    let round3_docx = PathBuf::from(STORAGE_ROOT)
        .join(job_id.to_string())
        .join("round3_final_report.docx");
    text_to_docx(&round3_text, &round3_docx).await?;

    sqlx::query(
        "UPDATE reviewer_documents SET file_path = $1, updated_at = NOW()
         WHERE job_id = $2 AND round = 3"
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
         WHERE job_id = $3"
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
         VALUES ($1, 1, $2, $3, $4)"
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
         WHERE job_id = $3 AND round = 1 AND review_index = $4"
    )
    .bind(STATUS_FAILED)
    .bind(&error_msg)
    .bind(job_id)
    .bind(idx)
    .execute(&pool)
    .await?;

    Err(anyhow!("Round 1 review {idx} failed after {ROUND1_RETRIES} attempts: {error_msg}"))
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
         VALUES ($1, 2, NULL, $2, $3)"
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
         WHERE job_id = $3 AND round = 2"
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
         VALUES ($1, 3, NULL, $2, $3)"
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
         WHERE job_id = $3 AND round = 3"
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

fn extract_docx_text(path: &Path) -> Result<String> {
    let file = fs::File::open(path)
        .with_context(|| format!("failed to open DOCX file {}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("failed to open DOCX archive {}", path.display()))?;

    let mut document = archive
        .by_name("word/document.xml")
        .with_context(|| format!("missing word/document.xml in {}", path.display()))?;

    let mut xml = String::new();
    document
        .read_to_string(&mut xml)
        .with_context(|| format!("failed to read DOCX XML for {}", path.display()))?;

    let mut reader = XmlReader::from_str(&xml);
    let mut buf = Vec::new();
    let mut output = String::new();
    let mut in_text_node = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"w:p" => {
                    if !output.is_empty() {
                        output.push_str("\n\n");
                    }
                }
                b"w:tab" => output.push('\t'),
                b"w:br" => output.push('\n'),
                b"w:t" => in_text_node = true,
                _ => {}
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"w:p" => {
                    if !output.is_empty() {
                        output.push_str("\n\n");
                    }
                }
                b"w:tab" => output.push('\t'),
                b"w:br" => output.push('\n'),
                _ => {}
            },
            Ok(Event::Text(e)) => {
                if in_text_node {
                    let value = e.unescape().map_err(|err| anyhow!(err))?.into_owned();
                    output.push_str(&value);
                }
            }
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == b"w:t" {
                    in_text_node = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(anyhow!("failed to parse DOCX XML: {}", err)),
            _ => {}
        }
        buf.clear();
    }

    Ok(output.trim().to_string())
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
