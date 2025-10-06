mod config;
pub mod llm;
mod modules;

use std::{env, net::SocketAddr, sync::Arc};

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
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use crate::{
    config::{ModelsConfig, PromptsConfig},
    llm::LlmClient,
};

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
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; display: flex; align-items: center; justify-content: center; min-height: 100vh; margin: 0; background: #f1f5f9; color: #0f172a; }}
        main {{ width: 100%; display: flex; justify-content: center; padding: 1.5rem; box-sizing: border-box; }}
        .panel {{ background: #ffffff; padding: 2.5rem 2.25rem; border-radius: 18px; box-shadow: 0 20px 60px rgba(15, 23, 42, 0.08); width: min(420px, 92vw); border: 1px solid #e2e8f0; }}
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
    models: Arc<ModelsConfig>,
    prompts: Arc<PromptsConfig>,
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
    usage_limit: Option<String>,
    #[serde(default)]
    is_admin: Option<String>,
}

#[derive(Deserialize)]
struct UpdatePasswordForm {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct GlossaryCreateForm {
    source_term: String,
    target_term: String,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Deserialize)]
struct GlossaryUpdateForm {
    id: Uuid,
    source_term: String,
    target_term: String,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Deserialize)]
struct GlossaryDeleteForm {
    id: Uuid,
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
    username: String,
    usage_count: i64,
    usage_limit: Option<i64>,
    is_admin: bool,
}

#[derive(sqlx::FromRow)]
pub(crate) struct GlossaryTermRow {
    pub(crate) id: Uuid,
    pub(crate) source_term: String,
    pub(crate) target_term: String,
    pub(crate) notes: Option<String>,
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
        .route("/dashboard/glossary", post(create_glossary_term))
        .route("/dashboard/glossary/update", post(update_glossary_term))
        .route("/dashboard/glossary/delete", post(delete_glossary_term))
        .merge(modules::summarizer::router())
        .merge(modules::translatedocx::router())
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
        let models_config =
            ModelsConfig::load_default().context("failed to load models configuration")?;
        let prompts_config =
            PromptsConfig::load_default().context("failed to load prompts configuration")?;
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

        Ok(Self {
            pool,
            models: Arc::new(models_config),
            prompts: Arc::new(prompts_config),
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

            sqlx::query(
                "INSERT INTO users (id, username, password_hash, usage_limit, is_admin) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(Uuid::new_v4())
            .bind("demo-admin")
            .bind(password_hash)
            .bind(Some(100_i64))
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

    fn models_config(&self) -> Arc<ModelsConfig> {
        Arc::clone(&self.models)
    }

    fn prompts_config(&self) -> Arc<PromptsConfig> {
        Arc::clone(&self.prompts)
    }

    fn llm_client(&self) -> LlmClient {
        self.llm.clone()
    }

    fn pool(&self) -> PgPool {
        self.pool.clone()
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

    let users = match fetch_dashboard_users(&state.pool).await {
        Ok(list) => list,
        Err(err) => {
            error!(?err, "failed to load dashboard users");
            return Err(Redirect::to("/login"));
        }
    };

    let mut table_rows = String::new();
    let mut user_options = String::new();

    if users.is_empty() {
        table_rows.push_str("<tr><td colspan=\"4\">当前还没有用户。</td></tr>");
    } else {
        for user in &users {
            let limit_display = user
                .usage_limit
                .map(|v| v.to_string())
                .unwrap_or_else(|| "不限".to_string());
            let role = if user.is_admin {
                "管理员"
            } else {
                "普通用户"
            };
            let highlight = if user.username == auth_user.username {
                " class=\"current-user\""
            } else {
                ""
            };

            table_rows.push_str(&format!(
                "<tr{highlight}><td>{name}</td><td>{count}</td><td>{limit}</td><td>{role}</td></tr>",
                name = escape_html(&user.username),
                count = user.usage_count,
                limit = limit_display,
                role = role,
                highlight = highlight
            ));

            user_options.push_str(&format!(
                "<option value=\"{value}\">{label}</option>",
                value = escape_html(&user.username),
                label = escape_html(&user.username)
            ));
        }
    }

    let (user_options, reset_disabled_attr) = if user_options.is_empty() {
        (
            "<option value=\"\" disabled selected>暂无可选用户</option>".to_string(),
            " disabled",
        )
    } else {
        (user_options, "")
    };

    let message_block = compose_flash_message(&params);

    let admin_controls = if auth_user.is_admin {
        let glossary_terms = match fetch_glossary_terms(&state.pool).await {
            Ok(list) => list,
            Err(err) => {
                error!(?err, "failed to load glossary terms");
                Vec::new()
            }
        };

        let mut glossary_rows = String::new();
        let mut glossary_select_options = String::new();

        if glossary_terms.is_empty() {
            glossary_rows.push_str("<tr><td colspan=\"4\">尚未添加术语。</td></tr>");
        } else {
            for term in &glossary_terms {
                let notes_display = term
                    .notes
                    .as_ref()
                    .map(|n| escape_html(n))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "—".to_string());
                glossary_rows.push_str(&format!(
                    r#"<tr><td>{en}</td><td>{cn}</td><td>{notes}</td><td>
    <form method="post" action="/dashboard/glossary/delete" onsubmit="return confirm('确定删除该术语？');">
        <input type="hidden" name="id" value="{id}">
        <button type="submit" class="danger">删除</button>
    </form>
</td></tr>"#,
                    en = escape_html(&term.source_term),
                    cn = escape_html(&term.target_term),
                    notes = notes_display,
                    id = term.id
                ));

                glossary_select_options.push_str(&format!(
                    "<option value=\"{id}\">EN：{en} → CN：{cn}</option>",
                    id = term.id,
                    en = escape_html(&term.source_term),
                    cn = escape_html(&term.target_term)
                ));
            }
        }

        let (glossary_select_options, glossary_update_disabled_attr) =
            if glossary_select_options.is_empty() {
                (
                    "<option value=\"\" disabled selected>暂无可选术语</option>".to_string(),
                    " disabled",
                )
            } else {
                (glossary_select_options, "")
            };

        let glossary_section = format!(
            r##"<section class="admin">
    <h2>术语表管理</h2>
    <p class="section-note">这些术语将用于翻译时的术语表以保持用词一致。</p>
    <div class="stack">
        <table class="glossary">
            <thead>
                <tr><th>英文</th><th>中文</th><th>备注</th><th>操作</th></tr>
            </thead>
            <tbody>
                {glossary_rows}
            </tbody>
        </table>
        <div class="glossary-forms">
            <form method="post" action="/dashboard/glossary">
                <h3>新增术语</h3>
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
                <div class="field">
                    <label for="glossary-update-id">选择术语</label>
                    <select id="glossary-update-id" name="id" required{glossary_update_disabled_attr}>
                        {glossary_select_options}
                    </select>
                </div>
                <div class="field">
                    <label for="glossary-update-source">更新后的英文术语</label>
                    <input id="glossary-update-source" name="source_term" required{glossary_update_disabled_attr}>
                </div>
                <div class="field">
                    <label for="glossary-update-target">更新后的中文术语</label>
                    <input id="glossary-update-target" name="target_term" required{glossary_update_disabled_attr}>
                </div>
                <div class="field">
                    <label for="glossary-update-notes">备注（可选）</label>
                    <input id="glossary-update-notes" name="notes" placeholder="填写上下文或使用说明"{glossary_update_disabled_attr}>
                </div>
                <button type="submit"{glossary_update_disabled_attr}>保存修改</button>
            </form>
        </div>
    </div>
</section>"##,
            glossary_rows = glossary_rows,
            glossary_select_options = glossary_select_options,
            glossary_update_disabled_attr = glossary_update_disabled_attr
        );

        let mut controls = format!(
            r##"<section class="admin">
    <h2>创建用户</h2>
    <form method="post" action="/dashboard/users">
        <div class="field">
            <label for="new-username">用户名</label>
            <input id="new-username" name="username" required>
        </div>
        <div class="field">
            <label for="new-password">密码</label>
            <input id="new-password" type="password" name="password" required>
        </div>
        <div class="field">
            <label for="usage-limit">调用上限（留空表示不限）</label>
            <input id="usage-limit" name="usage_limit" placeholder="例如：250">
        </div>
        <div class="field checkbox">
            <label><input type="checkbox" name="is_admin" value="on"> 授予管理员权限</label>
        </div>
        <button type="submit">创建用户</button>
    </form>
</section>
<section class="admin">
    <h2>重置密码</h2>
    <form method="post" action="/dashboard/users/password">
        <div class="field">
            <label for="reset-username">选择用户</label>
            <select id="reset-username" name="username" required{reset_disabled_attr}>
                {user_options}
            </select>
        </div>
        <div class="field">
            <label for="reset-password">新密码</label>
            <input id="reset-password" type="password" name="password" required{reset_disabled_attr}>
        </div>
        <button type="submit"{reset_disabled_attr}>更新密码</button>
    </form>
</section>"##,
            user_options = user_options,
            reset_disabled_attr = reset_disabled_attr
        );
        controls.push_str(&glossary_section);
        controls
    } else {
        String::new()
    };

    let models_config = state.models_config();
    let mut model_notes = String::new();
    if let Some(models) = models_config.summarizer() {
        model_notes.push_str(&format!(
            "<p class=\"meta-note\">摘要使用模型 <code>{summary}</code>，翻译使用模型 <code>{translation}</code>。</p>",
            summary = escape_html(models.summary_model()),
            translation = escape_html(models.translation_model())
        ));
    }
    if let Some(models) = models_config.translate_docx() {
        model_notes.push_str(&format!(
            "<p class=\"meta-note\">DOCX 翻译使用模型 <code>{translation}</code>。</p>",
            translation = escape_html(models.translation_model())
        ));
    }

    let footer = render_footer();

    let html = format!(
        r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>管理后台</title>
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
        table {{ width: 100%; border-collapse: collapse; margin-top: 1.5rem; background: #ffffff; border: 1px solid #e2e8f0; border-radius: 12px; overflow: hidden; }}
        th, td {{ padding: 0.75rem 1rem; border-bottom: 1px solid #e2e8f0; text-align: left; }}
        th {{ background: #f1f5f9; color: #0f172a; font-weight: 600; }}
        tr.current-user td {{ background: #eff6ff; }}
        a {{ color: #2563eb; text-decoration: none; }}
        a:hover {{ text-decoration: underline; }}
        .meta {{ margin-top: 1.5rem; font-size: 0.95rem; color: #475569; }}
        .meta-note {{ margin-bottom: 0.5rem; }}
        .flash {{ padding: 1rem; border-radius: 8px; margin-top: 1rem; border: 1px solid transparent; }}
        .flash.success {{ background: #ecfdf3; border-color: #bbf7d0; color: #166534; }}
        .flash.error {{ background: #fef2f2; border-color: #fecaca; color: #b91c1c; }}
        .admin {{ margin-top: 2.5rem; padding: 1.5rem; border-radius: 12px; background: #ffffff; border: 1px solid #e2e8f0; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); }}
        .admin h2 {{ margin-top: 0; color: #1d4ed8; }}
        .admin h3 {{ margin-top: 0; color: #1d4ed8; font-size: 1.05rem; }}
        .field {{ margin-bottom: 1rem; display: flex; flex-direction: column; gap: 0.4rem; }}
        .field input, .field select {{ padding: 0.75rem; border-radius: 8px; border: 1px solid #cbd5f5; background: #f8fafc; color: #0f172a; }}
        .field input:focus, .field select:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        .field.checkbox {{ flex-direction: row; align-items: center; gap: 0.6rem; }}
        button {{ padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        button.danger {{ background: #ef4444; color: #ffffff; }}
        button.danger:hover {{ background: #dc2626; }}
        button:disabled {{ opacity: 0.6; cursor: not-allowed; }}
        .section-note {{ margin-top: -0.5rem; margin-bottom: 1rem; color: #64748b; }}
        .stack {{ display: flex; flex-direction: column; gap: 1.5rem; }}
        table.glossary {{ margin-top: 0; }}
        .glossary-forms {{ display: grid; gap: 1.5rem; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); }}
        .glossary-forms form {{ border: 1px solid #e2e8f0; border-radius: 12px; padding: 1rem; background: #f8fafc; box-shadow: inset 0 0 0 1px rgba(148, 163, 184, 0.1); }}
        .tools {{ margin-top: 2.5rem; padding: 1.5rem; border-radius: 12px; background: #ffffff; border: 1px solid #e2e8f0; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); }}
        .tools h2 {{ margin-top: 0; color: #1d4ed8; }}
        .tools ul {{ list-style: none; margin: 0; padding: 0; }}
        .tools li {{ margin-bottom: 0.5rem; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>使用情况仪表盘</h1>
            <a class="back-link" href="/">← 返回首页</a>
        </div>
        <p>快速查看各账号的额度与后台任务。</p>
    </header>
    <main>
        <p data-user-id="{auth_user_id}">当前登录：<strong>{username}</strong>。</p>
        {message_block}
        <table>
            <thead>
                <tr><th>用户名</th><th>已用次数</th><th>调用上限</th><th>角色</th></tr>
            </thead>
            <tbody>
                {table_rows}
            </tbody>
        </table>
        <div class="meta">
            {model_notes}
            <p>后续计划：扩展用户管理、日志记录和实时 LLM 监控。</p>
        </div>
        <section class="tools">
            <h2>可用工具</h2>
            <ul>
                <li><a href="/tools/summarizer">文档摘要与翻译模块</a></li>
                <li><a href="/tools/translatedocx">DOCX 文档翻译模块</a></li>
            </ul>
        </section>
        {admin_controls}
        {footer}
    </main>
</body>
</html>"##,
        username = escape_html(&auth_user.username),
        auth_user_id = auth_user.id,
        message_block = message_block,
        table_rows = table_rows,
        admin_controls = admin_controls,
        model_notes = model_notes,
        footer = footer
    );

    Ok(Html(html))
}

async fn create_user(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<CreateUserForm>,
) -> Result<Redirect, Redirect> {
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
        return Ok(Redirect::to("/?error=not_authorized"));
    }

    let username = form.username.trim();
    if username.is_empty() {
        return Ok(Redirect::to("/dashboard?error=missing_username"));
    }

    if form.password.trim().is_empty() {
        return Ok(Redirect::to("/dashboard?error=missing_password"));
    }

    let usage_limit: Option<i64> = form.usage_limit.as_ref().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            trimmed.parse::<i64>().ok()
        }
    });

    let password_hash = match hash_password(form.password.trim()) {
        Ok(hash) => hash,
        Err(err) => {
            error!(?err, "failed to hash password when creating user");
            return Ok(Redirect::to("/dashboard?error=hash_failed"));
        }
    };

    let is_admin = form.is_admin.is_some();

    let insert_result = sqlx::query(
        "INSERT INTO users (id, username, password_hash, usage_limit, is_admin) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(username)
    .bind(password_hash)
    .bind(usage_limit)
    .bind(is_admin)
    .execute(&state.pool)
    .await;

    match insert_result {
        Ok(_) => Ok(Redirect::to("/dashboard?status=created")),
        Err(sqlx::Error::Database(db_err)) if db_err.constraint() == Some("users_username_key") => {
            Ok(Redirect::to("/dashboard?error=duplicate"))
        }
        Err(err) => {
            error!(?err, "failed to insert new user");
            Ok(Redirect::to("/dashboard?error=unknown"))
        }
    }
}

async fn update_user_password(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<UpdatePasswordForm>,
) -> Result<Redirect, Redirect> {
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
        return Ok(Redirect::to("/?error=not_authorized"));
    }

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

async fn create_glossary_term(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GlossaryCreateForm>,
) -> Result<Redirect, Redirect> {
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
        return Ok(Redirect::to("/?error=not_authorized"));
    }

    let GlossaryCreateForm {
        source_term,
        target_term,
        notes,
    } = form;

    let source_clean = source_term.trim().to_owned();
    let target_clean = target_term.trim().to_owned();

    if source_clean.is_empty() || target_clean.is_empty() {
        return Ok(Redirect::to("/dashboard?error=glossary_missing_fields"));
    }

    let notes_clean = notes.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

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
        Ok(_) => Ok(Redirect::to("/dashboard?status=glossary_created")),
        Err(sqlx::Error::Database(db_err))
            if db_err.constraint() == Some("idx_glossary_terms_source_lower") =>
        {
            Ok(Redirect::to("/dashboard?error=glossary_duplicate"))
        }
        Err(err) => {
            error!(?err, "failed to create glossary term");
            Ok(Redirect::to("/dashboard?error=unknown"))
        }
    }
}

async fn update_glossary_term(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GlossaryUpdateForm>,
) -> Result<Redirect, Redirect> {
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
        return Ok(Redirect::to("/?error=not_authorized"));
    }

    let GlossaryUpdateForm {
        id,
        source_term,
        target_term,
        notes,
    } = form;

    let source_clean = source_term.trim().to_owned();
    let target_clean = target_term.trim().to_owned();

    if source_clean.is_empty() || target_clean.is_empty() {
        return Ok(Redirect::to("/dashboard?error=glossary_missing_fields"));
    }

    let notes_clean = notes.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    let update_result = sqlx::query(
        "UPDATE glossary_terms SET source_term = $2, target_term = $3, notes = $4, updated_at = NOW() WHERE id = $1",
    )
    .bind(id)
    .bind(&source_clean)
    .bind(&target_clean)
    .bind(notes_clean.as_deref())
    .execute(&state.pool)
    .await;

    match update_result {
        Ok(result) if result.rows_affected() > 0 => {
            Ok(Redirect::to("/dashboard?status=glossary_updated"))
        }
        Ok(_) => Ok(Redirect::to("/dashboard?error=glossary_not_found")),
        Err(sqlx::Error::Database(db_err))
            if db_err.constraint() == Some("idx_glossary_terms_source_lower") =>
        {
            Ok(Redirect::to("/dashboard?error=glossary_duplicate"))
        }
        Err(err) => {
            error!(?err, "failed to update glossary term");
            Ok(Redirect::to("/dashboard?error=unknown"))
        }
    }
}

async fn delete_glossary_term(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GlossaryDeleteForm>,
) -> Result<Redirect, Redirect> {
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
        return Ok(Redirect::to("/?error=not_authorized"));
    }

    let delete_result = sqlx::query("DELETE FROM glossary_terms WHERE id = $1")
        .bind(form.id)
        .execute(&state.pool)
        .await;

    match delete_result {
        Ok(result) if result.rows_affected() > 0 => {
            Ok(Redirect::to("/dashboard?status=glossary_deleted"))
        }
        Ok(_) => Ok(Redirect::to("/dashboard?error=glossary_not_found")),
        Err(err) => {
            error!(?err, "failed to delete glossary term");
            Ok(Redirect::to("/dashboard?error=unknown"))
        }
    }
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
            _ => "发生未知错误，请查看日志。",
        };

        return format!("<div class=\"flash error\">{message}</div>");
    }

    String::new()
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
        "SELECT username, usage_count, usage_limit, is_admin FROM users ORDER BY username",
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

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .init();
}
