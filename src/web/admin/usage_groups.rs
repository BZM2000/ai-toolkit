use std::collections::HashMap;

use axum::{
    extract::{Form, State},
    response::Redirect,
};
use axum_extra::extract::cookie::CookieJar;
use tracing::error;
use uuid::Uuid;

use crate::{usage, web::AppState};

use super::auth::require_admin_user;

pub async fn save_usage_group(
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
                .execute(state.pool_ref())
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
                .execute(state.pool_ref())
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

    if let Err(err) = usage::upsert_group_limits(state.pool_ref(), group_id, &allocations).await {
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
