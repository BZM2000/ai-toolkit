use axum::{
    extract::{Form, State},
    response::Redirect,
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use tracing::error;
use uuid::Uuid;

use crate::web::{AppState, admin_utils::sanitize_module_redirect};

use super::auth::require_admin_user;

#[derive(Deserialize)]
pub(crate) struct GlossaryCreateForm {
    source_term: String,
    target_term: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct GlossaryUpdateForm {
    id: Uuid,
    source_term: String,
    target_term: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct GlossaryDeleteForm {
    id: Uuid,
    #[serde(default)]
    redirect: Option<String>,
}

pub async fn create_glossary_term(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GlossaryCreateForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;

    let redirect_base = sanitize_module_redirect(form.redirect.as_deref());

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
    .execute(state.pool_ref())
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

pub async fn update_glossary_term(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GlossaryUpdateForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_module_redirect(form.redirect.as_deref());

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
    .execute(state.pool_ref())
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

pub async fn delete_glossary_term(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GlossaryDeleteForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_module_redirect(form.redirect.as_deref());

    match sqlx::query("DELETE FROM glossary_terms WHERE id = $1")
        .bind(form.id)
        .execute(state.pool_ref())
        .await
    {
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
