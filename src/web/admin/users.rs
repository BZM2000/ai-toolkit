use axum::{
    extract::{Form, State},
    response::Redirect,
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use tracing::error;
use uuid::Uuid;

use crate::web::{
    AppState,
    auth::{self},
};

use super::auth::require_admin_user;

#[derive(Deserialize)]
pub(crate) struct CreateUserForm {
    username: String,
    password: String,
    #[serde(default)]
    usage_group_id: Option<Uuid>,
    #[serde(default)]
    is_admin: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct UpdatePasswordForm {
    username: String,
    password: String,
}

#[derive(Deserialize)]
pub(crate) struct AssignUserGroupForm {
    username: String,
    usage_group_id: Uuid,
}

pub async fn create_user(
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

    let password_hash = match auth::hash_password(password) {
        Ok(hash) => hash,
        Err(err) => {
            error!(?err, "failed to hash password while creating user");
            return Ok(Redirect::to("/dashboard?error=unknown"));
        }
    };

    let result = sqlx::query(
        "INSERT INTO users (id, username, password_hash, usage_group_id, is_admin)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::new_v4())
    .bind(username)
    .bind(password_hash)
    .bind(group_id)
    .bind(is_admin)
    .execute(state.pool_ref())
    .await;

    match result {
        Ok(_) => Ok(Redirect::to("/dashboard?status=created")),
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("23505") => {
            Ok(Redirect::to("/dashboard?error=duplicate"))
        }
        Err(err) => {
            error!(?err, "failed to create user");
            Ok(Redirect::to("/dashboard?error=unknown"))
        }
    }
}

pub async fn update_user_password(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<UpdatePasswordForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;

    let username = form.username.trim();
    if username.is_empty() {
        return Ok(Redirect::to("/dashboard?error=user_missing"));
    }

    let password = form.password.trim();
    if password.is_empty() {
        return Ok(Redirect::to("/dashboard?error=missing_password"));
    }

    let password_hash = match auth::hash_password(password) {
        Ok(hash) => hash,
        Err(err) => {
            error!(
                ?err,
                "failed to hash password while resetting user password"
            );
            return Ok(Redirect::to("/dashboard?error=unknown"));
        }
    };

    let result = sqlx::query("UPDATE users SET password_hash = $2 WHERE username = $1")
        .bind(username)
        .bind(password_hash)
        .execute(state.pool_ref())
        .await;

    match result {
        Ok(res) if res.rows_affected() > 0 => {
            Ok(Redirect::to("/dashboard?status=password_updated"))
        }
        Ok(_) => Ok(Redirect::to("/dashboard?error=user_missing")),
        Err(err) => {
            error!(?err, "failed to update user password");
            Ok(Redirect::to("/dashboard?error=unknown"))
        }
    }
}

pub async fn assign_user_group(
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
        .execute(state.pool_ref())
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
