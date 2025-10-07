use std::collections::HashMap;

use anyhow::Result;
use axum::{
    extract::{Query, State},
    response::{Html, Redirect},
};
use axum_extra::extract::cookie::CookieJar;
use sqlx::PgPool;
use tracing::error;
use uuid::Uuid;

use crate::{
    usage,
    web::{AppState, admin_utils::compose_flash_message, escape_html, render_footer},
};

use super::{auth::require_admin_user, types::DashboardQuery};

pub async fn dashboard(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(params): Query<DashboardQuery>,
) -> Result<Html<String>, Redirect> {
    let auth_user = require_admin_user(&state, &jar).await?;

    let users = fetch_dashboard_users(state.pool_ref())
        .await
        .map_err(|err| {
            error!(?err, "failed to load dashboard users");
            Redirect::to("/login")
        })?;

    let user_ids: Vec<Uuid> = users.iter().map(|user| user.id).collect();
    let usage_map = usage::usage_for_users(state.pool_ref(), &user_ids)
        .await
        .unwrap_or_default();

    let groups = fetch_usage_groups_with_limits(state.pool_ref())
        .await
        .map_err(|err| {
            error!(?err, "failed to load usage groups");
            Redirect::to("/login")
        })?;

    if groups.is_empty() {
        error!("no usage groups configured");
        return Err(Redirect::to("/login"));
    }

    let mut group_lookup: HashMap<Uuid, HashMap<String, GroupLimitDisplay>> = HashMap::new();
    let mut group_options_for_create = String::new();
    let mut group_options_for_assign = String::new();

    for (idx, group) in groups.iter().enumerate() {
        group_lookup.insert(group.id, group.limits.clone());
        let option = format!(
            "<option value=\"{value}\"{selected}>{label}</option>",
            value = escape_html(&group.id.to_string()),
            label = escape_html(&group.name),
            selected = if idx == 0 { " selected" } else { "" }
        );
        group_options_for_create.push_str(&option);
        group_options_for_assign.push_str(&format!(
            "<option value=\"{value}\">{label}</option>",
            value = escape_html(&group.id.to_string()),
            label = escape_html(&group.name)
        ));
    }

    let mut table_rows = String::new();

    if users.is_empty() {
        table_rows.push_str("<tr><td colspan=\"5\">当前还没有用户。</td></tr>");
    } else {
        for user in &users {
            let role = if user.is_admin {
                "管理员"
            } else {
                "普通用户"
            };
            let highlight_class = if user.username == auth_user.username {
                "current-user"
            } else {
                ""
            };

            let usage_entries = usage_map.get(&user.id);
            let limit_entries = group_lookup.get(&user.usage_group_id);

            let mut chips = String::new();
            let mut total_units = 0;
            let mut total_tokens = 0;
            for descriptor in usage::REGISTERED_MODULES {
                let usage_snapshot = usage_entries.and_then(|map| map.get(descriptor.key));
                let units_used = usage_snapshot.map(|s| s.units).unwrap_or(0);
                let tokens_used = usage_snapshot.map(|s| s.tokens).unwrap_or(0);

                total_units += units_used;
                total_tokens += tokens_used;

                let limit_snapshot = limit_entries.and_then(|map| map.get(descriptor.key));

                let unit_text = match limit_snapshot.and_then(|l| l.unit_limit) {
                    Some(limit) => format!(
                        "{units_used}/{limit} {label}",
                        label = descriptor.unit_label
                    ),
                    None => format!("{units_used} {label}", label = descriptor.unit_label),
                };
                let token_text = match limit_snapshot.and_then(|l| l.token_limit) {
                    Some(limit) => format!("{tokens_used}/{limit} 令牌"),
                    None => format!("{tokens_used} 令牌"),
                };

                chips.push_str(&format!(
                    "<div class=\"usage-chip\"><span class=\"chip-title\">{title}</span><span>{unit}</span><span>{tokens}</span></div>",
                    title = escape_html(descriptor.label),
                    unit = escape_html(&unit_text),
                    tokens = escape_html(&token_text),
                ));
            }

            let usage_detail_html = format!("<div class=\"usage-grid\">{chips}</div>");
            let usage_summary = format!("{total_units} 项 · {total_tokens} 令牌");

            let mut group_select = format!(
                "<form method=\"post\" action=\"/dashboard/users/group\" class=\"inline-form\" onsubmit=\"return confirm('确认更改 {} 的额度组？');\">",
                escape_html(&user.username)
            );
            group_select.push_str(&format!(
                "<input type=\"hidden\" name=\"username\" value=\"{}\">",
                escape_html(&user.username)
            ));
            group_select.push_str("<select name=\"usage_group_id\" class=\"inline-select\" onchange=\"this.form.submit()\">");
            for group in &groups {
                let selected = if group.id == user.usage_group_id {
                    " selected"
                } else {
                    ""
                };
                group_select.push_str(&format!(
                    "<option value=\"{}\"{}>{}</option>",
                    escape_html(&group.id.to_string()),
                    selected,
                    escape_html(&group.name)
                ));
            }
            group_select.push_str("</select></form>");

            table_rows.push_str(&format!(
                "<tr class=\"user-row {highlight}\" data-user-id=\"{id}\"><td><span class=\"expand-icon\">▶</span> {name}</td><td>{group_dropdown}</td><td>{role}</td><td class=\"usage-summary\">{summary}</td><td class=\"actions\"><button class=\"btn-sm\" onclick=\"toggleUserDetails('{id}')\">详情</button><button class=\"btn-sm btn-warning\" data-username=\"{username}\" onclick=\"resetPassword(this)\">重置密码</button></td></tr>",
                id = user.id,
                name = escape_html(&user.username),
                username = escape_html(&user.username),
                group_dropdown = group_select,
                role = role,
                summary = escape_html(&usage_summary),
                highlight = highlight_class
            ));

            table_rows.push_str(&format!(
                "<tr class=\"user-detail-row\" id=\"detail-{id}\" style=\"display: none;\"><td colspan=\"5\">{usage}</td></tr>",
                id = user.id,
                usage = usage_detail_html
            ));
        }
    }

    let message_block = compose_flash_message(params.status.as_deref(), params.error.as_deref());

    let user_controls = format!(
        r##"<div class=\"admin-actions\">
    <button class=\"btn-primary\" onclick=\"openCreateUserModal()\">+ 创建用户</button>
</div>
<div id=\"create-user-modal\" class=\"modal\">
    <div class=\"modal-content\">
        <div class=\"modal-header\">
            <h3>创建新用户</h3>
        </div>
        <form method=\"post\" action=\"/dashboard/users\">
            <div class=\"field\">
                <label for=\"new-username\">用户名</label>
                <input id=\"new-username\" name=\"username\" required>
            </div>
            <div class=\"field\">
                <label for=\"new-password\">密码</label>
                <input id=\"new-password\" type=\"password\" name=\"password\" required>
            </div>
            <div class=\"field\">
                <label for=\"new-group\">额度组</label>
                <select id=\"new-group\" name=\"usage_group_id\" required>
                    {group_options}
                </select>
            </div>
            <div class=\"field checkbox\">
                <label><input type=\"checkbox\" name=\"is_admin\" value=\"on\"> 授予管理员权限</label>
            </div>
            <div class=\"modal-actions\">
                <button type=\"button\" class=\"btn-sm\" onclick=\"closeCreateUserModal()\">取消</button>
                <button type=\"submit\">创建用户</button>
            </div>
        </form>
    </div>
</div>"##,
        group_options = group_options_for_create,
    );

    let mut group_sections = String::from("<h2 class=\"section-title\">管理额度组</h2>");
    for group in &groups {
        let mut module_fields = String::new();
        for descriptor in usage::REGISTERED_MODULES {
            let limit = group.limits.get(descriptor.key);
            let units_value = limit
                .and_then(|l| l.unit_limit)
                .map(|v| v.to_string())
                .unwrap_or_default();
            let tokens_value = limit
                .and_then(|l| l.token_limit)
                .map(|v| v.to_string())
                .unwrap_or_default();

            module_fields.push_str(&format!(
                r#"<div class=\"field-set\">
        <h3>{title}</h3>
        <div class=\"dual-inputs\">
            <div class=\"field\">
                <label for=\"units-{key}-{id}\">{unit_label}（近 7 日）</label>
                <input id=\"units-{key}-{id}\" name=\"units_{key}\" value=\"{units}\" placeholder=\"留空表示不限\">
            </div>
            <div class=\"field\">
                <label for=\"tokens-{key}-{id}\">令牌上限（近 7 日）</label>
                <input id=\"tokens-{key}-{id}\" name=\"tokens_{key}\" value=\"{tokens}\" placeholder=\"留空表示不限\">
            </div>
        </div>
    </div>"#,
                title = escape_html(descriptor.label),
                key = descriptor.key,
                id = group.id,
                unit_label = descriptor.unit_label,
                units = escape_html(&units_value),
                tokens = escape_html(&tokens_value),
            ));
        }

        let desc_display = group
            .description
            .as_ref()
            .map(|d| escape_html(d))
            .unwrap_or_else(|| "无描述".to_string());
        let desc_value = group
            .description
            .as_ref()
            .map(|d| escape_html(d))
            .unwrap_or_default();

        let section_id = format!("group-{}", group.id);
        group_sections.push_str(&format!(
            r##"<section class=\"admin collapsible-section group-panel\">
    <h2 class=\"section-header\" onclick=\"toggleSection('{section_id}')\">
        <span class=\"toggle-icon\" id=\"icon-{section_id}\">▶</span> 额度组：{name}
    </h2>
    <div class=\"section-content collapsed\" id=\"content-{section_id}\">
        <p class=\"meta-note\">{desc}</p>
        <form method=\"post\" action=\"/dashboard/usage-groups\">
            <input type=\"hidden\" name=\"group_id\" value=\"{id}\">
            <div class=\"field\">
                <label for=\"group-name-{id}\">组名称</label>
                <input id=\"group-name-{id}\" name=\"name\" value=\"{name}\" required>
            </div>
            <div class=\"field\">
                <label for=\"group-desc-{id}\">描述</label>
                <input id=\"group-desc-{id}\" name=\"description\" value=\"{desc_value}\" placeholder=\"可选\">
            </div>
            {module_fields}
            <div class=\"action-stack\">
                <button type=\"submit\">保存额度</button>
            </div>
        </form>
    </div>
</section>"##,
            id = group.id,
            section_id = section_id,
            name = escape_html(&group.name),
            desc = desc_display,
            desc_value = desc_value,
            module_fields = module_fields,
        ));
    }

    let mut new_group_fields = String::new();
    for descriptor in usage::REGISTERED_MODULES {
        new_group_fields.push_str(&format!(
            r#"<div class=\"field-set\">
        <h3>{title}</h3>
        <div class=\"dual-inputs\">
            <div class=\"field\">
                <label for=\"new-units-{key}\">{unit_label}（近 7 日）</label>
                <input id=\"new-units-{key}\" name=\"units_{key}\" placeholder=\"留空表示不限\">
            </div>
            <div class=\"field\">
                <label for=\"new-tokens-{key}\">令牌上限（近 7 日）</label>
                <input id=\"new-tokens-{key}\" name=\"tokens_{key}\" placeholder=\"留空表示不限\">
            </div>
        </div>
    </div>"#,
            title = escape_html(descriptor.label),
            key = descriptor.key,
            unit_label = descriptor.unit_label,
        ));
    }

    let new_group_section = format!(
        r##"<section class=\"admin\">
    <h2>创建新额度组</h2>
    <button class=\"btn-primary\" onclick=\"openCreateGroupModal()\" type=\"button\">+ 创建额度组</button>
    <div id=\"create-group-modal\" class=\"modal\">
        <div class=\"modal-content modal-large\">
            <div class=\"modal-header\">
                <h3>新额度组</h3>
            </div>
            <form method=\"post\" action=\"/dashboard/usage-groups\">
                <input type=\"hidden\" name=\"group_id\" value=\"\">
                <div class=\"field\">
                    <label for=\"new-group-name\">组名称</label>
                    <input id=\"new-group-name\" name=\"name\" required>
                </div>
                <div class=\"field\">
                    <label for=\"new-group-desc\">描述</label>
                    <input id=\"new-group-desc\" name=\"description\" placeholder=\"可选\">
                </div>
                {new_group_fields}
                <div class=\"modal-actions\">
                    <button type=\"button\" class=\"btn-sm\" onclick=\"closeCreateGroupModal()\">取消</button>
                    <button type=\"submit\">保存额度</button>
                </div>
            </form>
        </div>
    </div>
</section>"##,
        new_group_fields = new_group_fields,
    );

    let footer = render_footer();

    let html = format!(
        r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>使用情况仪表盘</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="robots" content="noindex,nofollow">
    <style>
        :root {{ color-scheme: light; }}
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; margin: 0; background: #f8fafc; color: #0f172a; }}
        header {{ background: #ffffff; padding: 2rem 1.5rem; border-bottom: 1px solid #e2e8f0; }}
        .header-bar {{ display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 1rem; }}
        .back-link {{ display: inline-flex; align-items: center; gap: 0.4rem; color: #1d4ed8; text-decoration: none; font-weight: 600; background: #e0f2fe; padding: 0.5rem 0.95rem; border-radius: 999px; border: 1px solid #bfdbfe; transition: background 0.15s ease, border 0.15s ease; }}
        .back-link:hover {{ background: #bfdbfe; border-color: #93c5fd; }}
        main {{ padding: 2rem 1.5rem; max-width: 1080px; margin: 0 auto; box-sizing: border-box; }}
        table {{ width: 100%; border-collapse: collapse; background: #ffffff; border-radius: 12px; overflow: hidden; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); }}
        thead {{ background: #f1f5f9; }}
        th, td {{ padding: 0.95rem 1rem; border-bottom: 1px solid #e2e8f0; text-align: left; }}
        tr.user-row {{ cursor: pointer; transition: background 0.15s ease; }}
        tr.user-row:hover {{ background: #f8fafc; }}
        tr.user-row.expanded {{ background: #dbeafe; }}
        tr.user-row.current-user {{ border-left: 4px solid #2563eb; }}
        tr.user-detail-row td {{ background: #f8fafc; }}
        .usage-summary {{ font-weight: 600; color: #1e293b; }}
        .usage-grid {{ display: grid; gap: 0.75rem; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); }}
        .usage-chip {{ background: #f1f5f9; border: 1px solid #e2e8f0; border-radius: 12px; padding: 0.85rem; display: flex; flex-direction: column; gap: 0.4rem; }}
        .usage-chip .chip-title {{ font-weight: 600; color: #1d4ed8; }}
        .admin {{ margin-top: 2rem; padding: 2rem; background: #ffffff; border-radius: 12px; box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08); border: 1px solid #e2e8f0; }}
        .collapsible-section {{ border: none; padding: 0; }}
        .section-header {{ margin: 0; padding: 1rem 1.25rem; background: #f1f5f9; border-bottom: 1px solid #e2e8f0; cursor: pointer; display: flex; align-items: center; gap: 0.5rem; font-size: 1.05rem; }}
        .section-header:hover {{ background: #e2e8f0; }}
        .toggle-icon {{ font-size: 0.85rem; color: #475569; }}
        .section-content {{ padding: 1.5rem 1.25rem; display: none; flex-direction: column; gap: 1.75rem; }}
        .section-content.collapsed {{ display: none; }}
        .section-content:not(.collapsed) {{ display: flex; }}
        .field-set {{ padding: 1.5rem; border: 1px solid #e2e8f0; border-radius: 12px; background: #f8fafc; }}
        .dual-inputs {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 1rem; }}
        .field {{ display: flex; flex-direction: column; gap: 0.35rem; }}
        .field label {{ font-weight: 600; color: #0f172a; }}
        .field input {{ padding: 0.75rem; border-radius: 8px; border: 1px solid #cbd5f5; background: #ffffff; color: #0f172a; }}
        .field input:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        .meta-note {{ margin-bottom: 0.5rem; color: #64748b; font-size: 0.95rem; }}
        .group-panel {{ margin-top: 1rem; margin-bottom: 1rem; border: 1px solid #e2e8f0; border-radius: 12px; background: #ffffff; }}
        .group-panel .section-header {{ border-radius: 12px; margin: 0; }}
        .group-panel .section-content {{ border-radius: 0 0 12px 12px; }}
        .app-footer {{ margin-top: 3rem; text-align: center; font-size: 0.85rem; color: #94a3b8; }}
        .inline-form {{ margin: 0; display: inline; }}
        .inline-select {{ padding: 0.5rem 0.75rem; border-radius: 6px; border: 1px solid #cbd5e1; background: #ffffff; color: #0f172a; font-size: 0.9rem; cursor: pointer; transition: border-color 0.15s ease, box-shadow 0.15s ease; }}
        .inline-select:hover {{ border-color: #94a3b8; }}
        .inline-select:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        .btn-warning {{ background: #f59e0b; color: #ffffff; margin-left: 0.5rem; }}
        .btn-warning:hover {{ background: #d97706; }}
        .actions {{ display: flex; gap: 0.5rem; justify-content: flex-end; }}
        .modal {{ display: none; position: fixed; z-index: 1000; left: 0; top: 0; width: 100%; height: 100%; background: rgba(0, 0, 0, 0.5); }}
        .modal-content {{ background: #ffffff; margin: 10% auto; padding: 2rem; border-radius: 12px; max-width: 400px; box-shadow: 0 4px 6px rgba(0, 0, 0, 0.1); }}
        .modal-content.modal-large {{ max-width: 600px; max-height: 80vh; overflow-y: auto; }}
        .modal-header {{ margin-bottom: 1.5rem; }}
        .modal-header h3 {{ margin: 0; color: #0f172a; }}
        .modal-actions {{ display: flex; gap: 0.75rem; justify-content: flex-end; margin-top: 1.5rem; }}
        .modal-actions button {{ padding: 0.75rem 1.25rem; }}
        .admin-actions {{ margin: 1.5rem 0; display: flex; gap: 0.75rem; flex-wrap: wrap; }}
        .btn-primary {{ padding: 0.85rem 1.5rem; border: none; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; cursor: pointer; transition: background 0.15s ease; font-size: 1rem; }}
        .btn-primary:hover {{ background: #1d4ed8; }}
        .section-title {{ color: #1d4ed8; margin-top: 3rem; margin-bottom: 1rem; font-size: 1.5rem; font-weight: 700; border-bottom: 2px solid #e2e8f0; padding-bottom: 0.5rem; }}
    </style>
</head>
<body>
    <header>
        <div class=\"header-bar\">
            <h1>使用情况仪表盘</h1>
            <a class=\"back-link\" href=\"/\">← 返回首页</a>
        </div>
        <p>管理账号额度，并进入各模块的配置页面。</p>
    </header>
    <main>
        <p data-user-id=\"{auth_id}\">当前登录：<strong>{username}</strong>。</p>
        {message_block}
        <table>
            <thead>
                <tr><th>用户名</th><th>额度组</th><th>角色</th><th>近 7 日使用（摘要）</th><th>操作</th></tr>
            </thead>
            <tbody>
                {table_rows}
            </tbody>
        </table>
        {user_controls}
        {group_sections}
        {new_group}
        {footer}
    </main>
    <div id=\"password-modal\" class=\"modal\">
        <div class=\"modal-content\">
            <div class=\"modal-header\">
                <h3>重置密码</h3>
            </div>
            <form id=\"password-reset-form\" method=\"post\" action=\"/dashboard/users/password\">
                <input type=\"hidden\" name=\"username\" value=\"\">
                <p>为用户 <strong id=\"reset-username-display\"></strong> 设置新密码：</p>
                <div class=\"field\">
                    <label for=\"modal-password-input\">新密码</label>
                    <input id=\"modal-password-input\" type=\"password\" name=\"password\" required>
                </div>
                <div class=\"modal-actions\">
                    <button type=\"button\" class=\"btn-sm\" onclick=\"closeModal()\">取消</button>
                    <button type=\"submit\">确认重置</button>
                </div>
            </form>
        </div>
    </div>
    <script>
        function toggleUserDetails(userId) {{
            const detailRow = document.getElementById('detail-' + userId);
            const userRow = document.querySelector('tr.user-row[data-user-id=\"' + userId + '\"]');

            if (detailRow.style.display === 'none') {{
                detailRow.style.display = 'table-row';
                userRow.classList.add('expanded');
            }} else {{
                detailRow.style.display = 'none';
                userRow.classList.remove('expanded');
            }}
        }}

        function toggleSection(sectionId) {{
            const content = document.getElementById('content-' + sectionId);
            const icon = document.getElementById('icon-' + sectionId);

            if (content.classList.contains('collapsed')) {{
                content.classList.remove('collapsed');
                icon.textContent = '▼';
            }} else {{
                content.classList.add('collapsed');
                icon.textContent = '▶';
            }}
        }}

        function resetPassword(buttonElement) {{
            const username = buttonElement.getAttribute('data-username');
            const modal = document.getElementById('password-modal');
            const usernameSpan = document.getElementById('reset-username-display');
            const passwordInput = document.getElementById('modal-password-input');
            const form = document.getElementById('password-reset-form');

            usernameSpan.textContent = username;
            form.querySelector('input[name=\"username\"]').value = username;
            passwordInput.value = '';

            modal.style.display = 'block';
            passwordInput.focus();
        }}

        function closeModal() {{
            document.getElementById('password-modal').style.display = 'none';
        }}

        function openCreateUserModal() {{
            const modal = document.getElementById('create-user-modal');
            modal.style.display = 'block';
            document.getElementById('new-username').focus();
        }}

        function closeCreateUserModal() {{
            document.getElementById('create-user-modal').style.display = 'none';
        }}

        function openCreateGroupModal() {{
            const modal = document.getElementById('create-group-modal');
            modal.style.display = 'block';
            document.getElementById('new-group-name').focus();
        }}

        function closeCreateGroupModal() {{
            document.getElementById('create-group-modal').style.display = 'none';
        }}

        window.onclick = function(event) {{
            const passwordModal = document.getElementById('password-modal');
            const createUserModal = document.getElementById('create-user-modal');
            const createGroupModal = document.getElementById('create-group-modal');

            if (event.target === passwordModal) {{
                closeModal();
            }} else if (event.target === createUserModal) {{
                closeCreateUserModal();
            }} else if (event.target === createGroupModal) {{
                closeCreateGroupModal();
            }}
        }}
    </script>
</body>
</html>"##,
        auth_id = auth_user.id,
        username = escape_html(&auth_user.username),
        message_block = message_block,
        table_rows = table_rows,
        user_controls = user_controls,
        group_sections = group_sections,
        new_group = new_group_section,
        footer = footer,
    );

    Ok(Html(html))
}

#[allow(dead_code)]
#[derive(sqlx::FromRow)]
struct DashboardUserRow {
    id: Uuid,
    username: String,
    usage_group_id: Uuid,
    usage_group_name: String,
    is_admin: bool,
}

#[derive(Clone)]
struct UsageGroupDisplay {
    id: Uuid,
    name: String,
    description: Option<String>,
    limits: HashMap<String, GroupLimitDisplay>,
}

#[derive(sqlx::FromRow)]
struct UsageGroupRow {
    id: Uuid,
    name: String,
    description: Option<String>,
}

#[derive(Clone)]
struct GroupLimitDisplay {
    token_limit: Option<i64>,
    unit_limit: Option<i64>,
}

async fn fetch_dashboard_users(pool: &PgPool) -> sqlx::Result<Vec<DashboardUserRow>> {
    sqlx::query_as::<_, DashboardUserRow>(
        "SELECT u.id, u.username, u.usage_group_id, ug.name AS usage_group_name, u.is_admin FROM users u JOIN usage_groups ug ON ug.id = u.usage_group_id ORDER BY u.username",
    )
    .fetch_all(pool)
    .await
}

async fn fetch_usage_groups_with_limits(pool: &PgPool) -> Result<Vec<UsageGroupDisplay>> {
    let groups = sqlx::query_as::<_, UsageGroupRow>(
        "SELECT id, name, description FROM usage_groups ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    let group_ids: Vec<Uuid> = groups.iter().map(|group| group.id).collect();
    let limit_map = usage::group_limits(pool, &group_ids).await?;

    let displays = groups
        .into_iter()
        .map(|group| {
            let limits = limit_map
                .get(&group.id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(module, snapshot)| {
                    (
                        module,
                        GroupLimitDisplay {
                            token_limit: snapshot.token_limit,
                            unit_limit: snapshot.unit_limit,
                        },
                    )
                })
                .collect();

            UsageGroupDisplay {
                id: group.id,
                name: group.name,
                description: group.description,
                limits,
            }
        })
        .collect();

    Ok(displays)
}
