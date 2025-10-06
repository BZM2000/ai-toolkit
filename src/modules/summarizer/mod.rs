use std::{
    fs,
    io::{Read, Write},
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
use pdf_extract::extract_text as extract_pdf_text;
use quick_xml::{Reader as XmlReader, events::Event};
use sanitize_filename::sanitize;
use serde::Serialize;
use tokio::{fs as tokio_fs, io::AsyncWriteExt};
use tracing::error;
use uuid::Uuid;
use zip::ZipArchive;

use crate::{
    AppState, GlossaryTermRow,
    config::SummarizerPrompts,
    escape_html, fetch_glossary_terms,
    llm::{ChatMessage, LlmRequest, MessageRole},
    render_footer,
};

const STORAGE_ROOT: &str = "storage/summarizer";
const STATUS_PENDING: &str = "pending";
const STATUS_PROCESSING: &str = "processing";
const STATUS_COMPLETED: &str = "completed";
const STATUS_FAILED: &str = "failed";

const GLOSSARY_PLACEHOLDER: &str = "{{GLOSSARY}}";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tools/summarizer", get(summarizer_page))
        .route("/tools/summarizer/jobs", post(create_job))
        .route("/api/summarizer/jobs/:id", get(job_status))
        .route(
            "/api/summarizer/jobs/:id/documents/:doc_id/download/:variant",
            get(download_document_output),
        )
        .route(
            "/api/summarizer/jobs/:id/combined/:variant",
            get(download_combined_output),
        )
}

async fn summarizer_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Html<String>, Redirect> {
    let user = require_user(&state, &jar).await?;

    let footer = render_footer();
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>文档摘要与翻译 | 张圆教授课题组 AI 工具箱</title>
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
        input[type="file"], select {{ width: 100%; padding: 0.75rem; border-radius: 8px; border: 1px solid #cbd5f5; background: #f8fafc; color: #0f172a; box-sizing: border-box; }}
        input[type="file"]:focus, select:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        input[type="checkbox"] {{ margin-right: 0.5rem; }}
        button {{ padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        button:disabled {{ opacity: 0.6; cursor: not-allowed; }}
        table {{ width: 100%; border-collapse: collapse; margin-top: 1.5rem; background: #ffffff; border: 1px solid #e2e8f0; border-radius: 12px; overflow: hidden; }}
        th, td {{ padding: 0.75rem 1rem; border-bottom: 1px solid #e2e8f0; text-align: left; }}
        th {{ background: #f1f5f9; color: #0f172a; font-weight: 600; }}
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
            <h1>文档摘要与翻译</h1>
            <a class="back-link" href="/">← 返回首页</a>
        </div>
        <p class="note">当前登录：<strong>{username}</strong>。上传 PDF、DOCX 或 TXT 文件生成结构化摘要，并可输出中文译文。</p>
    </header>
    <main>
        <section class="panel">
            <h2>发起新任务</h2>
            <form id="summarizer-form">
                <label for="files">上传文件</label>
                <div id="drop-area" class="drop-zone">
                    <p><strong>拖拽文件</strong>到此处，或<span class="browse-link">点击选择</span>文件。</p>
                    <p class="note">每个任务最多可提交 10 个文件。</p>
                    <input id="files" name="files" type="file" multiple accept=".pdf,.docx,.txt" required>
                </div>
                <label for="document-type">文档类型</label>
                <select id="document-type" name="document_type">
                    <option value="research">科研论文</option>
                    <option value="other">其他文档</option>
                </select>
                <label><input type="checkbox" name="translate" id="translate" checked> 生成中文译文</label>
                <button type="submit">开始处理</button>
            </form>
            <div id="submission-status" class="status"></div>
        </section>
        <section class="panel jobs-list">
            <h2>任务进度</h2>
            <div id="job-status"></div>
        </section>
        {footer}
    </main>
    <script>
        const form = document.getElementById('summarizer-form');
        const statusBox = document.getElementById('submission-status');
        const jobStatus = document.getElementById('job-status');
        const dropArea = document.getElementById('drop-area');
        const fileInput = document.getElementById('files');
        let activeJobId = null;
        let statusTimer = null;

        const updateSelectionStatus = () => {{
            if (fileInput.files.length > 0) {{
                statusBox.textContent = `已选择 ${{fileInput.files.length}} 个文件。`;
            }}
        }};

        const handleFiles = (list) => {{
            if (!list || !list.length) {{
                return;
            }}

            const dt = new DataTransfer();
            for (const file of list) {{
                dt.items.add(file);
            }}
            fileInput.files = dt.files;
            updateSelectionStatus();
        }};

        fileInput.addEventListener('change', () => {{
            updateSelectionStatus();
        }});

        dropArea.addEventListener('click', () => {{
            fileInput.click();
        }});

        dropArea.addEventListener('dragenter', (event) => {{
            event.preventDefault();
            dropArea.classList.add('dragover');
        }});

        dropArea.addEventListener('dragover', (event) => {{
            event.preventDefault();
        }});

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
            statusBox.textContent = '正在上传文件...';
            const data = new FormData(form);

            try {{
                const response = await fetch('/tools/summarizer/jobs', {{
                    method: 'POST',
                    body: data,
                }});

                if (!response.ok) {{
                    const payload = await response.json().catch(() => ({{ message: '任务提交失败。' }}));
                    statusBox.textContent = payload.message || '任务提交失败。';
                    return;
                }}

                const payload = await response.json();
                activeJobId = payload.job_id;
                statusBox.textContent = '任务已入队，正在监控进度...';
                pollStatus();
            }} catch (err) {{
                console.error(err);
                statusBox.textContent = '提交任务时发生异常。';
            }}
        }});

        function pollStatus() {{
            if (!activeJobId) return;

            clearTimeout(statusTimer);
            fetch(`/api/summarizer/jobs/${{activeJobId}}`).then(async (response) => {{
                if (!response.ok) {{
                    jobStatus.innerHTML = '<p class="note">无法加载任务状态，请刷新页面。</p>';
                    return;
                }}

                const payload = await response.json();
                renderStatus(payload);

                if (payload.status === '{completed}' || payload.status === '{failed}') {{
                    activeJobId = null;
                    return;
                }}

                statusTimer = setTimeout(pollStatus, 4000);
            }}).catch((err) => {{
                console.error(err);
                jobStatus.innerHTML = '<p class="note">无法加载任务状态，请刷新页面。</p>';
            }});
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
            let docRows = payload.documents.map((doc) => {{
                const summaryLink = doc.summary_download_url ? `<a href="${{doc.summary_download_url}}">摘要</a>` : '';
                const translationLink = doc.translation_download_url ? `<a href="${{doc.translation_download_url}}">译文</a>` : '';
                const downloads = summaryLink || translationLink ? `<div class="downloads">${{summaryLink}}${{translationLink}}</div>` : '';
                const detail = doc.status_detail ? `<div class="note">${{doc.status_detail}}</div>` : '';
                const error = doc.error_message ? `<div class="note">${{doc.error_message}}</div>` : '';
                const statusLabel = translateStatus(doc.status);
                return `<tr><td>${{doc.original_filename}}</td><td>${{statusLabel}}</td><td>${{downloads}}</td></tr>${{detail ? `<tr><td colspan=3>${{detail}}</td></tr>` : ''}}{{error ? `<tr><td colspan=3>${{error}}</td></tr>` : ''}}`;
            }}).join('');
            if (!docRows) {{
                docRows = '<tr><td colspan="3">暂无文件记录。</td></tr>';
            }}

            const combinedSummary = payload.combined_summary_url ? `<a href="${{payload.combined_summary_url}}">下载汇总摘要</a>` : '';
            const combinedTranslation = payload.combined_translation_url ? `<a href="${{payload.combined_translation_url}}">下载汇总译文</a>` : '';
            const combinedBlock = combinedSummary || combinedTranslation ? `<p class="downloads">${{combinedSummary}} ${{combinedTranslation}}</p>` : '';
            const errorBlock = payload.error_message ? `<p class="note">${{payload.error_message}}</p>` : '';
            const detailBlock = payload.status_detail ? `<p class="note">${{payload.status_detail}}</p>` : '';
            const jobStatusLabel = translateStatus(payload.status);

            jobStatus.innerHTML = `
                <div class="status">
                    <p><strong>任务状态：</strong> ${{jobStatusLabel}}</p>
                    ${{detailBlock}}
                    ${{errorBlock}}
                    <table>
                        <thead><tr><th>文件名</th><th>状态</th><th>下载</th></tr></thead>
                        <tbody>${{docRows}}</tbody>
                    </table>
                    ${{combinedBlock}}
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

    let mut document_type = DocumentKind::ResearchArticle;
    let mut translate = true;
    let mut files: Vec<UploadedFile> = Vec::new();
    let mut file_index = 0;

    ensure_storage_root()
        .await
        .map_err(|err| internal_error(err.into()))?;
    let job_id = Uuid::new_v4();
    let job_dir = PathBuf::from(STORAGE_ROOT).join(job_id.to_string());
    tokio_fs::create_dir_all(&job_dir)
        .await
        .map_err(|err| internal_error(err.into()))?;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| internal_error(err.into()))?
    {
        let name = field.name().map(|n| n.to_string());

        match name.as_deref() {
            Some("document_type") => {
                let value = field
                    .text()
                    .await
                    .map_err(|err| internal_error(err.into()))?;
                document_type = DocumentKind::from_str(value.trim());
            }
            Some("translate") => {
                let value = field
                    .text()
                    .await
                    .map_err(|err| internal_error(err.into()))?;
                translate = matches!(value.trim(), "on" | "true" | "1" | "yes");
            }
            Some("files") => {
                let Some(filename) = field.file_name().map(|s| s.to_string()) else {
                    continue;
                };
                let safe_name = sanitize(&filename);
                let ext = Path::new(&safe_name)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !matches!(ext.as_str(), "pdf" | "docx" | "txt") {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(ApiError::new("仅支持上传 PDF、DOCX 和 TXT 文件。")),
                    ));
                }
                let stored_path = job_dir.join(format!("source_{file_index}_{safe_name}"));
                file_index += 1;

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

                files.push(UploadedFile {
                    stored_path,
                    original_name: filename,
                });
            }
            _ => {}
        }
    }

    if files.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("请至少上传一个文件。")),
        ));
    }

    if files.len() > 10 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("每个任务最多上传 10 个文件。")),
        ));
    }

    if let Some(limit) = user.usage_limit {
        let projected = user.usage_count + files.len() as i64;
        if projected > limit {
            let _ = tokio_fs::remove_dir_all(&job_dir).await;
            return Err((
                StatusCode::FORBIDDEN,
                Json(ApiError::new("该账户已达到使用上限。")),
            ));
        }
    }

    let mut transaction = state
        .pool()
        .begin()
        .await
        .map_err(|err| internal_error(err.into()))?;

    sqlx::query(
        "INSERT INTO summary_jobs (id, user_id, status, document_type, translate) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(job_id)
    .bind(user.id)
    .bind(STATUS_PENDING)
    .bind(document_type.as_str())
    .bind(translate)
    .execute(&mut *transaction)
    .await
    .map_err(|err| internal_error(err.into()))?;

    for (ordinal, file) in files.iter().enumerate() {
        sqlx::query("INSERT INTO summary_documents (id, job_id, ordinal, original_filename, source_path, status) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(Uuid::new_v4())
            .bind(job_id)
            .bind(ordinal as i32)
            .bind(&file.original_name)
            .bind(file.stored_path.to_string_lossy().to_string())
            .bind(STATUS_PENDING)
            .execute(&mut *transaction)
            .await
            .map_err(|err| internal_error(err.into()))?;
    }

    transaction
        .commit()
        .await
        .map_err(|err| internal_error(err.into()))?;

    spawn_job_worker(state.clone(), job_id);

    Ok(Json(JobSubmissionResponse {
        job_id,
        status_url: format!("/api/summarizer/jobs/{}", job_id),
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
        "SELECT id, user_id, status, status_detail, error_message, combined_summary_path, combined_translation_path, created_at, updated_at FROM summary_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_optional(&pool)
    .await
    .map_err(|err| internal_error(err.into()))?
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("未找到任务或任务已失效。")),
        )
    })?;

    if job.user_id != user.id && !user.is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError::new("您无权访问该任务。")),
        ));
    }

    let documents = sqlx::query_as::<_, DocumentRecord>(
        "SELECT id, original_filename, status, status_detail, summary_path, translation_path, error_message FROM summary_documents WHERE job_id = $1 ORDER BY ordinal",
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
            summary_download_url: doc.summary_path.map(|_| {
                format!(
                    "/api/summarizer/jobs/{}/documents/{}/download/summary",
                    job_id, doc.id
                )
            }),
            translation_download_url: doc.translation_path.map(|_| {
                format!(
                    "/api/summarizer/jobs/{}/documents/{}/download/translation",
                    job_id, doc.id
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
        combined_summary_url: job
            .combined_summary_path
            .map(|_| format!("/api/summarizer/jobs/{}/combined/summary", job.id)),
        combined_translation_url: job
            .combined_translation_path
            .map(|_| format!("/api/summarizer/jobs/{}/combined/translation", job.id)),
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
    let user = require_user(&state, &jar)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ApiError::new("请先登录。"))))?;

    let pool = state.pool();

    let document = sqlx::query_as::<_, DocumentDownloadRecord>(
        "SELECT j.user_id, d.original_filename, d.summary_path, d.translation_path FROM summary_documents d INNER JOIN summary_jobs j ON j.id = d.job_id WHERE d.id = $1 AND d.job_id = $2",
    )
    .bind(document_id)
    .bind(job_id)
    .fetch_optional(&pool)
    .await
    .map_err(|err| internal_error(err.into()))?
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("未找到对应的输出文件。")),
        )
    })?;

    if document.user_id != user.id && !user.is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError::new("您无权下载该输出。")),
        ));
    }

    let (path, suffix) = match variant.as_str() {
        "summary" => document
            .summary_path
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(ApiError::new("摘要文件尚未生成。")),
                )
            })
            .map(|path| (path, "summary"))?,
        "translation" => document
            .translation_path
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(ApiError::new("译文文件尚未生成。")),
                )
            })
            .map(|path| (path, "translation"))?,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiError::new("未知的下载类型。")),
            ));
        }
    };

    serve_file(Path::new(&path), &document.original_filename, suffix)
        .await
        .map_err(|err| internal_error(err.into()))
}

async fn download_combined_output(
    State(state): State<AppState>,
    jar: CookieJar,
    AxumPath((job_id, variant)): AxumPath<(Uuid, String)>,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
    let user = require_user(&state, &jar)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, Json(ApiError::new("请先登录。"))))?;

    let pool = state.pool();

    let job = sqlx::query_as::<_, CombinedJobRecord>(
        "SELECT user_id, combined_summary_path, combined_translation_path FROM summary_jobs WHERE id = $1",
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

    let (path, suffix) = match variant.as_str() {
        "summary" => job
            .combined_summary_path
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(ApiError::new("汇总摘要尚不可用。")),
                )
            })
            .map(|path| (path, "combined-summary"))?,
        "translation" => job
            .combined_translation_path
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(ApiError::new("汇总译文尚不可用。")),
                )
            })
            .map(|path| (path, "combined-translation"))?,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiError::new("未知的下载类型。")),
            ));
        }
    };

    serve_file(Path::new(&path), "combined.txt", suffix)
        .await
        .map_err(|err| internal_error(err.into()))
}

fn build_translation_prompt(prompts: &SummarizerPrompts, glossary: &[GlossaryTermRow]) -> String {
    let glossary_block = glossary
        .iter()
        .map(|term| {
            format!(
                "- EN: {} -> CN: {}",
                term.source_term.trim(),
                term.target_term.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let substitution = if glossary_block.is_empty() {
        "- (no glossary terms configured)".to_string()
    } else {
        glossary_block
    };

    if prompts.translation.contains(GLOSSARY_PLACEHOLDER) {
        prompts
            .translation
            .replace(GLOSSARY_PLACEHOLDER, &substitution)
    } else {
        format!("{}\n{}", prompts.translation.trim_end(), substitution)
    }
}

fn document_prompt<'a>(prompts: &'a SummarizerPrompts, kind: DocumentKind) -> &'a str {
    match kind {
        DocumentKind::ResearchArticle => prompts.research_summary.as_str(),
        DocumentKind::OtherDocument => prompts.general_summary.as_str(),
    }
}

fn translation_enabled_text(enabled: bool) -> &'static str {
    if enabled { "enabled" } else { "disabled" }
}

fn sanitize_for_output(filename: &str, suffix: &str) -> String {
    let mut base = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document")
        .to_string();
    if base.is_empty() {
        base = "document".to_string();
    }
    let safe_base = sanitize(base);
    format!("{}_{}.txt", safe_base, suffix)
}

async fn serve_file(path: &Path, original_name: &str, suffix: &str) -> Result<Response> {
    let bytes = tokio_fs::read(path)
        .await
        .with_context(|| format!("failed to read file at {}", path.display()))?;

    let filename = sanitize_for_output(original_name, suffix);

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_str(&format!("attachment; filename=\"{}\"", filename))
            .unwrap_or_else(|_| header::HeaderValue::from_static("attachment")),
    );

    Ok((headers, bytes).into_response())
}

fn build_summary_request(model: &str, prompt: &str, text: &str) -> LlmRequest {
    LlmRequest::new(
        model.to_string(),
        vec![
            ChatMessage::new(MessageRole::System, prompt),
            ChatMessage::new(MessageRole::User, text.to_string()),
        ],
    )
}

fn build_translation_request(model: &str, prompt: String, summary: &str) -> LlmRequest {
    LlmRequest::new(
        model.to_string(),
        vec![
            ChatMessage::new(MessageRole::System, prompt),
            ChatMessage::new(
                MessageRole::User,
                format!(
                    "Translate the following text to Chinese while adhering to the glossary:\n\n{}",
                    summary
                ),
            ),
        ],
    )
}

async fn ensure_storage_root() -> Result<()> {
    tokio_fs::create_dir_all(STORAGE_ROOT)
        .await
        .with_context(|| format!("failed to ensure storage root at {}", STORAGE_ROOT))
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

fn read_document_text(path: &Path) -> Result<String> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();

    match extension.as_str() {
        "pdf" => extract_pdf_text(path)
            .with_context(|| format!("failed to extract PDF text from {}", path.display())),
        "docx" => extract_docx_text(path),
        "txt" => fs::read_to_string(path)
            .with_context(|| format!("failed to read text file {}", path.display())),
        other => Err(anyhow!("Unsupported file type: {}", other)),
    }
    .map(|content| content.trim().to_string())
}

fn combined_output_path(job_dir: &Path, variant: &str) -> PathBuf {
    job_dir.join(format!("combined_{}.txt", variant))
}

fn append_to_file(path: &Path, heading: &str, body: &str) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    writeln!(file, "# {}\n\n{}\n\n", heading, body)
        .with_context(|| format!("failed to write to {}", path.display()))?;
    Ok(())
}

fn create_text_file(path: &Path, content: &str, heading: &str) -> Result<()> {
    let mut file =
        fs::File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    writeln!(file, "# {}\n\n{}", heading, content)
        .with_context(|| format!("failed to write summary file {}", path.display()))?;
    Ok(())
}

fn format_heading(idx: usize, filename: &str) -> String {
    format!("Document {} — {}", idx + 1, filename)
}

async fn process_job(state: AppState, job_id: Uuid) -> Result<()> {
    let pool = state.pool();
    let job = sqlx::query_as::<_, ProcessingJobRecord>(
        "SELECT user_id, status, document_type, translate FROM summary_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_one(&pool)
    .await
    .context("failed to load job record")?;

    if job.status != STATUS_PENDING {
        return Ok(());
    }

    let document_kind = DocumentKind::from_str(&job.document_type);

    sqlx::query(
        "UPDATE summary_jobs SET status = $2, status_detail = $3, updated_at = NOW() WHERE id = $1",
    )
    .bind(job_id)
    .bind(STATUS_PROCESSING)
    .bind("Preparing documents")
    .execute(&pool)
    .await
    .context("failed to update job status")?;

    let documents = sqlx::query_as::<_, ProcessingDocumentRecord>(
        "SELECT id, original_filename, source_path FROM summary_documents WHERE job_id = $1 ORDER BY ordinal",
    )
    .bind(job_id)
    .fetch_all(&pool)
    .await
    .context("failed to load job documents")?;

    let job_dir = PathBuf::from(STORAGE_ROOT).join(job_id.to_string());
    let models = state
        .models_config()
        .summarizer()
        .cloned()
        .ok_or_else(|| anyhow!("Summarizer models are not configured."))?;
    let prompts = state
        .prompts_config()
        .summarizer()
        .cloned()
        .ok_or_else(|| anyhow!("Summarizer prompts are not configured."))?;

    let glossary_terms = fetch_glossary_terms(&pool).await.unwrap_or_else(|err| {
        error!(?err, "failed to load glossary terms");
        Vec::new()
    });
    let translation_prompt = build_translation_prompt(&prompts, &glossary_terms);

    let llm_client = state.llm_client();
    let mut combined_summary_path: Option<String> = None;
    let mut combined_translation_path: Option<String> = None;
    let mut success_count = 0_i64;
    let mut summary_tokens_total = 0_i64;
    let mut translation_tokens_total = 0_i64;

    for (idx, document) in documents.iter().enumerate() {
        let heading = format_heading(idx, &document.original_filename);
        let status_detail = format!("Reading {}", document.original_filename);
        update_document_status(
            &pool,
            document.id,
            STATUS_PROCESSING,
            Some(&status_detail),
            None,
        )
        .await?;
        update_job_status(&pool, job_id, Some(&status_detail)).await?;

        let text = match tokio::task::spawn_blocking({
            let path = document.source_path.clone();
            move || read_document_text(Path::new(&path))
        })
        .await
        .unwrap_or_else(|err| Err(anyhow!(err)))
        .and_then(|text| {
            if text.is_empty() {
                Err(anyhow!("No extractable text found"))
            } else {
                Ok(text)
            }
        }) {
            Ok(text) => text,
            Err(err) => {
                error!(?err, document_id = %document.id, "failed to read input document");
                update_document_status(
                    &pool,
                    document.id,
                    STATUS_FAILED,
                    Some("Unable to extract text from the document."),
                    Some(&err.to_string()),
                )
                .await?;
                continue;
            }
        };

        let summary_prompt = document_prompt(&prompts, document_kind);
        let summary_request = build_summary_request(models.summary_model(), summary_prompt, &text);
        let summary_response = match llm_client.execute(summary_request).await {
            Ok(resp) => resp,
            Err(err) => {
                error!(?err, document_id = %document.id, "summarization request failed");
                update_document_status(
                    &pool,
                    document.id,
                    STATUS_FAILED,
                    Some("Summarization failed."),
                    Some(&err.to_string()),
                )
                .await?;
                continue;
            }
        };

        let summary_text = summary_response.text.trim().to_string();
        summary_tokens_total += summary_response.token_usage.total_tokens as i64;

        let summary_file_path = job_dir.join(format!("summary_{}.txt", idx + 1));
        tokio::task::spawn_blocking({
            let path = summary_file_path.clone();
            let content = summary_text.clone();
            let heading = heading.clone();
            move || create_text_file(&path, &content, &heading)
        })
        .await
        .unwrap_or_else(|err| Err(anyhow!(err)))
        .context("failed to write summary file")?;

        if combined_summary_path.is_none() {
            combined_summary_path = Some(
                combined_output_path(&job_dir, "summary")
                    .to_string_lossy()
                    .to_string(),
            );
        }
        let combined_summary_target = PathBuf::from(combined_summary_path.as_ref().unwrap());
        tokio::task::spawn_blocking({
            let path = combined_summary_target.clone();
            let heading = heading.clone();
            let content = summary_text.clone();
            move || append_to_file(&path, &heading, &content)
        })
        .await
        .unwrap_or_else(|err| Err(anyhow!(err)))
        .context("failed to append to combined summary")?;

        let mut translation_path: Option<String> = None;
        let mut translation_text: Option<String> = None;
        let mut translation_tokens_for_doc: i64 = 0;
        let mut translation_status_detail: Option<String> = None;
        let mut translation_error_detail: Option<String> = None;

        if job.translate {
            update_job_status(
                &pool,
                job_id,
                Some(&format!(
                    "Translating {} (glossary {})",
                    document.original_filename,
                    translation_enabled_text(job.translate)
                )),
            )
            .await?;

            let translation_request = build_translation_request(
                models.translation_model(),
                translation_prompt.clone(),
                &summary_text,
            );
            match llm_client.execute(translation_request).await {
                Ok(response) => {
                    let text = response.text.trim().to_string();
                    translation_tokens_for_doc = response.token_usage.total_tokens as i64;
                    translation_tokens_total += translation_tokens_for_doc;
                    let translation_file_path =
                        job_dir.join(format!("translation_{}.txt", idx + 1));
                    tokio::task::spawn_blocking({
                        let path = translation_file_path.clone();
                        let heading = heading.clone();
                        let content = text.clone();
                        move || create_text_file(&path, &content, &heading)
                    })
                    .await
                    .unwrap_or_else(|err| Err(anyhow!(err)))
                    .context("failed to write translation file")?;

                    if combined_translation_path.is_none() {
                        combined_translation_path = Some(
                            combined_output_path(&job_dir, "translation")
                                .to_string_lossy()
                                .to_string(),
                        );
                    }
                    let combined_translation_target =
                        PathBuf::from(combined_translation_path.as_ref().unwrap());
                    tokio::task::spawn_blocking({
                        let path = combined_translation_target.clone();
                        let heading = heading.clone();
                        let content = text.clone();
                        move || append_to_file(&path, &heading, &content)
                    })
                    .await
                    .unwrap_or_else(|err| Err(anyhow!(err)))
                    .context("failed to append to combined translation")?;

                    translation_path = Some(translation_file_path.to_string_lossy().to_string());
                    translation_text = Some(text);
                }
                Err(err) => {
                    error!(?err, document_id = %document.id, "translation request failed");
                    translation_status_detail =
                        Some("Translation failed; summary available.".to_string());
                    translation_error_detail = Some(err.to_string());
                }
            }
        }

        sqlx::query("UPDATE summary_documents SET status = $2, status_detail = $3, summary_text = $4, translation_text = $5, summary_path = $6, translation_path = $7, summary_tokens = $8, translation_tokens = $9, error_message = $10, updated_at = NOW() WHERE id = $1")
            .bind(document.id)
            .bind(STATUS_COMPLETED)
            .bind(translation_status_detail.as_deref())
            .bind(&summary_text)
            .bind(translation_text.as_ref())
            .bind(summary_file_path.to_string_lossy().to_string())
            .bind(translation_path)
            .bind(summary_response.token_usage.total_tokens as i64)
            .bind(translation_tokens_for_doc)
            .bind(translation_error_detail.as_deref())
            .execute(&pool)
            .await
            .context("failed to update document record")?;

        success_count += 1;
    }

    let status_detail = if success_count > 0 {
        Some(format!(
            "Completed with {} successful documents",
            success_count
        ))
    } else {
        Some("Job finished but no documents were successfully processed".to_string())
    };

    let job_status = if success_count > 0 {
        STATUS_COMPLETED
    } else {
        STATUS_FAILED
    };

    sqlx::query("UPDATE summary_jobs SET status = $2, status_detail = $3, combined_summary_path = $4, combined_translation_path = $5, summary_tokens = $6, translation_tokens = $7, usage_delta = $8, updated_at = NOW() WHERE id = $1")
        .bind(job_id)
        .bind(job_status)
        .bind(status_detail.as_ref())
        .bind(combined_summary_path.as_ref())
        .bind(combined_translation_path.as_ref())
        .bind(summary_tokens_total)
        .bind(translation_tokens_total)
        .bind(success_count)
        .execute(&pool)
        .await
        .context("failed to finalize job record")?;

    if success_count > 0 {
        let update_usage = sqlx::query(
            "UPDATE users SET usage_count = usage_count + $2 WHERE id = $1 AND (usage_limit IS NULL OR usage_count + $2 <= usage_limit)",
        )
        .bind(job.user_id)
        .bind(success_count)
        .execute(&pool)
        .await?;

        if update_usage.rows_affected() == 0 {
            sqlx::query("UPDATE summary_jobs SET status = $2, status_detail = $3 WHERE id = $1")
                .bind(job_id)
                .bind(STATUS_FAILED)
                .bind("Usage limit reached before completion.")
                .execute(&pool)
                .await?;
        }
    }

    Ok(())
}

fn spawn_job_worker(state: AppState, job_id: Uuid) {
    tokio::spawn(async move {
        if let Err(err) = process_job(state.clone(), job_id).await {
            error!(?err, %job_id, "summarizer job failed");
            let pool = state.pool();
            if let Err(update_err) = sqlx::query(
                "UPDATE summary_jobs SET status = $2, status_detail = $3, error_message = $4, updated_at = NOW() WHERE id = $1",
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

async fn update_document_status(
    pool: &sqlx::PgPool,
    document_id: Uuid,
    status: &str,
    detail: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE summary_documents SET status = $2, status_detail = $3, error_message = $4, updated_at = NOW() WHERE id = $1")
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
    sqlx::query("UPDATE summary_jobs SET status_detail = $2, updated_at = NOW() WHERE id = $1")
        .bind(job_id)
        .bind(detail)
        .execute(pool)
        .await
        .context("failed to update job detail")?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum DocumentKind {
    ResearchArticle,
    OtherDocument,
}

impl DocumentKind {
    fn from_str(value: &str) -> Self {
        match value.to_lowercase().as_str() {
            "other" => DocumentKind::OtherDocument,
            _ => DocumentKind::ResearchArticle,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            DocumentKind::ResearchArticle => "research",
            DocumentKind::OtherDocument => "other",
        }
    }
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
    combined_summary_path: Option<String>,
    combined_translation_path: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct DocumentRecord {
    id: Uuid,
    original_filename: String,
    status: String,
    status_detail: Option<String>,
    summary_path: Option<String>,
    translation_path: Option<String>,
    error_message: Option<String>,
}

#[derive(sqlx::FromRow)]
struct DocumentDownloadRecord {
    user_id: Uuid,
    original_filename: String,
    summary_path: Option<String>,
    translation_path: Option<String>,
}

#[derive(sqlx::FromRow)]
struct CombinedJobRecord {
    user_id: Uuid,
    combined_summary_path: Option<String>,
    combined_translation_path: Option<String>,
}

#[derive(Serialize)]
struct JobStatusResponse {
    job_id: Uuid,
    status: String,
    status_detail: Option<String>,
    error_message: Option<String>,
    created_at: String,
    updated_at: String,
    combined_summary_url: Option<String>,
    combined_translation_url: Option<String>,
    documents: Vec<JobDocumentStatus>,
}

#[derive(Serialize)]
struct JobDocumentStatus {
    id: Uuid,
    original_filename: String,
    status: String,
    status_detail: Option<String>,
    error_message: Option<String>,
    summary_download_url: Option<String>,
    translation_download_url: Option<String>,
}

#[derive(sqlx::FromRow, Clone)]
struct ProcessingJobRecord {
    user_id: Uuid,
    status: String,
    document_type: String,
    translate: bool,
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
    usage_count: i64,
    usage_limit: Option<i64>,
    is_admin: bool,
}

async fn require_user(state: &AppState, jar: &CookieJar) -> Result<SessionUser, Redirect> {
    let token_cookie = jar
        .get(crate::SESSION_COOKIE)
        .ok_or_else(|| Redirect::to("/login"))?;

    let token = Uuid::parse_str(token_cookie.value()).map_err(|_| Redirect::to("/login"))?;

    let pool = state.pool();

    let user = sqlx::query_as::<_, SessionUser>(
        "SELECT users.id, users.username, users.usage_count, users.usage_limit, users.is_admin FROM sessions INNER JOIN users ON users.id = sessions.user_id WHERE sessions.id = $1 AND sessions.expires_at > NOW()",
    )
    .bind(token)
    .fetch_optional(&pool)
    .await
    .map_err(|err| {
        error!(?err, "failed to load session for summarizer");
        Redirect::to("/login")
    })?
    .ok_or_else(|| Redirect::to("/login"))?;

    Ok(user)
}

fn internal_error(err: anyhow::Error) -> (StatusCode, Json<ApiError>) {
    error!(?err, "internal error in summarizer module");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError::new("服务器内部错误。")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    #[test]
    fn generates_translation_prompt_with_terms() {
        let terms = vec![GlossaryTermRow {
            id: Uuid::new_v4(),
            source_term: "neuron".to_string(),
            target_term: "神经元".to_string(),
            notes: None,
        }];

        let prompts = SummarizerPrompts {
            research_summary: String::from("summary"),
            general_summary: String::from("general"),
            translation: String::from("Use glossary terms:\n{{GLOSSARY}}\nPreserve citations."),
        };

        let prompt = build_translation_prompt(&prompts, &terms);

        assert!(prompt.contains("EN: neuron"));
        assert!(prompt.contains("CN: 神经元"));
        assert!(prompt.contains("Use glossary terms"));
    }

    #[test]
    fn extract_docx_text_returns_plain_text() {
        let dir = tempdir().expect("temp dir");
        let docx_path = dir.path().join("sample.docx");
        let file = fs::File::create(&docx_path).expect("create docx");
        let mut zip = zip::ZipWriter::new(file);

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Hello</w:t></w:r></w:p>
    <w:p><w:r><w:t>World</w:t></w:r></w:p>
  </w:body>
</w:document>"#;

        zip.start_file("word/document.xml", SimpleFileOptions::default())
            .expect("zip start file");
        zip.write_all(xml.as_bytes()).expect("write xml");
        zip.finish().expect("finish zip");

        let extracted = extract_docx_text(&docx_path).expect("extract docx");
        assert_eq!(extracted, "Hello\n\nWorld");
    }
}
