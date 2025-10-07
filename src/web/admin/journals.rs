use std::collections::HashMap;

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
pub(crate) struct JournalTopicUpsertForm {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct JournalTopicDeleteForm {
    id: Uuid,
    #[serde(default)]
    redirect: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct JournalReferenceUpsertForm {
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
pub(crate) struct JournalReferenceDeleteForm {
    id: Uuid,
    #[serde(default)]
    redirect: Option<String>,
}

pub async fn upsert_journal_topic(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<JournalTopicUpsertForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_module_redirect(form.redirect.as_deref());

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
        .filter(|v| !v.is_empty());

    match sqlx::query(
        "INSERT INTO journal_topics (id, name, description)
         VALUES ($1, $2, $3)
         ON CONFLICT (name)
         DO UPDATE SET description = EXCLUDED.description, updated_at = NOW()",
    )
    .bind(Uuid::new_v4())
    .bind(name)
    .bind(description)
    .execute(state.pool_ref())
    .await
    {
        Ok(_) => Ok(Redirect::to(&format!("{redirect_base}?status=topic_saved"))),
        Err(err) => {
            error!(?err, "failed to upsert journal topic");
            Ok(Redirect::to(&format!("{redirect_base}?error=unknown")))
        }
    }
}

pub async fn delete_journal_topic(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<JournalTopicDeleteForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_module_redirect(form.redirect.as_deref());

    match sqlx::query("DELETE FROM journal_topics WHERE id = $1")
        .bind(form.id)
        .execute(state.pool_ref())
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

pub async fn upsert_journal_reference(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<JournalReferenceUpsertForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_module_redirect(form.redirect.as_deref());

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

                if !(0..=2).contains(&score) {
                    return Ok(Redirect::to(&format!(
                        "{redirect_base}?error=journal_invalid_score"
                    )));
                }

                parsed_scores.push((topic_id, score));
            }
        }
    }

    let mut transaction = match state.pool_ref().begin().await {
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

pub async fn delete_journal_reference(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<JournalReferenceDeleteForm>,
) -> Result<Redirect, Redirect> {
    let _admin = require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_module_redirect(form.redirect.as_deref());

    match sqlx::query("DELETE FROM journal_reference_entries WHERE id = $1")
        .bind(form.id)
        .execute(state.pool_ref())
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
