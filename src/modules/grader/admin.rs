use axum::{
    extract::{Form, Query, State},
    response::{Html, Redirect},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use tracing::error;

use crate::{
    AppState,
    config::{GraderModels, GraderPrompts, update_grader_models, update_grader_prompts},
    escape_html, fetch_journal_references, fetch_journal_topic_scores, fetch_journal_topics,
    render_footer,
    web::{
        admin::DashboardQuery,
        admin_utils::{compose_flash_message, sanitize_module_redirect},
    },
};

use super::super::admin_shared::{
    MODULE_ADMIN_SHARED_STYLES, render_journal_section, render_topic_section,
};

#[derive(Deserialize)]
pub struct GraderModelForm {
    pub grading_model: String,
    pub keyword_model: String,
    #[serde(default)]
    pub redirect: Option<String>,
}

#[derive(Deserialize)]
pub struct GraderPromptForm {
    pub grading_instructions: String,
    pub keyword_selection: String,
    #[serde(default)]
    pub redirect: Option<String>,
}

pub async fn settings_page(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<DashboardQuery>,
) -> Result<Html<String>, Redirect> {
    let auth_user = crate::web::admin::require_admin_user(&state, &jar).await?;

    let settings = state.grader_settings().await;
    let models = settings
        .as_ref()
        .map(|s| s.models.clone())
        .unwrap_or_default();
    let prompts = settings
        .as_ref()
        .map(|s| s.prompts.clone())
        .unwrap_or_default();

    let topics = fetch_journal_topics(state.pool_ref())
        .await
        .unwrap_or_else(|err| {
            error!(?err, "failed to load journal topics");
            Vec::new()
        });
    let topic_scores = fetch_journal_topic_scores(state.pool_ref())
        .await
        .unwrap_or_else(|err| {
            error!(?err, "failed to load journal topic scores");
            Vec::new()
        });
    let references = fetch_journal_references(state.pool_ref())
        .await
        .unwrap_or_else(|err| {
            error!(?err, "failed to load journal references");
            Vec::new()
        });

    let redirect_base = "/dashboard/modules/grader";
    let message_block = compose_flash_message(params.status.as_deref(), params.error.as_deref());
    let topic_html = render_topic_section(&topics, redirect_base);
    let journal_html = render_journal_section(&references, &topics, &topic_scores, redirect_base);
    let footer = render_footer();
    let shared_styles = MODULE_ADMIN_SHARED_STYLES;

    let html = format!(
        r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>稿件评估模块设置</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="robots" content="noindex,nofollow">
    <style>
        :root {{ color-scheme: light; }}
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; margin: 0; background: #f8fafc; color: #0f172a; }}
        header {{ background: #ffffff; padding: 2rem 1.5rem; border-bottom: 1px solid #e2e8f0; }}
        .header-bar {{ display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 1rem; }}
        .back-link {{ display: inline-flex; align-items: center; gap: 0.4rem; color: #1d4ed8; text-decoration: none; font-weight: 600; background: #e0f2fe; padding: 0.5rem 0.95rem; border-radius: 999px; border: 1px solid #bfdbfe; transition: background 0.15s ease, border 0.15s ease; }}
        .back-link:hover {{ background: #bfdbfe; border-color: #93c5fd; }}
        main {{ padding: 2rem 1.5rem; max-width: 1100px; margin: 0 auto; box-sizing: border-box; }}
        .panel {{ background: #ffffff; border-radius: 12px; border: 1px solid #e2e8f0; padding: 1.5rem; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); margin-bottom: 2rem; }}
        label {{ display: block; margin-bottom: 0.5rem; font-weight: 600; color: #0f172a; }}
        input[type="text"], textarea {{ width: 100%; padding: 0.75rem; border-radius: 8px; border: 1px solid #cbd5f5; background: #f8fafc; color: #0f172a; box-sizing: border-box; font-family: inherit; }}
        textarea {{ min-height: 160px; }}
        input[type="text"]:focus, textarea:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        button {{ padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        .flash {{ padding: 1rem; border-radius: 8px; margin-bottom: 1.5rem; border: 1px solid transparent; }}
        .flash.success {{ background: #ecfdf3; border-color: #bbf7d0; color: #166534; }}
        .flash.error {{ background: #fef2f2; border-color: #fecaca; color: #b91c1c; }}
        table {{ width: 100%; border-collapse: collapse; margin-top: 1.5rem; background: #ffffff; border: 1px solid #e2e8f0; border-radius: 12px; overflow: hidden; }}
        th, td {{ padding: 0.75rem 1rem; border-bottom: 1px solid #e2e8f0; text-align: left; }}
        th {{ background: #f1f5f9; color: #0f172a; font-weight: 600; }}
        .topic-grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(190px, 1fr)); gap: 1rem; margin-bottom: 1rem; }}
        .topic-picker {{ display: flex; flex-direction: column; gap: 0.45rem; padding: 0.75rem; background: #f8fafc; border: 1px solid #dbeafe; border-radius: 10px; transition: border 0.15s ease, box-shadow 0.15s ease, background 0.15s ease; }}
        .topic-picker select {{ padding: 0.6rem; border-radius: 8px; border: 1px solid #cbd5f5; background: #ffffff; }}
        .topic-picker.active {{ border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); background: #eff6ff; }}
        .journal-form-actions {{ display: flex; flex-wrap: wrap; gap: 0.75rem; margin-top: 1rem; }}
        button.secondary {{ background: #ffffff; color: #1d4ed8; border: 1px solid #93c5fd; }}
        button.secondary:hover {{ background: #dbeafe; }}
        .action-stack {{ display: flex; flex-direction: column; gap: 0.5rem; }}
        .action-stack form {{ margin: 0; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
{shared_styles}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>稿件评估模块设置</h1>
            <a class="back-link" href="/tools/grader">← 返回评估工具</a>
        </div>
        <p>配置稿件评估与期刊匹配使用的模型、提示词、主题与期刊阈值。</p>
    </header>
    <main>
        <p>当前登录：<strong>{username}</strong></p>
        {message_block}
        <section class="panel">
            <h2>模型配置</h2>
            <form method="post" action="/dashboard/modules/grader/models">
                <input type="hidden" name="redirect" value="{redirect_base}">
                <label for="grader-model">评分模型</label>
                <input id="grader-model" name="grading_model" type="text" value="{grading_model}" required>
                <label for="keyword-model">关键词模型</label>
                <input id="keyword-model" name="keyword_model" type="text" value="{keyword_model}" required>
                <button type="submit">保存模型</button>
            </form>
        </section>
        <section class="panel">
            <h2>提示词配置</h2>
            <form method="post" action="/dashboard/modules/grader/prompts">
                <input type="hidden" name="redirect" value="{redirect_base}">
                <label for="grader-instructions">评分提示词</label>
                <textarea id="grader-instructions" name="grading_instructions" required>{grading_prompt}</textarea>
                <label for="keyword-selection">关键词识别提示词</label>
                <textarea id="keyword-selection" name="keyword_selection" required>{keyword_prompt}</textarea>
                <button type="submit">保存提示词</button>
            </form>
        </section>
        {topic_html}
        {journal_html}
        {footer}
    </main>
</body>
</html>"##,
        username = escape_html(&auth_user.username),
        message_block = message_block,
        redirect_base = redirect_base,
        grading_model = escape_html(&models.grading_model),
        keyword_model = escape_html(&models.keyword_model),
        grading_prompt = escape_html(&prompts.grading_instructions),
        keyword_prompt = escape_html(&prompts.keyword_selection),
        topic_html = topic_html,
        journal_html = journal_html,
        footer = footer,
        shared_styles = shared_styles,
    );

    Ok(Html(html))
}

pub async fn save_models(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GraderModelForm>,
) -> Result<Redirect, Redirect> {
    let _admin = crate::web::admin::require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_module_redirect(form.redirect.as_deref());

    let grading = form.grading_model.trim();
    let keyword = form.keyword_model.trim();
    if grading.is_empty() || keyword.is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=grader_invalid_models"
        )));
    }

    let payload = GraderModels {
        grading_model: grading.to_string(),
        keyword_model: keyword.to_string(),
    };

    if let Err(err) = update_grader_models(state.pool_ref(), &payload).await {
        error!(?err, "failed to update grader models");
        return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
    }

    if let Err(err) = state.reload_settings().await {
        error!(
            ?err,
            "failed to reload module settings after grader model update"
        );
    }

    Ok(Redirect::to(&format!(
        "{redirect_base}?status=grader_models_saved"
    )))
}

pub async fn save_prompts(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<GraderPromptForm>,
) -> Result<Redirect, Redirect> {
    let _admin = crate::web::admin::require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_module_redirect(form.redirect.as_deref());

    if form.grading_instructions.trim().is_empty() || form.keyword_selection.trim().is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=grader_invalid_prompts"
        )));
    }

    let payload = GraderPrompts {
        grading_instructions: form.grading_instructions.trim().to_string(),
        keyword_selection: form.keyword_selection.trim().to_string(),
    };

    if let Err(err) = update_grader_prompts(state.pool_ref(), &payload).await {
        error!(?err, "failed to update grader prompts");
        return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
    }

    if let Err(err) = state.reload_settings().await {
        error!(
            ?err,
            "failed to reload module settings after grader prompt update"
        );
    }

    Ok(Redirect::to(&format!(
        "{redirect_base}?status=grader_prompts_saved"
    )))
}
