use axum::{
    extract::{Query, State},
    response::Html,
};
use axum_extra::extract::cookie::CookieJar;
use tracing::error;
use uuid::Uuid;

use crate::web::{AppState, AuthUser, auth, escape_html, render_footer, render_login_page};

use serde::Deserialize;

#[derive(Default, Deserialize)]
pub struct LandingQuery {
    pub status: Option<String>,
    pub error: Option<String>,
}

pub async fn landing_page(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<LandingQuery>,
) -> Html<String> {
    let maybe_user = if let Some(cookie) = jar.get(auth::SESSION_COOKIE) {
        if let Ok(token) = Uuid::parse_str(cookie.value()) {
            let pool = state.pool();
            match auth::fetch_user_by_session(&pool, token).await {
                Ok(user) => user,
                Err(err) => {
                    error!(?err, "failed to resolve session for landing page");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    if let Some(user) = maybe_user {
        Html(render_main_page(&user, &params))
    } else {
        Html(render_login_page())
    }
}

fn render_main_page(user: &AuthUser, params: &LandingQuery) -> String {
    let username = escape_html(&user.username);
    let flash = compose_landing_flash(params);
    let footer = render_footer();

    let modules = [
        (
            "文档摘要与翻译",
            "上传 PDF、Word 或文本文件，生成结构化摘要并输出中文译文。",
            "/tools/summarizer",
        ),
        (
            "系统综述信息提取",
            "批量上传论文与提取模板，自动抽取研究地点、样本量等自定义字段。",
            "/tools/infoextract",
        ),
        (
            "DOCX 文档翻译",
            "上传 Word 文档，利用术语表逐段翻译。",
            "/tools/translatedocx",
        ),
        (
            "稿件评估与期刊推荐",
            "评估稿件投稿级别并给出匹配期刊建议。",
            "/tools/grader",
        ),
        (
            "审稿助手",
            "上传学术稿件，通过8个模型并行审稿，生成元审稿报告和事实核查。",
            "/tools/reviewer",
        ),
    ];

    let module_cards = modules
        .iter()
        .map(|(title, description, href)| {
            format!(
                r#"<a class="module-card" href="{href}"><h2>{title}</h2><p>{description}</p><span class="cta">进入工具 →</span></a>"#,
                title = escape_html(title),
                description = escape_html(description),
                href = href,
            )
        })
        .collect::<String>();

    let admin_button = if user.is_admin {
        r#"<a class="admin-link" href="/dashboard">管理后台</a>"#.to_string()
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>张圆教授课题组 AI 工具箱</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="robots" content="noindex,nofollow">
    <style>
        :root {{ color-scheme: light; }}
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; margin: 0; background: #f8fafc; color: #0f172a; min-height: 100vh; display: flex; flex-direction: column; }}
        header {{ background: #ffffff; padding: clamp(2rem, 4vw, 2.75rem) clamp(1.5rem, 6vw, 3rem); display: flex; flex-direction: column; gap: 1rem; border-bottom: 1px solid #e2e8f0; }}
        .header-top {{ display: flex; flex-direction: column; gap: 0.5rem; }}
        .header-top h1 {{ margin: 0; font-size: clamp(1.9rem, 3vw, 2.4rem); }}
        .header-top p {{ margin: 0; color: #64748b; }}
        .header-actions {{ display: flex; flex-wrap: wrap; align-items: center; gap: 1rem; }}
        .header-actions span {{ color: #475569; font-size: 0.95rem; }}
        .logout-form button {{ padding: 0.6rem 1.3rem; border: none; border-radius: 999px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }}
        .logout-form button:hover {{ background: #1d4ed8; }}
        main {{ flex: 1; padding: clamp(2rem, 5vw, 3rem); max-width: 1100px; margin: 0 auto; width: 100%; box-sizing: border-box; }}
        .flash {{ padding: 1rem 1.25rem; border-radius: 10px; margin-bottom: 1.5rem; font-weight: 600; border: 1px solid transparent; }}
        .flash.success {{ background: #ecfdf3; border-color: #bbf7d0; color: #166534; }}
        .flash.error {{ background: #fef2f2; border-color: #fecaca; color: #b91c1c; }}
        .modules-grid {{ display: grid; gap: 1.5rem; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); }}
        .module-card {{ display: block; background: #ffffff; padding: 1.75rem; border-radius: 16px; text-decoration: none; color: inherit; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); transition: transform 0.15s ease, box-shadow 0.15s ease, border 0.15s ease; border: 1px solid #e2e8f0; }}
        .module-card:hover {{ transform: translateY(-4px); box-shadow: 0 24px 55px rgba(15, 23, 42, 0.12); border-color: #bfdbfe; }}
        .module-card h2 {{ margin-top: 0; margin-bottom: 0.75rem; font-size: 1.25rem; }}
        .module-card p {{ margin: 0 0 1.25rem 0; color: #475569; font-size: 0.95rem; line-height: 1.6; }}
        .module-card .cta {{ font-weight: 600; color: #2563eb; }}
        .admin-link {{ display: inline-flex; align-items: center; justify-content: center; margin-top: 2.5rem; padding: 0.85rem 1.5rem; border-radius: 12px; background: #e0f2fe; color: #1d4ed8; text-decoration: none; font-weight: 600; border: 1px solid #bfdbfe; transition: background 0.15s ease, border 0.15s ease; }}
        .admin-link:hover {{ background: #bfdbfe; border-color: #93c5fd; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
    </style>
</head>
<body>
    <header>
        <div class="header-top">
            <h1>张圆教授课题组 AI 工具箱</h1>
            <p>请选择功能模块开始使用。</p>
        </div>
        <div class="header-actions">
            <span>当前登录：<strong>{username}</strong></span>
            <form class="logout-form" method="post" action="/logout">
                <button type="submit">退出登录</button>
            </form>
        </div>
    </header>
    <main>
        {flash}
        <div class="modules-grid">
            {module_cards}
        </div>
        {admin_button}
        {footer}
    </main>
</body>
</html>"#,
        username = username,
        flash = flash,
        module_cards = module_cards,
        admin_button = admin_button,
        footer = footer,
    )
}

fn compose_landing_flash(params: &LandingQuery) -> String {
    if let Some(status) = params.status.as_deref() {
        if status == "logged_out" {
            return r#"<div class="flash success">已退出登录。</div>"#.to_string();
        }
    }

    if let Some(error) = params.error.as_deref() {
        let message = match error {
            "not_authorized" => "该操作需要管理员权限。",
            _ => "发生未知错误，请稍后重试。",
        };

        return format!(r#"<div class="flash error">{message}</div>"#);
    }

    String::new()
}
