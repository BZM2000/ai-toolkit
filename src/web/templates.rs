use chrono::{Datelike, Utc};

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
