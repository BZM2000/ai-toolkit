mod config;
pub mod llm;
mod modules;
mod usage;

use std::{collections::HashMap, env, net::SocketAddr, sync::Arc};

use anyhow::{Context, Result, anyhow};
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::{
    Router,
    extract::{Form, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{Datelike, Duration as ChronoDuration, Utc};
use cookie::time::Duration as CookieDuration;
use dotenvy::dotenv;
use rand_core::OsRng;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use crate::{
    config::{
        DocxTranslatorModels, DocxTranslatorPrompts, DocxTranslatorSettings, GraderModels,
        GraderPrompts, GraderSettings, ModuleSettings, SummarizerModels, SummarizerPrompts,
        SummarizerSettings, update_docx_models, update_docx_prompts, update_grader_models,
        update_grader_prompts, update_summarizer_models, update_summarizer_prompts,
    },
    llm::LlmClient,
};
use tokio::sync::RwLock;

pub(crate) const SESSION_COOKIE: &str = "auth_token";
const SESSION_TTL_DAYS: i64 = 7;
const ROBOTS_TXT_BODY: &str = include_str!("../robots.txt");
fn render_login_page() -> String {
    let footer = render_footer();
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>张圆教授课题组 AI 工具箱</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="robots" content="noindex,nofollow">
    <style>
        :root {{ color-scheme: light; }}
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; display: flex; flex-direction: column; align-items: center; justify-content: center; min-height: 100vh; margin: 0; background: #f1f5f9; color: #0f172a; padding: 1.5rem; box-sizing: border-box; gap: 1.5rem; }}
        main {{ width: 100%; max-width: 480px; display: flex; flex-direction: column; align-items: center; gap: 1.5rem; }}
        .panel {{ background: #ffffff; padding: 2.5rem 2.25rem; border-radius: 18px; box-shadow: 0 20px 60px rgba(15, 23, 42, 0.08); width: 100%; border: 1px solid #e2e8f0; box-sizing: border-box; }}
        h1 {{ margin: 0 0 1rem; font-size: 1.8rem; text-align: center; }}
        p.description {{ margin: 0 0 1.75rem; color: #475569; text-align: center; font-size: 0.95rem; }}
        label {{ display: block; margin-top: 1.2rem; font-weight: 600; letter-spacing: 0.01em; color: #0f172a; }}
        input {{ width: 100%; padding: 0.85rem; margin-top: 0.65rem; border-radius: 10px; border: 1px solid #cbd5f5; background: #f8fafc; color: #0f172a; font-size: 1rem; box-sizing: border-box; }}
        input:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.15); }}
        button {{ margin-top: 2rem; width: 100%; padding: 0.95rem; border: none; border-radius: 10px; background: #2563eb; color: #ffffff; font-weight: 600; font-size: 1.05rem; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        .app-footer {{ margin-top: 2.5rem; text-align: center; font-size: 0.85rem; color: #64748b; }}
    </style>
</head>
<body>
    <main>
        <section class="panel">
            <h1>张圆教授课题组 AI 工具箱</h1>
            <p class="description">请输入管理员分配的账号与密码。</p>
            <form method="post" action="/login">
                <label for="username">用户名</label>
                <input id="username" name="username" required>
                <label for="password">密码</label>
                <input id="password" type="password" name="password" required>
                <button type="submit">登录</button>
            </form>
        </section>
        {footer}
    </main>
</body>
</html>"#,
        footer = footer,
    )
}

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    settings: Arc<RwLock<ModuleSettings>>,
    llm: LlmClient,
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

#[derive(Default, Deserialize)]
struct LandingQuery {
    status: Option<String>,
    error: Option<String>,
}

#[derive(Default, Deserialize)]
struct DashboardQuery {
    status: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct CreateUserForm {
    username: String,
    password: String,
    #[serde(default)]
    usage_group_id: Option<Uuid>,
    #[serde(default)]
    is_admin: Option<String>,
}

#[derive(Deserialize)]
struct UpdatePasswordForm {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct AssignUserGroupForm {
    username: String,
    usage_group_id: Uuid,
}

#[derive(Deserialize)]
struct GlossaryCreateForm {
    source_term: String,
    target_term: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct GlossaryUpdateForm {
    id: Uuid,
    source_term: String,
    target_term: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct GlossaryDeleteForm {
    id: Uuid,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct JournalTopicUpsertForm {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct JournalTopicDeleteForm {
    id: Uuid,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct JournalReferenceUpsertForm {
    journal_name: String,
    #[serde(default)]
    reference_mark: Option<String>,
    low_bound: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(flatten)]
    scores: HashMap<String, String>,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct JournalReferenceDeleteForm {
    id: Uuid,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct SummarizerModelForm {
    summary_model: String,
    translation_model: String,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct SummarizerPromptForm {
    research_summary: String,
    general_summary: String,
    translation: String,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct DocxModelForm {
    translation_model: String,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct DocxPromptForm {
    en_to_cn: String,
    cn_to_en: String,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct GraderModelForm {
    grading_model: String,
    keyword_model: String,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct GraderPromptForm {
    grading_instructions: String,
    keyword_selection: String,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Clone, sqlx::FromRow)]
struct DbUserAuth {
    id: Uuid,
    password_hash: String,
}

#[derive(Clone, sqlx::FromRow)]
struct AuthUser {
    id: Uuid,
    username: String,
    is_admin: bool,
}

#[derive(sqlx::FromRow)]
struct DashboardUserRow {
    id: Uuid,
    username: String,
    usage_group_id: Uuid,
    usage_group_name: String,
    is_admin: bool,
}

#[derive(Clone)]
struct UsageGroupDisplay {
    id: Uuid,
    name: String,
    description: Option<String>,
    limits: HashMap<String, GroupLimitDisplay>,
}

#[derive(sqlx::FromRow)]
struct UsageGroupRow {
    id: Uuid,
    name: String,
    description: Option<String>,
}

#[derive(Clone)]
struct GroupLimitDisplay {
    token_limit: Option<i64>,
    unit_limit: Option<i64>,
}

#[derive(sqlx::FromRow)]
pub(crate) struct GlossaryTermRow {
    pub(crate) id: Uuid,
    pub(crate) source_term: String,
    pub(crate) target_term: String,
    pub(crate) notes: Option<String>,
}

#[derive(Clone, sqlx::FromRow)]
pub(crate) struct JournalTopicRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    created_at: chrono::DateTime<Utc>,
}

#[derive(Clone, sqlx::FromRow)]
pub(crate) struct JournalReferenceRow {
    id: Uuid,
    journal_name: String,
    reference_mark: Option<String>,
    low_bound: f64,
    notes: Option<String>,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Clone, sqlx::FromRow)]
pub(crate) struct JournalTopicScoreRow {
    journal_id: Uuid,
    topic_id: Uuid,
    score: i16,
}

#[tokio::main]
async fn main() {
    dotenv().ok();
    init_tracing();

    if let Err(err) = app_main().await {
        error!(?err, "application error");
        std::process::exit(1);
    }
}

async fn app_main() -> Result<()> {
    let state = AppState::new().await?;
    state.ensure_seed_admin().await?;

    let app = Router::new()
        .route("/", get(landing_page))
        .route("/login", get(login_page).post(process_login))
        .route("/logout", post(logout))
        .route("/robots.txt", get(robots_txt))
        .route("/dashboard", get(dashboard))
        .route("/dashboard/users", post(create_user))
        .route("/dashboard/users/password", post(update_user_password))
        .route("/dashboard/users/group", post(assign_user_group))
        .route("/dashboard/usage-groups", post(save_usage_group))
        .route("/dashboard/glossary", post(create_glossary_term))
        .route("/dashboard/glossary/update", post(update_glossary_term))
        .route("/dashboard/glossary/delete", post(delete_glossary_term))
        .route("/dashboard/modules/summarizer", get(summarizer_admin))
        .route(
            "/dashboard/modules/summarizer/models",
            post(save_summarizer_models),
        )
        .route(
            "/dashboard/modules/summarizer/prompts",
            post(save_summarizer_prompts),
        )
        .route("/dashboard/modules/translatedocx", get(docx_admin))
        .route(
            "/dashboard/modules/translatedocx/models",
            post(save_docx_models),
        )
        .route(
            "/dashboard/modules/translatedocx/prompts",
            post(save_docx_prompts),
        )
        .route("/dashboard/modules/grader", get(grader_admin))
        .route("/dashboard/modules/grader/models", post(save_grader_models))
        .route(
            "/dashboard/modules/grader/prompts",
            post(save_grader_prompts),
        )
        .route("/dashboard/journal-topics", post(upsert_journal_topic))
        .route(
            "/dashboard/journal-topics/delete",
            post(delete_journal_topic),
        )
        .route(
            "/dashboard/journal-references",
            post(upsert_journal_reference),
        )
        .route(
            "/dashboard/journal-references/delete",
            post(delete_journal_reference),
        )
        .merge(modules::summarizer::router())
        .merge(modules::translatedocx::router())
        .merge(modules::grader::router())
        .with_state(state);

    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!(%addr, "listening");

    let listener = TcpListener::bind(addr)
        .await
        .context("failed to bind listener")?;
    axum::serve(listener, app).await.context("server error")?;

    Ok(())
}

impl AppState {
    async fn new() -> Result<Self> {
        let database_url = env::var("DATABASE_URL").context("DATABASE_URL env var is missing")?;

        let llm_client = LlmClient::from_env().context("failed to initialize LLM client")?;

        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(&database_url)
            .await
            .context("failed to connect to Postgres")?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("failed to run database migrations")?;

        ModuleSettings::ensure_defaults(&pool)
            .await
            .context("failed to seed default module settings")?;
        let settings = ModuleSettings::load(&pool)
            .await
            .context("failed to load module settings")?;

        Ok(Self {
            pool,
            settings: Arc::new(RwLock::new(settings)),
            llm: llm_client,
        })
    }

    async fn ensure_seed_admin(&self) -> Result<()> {
        let has_admin: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE is_admin = TRUE)")
                .fetch_one(&self.pool)
                .await
                .context("failed to verify admin presence")?;

        if !has_admin {
            let password_hash = hash_password("change-me")
                .map_err(|err| anyhow!("failed to hash seed admin password: {err}"))?;

            let default_group: Uuid =
                sqlx::query_scalar("SELECT id FROM usage_groups ORDER BY created_at LIMIT 1")
                    .fetch_one(&self.pool)
                    .await
                    .context("failed to locate default usage group")?;

            sqlx::query(
                "INSERT INTO users (id, username, password_hash, usage_group_id, is_admin) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(Uuid::new_v4())
            .bind("demo-admin")
            .bind(password_hash)
            .bind(default_group)
            .bind(true)
            .execute(&self.pool)
            .await
            .context("failed to insert seed admin user")?;

            info!(
                "Seeded default admin user 'demo-admin' (password: 'change-me'). Update it promptly."
            );
        }

        Ok(())
    }

    fn llm_client(&self) -> LlmClient {
        self.llm.clone()
    }

    fn pool(&self) -> PgPool {
        self.pool.clone()
    }

    async fn summarizer_settings(&self) -> Option<SummarizerSettings> {
        let guard = self.settings.read().await;
        guard.summarizer().cloned()
    }

    async fn translate_docx_settings(&self) -> Option<DocxTranslatorSettings> {
        let guard = self.settings.read().await;
        guard.translate_docx().cloned()
    }

    async fn grader_settings(&self) -> Option<GraderSettings> {
        let guard = self.settings.read().await;
        guard.grader().cloned()
    }

    async fn reload_settings(&self) -> Result<()> {
        let latest = ModuleSettings::load(&self.pool)
            .await
            .context("failed to reload module settings")?;
        let mut guard = self.settings.write().await;
        *guard = latest;
        Ok(())
    }
}

async fn landing_page(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<LandingQuery>,
) -> Html<String> {
    let maybe_user = if let Some(cookie) = jar.get(SESSION_COOKIE) {
        if let Ok(token) = Uuid::parse_str(cookie.value()) {
            match fetch_user_by_session(&state.pool, token).await {
                Ok(user) => user,
                Err(err) => {
                    error!(?err, "failed to resolve session for landing page");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    if let Some(user) = maybe_user {
        Html(render_main_page(&user, &params))
    } else {
        Html(render_login_page())
    }
}

async fn login_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Html<String>, Redirect> {
    if let Some(redirect) = redirect_if_authenticated(&state, &jar).await {
        return Err(redirect);
    }

    Ok(Html(render_login_page()))
}

async fn redirect_if_authenticated(state: &AppState, jar: &CookieJar) -> Option<Redirect> {
    let token_cookie = jar.get(SESSION_COOKIE)?;
    let token = Uuid::parse_str(token_cookie.value()).ok()?;

    match fetch_user_by_session(&state.pool, token).await {
        Ok(Some(_)) => Some(Redirect::to("/")),
        Ok(None) => None,
        Err(err) => {
            error!(?err, "failed to validate session for access gate");
            None
        }
    }
}

async fn process_login(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> Result<(CookieJar, Redirect), (StatusCode, Html<String>)> {
    let username = form.username.trim();

    let user = match fetch_user_by_username(&state.pool, username).await {
        Ok(Some(user)) => user,
        Ok(None) => return Err(invalid_credentials()),
        Err(err) => {
            error!(?err, "failed to fetch user during login");
            return Err(server_error());
        }
    };

    if !verify_password(&form.password, &user.password_hash) {
        return Err(invalid_credentials());
    }

    let session_token = Uuid::new_v4();
    let expires_at = Utc::now() + ChronoDuration::days(SESSION_TTL_DAYS);

    if let Err(err) =
        sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES ($1, $2, $3)")
            .bind(session_token)
            .bind(user.id)
            .bind(expires_at)
            .execute(&state.pool)
            .await
    {
        error!(?err, "failed to create session");
        return Err(server_error());
    }

    let mut cookie = Cookie::new(SESSION_COOKIE, session_token.to_string());
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_max_age(CookieDuration::days(SESSION_TTL_DAYS));

    let jar = jar.add(cookie);
    Ok((jar, Redirect::to("/")))
}

async fn logout(State(state): State<AppState>, jar: CookieJar) -> (CookieJar, Redirect) {
    let mut jar = jar;

    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        if let Ok(token) = Uuid::parse_str(cookie.value()) {
            if let Err(err) = sqlx::query("DELETE FROM sessions WHERE id = $1")
                .bind(token)
                .execute(&state.pool)
                .await
            {
                error!(?err, "failed to remove session during logout");
            }
        }
    }

    let mut removal = Cookie::new(SESSION_COOKIE, "");
    removal.set_path("/");
    removal.set_http_only(true);
    removal.set_same_site(SameSite::Lax);
    removal.set_max_age(CookieDuration::seconds(0));
    jar = jar.remove(removal);

    (jar, Redirect::to("/?status=logged_out"))
}

async fn robots_txt() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        ROBOTS_TXT_BODY,
    )
}

async fn dashboard(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<DashboardQuery>,
) -> Result<Html<String>, Redirect> {
    let Some(token_cookie) = jar.get(SESSION_COOKIE) else {
        return Err(Redirect::to("/login"));
    };

    let token = match Uuid::parse_str(token_cookie.value()) {
        Ok(token) => token,
        Err(_) => return Err(Redirect::to("/login")),
    };

    let auth_user = match fetch_user_by_session(&state.pool, token).await {
        Ok(Some(user)) => user,
        Ok(None) => return Err(Redirect::to("/login")),
        Err(err) => {
            error!(?err, "failed to fetch session user");
            return Err(Redirect::to("/login"));
        }
    };

    if !auth_user.is_admin {
        return Err(Redirect::to("/?error=not_authorized"));
    }

    let users = fetch_dashboard_users(&state.pool).await.map_err(|err| {
        error!(?err, "failed to load dashboard users");
        Redirect::to("/login")
    })?;

    let user_ids: Vec<Uuid> = users.iter().map(|user| user.id).collect();
    let usage_map = usage::usage_for_users(&state.pool, &user_ids)
        .await
        .unwrap_or_default();

    let groups = fetch_usage_groups_with_limits(&state.pool)
        .await
        .map_err(|err| {
            error!(?err, "failed to load usage groups");
            Redirect::to("/login")
        })?;

    if groups.is_empty() {
        error!("no usage groups configured");
        return Err(Redirect::to("/login"));
    }

    let mut group_lookup: HashMap<Uuid, HashMap<String, GroupLimitDisplay>> = HashMap::new();
    let mut group_options_for_create = String::new();
    let mut group_options_for_assign = String::new();

    for (idx, group) in groups.iter().enumerate() {
        group_lookup.insert(group.id, group.limits.clone());
        let option = format!(
            "<option value=\"{value}\"{selected}>{label}</option>",
            value = escape_html(&group.id.to_string()),
            label = escape_html(&group.name),
            selected = if idx == 0 { " selected" } else { "" }
        );
        group_options_for_create.push_str(&option);
        group_options_for_assign.push_str(&format!(
            "<option value=\"{value}\">{label}</option>",
            value = escape_html(&group.id.to_string()),
            label = escape_html(&group.name)
        ));
    }

    let mut table_rows = String::new();

    if users.is_empty() {
        table_rows.push_str("<tr><td colspan=\"5\">当前还没有用户。</td></tr>");
    } else {
        for user in &users {
            let role = if user.is_admin {
                "管理员"
            } else {
                "普通用户"
            };
            let highlight_class = if user.username == auth_user.username {
                "current-user"
            } else {
                ""
            };

            let usage_entries = usage_map.get(&user.id);
            let limit_entries = group_lookup.get(&user.usage_group_id);

            let mut chips = String::new();
            let mut total_units = 0;
            let mut total_tokens = 0;
            for descriptor in usage::REGISTERED_MODULES {
                let usage_snapshot = usage_entries.and_then(|map| map.get(descriptor.key));
                let units_used = usage_snapshot.map(|s| s.units).unwrap_or(0);
                let tokens_used = usage_snapshot.map(|s| s.tokens).unwrap_or(0);

                total_units += units_used;
                total_tokens += tokens_used;

                let limit_snapshot = limit_entries.and_then(|map| map.get(descriptor.key));

                let unit_text = match limit_snapshot.and_then(|l| l.unit_limit) {
                    Some(limit) => format!(
                        "{units_used}/{limit} {label}",
                        label = descriptor.unit_label
                    ),
                    None => format!("{units_used} {label}", label = descriptor.unit_label),
                };
                let token_text = match limit_snapshot.and_then(|l| l.token_limit) {
                    Some(limit) => format!("{tokens_used}/{limit} 令牌"),
                    None => format!("{tokens_used} 令牌"),
                };

                chips.push_str(&format!(
                    "<div class=\"usage-chip\"><span class=\"chip-title\">{title}</span><span>{unit}</span><span>{tokens}</span></div>",
                    title = escape_html(descriptor.label),
                    unit = escape_html(&unit_text),
                    tokens = escape_html(&token_text),
                ));
            }

            let usage_detail_html = format!("<div class=\"usage-grid\">{chips}</div>");
            let usage_summary = format!("{total_units} 项 · {total_tokens} 令牌");

            // Build group dropdown for this user
            let mut group_select = format!(
                "<form method=\"post\" action=\"/dashboard/users/group\" class=\"inline-form\" onsubmit=\"return confirm('确认更改 {} 的额度组？');\">",
                escape_html(&user.username)
            );
            group_select.push_str(&format!(
                "<input type=\"hidden\" name=\"username\" value=\"{}\">",
                escape_html(&user.username)
            ));
            group_select.push_str("<select name=\"usage_group_id\" class=\"inline-select\" onchange=\"this.form.submit()\">");
            for group in &groups {
                let selected = if group.id == user.usage_group_id { " selected" } else { "" };
                group_select.push_str(&format!(
                    "<option value=\"{}\"{}>{}</option>",
                    escape_html(&group.id.to_string()),
                    selected,
                    escape_html(&group.name)
                ));
            }
            group_select.push_str("</select></form>");

            // Main row with summary
            table_rows.push_str(&format!(
                "<tr class=\"user-row {highlight}\" data-user-id=\"{id}\"><td><span class=\"expand-icon\">▶</span> {name}</td><td>{group_dropdown}</td><td>{role}</td><td class=\"usage-summary\">{summary}</td><td class=\"actions\"><button class=\"btn-sm\" onclick=\"toggleUserDetails('{id}')\">详情</button><button class=\"btn-sm btn-warning\" data-username=\"{username}\" onclick=\"resetPassword(this)\">重置密码</button></td></tr>",
                id = user.id,
                name = escape_html(&user.username),
                username = escape_html(&user.username),
                group_dropdown = group_select,
                role = role,
                summary = escape_html(&usage_summary),
                highlight = highlight_class
            ));

            // Detail row (hidden by default)
            table_rows.push_str(&format!(
                "<tr class=\"user-detail-row\" id=\"detail-{id}\" style=\"display: none;\"><td colspan=\"5\">{usage}</td></tr>",
                id = user.id,
                usage = usage_detail_html
            ));
        }
    }

    let message_block = compose_flash_message(&params);

    let user_controls = format!(
        r##"<section class=\"admin collapsible-section\">
    <h2 class=\"section-header\" onclick=\"toggleSection('create-user')\">
        <span class=\"toggle-icon\" id=\"icon-create-user\">▼</span> 创建用户
    </h2>
    <div class=\"section-content\" id=\"content-create-user\">
        <form method=\"post\" action=\"/dashboard/users\">
            <div class=\"field\">
                <label for=\"new-username\">用户名</label>
                <input id=\"new-username\" name=\"username\" required>
            </div>
            <div class=\"field\">
                <label for=\"new-password\">密码</label>
                <input id=\"new-password\" type=\"password\" name=\"password\" required>
            </div>
            <div class=\"field\">
                <label for=\"new-group\">额度组</label>
                <select id=\"new-group\" name=\"usage_group_id\" required>
                    {group_options}
                </select>
            </div>
            <div class=\"field checkbox\">
                <label><input type=\"checkbox\" name=\"is_admin\" value=\"on\"> 授予管理员权限</label>
            </div>
            <button type=\"submit\">创建用户</button>
        </form>
    </div>
</section>"##,
        group_options = group_options_for_create,
    );

    let mut group_sections = String::new();
    for group in &groups {
        let mut module_fields = String::new();
        for descriptor in usage::REGISTERED_MODULES {
            let limit = group.limits.get(descriptor.key);
            let units_value = limit
                .and_then(|l| l.unit_limit)
                .map(|v| v.to_string())
                .unwrap_or_default();
            let tokens_value = limit
                .and_then(|l| l.token_limit)
                .map(|v| v.to_string())
                .unwrap_or_default();

            module_fields.push_str(&format!(
                r#"<div class=\"field-set\">
        <h3>{title}</h3>
        <div class=\"dual-inputs\">
            <div class=\"field\">
                <label for=\"units-{key}-{id}\">{unit_label}（近 7 日）</label>
                <input id=\"units-{key}-{id}\" name=\"units_{key}\" value=\"{units}\" placeholder=\"留空表示不限\">
            </div>
            <div class=\"field\">
                <label for=\"tokens-{key}-{id}\">令牌上限（近 7 日）</label>
                <input id=\"tokens-{key}-{id}\" name=\"tokens_{key}\" value=\"{tokens}\" placeholder=\"留空表示不限\">
            </div>
        </div>
    </div>"#,
                title = escape_html(descriptor.label),
                key = descriptor.key,
                id = group.id,
                unit_label = descriptor.unit_label,
                units = escape_html(&units_value),
                tokens = escape_html(&tokens_value),
            ));
        }

        let desc_display = group
            .description
            .as_ref()
            .map(|d| escape_html(d))
            .unwrap_or_else(|| "无描述".to_string());
        let desc_value = group
            .description
            .as_ref()
            .map(|d| escape_html(d))
            .unwrap_or_default();

        let section_id = format!("group-{}", group.id);
        group_sections.push_str(&format!(
            r##"<section class=\"admin collapsible-section group-panel\">
    <h2 class=\"section-header\" onclick=\"toggleSection('{section_id}')\">
        <span class=\"toggle-icon\" id=\"icon-{section_id}\">▶</span> 额度组：{name}
    </h2>
    <div class=\"section-content collapsed\" id=\"content-{section_id}\">
        <p class=\"meta-note\">{desc}</p>
        <form method=\"post\" action=\"/dashboard/usage-groups\">
            <input type=\"hidden\" name=\"group_id\" value=\"{id}\">
            <div class=\"field\">
                <label for=\"group-name-{id}\">组名称</label>
                <input id=\"group-name-{id}\" name=\"name\" value=\"{name}\" required>
            </div>
            <div class=\"field\">
                <label for=\"group-desc-{id}\">描述</label>
                <input id=\"group-desc-{id}\" name=\"description\" value=\"{desc_value}\" placeholder=\"可选\">
            </div>
            {module_fields}
            <div class=\"action-stack\">
                <button type=\"submit\">保存额度</button>
            </div>
        </form>
    </div>
</section>"##,
            id = group.id,
            section_id = section_id,
            name = escape_html(&group.name),
            desc = desc_display,
            desc_value = desc_value,
            module_fields = module_fields,
        ));
    }

    let mut new_group_fields = String::new();
    for descriptor in usage::REGISTERED_MODULES {
        new_group_fields.push_str(&format!(
            r#"<div class=\"field-set\">
        <h3>{title}</h3>
        <div class=\"dual-inputs\">
            <div class=\"field\">
                <label for=\"new-units-{key}\">{unit_label}（近 7 日）</label>
                <input id=\"new-units-{key}\" name=\"units_{key}\" placeholder=\"留空表示不限\">
            </div>
            <div class=\"field\">
                <label for=\"new-tokens-{key}\">令牌上限（近 7 日）</label>
                <input id=\"new-tokens-{key}\" name=\"tokens_{key}\" placeholder=\"留空表示不限\">
            </div>
        </div>
    </div>"#,
            title = escape_html(descriptor.label),
            unit_label = descriptor.unit_label,
            key = descriptor.key,
        ));
    }

    let new_group_section = format!(
        r##"<section class=\"admin collapsible-section group-panel\">
    <h2 class=\"section-header\" onclick=\"toggleSection('new-group')\">
        <span class=\"toggle-icon\" id=\"icon-new-group\">▶</span> 新建额度组
    </h2>
    <div class=\"section-content collapsed\" id=\"content-new-group\">
        <form method=\"post\" action=\"/dashboard/usage-groups\">
            <div class=\"field\">
                <label for=\"new-group-name\">组名称</label>
                <input id=\"new-group-name\" name=\"name\" required>
            </div>
            <div class=\"field\">
                <label for=\"new-group-desc\">描述</label>
                <input id=\"new-group-desc\" name=\"description\" placeholder=\"可选\">
            </div>
            {new_group_fields}
            <div class=\"action-stack\">
                <button type=\"submit\">创建额度组</button>
            </div>
        </form>
    </div>
</section>"##,
        new_group_fields = new_group_fields,
    );

    let footer = render_footer();

    let html = format!(
        r##"<!DOCTYPE html>
<html lang=\"zh-CN\">
<head>
    <meta charset=\"UTF-8\">
    <title>管理后台</title>
    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
    <meta name=\"robots\" content=\"noindex,nofollow\">
    <style>
        :root {{ color-scheme: light; }}
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; margin: 0; background: #f8fafc; color: #0f172a; }}
        header {{ background: #ffffff; padding: 2rem 1.5rem; border-bottom: 1px solid #e2e8f0; }}
        .header-bar {{ display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 1rem; }}
        .back-link {{ display: inline-flex; align-items: center; gap: 0.4rem; color: #1d4ed8; text-decoration: none; font-weight: 600; background: #e0f2fe; padding: 0.5rem 0.95rem; border-radius: 999px; border: 1px solid #bfdbfe; transition: background 0.15s ease, border 0.15s ease; }}
        .back-link:hover {{ background: #bfdbfe; border-color: #93c5fd; }}
        main {{ padding: 2rem 1.5rem; max-width: 1100px; margin: 0 auto; box-sizing: border-box; }}
        table {{ width: 100%; border-collapse: collapse; margin-top: 1.5rem; background: #ffffff; border: 1px solid #e2e8f0; border-radius: 12px; overflow: hidden; }}
        th, td {{ padding: 0.75rem 1rem; border-bottom: 1px solid #e2e8f0; text-align: left; vertical-align: middle; }}
        th {{ background: #f1f5f9; color: #0f172a; font-weight: 600; }}
        tr.user-row {{ cursor: pointer; transition: background 0.15s ease; }}
        tr.user-row:hover {{ background: #f8fafc; }}
        tr.user-row.current-user td {{ background: #eff6ff; }}
        tr.user-row.current-user:hover td {{ background: #dbeafe; }}
        tr.user-detail-row td {{ background: #f8fafc; padding: 1.5rem; vertical-align: top; }}
        .expand-icon {{ display: inline-block; width: 1.2em; font-size: 0.85em; color: #64748b; transition: transform 0.2s ease; }}
        .user-row.expanded .expand-icon {{ transform: rotate(90deg); }}
        .usage-summary {{ color: #64748b; font-size: 0.95em; }}
        .usage-grid {{ display: grid; gap: 0.6rem; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); }}
        .usage-chip {{ display: flex; flex-direction: column; gap: 0.25rem; padding: 0.75rem; border-radius: 10px; border: 1px solid #e2e8f0; background: #ffffff; }}
        .usage-chip .chip-title {{ font-weight: 600; color: #1d4ed8; }}
        .admin {{ margin-top: 2.5rem; padding: 1.5rem; border-radius: 12px; background: #ffffff; border: 1px solid #e2e8f0; box-shadow: 0 1px 3px rgba(15, 23, 42, 0.08); }}
        .admin h2 {{ margin-top: 0; color: #1d4ed8; }}
        .collapsible-section {{ padding: 0; }}
        .section-header {{ margin: 0; padding: 1rem 1.5rem; cursor: pointer; user-select: none; transition: background 0.15s ease; border-radius: 12px; }}
        .section-header:hover {{ background: #f8fafc; }}
        .toggle-icon {{ display: inline-block; width: 1.2em; font-size: 0.9em; color: #64748b; transition: transform 0.2s ease; }}
        .section-content {{ padding: 0 1.5rem 1.5rem 1.5rem; overflow: hidden; transition: max-height 0.3s ease, opacity 0.3s ease; max-height: 2000px; opacity: 1; }}
        .section-content.collapsed {{ max-height: 0; opacity: 0; padding-top: 0; padding-bottom: 0; }}
        .btn-sm {{ padding: 0.4rem 0.8rem; font-size: 0.85rem; border: 1px solid #cbd5e1; background: #ffffff; color: #475569; border-radius: 6px; cursor: pointer; transition: all 0.15s ease; }}
        .btn-sm:hover {{ background: #f1f5f9; border-color: #94a3b8; }}
        .actions {{ text-align: right; }}
        .field {{ margin-bottom: 1rem; display: flex; flex-direction: column; gap: 0.4rem; }}
        .field label {{ font-weight: 600; color: #0f172a; font-size: 0.95rem; }}
        .field input, .field select, .field textarea {{ padding: 0.75rem; border-radius: 8px; border: 1px solid #cbd5e1; background: #f8fafc; color: #0f172a; font-family: inherit; }}
        .field input:focus, .field select:focus, .field textarea:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        .field.checkbox {{ flex-direction: row; align-items: center; gap: 0.6rem; }}
        .field.checkbox label {{ font-weight: 500; }}
        .field-set {{ margin-bottom: 1.5rem; }}
        .field-set h3 {{ margin: 0 0 0.75rem; color: #0f172a; font-size: 1rem; }}
        .dual-inputs {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 1rem; }}
        .action-stack {{ display: flex; flex-wrap: wrap; gap: 0.75rem; margin-top: 1rem; }}
        button {{ padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        button:disabled {{ opacity: 0.6; cursor: not-allowed; }}
        .flash {{ padding: 1rem; border-radius: 8px; margin-top: 1rem; border: 1px solid transparent; }}
        .flash.success {{ background: #ecfdf3; border-color: #bbf7d0; color: #166534; }}
        .flash.error {{ background: #fef2f2; border-color: #fecaca; color: #b91c1c; }}
        .meta-note {{ margin-bottom: 0.5rem; color: #64748b; font-size: 0.95rem; }}
        .group-panel {{ margin-top: 2.5rem; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
        .inline-form {{ margin: 0; display: inline; }}
        .inline-select {{ padding: 0.5rem 0.75rem; border-radius: 6px; border: 1px solid #cbd5e1; background: #ffffff; color: #0f172a; font-size: 0.9rem; cursor: pointer; transition: border-color 0.15s ease, box-shadow 0.15s ease; }}
        .inline-select:hover {{ border-color: #94a3b8; }}
        .inline-select:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        .btn-warning {{ background: #f59e0b; color: #ffffff; margin-left: 0.5rem; }}
        .btn-warning:hover {{ background: #d97706; }}
        .actions {{ display: flex; gap: 0.5rem; justify-content: flex-end; }}
        .modal {{ display: none; position: fixed; z-index: 1000; left: 0; top: 0; width: 100%; height: 100%; background: rgba(0, 0, 0, 0.5); }}
        .modal-content {{ background: #ffffff; margin: 10% auto; padding: 2rem; border-radius: 12px; max-width: 400px; box-shadow: 0 4px 6px rgba(0, 0, 0, 0.1); }}
        .modal-header {{ margin-bottom: 1.5rem; }}
        .modal-header h3 {{ margin: 0; color: #0f172a; }}
        .modal-actions {{ display: flex; gap: 0.75rem; justify-content: flex-end; margin-top: 1.5rem; }}
        .modal-actions button {{ padding: 0.75rem 1.25rem; }}
    </style>
</head>
<body>
    <header>
        <div class=\"header-bar\">
            <h1>使用情况仪表盘</h1>
            <a class=\"back-link\" href=\"/\">← 返回首页</a>
        </div>
        <p>管理账号额度，并进入各模块的配置页面。</p>
    </header>
    <main>
        <p data-user-id=\"{auth_id}\">当前登录：<strong>{username}</strong>。</p>
        {message_block}
        <table>
            <thead>
                <tr><th>用户名</th><th>额度组</th><th>角色</th><th>近 7 日使用（摘要）</th><th>操作</th></tr>
            </thead>
            <tbody>
                {table_rows}
            </tbody>
        </table>
        {user_controls}
        {group_sections}
        {new_group}
        {footer}
    </main>
    <div id=\"password-modal\" class=\"modal\">
        <div class=\"modal-content\">
            <div class=\"modal-header\">
                <h3>重置密码</h3>
            </div>
            <form id=\"password-reset-form\" method=\"post\" action=\"/dashboard/users/password\">
                <input type=\"hidden\" name=\"username\" value=\"\">
                <p>为用户 <strong id=\"reset-username-display\"></strong> 设置新密码：</p>
                <div class=\"field\">
                    <label for=\"modal-password-input\">新密码</label>
                    <input id=\"modal-password-input\" type=\"password\" name=\"password\" required>
                </div>
                <div class=\"modal-actions\">
                    <button type=\"button\" class=\"btn-sm\" onclick=\"closeModal()\">取消</button>
                    <button type=\"submit\">确认重置</button>
                </div>
            </form>
        </div>
    </div>
    <script>
        function toggleUserDetails(userId) {{
            const detailRow = document.getElementById('detail-' + userId);
            const userRow = document.querySelector('tr.user-row[data-user-id=\"' + userId + '\"]');

            if (detailRow.style.display === 'none') {{
                detailRow.style.display = 'table-row';
                userRow.classList.add('expanded');
            }} else {{
                detailRow.style.display = 'none';
                userRow.classList.remove('expanded');
            }}
        }}

        function toggleSection(sectionId) {{
            const content = document.getElementById('content-' + sectionId);
            const icon = document.getElementById('icon-' + sectionId);

            if (content.classList.contains('collapsed')) {{
                content.classList.remove('collapsed');
                icon.textContent = '▼';
            }} else {{
                content.classList.add('collapsed');
                icon.textContent = '▶';
            }}
        }}

        function resetPassword(buttonElement) {{
            const username = buttonElement.getAttribute('data-username');
            const modal = document.getElementById('password-modal');
            const usernameSpan = document.getElementById('reset-username-display');
            const passwordInput = document.getElementById('modal-password-input');
            const form = document.getElementById('password-reset-form');

            usernameSpan.textContent = username;
            form.querySelector('input[name=\"username\"]').value = username;
            passwordInput.value = '';

            modal.style.display = 'block';
            passwordInput.focus();
        }}

        function closeModal() {{
            document.getElementById('password-modal').style.display = 'none';
        }}

        // Close modal when clicking outside
        window.onclick = function(event) {{
            const modal = document.getElementById('password-modal');
            if (event.target === modal) {{
                closeModal();
            }}
        }}
    </script>
</body>
</html>"##,
        auth_id = auth_user.id,
        username = escape_html(&auth_user.username),
        message_block = message_block,
        table_rows = table_rows,
        user_controls = user_controls,
        group_sections = group_sections,
        new_group = new_group_section,
        footer = footer,
    );

    Ok(Html(html))
}

fn compose_flash_message(params: &DashboardQuery) -> String {
    if let Some(status) = params.status.as_deref() {
        match status {
            "created" => {
                return "<div class=\"flash success\">已成功创建用户。</div>".to_string();
            }
            "password_updated" => {
                return "<div class=\"flash success\">已更新密码。</div>".to_string();
            }
            "glossary_created" => {
                return "<div class=\"flash success\">已新增术语。</div>".to_string();
            }
            "glossary_updated" => {
                return "<div class=\"flash success\">已更新术语。</div>".to_string();
            }
            "glossary_deleted" => {
                return "<div class=\"flash success\">已删除术语。</div>".to_string();
            }
            "topic_saved" => {
                return "<div class=\"flash success\">已保存主题。</div>".to_string();
            }
            "topic_deleted" => {
                return "<div class=\"flash success\">已删除主题。</div>".to_string();
            }
            "journal_saved" => {
                return "<div class=\"flash success\">已保存期刊参考。</div>".to_string();
            }
            "journal_deleted" => {
                return "<div class=\"flash success\">已删除期刊参考。</div>".to_string();
            }
            "summarizer_models_saved" => {
                return "<div class=\"flash success\">已更新摘要模块模型。</div>".to_string();
            }
            "summarizer_prompts_saved" => {
                return "<div class=\"flash success\">已更新摘要模块提示词。</div>".to_string();
            }
            "docx_models_saved" => {
                return "<div class=\"flash success\">已更新 DOCX 模块模型。</div>".to_string();
            }
            "docx_prompts_saved" => {
                return "<div class=\"flash success\">已更新 DOCX 模块提示词。</div>".to_string();
            }
            "grader_models_saved" => {
                return "<div class=\"flash success\">已更新稿件评估模型。</div>".to_string();
            }
            "grader_prompts_saved" => {
                return "<div class=\"flash success\">已更新稿件评估提示词。</div>".to_string();
            }
            "group_created" => {
                return "<div class=\"flash success\">已创建额度组。</div>".to_string();
            }
            "group_saved" => {
                return "<div class=\"flash success\">已更新额度组。</div>".to_string();
            }
            "group_assigned" => {
                return "<div class=\"flash success\">已更新用户额度组。</div>".to_string();
            }
            _ => {}
        }
    }

    if let Some(error) = params.error.as_deref() {
        let message = match error {
            "duplicate" => "用户名已存在。",
            "not_authorized" => "需要管理员权限。",
            "missing_username" => "请输入用户名。",
            "missing_password" => "请输入密码。",
            "password_missing" => "请输入新密码。",
            "user_missing" => "未找到该用户。",
            "hash_failed" => "处理密码时出错，请重试。",
            "glossary_missing_fields" => "请填写英文和中文术语。",
            "glossary_duplicate" => "已存在相同英文术语。",
            "glossary_not_found" => "未找到对应术语。",
            "topic_missing_name" => "请填写主题名称。",
            "topic_not_found" => "未找到对应主题。",
            "journal_missing_name" => "请填写期刊名称。",
            "journal_invalid_low" => "请输入有效的低区间数值。",
            "journal_invalid_score" => "主题分值必须是 0-9 的整数。",
            "journal_not_found" => "未找到对应期刊参考。",
            "summarizer_invalid_models" => "请提供摘要模块所需的全部模型字段。",
            "summarizer_invalid_prompts" => "请填写摘要模块的所有提示文案。",
            "docx_invalid_models" => "请提供 DOCX 模块的模型配置。",
            "docx_invalid_prompts" => "请填写 DOCX 模块的提示文案。",
            "grader_invalid_models" => "请提供稿件评估模块的模型配置。",
            "grader_invalid_prompts" => "请填写稿件评估模块的提示文案。",
            "group_missing" => "请选择有效的额度组。",
            "group_invalid" => "额度组标识无效。",
            "group_invalid_limit" => "额度上限需为非负整数。",
            "group_duplicate" => "已存在同名额度组。",
            "group_name_missing" => "请输入额度组名称。",
            _ => "发生未知错误，请查看日志。",
        };

        return format!("<div class=\"flash error\">{message}</div>");
    }

    String::new()
}

fn sanitize_redirect_path(input: Option<&str>) -> &'static str {
    match input {
        Some("/dashboard/modules/summarizer") => "/dashboard/modules/summarizer",
        Some("/dashboard/modules/translatedocx") => "/dashboard/modules/translatedocx",
        Some("/dashboard/modules/grader") => "/dashboard/modules/grader",
        _ => "/dashboard",
    }
}

const MODULE_ADMIN_SHARED_STYLES: &str = r#"
        section.admin {
            margin-bottom: 2rem;
            padding: 1.5rem;
            border-radius: 12px;
            background: #ffffff;
            border: 1px solid #e2e8f0;
            box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08);
        }
        section.admin h2 {
            margin-top: 0;
            color: #1d4ed8;
        }
        section.admin h3 {
            margin-top: 0;
            color: #0f172a;
        }
        .section-note {
            color: #475569;
            font-size: 0.95rem;
            margin-bottom: 1rem;
        }
        .stack {
            display: flex;
            flex-direction: column;
            gap: 1.5rem;
        }
        .glossary-forms {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
            gap: 1.5rem;
        }
        .glossary-forms form {
            background: #f8fafc;
            border: 1px solid #e2e8f0;
            border-radius: 10px;
            padding: 1.25rem;
        }
        .glossary-forms h3 {
            margin-bottom: 0.75rem;
        }
        .field {
            margin-bottom: 1rem;
            display: flex;
            flex-direction: column;
            gap: 0.4rem;
        }
        .field label {
            font-weight: 600;
            color: #0f172a;
        }
        .field input,
        .field select,
        .field textarea {
            padding: 0.75rem;
            border-radius: 8px;
            border: 1px solid #cbd5e1;
            background: #f8fafc;
            color: #0f172a;
            font-family: inherit;
        }
        .field input:focus,
        .field select:focus,
        .field textarea:focus {
            outline: none;
            border-color: #2563eb;
            box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12);
        }
        table.glossary {
            width: 100%;
            border-collapse: collapse;
            border: 1px solid #e2e8f0;
            border-radius: 12px;
            overflow: hidden;
            background: #ffffff;
        }
        table.glossary th,
        table.glossary td {
            padding: 0.75rem 1rem;
            border-bottom: 1px solid #e2e8f0;
            text-align: left;
        }
        table.glossary th {
            background: #f1f5f9;
            color: #0f172a;
        }
        table.glossary td form {
            margin: 0;
        }
        button.danger {
            background: #dc2626;
            border: 1px solid #b91c1c;
            color: #ffffff;
        }
        button.danger:hover {
            background: #b91c1c;
        }
        .topic-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
            gap: 1rem;
            margin-bottom: 1rem;
        }
        .topic-picker {
            display: flex;
            flex-direction: column;
            gap: 0.45rem;
            padding: 0.75rem;
            background: #f8fafc;
            border: 1px solid #dbeafe;
            border-radius: 10px;
            transition: border 0.15s ease, box-shadow 0.15s ease, background 0.15s ease;
        }
        .topic-picker select {
            padding: 0.6rem;
            border-radius: 8px;
            border: 1px solid #cbd5e1;
            background: #ffffff;
        }
        .topic-picker.active {
            border-color: #2563eb;
            box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12);
            background: #eff6ff;
        }
        .topic-picker.active label {
            color: #1d4ed8;
        }
        .journal-form-actions {
            display: flex;
            flex-wrap: wrap;
            gap: 0.75rem;
            margin-top: 1rem;
        }
        button.secondary {
            background: #ffffff;
            color: #1d4ed8;
            border: 1px solid #93c5fd;
        }
        button.secondary:hover {
            background: #dbeafe;
        }
        .action-stack {
            display: flex;
            flex-direction: column;
            gap: 0.5rem;
        }
        .action-stack form {
            margin: 0;
        }
"#;

fn render_glossary_section(terms: &[GlossaryTermRow], redirect: &str) -> String {
    let mut rows = String::new();
    let mut select_options = String::new();

    if terms.is_empty() {
        rows.push_str("<tr><td colspan=\\\"4\\\">尚未添加术语。</td></tr>");
    } else {
        for term in terms {
            let notes_display = term
                .notes
                .as_ref()
                .map(|n| escape_html(n))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "—".to_string());
            rows.push_str(&format!(
                r#"<tr><td>{en}</td><td>{cn}</td><td>{notes}</td><td>
    <form method="post" action="/dashboard/glossary/delete" onsubmit="return confirm('确定删除该术语？');">
        <input type="hidden" name="id" value="{id}">
        <input type="hidden" name="redirect" value="{redirect}">
        <button type="submit" class="danger">删除</button>
    </form>
</td></tr>"#,
                en = escape_html(&term.source_term),
                cn = escape_html(&term.target_term),
                notes = notes_display,
                id = term.id,
                redirect = redirect
            ));

            select_options.push_str(&format!(
                "<option value=\\\"{id}\\\">EN：{en} → CN：{cn}</option>",
                id = term.id,
                en = escape_html(&term.source_term),
                cn = escape_html(&term.target_term)
            ));
        }
    }

    let (select_options, disabled_attr) = if select_options.is_empty() {
        (
            "<option value=\\\"\\\" disabled selected>暂无可选术语</option>".to_string(),
            " disabled",
        )
    } else {
        (select_options, "")
    };

    format!(
        r##"<section class="admin">
    <h2>术语表管理</h2>
    <p class="section-note">该术语表同时用于摘要与 DOCX 翻译模块。</p>
    <div class="stack">
        <table class="glossary">
            <thead>
                <tr><th>英文</th><th>中文</th><th>备注</th><th>操作</th></tr>
            </thead>
            <tbody>
                {rows}
            </tbody>
        </table>
        <div class="glossary-forms">
            <form method="post" action="/dashboard/glossary">
                <h3>新增术语</h3>
                <input type="hidden" name="redirect" value="{redirect}">
                <div class="field">
                    <label for="glossary-source">英文术语</label>
                    <input id="glossary-source" name="source_term" required>
                </div>
                <div class="field">
                    <label for="glossary-target">中文术语</label>
                    <input id="glossary-target" name="target_term" required>
                </div>
                <div class="field">
                    <label for="glossary-notes">备注（可选）</label>
                    <input id="glossary-notes" name="notes" placeholder="填写上下文或使用说明">
                </div>
                <button type="submit">保存术语</button>
            </form>
            <form method="post" action="/dashboard/glossary/update">
                <h3>更新术语</h3>
                <input type="hidden" name="redirect" value="{redirect}">
                <div class="field">
                    <label for="glossary-update-id">选择术语</label>
                    <select id="glossary-update-id" name="id" required{disabled_attr}>
                        {select_options}
                    </select>
                </div>
                <div class="field">
                    <label for="glossary-update-source">更新后的英文</label>
                    <input id="glossary-update-source" name="source_term" required{disabled_attr}>
                </div>
                <div class="field">
                    <label for="glossary-update-target">更新后的中文</label>
                    <input id="glossary-update-target" name="target_term" required{disabled_attr}>
                </div>
                <div class="field">
                    <label for="glossary-update-notes">备注（可选）</label>
                    <input id="glossary-update-notes" name="notes" placeholder="填写上下文或使用说明"{disabled_attr}>
                </div>
                <button type="submit"{disabled_attr}>保存修改</button>
            </form>
        </div>
    </div>
</section>"##,
        rows = rows,
        select_options = select_options,
        disabled_attr = disabled_attr,
        redirect = redirect,
    )
}

async fn require_admin_user(state: &AppState, jar: &CookieJar) -> Result<AuthUser, Redirect> {
    let Some(token_cookie) = jar.get(SESSION_COOKIE) else {
        return Err(Redirect::to("/login"));
    };

    let token = match Uuid::parse_str(token_cookie.value()) {
        Ok(token) => token,
        Err(_) => return Err(Redirect::to("/login")),
    };

    let auth_user = match fetch_user_by_session(&state.pool, token).await {
        Ok(Some(user)) => user,
        _ => return Err(Redirect::to("/login")),
    };

    if !auth_user.is_admin {
        return Err(Redirect::to("/?error=not_authorized"));
    }

    Ok(auth_user)
}

fn render_topic_section(topics: &[JournalTopicRow], redirect: &str) -> String {
    let mut rows = String::new();

    if topics.is_empty() {
        rows.push_str("<tr><td colspan=\\\"4\\\">尚未添加主题。</td></tr>");
    } else {
        for topic in topics {
            let description = topic
                .description
                .as_ref()
                .map(|d| escape_html(d))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "—".to_string());
            let created = topic.created_at.format("%Y-%m-%d");
            rows.push_str(&format!(
                r#"<tr><td>{name}</td><td>{description}</td><td>{created}</td><td>
    <form method="post" action="/dashboard/journal-topics/delete" onsubmit="return confirm('确定删除该主题？');">
        <input type="hidden" name="id" value="{id}">
        <input type="hidden" name="redirect" value="{redirect}">
        <button type="submit" class="danger">删除</button>
    </form>
</td></tr>"#,
                name = escape_html(&topic.name),
                description = description,
                created = created,
                id = topic.id,
                redirect = redirect
            ));
        }
    }

    format!(
        r##"<section class="admin">
    <h2>主题管理</h2>
    <p class="section-note">主题用于稿件关键词识别，可重复提交同名主题覆盖描述。</p>
    <table>
        <thead>
            <tr><th>主题名称</th><th>描述</th><th>创建时间</th><th>操作</th></tr>
        </thead>
        <tbody>
            {rows}
        </tbody>
    </table>
    <form method="post" action="/dashboard/journal-topics">
        <input type="hidden" name="redirect" value="{redirect}">
        <h3>新增或更新主题</h3>
        <div class="field">
            <label for="topic-name">主题名称</label>
            <input id="topic-name" name="name" required>
        </div>
        <div class="field">
            <label for="topic-description">描述（可选）</label>
            <input id="topic-description" name="description" placeholder="例如：用于描述简要范围">
        </div>
        <button type="submit">保存主题</button>
    </form>
</section>"##,
        rows = rows,
        redirect = redirect,
    )
}

fn render_journal_section(
    references: &[JournalReferenceRow],
    topics: &[JournalTopicRow],
    scores: &[JournalTopicScoreRow],
    redirect: &str,
) -> String {
    use std::collections::{HashMap, HashSet};

    let mut name_lookup: HashMap<Uuid, String> = HashMap::new();
    for topic in topics {
        name_lookup.insert(topic.id, topic.name.clone());
    }

    let valid_ids: HashSet<Uuid> = references.iter().map(|r| r.id).collect();
    let mut scores_map: HashMap<Uuid, Vec<(Uuid, String, i16)>> = HashMap::new();
    for score in scores {
        if !valid_ids.contains(&score.journal_id) {
            continue;
        }
        if let Some(name) = name_lookup.get(&score.topic_id) {
            scores_map.entry(score.journal_id).or_default().push((
                score.topic_id,
                name.clone(),
                score.score,
            ));
        }
    }

    for values in scores_map.values_mut() {
        values.sort_by(|a, b| a.1.cmp(&b.1));
    }

    let mut rows = String::new();
    if references.is_empty() {
        rows.push_str("<tr><td colspan=\\\"6\\\">尚未添加期刊参考。</td></tr>");
    } else {
        for reference in references {
            let mark = reference
                .reference_mark
                .as_ref()
                .map(|m| escape_html(m))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "—".to_string());
            let notes = reference
                .notes
                .as_ref()
                .map(|n| escape_html(n))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "—".to_string());
            let score_display = scores_map
                .get(&reference.id)
                .map(|entries| {
                    if entries.is_empty() {
                        "—".to_string()
                    } else {
                        entries
                            .iter()
                            .map(|(_, name, score)| format!("{}：{}", escape_html(name), score))
                            .collect::<Vec<_>>()
                            .join("<br>")
                    }
                })
                .unwrap_or_else(|| "—".to_string());

            let mut score_payload = serde_json::Map::new();
            if let Some(entries) = scores_map.get(&reference.id) {
                for (topic_id, _name, score) in entries {
                    score_payload.insert(topic_id.to_string(), json!(score));
                }
            }

            let payload_value = json!({
                "journal_name": &reference.journal_name,
                "reference_mark": &reference.reference_mark,
                "low_bound": reference.low_bound,
                "notes": &reference.notes,
                "scores": Value::Object(score_payload),
            });
            let payload_attr = escape_html(&payload_value.to_string());

            rows.push_str(&format!(
                r#"<tr><td>{name}</td><td>{mark}</td><td>{low:.2}</td><td>{notes}</td><td>{scores}</td><td>
    <div class="action-stack">
        <button type="button" class="secondary" data-load-journal="{payload}">载入表单</button>
        <form method="post" action="/dashboard/journal-references/delete" onsubmit="return confirm('确定删除该期刊参考？');">
            <input type="hidden" name="id" value="{id}">
            <input type="hidden" name="redirect" value="{redirect}">
            <button type="submit" class="danger">删除</button>
        </form>
    </div>
</td></tr>"#,
                name = escape_html(&reference.journal_name),
                mark = mark,
                low = reference.low_bound,
                notes = notes,
                scores = score_display,
                id = reference.id,
                redirect = redirect,
                payload = payload_attr
            ));
        }
    }

    let score_inputs = if topics.is_empty() {
        "<p class=\\\"section-note\\\">暂无主题，请先添加主题后再录入分值。</p>".to_string()
    } else {
        let fields = topics
            .iter()
            .map(|topic| {
                let mut options = String::new();
                for value in 0..=3 {
                    options.push_str(&format!(
                        "<option value=\\\"{value}\\\"{selected}>{value}</option>",
                        value = value,
                        selected = if value == 0 { " selected" } else { "" },
                    ));
                }
                format!(
                    r#"<div class=\"topic-picker\" data-topic=\"{id}\"><label for=\"score-{id}\">{name}</label><select id=\"score-{id}\" name=\"score_{id}\" data-topic-select=\"{id}\">{options}</select></div>"#,
                    id = topic.id,
                    name = escape_html(&topic.name),
                    options = options,
                )
            })
            .collect::<String>();
        format!(
            "<div class=\\\"topic-grid\\\">{fields}</div>",
            fields = fields
        )
    };

    let mut section_html = format!(
        r##"<section class="admin">
    <h2>期刊参考</h2>
    <p class="section-note">该列表支撑稿件评估模块的期刊推荐逻辑。</p>
    <table>
        <thead>
            <tr><th>期刊名称</th><th>参考标记</th><th>低区间阈值</th><th>备注</th><th>主题分值</th><th>操作</th></tr>
        </thead>
        <tbody>
            {rows}
        </tbody>
    </table>
    <form id="journal-form" method="post" action="/dashboard/journal-references">
        <input type="hidden" name="redirect" value="{redirect}">
        <h3>新增或更新期刊</h3>
        <div class="field">
            <label for="journal-name">期刊名称</label>
            <input id="journal-name" name="journal_name" required>
        </div>
        <div class="field">
            <label for="journal-mark">参考标记（可选）</label>
            <input id="journal-mark" name="reference_mark" placeholder="例如：Level 3 或 2/3">
        </div>
        <div class="field">
            <label for="journal-low">低区间阈值</label>
            <input id="journal-low" name="low_bound" required placeholder="例如：37.5">
        </div>
        <div class="field">
            <label for="journal-notes">备注（可选）</label>
            <input id="journal-notes" name="notes" placeholder="简要说明">
        </div>
        {score_inputs}
        <div class="journal-form-actions">
            <button type="submit">保存期刊</button>
            <button type="button" class="secondary" data-clear-journal-form>清空表单</button>
        </div>
    </form>
</section>"##,
        rows = rows,
        score_inputs = score_inputs,
        redirect = redirect,
    );

    let script = r#"
<script>
document.addEventListener('DOMContentLoaded', function () {
    const form = document.getElementById('journal-form');
    if (!form) { return; }
    const selects = Array.from(form.querySelectorAll('[data-topic-select]'));

    function updateHighlight(select) {
        const wrapper = select.closest('.topic-picker');
        if (!wrapper) { return; }
        const value = parseInt(select.value || '0', 10);
        if (!Number.isNaN(value) && value > 0) {
            wrapper.classList.add('active');
        } else {
            wrapper.classList.remove('active');
        }
    }

    selects.forEach((select) => {
        updateHighlight(select);
        select.addEventListener('change', () => updateHighlight(select));
    });

    document.querySelectorAll('[data-load-journal]').forEach((button) => {
        button.addEventListener('click', () => {
            const payloadRaw = button.getAttribute('data-load-journal');
            if (!payloadRaw) { return; }

            let data;
            try {
                data = JSON.parse(payloadRaw);
            } catch (error) {
                console.error('Failed to parse journal payload', error);
                return;
            }

            const nameInput = form.querySelector('#journal-name');
            const markInput = form.querySelector('#journal-mark');
            const lowInput = form.querySelector('#journal-low');
            const notesInput = form.querySelector('#journal-notes');

            if (nameInput) { nameInput.value = data.journal_name ? String(data.journal_name) : ''; }
            if (markInput) { markInput.value = data.reference_mark ? String(data.reference_mark) : ''; }
            if (lowInput) {
                if (data.low_bound === null || data.low_bound === undefined || data.low_bound === '') {
                    lowInput.value = '';
                } else {
                    lowInput.value = String(data.low_bound);
                }
            }
            if (notesInput) { notesInput.value = data.notes ? String(data.notes) : ''; }

            const scoreMap = (data.scores && typeof data.scores === 'object') ? data.scores : {};
            selects.forEach((select) => {
                const topicId = select.getAttribute('data-topic-select');
                const rawValue = topicId && Object.prototype.hasOwnProperty.call(scoreMap, topicId)
                    ? scoreMap[topicId]
                    : 0;
                select.value = String(rawValue ?? 0);
                select.dispatchEvent(new Event('change'));
            });

            if (nameInput) { nameInput.focus(); }
        });
    });

    const resetButton = form.querySelector('[data-clear-journal-form]');
    if (resetButton) {
        resetButton.addEventListener('click', () => {
            form.reset();
            selects.forEach((select) => {
                select.value = '0';
                select.dispatchEvent(new Event('change'));
            });
        });
    }
});
</script>
"#;

    section_html.push_str(script);
    section_html
}

async fn summarizer_admin(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<DashboardQuery>,
) -> Result<Html<String>, Redirect> {
    let auth_user = require_admin_user(&state, &jar).await?;
    let settings = state.summarizer_settings().await;
    let models = settings
        .as_ref()
        .map(|s| s.models.clone())
        .unwrap_or_default();
    let prompts = settings
        .as_ref()
        .map(|s| s.prompts.clone())
        .unwrap_or_default();

    let glossary_terms = fetch_glossary_terms(&state.pool)
        .await
        .unwrap_or_else(|err| {
            error!(?err, "failed to load glossary terms");
            Vec::new()
        });

    let message_block = compose_flash_message(&params);
    let models_redirect = "/dashboard/modules/summarizer";
    let glossary_html = render_glossary_section(&glossary_terms, models_redirect);
    let footer = render_footer();

    let shared_styles = MODULE_ADMIN_SHARED_STYLES;
    let html = format!(
        r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>摘要模块设置</title>
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
        .panel {{ background: #ffffff; border-radius: 12px; border: 1px solid #e2e8f0; padding: 1.5rem; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); margin-bottom: 2rem; }}
        label {{ display: block; margin-bottom: 0.5rem; font-weight: 600; color: #0f172a; }}
        input[type="text"], textarea {{ width: 100%; padding: 0.75rem; border-radius: 8px; border: 1px solid #cbd5f5; background: #f8fafc; color: #0f172a; box-sizing: border-box; font-family: inherit; }}
        textarea {{ min-height: 140px; }}
        input[type="text"]:focus, textarea:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        button {{ padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        .flash {{ padding: 1rem; border-radius: 8px; margin-bottom: 1.5rem; border: 1px solid transparent; }}
        .flash.success {{ background: #ecfdf3; border-color: #bbf7d0; color: #166534; }}
        .flash.error {{ background: #fef2f2; border-color: #fecaca; color: #b91c1c; }}
        .note {{ color: #475569; font-size: 0.95rem; margin-bottom: 1rem; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
{shared_styles}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>摘要模块设置</h1>
            <a class="back-link" href="/tools/summarizer">← 返回摘要工具</a>
        </div>
        <p>配置摘要与翻译调用的模型和提示词，术语表与 DOCX 模块共用。</p>
    </header>
    <main>
        <p>当前登录：<strong>{username}</strong></p>
        {message_block}
        <section class="panel">
            <h2>模型配置</h2>
            <form method="post" action="/dashboard/modules/summarizer/models">
                <input type="hidden" name="redirect" value="{models_redirect}">
                <label for="summary-model">摘要模型</label>
                <input id="summary-model" name="summary_model" type="text" value="{summary_model}" required>
                <label for="translation-model">翻译模型</label>
                <input id="translation-model" name="translation_model" type="text" value="{translation_model}" required>
                <button type="submit">保存模型</button>
            </form>
        </section>
        <section class="panel">
            <h2>提示词配置</h2>
            <form method="post" action="/dashboard/modules/summarizer/prompts">
                <input type="hidden" name="redirect" value="{models_redirect}">
                <label for="prompt-research">科研论文摘要提示</label>
                <textarea id="prompt-research" name="research_summary" required>{research_prompt}</textarea>
                <label for="prompt-general">其他文档摘要提示</label>
                <textarea id="prompt-general" name="general_summary" required>{general_prompt}</textarea>
                <label for="prompt-translation">翻译提示（需包含 {{GLOSSARY}} ）</label>
                <textarea id="prompt-translation" name="translation" required>{translation_prompt}</textarea>
                <button type="submit">保存提示词</button>
            </form>
        </section>
        {glossary_html}
        {footer}
    </main>
</body>
</html>"##,
        username = escape_html(&auth_user.username),
        message_block = message_block,
        models_redirect = models_redirect,
        summary_model = escape_html(&models.summary_model),
        translation_model = escape_html(&models.translation_model),
        research_prompt = escape_html(&prompts.research_summary),
        general_prompt = escape_html(&prompts.general_summary),
        translation_prompt = escape_html(&prompts.translation),
        glossary_html = glossary_html,
        footer = footer,
        shared_styles = shared_styles,
    );

    Ok(Html(html))
}

async fn save_summarizer_models(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<SummarizerModelForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    let summary = form.summary_model.trim();
    let translation = form.translation_model.trim();
    if summary.is_empty() || translation.is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=summarizer_invalid_models"
        )));
    }

    let payload = SummarizerModels {
        summary_model: summary.to_string(),
        translation_model: translation.to_string(),
    };

    if let Err(err) = update_summarizer_models(&state.pool, &payload).await {
        error!(?err, "failed to update summarizer models");
        return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
    }

    if let Err(err) = state.reload_settings().await {
        error!(
            ?err,
            "failed to reload module settings after summarizer model update"
        );
    }

    Ok(Redirect::to(&format!(
        "{redirect_base}?status=summarizer_models_saved"
    )))
}

async fn save_summarizer_prompts(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<SummarizerPromptForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    if form.research_summary.trim().is_empty()
        || form.general_summary.trim().is_empty()
        || form.translation.trim().is_empty()
    {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=summarizer_invalid_prompts"
        )));
    }

    if !form.translation.contains("{{GLOSSARY}}") {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=summarizer_invalid_prompts"
        )));
    }

    let payload = SummarizerPrompts {
        research_summary: form.research_summary.trim().to_string(),
        general_summary: form.general_summary.trim().to_string(),
        translation: form.translation.trim().to_string(),
    };

    if let Err(err) = update_summarizer_prompts(&state.pool, &payload).await {
        error!(?err, "failed to update summarizer prompts");
        return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
    }

    if let Err(err) = state.reload_settings().await {
        error!(
            ?err,
            "failed to reload module settings after summarizer prompt update"
        );
    }

    Ok(Redirect::to(&format!(
        "{redirect_base}?status=summarizer_prompts_saved"
    )))
}

async fn docx_admin(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<DashboardQuery>,
) -> Result<Html<String>, Redirect> {
    let auth_user = require_admin_user(&state, &jar).await?;
    let settings = state.translate_docx_settings().await;
    let models = settings
        .as_ref()
        .map(|s| s.models.clone())
        .unwrap_or_default();
    let prompts = settings
        .as_ref()
        .map(|s| s.prompts.clone())
        .unwrap_or_default();

    let glossary_terms = fetch_glossary_terms(&state.pool)
        .await
        .unwrap_or_else(|err| {
            error!(?err, "failed to load glossary terms");
            Vec::new()
        });

    let message_block = compose_flash_message(&params);
    let redirect_base = "/dashboard/modules/translatedocx";
    let glossary_html = render_glossary_section(&glossary_terms, redirect_base);
    let footer = render_footer();

    let shared_styles = MODULE_ADMIN_SHARED_STYLES;
    let html = format!(
        r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>DOCX 模块设置</title>
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
        .panel {{ background: #ffffff; border-radius: 12px; border: 1px solid #e2e8f0; padding: 1.5rem; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); margin-bottom: 2rem; }}
        label {{ display: block; margin-bottom: 0.5rem; font-weight: 600; color: #0f172a; }}
        input[type="text"], textarea {{ width: 100%; padding: 0.75rem; border-radius: 8px; border: 1px solid #cbd5f5; background: #f8fafc; color: #0f172a; box-sizing: border-box; font-family: inherit; }}
        textarea {{ min-height: 140px; }}
        input[type="text"]:focus, textarea:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        button {{ padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        .flash {{ padding: 1rem; border-radius: 8px; margin-bottom: 1.5rem; border: 1px solid transparent; }}
        .flash.success {{ background: #ecfdf3; border-color: #bbf7d0; color: #166534; }}
        .flash.error {{ background: #fef2f2; border-color: #fecaca; color: #b91c1c; }}
        .note {{ color: #475569; font-size: 0.95rem; margin-bottom: 1rem; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
{shared_styles}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>DOCX 模块设置</h1>
            <a class="back-link" href="/tools/translatedocx">← 返回 DOCX 工具</a>
        </div>
        <p>配置 DOCX 翻译调用的模型和提示词。术语表与摘要模块共用。</p>
    </header>
    <main>
        <p>当前登录：<strong>{username}</strong></p>
        {message_block}
        <section class="panel">
            <h2>模型配置</h2>
            <form method="post" action="/dashboard/modules/translatedocx/models">
                <input type="hidden" name="redirect" value="{redirect_base}">
                <label for="docx-model">翻译模型</label>
                <input id="docx-model" name="translation_model" type="text" value="{translation_model}" required>
                <button type="submit">保存模型</button>
            </form>
        </section>
        <section class="panel">
            <h2>提示词配置</h2>
            <form method="post" action="/dashboard/modules/translatedocx/prompts">
                <input type="hidden" name="redirect" value="{redirect_base}">
                <label for="docx-en-cn">英译中提示（需包含 {{GLOSSARY}} 与 {{PARAGRAPH_SEPARATOR}}）</label>
                <textarea id="docx-en-cn" name="en_to_cn" required>{en_cn_prompt}</textarea>
                <label for="docx-cn-en">中译英提示（需包含 {{GLOSSARY}} 与 {{PARAGRAPH_SEPARATOR}}）</label>
                <textarea id="docx-cn-en" name="cn_to_en" required>{cn_en_prompt}</textarea>
                <button type="submit">保存提示词</button>
            </form>
        </section>
        {glossary_html}
        {footer}
    </main>
</body>
</html>"##,
        username = escape_html(&auth_user.username),
        message_block = message_block,
        redirect_base = redirect_base,
        translation_model = escape_html(&models.translation_model),
        en_cn_prompt = escape_html(&prompts.en_to_cn),
        cn_en_prompt = escape_html(&prompts.cn_to_en),
        glossary_html = glossary_html,
        footer = footer,
        shared_styles = shared_styles,
    );

    Ok(Html(html))
}

async fn save_docx_models(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<DocxModelForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    let model = form.translation_model.trim();
    if model.is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=docx_invalid_models"
        )));
    }

    let payload = DocxTranslatorModels {
        translation_model: model.to_string(),
    };

    if let Err(err) = update_docx_models(&state.pool, &payload).await {
        error!(?err, "failed to update docx models");
        return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
    }

    if let Err(err) = state.reload_settings().await {
        error!(
            ?err,
            "failed to reload module settings after docx model update"
        );
    }

    Ok(Redirect::to(&format!(
        "{redirect_base}?status=docx_models_saved"
    )))
}

async fn save_docx_prompts(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<DocxPromptForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    let en_to_cn = form.en_to_cn.trim();
    let cn_to_en = form.cn_to_en.trim();

    let required = en_to_cn.is_empty()
        || cn_to_en.is_empty()
        || !en_to_cn.contains("{{GLOSSARY}}")
        || !en_to_cn.contains("{{PARAGRAPH_SEPARATOR}}")
        || !cn_to_en.contains("{{GLOSSARY}}")
        || !cn_to_en.contains("{{PARAGRAPH_SEPARATOR}}");

    if required {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=docx_invalid_prompts"
        )));
    }

    let payload = DocxTranslatorPrompts {
        en_to_cn: en_to_cn.to_string(),
        cn_to_en: cn_to_en.to_string(),
    };

    if let Err(err) = update_docx_prompts(&state.pool, &payload).await {
        error!(?err, "failed to update docx prompts");
        return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
    }

    if let Err(err) = state.reload_settings().await {
        error!(
            ?err,
            "failed to reload module settings after docx prompt update"
        );
    }

    Ok(Redirect::to(&format!(
        "{redirect_base}?status=docx_prompts_saved"
    )))
}

async fn grader_admin(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<DashboardQuery>,
) -> Result<Html<String>, Redirect> {
    let auth_user = require_admin_user(&state, &jar).await?;
    let settings = state.grader_settings().await;
    let models = settings
        .as_ref()
        .map(|s| s.models.clone())
        .unwrap_or_default();
    let prompts = settings
        .as_ref()
        .map(|s| s.prompts.clone())
        .unwrap_or_default();

    let topics = fetch_journal_topics(&state.pool)
        .await
        .unwrap_or_else(|err| {
            error!(?err, "failed to load journal topics");
            Vec::new()
        });
    let references = fetch_journal_references(&state.pool)
        .await
        .unwrap_or_else(|err| {
            error!(?err, "failed to load journal references");
            Vec::new()
        });
    let topic_scores = fetch_journal_topic_scores(&state.pool)
        .await
        .unwrap_or_else(|err| {
            error!(?err, "failed to load journal topic scores");
            Vec::new()
        });

    let message_block = compose_flash_message(&params);
    let redirect_base = "/dashboard/modules/grader";
    let topic_html = render_topic_section(&topics, redirect_base);
    let journal_html = render_journal_section(&references, &topics, &topic_scores, redirect_base);
    let footer = render_footer();

    let shared_styles = MODULE_ADMIN_SHARED_STYLES;
    let html = format!(
        r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>稿件评估模块设置</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="robots" content="noindex,nofollow">
    <style>
        :root {{ color-scheme: light; }}
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; margin: 0; background: #f8fafc; color: #0f172a; }}
        header {{ background: #ffffff; padding: 2rem 1.5rem; border-bottom: 1px solid #e2e8f0; }}
        .header-bar {{ display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 1rem; }}
        .back-link {{ display: inline-flex; align-items: center; gap: 0.4rem; color: #1d4ed8; text-decoration: none; font-weight: 600; background: #e0f2fe; padding: 0.5rem 0.95rem; border-radius: 999px; border: 1px solid #bfdbfe; transition: background 0.15s ease, border 0.15s ease; }}
        .back-link:hover {{ background: #bfdbfe; border-color: #93c5fd; }}
        main {{ padding: 2rem 1.5rem; max-width: 1100px; margin: 0 auto; box-sizing: border-box; }}
        .panel {{ background: #ffffff; border-radius: 12px; border: 1px solid #e2e8f0; padding: 1.5rem; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); margin-bottom: 2rem; }}
        label {{ display: block; margin-bottom: 0.5rem; font-weight: 600; color: #0f172a; }}
        input[type="text"], textarea {{ width: 100%; padding: 0.75rem; border-radius: 8px; border: 1px solid #cbd5f5; background: #f8fafc; color: #0f172a; box-sizing: border-box; font-family: inherit; }}
        textarea {{ min-height: 160px; }}
        input[type="text"]:focus, textarea:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        button {{ padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        .flash {{ padding: 1rem; border-radius: 8px; margin-bottom: 1.5rem; border: 1px solid transparent; }}
        .flash.success {{ background: #ecfdf3; border-color: #bbf7d0; color: #166534; }}
        .flash.error {{ background: #fef2f2; border-color: #fecaca; color: #b91c1c; }}
        table {{ width: 100%; border-collapse: collapse; margin-top: 1.5rem; background: #ffffff; border: 1px solid #e2e8f0; border-radius: 12px; overflow: hidden; }}
        th, td {{ padding: 0.75rem 1rem; border-bottom: 1px solid #e2e8f0; text-align: left; }}
        th {{ background: #f1f5f9; color: #0f172a; font-weight: 600; }}
        .topic-grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(190px, 1fr)); gap: 1rem; margin-bottom: 1rem; }}
        .topic-picker {{ display: flex; flex-direction: column; gap: 0.45rem; padding: 0.75rem; background: #f8fafc; border: 1px solid #dbeafe; border-radius: 10px; transition: border 0.15s ease, box-shadow 0.15s ease, background 0.15s ease; }}
        .topic-picker select {{ padding: 0.6rem; border-radius: 8px; border: 1px solid #cbd5f5; background: #ffffff; }}
        .topic-picker.active {{ border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); background: #eff6ff; }}
        .journal-form-actions {{ display: flex; flex-wrap: wrap; gap: 0.75rem; margin-top: 1rem; }}
        button.secondary {{ background: #ffffff; color: #1d4ed8; border: 1px solid #93c5fd; }}
        button.secondary:hover {{ background: #dbeafe; }}
        .action-stack {{ display: flex; flex-direction: column; gap: 0.5rem; }}
        .action-stack form {{ margin: 0; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
{shared_styles}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>稿件评估模块设置</h1>
            <a class="back-link" href="/tools/grader">← 返回评估工具</a>
        </div>
        <p>配置稿件评估与期刊匹配使用的模型、提示词、主题与期刊阈值。</p>
    </header>
    <main>
        <p>当前登录：<strong>{username}</strong></p>
        {message_block}
        <section class="panel">
            <h2>模型配置</h2>
            <form method="post" action="/dashboard/modules/grader/models">
                <input type="hidden" name="redirect" value="{redirect_base}">
                <label for="grader-model">评分模型</label>
                <input id="grader-model" name="grading_model" type="text" value="{grading_model}" required>
                <label for="keyword-model">关键词模型</label>
                <input id="keyword-model" name="keyword_model" type="text" value="{keyword_model}" required>
                <button type="submit">保存模型</button>
            </form>
        </section>
        <section class="panel">
            <h2>提示词配置</h2>
            <form method="post" action="/dashboard/modules/grader/prompts">
                <input type="hidden" name="redirect" value="{redirect_base}">
                <label for="grader-instructions">评分提示词</label>
                <textarea id="grader-instructions" name="grading_instructions" required>{grading_prompt}</textarea>
                <label for="keyword-selection">关键词识别提示词</label>
                <textarea id="keyword-selection" name="keyword_selection" required>{keyword_prompt}</textarea>
                <button type="submit">保存提示词</button>
            </form>
        </section>
        {topic_html}
        {journal_html}
        {footer}
    </main>
</body>
</html>"##,
        username = escape_html(&auth_user.username),
        message_block = message_block,
        redirect_base = redirect_base,
        grading_model = escape_html(&models.grading_model),
        keyword_model = escape_html(&models.keyword_model),
        grading_prompt = escape_html(&prompts.grading_instructions),
        keyword_prompt = escape_html(&prompts.keyword_selection),
        topic_html = topic_html,
        journal_html = journal_html,
        footer = footer,
        shared_styles = shared_styles,
    );

    Ok(Html(html))
}

async fn save_grader_models(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GraderModelForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    let grading = form.grading_model.trim();
    let keyword = form.keyword_model.trim();
    if grading.is_empty() || keyword.is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=grader_invalid_models"
        )));
    }

    let payload = GraderModels {
        grading_model: grading.to_string(),
        keyword_model: keyword.to_string(),
    };

    if let Err(err) = update_grader_models(&state.pool, &payload).await {
        error!(?err, "failed to update grader models");
        return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
    }

    if let Err(err) = state.reload_settings().await {
        error!(
            ?err,
            "failed to reload module settings after grader model update"
        );
    }

    Ok(Redirect::to(&format!(
        "{redirect_base}?status=grader_models_saved"
    )))
}

async fn save_grader_prompts(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GraderPromptForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    if form.grading_instructions.trim().is_empty() || form.keyword_selection.trim().is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=grader_invalid_prompts"
        )));
    }

    let payload = GraderPrompts {
        grading_instructions: form.grading_instructions.trim().to_string(),
        keyword_selection: form.keyword_selection.trim().to_string(),
    };

    if let Err(err) = update_grader_prompts(&state.pool, &payload).await {
        error!(?err, "failed to update grader prompts");
        return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
    }

    if let Err(err) = state.reload_settings().await {
        error!(
            ?err,
            "failed to reload module settings after grader prompt update"
        );
    }

    Ok(Redirect::to(&format!(
        "{redirect_base}?status=grader_prompts_saved"
    )))
}

async fn create_user(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<CreateUserForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;

    let username = form.username.trim();
    if username.is_empty() {
        return Ok(Redirect::to("/dashboard?error=missing_username"));
    }

    let password = form.password.trim();
    if password.is_empty() {
        return Ok(Redirect::to("/dashboard?error=missing_password"));
    }

    let group_id = match form.usage_group_id {
        Some(id) => id,
        None => return Ok(Redirect::to("/dashboard?error=group_missing")),
    };

    let is_admin = form.is_admin.is_some();

    let password_hash = match hash_password(password) {
        Ok(hash) => hash,
        Err(err) => {
            error!(?err, "failed to hash password while creating user");
            return Ok(Redirect::to("/dashboard?error=hash_failed"));
        }
    };

    let result = sqlx::query(
        "INSERT INTO users (id, username, password_hash, usage_group_id, is_admin) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(username)
    .bind(password_hash)
    .bind(group_id)
    .bind(is_admin)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => Ok(Redirect::to("/dashboard?status=created")),
        Err(sqlx::Error::Database(db_err)) if db_err.constraint() == Some("users_username_key") => {
            Ok(Redirect::to("/dashboard?error=duplicate"))
        }
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("23503") => {
            Ok(Redirect::to("/dashboard?error=group_missing"))
        }
        Err(err) => {
            error!(?err, "failed to create user");
            Ok(Redirect::to("/dashboard?error=unknown"))
        }
    }
}

async fn update_user_password(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<UpdatePasswordForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;

    let username = form.username.trim();
    if username.is_empty() {
        return Ok(Redirect::to("/dashboard?error=user_missing"));
    }

    let new_password = form.password.trim();
    if new_password.is_empty() {
        return Ok(Redirect::to("/dashboard?error=password_missing"));
    }

    let password_hash = match hash_password(new_password) {
        Ok(hash) => hash,
        Err(err) => {
            error!(?err, "failed to hash password during reset");
            return Ok(Redirect::to("/dashboard?error=hash_failed"));
        }
    };

    match sqlx::query("UPDATE users SET password_hash = $2 WHERE username = $1")
        .bind(username)
        .bind(password_hash)
        .execute(&state.pool)
        .await
    {
        Ok(result) if result.rows_affected() > 0 => {
            Ok(Redirect::to("/dashboard?status=password_updated"))
        }
        Ok(_) => Ok(Redirect::to("/dashboard?error=user_missing")),
        Err(err) => {
            error!(?err, "failed to update user password");
            Ok(Redirect::to("/dashboard?error=unknown"))
        }
    }
}

async fn assign_user_group(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<AssignUserGroupForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;

    let username = form.username.trim();
    if username.is_empty() {
        return Ok(Redirect::to("/dashboard?error=user_missing"));
    }

    let result = sqlx::query("UPDATE users SET usage_group_id = $2 WHERE username = $1")
        .bind(username)
        .bind(form.usage_group_id)
        .execute(&state.pool)
        .await;

    match result {
        Ok(res) if res.rows_affected() > 0 => Ok(Redirect::to("/dashboard?status=group_assigned")),
        Ok(_) => Ok(Redirect::to("/dashboard?error=user_missing")),
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("23503") => {
            Ok(Redirect::to("/dashboard?error=group_missing"))
        }
        Err(err) => {
            error!(?err, "failed to assign usage group");
            Ok(Redirect::to("/dashboard?error=unknown"))
        }
    }
}

async fn save_usage_group(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(mut form): Form<HashMap<String, String>>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;

    let name = form.remove("name").unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return Ok(Redirect::to("/dashboard?error=group_name_missing"));
    }

    let description = form
        .remove("description")
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());

    let group_id_value = form
        .remove("group_id")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let (group_id, existing) = if let Some(raw) = group_id_value {
        let parsed =
            Uuid::parse_str(&raw).map_err(|_| Redirect::to("/dashboard?error=group_invalid"))?;
        (parsed, true)
    } else {
        (Uuid::new_v4(), false)
    };

    let mut allocations: HashMap<String, (Option<i64>, Option<i64>)> = HashMap::new();
    for module in usage::REGISTERED_MODULES {
        let unit_key = format!("units_{}", module.key);
        let token_key = format!("tokens_{}", module.key);

        let unit_limit = match usage::parse_optional_limit(form.get(&unit_key).map(String::as_str))
        {
            Ok(value) => value,
            Err(_) => return Ok(Redirect::to("/dashboard?error=group_invalid_limit")),
        };
        let token_limit =
            match usage::parse_optional_limit(form.get(&token_key).map(String::as_str)) {
                Ok(value) => value,
                Err(_) => return Ok(Redirect::to("/dashboard?error=group_invalid_limit")),
            };

        allocations.insert(module.key.to_string(), (token_limit, unit_limit));
    }

    if existing {
        let updated =
            sqlx::query("UPDATE usage_groups SET name = $2, description = $3 WHERE id = $1")
                .bind(group_id)
                .bind(&name)
                .bind(description.as_deref())
                .execute(&state.pool)
                .await
                .map_err(|err| {
                    if let sqlx::Error::Database(db_err) = &err {
                        if db_err.code().as_deref() == Some("23505") {
                            return Redirect::to("/dashboard?error=group_duplicate");
                        }
                    }
                    error!(?err, "failed to update usage group");
                    Redirect::to("/dashboard?error=unknown")
                })?;

        if updated.rows_affected() == 0 {
            return Ok(Redirect::to("/dashboard?error=group_missing"));
        }
    } else {
        if let Err(err) =
            sqlx::query("INSERT INTO usage_groups (id, name, description) VALUES ($1, $2, $3)")
                .bind(group_id)
                .bind(&name)
                .bind(description.as_deref())
                .execute(&state.pool)
                .await
        {
            if let sqlx::Error::Database(db_err) = &err {
                if db_err.code().as_deref() == Some("23505") {
                    return Ok(Redirect::to("/dashboard?error=group_duplicate"));
                }
            }
            error!(?err, "failed to create usage group");
            return Ok(Redirect::to("/dashboard?error=unknown"));
        }
    }

    if let Err(err) = usage::upsert_group_limits(&state.pool, group_id, &allocations).await {
        error!(?err, "failed to update usage group limits");
        return Ok(Redirect::to("/dashboard?error=unknown"));
    }

    let status = if existing {
        "group_saved"
    } else {
        "group_created"
    };
    Ok(Redirect::to(&format!("/dashboard?status={status}")))
}

async fn create_glossary_term(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GlossaryCreateForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;

    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    let source_clean = form.source_term.trim().to_owned();
    let target_clean = form.target_term.trim().to_owned();

    if source_clean.is_empty() || target_clean.is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=glossary_missing_fields"
        )));
    }

    let notes_clean = form
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    let insert_result = sqlx::query(
        "INSERT INTO glossary_terms (id, source_term, target_term, notes) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::new_v4())
    .bind(&source_clean)
    .bind(&target_clean)
    .bind(notes_clean.as_deref())
    .execute(&state.pool)
    .await;

    match insert_result {
        Ok(_) => Ok(Redirect::to(&format!(
            "{redirect_base}?status=glossary_created"
        ))),
        Err(sqlx::Error::Database(db_err))
            if db_err.constraint() == Some("idx_glossary_terms_source_lower") =>
        {
            Ok(Redirect::to(&format!(
                "{redirect_base}?error=glossary_duplicate"
            )))
        }
        Err(err) => {
            error!(?err, "failed to insert glossary term");
            Ok(Redirect::to(&format!("{redirect_base}?error=unknown")))
        }
    }
}

async fn update_glossary_term(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GlossaryUpdateForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    let source_clean = form.source_term.trim().to_owned();
    let target_clean = form.target_term.trim().to_owned();

    if source_clean.is_empty() || target_clean.is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=glossary_missing_fields"
        )));
    }

    let notes_clean = form
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let update_result = sqlx::query(
        "UPDATE glossary_terms SET source_term = $2, target_term = $3, notes = $4 WHERE id = $1",
    )
    .bind(form.id)
    .bind(&source_clean)
    .bind(&target_clean)
    .bind(notes_clean.as_deref())
    .execute(&state.pool)
    .await;

    match update_result {
        Ok(result) if result.rows_affected() > 0 => Ok(Redirect::to(&format!(
            "{redirect_base}?status=glossary_updated"
        ))),
        Ok(_) => Ok(Redirect::to(&format!(
            "{redirect_base}?error=glossary_not_found"
        ))),
        Err(sqlx::Error::Database(db_err))
            if db_err.constraint() == Some("idx_glossary_terms_source_lower") =>
        {
            Ok(Redirect::to(&format!(
                "{redirect_base}?error=glossary_duplicate"
            )))
        }
        Err(err) => {
            error!(?err, "failed to update glossary term");
            Ok(Redirect::to(&format!("{redirect_base}?error=unknown")))
        }
    }
}

async fn delete_glossary_term(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GlossaryDeleteForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    let delete_result = sqlx::query("DELETE FROM glossary_terms WHERE id = $1")
        .bind(form.id)
        .execute(&state.pool)
        .await;

    match delete_result {
        Ok(result) if result.rows_affected() > 0 => Ok(Redirect::to(&format!(
            "{redirect_base}?status=glossary_deleted"
        ))),
        Ok(_) => Ok(Redirect::to(&format!(
            "{redirect_base}?error=glossary_not_found"
        ))),
        Err(err) => {
            error!(?err, "failed to delete glossary term");
            Ok(Redirect::to(&format!("{redirect_base}?error=unknown")))
        }
    }
}

async fn upsert_journal_topic(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<JournalTopicUpsertForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    let name = form.name.trim();
    if name.is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=topic_missing_name"
        )));
    }

    let description = form
        .description
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    let result = sqlx::query(
        "INSERT INTO journal_topics (id, name, description) VALUES ($1, $2, $3)
         ON CONFLICT (name) DO UPDATE SET description = EXCLUDED.description",
    )
    .bind(Uuid::new_v4())
    .bind(name)
    .bind(description.as_deref())
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => Ok(Redirect::to(&format!("{redirect_base}?status=topic_saved"))),
        Err(err) => {
            error!(?err, "failed to upsert journal topic");
            Ok(Redirect::to(&format!("{redirect_base}?error=unknown")))
        }
    }
}

async fn delete_journal_topic(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<JournalTopicDeleteForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    match sqlx::query("DELETE FROM journal_topics WHERE id = $1")
        .bind(form.id)
        .execute(&state.pool)
        .await
    {
        Ok(result) if result.rows_affected() > 0 => Ok(Redirect::to(&format!(
            "{redirect_base}?status=topic_deleted"
        ))),
        Ok(_) => Ok(Redirect::to(&format!(
            "{redirect_base}?error=topic_not_found"
        ))),
        Err(err) => {
            error!(?err, "failed to delete journal topic");
            Ok(Redirect::to(&format!("{redirect_base}?error=unknown")))
        }
    }
}

async fn upsert_journal_reference(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<JournalReferenceUpsertForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    let name = form.journal_name.trim();
    if name.is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=journal_missing_name"
        )));
    }

    let low_bound_value: f64 = match form.low_bound.trim().parse() {
        Ok(value) => value,
        Err(_) => {
            return Ok(Redirect::to(&format!(
                "{redirect_base}?error=journal_invalid_low"
            )));
        }
    };

    let reference_mark = form
        .reference_mark
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let notes = form
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    let mut parsed_scores = Vec::new();
    for (key, value) in &form.scores {
        if let Some(id_part) = key.strip_prefix("score_") {
            if let Ok(topic_id) = Uuid::parse_str(id_part) {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let score: i16 = match trimmed.parse() {
                    Ok(val) => val,
                    Err(_) => {
                        return Ok(Redirect::to(&format!(
                            "{redirect_base}?error=journal_invalid_score"
                        )));
                    }
                };

                if !(0..=9).contains(&score) {
                    return Ok(Redirect::to(&format!(
                        "{redirect_base}?error=journal_invalid_score"
                    )));
                }

                parsed_scores.push((topic_id, score));
            }
        }
    }

    let mut transaction = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(err) => {
            error!(?err, "failed to begin transaction for journal reference");
            return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
        }
    };

    let journal_id = match sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO journal_reference_entries (id, journal_name, reference_mark, low_bound, notes) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (journal_name)
         DO UPDATE SET reference_mark = EXCLUDED.reference_mark,
                       low_bound = EXCLUDED.low_bound,
                       notes = EXCLUDED.notes,
                       updated_at = NOW()
         RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(name)
    .bind(reference_mark.as_deref())
    .bind(low_bound_value)
    .bind(notes.as_deref())
    .fetch_one(&mut *transaction)
    .await
    {
        Ok(id) => id,
        Err(err) => {
            error!(?err, "failed to upsert journal reference entry");
            let _ = transaction.rollback().await;
            return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
        }
    };

    if let Err(err) = sqlx::query("DELETE FROM journal_topic_scores WHERE journal_id = $1")
        .bind(journal_id)
        .execute(&mut *transaction)
        .await
    {
        error!(?err, "failed to clear existing journal topic scores");
        let _ = transaction.rollback().await;
        return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
    }

    for (topic_id, score) in parsed_scores.into_iter().filter(|(_, s)| *s > 0) {
        if let Err(err) = sqlx::query(
            "INSERT INTO journal_topic_scores (journal_id, topic_id, score) VALUES ($1, $2, $3)",
        )
        .bind(journal_id)
        .bind(topic_id)
        .bind(score)
        .execute(&mut *transaction)
        .await
        {
            error!(?err, "failed to insert journal topic score");
            let _ = transaction.rollback().await;
            return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
        }
    }

    if let Err(err) = transaction.commit().await {
        error!(?err, "failed to commit journal reference transaction");
        return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
    }

    Ok(Redirect::to(&format!(
        "{redirect_base}?status=journal_saved"
    )))
}

async fn delete_journal_reference(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<JournalReferenceDeleteForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_redirect_path(form.redirect.as_deref());

    match sqlx::query("DELETE FROM journal_reference_entries WHERE id = $1")
        .bind(form.id)
        .execute(&state.pool)
        .await
    {
        Ok(result) if result.rows_affected() > 0 => Ok(Redirect::to(&format!(
            "{redirect_base}?status=journal_deleted"
        ))),
        Ok(_) => Ok(Redirect::to(&format!(
            "{redirect_base}?error=journal_not_found"
        ))),
        Err(err) => {
            error!(?err, "failed to delete journal reference entry");
            Ok(Redirect::to(&format!("{redirect_base}?error=unknown")))
        }
    }
}

pub(crate) fn render_footer() -> String {
    let current_year = Utc::now().year();
    format!(
        "<footer class=\"app-footer\">© 2024-{year} 张圆教授课题组，仅限内部使用</footer>",
        year = current_year
    )
}

fn render_main_page(user: &AuthUser, params: &LandingQuery) -> String {
    let username = escape_html(&user.username);
    let flash = compose_landing_flash(params);
    let footer = render_footer();

    let modules = [
        (
            "文档摘要与翻译",
            "上传 PDF、Word 或文本文件，生成结构化摘要并输出中文译文。",
            "/tools/summarizer",
        ),
        (
            "DOCX 文档翻译",
            "上传 Word 文档，利用术语表逐段翻译。",
            "/tools/translatedocx",
        ),
        (
            "稿件评估与期刊推荐",
            "评估稿件投稿级别并给出匹配期刊建议。",
            "/tools/grader",
        ),
    ];

    let module_cards = modules
        .iter()
        .map(|(title, description, href)| {
            format!(
                "<a class=\"module-card\" href=\"{href}\"><h2>{title}</h2><p>{description}</p><span class=\"cta\">进入工具 →</span></a>",
                title = escape_html(title),
                description = escape_html(description),
                href = href,
            )
        })
        .collect::<String>();

    let admin_button = if user.is_admin {
        "<a class=\"admin-link\" href=\"/dashboard\">管理后台</a>".to_string()
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>张圆教授课题组 AI 工具箱</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="robots" content="noindex,nofollow">
    <style>
        :root {{ color-scheme: light; }}
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; margin: 0; background: #f8fafc; color: #0f172a; min-height: 100vh; display: flex; flex-direction: column; }}
        header {{ background: #ffffff; padding: clamp(2rem, 4vw, 2.75rem) clamp(1.5rem, 6vw, 3rem); display: flex; flex-direction: column; gap: 1rem; border-bottom: 1px solid #e2e8f0; }}
        .header-top {{ display: flex; flex-direction: column; gap: 0.5rem; }}
        .header-top h1 {{ margin: 0; font-size: clamp(1.9rem, 3vw, 2.4rem); }}
        .header-top p {{ margin: 0; color: #64748b; }}
        .header-actions {{ display: flex; flex-wrap: wrap; align-items: center; gap: 1rem; }}
        .header-actions span {{ color: #475569; font-size: 0.95rem; }}
        .logout-form button {{ padding: 0.6rem 1.3rem; border: none; border-radius: 999px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        .logout-form button:hover {{ background: #1d4ed8; }}
        main {{ flex: 1; padding: clamp(2rem, 5vw, 3rem); max-width: 1100px; margin: 0 auto; width: 100%; box-sizing: border-box; }}
        .flash {{ padding: 1rem 1.25rem; border-radius: 10px; margin-bottom: 1.5rem; font-weight: 600; border: 1px solid transparent; }}
        .flash.success {{ background: #ecfdf3; border-color: #bbf7d0; color: #166534; }}
        .flash.error {{ background: #fef2f2; border-color: #fecaca; color: #b91c1c; }}
        .modules-grid {{ display: grid; gap: 1.5rem; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); }}
        .module-card {{ display: block; background: #ffffff; padding: 1.75rem; border-radius: 16px; text-decoration: none; color: inherit; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); transition: transform 0.15s ease, box-shadow 0.15s ease, border 0.15s ease; border: 1px solid #e2e8f0; }}
        .module-card:hover {{ transform: translateY(-4px); box-shadow: 0 24px 55px rgba(15, 23, 42, 0.12); border-color: #bfdbfe; }}
        .module-card h2 {{ margin-top: 0; margin-bottom: 0.75rem; font-size: 1.25rem; }}
        .module-card p {{ margin: 0 0 1.25rem 0; color: #475569; font-size: 0.95rem; line-height: 1.6; }}
        .module-card .cta {{ font-weight: 600; color: #2563eb; }}
        .admin-link {{ display: inline-flex; align-items: center; justify-content: center; margin-top: 2.5rem; padding: 0.85rem 1.5rem; border-radius: 12px; background: #e0f2fe; color: #1d4ed8; text-decoration: none; font-weight: 600; border: 1px solid #bfdbfe; transition: background 0.15s ease, border 0.15s ease; }}
        .admin-link:hover {{ background: #bfdbfe; border-color: #93c5fd; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
    </style>
</head>
<body>
    <header>
        <div class="header-top">
            <h1>张圆教授课题组 AI 工具箱</h1>
            <p>请选择功能模块开始使用。</p>
        </div>
        <div class="header-actions">
            <span>当前登录：<strong>{username}</strong></span>
            <form class="logout-form" method="post" action="/logout">
                <button type="submit">退出登录</button>
            </form>
        </div>
    </header>
    <main>
        {flash}
        <div class="modules-grid">
            {module_cards}
        </div>
        {admin_button}
        {footer}
    </main>
</body>
</html>"#,
        username = username,
        flash = flash,
        module_cards = module_cards,
        admin_button = admin_button,
        footer = footer,
    )
}

fn compose_landing_flash(params: &LandingQuery) -> String {
    if let Some(status) = params.status.as_deref() {
        if status == "logged_out" {
            return "<div class=\"flash success\">已退出登录。</div>".to_string();
        }
    }

    if let Some(error) = params.error.as_deref() {
        let message = match error {
            "not_authorized" => "该操作需要管理员权限。",
            _ => "发生未知错误，请稍后重试。",
        };

        return format!("<div class=\"flash error\">{message}</div>");
    }

    String::new()
}

fn invalid_credentials() -> (StatusCode, Html<String>) {
    (
        StatusCode::UNAUTHORIZED,
        Html(
            r##"<!DOCTYPE html><html lang=\"zh-CN\"><head><meta charset=\"UTF-8\"><meta name=\"robots\" content=\"noindex,nofollow\"><title>登录失败</title></head><body><h3>账号或密码错误</h3><p><a href=\"/login\">返回登录</a></p></body></html>"##
                .to_string(),
        ),
    )
}

fn server_error() -> (StatusCode, Html<String>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(
            r##"<!DOCTYPE html><html lang=\"zh-CN\"><head><meta charset=\"UTF-8\"><meta name=\"robots\" content=\"noindex,nofollow\"><title>系统错误</title></head><body><h3>系统出现问题</h3><p>请稍后再试。</p></body></html>"##
                .to_string(),
        ),
    )
}

fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2.hash_password(password.as_bytes(), &salt)?;
    Ok(password_hash.to_string())
}

fn verify_password(password: &str, password_hash: &str) -> bool {
    let parsed_hash = match PasswordHash::new(password_hash) {
        Ok(hash) => hash,
        Err(_) => return false,
    };

    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

pub(crate) fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

async fn fetch_user_by_username(pool: &PgPool, username: &str) -> sqlx::Result<Option<DbUserAuth>> {
    sqlx::query_as::<_, DbUserAuth>("SELECT id, password_hash FROM users WHERE username = $1")
        .bind(username)
        .fetch_optional(pool)
        .await
}

async fn fetch_user_by_session(pool: &PgPool, token: Uuid) -> sqlx::Result<Option<AuthUser>> {
    sqlx::query_as::<_, AuthUser>(
        "SELECT users.id, users.username, users.is_admin FROM sessions JOIN users ON users.id = sessions.user_id WHERE sessions.id = $1 AND sessions.expires_at > NOW()",
    )
    .bind(token)
    .fetch_optional(pool)
    .await
}

async fn fetch_dashboard_users(pool: &PgPool) -> sqlx::Result<Vec<DashboardUserRow>> {
    sqlx::query_as::<_, DashboardUserRow>(
        "SELECT u.id, u.username, u.usage_group_id, ug.name AS usage_group_name, u.is_admin FROM users u JOIN usage_groups ug ON ug.id = u.usage_group_id ORDER BY u.username",
    )
    .fetch_all(pool)
    .await
}

pub(crate) async fn fetch_glossary_terms(pool: &PgPool) -> sqlx::Result<Vec<GlossaryTermRow>> {
    sqlx::query_as::<_, GlossaryTermRow>(
        "SELECT id, source_term, target_term, notes FROM glossary_terms ORDER BY LOWER(source_term)",
    )
    .fetch_all(pool)
    .await
}

pub(crate) async fn fetch_journal_topics(pool: &PgPool) -> sqlx::Result<Vec<JournalTopicRow>> {
    sqlx::query_as::<_, JournalTopicRow>(
        "SELECT id, name, description, created_at FROM journal_topics ORDER BY LOWER(name)",
    )
    .fetch_all(pool)
    .await
}

pub(crate) async fn fetch_journal_references(
    pool: &PgPool,
) -> sqlx::Result<Vec<JournalReferenceRow>> {
    sqlx::query_as::<_, JournalReferenceRow>(
        "SELECT id, journal_name, reference_mark, low_bound, notes, created_at, updated_at FROM journal_reference_entries ORDER BY low_bound",
    )
    .fetch_all(pool)
    .await
}

pub(crate) async fn fetch_journal_topic_scores(
    pool: &PgPool,
) -> sqlx::Result<Vec<JournalTopicScoreRow>> {
    sqlx::query_as::<_, JournalTopicScoreRow>(
        "SELECT journal_id, topic_id, score FROM journal_topic_scores",
    )
    .fetch_all(pool)
    .await
}

async fn fetch_usage_groups_with_limits(pool: &PgPool) -> Result<Vec<UsageGroupDisplay>> {
    let groups = sqlx::query_as::<_, UsageGroupRow>(
        "SELECT id, name, description FROM usage_groups ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    let group_ids: Vec<Uuid> = groups.iter().map(|group| group.id).collect();
    let limit_map = usage::group_limits(pool, &group_ids).await?;

    let displays = groups
        .into_iter()
        .map(|group| {
            let limits = limit_map
                .get(&group.id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(module, snapshot)| {
                    (
                        module,
                        GroupLimitDisplay {
                            token_limit: snapshot.token_limit,
                            unit_limit: snapshot.unit_limit,
                        },
                    )
                })
                .collect();

            UsageGroupDisplay {
                id: group.id,
                name: group.name,
                description: group.description,
                limits,
            }
        })
        .collect();

    Ok(displays)
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .init();
}
