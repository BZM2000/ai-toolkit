use axum::response::Redirect;
use axum_extra::extract::cookie::CookieJar;
use uuid::Uuid;

use crate::web::{
    AppState, AuthUser,
    auth::{self, SESSION_COOKIE},
};

pub async fn require_admin_user(state: &AppState, jar: &CookieJar) -> Result<AuthUser, Redirect> {
    let Some(token_cookie) = jar.get(SESSION_COOKIE) else {
        return Err(Redirect::to("/login"));
    };

    let token = match Uuid::parse_str(token_cookie.value()) {
        Ok(token) => token,
        Err(_) => return Err(Redirect::to("/login")),
    };

    let pool = state.pool();
    let auth_user = match auth::fetch_user_by_session(&pool, token).await {
        Ok(Some(user)) => user,
        _ => return Err(Redirect::to("/login")),
    };

    if !auth_user.is_admin {
        return Err(Redirect::to("/?error=not_authorized"));
    }

    Ok(auth_user)
}
