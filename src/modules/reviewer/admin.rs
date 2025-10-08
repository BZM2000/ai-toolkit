use axum::{
    extract::{Form, Query, State},
    response::{Html, Redirect},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use crate::{
    AppState,
    config::{ReviewerModels, ReviewerPrompts, update_reviewer_models, update_reviewer_prompts},
    escape_html, render_footer,
    web::{admin::DashboardQuery, admin_utils::compose_flash_message},
};

use super::super::admin_shared::MODULE_ADMIN_SHARED_STYLES;

#[derive(Deserialize)]
pub struct ReviewerModelForm {
    pub round1_model_1: String,
    pub round1_model_2: String,
    pub round1_model_3: String,
    pub round1_model_4: String,
    pub round1_model_5: String,
    pub round1_model_6: String,
    pub round1_model_7: String,
    pub round1_model_8: String,
    pub round2_model: String,
    pub round3_model: String,
    #[serde(default)]
    pub redirect: Option<String>,
}

#[derive(Deserialize)]
pub struct ReviewerPromptForm {
    pub initial_prompt: String,
    pub initial_prompt_zh: String,
    pub secondary_prompt: String,
    pub secondary_prompt_zh: String,
    pub final_prompt: String,
    pub final_prompt_zh: String,
    #[serde(default)]
    pub redirect: Option<String>,
}

pub async fn settings_page(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<DashboardQuery>,
) -> Result<Html<String>, Redirect> {
    let auth_user = crate::web::admin::require_admin_user(&state, &jar).await?;

    let settings = state.reviewer_settings().await;
    let models = settings
        .as_ref()
        .map(|s| s.models.clone())
        .unwrap_or_default();
    let prompts = settings
        .as_ref()
        .map(|s| s.prompts.clone())
        .unwrap_or_default();

    let redirect_base = "/dashboard/modules/reviewer";
    let message_block = compose_flash_message(params.status.as_deref(), params.error.as_deref());
    let footer = render_footer();
    let shared_styles = MODULE_ADMIN_SHARED_STYLES;

    let html = format!(
        r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>审稿助手模块设置</title>
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
        .model-group {{ display: grid; grid-template-columns: 1fr; gap: 1rem; margin-bottom: 1rem; }}
        .model-subgroup {{ background: #f8fafc; padding: 1rem; border: 1px solid #e2e8f0; border-radius: 8px; }}
        .model-subgroup h3 {{ margin-top: 0; margin-bottom: 1rem; font-size: 1.05rem; color: #334155; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
{shared_styles}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>审稿助手模块设置</h1>
            <a class="back-link" href="/tools/reviewer">← 返回审稿工具</a>
        </div>
        <p>配置审稿助手使用的模型和提示词。系统会使用8个不同模型进行首轮审稿，然后使用单一模型生成元审稿和事实核查报告。</p>
    </header>
    <main>
        <p>当前登录：<strong>{username}</strong></p>
        {message_block}
        <section class="panel">
            <h2>模型配置</h2>
            <form method="post" action="/dashboard/modules/reviewer/models">
                <input type="hidden" name="redirect" value="{redirect_base}">
                <div class="model-group">
                    <div class="model-subgroup">
                        <h3>第一轮审稿模型（8个并行）</h3>
                        <label for="round1-model-1">模型 1</label>
                        <input id="round1-model-1" name="round1_model_1" type="text" value="{round1_model_1}" required>
                        <label for="round1-model-2">模型 2</label>
                        <input id="round1-model-2" name="round1_model_2" type="text" value="{round1_model_2}" required>
                        <label for="round1-model-3">模型 3</label>
                        <input id="round1-model-3" name="round1_model_3" type="text" value="{round1_model_3}" required>
                        <label for="round1-model-4">模型 4</label>
                        <input id="round1-model-4" name="round1_model_4" type="text" value="{round1_model_4}" required>
                        <label for="round1-model-5">模型 5</label>
                        <input id="round1-model-5" name="round1_model_5" type="text" value="{round1_model_5}" required>
                        <label for="round1-model-6">模型 6</label>
                        <input id="round1-model-6" name="round1_model_6" type="text" value="{round1_model_6}" required>
                        <label for="round1-model-7">模型 7</label>
                        <input id="round1-model-7" name="round1_model_7" type="text" value="{round1_model_7}" required>
                        <label for="round1-model-8">模型 8</label>
                        <input id="round1-model-8" name="round1_model_8" type="text" value="{round1_model_8}" required>
                    </div>
                    <div class="model-subgroup">
                        <h3>第二轮元审稿模型</h3>
                        <label for="round2-model">综合审稿模型</label>
                        <input id="round2-model" name="round2_model" type="text" value="{round2_model}" required>
                    </div>
                    <div class="model-subgroup">
                        <h3>第三轮事实核查模型</h3>
                        <label for="round3-model">事实核查模型</label>
                        <input id="round3-model" name="round3_model" type="text" value="{round3_model}" required>
                    </div>
                </div>
                <button type="submit">保存模型</button>
            </form>
        </section>
        <section class="panel">
            <h2>提示词配置</h2>
            <form method="post" action="/dashboard/modules/reviewer/prompts">
                <input type="hidden" name="redirect" value="{redirect_base}">
                <label for="initial-prompt">第一轮审稿提示词（英文）</label>
                <textarea id="initial-prompt" name="initial_prompt" required>{initial_prompt}</textarea>
                <label for="initial-prompt-zh">第一轮审稿提示词（中文）</label>
                <textarea id="initial-prompt-zh" name="initial_prompt_zh" required>{initial_prompt_zh}</textarea>
                <label for="secondary-prompt">第二轮元审稿提示词（英文）</label>
                <textarea id="secondary-prompt" name="secondary_prompt" required>{secondary_prompt}</textarea>
                <label for="secondary-prompt-zh">第二轮元审稿提示词（中文）</label>
                <textarea id="secondary-prompt-zh" name="secondary_prompt_zh" required>{secondary_prompt_zh}</textarea>
                <label for="final-prompt">第三轮事实核查提示词（英文）</label>
                <textarea id="final-prompt" name="final_prompt" required>{final_prompt}</textarea>
                <label for="final-prompt-zh">第三轮事实核查提示词（中文）</label>
                <textarea id="final-prompt-zh" name="final_prompt_zh" required>{final_prompt_zh}</textarea>
                <button type="submit">保存提示词</button>
            </form>
        </section>
    </main>
    {footer}
</body>
</html>"##,
        username = escape_html(&auth_user.username),
        round1_model_1 = escape_html(&models.round1_model_1),
        round1_model_2 = escape_html(&models.round1_model_2),
        round1_model_3 = escape_html(&models.round1_model_3),
        round1_model_4 = escape_html(&models.round1_model_4),
        round1_model_5 = escape_html(&models.round1_model_5),
        round1_model_6 = escape_html(&models.round1_model_6),
        round1_model_7 = escape_html(&models.round1_model_7),
        round1_model_8 = escape_html(&models.round1_model_8),
        round2_model = escape_html(&models.round2_model),
        round3_model = escape_html(&models.round3_model),
        initial_prompt = escape_html(&prompts.initial_prompt),
        initial_prompt_zh = escape_html(&prompts.initial_prompt_zh),
        secondary_prompt = escape_html(&prompts.secondary_prompt),
        secondary_prompt_zh = escape_html(&prompts.secondary_prompt_zh),
        final_prompt = escape_html(&prompts.final_prompt),
        final_prompt_zh = escape_html(&prompts.final_prompt_zh),
    );

    Ok(Html(html))
}

pub async fn save_models(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<ReviewerModelForm>,
) -> Redirect {
    if let Err(e) = crate::web::admin::require_admin_user(&state, &jar).await {
        return e;
    }

    let models = ReviewerModels {
        round1_model_1: form.round1_model_1,
        round1_model_2: form.round1_model_2,
        round1_model_3: form.round1_model_3,
        round1_model_4: form.round1_model_4,
        round1_model_5: form.round1_model_5,
        round1_model_6: form.round1_model_6,
        round1_model_7: form.round1_model_7,
        round1_model_8: form.round1_model_8,
        round2_model: form.round2_model,
        round3_model: form.round3_model,
    };

    match update_reviewer_models(state.pool_ref(), &models).await {
        Ok(_) => {
            let _ = state.reload_settings().await;
            let redirect_path = form
                .redirect
                .unwrap_or_else(|| "/dashboard/modules/reviewer".to_string());
            Redirect::to(&format!("{}?status=models_saved", redirect_path))
        }
        Err(err) => {
            let redirect_path = form
                .redirect
                .unwrap_or_else(|| "/dashboard/modules/reviewer".to_string());
            let error_msg = err.to_string().replace("&", "%26").replace("=", "%3D");
            Redirect::to(&format!("{}?error={}", redirect_path, error_msg))
        }
    }
}

pub async fn save_prompts(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<ReviewerPromptForm>,
) -> Redirect {
    if let Err(e) = crate::web::admin::require_admin_user(&state, &jar).await {
        return e;
    }

    let prompts = ReviewerPrompts {
        initial_prompt: form.initial_prompt,
        initial_prompt_zh: form.initial_prompt_zh,
        secondary_prompt: form.secondary_prompt,
        secondary_prompt_zh: form.secondary_prompt_zh,
        final_prompt: form.final_prompt,
        final_prompt_zh: form.final_prompt_zh,
    };

    match update_reviewer_prompts(state.pool_ref(), &prompts).await {
        Ok(_) => {
            let _ = state.reload_settings().await;
            let redirect_path = form
                .redirect
                .unwrap_or_else(|| "/dashboard/modules/reviewer".to_string());
            Redirect::to(&format!("{}?status=prompts_saved", redirect_path))
        }
        Err(err) => {
            let redirect_path = form
                .redirect
                .unwrap_or_else(|| "/dashboard/modules/reviewer".to_string());
            let error_msg = err.to_string().replace("&", "%26").replace("=", "%3D");
            Redirect::to(&format!("{}?error={}", redirect_path, error_msg))
        }
    }
}
