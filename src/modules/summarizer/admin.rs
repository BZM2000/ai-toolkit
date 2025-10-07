use axum::{
    extract::{Form, Query, State},
    response::{Html, Redirect},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use tracing::error;

use crate::{
    AppState,
    config::{
        SummarizerModels, SummarizerPrompts, update_summarizer_models, update_summarizer_prompts,
    },
    escape_html, fetch_glossary_terms, render_footer,
    web::{
        admin::DashboardQuery,
        admin_utils::{compose_flash_message, sanitize_module_redirect},
    },
};

use super::super::admin_shared::{MODULE_ADMIN_SHARED_STYLES, render_glossary_section};

#[derive(Deserialize)]
pub struct SummarizerModelForm {
    pub summary_model: String,
    pub translation_model: String,
    #[serde(default)]
    pub redirect: Option<String>,
}

#[derive(Deserialize)]
pub struct SummarizerPromptForm {
    pub research_summary: String,
    pub general_summary: String,
    pub translation: String,
    #[serde(default)]
    pub redirect: Option<String>,
}

pub async fn settings_page(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<DashboardQuery>,
) -> Result<Html<String>, Redirect> {
    let auth_user = crate::web::admin::require_admin_user(&state, &jar).await?;
    let settings = state.summarizer_settings().await;
    let models = settings
        .as_ref()
        .map(|s| s.models.clone())
        .unwrap_or_default();
    let prompts = settings
        .as_ref()
        .map(|s| s.prompts.clone())
        .unwrap_or_default();

    let glossary_terms = fetch_glossary_terms(state.pool_ref())
        .await
        .unwrap_or_else(|err| {
            error!(?err, "failed to load glossary terms");
            Vec::new()
        });

    let message_block = compose_flash_message(params.status.as_deref(), params.error.as_deref());
    let redirect_base = "/dashboard/modules/summarizer";
    let glossary_html = render_glossary_section(&glossary_terms, redirect_base);
    let footer = render_footer();
    let shared_styles = MODULE_ADMIN_SHARED_STYLES;

    let html = format!(
        r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>摘要模块设置</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="robots" content="noindex,nofollow">
    <style>
        :root {{ color-scheme: light; }}
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; margin: 0; background: #f8fafc; color: #0f172a; }}
        header {{ background: #ffffff; padding: 2rem 1.5rem; border-bottom: 1px solid #e2e8f0; }}
        .header-bar {{ display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 1rem; }}
        .back-link {{ display: inline-flex; align-items: center; gap: 0.4rem; color: #1d4ed8; text-decoration: none; font-weight: 600; background: #e0f2fe; padding: 0.5rem 0.95rem; border-radius: 999px; border: 1px solid #bfdbfe; transition: background 0.15s ease, border 0.15s ease; }}
        .back-link:hover {{ background: #bfdbfe; border-color: #93c5fd; }}
        main {{ padding: 2rem 1.5rem; max-width: 960px; margin: 0 auto; box-sizing: border-box; }}
        .panel {{ background: #ffffff; border-radius: 12px; border: 1px solid #e2e8f0; padding: 1.5rem; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); margin-bottom: 2rem; }}
        label {{ display: block; margin-bottom: 0.5rem; font-weight: 600; color: #0f172a; }}
        input[type="text"], textarea {{ width: 100%; padding: 0.75rem; border-radius: 8px; border: 1px solid #cbd5f5; background: #f8fafc; color: #0f172a; box-sizing: border-box; font-family: inherit; }}
        textarea {{ min-height: 140px; }}
        input[type="text"]:focus, textarea:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        button {{ padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        .flash {{ padding: 1rem; border-radius: 8px; margin-bottom: 1.5rem; border: 1px solid transparent; }}
        .flash.success {{ background: #ecfdf3; border-color: #bbf7d0; color: #166534; }}
        .flash.error {{ background: #fef2f2; border-color: #fecaca; color: #b91c1c; }}
        .note {{ color: #475569; font-size: 0.95rem; margin-bottom: 1rem; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
{shared_styles}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>摘要模块设置</h1>
            <a class="back-link" href="/tools/summarizer">← 返回摘要工具</a>
        </div>
        <p>配置摘要与翻译调用的模型和提示词，术语表与 DOCX 模块共用。</p>
    </header>
    <main>
        <p>当前登录：<strong>{username}</strong></p>
        {message_block}
        <section class="panel">
            <h2>模型配置</h2>
            <form method="post" action="/dashboard/modules/summarizer/models">
                <input type="hidden" name="redirect" value="{redirect_base}">
                <label for="summary-model">摘要模型</label>
                <input id="summary-model" name="summary_model" type="text" value="{summary_model}" required>
                <label for="translation-model">翻译模型</label>
                <input id="translation-model" name="translation_model" type="text" value="{translation_model}" required>
                <button type="submit">保存模型</button>
            </form>
        </section>
        <section class="panel">
            <h2>提示词配置</h2>
            <form method="post" action="/dashboard/modules/summarizer/prompts">
                <input type="hidden" name="redirect" value="{redirect_base}">
                <label for="prompt-research">科研论文摘要提示</label>
                <textarea id="prompt-research" name="research_summary" required>{research_prompt}</textarea>
                <label for="prompt-general">其他文档摘要提示</label>
                <textarea id="prompt-general" name="general_summary" required>{general_prompt}</textarea>
                <label for="prompt-translation">翻译提示（需包含 {{GLOSSARY}} ）</label>
                <textarea id="prompt-translation" name="translation" required>{translation_prompt}</textarea>
                <button type="submit">保存提示词</button>
            </form>
        </section>
        {glossary_html}
        {footer}
    </main>
</body>
</html>"##,
        username = escape_html(&auth_user.username),
        message_block = message_block,
        redirect_base = redirect_base,
        summary_model = escape_html(&models.summary_model),
        translation_model = escape_html(&models.translation_model),
        research_prompt = escape_html(&prompts.research_summary),
        general_prompt = escape_html(&prompts.general_summary),
        translation_prompt = escape_html(&prompts.translation),
        glossary_html = glossary_html,
        footer = footer,
        shared_styles = shared_styles,
    );

    Ok(Html(html))
}

pub async fn save_models(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<SummarizerModelForm>,
) -> Result<Redirect, Redirect> {
    let _admin = crate::web::admin::require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_module_redirect(form.redirect.as_deref());

    let summary = form.summary_model.trim();
    let translation = form.translation_model.trim();
    if summary.is_empty() || translation.is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=summarizer_invalid_models"
        )));
    }

    let payload = SummarizerModels {
        summary_model: summary.to_string(),
        translation_model: translation.to_string(),
    };

    if let Err(err) = update_summarizer_models(state.pool_ref(), &payload).await {
        error!(?err, "failed to update summarizer models");
        return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
    }

    if let Err(err) = state.reload_settings().await {
        error!(
            ?err,
            "failed to reload module settings after summarizer model update"
        );
    }

    Ok(Redirect::to(&format!(
        "{redirect_base}?status=summarizer_models_saved"
    )))
}

pub async fn save_prompts(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<SummarizerPromptForm>,
) -> Result<Redirect, Redirect> {
    let _admin = crate::web::admin::require_admin_user(&state, &jar).await?;
    let redirect_base = sanitize_module_redirect(form.redirect.as_deref());

    if form.research_summary.trim().is_empty()
        || form.general_summary.trim().is_empty()
        || form.translation.trim().is_empty()
    {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=summarizer_invalid_prompts"
        )));
    }

    if !form.translation.contains("{{GLOSSARY}}") {
        return Ok(Redirect::to(&format!(
            "{redirect_base}?error=summarizer_invalid_prompts"
        )));
    }

    let payload = SummarizerPrompts {
        research_summary: form.research_summary.trim().to_string(),
        general_summary: form.general_summary.trim().to_string(),
        translation: form.translation.trim().to_string(),
    };

    if let Err(err) = update_summarizer_prompts(state.pool_ref(), &payload).await {
        error!(?err, "failed to update summarizer prompts");
        return Ok(Redirect::to(&format!("{redirect_base}?error=unknown")));
    }

    if let Err(err) = state.reload_settings().await {
        error!(
            ?err,
            "failed to reload module settings after summarizer prompt update"
        );
    }

    Ok(Redirect::to(&format!(
        "{redirect_base}?status=summarizer_prompts_saved"
    )))
}
