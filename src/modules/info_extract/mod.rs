use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use axum::{
    Json, Router,
    extract::{Multipart, Path as AxumPath, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use axum_extra::extract::cookie::CookieJar;
use calamine::{DataType, Reader, Xlsx};
use futures::future::join_all;
use pdf_extract::extract_text as extract_pdf_text;
use rust_xlsxwriter::Workbook;
use sanitize_filename::sanitize;
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::{fs as tokio_fs, io::AsyncWriteExt, sync::Semaphore, task, time::sleep};
use tracing::{error, warn};
use uuid::Uuid;

mod admin;

use crate::{
    AppState, SESSION_COOKIE,
    config::{InfoExtractModels, InfoExtractPrompts},
    escape_html,
    llm::{ChatMessage, LlmRequest, MessageRole},
    render_footer,
    usage::{self, MODULE_INFO_EXTRACT},
};

const STORAGE_ROOT: &str = "storage/infoextract";
const STATUS_PENDING: &str = "pending";
const STATUS_PROCESSING: &str = "processing";
const STATUS_COMPLETED: &str = "completed";
const STATUS_FAILED: &str = "failed";
const MAX_DOCUMENTS: usize = 100;
const MAX_RETRIES: usize = 3;
const RETRY_DELAY_MS: u64 = 1_500;
const MAX_DOCUMENT_TEXT_CHARS: usize = 20_000;
const MAX_CONCURRENT_DOCUMENTS: usize = 5;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tools/infoextract", get(info_extract_page))
        .route("/tools/infoextract/jobs", post(create_job))
        .route("/api/infoextract/jobs/:id", get(job_status))
        .route(
            "/api/infoextract/jobs/:id/download/result",
            get(download_result),
        )
        .route("/dashboard/modules/infoextract", get(admin::settings_page))
        .route(
            "/dashboard/modules/infoextract/models",
            post(admin::save_models),
        )
        .route(
            "/dashboard/modules/infoextract/prompts",
            post(admin::save_prompts),
        )
}

#[derive(sqlx::FromRow)]
struct SessionUser {
    id: Uuid,
    username: String,
    is_admin: bool,
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

#[derive(Serialize)]
struct JobSubmissionResponse {
    job_id: Uuid,
    status_url: String,
}

#[derive(Serialize)]
struct JobStatusResponse {
    job_id: Uuid,
    status: String,
    status_detail: Option<String>,
    error_message: Option<String>,
    result_download_url: Option<String>,
    documents: Vec<JobDocumentStatus>,
}

#[derive(Serialize)]
struct JobDocumentStatus {
    id: Uuid,
    original_filename: String,
    status: String,
    status_detail: Option<String>,
    error_message: Option<String>,
    attempt_count: i32,
}

#[derive(sqlx::FromRow)]
struct JobRecord {
    user_id: Uuid,
    status: String,
    status_detail: Option<String>,
    error_message: Option<String>,
    result_path: Option<String>,
}

#[derive(sqlx::FromRow)]
struct DocumentRecord {
    id: Uuid,
    original_filename: String,
    status: String,
    status_detail: Option<String>,
    error_message: Option<String>,
    attempt_count: i32,
}

#[derive(sqlx::FromRow)]
struct DocumentSourceRecord {
    id: Uuid,
    ordinal: i32,
    original_filename: String,
    source_path: String,
}

#[derive(sqlx::FromRow)]
struct DownloadRecord {
    user_id: Uuid,
    result_path: Option<String>,
}

#[derive(Debug, Clone)]
struct ExtractionField {
    name: String,
    description: Option<String>,
    examples: Vec<String>,
    allowed_values: Vec<String>,
}

#[derive(Debug, Clone)]
struct DocumentExtractionResult {
    ordinal: i32,
    filename: String,
    values: Option<Map<String, Value>>,
    error: Option<String>,
    tokens_used: i64,
    success: bool,
}

#[derive(Debug)]
struct UploadedDocument {
    stored_path: PathBuf,
    original_name: String,
}

async fn fetch_session_user(state: &AppState, jar: &CookieJar) -> Result<SessionUser> {
    let token_cookie = jar.get(SESSION_COOKIE).context("missing auth cookie")?;

    let token = Uuid::parse_str(token_cookie.value()).context("invalid session token")?;

    let user = sqlx::query_as::<_, SessionUser>(
        "SELECT users.id, users.username, users.is_admin
         FROM sessions
         INNER JOIN users ON users.id = sessions.user_id
         WHERE sessions.id = $1 AND sessions.expires_at > NOW()",
    )
    .bind(token)
    .fetch_optional(state.pool_ref())
    .await
    .context("failed to load session user")?
    .context("session expired")?;

    Ok(user)
}

async fn info_extract_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Html<String>, Response> {
    let user = fetch_session_user(&state, &jar)
        .await
        .map_err(|_| (StatusCode::UNAUTHORIZED, "未登录或会话失效").into_response())?;

    let footer = render_footer();
    let username = escape_html(&user.username);
    let admin_link = if user.is_admin {
        r#"<a class=\"admin-link\" href=\"/dashboard/modules/infoextract\">模块管理</a>"#
            .to_string()
    } else {
        String::new()
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>信息提取 | Zhang Group AI Toolkit</title>
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
        button {{ padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        button:disabled {{ opacity: 0.6; cursor: not-allowed; }}
        .note {{ color: #475569; font-size: 0.95rem; line-height: 1.6; }}
        .drop-zone {{ border: 2px dashed #cbd5f5; border-radius: 12px; padding: 2rem; text-align: center; background: #f8fafc; transition: border-color 0.2s ease, background 0.2s ease; cursor: pointer; margin-bottom: 1rem; color: #475569; }}
        .drop-zone strong {{ color: #1d4ed8; }}
        .drop-zone.dragover {{ border-color: #2563eb; background: #e0f2fe; }}
        .drop-zone input[type="file"] {{ display: none; }}
        .browse-link {{ color: #2563eb; text-decoration: underline; cursor: pointer; font-weight: 600; }}
        .file-list {{ margin: 1rem 0; padding: 0; list-style: none; display: grid; gap: 0.6rem; }}
        .file-item {{ display: flex; justify-content: space-between; align-items: center; padding: 0.6rem 0.8rem; border: 1px solid #e2e8f0; border-radius: 8px; background: #ffffff; box-shadow: 0 6px 16px rgba(15,23,42,0.05); font-size: 0.92rem; }}
        .file-item span {{ flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; padding-right: 0.75rem; }}
        .remove-file {{ background: transparent; color: #dc2626; border: none; font-weight: 600; cursor: pointer; }}
        .remove-file:hover {{ text-decoration: underline; }}
        .status {{ margin-top: 1.5rem; font-size: 0.95rem; }}
        .status .error {{ color: #b91c1c; }}
        .status .success {{ color: #166534; }}
        .job-table {{ width: 100%; border-collapse: collapse; margin-top: 1rem; }}
        .job-table th, .job-table td {{ padding: 0.65rem 0.85rem; border: 1px solid #e2e8f0; text-align: left; font-size: 0.92rem; }}
        .job-table th {{ background: #f1f5f9; }}
        .status-tag {{ display: inline-block; padding: 0.2rem 0.65rem; border-radius: 999px; font-size: 0.85rem; font-weight: 600; }}
        .status-tag.pending {{ background: #fef3c7; color: #92400e; }}
        .status-tag.processing {{ background: #e0f2fe; color: #1d4ed8; }}
        .status-tag.completed {{ background: #dcfce7; color: #166534; }}
        .status-tag.failed {{ background: #fee2e2; color: #b91c1c; }}
        .downloads a {{ color: #2563eb; text-decoration: none; font-weight: 600; }}
        .downloads a:hover {{ text-decoration: underline; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
        @media (max-width: 768px) {{
            header {{ padding: 1.5rem 1rem; }}
            main {{ padding: 1.5rem 1rem; }}
            .header-bar {{ flex-direction: column; align-items: flex-start; }}
            .drop-zone {{ padding: 1.5rem 1rem; }}
            .job-table {{ font-size: 0.85rem; }}
        }}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>信息提取</h1>
            <div style="display:flex; gap:0.75rem; align-items:center; flex-wrap:wrap;">
                <a class="back-link" href="/">← 返回首页</a>
                {admin_link}
            </div>
        </div>
        <p class="note">当前登录：<strong>{username}</strong>。上传最多 100 篇 PDF 论文与字段定义表（XLSX），系统将批量抽取自定义信息并生成汇总表。</p>
    </header>
    <main>
        <section class="panel">
            <h2>发起新任务</h2>
            <form id="infoextract-form">
                <label>上传论文（PDF，最多 100 篇）</label>
                <div id="doc-drop-area" class="drop-zone">
                    <p><strong>拖拽 PDF 文件</strong>到此处，或<span class="browse-link" data-target="documents">点击选择</span>文件。</p>
                    <p class="note" style="margin:0">支持批量上传，单次任务上限 100 篇。</p>
                    <input id="documents" name="documents" type="file" multiple accept=".pdf" required>
                </div>
                <ul id="documents-list" class="file-list"></ul>

                <label>上传字段定义表（XLSX）</label>
                <div id="spec-drop-area" class="drop-zone">
                    <p><strong>拖拽或选择 XLSX 文件</strong>，描述要提取的字段。</p>
                    <p class="note" style="margin:0">第 1 行名称，第 2 行说明，第 3 行示例（分号分隔），第 4 行枚举（分号分隔）。至少填写一行说明/示例/枚举，且示例与枚举不可同时填写。</p>
                    <span class="browse-link" data-target="spec">点击选择</span>
                    <input id="spec" name="spec" type="file" accept=".xlsx" required>
                </div>
                <ul id="spec-list" class="file-list"></ul>

                <button type="submit">开始处理</button>
            </form>
            <div id="form-status" class="status"></div>
        </section>
        <section class="panel">
            <h2>任务进度</h2>
            <div id="job-status"></div>
        </section>
        {footer}
    </main>
    <script>
        const form = document.getElementById('infoextract-form');
        const statusBox = document.getElementById('form-status');
        const jobStatus = document.getElementById('job-status');
        const documentsInput = document.getElementById('documents');
        const specInput = document.getElementById('spec');
        const documentsList = document.getElementById('documents-list');
        const specList = document.getElementById('spec-list');
        const docDropArea = document.getElementById('doc-drop-area');
        const specDropArea = document.getElementById('spec-drop-area');
        let pollTimer = null;

        function renderDocuments() {{
            documentsList.innerHTML = '';
            const files = Array.from(documentsInput.files);
            files.forEach((file, idx) => {{
                const li = document.createElement('li');
                li.className = 'file-item';
                li.innerHTML = `<span>${{file.name}}</span>`;
                const remove = document.createElement('button');
                remove.type = 'button';
                remove.className = 'remove-file';
                remove.textContent = '移除';
                remove.addEventListener('click', () => removeDocument(idx));
                li.appendChild(remove);
                documentsList.appendChild(li);
            }});
        }}

        function removeDocument(index) {{
            const files = Array.from(documentsInput.files);
            files.splice(index, 1);
            const dataTransfer = new DataTransfer();
            files.forEach(file => dataTransfer.items.add(file));
            documentsInput.files = dataTransfer.files;
            renderDocuments();
        }}

        function setSpecFile(file) {{
            const dataTransfer = new DataTransfer();
            dataTransfer.items.add(file);
            specInput.files = dataTransfer.files;
            renderSpec();
        }}

        function renderSpec() {{
            specList.innerHTML = '';
            const file = specInput.files[0];
            if (!file) return;
            const li = document.createElement('li');
            li.className = 'file-item';
            li.innerHTML = `<span>${{file.name}}</span>`;
            const remove = document.createElement('button');
            remove.type = 'button';
            remove.className = 'remove-file';
            remove.textContent = '移除';
            remove.addEventListener('click', () => {{ specInput.value = ''; specList.innerHTML = ''; }});
            li.appendChild(remove);
            specList.appendChild(li);
        }}

        function preventDefaults(e) {{
            e.preventDefault();
            e.stopPropagation();
        }}

        function handleDrop(zone, handler) {{
            zone.addEventListener('dragenter', preventDefaults);
            zone.addEventListener('dragover', e => {{
                preventDefaults(e);
                zone.classList.add('dragover');
            }});
            zone.addEventListener('dragleave', e => {{
                preventDefaults(e);
                zone.classList.remove('dragover');
            }});
            zone.addEventListener('drop', e => {{
                preventDefaults(e);
                zone.classList.remove('dragover');
                handler(e.dataTransfer.files);
            }});
        }}

        handleDrop(docDropArea, files => {{
            const current = Array.from(documentsInput.files);
            for (const file of files) {{
                if (!file.name.toLowerCase().endsWith('.pdf')) continue;
                current.push(file);
            }}
            if (current.length > {max_docs}) {{
                statusBox.innerHTML = '<span class="error">单次任务最多 100 篇论文。</span>';
                current.splice({max_docs});
            }}
            const dt = new DataTransfer();
            current.forEach(file => dt.items.add(file));
            documentsInput.files = dt.files;
            renderDocuments();
        }});

        handleDrop(specDropArea, files => {{
            const file = Array.from(files).find(f => f.name.toLowerCase().endsWith('.xlsx'));
            if (file) {{
                setSpecFile(file);
            }}
        }});

        document.querySelectorAll('.browse-link').forEach(link => {{
            link.addEventListener('click', () => {{
                const target = link.getAttribute('data-target');
                if (target === 'documents') {{
                    documentsInput.click();
                }} else if (target === 'spec') {{
                    specInput.click();
                }}
            }});
        }});

        documentsInput.addEventListener('change', renderDocuments);
        specInput.addEventListener('change', renderSpec);

        form.addEventListener('submit', async (event) => {{
            event.preventDefault();
            statusBox.textContent = '';

            const pdfs = Array.from(documentsInput.files);
            if (pdfs.length === 0) {{
                statusBox.innerHTML = '<span class="error">请至少上传一篇 PDF 论文。</span>';
                return;
            }}
            if (pdfs.length > {max_docs}) {{
                statusBox.innerHTML = '<span class="error">单次任务最多 100 篇论文。</span>';
                return;
            }}
            if (!specInput.files[0]) {{
                statusBox.innerHTML = '<span class="error">请提供字段定义表 XLSX。</span>';
                return;
            }}

            const formData = new FormData();
            pdfs.forEach(file => formData.append('documents', file));
            formData.append('spec', specInput.files[0]);

            form.querySelector('button').disabled = true;
            statusBox.innerHTML = '任务提交中，请稍候…';

            try {{
                const response = await fetch('/tools/infoextract/jobs', {{
                    method: 'POST',
                    body: formData,
                }});

                if (!response.ok) {{
                    const payload = await response.json().catch(() => ({{ message: '提交失败。' }}));
                    statusBox.innerHTML = `<span class="error">${{payload.message || '提交失败。'}}</span>`;
                    form.querySelector('button').disabled = false;
                    return;
                }}

                const payload = await response.json();
                statusBox.innerHTML = '<span class="success">任务已创建，正在处理。</span>';
                startPolling(payload.status_url);
            }} catch (err) {{
                statusBox.innerHTML = '<span class="error">网络请求失败，请稍后重试。</span>';
            }} finally {{
                form.querySelector('button').disabled = false;
            }}
        }});

        function startPolling(url) {{
            if (pollTimer) {{
                clearInterval(pollTimer);
            }}
            fetchStatus(url);
            pollTimer = setInterval(() => fetchStatus(url), 3500);
        }}

        async function fetchStatus(url) {{
            if (!url) return;
            try {{
                const response = await fetch(url);
                if (!response.ok) {{
                    jobStatus.innerHTML = '<p class="error">获取任务状态失败。</p>';
                    return;
                }}
                const payload = await response.json();
                renderStatus(payload);
                if (payload.status === '{completed}' || payload.status === '{failed}') {{
                    clearInterval(pollTimer);
                    pollTimer = null;
                }}
            }} catch (err) {{
                jobStatus.innerHTML = '<p class="error">获取任务状态失败。</p>';
            }}
        }}

        function renderStatus(payload) {{
            const rows = payload.documents.map(doc => {{
                const detail = doc.status_detail ? `<div>${{doc.status_detail}}</div>` : '';
                const error = doc.error_message ? `<div class="error">${{doc.error_message}}</div>` : '';
                return `
                    <tr>
                        <td>${{doc.original_filename}}</td>
                        <td><span class="status-tag ${{doc.status}}">${{translateStatus(doc.status)}}</span></td>
                        <td>重试次数：${{doc.attempt_count}}</td>
                        <td>${{detail}}${{error}}</td>
                    </tr>
                `;
            }}).join('');

            const download = payload.result_download_url
                ? `<div class="downloads"><a href="${{payload.result_download_url}}">下载提取结果 XLSX</a></div>`
                : '';

            const overallError = payload.error_message
                ? `<p class="error">${{payload.error_message}}</p>`
                : '';

            jobStatus.innerHTML = `
                <p>任务 ID：<code>${{payload.job_id}}</code></p>
                <p>状态：<span class="status-tag ${{payload.status}}">${{translateStatus(payload.status)}}</span></p>
                ${{download}}
                ${{overallError}}
                <table class="job-table">
                    <thead>
                        <tr>
                            <th>文件名</th>
                            <th>状态</th>
                            <th>尝试次数</th>
                            <th>详情</th>
                        </tr>
                    </thead>
                    <tbody>${{rows}}</tbody>
                </table>
            `;
        }}

        function translateStatus(status) {{
            switch (status) {{
                case '{pending}': return '排队中';
                case '{processing}': return '处理中';
                case '{completed}': return '已完成';
                case '{failed}': return '失败';
                default: return status;
            }}
        }}
    </script>
</body>
</html>"#,
        username = username,
        admin_link = admin_link,
        footer = footer,
        pending = STATUS_PENDING,
        processing = STATUS_PROCESSING,
        completed = STATUS_COMPLETED,
        failed = STATUS_FAILED,
        max_docs = MAX_DOCUMENTS,
    );

    Ok(Html(html))
}

async fn create_job(
    State(state): State<AppState>,
    jar: CookieJar,
    mut multipart: Multipart,
) -> Result<Json<JobSubmissionResponse>, (StatusCode, Json<ApiError>)> {
    let user = fetch_session_user(&state, &jar).await.map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            Json(ApiError::new("请先登录后再提交任务。")),
        )
    })?;

    ensure_storage_root()
        .await
        .map_err(|err| internal_error(err.into()))?;

    let job_id = Uuid::new_v4();
    let job_dir = PathBuf::from(STORAGE_ROOT).join(job_id.to_string());
    tokio_fs::create_dir_all(&job_dir)
        .await
        .map_err(|err| internal_error(err.into()))?;

    let result = async {
        let mut documents: Vec<UploadedDocument> = Vec::new();
        let mut spec_bytes: Option<Vec<u8>> = None;
        let mut spec_filename: Option<String> = None;
        let mut extraction_fields: Option<Vec<ExtractionField>> = None;
        let mut file_index = 0usize;

        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|err| internal_error(err.into()))?
        {
            let name = field.name().map(|s| s.to_string());

            match name.as_deref() {
                Some("documents") => {
                    let Some(filename) = field.file_name().map(|s| s.to_string()) else {
                        continue;
                    };
                    let safe_name = sanitize(&filename);
                    let ext = Path::new(&safe_name)
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if ext != "pdf" {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            Json(ApiError::new("仅支持上传 PDF 文件。")),
                        ));
                    }

                    let stored_path = job_dir.join(format!("paper_{file_index:03}_{safe_name}"));
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

                    documents.push(UploadedDocument {
                        stored_path,
                        original_name: filename,
                    });
                }
                Some("spec") => {
                    if spec_bytes.is_some() {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            Json(ApiError::new("仅允许上传 1 个字段定义表。")),
                        ));
                    }

                    let filename = field
                        .file_name()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "spec.xlsx".to_string());
                    let safe_name = sanitize(&filename);
                    if !safe_name.to_lowercase().ends_with(".xlsx") {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            Json(ApiError::new("字段定义表必须为 XLSX 格式。")),
                        ));
                    }

                    let bytes = field
                        .bytes()
                        .await
                        .map_err(|err| internal_error(err.into()))?;

                    let bytes_vec = bytes.to_vec();

                    let fields = parse_extraction_spec(&bytes_vec).map_err(|err| {
                        (
                            StatusCode::BAD_REQUEST,
                            Json(ApiError::new(format!("字段定义表格式错误：{}", err))),
                        )
                    })?;

                    spec_bytes = Some(bytes_vec);
                    spec_filename = Some(safe_name);
                    extraction_fields = Some(fields);
                }
                _ => {}
            }
        }

        if documents.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiError::new("请至少上传一篇 PDF 论文。")),
            ));
        }

        if documents.len() > MAX_DOCUMENTS {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiError::new("单次任务最多 100 篇 PDF。")),
            ));
        }

        let spec_bytes = spec_bytes.ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ApiError::new("请上传字段定义表 XLSX。")),
            )
        })?;
        let spec_filename = spec_filename.unwrap_or_else(|| "spec.xlsx".to_string());
        let fields = extraction_fields.expect("spec bytes guarantee fields");

        let spec_path = job_dir.join(&spec_filename);
        tokio_fs::write(&spec_path, &spec_bytes)
            .await
            .map_err(|err| internal_error(err.into()))?;

        let pool = state.pool();

        if let Err(err) = usage::ensure_within_limits(
            &pool,
            user.id,
            MODULE_INFO_EXTRACT,
            documents.len() as i64,
        )
        .await
        {
            return Err((StatusCode::FORBIDDEN, Json(ApiError::new(err.message()))));
        }

        let mut transaction = pool
            .begin()
            .await
            .map_err(|err| internal_error(err.into()))?;

        sqlx::query(
            "INSERT INTO info_extract_jobs (id, user_id, status, spec_filename, spec_path)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(job_id)
        .bind(user.id)
        .bind(STATUS_PENDING)
        .bind(&spec_filename)
        .bind(spec_path.to_string_lossy().to_string())
        .execute(&mut *transaction)
        .await
        .map_err(|err| internal_error(err.into()))?;

        for (ordinal, document) in documents.iter().enumerate() {
            sqlx::query(
                "INSERT INTO info_extract_documents (id, job_id, ordinal, original_filename, source_path, status)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(Uuid::new_v4())
            .bind(job_id)
            .bind(ordinal as i32)
            .bind(&document.original_name)
            .bind(document.stored_path.to_string_lossy().to_string())
            .bind(STATUS_PENDING)
            .execute(&mut *transaction)
            .await
            .map_err(|err| internal_error(err.into()))?;
        }

        transaction
            .commit()
            .await
            .map_err(|err| internal_error(err.into()))?;

        spawn_job_worker(state.clone(), job_id, fields);

        Ok(Json(JobSubmissionResponse {
            job_id,
            status_url: format!("/api/infoextract/jobs/{}", job_id),
        }))
    }
    .await;

    if result.is_err() {
        let _ = tokio_fs::remove_dir_all(&job_dir).await;
    }

    result
}

async fn job_status(
    State(state): State<AppState>,
    jar: CookieJar,
    AxumPath(job_id): AxumPath<Uuid>,
) -> Result<Json<JobStatusResponse>, (StatusCode, Json<ApiError>)> {
    let user = fetch_session_user(&state, &jar).await.map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            Json(ApiError::new("请先登录后查看任务状态。")),
        )
    })?;

    let pool = state.pool();

    let job = sqlx::query_as::<_, JobRecord>(
        "SELECT user_id, status, status_detail, error_message, result_path
         FROM info_extract_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_optional(&pool)
    .await
    .map_err(|err| internal_error(err.into()))?
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("未找到任务或任务已过期。")),
        )
    })?;

    if job.user_id != user.id && !user.is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError::new("您无权访问该任务。")),
        ));
    }

    let documents = sqlx::query_as::<_, DocumentRecord>(
        "SELECT id, original_filename, status, status_detail, error_message, attempt_count
         FROM info_extract_documents WHERE job_id = $1 ORDER BY ordinal",
    )
    .bind(job_id)
    .fetch_all(&pool)
    .await
    .map_err(|err| internal_error(err.into()))?;

    let result_download_url = job
        .result_path
        .as_ref()
        .map(|_| format!("/api/infoextract/jobs/{}/download/result", job_id));

    let documents = documents
        .into_iter()
        .map(|doc| JobDocumentStatus {
            id: doc.id,
            original_filename: doc.original_filename,
            status: doc.status,
            status_detail: doc.status_detail,
            error_message: doc.error_message,
            attempt_count: doc.attempt_count,
        })
        .collect();

    Ok(Json(JobStatusResponse {
        job_id,
        status: job.status,
        status_detail: job.status_detail,
        error_message: job.error_message,
        result_download_url,
        documents,
    }))
}

async fn download_result(
    State(state): State<AppState>,
    jar: CookieJar,
    AxumPath(job_id): AxumPath<Uuid>,
) -> Result<impl IntoResponse, (StatusCode, Json<ApiError>)> {
    let user = fetch_session_user(&state, &jar).await.map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            Json(ApiError::new("请先登录后下载结果。")),
        )
    })?;

    let pool = state.pool();
    let record = sqlx::query_as::<_, DownloadRecord>(
        "SELECT user_id, result_path FROM info_extract_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_optional(&pool)
    .await
    .map_err(|err| internal_error(err.into()))?
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("未找到任务或暂无可下载结果。")),
        )
    })?;

    if record.user_id != user.id && !user.is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError::new("您无权下载该任务的结果。")),
        ));
    }

    let result_path = record.result_path.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("任务尚未生成结果。")),
        )
    })?;

    let bytes = tokio_fs::read(&result_path)
        .await
        .map_err(|err| internal_error(err.into()))?;

    let filename = format!("info_extract_{}.xlsx", job_id);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ),
    );
    let disposition = format!("attachment; filename=\"{}\"", filename);
    let disposition = HeaderValue::from_str(&disposition)
        .map_err(|err| internal_error(anyhow!("invalid header value: {err}")))?;
    headers.insert(header::CONTENT_DISPOSITION, disposition);

    Ok((headers, bytes))
}

fn ensure_status_detail(truncated: bool) -> Option<String> {
    if truncated {
        Some(format!(
            "正文超过 {} 字符，已截断后送入模型。",
            MAX_DOCUMENT_TEXT_CHARS
        ))
    } else {
        None
    }
}

fn split_semicolon(input: &str) -> Vec<String> {
    input
        .split(';')
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(|item| item.to_string())
        .collect()
}

fn cell_to_string(cell: Option<&DataType>) -> Option<String> {
    let value = cell?;
    let text = match value {
        DataType::String(s) => s.trim().to_string(),
        DataType::Float(f) => {
            let mut s = format!("{f}");
            if s.ends_with(".0") {
                s.truncate(s.len() - 2);
            }
            s
        }
        DataType::Int(i) => i.to_string(),
        DataType::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        DataType::DateTime(dt) => dt.to_string(),
        DataType::Empty => String::new(),
        other => other.to_string(),
    };

    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_extraction_spec(bytes: &[u8]) -> Result<Vec<ExtractionField>> {
    let mut workbook =
        Xlsx::new(Cursor::new(bytes)).context("无法打开 XLSX 文件，请确认文件格式无误")?;
    let range = workbook
        .worksheet_range_at(0)
        .ok_or_else(|| anyhow!("Excel 中未找到任何工作表"))??;

    let mut fields = Vec::new();

    let (_, max_cols) = range.get_size();
    for col_idx in 0..max_cols {
        let Some(name) = cell_to_string(range.get((0, col_idx))) else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }

        let description = cell_to_string(range.get((1, col_idx)));
        let examples = cell_to_string(range.get((2, col_idx)));
        let allowed = cell_to_string(range.get((3, col_idx)));

        if description.is_none() && examples.is_none() && allowed.is_none() {
            bail!("第 {} 列至少需要填写说明、示例或枚举之一。", col_idx + 1);
        }

        if examples.is_some() && allowed.is_some() {
            bail!("第 {} 列的示例与枚举不能同时填写，请二选一。", col_idx + 1);
        }

        fields.push(ExtractionField {
            name: name.to_string(),
            description: description.map(|s| s.trim().to_string()),
            examples: examples
                .map(|raw| split_semicolon(&raw))
                .unwrap_or_default(),
            allowed_values: allowed.map(|raw| split_semicolon(&raw)).unwrap_or_default(),
        });
    }

    if fields.is_empty() {
        bail!("字段定义表中未找到有效的列，请检查前四行内容。");
    }

    Ok(fields)
}

fn clamp_document_text(text: &str) -> (String, bool) {
    if text.chars().count() <= MAX_DOCUMENT_TEXT_CHARS {
        return (text.to_string(), false);
    }

    let clipped: String = text.chars().take(MAX_DOCUMENT_TEXT_CHARS).collect();
    (clipped, true)
}

fn build_user_prompt(
    filename: &str,
    fields: &[ExtractionField],
    guidance: &str,
    doc_text: &str,
    truncated: bool,
) -> String {
    let mut buffer = String::new();
    buffer.push_str(&format!("文件名：{}\n\n", filename));
    buffer.push_str("请根据以下字段定义从论文中提取信息：\n");

    for (idx, field) in fields.iter().enumerate() {
        buffer.push_str(&format!("{}. {}\n", idx + 1, field.name));
        if let Some(desc) = &field.description {
            buffer.push_str(&format!("   说明：{}\n", desc));
        }
        if !field.examples.is_empty() {
            buffer.push_str(&format!("   示例：{}\n", field.examples.join("；")));
        }
        if !field.allowed_values.is_empty() {
            buffer.push_str(&format!("   枚举值：{}\n", field.allowed_values.join("；")));
        }
        buffer.push('\n');
    }

    let guidance = guidance.trim();
    if !guidance.is_empty() {
        buffer.push_str("输出要求：\n");
        buffer.push_str(guidance);
        buffer.push_str("\n\n");
    }

    if truncated {
        buffer.push_str(&format!(
            "注意：正文已截断至前 {} 个字符，请结合上下文谨慎推理。\n\n",
            MAX_DOCUMENT_TEXT_CHARS
        ));
    }

    buffer.push_str("以下为论文正文内容：\n\n");
    buffer.push_str(doc_text);

    buffer
}

fn extract_object_from_response(text: &str) -> Result<Map<String, Value>> {
    let trimmed = text.trim();
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed) {
        return Ok(map);
    }

    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if end > start {
            let candidate = &trimmed[start..=end];
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(candidate) {
                return Ok(map);
            }
        }
    }

    bail!("模型输出不是可解析的 JSON 对象");
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Array(items) => items
            .iter()
            .map(value_to_string)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("；"),
        Value::Object(obj) => serde_json::to_string(obj).unwrap_or_default(),
    }
}

fn read_pdf_text(path: &Path) -> Result<String> {
    extract_pdf_text(path)
        .with_context(|| format!("无法读取 PDF 文本：{}", path.display()))
        .map(|content| content.trim().to_string())
}

async fn ensure_storage_root() -> Result<()> {
    if !Path::new(STORAGE_ROOT).exists() {
        tokio_fs::create_dir_all(STORAGE_ROOT)
            .await
            .context("无法创建信息提取存储目录")?;
    }
    Ok(())
}

fn spawn_job_worker(state: AppState, job_id: Uuid, fields: Vec<ExtractionField>) {
    tokio::spawn(async move {
        if let Err(err) = process_job(state.clone(), job_id, fields.clone()).await {
            error!(?err, %job_id, "信息提取任务失败");
            let pool = state.pool();
            if let Err(update_err) = sqlx::query(
                "UPDATE info_extract_jobs SET status = $2, status_detail = $3, error_message = $4, updated_at = NOW() WHERE id = $1",
            )
            .bind(job_id)
            .bind(STATUS_FAILED)
            .bind("任务执行出错，已终止。")
            .bind(err.to_string())
            .execute(&pool)
            .await
            {
                error!(?update_err, %job_id, "更新任务失败状态时出错");
            }
        }
    });
}

async fn process_job(state: AppState, job_id: Uuid, fields: Vec<ExtractionField>) -> Result<()> {
    let pool = state.pool();
    let settings = state.info_extract_settings().await.unwrap_or_default();

    let job_user_id: Uuid =
        sqlx::query_scalar("SELECT user_id FROM info_extract_jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .context("无法获取任务所属用户")?;

    sqlx::query(
        "UPDATE info_extract_jobs SET status = $2, status_detail = $3, updated_at = NOW() WHERE id = $1",
    )
    .bind(job_id)
    .bind(STATUS_PROCESSING)
    .bind("任务已启动，正在读取文献。")
    .execute(&pool)
    .await
    .context("无法更新任务状态")?;

    let documents = sqlx::query_as::<_, DocumentSourceRecord>(
        "SELECT id, ordinal, original_filename, source_path FROM info_extract_documents WHERE job_id = $1 ORDER BY ordinal",
    )
    .bind(job_id)
    .fetch_all(&pool)
    .await
    .context("无法读取任务文献列表")?;

    let job_dir = PathBuf::from(STORAGE_ROOT).join(job_id.to_string());

    let models = settings.models.clone();
    let prompts = settings.prompts.clone();
    let fields_arc = Arc::new(fields.clone());
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_DOCUMENTS));

    let tasks = documents
        .into_iter()
        .map(|document| {
            let state_clone = state.clone();
            let models_clone = models.clone();
            let prompts_clone = prompts.clone();
            let fields_clone = fields_arc.clone();
            let semaphore_clone = semaphore.clone();

            tokio::spawn(async move {
                process_single_document(
                    state_clone,
                    job_id,
                    document,
                    models_clone,
                    prompts_clone,
                    fields_clone,
                    semaphore_clone,
                )
                .await
            })
        })
        .collect::<Vec<_>>();

    let mut results: Vec<DocumentExtractionResult> = Vec::new();
    for handle in join_all(tasks).await {
        match handle {
            Ok(result) => results.push(result),
            Err(err) => {
                error!(?err, %job_id, "信息提取子任务异常退出");
            }
        }
    }

    results.sort_by_key(|r| r.ordinal);

    let total_tokens: i64 = results.iter().map(|r| r.tokens_used).sum();
    let success_count = results.iter().filter(|r| r.success).count();
    let total_docs = results.len();
    let failed_docs = total_docs.saturating_sub(success_count);

    let mut job_status_detail = if success_count == total_docs && total_docs > 0 {
        Some(format!("{} 篇文献已全部提取完成。", total_docs))
    } else if success_count > 0 {
        Some(format!(
            "{} 篇成功，{} 篇失败。",
            success_count, failed_docs
        ))
    } else if total_docs > 0 {
        Some("所有尝试均失败，请检查输入后重试。".to_string())
    } else {
        Some("任务执行失败，未能处理任何文献。".to_string())
    };

    let mut job_error_message: Option<String> = None;
    let mut result_path: Option<String> = None;

    if success_count > 0 {
        let result_file = job_dir.join("extraction_result.xlsx");
        if let Err(err) = write_result_workbook(&result_file, &fields, &results).await {
            error!(?err, %job_id, "生成结果表失败");
            job_error_message = Some("提取成功但结果汇总文件生成失败，请联系管理员。".to_string());
            job_status_detail = Some("部分文献完成，但结果文件生成失败。".to_string());
        } else {
            result_path = Some(result_file.to_string_lossy().to_string());
        }
    }

    let final_status = if success_count > 0 {
        if result_path.is_some() {
            STATUS_COMPLETED
        } else {
            STATUS_FAILED
        }
    } else {
        STATUS_FAILED
    };

    sqlx::query(
        "UPDATE info_extract_jobs SET status = $2, status_detail = $3, error_message = $4, result_path = $5, total_tokens = $6, usage_units = $7, updated_at = NOW() WHERE id = $1",
    )
    .bind(job_id)
    .bind(final_status)
    .bind(job_status_detail.as_deref())
    .bind(job_error_message.as_deref())
    .bind(result_path.as_deref())
    .bind(total_tokens)
    .bind(success_count as i64)
    .execute(&pool)
    .await
    .context("无法更新任务最终状态")?;

    if success_count > 0 && result_path.is_some() {
        if let Err(err) = usage::record_usage(
            &pool,
            job_user_id,
            MODULE_INFO_EXTRACT,
            total_tokens,
            success_count as i64,
        )
        .await
        {
            error!(?err, %job_id, "记录用量失败");
        }
    }

    Ok(())
}

async fn write_result_workbook(
    path: &Path,
    fields: &[ExtractionField],
    results: &[DocumentExtractionResult],
) -> Result<()> {
    let path = path.to_path_buf();
    let fields = fields.to_vec();
    let results = results.to_vec();

    task::spawn_blocking(move || generate_result_workbook(&path, &fields, &results))
        .await
        .map_err(|err| anyhow!("结果表生成线程异常：{}", err))??;

    Ok(())
}

async fn process_single_document(
    state: AppState,
    job_id: Uuid,
    document: DocumentSourceRecord,
    models: InfoExtractModels,
    prompts: InfoExtractPrompts,
    fields: Arc<Vec<ExtractionField>>,
    semaphore: Arc<Semaphore>,
) -> DocumentExtractionResult {
    let permit = match semaphore.acquire_owned().await {
        Ok(permit) => permit,
        Err(err) => {
            error!(?err, %job_id, "获取并发许可失败");
            return DocumentExtractionResult {
                ordinal: document.ordinal,
                filename: document.original_filename,
                values: None,
                error: Some("无法开始处理该文献".to_string()),
                tokens_used: 0,
                success: false,
            };
        }
    };

    let pool = state.pool();
    let llm_client = state.llm_client();

    let mut result = DocumentExtractionResult {
        ordinal: document.ordinal,
        filename: document.original_filename.clone(),
        values: None,
        error: None,
        tokens_used: 0,
        success: false,
    };

    if let Err(err) = sqlx::query(
        "UPDATE info_extract_documents SET status = $2, status_detail = $3, updated_at = NOW() WHERE id = $1",
    )
    .bind(document.id)
    .bind(STATUS_PROCESSING)
    .bind("正在提取信息…")
    .execute(&pool)
    .await
    {
        error!(?err, %job_id, document_id = %document.id, "更新文献状态失败");
        result.error = Some("无法更新文献状态".to_string());
        drop(permit);
        return result;
    }

    let pdf_path = PathBuf::from(&document.source_path);
    let text = match task::spawn_blocking({
        let path = pdf_path.clone();
        move || read_pdf_text(&path)
    })
    .await
    {
        Ok(Ok(content)) => content,
        Ok(Err(err)) => {
            error!(?err, %job_id, document_id = %document.id, "读取 PDF 失败");
            let _ = sqlx::query(
                "UPDATE info_extract_documents SET status = $2, status_detail = $3, error_message = $4, attempt_count = $5, updated_at = NOW() WHERE id = $1",
            )
            .bind(document.id)
            .bind(STATUS_FAILED)
            .bind("无法读取 PDF 内容")
            .bind(err.to_string())
            .bind(0_i32)
            .execute(&pool)
            .await;

            result.error = Some("无法读取 PDF 内容".to_string());
            drop(permit);
            return result;
        }
        Err(join_err) => {
            error!(?join_err, %job_id, document_id = %document.id, "PDF 读取线程异常");
            let _ = sqlx::query(
                "UPDATE info_extract_documents SET status = $2, status_detail = $3, error_message = $4, attempt_count = $5, updated_at = NOW() WHERE id = $1",
            )
            .bind(document.id)
            .bind(STATUS_FAILED)
            .bind("无法读取 PDF 内容")
            .bind("读取线程异常")
            .bind(0_i32)
            .execute(&pool)
            .await;

            result.error = Some("无法读取 PDF 内容".to_string());
            drop(permit);
            return result;
        }
    };

    let (clamped_text, truncated) = clamp_document_text(&text);
    let status_detail = ensure_status_detail(truncated);

    let mut attempts = 0i32;
    let mut doc_tokens = 0i64;
    let mut parsed: Option<Map<String, Value>> = None;
    let mut last_error: Option<String> = None;
    let mut last_response: Option<String> = None;

    while attempts < MAX_RETRIES as i32 {
        attempts += 1;

        let mut messages = Vec::new();
        let system_text = prompts.system_prompt.trim();
        if !system_text.is_empty() {
            messages.push(ChatMessage::new(MessageRole::System, system_text));
        }

        let user_prompt = build_user_prompt(
            &document.original_filename,
            fields.as_ref(),
            prompts.response_guidance.trim(),
            &clamped_text,
            truncated,
        );
        messages.push(ChatMessage::new(MessageRole::User, user_prompt));

        let request = LlmRequest::new(models.extraction_model.clone(), messages);

        match llm_client.execute(request).await {
            Ok(response) => {
                doc_tokens += response.token_usage.total_tokens as i64;
                last_response = Some(response.text.clone());

                match extract_object_from_response(&response.text) {
                    Ok(map) => {
                        parsed = Some(map);
                        last_error = None;
                        break;
                    }
                    Err(err) => {
                        warn!(?err, attempt = attempts, document_id = %document.id, "解析模型返回结果失败");
                        last_error = Some(err.to_string());
                    }
                }
            }
            Err(err) => {
                warn!(?err, attempt = attempts, document_id = %document.id, "模型调用失败，准备重试");
                last_error = Some(err.to_string());
            }
        }

        if attempts < MAX_RETRIES as i32 {
            sleep(Duration::from_millis(RETRY_DELAY_MS * attempts as u64)).await;
        }
    }

    result.tokens_used = doc_tokens;

    match parsed {
        Some(map) => {
            let db_value = Value::Object(map.clone());
            if let Err(err) = sqlx::query(
                "UPDATE info_extract_documents SET status = $2, status_detail = $3, response_text = $4, parsed_values = $5, error_message = NULL, attempt_count = $6, tokens_used = $7, updated_at = NOW() WHERE id = $1",
            )
            .bind(document.id)
            .bind(STATUS_COMPLETED)
            .bind(status_detail.as_deref())
            .bind(last_response.as_deref())
            .bind(db_value)
            .bind(attempts)
            .bind(doc_tokens)
            .execute(&pool)
            .await
            {
                error!(?err, %job_id, document_id = %document.id, "写入文献结果失败");
                let _ = sqlx::query(
                    "UPDATE info_extract_documents SET status = $2, status_detail = $3, error_message = $4, updated_at = NOW() WHERE id = $1",
                )
                .bind(document.id)
                .bind(STATUS_FAILED)
                .bind("结果写入数据库失败")
                .bind(err.to_string())
                .execute(&pool)
                .await;

                result.error = Some("结果写入数据库失败".to_string());
            } else {
                result.success = true;
                result.values = Some(map);
            }
        }
        None => {
            let error_message =
                last_error.unwrap_or_else(|| "模型多次尝试仍未返回有效结果".to_string());
            if let Err(err) = sqlx::query(
                "UPDATE info_extract_documents SET status = $2, status_detail = $3, error_message = $4, response_text = $5, parsed_values = NULL, attempt_count = $6, tokens_used = $7, updated_at = NOW() WHERE id = $1",
            )
            .bind(document.id)
            .bind(STATUS_FAILED)
            .bind(status_detail.as_deref())
            .bind(&error_message)
            .bind(last_response.as_deref())
            .bind(attempts)
            .bind(doc_tokens)
            .execute(&pool)
            .await
            {
                error!(?err, %job_id, document_id = %document.id, "写入失败状态时出错");
            }
            result.error = Some(error_message);
        }
    }

    drop(permit);
    result
}

fn generate_result_workbook(
    path: &Path,
    fields: &[ExtractionField],
    results: &[DocumentExtractionResult],
) -> Result<()> {
    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();

    worksheet
        .write_string(0, 0, "文件名")
        .context("写入表头失败")?;
    for (idx, field) in fields.iter().enumerate() {
        let col: u16 = (idx + 1)
            .try_into()
            .map_err(|_| anyhow!("字段数量过多，超出 Excel 列限制"))?;
        worksheet
            .write_string(0, col, &field.name)
            .context("写入字段表头失败")?;
    }
    let error_col: u16 = (fields.len() + 1)
        .try_into()
        .map_err(|_| anyhow!("字段数量过多，超出 Excel 列限制"))?;
    worksheet
        .write_string(0, error_col, "错误信息")
        .context("写入错误信息表头失败")?;

    for (row_idx, result) in results.iter().enumerate() {
        let row = (row_idx + 1) as u32;
        worksheet
            .write_string(row, 0, &result.filename)
            .context("写入文件名失败")?;

        for (col_idx, field) in fields.iter().enumerate() {
            let col: u16 = (col_idx + 1)
                .try_into()
                .map_err(|_| anyhow!("字段数量过多，超出 Excel 列限制"))?;
            let value = result
                .values
                .as_ref()
                .and_then(|map| map.get(&field.name))
                .map(value_to_string)
                .unwrap_or_default();
            worksheet
                .write_string(row, col, &value)
                .context("写入字段值失败")?;
        }

        let error_text = result.error.clone().unwrap_or_default();
        worksheet
            .write_string(row, error_col, &error_text)
            .context("写入错误信息失败")?;
    }

    workbook.save(path).context("保存结果工作簿失败")?;

    Ok(())
}

fn internal_error(err: anyhow::Error) -> (StatusCode, Json<ApiError>) {
    error!(?err, "信息提取模块内部错误");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError::new("服务器内部错误，请稍后再试。")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_spec_succeeds_with_examples() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("spec.xlsx");

        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet();
        worksheet.write_string(0, 0, "Location").unwrap();
        worksheet.write_string(1, 0, "城市或国家名称").unwrap();
        worksheet.write_string(2, 0, "上海; 北京").unwrap();
        worksheet.write_string(0, 1, "Sample Size").unwrap();
        worksheet.write_string(3, 1, "100; 250; 1000").unwrap();
        workbook.save(&path).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let fields = parse_extraction_spec(&bytes).unwrap();

        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "Location");
        assert_eq!(fields[0].examples, vec!["上海", "北京"]);
        assert_eq!(fields[1].allowed_values, vec!["100", "250", "1000"]);
    }

    #[test]
    fn parse_spec_rejects_empty_definition() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("spec.xlsx");

        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet();
        worksheet.write_string(0, 0, "Location").unwrap();
        workbook.save(&path).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let err = parse_extraction_spec(&bytes).unwrap_err();
        assert!(format!("{err}").contains("至少需要填写"));
    }

    #[test]
    fn extract_object_handles_wrapped_text() {
        let payload =
            "生成如下：\n```json\n{\n  \"Location\": \"Shanghai\",\n  \"Sample Size\": 120\n}\n```";
        let map = extract_object_from_response(payload).unwrap();
        assert_eq!(
            map.get("Location").unwrap(),
            &Value::String("Shanghai".into())
        );
    }
}
