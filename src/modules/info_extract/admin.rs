use axum::{
    extract::{Form, Query, State},
    response::{Html, Redirect},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use crate::{
    AppState,
    config::{
        InfoExtractModels, InfoExtractPrompts, update_info_extract_models,
        update_info_extract_prompts,
    },
    escape_html, render_footer,
    web::{
        admin::DashboardQuery,
        admin_utils::{compose_flash_message, sanitize_module_redirect},
    },
};

use super::super::admin_shared::MODULE_ADMIN_SHARED_STYLES;

#[derive(Deserialize)]
pub struct ModelForm {
    pub extraction_model: String,
    #[serde(default)]
    pub redirect: Option<String>,
}

#[derive(Deserialize)]
pub struct PromptForm {
    pub system_prompt: String,
    pub response_guidance: String,
    #[serde(default)]
    pub redirect: Option<String>,
}

pub async fn settings_page(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<DashboardQuery>,
) -> Result<Html<String>, Redirect> {
    let admin = crate::web::admin::require_admin_user(&state, &jar).await?;

    let settings = state.info_extract_settings().await.unwrap_or_default();
    let models = settings.models;
    let prompts = settings.prompts;

    let redirect_base = "/dashboard/modules/infoextract";
    let message_block = compose_flash_message(params.status.as_deref(), params.error.as_deref());
    let footer = render_footer();
    let shared_styles = MODULE_ADMIN_SHARED_STYLES;

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>信息提取模块设置</title>
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
        textarea {{ min-height: 160px; }}
        input[type="text"]:focus, textarea:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        button {{ padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        .flash {{ padding: 1rem; border-radius: 8px; margin-bottom: 1.5rem; border: 1px solid transparent; }}
        .flash.success {{ background: #ecfdf3; border-color: #bbf7d0; color: #166534; }}
        .flash.error {{ background: #fef2f2; border-color: #fecaca; color: #b91c1c; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
{shared_styles}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>信息提取模块设置</h1>
            <a class="back-link" href="/tools/infoextract">← 返回模块</a>
        </div>
        <p>配置用于系统综述信息抽取的模型与提示词。</p>
    </header>
    <main>
        <p>当前登录：<strong>{admin}</strong></p>
        {message_block}
        <section class="panel">
            <h2>模型配置</h2>
            <form method="post" action="/dashboard/modules/infoextract/models">
                <input type="hidden" name="redirect" value="{redirect}">
                <label for="model">信息提取模型</label>
                <input id="model" name="extraction_model" type="text" value="{model}" required>
                <button type="submit">保存模型</button>
            </form>
        </section>
        <section class="panel">
            <h2>提示词配置</h2>
            <form method="post" action="/dashboard/modules/infoextract/prompts">
                <input type="hidden" name="redirect" value="{redirect}">
                <label for="system">系统提示词</label>
                <textarea id="system" name="system_prompt" required>{system_prompt}</textarea>
                <label for="guidance">输出指引</label>
                <textarea id="guidance" name="response_guidance" required>{response_guidance}</textarea>
                <button type="submit">保存提示词</button>
            </form>
        </section>
        {footer}
    </main>
</body>
</html>"#,
        admin = escape_html(&admin.username),
        message_block = message_block,
        redirect = redirect_base,
        model = escape_html(&models.extraction_model),
        system_prompt = escape_html(&prompts.system_prompt),
        response_guidance = escape_html(&prompts.response_guidance),
        footer = footer,
        shared_styles = shared_styles,
    );

    Ok(Html(html))
}

pub async fn save_models(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<ModelForm>,
) -> Result<Redirect, Redirect> {
    let _admin = crate::web::admin::require_admin_user(&state, &jar).await?;
    let redirect = sanitize_module_redirect(form.redirect.as_deref());

    let model = form.extraction_model.trim();
    if model.is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect}?error=infoextract_invalid_model"
        )));
    }

    let payload = InfoExtractModels {
        extraction_model: model.to_string(),
    };

    update_info_extract_models(state.pool_ref(), &payload)
        .await
        .map_err(|err| {
            tracing::error!(?err, "failed to update info extract model");
            Redirect::to(&format!("{redirect}?error=infoextract_save_failed"))
        })?;

    state.reload_settings().await.map_err(|err| {
        tracing::error!(?err, "failed to reload settings after saving model");
        Redirect::to(&format!("{redirect}?error=infoextract_reload_failed"))
    })?;

    Ok(Redirect::to(&format!(
        "{redirect}?status=infoextract_model_saved"
    )))
}

pub async fn save_prompts(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<PromptForm>,
) -> Result<Redirect, Redirect> {
    let _admin = crate::web::admin::require_admin_user(&state, &jar).await?;
    let redirect = sanitize_module_redirect(form.redirect.as_deref());

    let system = form.system_prompt.trim();
    let guidance = form.response_guidance.trim();
    if system.is_empty() || guidance.is_empty() {
        return Ok(Redirect::to(&format!(
            "{redirect}?error=infoextract_invalid_prompts"
        )));
    }

    let payload = InfoExtractPrompts {
        system_prompt: system.to_string(),
        response_guidance: guidance.to_string(),
    };

    update_info_extract_prompts(state.pool_ref(), &payload)
        .await
        .map_err(|err| {
            tracing::error!(?err, "failed to update info extract prompts");
            Redirect::to(&format!("{redirect}?error=infoextract_save_failed"))
        })?;

    state.reload_settings().await.map_err(|err| {
        tracing::error!(?err, "failed to reload settings after saving prompts");
        Redirect::to(&format!("{redirect}?error=infoextract_reload_failed"))
    })?;

    Ok(Redirect::to(&format!(
        "{redirect}?status=infoextract_prompts_saved"
    )))
}
