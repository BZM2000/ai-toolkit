use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{Multipart, Path as AxumPath, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Utc};
use docx_rs::{BreakType, Docx, Paragraph, Run};
use quick_xml::{Reader as XmlReader, events::Event};
use sanitize_filename::sanitize;
use serde::Serialize;
use tokio::{fs as tokio_fs, io::AsyncWriteExt};
use tracing::error;
use uuid::Uuid;
use zip::ZipArchive;

mod admin;

use crate::{
    AppState, GlossaryTermRow,
    config::DocxTranslatorPrompts,
    escape_html, fetch_glossary_terms,
    llm::{ChatMessage, LlmRequest, MessageRole},
    render_footer,
    usage::{self, MODULE_TRANSLATE_DOCX},
};

const STORAGE_ROOT: &str = "storage/translatedocx";
const STATUS_PENDING: &str = "pending";
const STATUS_PROCESSING: &str = "processing";
const STATUS_COMPLETED: &str = "completed";
const STATUS_FAILED: &str = "failed";

const PARAGRAPH_SEPARATOR: &str = "[[__PARAGRAPH_BREAK__]]";
const CHUNK_MAX_PARAGRAPHS: usize = 20;
const CHUNK_MAX_EQUIVALENT_WORDS: f64 = 700.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranslationDirection {
    EnToCn,
    CnToEn,
}

impl TranslationDirection {
    fn as_db_value(self) -> &'static str {
        match self {
            TranslationDirection::EnToCn => "en_to_cn",
            TranslationDirection::CnToEn => "cn_to_en",
        }
    }

    fn display_label(self) -> &'static str {
        match self {
            TranslationDirection::EnToCn => "英文 → 中文",
            TranslationDirection::CnToEn => "中文 → 英文",
        }
    }

    fn from_form_value(value: &str) -> Self {
        match value {
            "cn_to_en" => TranslationDirection::CnToEn,
            _ => TranslationDirection::EnToCn,
        }
    }

    fn from_db_value(value: &str) -> Self {
        Self::from_form_value(value)
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tools/translatedocx", get(translatedocx_page))
        .route("/tools/translatedocx/jobs", post(create_job))
        .route("/api/translatedocx/jobs/:id", get(job_status))
        .route(
            "/api/translatedocx/jobs/:id/documents/:doc_id/download/:variant",
            get(download_document_output),
        )
        .route(
            "/dashboard/modules/translatedocx",
            get(admin::settings_page),
        )
        .route(
            "/dashboard/modules/translatedocx/models",
            post(admin::save_models),
        )
        .route(
            "/dashboard/modules/translatedocx/prompts",
            post(admin::save_prompts),
        )
}

async fn translatedocx_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Html<String>, Redirect> {
    let user = require_user(&state, &jar).await?;

    let footer = render_footer();
    let admin_link = if user.is_admin {
        "<a class=\"admin-link\" href=\"/dashboard/modules/translatedocx\">模块管理</a>"
    } else {
        ""
    };
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>DOCX 文档翻译 | 张圆教授课题组 AI 工具箱</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="robots" content="noindex,nofollow">
    <style>
        :root {{ color-scheme: light; }}
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; margin: 0; background: #f8fafc; color: #0f172a; }}
        header {{ background: #ffffff; padding: 2rem 1.5rem; border-bottom: 1px solid #e2e8f0; }}
        .header-bar {{ display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 1rem; }}
        .back-link {{ display: inline-flex; align-items: center; gap: 0.4rem; color: #1d4ed8; text-decoration: none; font-weight: 600; background: #e0f2fe; padding: 0.5rem 0.95rem; border-radius: 999px; border: 1px solid #bfdbfe; transition: background 0.15s ease, border 0.15s ease; }}
        .back-link:hover {{ background: #bfdbfe; border-color: #93c5fd; }}
        main {{ padding: 2rem 1.5rem; max-width: 960px; margin: 0 auto; box-sizing: border-box; }}
        section {{ margin-bottom: 2.5rem; }}
        .panel {{ background: #ffffff; border-radius: 12px; border: 1px solid #e2e8f0; padding: 1.5rem; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); }}
        label {{ display: block; margin-bottom: 0.5rem; font-weight: 600; color: #0f172a; }}
        button {{ padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        button:disabled {{ opacity: 0.6; cursor: not-allowed; }}
        table {{ width: 100%; border-collapse: collapse; margin-top: 1.5rem; background: #ffffff; border: 1px solid #e2e8f0; border-radius: 12px; overflow: hidden; }}
        th, td {{ padding: 0.75rem 1rem; border-bottom: 1px solid #e2e8f0; text-align: left; }}
        th {{ background: #f1f5f9; color: #0f172a; font-weight: 600; }}
        .admin-link {{ display: inline-flex; align-items: center; gap: 0.35rem; color: #0f172a; background: #fee2e2; border: 1px solid #fecaca; padding: 0.45rem 0.9rem; border-radius: 999px; text-decoration: none; font-weight: 600; }}
        .admin-link:hover {{ background: #fecaca; border-color: #fca5a5; }}
        .status {{ margin-top: 1.5rem; }}
        .status p {{ margin: 0.25rem 0; }}
        .note {{ color: #475569; font-size: 0.95rem; }}
        .downloads a {{ color: #2563eb; text-decoration: none; margin-right: 1rem; }}
        .downloads a:hover {{ text-decoration: underline; }}
        .drop-zone {{ border: 2px dashed #cbd5f5; border-radius: 12px; padding: 2rem; text-align: center; background: #f8fafc; transition: border-color 0.2s ease, background 0.2s ease; cursor: pointer; margin-bottom: 1rem; color: #475569; }}
        .drop-zone.dragover {{ border-color: #2563eb; background: #e0f2fe; }}
        .drop-zone strong {{ color: #1d4ed8; }}
        .drop-zone input[type="file"] {{ display: none; }}
        .browse-link {{ color: #2563eb; text-decoration: underline; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>DOCX 文档翻译</h1>
            <div style="display:flex; gap:0.75rem; align-items:center; flex-wrap:wrap;">
                <a class="back-link" href="/">← 返回首页</a>
                {admin_link}
            </div>
        </div>
        <p class="note">当前登录：<strong>{username}</strong>。上传 DOCX 文件，按照术语表进行精准翻译。</p>
    </header>
    <main>
        <section class="panel">
            <h2>提交新任务</h2>
            <form id="translator-form">
                <label for="files">上传 DOCX 文件</label>
                <div id="drop-area" class="drop-zone">
                    <p><strong>拖拽 DOCX 文件</strong>到此处，或<span class="browse-link">点击选择</span>文件。</p>
                    <p class="note">本工具一次仅支持处理 1 个文件。</p>
                    <input id="files" name="files" type="file" accept=".docx" required>
                </div>
                <label for="direction">翻译方向</label>
                <select id="direction" name="direction">
                    <option value="en_to_cn">英文 → 中文</option>
                    <option value="cn_to_en">中文 → 英文</option>
                </select>
                <button type="submit">开始翻译</button>
            </form>
            <div id="submission-status" class="status"></div>
        </section>
        <section class="panel">
            <h2>任务进度</h2>
            <div id="job-status"></div>
        </section>
        {footer}
    </main>
    <script>
        const form = document.getElementById('translator-form');
        const statusBox = document.getElementById('submission-status');
        const jobStatus = document.getElementById('job-status');
        const dropArea = document.getElementById('drop-area');
        const fileInput = document.getElementById('files');
        let activeJobId = null;
        let statusTimer = null;

        const updateSelectionStatus = () => {{
            if (fileInput.files.length > 0) {{
                statusBox.textContent = `已选择 ${{fileInput.files.length}} 个文件。`;
            }} else {{
                statusBox.textContent = '';
            }}
        }};

        const handleFiles = (list) => {{
            if (!list || !list.length) {{
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

        form.addEventListener('submit', async (event) => {{
            event.preventDefault();
            if (!fileInput.files.length) {{
                statusBox.textContent = '请先选择 DOCX 文件。';
                return;
            }}

            const directionValue = document.getElementById('direction').value;
            const directionLabel = directionValue === 'cn_to_en' ? '中文 → 英文' : '英文 → 中文';
            statusBox.textContent = `正在上传文档（${{directionLabel}}）...`;
            const data = new FormData(form);

            try {{
                const response = await fetch('/tools/translatedocx/jobs', {{
                    method: 'POST',
                    body: data,
                }});

                if (!response.ok) {{
                    const payload = await response.json();
                    statusBox.textContent = payload.message || '任务创建失败。';
                    return;
                }}

                const payload = await response.json();
                activeJobId = payload.job_id;
                statusBox.textContent = '任务已创建，正在监控进度...';
                pollStatus(payload.status_url);
            }} catch (error) {{
                statusBox.textContent = '提交任务失败。';
            }}
        }});

        function pollStatus(url) {{
            if (statusTimer) {{
                clearInterval(statusTimer);
            }}

            const fetchStatus = async () => {{
                try {{
                    const response = await fetch(url);
                    if (!response.ok) {{
                        jobStatus.textContent = '暂时无法加载任务状态。';
                        return;
                    }}
                    const payload = await response.json();
                    renderStatus(payload);

                    if (payload.status === '{completed}' || payload.status === '{failed}') {{
                        clearInterval(statusTimer);
                    }}
                }} catch (error) {{
                    jobStatus.textContent = '暂时无法加载任务状态。';
                }}
            }};

            fetchStatus();
            statusTimer = setInterval(fetchStatus, 4000);
        }}

        function translateStatus(status) {{
            const map = {{
                pending: '待处理',
                processing: '处理中',
                completed: '已完成',
                failed: '已失败',
                queued: '排队中',
            }};
            return map[status] || status;
        }}

        function renderStatus(payload) {{
            if (!payload) {{
                jobStatus.textContent = '';
                return;
            }}

            let docRows = payload.documents.map((doc) => {{
                const downloadLink = doc.translated_download_url ? `<a href="${{doc.translated_download_url}}">下载译文 DOCX</a>` : '处理中';
                const detailRow = doc.status_detail ? `<tr><td colspan="3"><div class="note">${{doc.status_detail}}</div></td></tr>` : '';
                const errorRow = doc.error_message ? `<tr><td colspan="3"><div class="note">${{doc.error_message}}</div></td></tr>` : '';
                const statusLabel = translateStatus(doc.status);
                return `
                    <tr>
                        <td>${{doc.original_filename}}</td>
                        <td>${{statusLabel}}</td>
                        <td class="downloads">${{downloadLink}}</td>
                    </tr>
                    ${{detailRow}}
                    ${{errorRow}}
                `;
            }}).join('');
            if (!docRows) {{
                docRows = '<tr><td colspan="3">暂无文件记录。</td></tr>';
            }}

            const directionBlock = payload.translation_direction ? `<p class="note">翻译方向：${{payload.translation_direction}}</p>` : '';
            const detailBlock = payload.status_detail ? `<p class="note">${{payload.status_detail}}</p>` : '';
            const errorBlock = payload.error_message ? `<p class="note">${{payload.error_message}}</p>` : '';
            const jobStatusLabel = translateStatus(payload.status);

            jobStatus.innerHTML = `
                <div class="status">
                    <p><strong>任务状态：</strong> ${{jobStatusLabel}}</p>
                    ${{directionBlock}}
                    ${{detailBlock}}
                    ${{errorBlock}}
                    <table>
                        <thead><tr><th>文件名</th><th>状态</th><th>下载</th></tr></thead>
                        <tbody>${{docRows}}</tbody>
                    </table>
                </div>
            `;
        }}
    </script>
</body>
</html>"#,
        username = escape_html(&user.username),
        completed = STATUS_COMPLETED,
        failed = STATUS_FAILED,
        footer = footer,
        admin_link = admin_link,
    );

    Ok(Html(html))
}

async fn create_job(
    State(state): State<AppState>,
    jar: CookieJar,
    mut multipart: Multipart,
) -> Result<Json<JobSubmissionResponse>, (StatusCode, Json<ApiError>)> {
    let user = require_user(&state, &jar)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ApiError::new("请先登录。"))))?;

    ensure_storage_root()
        .await
        .map_err(|err| internal_error(err.into()))?;

    let job_id = Uuid::new_v4();
    let job_dir = PathBuf::from(STORAGE_ROOT).join(job_id.to_string());
    tokio_fs::create_dir_all(&job_dir)
        .await
        .map_err(|err| internal_error(err.into()))?;

    let mut uploaded_file: Option<UploadedFile> = None;
    let mut direction = TranslationDirection::EnToCn;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| internal_error(err.into()))?
    {
        let Some(name) = field.name() else {
            continue;
        };

        match name {
            "files" => {
                let Some(filename) = field.file_name().map(|s| s.to_string()) else {
                    continue;
                };
                if uploaded_file.is_some() {
                    let _ = tokio_fs::remove_dir_all(&job_dir).await;
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(ApiError::new("每个任务仅支持上传一个 DOCX 文件。")),
                    ));
                }
                let safe_name = sanitize(&filename);
                let ext = Path::new(&safe_name)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if ext != "docx" {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(ApiError::new("仅支持上传 DOCX 文件。")),
                    ));
                }

                let stored_path = job_dir.join(format!("source_{}", safe_name));

                let mut file = tokio_fs::File::create(&stored_path)
                    .await
                    .map_err(|err| internal_error(err.into()))?;
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|err| internal_error(err.into()))?;
                file.write_all(&bytes)
                    .await
                    .map_err(|err| internal_error(err.into()))?;
                file.flush()
                    .await
                    .map_err(|err| internal_error(err.into()))?;

                uploaded_file = Some(UploadedFile {
                    stored_path,
                    original_name: filename,
                });
            }
            "direction" => {
                let value = field
                    .text()
                    .await
                    .map_err(|err| internal_error(err.into()))?;
                direction = TranslationDirection::from_form_value(value.trim());
            }
            _ => {}
        }
    }

    let Some(file) = uploaded_file else {
        let _ = tokio_fs::remove_dir_all(&job_dir).await;
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("请上传一个 DOCX 文件。")),
        ));
    };

    let pool = state.pool();

    if let Err(err) = usage::ensure_within_limits(&pool, user.id, MODULE_TRANSLATE_DOCX, 1).await {
        let _ = tokio_fs::remove_dir_all(&job_dir).await;
        return Err((StatusCode::FORBIDDEN, Json(ApiError::new(err.message()))));
    }

    let mut transaction = pool
        .begin()
        .await
        .map_err(|err| internal_error(err.into()))?;

    sqlx::query(
        "INSERT INTO docx_jobs (id, user_id, status, translation_direction) VALUES ($1, $2, $3, $4)",
    )
    .bind(job_id)
    .bind(user.id)
    .bind(STATUS_PENDING)
    .bind(direction.as_db_value())
    .execute(&mut *transaction)
    .await
    .map_err(|err| internal_error(err.into()))?;

    sqlx::query(
        "INSERT INTO docx_documents (id, job_id, original_filename, source_path, status) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(job_id)
    .bind(&file.original_name)
    .bind(file.stored_path.to_string_lossy().to_string())
    .bind(STATUS_PENDING)
    .execute(&mut *transaction)
    .await
    .map_err(|err| internal_error(err.into()))?;

    transaction
        .commit()
        .await
        .map_err(|err| internal_error(err.into()))?;

    spawn_job_worker(state.clone(), job_id);

    Ok(Json(JobSubmissionResponse {
        job_id,
        status_url: format!("/api/translatedocx/jobs/{}", job_id),
    }))
}

async fn job_status(
    State(state): State<AppState>,
    jar: CookieJar,
    AxumPath(job_id): AxumPath<Uuid>,
) -> Result<Json<JobStatusResponse>, (StatusCode, Json<ApiError>)> {
    let user = require_user(&state, &jar)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ApiError::new("请先登录。"))))?;

    let pool = state.pool();

    let job = sqlx::query_as::<_, JobRecord>(
        "SELECT id, user_id, status, status_detail, error_message, translation_direction, created_at, updated_at FROM docx_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_optional(&pool)
    .await
    .map_err(|err| internal_error(err.into()))?
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("未找到任务。")),
        )
    })?;

    if job.user_id != user.id && !user.is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError::new("您无权访问该任务。")),
        ));
    }

    let direction = TranslationDirection::from_db_value(&job.translation_direction);
    let documents = sqlx::query_as::<_, DocumentRecord>(
        "SELECT id, original_filename, status, status_detail, translated_path, error_message FROM docx_documents WHERE job_id = $1 ORDER BY created_at",
    )
    .bind(job_id)
    .fetch_all(&pool)
    .await
    .map_err(|err| internal_error(err.into()))?;

    let docs = documents
        .into_iter()
        .map(|doc| JobDocumentStatus {
            id: doc.id,
            original_filename: doc.original_filename,
            status: doc.status,
            status_detail: doc.status_detail,
            error_message: doc.error_message,
            translated_download_url: doc.translated_path.map(|_| {
                format!(
                    "/api/translatedocx/jobs/{job_id}/documents/{}/download/translated",
                    doc.id
                )
            }),
        })
        .collect();

    let response = JobStatusResponse {
        job_id: job.id,
        status: job.status,
        status_detail: job.status_detail,
        error_message: job.error_message,
        created_at: job.created_at.to_rfc3339(),
        updated_at: job.updated_at.to_rfc3339(),
        translation_direction: direction.display_label().to_string(),
        documents: docs,
    };

    Ok(Json(response))
}

async fn download_document_output(
    State(state): State<AppState>,
    jar: CookieJar,
    AxumPath(params): AxumPath<(Uuid, Uuid, String)>,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
    let (job_id, document_id, variant) = params;
    if variant != "translated" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("Unknown download variant.")),
        ));
    }

    let user = require_user(&state, &jar)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ApiError::new("请先登录。"))))?;

    let pool = state.pool();
    let document = sqlx::query_as::<_, DocumentDownloadRecord>(
        "SELECT j.user_id, d.original_filename, d.translated_path FROM docx_documents d INNER JOIN docx_jobs j ON j.id = d.job_id WHERE d.id = $1 AND d.job_id = $2",
    )
    .bind(document_id)
    .bind(job_id)
    .fetch_optional(&pool)
    .await
    .map_err(|err| internal_error(err.into()))?
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("Document output not found.")),
        )
    })?;

    if document.user_id != user.id && !user.is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError::new("You do not have access to this output.")),
        ));
    }

    let path = document.translated_path.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("译文文件尚未生成。")),
        )
    })?;

    serve_docx_file(Path::new(&path), &document.original_filename)
        .await
        .map_err(|err| internal_error(err.into()))
}

fn spawn_job_worker(state: AppState, job_id: Uuid) {
    tokio::spawn(async move {
        if let Err(err) = process_job(state.clone(), job_id).await {
            error!(?err, %job_id, "docx translator job failed");
            let pool = state.pool();
            if let Err(update_err) = sqlx::query(
                "UPDATE docx_jobs SET status = $2, status_detail = $3, error_message = $4, updated_at = NOW() WHERE id = $1",
            )
            .bind(job_id)
            .bind(STATUS_FAILED)
            .bind("Job failed to complete.")
            .bind(err.to_string())
            .execute(&pool)
            .await
            {
                error!(?update_err, %job_id, "failed to update job after error");
            }
        }
    });
}

async fn process_job(state: AppState, job_id: Uuid) -> Result<()> {
    let pool = state.pool();
    let job = sqlx::query_as::<_, ProcessingJobRecord>(
        "SELECT user_id, status, translation_direction FROM docx_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_one(&pool)
    .await
    .context("failed to load job record")?;

    if job.status != STATUS_PENDING {
        return Ok(());
    }

    sqlx::query(
        "UPDATE docx_jobs SET status = $2, status_detail = $3, updated_at = NOW() WHERE id = $1",
    )
    .bind(job_id)
    .bind(STATUS_PROCESSING)
    .bind("Preparing documents")
    .execute(&pool)
    .await
    .context("failed to update job status")?;

    let direction = TranslationDirection::from_db_value(&job.translation_direction);

    let documents = sqlx::query_as::<_, ProcessingDocumentRecord>(
        "SELECT id, original_filename, source_path FROM docx_documents WHERE job_id = $1 ORDER BY created_at",
    )
    .bind(job_id)
    .fetch_all(&pool)
    .await
    .context("failed to load job documents")?;

    let job_dir = PathBuf::from(STORAGE_ROOT).join(job_id.to_string());
    let settings = state
        .translate_docx_settings()
        .await
        .ok_or_else(|| anyhow!("DOCX translator settings are not configured."))?;
    let models = settings.models.clone();
    let prompts = settings.prompts.clone();

    let glossary_terms = fetch_glossary_terms(&pool).await.unwrap_or_else(|err| {
        error!(?err, "failed to load glossary terms");
        Vec::new()
    });
    let translation_prompt = build_translation_prompt(&prompts, &glossary_terms, direction);
    let llm_client = state.llm_client();

    let mut success_count = 0_i64;
    let mut translation_tokens_total = 0_i64;

    for document in documents {
        let status_detail = format!(
            "Reading {} ({})",
            document.original_filename,
            direction.display_label()
        );
        update_document_status(
            &pool,
            document.id,
            STATUS_PROCESSING,
            Some(&status_detail),
            None,
        )
        .await?;
        update_job_status(&pool, job_id, Some(&status_detail)).await?;

        let paragraphs = match tokio::task::spawn_blocking({
            let path = document.source_path.clone();
            move || extract_docx_paragraphs(Path::new(&path))
        })
        .await
        .unwrap_or_else(|err| Err(anyhow!(err)))
        {
            Ok(paragraphs) => paragraphs,
            Err(err) => {
                error!(?err, document_id = %document.id, "failed to read DOCX content");
                update_document_status(
                    &pool,
                    document.id,
                    STATUS_FAILED,
                    Some("Unable to read DOCX content."),
                    Some(&err.to_string()),
                )
                .await?;
                continue;
            }
        };

        if paragraphs.is_empty() {
            update_document_status(
                &pool,
                document.id,
                STATUS_FAILED,
                Some("No translatable content found."),
                None,
            )
            .await?;
            continue;
        }

        let chunks = plan_translation_chunks(&paragraphs);
        if chunks.is_empty() {
            update_document_status(
                &pool,
                document.id,
                STATUS_FAILED,
                Some("No translation chunks generated."),
                None,
            )
            .await?;
            continue;
        }

        let mut translated_paragraphs = paragraphs.clone();
        let mut translation_tokens_for_doc = 0_i64;
        let mut chunk_failure = false;

        for chunk in &chunks {
            update_job_status(
                &pool,
                job_id,
                Some(&format!(
                    "Translating {} ({}) chunk {}/{}",
                    document.original_filename,
                    direction.display_label(),
                    chunk.id + 1,
                    chunks.len()
                )),
            )
            .await?;

            let request = build_translation_request(
                models.translation_model.as_str(),
                translation_prompt.clone(),
                &chunk.source_text,
                direction,
            );

            let response = match llm_client.execute(request).await {
                Ok(resp) => resp,
                Err(err) => {
                    error!(?err, document_id = %document.id, "translation request failed");
                    chunk_failure = true;
                    update_document_status(
                        &pool,
                        document.id,
                        STATUS_FAILED,
                        Some("Translation request failed."),
                        Some(&err.to_string()),
                    )
                    .await?;
                    break;
                }
            };

            translation_tokens_for_doc += response.token_usage.total_tokens as i64;
            let translated = response.text.trim().to_string();
            if translated.is_empty() {
                chunk_failure = true;
                update_document_status(
                    &pool,
                    document.id,
                    STATUS_FAILED,
                    Some("Translation response was empty."),
                    None,
                )
                .await?;
                break;
            }

            if let Err(err) =
                apply_chunk_translation(&mut translated_paragraphs, chunk, &translated)
            {
                chunk_failure = true;
                update_document_status(
                    &pool,
                    document.id,
                    STATUS_FAILED,
                    Some("Translation response did not match paragraph layout."),
                    Some(&err.to_string()),
                )
                .await?;
                break;
            }
        }

        if chunk_failure {
            continue;
        }

        let translated_path = job_dir.join(format!("translated_{}.docx", success_count + 1));
        let translated_path_clone = translated_path.clone();
        tokio::task::spawn_blocking(move || {
            write_translated_docx(&translated_path_clone, &translated_paragraphs)
        })
        .await
        .unwrap_or_else(|err| Err(anyhow!(err)))
        .with_context(|| "failed to write translated DOCX")?;

        let translated_path_string = translated_path.to_string_lossy().to_string();

        sqlx::query("UPDATE docx_documents SET status = $2, status_detail = NULL, translated_path = $3, translation_tokens = $4, chunk_count = $5, updated_at = NOW() WHERE id = $1")
            .bind(document.id)
            .bind(STATUS_COMPLETED)
            .bind(&translated_path_string)
            .bind(translation_tokens_for_doc)
            .bind(chunks.len() as i32)
            .execute(&pool)
            .await
            .context("failed to update document record")?;

        success_count += 1;
        translation_tokens_total += translation_tokens_for_doc;
    }

    let status_detail = if success_count > 0 {
        Some(format!(
            "Completed {} translated document(s) ({})",
            success_count,
            direction.display_label()
        ))
    } else {
        Some("Job finished but no documents were successfully translated".to_string())
    };

    let job_status = if success_count > 0 {
        STATUS_COMPLETED
    } else {
        STATUS_FAILED
    };

    sqlx::query(
        "UPDATE docx_jobs SET status = $2, status_detail = $3, translation_tokens = $4, usage_delta = $5, updated_at = NOW() WHERE id = $1",
    )
        .bind(job_id)
        .bind(job_status)
        .bind(status_detail.as_ref())
        .bind(translation_tokens_total)
        .bind(success_count)
        .execute(&pool)
        .await
        .context("failed to finalize job record")?;

    if success_count > 0 {
        if let Err(err) = usage::record_usage(
            &pool,
            job.user_id,
            MODULE_TRANSLATE_DOCX,
            translation_tokens_total,
            success_count as i64,
        )
        .await
        {
            error!(?err, "failed to record DOCX translator usage");
        }
    }

    Ok(())
}

fn build_translation_prompt(
    prompts: &DocxTranslatorPrompts,
    terms: &[GlossaryTermRow],
    direction: TranslationDirection,
) -> String {
    let (template, glossary) = match direction {
        TranslationDirection::EnToCn => {
            let glossary = if terms.is_empty() {
                "No glossary entries configured.".to_string()
            } else {
                let mut lines = Vec::new();
                for term in terms {
                    lines.push(format!(
                        "EN: {} -> CN: {}",
                        term.source_term, term.target_term
                    ));
                }
                lines.join("\n")
            };
            (&prompts.en_to_cn, glossary)
        }
        TranslationDirection::CnToEn => {
            let glossary = if terms.is_empty() {
                "No glossary entries configured.".to_string()
            } else {
                let mut lines = Vec::new();
                for term in terms {
                    lines.push(format!(
                        "CN: {} -> EN: {}",
                        term.target_term, term.source_term
                    ));
                }
                lines.join("\n")
            };
            (&prompts.cn_to_en, glossary)
        }
    };

    template
        .replace("{{GLOSSARY}}", &glossary)
        .replace("{{PARAGRAPH_SEPARATOR}}", PARAGRAPH_SEPARATOR)
}

fn build_translation_request(
    model: &str,
    prompt: String,
    chunk: &str,
    direction: TranslationDirection,
) -> LlmRequest {
    let instruction = match direction {
        TranslationDirection::EnToCn => format!(
            "Translate the following EN paragraphs into CN while preserving the separator {}:\n\n{}",
            PARAGRAPH_SEPARATOR, chunk
        ),
        TranslationDirection::CnToEn => format!(
            "Translate the following CN paragraphs into EN while preserving the separator {}:\n\n{}",
            PARAGRAPH_SEPARATOR, chunk
        ),
    };

    LlmRequest::new(
        model.to_string(),
        vec![
            ChatMessage::new(MessageRole::System, prompt),
            ChatMessage::new(MessageRole::User, instruction),
        ],
    )
}

fn extract_docx_paragraphs(path: &Path) -> Result<Vec<String>> {
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
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    let mut in_text_node = false;
    let mut in_paragraph = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name().as_ref() {
                b"w:p" => {
                    if in_paragraph {
                        paragraphs.push(current.trim_end().to_string());
                        current.clear();
                    }
                    in_paragraph = true;
                }
                b"w:br" => current.push('\n'),
                b"w:tab" => current.push('\t'),
                b"w:t" => in_text_node = true,
                _ => {}
            },
            Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"w:p" => {
                    if in_paragraph {
                        paragraphs.push(current.trim_end().to_string());
                        current.clear();
                    }
                    in_paragraph = true;
                }
                b"w:br" => current.push('\n'),
                b"w:tab" => current.push('\t'),
                _ => {}
            },
            Ok(Event::Text(e)) => {
                if in_text_node {
                    let value = e.unescape().map_err(|err| anyhow!(err))?.into_owned();
                    current.push_str(&value);
                }
            }
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == b"w:t" {
                    in_text_node = false;
                }
                if e.name().as_ref() == b"w:p" {
                    paragraphs.push(current.trim_end().to_string());
                    current.clear();
                    in_paragraph = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(anyhow!("failed to parse DOCX XML: {}", err)),
            _ => {}
        }
        buf.clear();
    }

    if !current.is_empty() {
        paragraphs.push(current.trim_end().to_string());
    }

    Ok(paragraphs)
}

#[derive(Debug, Clone)]
struct TranslationChunk {
    id: usize,
    paragraph_indices: Vec<usize>,
    source_text: String,
}

fn plan_translation_chunks(paragraphs: &[String]) -> Vec<TranslationChunk> {
    let mut chunks = Vec::new();
    let mut current_indices = Vec::new();
    let mut current_words = 0.0;

    let mut push_chunk = |chunk_id: usize, indices: Vec<usize>, paragraphs: &[String]| {
        if indices.is_empty() {
            return;
        }
        let mut parts = Vec::new();
        for &idx in &indices {
            parts.push(paragraphs[idx].trim().to_string());
        }
        let source_text = parts.join(PARAGRAPH_SEPARATOR);
        chunks.push(TranslationChunk {
            id: chunk_id,
            paragraph_indices: indices,
            source_text,
        });
    };

    let mut chunk_id = 0usize;

    for (idx, paragraph) in paragraphs.iter().enumerate() {
        if paragraph.trim().is_empty() {
            if !current_indices.is_empty() {
                let indices = std::mem::take(&mut current_indices);
                push_chunk(chunk_id, indices, paragraphs);
                chunk_id += 1;
                current_words = 0.0;
            }
            continue;
        }

        let para_words = calculate_equivalent_words(paragraph.trim());
        let would_exceed = !current_indices.is_empty()
            && (current_indices.len() >= CHUNK_MAX_PARAGRAPHS
                || current_words + para_words > CHUNK_MAX_EQUIVALENT_WORDS);
        if would_exceed {
            let indices = std::mem::take(&mut current_indices);
            push_chunk(chunk_id, indices, paragraphs);
            chunk_id += 1;
            current_words = 0.0;
        }

        current_indices.push(idx);
        current_words += para_words;
    }

    if !current_indices.is_empty() {
        let indices = std::mem::take(&mut current_indices);
        push_chunk(chunk_id, indices, paragraphs);
    }

    chunks
}

fn calculate_equivalent_words(text: &str) -> f64 {
    let mut count = 0.0;
    let mut buffer = String::new();

    for ch in text.chars() {
        if ch.is_whitespace() {
            if !buffer.is_empty() {
                count += 1.0;
                buffer.clear();
            }
        } else if ('\u{4E00}'..='\u{9FFF}').contains(&ch) {
            if !buffer.is_empty() {
                count += 1.0;
                buffer.clear();
            }
            count += 0.7;
        } else {
            buffer.push(ch);
        }
    }

    if !buffer.is_empty() {
        count += 1.0;
    }

    count
}

fn apply_chunk_translation(
    paragraphs: &mut [String],
    chunk: &TranslationChunk,
    translated: &str,
) -> Result<()> {
    let parts: Vec<&str> = translated
        .split(PARAGRAPH_SEPARATOR)
        .map(|s| s.trim())
        .collect();
    if parts.len() != chunk.paragraph_indices.len() {
        return Err(anyhow!(
            "translation returned {} segments but {} were expected",
            parts.len(),
            chunk.paragraph_indices.len()
        ));
    }

    for (idx, &paragraph_index) in chunk.paragraph_indices.iter().enumerate() {
        paragraphs[paragraph_index] = parts[idx].to_string();
    }

    Ok(())
}

fn write_translated_docx(path: &Path, paragraphs: &[String]) -> Result<()> {
    let mut docx = Docx::new();
    for paragraph_text in paragraphs {
        let mut paragraph = Paragraph::new();
        if paragraph_text.is_empty() {
            paragraph = paragraph.add_run(Run::new());
        } else {
            let mut first = true;
            for segment in paragraph_text.split('\n') {
                if !first {
                    paragraph = paragraph.add_run(Run::new().add_break(BreakType::TextWrapping));
                }
                paragraph = paragraph.add_run(Run::new().add_text(segment));
                first = false;
            }
        }
        docx = docx.add_paragraph(paragraph);
    }

    let file = fs::File::create(path)
        .with_context(|| format!("failed to create DOCX at {}", path.display()))?;
    docx.build()
        .pack(file)
        .with_context(|| format!("failed to pack DOCX to {}", path.display()))?;
    Ok(())
}

fn sanitize_for_docx(original_name: &str) -> String {
    let stem = Path::new(original_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    let safe_stem = sanitize(stem);
    format!("{}_translated.docx", safe_stem)
}

async fn serve_docx_file(path: &Path, original_name: &str) -> Result<Response> {
    let bytes = tokio_fs::read(path)
        .await
        .with_context(|| format!("failed to read file at {}", path.display()))?;

    let filename = sanitize_for_docx(original_name);

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_str(&format!("attachment; filename=\"{}\"", filename))
            .unwrap_or_else(|_| header::HeaderValue::from_static("attachment")),
    );

    Ok((headers, bytes).into_response())
}

async fn update_document_status(
    pool: &sqlx::PgPool,
    document_id: Uuid,
    status: &str,
    detail: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE docx_documents SET status = $2, status_detail = $3, error_message = $4, updated_at = NOW() WHERE id = $1")
        .bind(document_id)
        .bind(status)
        .bind(detail)
        .bind(error)
        .execute(pool)
        .await
        .context("failed to update document status")?;
    Ok(())
}

async fn update_job_status(pool: &sqlx::PgPool, job_id: Uuid, detail: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE docx_jobs SET status_detail = $2, updated_at = NOW() WHERE id = $1")
        .bind(job_id)
        .bind(detail)
        .execute(pool)
        .await
        .context("failed to update job detail")?;
    Ok(())
}

async fn ensure_storage_root() -> Result<()> {
    tokio_fs::create_dir_all(STORAGE_ROOT)
        .await
        .with_context(|| format!("failed to ensure storage root at {}", STORAGE_ROOT))
}

#[derive(Debug)]
struct UploadedFile {
    stored_path: PathBuf,
    original_name: String,
}

#[derive(Serialize)]
struct JobSubmissionResponse {
    job_id: Uuid,
    status_url: String,
}

#[derive(Serialize)]
struct ApiError {
    message: String,
}

impl ApiError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(sqlx::FromRow)]
struct JobRecord {
    id: Uuid,
    user_id: Uuid,
    status: String,
    status_detail: Option<String>,
    error_message: Option<String>,
    translation_direction: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct DocumentRecord {
    id: Uuid,
    original_filename: String,
    status: String,
    status_detail: Option<String>,
    translated_path: Option<String>,
    error_message: Option<String>,
}

#[derive(Serialize)]
struct JobStatusResponse {
    job_id: Uuid,
    status: String,
    status_detail: Option<String>,
    error_message: Option<String>,
    created_at: String,
    updated_at: String,
    translation_direction: String,
    documents: Vec<JobDocumentStatus>,
}

#[derive(Serialize)]
struct JobDocumentStatus {
    id: Uuid,
    original_filename: String,
    status: String,
    status_detail: Option<String>,
    error_message: Option<String>,
    translated_download_url: Option<String>,
}

#[derive(sqlx::FromRow)]
struct DocumentDownloadRecord {
    user_id: Uuid,
    original_filename: String,
    translated_path: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ProcessingJobRecord {
    user_id: Uuid,
    status: String,
    translation_direction: String,
}

#[derive(sqlx::FromRow)]
struct ProcessingDocumentRecord {
    id: Uuid,
    original_filename: String,
    source_path: String,
}

#[derive(sqlx::FromRow)]
struct SessionUser {
    id: Uuid,
    username: String,
    is_admin: bool,
}

async fn require_user(state: &AppState, jar: &CookieJar) -> Result<SessionUser, Redirect> {
    let token_cookie = jar
        .get(crate::SESSION_COOKIE)
        .ok_or_else(|| Redirect::to("/login"))?;

    let token = Uuid::parse_str(token_cookie.value()).map_err(|_| Redirect::to("/login"))?;
    let pool = state.pool();

    let user = sqlx::query_as::<_, SessionUser>(
        "SELECT users.id, users.username, users.is_admin FROM sessions INNER JOIN users ON users.id = sessions.user_id WHERE sessions.id = $1 AND sessions.expires_at > NOW()",
    )
    .bind(token)
    .fetch_optional(&pool)
    .await
    .map_err(|err| {
        error!(?err, "failed to load session for docx translator");
        Redirect::to("/login")
    })?
    .ok_or_else(|| Redirect::to("/login"))?;

    Ok(user)
}

fn internal_error(err: anyhow::Error) -> (StatusCode, Json<ApiError>) {
    error!(?err, "internal error in docx translator module");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError::new("服务器内部错误。")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glossary_prompt_includes_terms() {
        let prompts = DocxTranslatorPrompts {
            en_to_cn: "Use glossary:\n{{GLOSSARY}}\nKeep marker {{PARAGRAPH_SEPARATOR}}"
                .to_string(),
            cn_to_en: "Use glossary:\n{{GLOSSARY}}\nKeep marker {{PARAGRAPH_SEPARATOR}}"
                .to_string(),
        };
        let terms = vec![GlossaryTermRow {
            id: Uuid::new_v4(),
            source_term: "neuron".to_string(),
            target_term: "神经元".to_string(),
            notes: None,
        }];

        let prompt_en = build_translation_prompt(&prompts, &terms, TranslationDirection::EnToCn);
        assert!(prompt_en.contains("EN: neuron -> CN: 神经元"));
        assert!(prompt_en.contains(PARAGRAPH_SEPARATOR));

        let prompt_cn = build_translation_prompt(&prompts, &terms, TranslationDirection::CnToEn);
        assert!(prompt_cn.contains("CN: 神经元 -> EN: neuron"));
        assert!(prompt_cn.contains(PARAGRAPH_SEPARATOR));
    }

    #[test]
    fn plan_chunks_splits_long_documents() {
        let paragraphs = vec!["Paragraph".repeat(10); 30];
        let chunks = plan_translation_chunks(&paragraphs);
        assert!(!chunks.is_empty());
        assert!(
            chunks
                .iter()
                .all(|chunk| !chunk.paragraph_indices.is_empty())
        );
    }

    #[test]
    fn apply_chunk_translation_matches_segments() {
        let mut paragraphs = vec!["A".to_string(), "B".to_string()];
        let chunk = TranslationChunk {
            id: 0,
            paragraph_indices: vec![0, 1],
            source_text: "A".to_string(),
        };
        let result =
            apply_chunk_translation(&mut paragraphs, &chunk, "一[[__PARAGRAPH_BREAK__]]二");
        assert!(result.is_ok());
        assert_eq!(paragraphs[0], "一");
        assert_eq!(paragraphs[1], "二");
    }
}
