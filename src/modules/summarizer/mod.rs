use std::{
    borrow::Cow,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
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
use tokio::{fs as tokio_fs, sync::Semaphore, time::sleep};
use tracing::{error, warn};
use uuid::Uuid;
use zip::ZipArchive;

mod admin;

use crate::web::history_ui;
use crate::web::{
    FileFieldConfig, FileNaming, ToolAdminLink, ToolPageLayout, UPLOAD_WIDGET_SCRIPT,
    UPLOAD_WIDGET_STYLES, UploadWidgetConfig, process_upload_form, render_tool_page,
    render_upload_widget,
};
use crate::{
    AppState, GlossaryTermRow,
    config::SummarizerPrompts,
    escape_html, fetch_glossary_terms, history,
    llm::{ChatMessage, LlmRequest, MessageRole},
    render_footer,
    usage::{self, MODULE_SUMMARIZER},
    web::{
        ApiMessage, JobSubmission,
        auth::{self, JsonAuthError},
        json_error,
    },
};

const STORAGE_ROOT: &str = "storage/summarizer";
const STATUS_PENDING: &str = "pending";
const STATUS_PROCESSING: &str = "processing";
const STATUS_COMPLETED: &str = "completed";
const STATUS_FAILED: &str = "failed";

const GLOSSARY_PLACEHOLDER: &str = "{{GLOSSARY}}";
const MAX_RETRIES: u32 = 3;
const INITIAL_RETRY_DELAY_MS: u64 = 1000;
const MAX_CONCURRENT_DOCUMENTS: usize = 5;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tools/summarizer", get(summarizer_page))
        .route("/tools/summarizer/jobs", post(create_job))
        .route("/api/summarizer/jobs/:id", get(job_status))
        .route(
            "/api/summarizer/jobs/:id/combined/:variant",
            get(download_combined_output),
        )
        .route("/dashboard/modules/summarizer", get(admin::settings_page))
        .route(
            "/dashboard/modules/summarizer/models",
            post(admin::save_models),
        )
        .route(
            "/dashboard/modules/summarizer/prompts",
            post(admin::save_prompts),
        )
}

async fn summarizer_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Html<String>, Redirect> {
    let user = auth::require_user_redirect(&state, &jar).await?;

    let username = escape_html(&user.username);
    let note_html = format!(
        "当前登录：<strong>{username}</strong>。上传 PDF、DOCX 或 TXT 文件生成结构化摘要，并可输出中文译文。",
        username = username,
    );
    let admin_link = if user.is_admin {
        Some(ToolAdminLink {
            href: "/dashboard/modules/summarizer",
            label: "模块管理",
        })
    } else {
        None
    };
    let upload_widget = render_upload_widget(
        &UploadWidgetConfig::new("summarizer-upload", "files", "files", "上传文件")
            .with_description("支持上传 PDF、DOCX 或 TXT 文档。")
            .with_multiple(Some(100))
            .with_note("每个任务最多可提交 100 个文件。")
            .with_accept(".pdf,.docx,.txt"),
    );
    let history_panel = history_ui::render_history_panel(MODULE_SUMMARIZER);
    let new_tab_html = format!(
        r#"                <section class="panel">
                    <h2>发起新任务</h2>
                    <form id="summarizer-form">
                        {upload_widget}
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
"#,
        upload_widget = upload_widget,
    );

    let summarizer_script = r#"const form = document.getElementById('summarizer-form');
const statusBox = document.getElementById('submission-status');
const jobStatus = document.getElementById('job-status');
const fileInput = document.getElementById('files');
let activeJobId = null;
let statusTimer = null;

form.addEventListener('submit', async (event) => {
    event.preventDefault();

    if (!fileInput || fileInput.files.length === 0) {
        statusBox.innerHTML = '<span style="color: #dc2626;">请至少选择一个文件。</span>';
        return;
    }

    if (fileInput.files.length > 100) {
        statusBox.innerHTML = '<span style="color: #dc2626;">文件数量超过限制（最多 100 个）。</span>';
        return;
    }

    statusBox.textContent = '正在上传文件...';
    const data = new FormData(form);

    try {
        const response = await fetch('/tools/summarizer/jobs', {
            method: 'POST',
            body: data,
        });

        if (!response.ok) {
            const payload = await response.json().catch(() => ({ message: '任务提交失败。' }));
            statusBox.innerHTML = `<span style="color: #dc2626;">${payload.message || '任务提交失败。'}</span>`;
            return;
        }

        const payload = await response.json();
        activeJobId = payload.job_id;
        statusBox.innerHTML = '<span style="color: #16a34a;">任务已入队，正在监控进度...</span>';
        form.reset();
        if (fileInput) {
            fileInput.value = '';
            fileInput.dispatchEvent(new Event('change'));
        }
        pollStatus();
    } catch (err) {
        console.error(err);
        statusBox.innerHTML = '<span style="color: #dc2626;">提交任务时发生异常。</span>';
    }
});

function pollStatus() {
    if (!activeJobId) return;

    clearTimeout(statusTimer);
    fetch(`/api/summarizer/jobs/${activeJobId}`).then(async (response) => {
        if (!response.ok) {
            jobStatus.innerHTML = '<p class="note">无法加载任务状态，请刷新页面。</p>';
            return;
        }

        const payload = await response.json();
        renderStatus(payload);

        if (payload.status === 'completed' || payload.status === 'failed') {
            activeJobId = null;
            return;
        }

        statusTimer = setTimeout(pollStatus, 4000);
    }).catch((err) => {
        console.error(err);
        jobStatus.innerHTML = '<p class="note">无法加载任务状态，请刷新页面。</p>';
    });
}

function translateStatus(status) {
    const map = {
        pending: '待处理',
        processing: '处理中',
        completed: '已完成',
        failed: '已失败',
        queued: '排队中',
    };
    return map[status] || status;
}

function renderStatus(payload) {
    let docRows = payload.documents.map((doc) => {
        const detail = doc.status_detail ? `<div class="note">${doc.status_detail}</div>` : '';
        const error = doc.error_message ? `<div class="note">${doc.error_message}</div>` : '';
        const statusLabel = translateStatus(doc.status);
        return `<tr><td>${doc.original_filename}</td><td>${statusLabel}</td></tr>${detail ? `<tr><td colspan=2>${detail}</td></tr>` : ''}${error ? `<tr><td colspan=2>${error}</td></tr>` : ''}`;
    }).join('');
    if (!docRows) {
        docRows = '<tr><td colspan="2">暂无文件记录。</td></tr>';
    }

    const combinedSummary = payload.combined_summary_url ? `<a href="${payload.combined_summary_url}">下载汇总摘要</a>` : '';
    const combinedTranslation = payload.combined_translation_url ? `<a href="${payload.combined_translation_url}">下载汇总译文</a>` : '';
    const combinedBlock = combinedSummary || combinedTranslation ? `<p class="downloads">${combinedSummary} ${combinedTranslation}</p>` : '';
    const errorBlock = payload.error_message ? `<p class="note">${payload.error_message}</p>` : '';
    const detailBlock = payload.status_detail ? `<p class="note">${payload.status_detail}</p>` : '';
    const jobStatusLabel = translateStatus(payload.status);

    jobStatus.innerHTML = `
        <div class="status">
            <p><strong>任务状态：</strong> ${jobStatusLabel}</p>
            ${detailBlock}
            ${errorBlock}
            <table>
                <thead><tr><th>文件名</th><th>状态</th></tr></thead>
                <tbody>${docRows}</tbody>
            </table>
            ${combinedBlock}
        </div>
    `;
}
"#;

    let html = render_tool_page(ToolPageLayout {
        meta_title: "文档摘要与翻译 | 张圆教授课题组 AI 工具箱",
        page_heading: "文档摘要与翻译",
        username: &username,
        note_html: Cow::Owned(note_html),
        tab_group: "summarizer",
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
                summarizer_script
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
) -> Result<Json<JobSubmission>, (StatusCode, Json<ApiMessage>)> {
    let user = auth::current_user_or_json_error(&state, &jar)
        .await
        .map_err(|JsonAuthError { status, message }| json_error(status, message))?;

    let mut document_type = DocumentKind::ResearchArticle;
    let mut translate = true;

    ensure_storage_root()
        .await
        .map_err(|err| internal_error(err.into()))?;
    let job_id = Uuid::new_v4();
    let job_dir = PathBuf::from(STORAGE_ROOT).join(job_id.to_string());

    let file_config = FileFieldConfig::new(
        "files",
        &["pdf", "docx", "txt"],
        100,
        FileNaming::Indexed {
            prefix: "source_",
            pad_width: 3,
        },
    )
    .with_min_files(1);

    let upload = match process_upload_form(multipart, &job_dir, &[file_config]).await {
        Ok(outcome) => outcome,
        Err(err) => {
            let _ = tokio_fs::remove_dir_all(&job_dir).await;
            return Err(json_error(
                StatusCode::BAD_REQUEST,
                err.message().to_string(),
            ));
        }
    };

    if let Some(value) = upload.first_text("document_type") {
        document_type = DocumentKind::from_str(value.trim());
    }

    if let Some(value) = upload.first_text("translate") {
        translate = matches!(value.trim(), "on" | "true" | "1" | "yes");
    }

    let files: Vec<_> = upload.files_for("files").cloned().collect();

    let pool = state.pool();

    if let Err(err) =
        usage::ensure_within_limits(&pool, user.id, MODULE_SUMMARIZER, files.len() as i64).await
    {
        let _ = tokio_fs::remove_dir_all(&job_dir).await;
        return Err(json_error(StatusCode::FORBIDDEN, err.message()));
    }

    let mut transaction = pool
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

    if let Err(err) =
        history::record_job_start(&pool, MODULE_SUMMARIZER, user.id, job_id.to_string()).await
    {
        error!(?err, %job_id, "failed to record summarizer job history");
    }

    spawn_job_worker(state.clone(), job_id);

    Ok(Json(JobSubmission::new(
        job_id,
        format!("/api/summarizer/jobs/{}", job_id),
    )))
}

async fn job_status(
    State(state): State<AppState>,
    jar: CookieJar,
    AxumPath(job_id): AxumPath<Uuid>,
) -> Result<Json<JobStatusResponse>, (StatusCode, Json<ApiMessage>)> {
    let user = auth::current_user_or_json_error(&state, &jar)
        .await
        .map_err(|JsonAuthError { status, message }| json_error(status, message))?;

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
            Json(ApiMessage::new("未找到任务或任务已失效。")),
        )
    })?;

    if job.user_id != user.id && !user.is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiMessage::new("您无权访问该任务。")),
        ));
    }

    let documents = sqlx::query_as::<_, DocumentRecord>(
        "SELECT id, original_filename, status, status_detail, error_message FROM summary_documents WHERE job_id = $1 ORDER BY ordinal",
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

async fn download_combined_output(
    State(state): State<AppState>,
    jar: CookieJar,
    AxumPath((job_id, variant)): AxumPath<(Uuid, String)>,
) -> Result<Response, (StatusCode, Json<ApiMessage>)> {
    let user = auth::current_user_or_json_error(&state, &jar)
        .await
        .map_err(|JsonAuthError { status, message }| json_error(status, message))?;

    let pool = state.pool();

    let job = sqlx::query_as::<_, CombinedJobRecord>(
        "SELECT user_id, combined_summary_path, combined_translation_path, files_purged_at FROM summary_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_optional(&pool)
    .await
    .map_err(|err| internal_error(err.into()))?
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiMessage::new("未找到任务。")),
        )
    })?;

    if job.user_id != user.id && !user.is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiMessage::new("您无权访问该任务。")),
        ));
    }

    if job.files_purged_at.is_some() {
        return Err((
            StatusCode::GONE,
            Json(ApiMessage::new("该任务的下载文件已过期并被清除。")),
        ));
    }

    let (path, suffix) = match variant.as_str() {
        "summary" => job
            .combined_summary_path
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(ApiMessage::new("汇总摘要尚不可用。")),
                )
            })
            .map(|path| (path, "combined-summary"))?,
        "translation" => job
            .combined_translation_path
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(ApiMessage::new("汇总译文尚不可用。")),
                )
            })
            .map(|path| (path, "combined-translation"))?,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiMessage::new("未知的下载类型。")),
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
        header::HeaderValue::from_str(&format!(r#"attachment; filename="{}""#, filename))
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

fn format_heading(idx: usize, filename: &str) -> String {
    format!("Document {} — {}", idx + 1, filename)
}

async fn execute_llm_with_retry(
    client: &crate::llm::LlmClient,
    request: crate::llm::LlmRequest,
    operation: &str,
) -> Result<crate::llm::LlmResponse> {
    let mut attempt = 0;
    let mut last_error = None;

    while attempt < MAX_RETRIES {
        attempt += 1;

        match client.execute(request.clone()).await {
            Ok(response) => return Ok(response),
            Err(err) => {
                warn!(
                    ?err,
                    attempt,
                    max_retries = MAX_RETRIES,
                    operation,
                    "LLM request failed, will retry"
                );
                last_error = Some(err);

                if attempt < MAX_RETRIES {
                    let delay = INITIAL_RETRY_DELAY_MS * (2_u64.pow(attempt - 1));
                    sleep(Duration::from_millis(delay)).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("LLM request failed after {} retries", MAX_RETRIES)))
}

struct DocumentProcessingResult {
    document_id: Uuid,
    idx: usize,
    original_filename: String,
    success: bool,
    summary_text: Option<String>,
    translation_text: Option<String>,
    summary_tokens: i64,
    translation_tokens: i64,
    error_message: Option<String>,
    status_detail: Option<String>,
}

async fn process_single_document(
    state: AppState,
    job_id: Uuid,
    document: ProcessingDocumentRecord,
    idx: usize,
    document_kind: DocumentKind,
    models: crate::config::SummarizerModels,
    prompts: crate::config::SummarizerPrompts,
    translation_prompt: String,
    should_translate: bool,
    semaphore: Arc<Semaphore>,
) -> DocumentProcessingResult {
    let _permit = semaphore.acquire().await.expect("semaphore closed");

    let pool = state.pool();
    let status_detail = format!("Reading {}", document.original_filename);

    let _ = update_document_status(
        &pool,
        document.id,
        STATUS_PROCESSING,
        Some(&status_detail),
        None,
    )
    .await;

    let _ = update_job_status(&pool, job_id, Some(&status_detail)).await;

    // Read document text
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
            let _ = update_document_status(
                &pool,
                document.id,
                STATUS_FAILED,
                Some("Unable to extract text from the document."),
                Some(&err.to_string()),
            )
            .await;

            return DocumentProcessingResult {
                document_id: document.id,
                idx,
                original_filename: document.original_filename,
                success: false,
                summary_text: None,
                translation_text: None,
                summary_tokens: 0,
                translation_tokens: 0,
                error_message: Some(err.to_string()),
                status_detail: Some("Unable to extract text from the document.".to_string()),
            };
        }
    };

    // Generate summary with retry
    let summary_prompt = document_prompt(&prompts, document_kind);
    let summary_request =
        build_summary_request(models.summary_model.as_str(), summary_prompt, &text);
    let llm_client = state.llm_client();

    let summary_response = match execute_llm_with_retry(
        &llm_client,
        summary_request,
        &format!("summarization for {}", document.original_filename),
    )
    .await
    {
        Ok(resp) => resp,
        Err(err) => {
            error!(?err, document_id = %document.id, "summarization request failed after retries");
            let _ = update_document_status(
                &pool,
                document.id,
                STATUS_FAILED,
                Some("Summarization failed."),
                Some(&err.to_string()),
            )
            .await;

            return DocumentProcessingResult {
                document_id: document.id,
                idx,
                original_filename: document.original_filename,
                success: false,
                summary_text: None,
                translation_text: None,
                summary_tokens: 0,
                translation_tokens: 0,
                error_message: Some(err.to_string()),
                status_detail: Some("Summarization failed.".to_string()),
            };
        }
    };

    let summary_text = summary_response.text.trim().to_string();
    let summary_tokens = summary_response.token_usage.total_tokens as i64;

    // Handle translation if needed
    let mut translation_text = None;
    let mut translation_tokens = 0_i64;
    let mut translation_status_detail = None;
    let mut translation_error = None;

    if should_translate {
        let _ = update_job_status(
            &pool,
            job_id,
            Some(&format!(
                "Translating {} (glossary {})",
                document.original_filename,
                translation_enabled_text(should_translate)
            )),
        )
        .await;

        let translation_request = build_translation_request(
            models.translation_model.as_str(),
            translation_prompt.clone(),
            &summary_text,
        );

        match execute_llm_with_retry(
            &llm_client,
            translation_request,
            &format!("translation for {}", document.original_filename),
        )
        .await
        {
            Ok(response) => {
                let text = response.text.trim().to_string();
                translation_tokens = response.token_usage.total_tokens as i64;
                translation_text = Some(text);
            }
            Err(err) => {
                error!(?err, document_id = %document.id, "translation request failed after retries");
                translation_status_detail =
                    Some("Translation failed; summary available.".to_string());
                translation_error = Some(err.to_string());
            }
        }
    }

    DocumentProcessingResult {
        document_id: document.id,
        idx,
        original_filename: document.original_filename,
        success: true,
        summary_text: Some(summary_text),
        translation_text,
        summary_tokens,
        translation_tokens,
        error_message: translation_error,
        status_detail: translation_status_detail,
    }
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
    let settings = state
        .summarizer_settings()
        .await
        .ok_or_else(|| anyhow!("Summarizer settings are not configured."))?;
    let models = settings.models.clone();
    let prompts = settings.prompts.clone();

    let glossary_terms = fetch_glossary_terms(&pool).await.unwrap_or_else(|err| {
        error!(?err, "failed to load glossary terms");
        Vec::new()
    });
    let translation_prompt = build_translation_prompt(&prompts, &glossary_terms);

    // Create semaphore for concurrency control
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_DOCUMENTS));

    // Spawn concurrent document processing tasks
    let mut tasks = Vec::new();

    for (idx, document) in documents.into_iter().enumerate() {
        let state_clone = state.clone();
        let models_clone = models.clone();
        let prompts_clone = prompts.clone();
        let translation_prompt_clone = translation_prompt.clone();
        let semaphore_clone = semaphore.clone();

        let task = tokio::spawn(process_single_document(
            state_clone,
            job_id,
            document,
            idx,
            document_kind,
            models_clone,
            prompts_clone,
            translation_prompt_clone,
            job.translate,
            semaphore_clone,
        ));

        tasks.push(task);
    }

    // Wait for all tasks to complete
    let results = futures::future::join_all(tasks).await;

    // Process results
    let mut combined_summary_path: Option<String> = None;
    let mut combined_translation_path: Option<String> = None;
    let mut success_count = 0_i64;
    let mut summary_tokens_total = 0_i64;
    let mut translation_tokens_total = 0_i64;

    // Sort results by index to maintain order
    let mut processed_results: Vec<DocumentProcessingResult> =
        results.into_iter().filter_map(|r| r.ok()).collect();
    processed_results.sort_by_key(|r| r.idx);

    for result in processed_results {
        let heading = format_heading(result.idx, &result.original_filename);

        // Handle failed documents - persist failure information
        if !result.success {
            let _ = sqlx::query("UPDATE summary_documents SET status = $2, status_detail = $3, error_message = $4, updated_at = NOW() WHERE id = $1")
                .bind(result.document_id)
                .bind(STATUS_FAILED)
                .bind(result.status_detail.as_deref())
                .bind(result.error_message.as_deref())
                .execute(&pool)
                .await;
            continue;
        }

        // Append to combined summary
        if let Some(ref summary_text) = result.summary_text {
            if combined_summary_path.is_none() {
                combined_summary_path = Some(
                    combined_output_path(&job_dir, "summary")
                        .to_string_lossy()
                        .to_string(),
                );
            }

            if let Some(ref combined_path) = combined_summary_path {
                let combined_summary_target = PathBuf::from(combined_path);
                match tokio::task::spawn_blocking({
                    let path = combined_summary_target.clone();
                    let heading = heading.clone();
                    let content = summary_text.clone();
                    move || append_to_file(&path, &heading, &content)
                })
                .await
                .unwrap_or_else(|err| Err(anyhow!(err)))
                {
                    Ok(_) => {}
                    Err(err) => {
                        error!(?err, document_id = %result.document_id, "failed to append to combined summary");
                        // Mark document as failed due to combined file write error
                        let _ = sqlx::query("UPDATE summary_documents SET status = $2, status_detail = $3, error_message = $4, updated_at = NOW() WHERE id = $1")
                            .bind(result.document_id)
                            .bind(STATUS_FAILED)
                            .bind("Failed to write combined summary file.")
                            .bind(err.to_string())
                            .execute(&pool)
                            .await;
                        continue;
                    }
                }
            }
        }

        // Append to combined translation
        if let Some(ref translation_text) = result.translation_text {
            if combined_translation_path.is_none() {
                combined_translation_path = Some(
                    combined_output_path(&job_dir, "translation")
                        .to_string_lossy()
                        .to_string(),
                );
            }

            if let Some(ref combined_path) = combined_translation_path {
                let combined_translation_target = PathBuf::from(combined_path);
                match tokio::task::spawn_blocking({
                    let path = combined_translation_target.clone();
                    let heading = heading.clone();
                    let content = translation_text.clone();
                    move || append_to_file(&path, &heading, &content)
                })
                .await
                .unwrap_or_else(|err| Err(anyhow!(err)))
                {
                    Ok(_) => {}
                    Err(err) => {
                        error!(?err, document_id = %result.document_id, "failed to append to combined translation");
                        // Mark document as failed due to combined file write error
                        let _ = sqlx::query("UPDATE summary_documents SET status = $2, status_detail = $3, error_message = $4, updated_at = NOW() WHERE id = $1")
                            .bind(result.document_id)
                            .bind(STATUS_FAILED)
                            .bind("Failed to write combined translation file.")
                            .bind(err.to_string())
                            .execute(&pool)
                            .await;
                        continue;
                    }
                }
            }
        }

        // Update database with results - propagate error on failure
        if let Err(err) = sqlx::query("UPDATE summary_documents SET status = $2, status_detail = $3, summary_text = $4, translation_text = $5, summary_path = NULL, translation_path = NULL, summary_tokens = $6, translation_tokens = $7, error_message = $8, updated_at = NOW() WHERE id = $1")
            .bind(result.document_id)
            .bind(STATUS_COMPLETED)
            .bind(result.status_detail.as_deref())
            .bind(result.summary_text.as_ref())
            .bind(result.translation_text.as_ref())
            .bind(result.summary_tokens)
            .bind(result.translation_tokens)
            .bind(result.error_message.as_deref())
            .execute(&pool)
            .await
        {
            error!(?err, document_id = %result.document_id, "failed to update document record in database");
            // This is a critical failure - we can't mark the document as completed
            // Try to mark it as failed instead
            let _ = sqlx::query("UPDATE summary_documents SET status = $2, status_detail = $3, error_message = $4, updated_at = NOW() WHERE id = $1")
                .bind(result.document_id)
                .bind(STATUS_FAILED)
                .bind("Failed to persist document results to database.")
                .bind(err.to_string())
                .execute(&pool)
                .await;
            continue;
        }

        summary_tokens_total += result.summary_tokens;
        translation_tokens_total += result.translation_tokens;
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
        let tokens_total = summary_tokens_total + translation_tokens_total;
        if let Err(err) = usage::record_usage(
            &pool,
            job.user_id,
            MODULE_SUMMARIZER,
            tokens_total,
            success_count as i64,
        )
        .await
        {
            error!(?err, "failed to record summarizer usage");
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
    error_message: Option<String>,
}

#[derive(sqlx::FromRow)]
struct CombinedJobRecord {
    user_id: Uuid,
    combined_summary_path: Option<String>,
    combined_translation_path: Option<String>,
    files_purged_at: Option<DateTime<Utc>>,
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

fn internal_error(err: anyhow::Error) -> (StatusCode, Json<ApiMessage>) {
    error!(?err, "internal error in summarizer module");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiMessage::new("服务器内部错误。")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    #[test]
    fn generates_translation_prompt_with_terms() {
        let now = Utc::now();
        let terms = vec![GlossaryTermRow {
            id: Uuid::new_v4(),
            source_term: "neuron".to_string(),
            target_term: "神经元".to_string(),
            notes: None,
            created_at: now,
            updated_at: now,
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
