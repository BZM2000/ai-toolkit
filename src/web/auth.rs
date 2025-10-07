use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::{
    extract::{Form, State},
    http::StatusCode,
    response::{Html, Redirect},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{Duration as ChronoDuration, Utc};
use cookie::time::Duration as CookieDuration;
use rand_core::OsRng;
use serde::Deserialize;
use sqlx::PgPool;
use tracing::error;
use uuid::Uuid;

use crate::web::{AppState, render_login_page};

#[derive(Clone, sqlx::FromRow)]
pub struct DbUserAuth {
    pub id: Uuid,
    pub password_hash: String,
}

#[derive(Clone, sqlx::FromRow)]
pub struct AuthUser {
    pub id: Uuid,
    pub username: String,
    pub is_admin: bool,
}

pub const SESSION_COOKIE: &str = "auth_token";
pub const SESSION_TTL_DAYS: i64 = 7;

#[derive(Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

pub async fn login_page(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Html<String>, Redirect> {
    if let Some(redirect) = redirect_if_authenticated(&state, &jar).await {
        return Err(redirect);
    }

    Ok(Html(render_login_page()))
}

pub async fn process_login(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> Result<(CookieJar, Redirect), (StatusCode, Html<String>)> {
    let username = form.username.trim();
    let pool = state.pool();

    let user = match fetch_user_by_username(&pool, username).await {
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
            .execute(state.pool_ref())
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

pub async fn logout(State(state): State<AppState>, jar: CookieJar) -> (CookieJar, Redirect) {
    let mut jar = jar;

    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        if let Ok(token) = Uuid::parse_str(cookie.value()) {
            if let Err(err) = sqlx::query("DELETE FROM sessions WHERE id = $1")
                .bind(token)
                .execute(state.pool_ref())
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

pub async fn redirect_if_authenticated(state: &AppState, jar: &CookieJar) -> Option<Redirect> {
    let token_cookie = jar.get(SESSION_COOKIE)?;
    let token = Uuid::parse_str(token_cookie.value()).ok()?;
    let pool = state.pool();

    match fetch_user_by_session(&pool, token).await {
        Ok(Some(_)) => Some(Redirect::to("/")),
        Ok(None) => None,
        Err(err) => {
            error!(?err, "failed to validate session for access gate");
            None
        }
    }
}

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
}

pub fn verify_password(password: &str, password_hash: &str) -> bool {
    let parsed = PasswordHash::new(password_hash);
    match parsed {
        Ok(hash) => Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok(),
        Err(_) => false,
    }
}

pub async fn fetch_user_by_username(
    pool: &PgPool,
    username: &str,
) -> sqlx::Result<Option<DbUserAuth>> {
    sqlx::query_as::<_, DbUserAuth>("SELECT id, password_hash FROM users WHERE username = $1")
        .bind(username)
        .fetch_optional(pool)
        .await
}

pub async fn fetch_user_by_session(pool: &PgPool, token: Uuid) -> sqlx::Result<Option<AuthUser>> {
    sqlx::query_as::<_, AuthUser>(
        "SELECT users.id, users.username, users.is_admin FROM sessions JOIN users ON users.id = sessions.user_id WHERE sessions.id = $1 AND sessions.expires_at > NOW()",
    )
    .bind(token)
    .fetch_optional(pool)
    .await
}

fn invalid_credentials() -> (StatusCode, Html<String>) {
    (
        StatusCode::UNAUTHORIZED,
        Html("<h1>登录失败</h1><p>用户名或密码错误。</p>".to_string()),
    )
}

fn server_error() -> (StatusCode, Html<String>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html("<h1>服务器错误</h1><p>请稍后再试。</p>".to_string()),
    )
}
