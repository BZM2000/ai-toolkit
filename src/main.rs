mod config;
mod history;
pub mod llm;
mod maintenance;
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
    println!("starting ai-toolkit service bootstrap");
    info!("starting ai-toolkit service");

    if let Err(err) = app_main().await {
        error!(?err, "application error");
        std::process::exit(1);
    }
}

async fn app_main() -> Result<()> {
    println!("initialising application state");
    info!("constructing application state");
    let state = AppState::new().await?;
    println!("application state initialised");
    info!("ensured application state is ready");
    state.ensure_seed_admin().await?;
    info!("seed admin check complete");

    maintenance::spawn(state.clone());
    info!("background maintenance tasks registered");

    let app = web::router::build_router(state);
    info!("router built");

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
        .with_ansi(false)
        .with_writer(std::io::stdout)
        .compact()
        .init();
}
