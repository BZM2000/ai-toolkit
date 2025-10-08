use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Duration, Utc};
use sqlx::{PgPool, Row};
use tracing::error;
use uuid::Uuid;

const WINDOW_DAYS: i64 = 7;
const WINDOW_DURATION: Duration = Duration::days(WINDOW_DAYS);

pub const MODULE_SUMMARIZER: &str = "summarizer";
pub const MODULE_TRANSLATE_DOCX: &str = "translatedocx";
pub const MODULE_GRADER: &str = "grader";

/// Human readable labels for known modules to drive forms and display.
pub struct ModuleDescriptor {
    pub key: &'static str,
    pub label: &'static str,
    pub unit_label: &'static str,
}

pub const REGISTERED_MODULES: &[ModuleDescriptor] = &[
    ModuleDescriptor {
        key: MODULE_SUMMARIZER,
        label: "摘要与翻译",
        unit_label: "文档数量",
    },
    ModuleDescriptor {
        key: MODULE_TRANSLATE_DOCX,
        label: "DOCX 翻译",
        unit_label: "文档数量",
    },
    ModuleDescriptor {
        key: MODULE_GRADER,
        label: "稿件评估",
        unit_label: "任务次数",
    },
];

#[derive(Debug, Clone, Copy, Default)]
pub struct UsageSnapshot {
    pub tokens: i64,
    pub units: i64,
    pub token_limit: Option<i64>,
    pub unit_limit: Option<i64>,
}

#[derive(Debug)]
pub enum UsageLimitErrorKind {
    TokensExceeded {
        limit: i64,
        used: i64,
    },
    UnitsExceeded {
        limit: i64,
        used: i64,
        requested: i64,
    },
    Backend,
}

#[derive(Debug)]
pub struct UsageLimitError {
    pub kind: UsageLimitErrorKind,
}

impl UsageLimitError {
    pub fn message(&self) -> String {
        match &self.kind {
            UsageLimitErrorKind::TokensExceeded { limit, used } => {
                format!("近 7 日累计令牌数已达上限（{used}/{limit}）。",)
            }
            UsageLimitErrorKind::UnitsExceeded {
                limit,
                used,
                requested,
            } => format!(
                "近 7 日累计任务数将超出上限（当前 {used}，本次 +{requested}，上限 {limit}）。",
            ),
            UsageLimitErrorKind::Backend => "额度校验失败，请稍后再试。".to_string(),
        }
    }
}

pub async fn ensure_within_limits(
    pool: &PgPool,
    user_id: Uuid,
    module_key: &str,
    units_to_add: i64,
) -> Result<UsageSnapshot, UsageLimitError> {
    let snapshot = match load_snapshot(pool, user_id, module_key).await {
        Ok(snapshot) => snapshot,
        Err(err) => {
            error!(?err, "failed to load usage snapshot");
            return Err(UsageLimitError {
                kind: UsageLimitErrorKind::Backend,
            });
        }
    };

    if let Some(limit) = snapshot.token_limit {
        if snapshot.tokens >= limit {
            return Err(UsageLimitError {
                kind: UsageLimitErrorKind::TokensExceeded {
                    limit,
                    used: snapshot.tokens,
                },
            });
        }
    }

    if let Some(limit) = snapshot.unit_limit {
        if snapshot.units + units_to_add > limit {
            return Err(UsageLimitError {
                kind: UsageLimitErrorKind::UnitsExceeded {
                    limit,
                    used: snapshot.units,
                    requested: units_to_add,
                },
            });
        }
    }

    Ok(snapshot)
}

pub async fn record_usage(
    pool: &PgPool,
    user_id: Uuid,
    module_key: &str,
    tokens: i64,
    units: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO usage_events (id, user_id, module_key, tokens, units, occurred_at) VALUES ($1, $2, $3, $4, $5, NOW())",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(module_key)
    .bind(tokens.max(0))
    .bind(units.max(0))
    .execute(pool)
    .await
    .context("failed to insert usage event")?;

    Ok(())
}

pub async fn load_snapshot(
    pool: &PgPool,
    user_id: Uuid,
    module_key: &str,
) -> Result<UsageSnapshot> {
    let limits_row = sqlx::query(
        "SELECT ugl.token_limit, ugl.unit_limit \
         FROM users u \
         JOIN usage_groups ug ON ug.id = u.usage_group_id \
         LEFT JOIN usage_group_limits ugl ON ugl.group_id = ug.id AND ugl.module_key = $2 \
         WHERE u.id = $1",
    )
    .bind(user_id)
    .bind(module_key)
    .fetch_optional(pool)
    .await
    .context("failed to load usage group limits")?
    .ok_or_else(|| anyhow!("用户未分配额度组"))?;

    let token_limit = limits_row.try_get::<Option<i64>, _>("token_limit")?;
    let unit_limit = limits_row.try_get::<Option<i64>, _>("unit_limit")?;

    let window_start = Utc::now() - WINDOW_DURATION;

    let aggregates_row = sqlx::query(
        "SELECT COALESCE(SUM(tokens)::BIGINT, 0::BIGINT) AS tokens, \
                COALESCE(SUM(units)::BIGINT, 0::BIGINT) AS units \
         FROM usage_events \
         WHERE user_id = $1 AND module_key = $2 AND occurred_at >= $3",
    )
    .bind(user_id)
    .bind(module_key)
    .bind(window_start)
    .fetch_one(pool)
    .await
    .context("failed to aggregate usage window")?;

    let tokens: i64 = aggregates_row.try_get("tokens")?;
    let units: i64 = aggregates_row.try_get("units")?;

    Ok(UsageSnapshot {
        tokens,
        units,
        token_limit,
        unit_limit,
    })
}

pub async fn usage_for_users(
    pool: &PgPool,
    user_ids: &[Uuid],
) -> Result<HashMap<Uuid, HashMap<String, UsageSnapshot>>> {
    if user_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let window_start = Utc::now() - WINDOW_DURATION;

    let rows = sqlx::query(
        "SELECT user_id, module_key, \
                COALESCE(SUM(tokens)::BIGINT, 0::BIGINT) AS tokens, \
                COALESCE(SUM(units)::BIGINT, 0::BIGINT) AS units \
         FROM usage_events \
         WHERE user_id = ANY($1) AND occurred_at >= $2 \
         GROUP BY user_id, module_key",
    )
    .bind(user_ids)
    .bind(window_start)
    .fetch_all(pool)
    .await
    .context("failed to fetch batch usage window")?;

    let mut result: HashMap<Uuid, HashMap<String, UsageSnapshot>> = HashMap::new();
    for row in rows {
        let user_id: Uuid = row.try_get("user_id")?;
        let module: String = row.try_get("module_key")?;
        let tokens: i64 = row.try_get("tokens")?;
        let units: i64 = row.try_get("units")?;

        result.entry(user_id).or_default().insert(
            module,
            UsageSnapshot {
                tokens,
                units,
                token_limit: None,
                unit_limit: None,
            },
        );
    }

    Ok(result)
}

pub async fn group_limits(
    pool: &PgPool,
    group_ids: &[Uuid],
) -> Result<HashMap<Uuid, HashMap<String, UsageSnapshot>>> {
    if group_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query(
        "SELECT group_id, module_key, token_limit, unit_limit FROM usage_group_limits WHERE group_id = ANY($1)",
    )
    .bind(group_ids)
    .fetch_all(pool)
    .await
    .context("failed to fetch usage group limits")?;

    let mut result: HashMap<Uuid, HashMap<String, UsageSnapshot>> = HashMap::new();
    for row in rows {
        let group_id: Uuid = row.try_get("group_id")?;
        let module: String = row.try_get("module_key")?;
        let token_limit = row.try_get::<Option<i64>, _>("token_limit")?;
        let unit_limit = row.try_get::<Option<i64>, _>("unit_limit")?;

        result.entry(group_id).or_default().insert(
            module,
            UsageSnapshot {
                tokens: 0,
                units: 0,
                token_limit,
                unit_limit,
            },
        );
    }

    Ok(result)
}

pub async fn upsert_group_limits(
    pool: &PgPool,
    group_id: Uuid,
    allocations: &HashMap<String, (Option<i64>, Option<i64>)>,
) -> Result<()> {
    let mut transaction = pool.begin().await?;

    sqlx::query("DELETE FROM usage_group_limits WHERE group_id = $1")
        .bind(group_id)
        .execute(&mut *transaction)
        .await?;

    for (module, (token_limit, unit_limit)) in allocations {
        if token_limit.is_none() && unit_limit.is_none() {
            continue;
        }

        sqlx::query(
            "INSERT INTO usage_group_limits (id, group_id, module_key, token_limit, unit_limit) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(group_id)
        .bind(module.as_str())
        .bind(token_limit.map(|v| v as i64))
        .bind(unit_limit.map(|v| v as i64))
        .execute(&mut *transaction)
        .await?;
    }

    transaction.commit().await?;

    Ok(())
}

pub fn parse_optional_limit(input: Option<&str>) -> Result<Option<i64>> {
    match input.map(str::trim).filter(|v| !v.is_empty()) {
        Some(value) => {
            let parsed: i64 = value.parse().map_err(|_| anyhow!("invalid limit value"))?;
            if parsed < 0 {
                bail!("limit cannot be negative");
            }
            Ok(Some(parsed))
        }
        None => Ok(None),
    }
}
