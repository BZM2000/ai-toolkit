use std::{
    borrow::Cow,
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{Multipart, Path as AxumPath, State},
    http::StatusCode,
    response::{Html, Redirect},
    routing::{get, post},
};
use axum_extra::extract::cookie::CookieJar;
use pdf_extract::extract_text as extract_pdf_text;
use quick_xml::{Reader as XmlReader, events::Event};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::{fs as tokio_fs, time::sleep};
use tracing::error;
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
    AppState, JournalReferenceRow, JournalTopicRow, JournalTopicScoreRow, escape_html,
    fetch_journal_references, fetch_journal_topic_scores, fetch_journal_topics, history,
    llm::{ChatMessage, LlmClient, LlmRequest, MessageRole},
    render_footer,
    usage::{self, MODULE_GRADER},
    web::auth::{self, JsonAuthError},
};

const STORAGE_ROOT: &str = "storage/grader";
const STATUS_PENDING: &str = "pending";
const STATUS_PROCESSING: &str = "processing";
const STATUS_COMPLETED: &str = "completed";
const STATUS_FAILED: &str = "failed";

const MAX_ATTEMPTS: usize = 30;
const TARGET_SUCCESSES: usize = 12;
const MIN_SUCCESSES: usize = 8;
const RATE_LIMIT_DELAY: Duration = Duration::from_millis(500);
const DOCX_PENALTY: f64 = 0.02;
const MAX_RECOMMENDATIONS: usize = 12;
const WEIGHTS: [f64; 6] = [4.0, 2.0, 1.0, 1.0, 1.0, 1.0];

const MATCH_SCORE_RULES: &[(i16, Option<f64>)] = &[
    (6, Some(0.90)),
    (5, Some(0.95)),
    (4, Some(1.00)),
    (3, Some(1.05)),
    (2, None),
    (1, None),
    (0, None),
];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tools/grader", get(grader_page))
        .route("/tools/grader/jobs", post(create_job))
        .route("/api/grader/jobs/:id", get(job_status))
        .route("/dashboard/modules/grader", get(admin::settings_page))
        .route("/dashboard/modules/grader/models", post(admin::save_models))
        .route(
            "/dashboard/modules/grader/prompts",
            post(admin::save_prompts),
        )
}

#[derive(sqlx::FromRow)]
struct JobProcessingRecord {
    user_id: Uuid,
    status: String,
}

#[derive(sqlx::FromRow, Clone)]
struct DocumentProcessingRecord {
    id: Uuid,
    source_path: String,
    is_docx: bool,
}

#[derive(sqlx::FromRow)]
struct JobStatusRow {
    user_id: Uuid,
    status: String,
    status_detail: Option<String>,
    error_message: Option<String>,
    attempts_run: Option<i32>,
    valid_runs: Option<i32>,
    iqm_score: Option<f64>,
    justification: Option<String>,
    decision_reason: Option<String>,
    keyword_main: Option<String>,
    keyword_peripherals: Option<Vec<String>>,
    recommendations: Option<Value>,
}

#[derive(sqlx::FromRow)]
struct JobDocumentStatusRow {
    original_filename: String,
    status: String,
    status_detail: Option<String>,
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

#[derive(Serialize)]
struct JobStatusResponse {
    job_id: Uuid,
    status: String,
    status_detail: Option<String>,
    error_message: Option<String>,
    attempts_run: Option<i32>,
    valid_runs: Option<i32>,
    iqm_score: Option<f64>,
    justification: Option<String>,
    decision_reason: Option<String>,
    keyword_main: Option<String>,
    keyword_peripherals: Vec<String>,
    recommendations: Vec<RecommendationDto>,
    document: JobDocumentStatus,
}

#[derive(Serialize)]
struct JobDocumentStatus {
    original_filename: String,
    status: String,
    status_detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRecommendation {
    journal_id: Uuid,
    journal_name: String,
    reference_mark: Option<String>,
    low_bound: f64,
    adjusted_threshold: f64,
    match_score: f64,
}

#[derive(Serialize)]
struct RecommendationDto {
    journal_name: String,
    reference_mark: Option<String>,
    adjusted_threshold: f64,
    match_score: f64,
    low_bound: f64,
}

#[derive(Clone)]
struct KeywordSummary {
    main: Option<String>,
    peripheral: Vec<String>,
}

struct GradingOutcome {
    per_level: [f64; 6],
    iqm_score: f64,
    attempts_run: usize,
    valid_runs: usize,
    justification: Option<String>,
    decision_reason: String,
}

#[derive(Deserialize)]
struct GradingResponsePayload {
    #[serde(rename = "Level 1")]
    level1: f64,
    #[serde(rename = "Level 2")]
    level2: f64,
    #[serde(rename = "Level 3")]
    level3: f64,
    #[serde(rename = "Level 4")]
    level4: f64,
    #[serde(rename = "Level 5")]
    level5: f64,
    #[serde(rename = "Level 6")]
    level6: f64,
    justification: Option<String>,
}

#[derive(Deserialize)]
struct KeywordResponsePayload {
    main_keyword: Option<String>,
    #[serde(default)]
    peripheral_keywords: Vec<String>,
}

pub async fn grader_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Html<String>, Redirect> {
    let user = auth::require_user_redirect(&state, &jar).await?;
    let username = escape_html(&user.username);
    let note_html = format!(
        "当前登录：<strong>{username}</strong>。上传 PDF、DOCX 或 TXT 稿件，系统会估计投稿水平并推荐期刊。",
        username = username,
    );
    let admin_link = if user.is_admin {
        Some(ToolAdminLink {
            href: "/dashboard/modules/grader",
            label: "模块管理",
        })
    } else {
        None
    };
    let upload_widget = render_upload_widget(
        &UploadWidgetConfig::new("grader-upload", "grader-file", "file", "稿件文件")
            .with_description("支持上传 PDF、DOCX 或 TXT 稿件。")
            .with_note("仅支持单个 PDF / DOCX / TXT 文件。")
            .with_accept(".pdf,.docx,.txt"),
    );
    let history_panel = history_ui::render_history_panel(MODULE_GRADER);
    let extra_styles = Cow::Borrowed(
        r#"        .results { background: #ffffff; border-radius: 12px; border: 1px solid #e2e8f0; padding: 1.5rem; box-shadow: 0 10px 30px rgba(15, 23, 42, 0.06); }
        .results h3 { margin-top: 0; }
"#,
    );
    let new_tab_html = format!(
        r#"                <section class="panel">
                    <h2>提交稿件</h2>
                    <form id="grader-form">
                        {upload_widget}
                        <button type="submit">开始评估</button>
                    </form>
                    <div id="status-box" class="status-box">等待上传。</div>
                </section>
                <section id="results-section" class="results" style="display:none;">
                    <h2>评估结果</h2>
                    <div id="score-summary"></div>
                    <div id="keyword-summary"></div>
                    <div id="recommendations"></div>
                </section>
"#,
        upload_widget = upload_widget,
    );

    let grader_script = r#"const form = document.getElementById('grader-form');
const fileInput = document.getElementById('grader-file');
const statusBox = document.getElementById('status-box');
const resultsSection = document.getElementById('results-section');
const scoreSummary = document.getElementById('score-summary');
const keywordSummary = document.getElementById('keyword-summary');
const recommendationsBox = document.getElementById('recommendations');

let pollTimer = null;

const resetResults = () => {
    resultsSection.style.display = 'none';
    scoreSummary.innerHTML = '';
    keywordSummary.innerHTML = '';
    recommendationsBox.innerHTML = '';
};

const renderRecommendations = (items) => {
    if (!items || items.length === 0) {
        recommendationsBox.innerHTML = '<p class="note">暂无匹配的期刊推荐。</p>';
        return;
    }
    const rows = items.map((item) => {
        const mark = item.reference_mark ? item.reference_mark : '—';
        return `<tr><td>${item.journal_name}</td><td>${mark}</td><td>${item.match_score.toFixed(1)}` +
               `</td><td>${item.adjusted_threshold.toFixed(2)}</td><td>${item.low_bound.toFixed(2)}</td></tr>`;
    }).join('');
    recommendationsBox.innerHTML = `
        <h3>期刊推荐</h3>
        <table>
            <thead><tr><th>期刊</th><th>参考标记</th><th>匹配得分</th><th>调整后阈值</th><th>原始阈值</th></tr></thead>
            <tbody>${rows}</tbody>
        </table>`;
};

const renderKeywords = (main, peripherals) => {
    const mainText = main ? `<strong>主要主题：</strong> ${main}` : '<strong>主要主题：</strong> 未识别';
    const peripheralText = peripherals && peripherals.length > 0 ? peripherals.join('，') : '无';
    keywordSummary.innerHTML = `
        <h3>主题分析</h3>
        <p>${mainText}</p>
        <p><strong>相关主题：</strong> ${peripheralText}</p>
    `;
};

const renderScore = (data) => {
    if (typeof data.iqm_score !== 'number') {
        scoreSummary.innerHTML = '<p class="note">尚未产生评分。</p>';
        return;
    }
    const attempts = data.attempts_run ?? 0;
    const valid = data.valid_runs ?? 0;
    const justification = data.justification ? `<p><strong>模型说明：</strong> ${data.justification}</p>` : '';
    const decision = data.decision_reason ? `<p class="note">${data.decision_reason}</p>` : '';
    scoreSummary.innerHTML = `
        <h3>综合评分</h3>
        <p><strong>IQM 评分：</strong> ${data.iqm_score.toFixed(1)}</p>
        <p class="note">有效结果 ${valid} 次，共尝试 ${attempts} 次。</p>
        ${justification}
        ${decision}
    `;
};

const updateStatus = (payload) => {
    statusBox.textContent = payload;
};

const handleStatusPayload = (payload) => {
    updateStatus(payload.status_detail || `当前状态：${payload.status}`);

    if (payload.status === 'completed') {
        renderScore(payload);
        renderKeywords(payload.keyword_main, payload.keyword_peripherals);
        renderRecommendations(payload.recommendations);
        resultsSection.style.display = 'block';
        if (pollTimer) {
            clearInterval(pollTimer);
            pollTimer = null;
        }
    } else if (payload.status === 'failed') {
        const message = payload.error_message || '评估失败，请稍后重试。';
        statusBox.textContent = message;
        if (pollTimer) {
            clearInterval(pollTimer);
            pollTimer = null;
        }
    }
};

const pollJob = (url) => {
    pollTimer = setInterval(async () => {
        try {
            const res = await fetch(url, { headers: { 'Accept': 'application/json' } });
            if (!res.ok) {
                throw new Error('状态查询失败');
            }
            const data = await res.json();
            handleStatusPayload(data);
            if (data.status === 'completed' || data.status === 'failed') {
                clearInterval(pollTimer);
                pollTimer = null;
            }
        } catch (err) {
            clearInterval(pollTimer);
            pollTimer = null;
            updateStatus('轮询失败：' + err.message);
        }
    }, 3000);
};

const handleFileSelection = () => {
    if (!fileInput || fileInput.files.length === 0) {
        updateStatus('等待上传。');
        return;
    }
    updateStatus(`已选择文件：${fileInput.files[0].name}`);
};

if (fileInput) {
    fileInput.addEventListener('change', handleFileSelection);
}

form.addEventListener('submit', async (event) => {
    event.preventDefault();
    if (!fileInput.files || fileInput.files.length === 0) {
        updateStatus('请先选择文件。');
        return;
    }
    resetResults();
    updateStatus('正在上传稿件...');
    const formData = new FormData(form);

    try {
        const res = await fetch('/tools/grader/jobs', { method: 'POST', body: formData });
        if (!res.ok) {
            const errorBody = await res.json().catch(() => ({ message: '提交失败' }));
            updateStatus(errorBody.message || '提交失败');
            return;
        }
        const data = await res.json();
        handleStatusPayload(data);
        if (fileInput) {
            fileInput.value = '';
            fileInput.dispatchEvent(new Event('change'));
        }
        if (data.status_url) {
            pollJob(data.status_url);
        }
    } catch (err) {
        updateStatus('提交失败：' + err.message);
    }
});
"#;

    let html = render_tool_page(ToolPageLayout {
        meta_title: "稿件评估与期刊推荐 | 张圆教授课题组 AI 工具箱",
        page_heading: "稿件评估与期刊推荐",
        username: &username,
        note_html: Cow::Owned(note_html),
        tab_group: "grader",
        new_tab_label: "新任务",
        new_tab_html: Cow::Owned(new_tab_html),
        history_tab_label: "历史记录",
        history_panel_html: Cow::Owned(history_panel),
        admin_link,
        footer_html: Cow::Owned(render_footer()),
        extra_style_blocks: vec![
            Cow::Borrowed(history_ui::HISTORY_STYLES),
            Cow::Borrowed(UPLOAD_WIDGET_STYLES),
            extra_styles,
        ],
        body_scripts: vec![
            Cow::Borrowed(UPLOAD_WIDGET_SCRIPT),
            Cow::Owned(format!(
                "<script>
{}
</script>",
                grader_script
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
) -> Result<Json<JobSubmissionResponse>, (StatusCode, Json<ApiError>)> {
    let user = auth::current_user_or_json_error(&state, &jar)
        .await
        .map_err(|JsonAuthError { status, message }| (status, Json(ApiError::new(message))))?;

    let pool = state.pool();

    if let Err(err) = usage::ensure_within_limits(&pool, user.id, MODULE_GRADER, 1).await {
        return Err((StatusCode::FORBIDDEN, Json(ApiError::new(err.message()))));
    }

    ensure_storage_root()
        .await
        .map_err(|err| internal_error(err.into()))?;

    let job_id = Uuid::new_v4();
    let doc_id = Uuid::new_v4();
    let job_dir = PathBuf::from(STORAGE_ROOT).join(job_id.to_string());

    let file_config = FileFieldConfig::new(
        "file",
        &["pdf", "docx", "txt"],
        1,
        FileNaming::PrefixOnly { prefix: "source_" },
    )
    .with_min_files(1);

    let upload = match process_upload_form(multipart, &job_dir, &[file_config]).await {
        Ok(outcome) => outcome,
        Err(err) => {
            let _ = tokio_fs::remove_dir_all(&job_dir).await;
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiError::new(err.message().to_string())),
            ));
        }
    };

    let files: Vec<_> = upload.files_for("file").cloned().collect();
    let file = files
        .first()
        .expect("file upload guaranteed by process_upload_form");

    let is_docx = file
        .original_name
        .rsplit('.')
        .next()
        .map(|ext| ext.eq_ignore_ascii_case("docx"))
        .unwrap_or(false);

    let mut transaction = pool
        .begin()
        .await
        .map_err(|err| internal_error(err.into()))?;

    sqlx::query("INSERT INTO grader_jobs (id, user_id, status) VALUES ($1, $2, $3)")
        .bind(job_id)
        .bind(user.id)
        .bind(STATUS_PENDING)
        .execute(&mut *transaction)
        .await
        .map_err(|err| internal_error(err.into()))?;

    sqlx::query(
        "INSERT INTO grader_documents (id, job_id, original_filename, source_path, is_docx, status) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(doc_id)
    .bind(job_id)
    .bind(&file.original_name)
    .bind(file.stored_path.to_string_lossy().to_string())
    .bind(is_docx)
    .bind(STATUS_PENDING)
    .execute(&mut *transaction)
    .await
    .map_err(|err| internal_error(err.into()))?;

    transaction
        .commit()
        .await
        .map_err(|err| internal_error(err.into()))?;

    if let Err(err) =
        history::record_job_start(&pool, MODULE_GRADER, user.id, job_id.to_string()).await
    {
        error!(?err, %job_id, "failed to record grader job history");
    }

    spawn_job_worker(state.clone(), job_id);

    Ok(Json(JobSubmissionResponse {
        job_id,
        status_url: format!("/api/grader/jobs/{}", job_id),
    }))
}

async fn job_status(
    State(state): State<AppState>,
    jar: CookieJar,
    AxumPath(job_id): AxumPath<Uuid>,
) -> Result<Json<JobStatusResponse>, (StatusCode, Json<ApiError>)> {
    let user = auth::current_user_or_json_error(&state, &jar)
        .await
        .map_err(|JsonAuthError { status, message }| (status, Json(ApiError::new(message))))?;

    let pool = state.pool();

    let job = sqlx::query_as::<_, JobStatusRow>(
        "SELECT id, user_id, status, status_detail, error_message, attempts_run, valid_runs, iqm_score, justification, decision_reason, keyword_main, keyword_peripherals, recommendations FROM grader_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_optional(&pool)
    .await
    .map_err(|err| internal_error(err.into()))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, Json(ApiError::new("未找到任务。"))))?;

    if job.user_id != user.id && !user.is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError::new("无权查看该任务。")),
        ));
    }

    let document = sqlx::query_as::<_, JobDocumentStatusRow>(
        "SELECT original_filename, status, status_detail FROM grader_documents WHERE job_id = $1 LIMIT 1",
    )
    .bind(job_id)
    .fetch_optional(&pool)
    .await
    .map_err(|err| internal_error(err.into()))?
    .unwrap_or(JobDocumentStatusRow {
        original_filename: "稿件".to_string(),
        status: STATUS_PENDING.to_string(),
        status_detail: None,
    });

    let recommendations = job
        .recommendations
        .as_ref()
        .and_then(|value| serde_json::from_value::<Vec<StoredRecommendation>>(value.clone()).ok())
        .unwrap_or_default();

    let recommendation_dtos = recommendations
        .into_iter()
        .map(|item| RecommendationDto {
            journal_name: item.journal_name,
            reference_mark: item.reference_mark,
            adjusted_threshold: item.adjusted_threshold,
            match_score: item.match_score,
            low_bound: item.low_bound,
        })
        .collect();

    let response = JobStatusResponse {
        job_id,
        status: job.status,
        status_detail: job.status_detail,
        error_message: job.error_message,
        attempts_run: job.attempts_run,
        valid_runs: job.valid_runs,
        iqm_score: job.iqm_score,
        justification: job.justification,
        decision_reason: job.decision_reason,
        keyword_main: job.keyword_main,
        keyword_peripherals: job.keyword_peripherals.unwrap_or_default(),
        recommendations: recommendation_dtos,
        document: JobDocumentStatus {
            original_filename: document.original_filename,
            status: document.status,
            status_detail: document.status_detail,
        },
    };

    Ok(Json(response))
}

fn spawn_job_worker(state: AppState, job_id: Uuid) {
    tokio::spawn(async move {
        if let Err(err) = process_job(state, job_id).await {
            error!(?err, %job_id, "grader job failed");
        }
    });
}

async fn process_job(state: AppState, job_id: Uuid) -> Result<()> {
    let pool = state.pool();

    let job = sqlx::query_as::<_, JobProcessingRecord>(
        "SELECT id, user_id, status FROM grader_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_one(&pool)
    .await
    .context("failed to load grader job")?;

    if job.status != STATUS_PENDING {
        return Ok(());
    }

    update_job_status(
        &pool,
        job_id,
        STATUS_PROCESSING,
        Some("正在提取稿件文本..."),
    )
    .await?;

    let doc = sqlx::query_as::<_, DocumentProcessingRecord>(
        "SELECT id, original_filename, source_path, is_docx FROM grader_documents WHERE job_id = $1",
    )
    .bind(job_id)
    .fetch_one(&pool)
    .await
    .context("failed to load grader document")?;

    update_document_status(
        &pool,
        doc.id,
        STATUS_PROCESSING,
        Some("正在读取稿件..."),
        None,
    )
    .await?;

    let source_path = Path::new(&doc.source_path);
    let text = read_document_text(source_path).map_err(|err| anyhow!(err))?;
    let text = text.trim().to_string();

    update_document_status(
        &pool,
        doc.id,
        STATUS_PROCESSING,
        Some(&format!("已提取文本，长度 {} 字符。", text.len())),
        Some(text.len() as i32),
    )
    .await?;

    if text.is_empty() {
        mark_job_failed(&pool, job_id, doc.id, "未能读取到稿件内容，请检查文件。").await?;
        return Ok(());
    }

    let Some(settings) = state.grader_settings().await else {
        mark_job_failed(&pool, job_id, doc.id, "未配置稿件评估设置，请联系管理员。").await?;
        return Ok(());
    };
    let models = settings.models.clone();
    let prompts = settings.prompts.clone();

    let llm = state.llm_client();

    let (grading_outcome, grading_tokens) = run_grading_sequence(
        &pool,
        job_id,
        &llm,
        models.grading_model.as_str(),
        &prompts.grading_instructions,
        &text,
    )
    .await?;

    let mut outcome = match grading_outcome {
        Some(outcome) => outcome,
        None => {
            mark_job_failed(
                &pool,
                job_id,
                doc.id,
                "模型未返回足够的有效结果，请稍后重试。",
            )
            .await?;
            return Ok(());
        }
    };

    update_job_attempts(
        &pool,
        job_id,
        outcome.attempts_run,
        outcome.valid_runs,
        Some("正在分析主题并匹配期刊..."),
    )
    .await?;

    let topics = fetch_journal_topics(&pool).await.unwrap_or_default();
    let references = fetch_journal_references(&pool).await.unwrap_or_default();
    let scores = fetch_journal_topic_scores(&pool).await.unwrap_or_default();
    let score_map = build_score_map(&references, &scores);

    let (keyword_summary, keyword_tokens) = run_keyword_selection(
        &llm,
        models.keyword_model.as_str(),
        &prompts.keyword_selection,
        &topics,
        &text,
    )
    .await
    .unwrap_or_else(|err| {
        error!(?err, %job_id, "keyword selection failed");
        (
            KeywordSummary {
                main: None,
                peripheral: Vec::new(),
            },
            0,
        )
    });

    if doc.is_docx {
        apply_docx_penalty(&mut outcome);
    }

    let recommendations = build_recommendations(
        &references,
        &score_map,
        &topics,
        &keyword_summary,
        outcome.iqm_score,
    );

    let recommendation_json = serde_json::to_value(&recommendations).unwrap_or(json!([]));

    let total_tokens = grading_tokens + keyword_tokens;

    let peripherals = if keyword_summary.peripheral.is_empty() {
        None
    } else {
        Some(keyword_summary.peripheral.clone())
    };

    if let Err(err) = usage::record_usage(&pool, job.user_id, MODULE_GRADER, total_tokens, 1).await
    {
        error!(?err, "failed to record grader usage");
    }

    sqlx::query(
        "UPDATE grader_jobs SET status = $2, status_detail = $3, error_message = NULL, attempts_run = $4, valid_runs = $5, iqm_score = $6, justification = $7, decision_reason = $8, keyword_main = $9, keyword_peripherals = $10, recommendations = $11, usage_delta = 1, updated_at = NOW() WHERE id = $1",
    )
    .bind(job_id)
    .bind(STATUS_COMPLETED)
    .bind("评估完成。")
    .bind(outcome.attempts_run as i32)
    .bind(outcome.valid_runs as i32)
    .bind(outcome.iqm_score)
    .bind(outcome.justification)
    .bind(outcome.decision_reason)
    .bind(keyword_summary.main)
    .bind(peripherals.as_ref())
    .bind(recommendation_json)
    .execute(&pool)
    .await
    .context("failed to finalize grader job")?;

    update_document_status(
        &pool,
        doc.id,
        STATUS_COMPLETED,
        Some("评估完成。"),
        Some(text.len() as i32),
    )
    .await?;

    Ok(())
}

async fn run_grading_sequence(
    pool: &PgPool,
    job_id: Uuid,
    llm: &LlmClient,
    model: &str,
    system_prompt: &str,
    manuscript: &str,
) -> Result<(Option<GradingOutcome>, i64)> {
    let mut attempts_run = 0usize;
    let mut valid_scores: Vec<[f64; 6]> = Vec::new();
    let mut justifications: Vec<String> = Vec::new();
    let mut token_total: i64 = 0;

    while attempts_run < MAX_ATTEMPTS && valid_scores.len() < TARGET_SUCCESSES {
        attempts_run += 1;

        if attempts_run > 1 {
            sleep(RATE_LIMIT_DELAY).await;
        }

        let request = build_grading_request(model, system_prompt, manuscript);

        match llm.execute(request).await {
            Ok(response) => {
                token_total += response.token_usage.total_tokens as i64;
                match parse_grading_response(&response.text) {
                    Ok(payload) => {
                        let mut values = payload_to_array(&payload);
                        normalize_scores(&mut values);
                        if is_non_decreasing(&values) {
                            valid_scores.push(values);
                            if let Some(justification) = payload.justification {
                                justifications.push(justification);
                            }
                        }
                    }
                    Err(err) => {
                        error!(?err, "failed to parse grading response");
                    }
                }
            }
            Err(err) => {
                error!(?err, "grader LLM call failed");
            }
        }

        update_job_attempts(
            pool,
            job_id,
            attempts_run,
            valid_scores.len(),
            Some(&format!(
                "正在收集模型结果：已获得 {} 次有效结果（共尝试 {} 次）。",
                valid_scores.len(),
                attempts_run
            )),
        )
        .await?;
    }

    if valid_scores.len() < MIN_SUCCESSES {
        return Ok((None, token_total));
    }

    let weighted_scores: Vec<f64> = valid_scores
        .iter()
        .map(|scores| weighted_mean(scores))
        .collect();

    let (iqm, kept_indices) = interquartile_mean(&weighted_scores);
    let kept_runs: Vec<&[f64; 6]> = if kept_indices.is_empty() {
        valid_scores.iter().collect()
    } else {
        kept_indices.iter().map(|&idx| &valid_scores[idx]).collect()
    };

    let mut per_level = [0.0; 6];
    if !kept_runs.is_empty() {
        for idx in 0..6 {
            let sum: f64 = kept_runs.iter().map(|run| run[idx]).sum();
            per_level[idx] = sum / kept_runs.len() as f64;
        }
    }

    let decision_reason = format!(
        "基于 {} 次有效结果的加权评分，取其中 {} 次的四分位平均值。",
        valid_scores.len(),
        kept_runs.len()
    );

    let justification = justifications.into_iter().next();

    Ok((
        Some(GradingOutcome {
            per_level,
            iqm_score: iqm,
            attempts_run,
            valid_runs: valid_scores.len(),
            justification,
            decision_reason,
        }),
        token_total,
    ))
}

fn build_grading_request(model: &str, system_prompt: &str, manuscript: &str) -> LlmRequest {
    LlmRequest::new(
        model.to_string(),
        vec![
            ChatMessage::new(MessageRole::System, system_prompt.to_string()),
            ChatMessage::new(
                MessageRole::User,
                format!("Manuscript to grade:\n\n{}", manuscript),
            ),
        ],
    )
}

fn parse_grading_response(payload: &str) -> Result<GradingResponsePayload> {
    serde_json::from_str::<GradingResponsePayload>(payload)
        .map_err(|err| anyhow!("invalid grading JSON: {}", err))
}

fn payload_to_array(payload: &GradingResponsePayload) -> [f64; 6] {
    [
        payload.level1,
        payload.level2,
        payload.level3,
        payload.level4,
        payload.level5,
        payload.level6,
    ]
}

fn normalize_scores(scores: &mut [f64; 6]) {
    for value in scores.iter_mut() {
        if !value.is_finite() || *value < 0.0 {
            *value = 0.0;
        } else if *value > 100.0 {
            *value = 100.0;
        }
    }
}

fn is_non_decreasing(values: &[f64; 6]) -> bool {
    values
        .windows(2)
        .all(|window| window[0] <= window[1] + f64::EPSILON)
}

fn weighted_mean(scores: &[f64; 6]) -> f64 {
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for (score, weight) in scores.iter().zip(WEIGHTS.iter()) {
        numerator += score * weight;
        denominator += weight;
    }
    if denominator == 0.0 {
        0.0
    } else {
        numerator / denominator
    }
}

fn interquartile_mean(values: &[f64]) -> (f64, Vec<usize>) {
    if values.is_empty() {
        return (0.0, Vec::new());
    }
    let mut indices: Vec<usize> = (0..values.len()).collect();
    indices.sort_by(|&a, &b| values[a].partial_cmp(&values[b]).unwrap());
    let k = (values.len() + 3) / 4;
    let kept = if values.len() > 2 * k {
        indices[k..values.len() - k].to_vec()
    } else {
        indices
    };

    if kept.is_empty() {
        return (0.0, Vec::new());
    }

    let sum: f64 = kept.iter().map(|&idx| values[idx]).sum();
    (sum / kept.len() as f64, kept)
}

async fn run_keyword_selection(
    llm: &LlmClient,
    model: &str,
    prompt_template: &str,
    topics: &[JournalTopicRow],
    manuscript: &str,
) -> Result<(KeywordSummary, i64)> {
    if topics.is_empty() {
        return Ok((
            KeywordSummary {
                main: None,
                peripheral: Vec::new(),
            },
            0,
        ));
    }

    let keywords_list = topics
        .iter()
        .map(|topic| topic.name.trim())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ");

    let prompt = prompt_template.replace("{{KEYWORDS}}", &keywords_list);
    let excerpt = if manuscript.len() > 10_000 {
        &manuscript[..10_000]
    } else {
        manuscript
    };

    let request = LlmRequest::new(
        model.to_string(),
        vec![
            ChatMessage::new(MessageRole::System, prompt),
            ChatMessage::new(
                MessageRole::User,
                format!("稿件内容（前 10000 字符）：\n\n{}", excerpt),
            ),
        ],
    );

    let response = llm
        .execute(request)
        .await
        .map_err(|err| anyhow!("keyword selection call failed: {}", err))?;

    let token_total = response.token_usage.total_tokens as i64;

    let payload: KeywordResponsePayload = serde_json::from_str(&response.text)
        .map_err(|err| anyhow!("invalid keyword JSON: {}", err))?;

    let mut main = payload.main_keyword.map(|kw| kw.trim().to_string());
    if let Some(ref mut kw) = main {
        if kw.is_empty() {
            main = None;
        }
    }

    let mut peripheral = Vec::new();
    for keyword in payload.peripheral_keywords {
        let trimmed = keyword.trim();
        if trimmed.is_empty() {
            continue;
        }
        if main
            .as_deref()
            .map(|m| m.eq_ignore_ascii_case(trimmed))
            .unwrap_or(false)
        {
            continue;
        }
        if peripheral
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(trimmed))
        {
            continue;
        }
        peripheral.push(trimmed.to_string());
    }

    Ok((KeywordSummary { main, peripheral }, token_total))
}

fn build_score_map(
    references: &[JournalReferenceRow],
    scores: &[JournalTopicScoreRow],
) -> HashMap<Uuid, HashMap<Uuid, i16>> {
    let mut map: HashMap<Uuid, HashMap<Uuid, i16>> = HashMap::new();
    let valid_journal_ids: std::collections::HashSet<Uuid> =
        references.iter().map(|row| row.id).collect();

    for score in scores {
        if !valid_journal_ids.contains(&score.journal_id) {
            continue;
        }
        map.entry(score.journal_id)
            .or_default()
            .insert(score.topic_id, score.score);
    }

    map
}

fn build_recommendations(
    references: &[JournalReferenceRow],
    score_map: &HashMap<Uuid, HashMap<Uuid, i16>>,
    topics: &[JournalTopicRow],
    summary: &KeywordSummary,
    overall_score: f64,
) -> Vec<StoredRecommendation> {
    if references.is_empty() {
        return Vec::new();
    }

    let mut name_lookup: HashMap<String, Uuid> = HashMap::new();
    for topic in topics {
        name_lookup.insert(topic.name.to_lowercase(), topic.id);
    }

    let mut weights: HashMap<Uuid, i16> = HashMap::new();
    if let Some(ref main) = summary.main {
        if let Some(topic_id) = name_lookup.get(&main.to_lowercase()) {
            weights.insert(*topic_id, 2);
        }
    }
    for keyword in &summary.peripheral {
        if let Some(topic_id) = name_lookup.get(&keyword.to_lowercase()) {
            weights.entry(*topic_id).or_insert(1);
        }
    }

    let mut results = Vec::new();

    for reference in references {
        let topic_scores = score_map.get(&reference.id).cloned().unwrap_or_default();
        let mut match_score: i16 = 0;
        for (topic_id, journal_score) in topic_scores {
            let weight = *weights.get(&topic_id).unwrap_or(&0);
            match_score += weight * journal_score;
        }

        let adjusted = adjust_lower_bound(reference.low_bound, match_score);
        let Some(adjusted_threshold) = adjusted else {
            continue;
        };

        if overall_score < adjusted_threshold {
            continue;
        }

        results.push(StoredRecommendation {
            journal_id: reference.id,
            journal_name: reference.journal_name.clone(),
            reference_mark: reference.reference_mark.clone(),
            low_bound: reference.low_bound,
            adjusted_threshold,
            match_score: match_score as f64,
        });
    }

    results.sort_by(|a, b| {
        a.adjusted_threshold
            .partial_cmp(&b.adjusted_threshold)
            .unwrap()
    });
    if results.len() > MAX_RECOMMENDATIONS {
        results = results.split_off(results.len() - MAX_RECOMMENDATIONS);
    }
    results
}

fn adjust_lower_bound(base: f64, score: i16) -> Option<f64> {
    for (threshold, multiplier) in MATCH_SCORE_RULES {
        if score >= *threshold {
            return multiplier.map(|m| base * m);
        }
    }
    None
}

fn apply_docx_penalty(outcome: &mut GradingOutcome) {
    outcome.iqm_score *= 1.0 - DOCX_PENALTY;
    for value in outcome.per_level.iter_mut() {
        *value *= 1.0 - DOCX_PENALTY;
    }
}

async fn update_job_status(
    pool: &PgPool,
    job_id: Uuid,
    status: &str,
    detail: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE grader_jobs SET status = $2, status_detail = $3, updated_at = NOW() WHERE id = $1",
    )
    .bind(job_id)
    .bind(status)
    .bind(detail)
    .execute(pool)
    .await
    .context("failed to update grader job status")?;
    Ok(())
}

async fn update_job_attempts(
    pool: &PgPool,
    job_id: Uuid,
    attempts: usize,
    valid: usize,
    detail: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE grader_jobs SET status = $2, status_detail = $3, attempts_run = $4, valid_runs = $5, updated_at = NOW() WHERE id = $1",
    )
    .bind(job_id)
    .bind(STATUS_PROCESSING)
    .bind(detail)
    .bind(attempts as i32)
    .bind(valid as i32)
    .execute(pool)
    .await
    .context("failed to update grader job progress")?;
    Ok(())
}

async fn update_document_status(
    pool: &PgPool,
    document_id: Uuid,
    status: &str,
    detail: Option<&str>,
    extracted_chars: Option<i32>,
) -> Result<()> {
    sqlx::query(
        "UPDATE grader_documents SET status = $2, status_detail = $3, extracted_chars = COALESCE($4, extracted_chars), updated_at = NOW() WHERE id = $1",
    )
    .bind(document_id)
    .bind(status)
    .bind(detail)
    .bind(extracted_chars)
    .execute(pool)
    .await
    .context("failed to update grader document status")?;
    Ok(())
}

async fn mark_job_failed(
    pool: &PgPool,
    job_id: Uuid,
    document_id: Uuid,
    message: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE grader_jobs SET status = $2, status_detail = $3, error_message = $3, updated_at = NOW() WHERE id = $1",
    )
    .bind(job_id)
    .bind(STATUS_FAILED)
    .bind(message)
    .execute(pool)
    .await
    .context("failed to mark grader job failed")?;

    sqlx::query(
        "UPDATE grader_documents SET status = $2, status_detail = $3, updated_at = NOW() WHERE id = $1",
    )
    .bind(document_id)
    .bind(STATUS_FAILED)
    .bind(message)
    .execute(pool)
    .await
    .context("failed to mark grader document failed")?;
    Ok(())
}

async fn ensure_storage_root() -> Result<()> {
    tokio_fs::create_dir_all(STORAGE_ROOT)
        .await
        .with_context(|| format!("failed to ensure storage root at {}", STORAGE_ROOT))
}

fn read_document_text(path: &Path) -> Result<String> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase();

    let content = match extension.as_str() {
        "pdf" => extract_pdf_text(path)
            .with_context(|| format!("failed to extract PDF text from {}", path.display()))?,
        "docx" => extract_docx_text(path)?,
        "txt" => fs::read_to_string(path)
            .with_context(|| format!("failed to read text file {}", path.display()))?,
        other => return Err(anyhow!("Unsupported file type: {}", other)),
    };

    Ok(content.trim().to_string())
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
                b"w:t" => in_text_node = true,
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

fn internal_error(err: anyhow::Error) -> (StatusCode, Json<ApiError>) {
    error!(?err, "internal error in grader module");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError::new("服务器内部错误。")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_mean_calculates_correctly() {
        let scores = [10.0, 20.0, 30.0, 30.0, 30.0, 30.0];
        let expected = (10.0 * 4.0 + 20.0 * 2.0 + 30.0 * 4.0) / 10.0;
        assert!((weighted_mean(&scores) - expected).abs() < 1e-6);
    }

    #[test]
    fn interquartile_mean_trims_extremes() {
        let values = vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0];
        let (iqm, kept) = interquartile_mean(&values);
        assert_eq!(kept, vec![2, 3]);
        assert!((iqm - 35.0).abs() < 1e-6);
    }

    #[test]
    fn adjust_lower_bound_obeys_rules() {
        assert_eq!(adjust_lower_bound(40.0, 6), Some(36.0));
        assert_eq!(adjust_lower_bound(40.0, 5), Some(38.0));
        assert_eq!(adjust_lower_bound(40.0, 2), None);
    }
}
