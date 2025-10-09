use std::borrow::Cow;

use chrono::{Datelike, Utc};

const TOOL_PAGE_BASE_STYLES: &str = r#"
        :root { color-scheme: light; }
        body { font-family: "Helvetica Neue", Arial, sans-serif; margin: 0; background: #f8fafc; color: #0f172a; }
        header { background: #ffffff; padding: 2rem 1.5rem; border-bottom: 1px solid #e2e8f0; }
        .header-bar { display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 1rem; }
        .back-link { display: inline-flex; align-items: center; gap: 0.4rem; color: #1d4ed8; text-decoration: none; font-weight: 600; background: #e0f2fe; padding: 0.5rem 0.95rem; border-radius: 999px; border: 1px solid #bfdbfe; transition: background 0.15s ease, border 0.15s ease; }
        .back-link:hover { background: #bfdbfe; border-color: #93c5fd; }
        .admin-link { display: inline-flex; align-items: center; gap: 0.35rem; color: #0f172a; background: #fee2e2; border: 1px solid #fecaca; padding: 0.45rem 0.9rem; border-radius: 999px; text-decoration: none; font-weight: 600; }
        .admin-link:hover { background: #fecaca; border-color: #fca5a5; }
        main { padding: 2rem 1.5rem; max-width: 960px; margin: 0 auto; box-sizing: border-box; }
        section { margin-bottom: 2.5rem; }
        .panel { background: #ffffff; border-radius: 12px; border: 1px solid #e2e8f0; padding: 1.5rem; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); }
        .panel h2 { margin-top: 0; }
        label { display: block; margin-bottom: 0.5rem; font-weight: 600; color: #0f172a; }
        select { width: 100%; padding: 0.75rem; border-radius: 8px; border: 1px solid #cbd5f5; background: #f8fafc; color: #0f172a; box-sizing: border-box; }
        select:focus { outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }
        input[type="checkbox"] { margin-right: 0.5rem; }
        button { padding: 0.85rem 1.2rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; }
        button:hover { background: #1d4ed8; }
        button:disabled { opacity: 0.6; cursor: not-allowed; }
        table { width: 100%; border-collapse: collapse; margin-top: 1.5rem; background: #ffffff; border: 1px solid #e2e8f0; border-radius: 12px; overflow: hidden; }
        th, td { padding: 0.75rem 1rem; border-bottom: 1px solid #e2e8f0; text-align: left; }
        th { background: #f1f5f9; color: #0f172a; font-weight: 600; }
        .status { margin-top: 1.5rem; font-size: 0.95rem; }
        .status p { margin: 0.25rem 0; }
        .status-box { margin-top: 1rem; padding: 1rem; border-radius: 12px; background: #f1f5f9; color: #0f172a; min-height: 3rem; }
        .status-box.error { color: #b91c1c; }
        .status-box.success { color: #166534; }
        .note { color: #475569; font-size: 0.95rem; line-height: 1.6; }
        .downloads a { color: #2563eb; text-decoration: none; margin-right: 1rem; font-weight: 600; }
        .downloads a:hover { text-decoration: underline; }
        .reviews { display: grid; gap: 1rem; margin-top: 1.5rem; }
        .review-card { background: #ffffff; border-radius: 12px; border: 1px solid #e2e8f0; padding: 1.25rem; box-shadow: 0 12px 30px rgba(15, 23, 42, 0.06); }
        .review-card h3 { margin-top: 0; font-size: 1rem; }
        .status-tag { display: inline-flex; align-items: center; gap: 0.4rem; padding: 0.25rem 0.75rem; border-radius: 999px; font-size: 0.85rem; font-weight: 600; }
        .status-tag.pending { background: #fef3c7; color: #92400e; }
        .status-tag.processing { background: #e0f2fe; color: #1d4ed8; }
        .status-tag.completed { background: #dcfce7; color: #166534; }
        .status-tag.failed { background: #fee2e2; color: #b91c1c; }
        .job-table { width: 100%; border-collapse: collapse; margin-top: 1rem; }
        .job-table th, .job-table td { padding: 0.65rem 0.85rem; border: 1px solid #e2e8f0; text-align: left; font-size: 0.92rem; }
        .job-table th { background: #f1f5f9; }
        .app-footer { margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }
        @media (max-width: 768px) {
            header { padding: 1.5rem 1rem; }
            main { padding: 1.5rem 1rem; }
            .header-bar { flex-direction: column; align-items: flex-start; }
            table { font-size: 0.9rem; }
            th, td { padding: 0.5rem; }
            .reviews { grid-template-columns: 1fr; }
        }
"#;

pub struct ToolAdminLink<'a> {
    pub href: &'a str,
    pub label: &'a str,
}

pub struct ToolPageLayout<'a> {
    pub meta_title: &'a str,
    pub page_heading: &'a str,
    pub username: &'a str,
    pub note_html: Cow<'a, str>,
    pub tab_group: &'a str,
    pub new_tab_label: &'a str,
    pub new_tab_html: Cow<'a, str>,
    pub history_tab_label: &'a str,
    pub history_panel_html: Cow<'a, str>,
    pub admin_link: Option<ToolAdminLink<'a>>,
    pub footer_html: Cow<'a, str>,
    pub extra_style_blocks: Vec<Cow<'a, str>>,
    pub body_scripts: Vec<Cow<'a, str>>,
}

pub fn render_tool_page(layout: ToolPageLayout<'_>) -> String {
    let ToolPageLayout {
        meta_title,
        page_heading,
        username: _username,
        note_html,
        tab_group,
        new_tab_label,
        new_tab_html,
        history_tab_label,
        history_panel_html,
        admin_link,
        footer_html,
        extra_style_blocks,
        body_scripts,
    } = layout;

    let admin_link_html = admin_link
        .map(|link| {
            format!(
                r#"<a class="admin-link" href="{href}">{label}</a>"#,
                href = link.href,
                label = link.label,
            )
        })
        .unwrap_or_default();

    let styles = std::iter::once(Cow::Borrowed(TOOL_PAGE_BASE_STYLES))
        .chain(extra_style_blocks.into_iter())
        .map(|block| block.into_owned())
        .collect::<Vec<_>>()
        .join("\n");

    let scripts = body_scripts
        .into_iter()
        .map(|script| script.into_owned())
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>{meta_title}</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="robots" content="noindex,nofollow">
    <style>
{styles}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>{page_heading}</h1>
            <div style="display:flex; gap:0.75rem; align-items:center; flex-wrap:wrap;">
                <a class="back-link" href="/">← 返回首页</a>
                {admin_link_html}
            </div>
        </div>
        <p class="note">{note_html}</p>
    </header>
    <main>
        <div class="tool-tabs" data-tab-group="{tab_group}">
            <button type="button" class="tab-toggle active" data-tab-target="new">{new_tab_label}</button>
            <button type="button" class="tab-toggle" data-tab-target="history">{history_tab_label}</button>
        </div>
        <div class="tab-container" data-tab-container="{tab_group}">
            <div class="tab-section active" data-tab-panel="new">
{new_tab_html}
            </div>
            <div class="tab-section" data-tab-panel="history">
{history_panel_html}
            </div>
        </div>
        {footer_html}
    </main>
{scripts}
</body>
</html>"#,
        meta_title = meta_title,
        page_heading = page_heading,
        note_html = note_html,
        tab_group = tab_group,
        new_tab_label = new_tab_label,
        new_tab_html = new_tab_html,
        history_tab_label = history_tab_label,
        history_panel_html = history_panel_html,
        admin_link_html = admin_link_html,
        footer_html = footer_html,
        styles = styles,
        scripts = scripts,
    )
}

pub fn render_login_page() -> String {
    let footer = render_footer();
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
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; display: flex; flex-direction: column; align-items: center; justify-content: center; min-height: 100vh; margin: 0; background: #f1f5f9; color: #0f172a; padding: 1.5rem; box-sizing: border-box; gap: 1.5rem; }}
        main {{ width: 100%; max-width: 480px; display: flex; flex-direction: column; align-items: center; gap: 1.5rem; }}
        .panel {{ background: #ffffff; padding: 2.5rem 2.25rem; border-radius: 18px; box-shadow: 0 20px 60px rgba(15, 23, 42, 0.08); width: 100%; border: 1px solid #e2e8f0; box-sizing: border-box; }}
        h1 {{ margin: 0 0 1rem; font-size: 1.8rem; text-align: center; }}
        p.description {{ margin: 0 0 1.75rem; color: #475569; text-align: center; font-size: 0.95rem; }}
        label {{ display: block; margin-top: 1.2rem; font-weight: 600; letter-spacing: 0.01em; color: #0f172a; }}
        input {{ width: 100%; padding: 0.85rem; margin-top: 0.65rem; border-radius: 10px; border: 1px solid #cbd5f5; background: #f8fafc; color: #0f172a; font-size: 1rem; box-sizing: border-box; }}
        input:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.15); }}
        button {{ margin-top: 2rem; width: 100%; padding: 0.95rem; border: none; border-radius: 10px; background: #2563eb; color: #ffffff; font-weight: 600; font-size: 1.05rem; cursor: pointer; transition: background 0.15s ease; }}
        button:hover {{ background: #1d4ed8; }}
        .app-footer {{ margin-top: 2.5rem; text-align: center; font-size: 0.85rem; color: #64748b; }}
    </style>
</head>
<body>
    <main>
        <section class="panel">
            <h1>张圆教授课题组 AI 工具箱</h1>
            <p class="description">请输入管理员分配的账号与密码。</p>
            <form method="post" action="/login">
                <label for="username">用户名</label>
                <input id="username" name="username" required>
                <label for="password">密码</label>
                <input id="password" type="password" name="password" required>
                <button type="submit">登录</button>
            </form>
        </section>
        {footer}
    </main>
</body>
</html>"#,
        footer = footer,
    )
}

pub fn render_footer() -> String {
    let current_year = Utc::now().year();
    format!(
        r#"<footer class="app-footer">© 2024-{year} 张圆教授课题组，仅限内部使用</footer>"#,
        year = current_year
    )
}

pub fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
