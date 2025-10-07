use std::{env, sync::Arc};

use anyhow::{Context, Result, anyhow};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

use crate::{
    config::{DocxTranslatorSettings, GraderSettings, ModuleSettings, SummarizerSettings},
    llm::LlmClient,
};

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    settings: Arc<RwLock<ModuleSettings>>,
    llm: LlmClient,
}

impl AppState {
    pub async fn new() -> Result<Self> {
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

    pub async fn ensure_seed_admin(&self) -> Result<()> {
        let has_admin: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE is_admin = TRUE)")
                .fetch_one(&self.pool)
                .await
                .context("failed to verify admin presence")?;

        if !has_admin {
            let password_hash = crate::web::auth::hash_password("change-me")
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

    pub fn llm_client(&self) -> LlmClient {
        self.llm.clone()
    }

    pub fn pool(&self) -> PgPool {
        self.pool.clone()
    }

    pub fn pool_ref(&self) -> &PgPool {
        &self.pool
    }

    pub async fn summarizer_settings(&self) -> Option<SummarizerSettings> {
        let guard = self.settings.read().await;
        guard.summarizer().cloned()
    }

    pub async fn translate_docx_settings(&self) -> Option<DocxTranslatorSettings> {
        let guard = self.settings.read().await;
        guard.translate_docx().cloned()
    }

    pub async fn grader_settings(&self) -> Option<GraderSettings> {
        let guard = self.settings.read().await;
        guard.grader().cloned()
    }

    pub async fn reload_settings(&self) -> Result<()> {
        let latest = ModuleSettings::load(&self.pool)
            .await
            .context("failed to reload module settings")?;
        let mut guard = self.settings.write().await;
        *guard = latest;
        Ok(())
    }
}
