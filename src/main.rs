mod config;
pub mod llm;
mod modules;
mod usage;
mod utils;
mod web;

pub use web::{
    AppState, GlossaryTermRow, JournalReferenceRow, JournalTopicRow, JournalTopicScoreRow,
    SESSION_COOKIE, SESSION_TTL_DAYS, escape_html, fetch_glossary_terms, fetch_journal_references,
    fetch_journal_topic_scores, fetch_journal_topics, render_footer, render_login_page,
};

use std::{env, net::SocketAddr};

use anyhow::{Context, Result};
use dotenvy::dotenv;
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

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

    let app = web::router::build_router(state);

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

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .init();
}
