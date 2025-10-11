use axum::{
    Router,
    http::{StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};

use crate::{
    modules,
    web::{AppState, admin, auth, history, landing},
};

const ROBOTS_TXT_BODY: &str = include_str!("../../robots.txt");

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(landing::landing_page))
        .route("/login", get(auth::login_page).post(auth::process_login))
        .route("/logout", post(auth::logout))
        .route("/healthz", get(healthz))
        .route("/robots.txt", get(robots_txt))
        .route("/dashboard", get(admin::dashboard))
        .route("/dashboard/users", post(admin::create_user))
        .route(
            "/dashboard/users/password",
            post(admin::update_user_password),
        )
        .route("/dashboard/users/group", post(admin::assign_user_group))
        .route("/dashboard/usage-groups", post(admin::save_usage_group))
        .route("/dashboard/glossary", post(admin::create_glossary_term))
        .route(
            "/dashboard/glossary/update",
            post(admin::update_glossary_term),
        )
        .route(
            "/dashboard/glossary/delete",
            post(admin::delete_glossary_term),
        )
        .route(
            "/dashboard/journal-topics",
            post(admin::upsert_journal_topic),
        )
        .route(
            "/dashboard/journal-topics/delete",
            post(admin::delete_journal_topic),
        )
        .route(
            "/dashboard/journal-references",
            post(admin::upsert_journal_reference),
        )
        .route(
            "/dashboard/journal-references/delete",
            post(admin::delete_journal_reference),
        )
        .route("/api/history", get(history::recent_history))
        .merge(modules::summarizer::router())
        .merge(modules::translatedocx::router())
        .merge(modules::grader::router())
        .merge(modules::info_extract::router())
        .merge(modules::reviewer::router())
        .with_state(state)
}

async fn robots_txt() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        ROBOTS_TXT_BODY,
    )
}

async fn healthz() -> impl IntoResponse {
    StatusCode::OK
}
