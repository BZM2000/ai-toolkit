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

    let mut group_lookup: HashMap<Uuid, UsageGroupDisplay> = HashMap::new();
    let mut group_options_for_create = String::new();
    let mut group_options_for_assign = String::new();

    for (idx, group) in groups.iter().enumerate() {
        group_lookup.insert(group.id, group.clone());
        let option = format!(
            r#"<option value="{value}"{selected}>{label}</option>"#,
            value = escape_html(&group.id.to_string()),
            label = escape_html(&group.name),
            selected = if idx == 0 { " selected" } else { "" }
        );
        group_options_for_create.push_str(&option);
        group_options_for_assign.push_str(&format!(
            r#"<option value="{value}">{label}</option>"#,
            value = escape_html(&group.id.to_string()),
            label = escape_html(&group.name)
        ));
    }

    let mut table_rows = String::new();

    if users.is_empty() {
        table_rows.push_str(r#"<tr><td colspan="5">当前还没有用户。</td></tr>"#);
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

            let usage_entry = usage_map.get(&user.id);
            let group_info = group_lookup.get(&user.usage_group_id);

            let mut chips = String::new();
            let mut total_units = 0;
            let global_tokens = usage_entry.map(|entry| entry.global_tokens).unwrap_or(0);

            let global_token_text = match group_info.and_then(|info| info.token_limit) {
                Some(limit) => format!("{global_tokens}/{limit} 令牌"),
                None => format!("{global_tokens} 令牌"),
            };

            chips.push_str(&format!(
                r#"<div class="usage-chip"><span class="chip-title">全部模块</span><span>令牌合计</span><span>{tokens}</span></div>"#,
                tokens = escape_html(&global_token_text),
            ));

            for descriptor in usage::REGISTERED_MODULES {
                let module_usage = usage_entry.and_then(|entry| entry.modules.get(descriptor.key));
                let units_used = module_usage.map(|usage| usage.units).unwrap_or(0);
                let tokens_used = module_usage.map(|usage| usage.tokens).unwrap_or(0);

                total_units += units_used;

                let unit_limit = group_info
                    .and_then(|info| info.unit_limits.get(descriptor.key))
                    .copied()
                    .flatten();

                let unit_text = match unit_limit {
                    Some(limit) => format!(
                        "{units_used}/{limit} {label}",
                        label = descriptor.unit_label
                    ),
                    None => format!("{units_used} {label}", label = descriptor.unit_label),
                };

                let token_text = format!("{tokens_used} 令牌");

                chips.push_str(&format!(
                    r#"<div class="usage-chip"><span class="chip-title">{title}</span><span>{unit}</span><span>{tokens}</span></div>"#,
                    title = escape_html(descriptor.label),
                    unit = escape_html(&unit_text),
                    tokens = escape_html(&token_text),
                ));
            }

            let usage_detail_html = format!(r#"<div class="usage-grid">{chips}</div>"#);
            let usage_summary = format!(
                "{total_units} 项 · {token_text}",
                token_text = global_token_text,
            );

            let mut group_select = format!(
                r#"<form method="post" action="/dashboard/users/group" class="inline-form" onsubmit="return confirm('确认更改 {} 的额度组？');">"#,
                escape_html(&user.username)
            );
            group_select.push_str(&format!(
                r#"<input type="hidden" name="username" value="{}">"#,
                escape_html(&user.username)
            ));
            group_select.push_str(r#"<select name="usage_group_id" class="inline-select" onchange="this.form.submit()">"#);
            for group in &groups {
                let selected = if group.id == user.usage_group_id {
                    " selected"
                } else {
                    ""
                };
                group_select.push_str(&format!(
                    r#"<option value="{}"{}>{}</option>"#,
                    escape_html(&group.id.to_string()),
                    selected,
                    escape_html(&group.name)
                ));
            }
            group_select.push_str("</select></form>");

            table_rows.push_str(&format!(
                r#"<tr class="user-row {highlight}" data-user-id="{id}"><td><span class="expand-icon">▶</span> {name}</td><td>{group_dropdown}</td><td>{role}</td><td class="usage-summary">{summary}</td><td class="actions"><button class="btn-sm" onclick="toggleUserDetails('{id}')">详情</button><button class="btn-sm btn-warning" data-username="{username}" onclick="resetPassword(this)">重置密码</button></td></tr>"#,
                id = user.id,
                name = escape_html(&user.username),
                username = escape_html(&user.username),
                group_dropdown = group_select,
                role = role,
                summary = escape_html(&usage_summary),
                highlight = highlight_class
            ));

            table_rows.push_str(&format!(
                r#"<tr class="user-detail-row" id="detail-{id}" style="display: none;"><td colspan="5">{usage}</td></tr>"#,
                id = user.id,
                usage = usage_detail_html
            ));
        }
    }

    let message_block = compose_flash_message(params.status.as_deref(), params.error.as_deref());

    let user_controls = format!(
        r##"<div class="admin-actions">
    <button class="btn-primary" onclick="openCreateUserModal()">+ 创建用户</button>
</div>
<div id="create-user-modal" class="modal">
    <div class="modal-content">
        <div class="modal-header">
            <h3>创建新用户</h3>
        </div>
        <form method="post" action="/dashboard/users">
            <div class="field">
                <label for="new-username">用户名</label>
                <input type="text" id="new-username" name="username" required>
            </div>
            <div class="field">
                <label for="new-password">密码</label>
                <input type="password" id="new-password" name="password" required>
            </div>
            <div class="field">
                <label for="new-group">额度组</label>
                <select id="new-group" name="usage_group_id" required>
                    {group_options}
                </select>
            </div>
            <div class="field checkbox">
                <label><input type="checkbox" name="is_admin" value="on"> 授予管理员权限</label>
            </div>
            <div class="modal-actions">
                <button type="button" class="btn-sm" onclick="closeCreateUserModal()">取消</button>
                <button type="submit" class="btn-primary">创建用户</button>
            </div>
        </form>
    </div>
</div>"##,
        group_options = group_options_for_create,
    );

    let mut group_sections = String::from(r#"<h2 class="section-title">管理额度组</h2>"#);
    for group in &groups {
        let mut module_fields = String::new();

        let token_attr = group
            .token_limit
            .map(|v| format!(r#" value="{}""#, v))
            .unwrap_or_default();

        module_fields.push_str(&format!(
            r#"<div class="field-set">
        <h3>全部模块</h3>
        <div class="field">
            <label for="tokens-global-{id}">令牌上限（近 7 日，全部模块共享）</label>
            <input type="number" id="tokens-global-{id}" name="tokens_global"{token_attr} placeholder="留空表示不限" min="0">
        </div>
    </div>"#,
            id = group.id,
            token_attr = token_attr,
        ));

        for descriptor in usage::REGISTERED_MODULES {
            let units_value = group.unit_limits.get(descriptor.key).copied().flatten();

            let units_attr = units_value
                .map(|v| format!(r#" value="{}""#, v))
                .unwrap_or_default();

            module_fields.push_str(&format!(
                r#"<div class="field-set">
        <h3>{title}</h3>
        <div class="field">
            <label for="units-{key}-{id}">{unit_label}（近 7 日）</label>
            <input type="number" id="units-{key}-{id}" name="units_{key}"{units_attr} placeholder="留空表示不限" min="0">
        </div>
    </div>"#,
                title = escape_html(descriptor.label),
                key = descriptor.key,
                id = group.id,
                unit_label = descriptor.unit_label,
                units_attr = units_attr,
            ));
        }

        let desc_display = group
            .description
            .as_ref()
            .map(|d| escape_html(d))
            .unwrap_or_else(|| "无描述".to_string());
        let desc_value_attr = group
            .description
            .as_ref()
            .map(|d| format!(r#" value="{}""#, escape_html(d)))
            .unwrap_or_default();

        let section_id = format!("group-{}", group.id);
        group_sections.push_str(&format!(
            r##"<section class="admin collapsible-section group-panel">
    <h2 class="section-header">
        <span class="toggle-icon">▶</span> 额度组：{name}
        <button class="btn-sm" onclick="toggleSection('{section_id}')">Edit</button>
    </h2>
    <div class="section-content collapsed" id="content-{section_id}">
        <p class="meta-note">{desc}</p>
        <form method="post" action="/dashboard/usage-groups">
            <input type="hidden" name="group_id" value="{id}">
            <div class="field">
                <label for="group-name-{id}">组名称</label>
                <input type="text" id="group-name-{id}" name="name" value="{name}" required>
            </div>
            <div class="field">
                <label for="group-desc-{id}">描述</label>
                <input type="text" id="group-desc-{id}" name="description"{desc_value_attr} placeholder="可选">
            </div>
            {module_fields}
            <div class="action-stack">
                <button type="submit" class="btn-primary">保存额度</button>
                <button type="button" class="btn-sm" onclick="toggleSection('{section_id}')">Cancel</button>
            </div>
        </form>
    </div>
</section>"##,
            id = group.id,
            section_id = section_id,
            name = escape_html(&group.name),
            desc = desc_display,
            desc_value_attr = desc_value_attr,
            module_fields = module_fields,
        ));
    }

    let mut new_group_fields = String::new();
    new_group_fields.push_str(
        r#"<div class="field-set">
        <h3>全部模块</h3>
        <div class="field">
            <label for="new-tokens-global">令牌上限（近 7 日，全部模块共享）</label>
            <input type="number" id="new-tokens-global" name="tokens_global" placeholder="留空表示不限" min="0">
        </div>
    </div>"#,
    );

    for descriptor in usage::REGISTERED_MODULES {
        new_group_fields.push_str(&format!(
            r#"<div class="field-set">
        <h3>{title}</h3>
        <div class="field">
            <label for="new-units-{key}">{unit_label}（近 7 日）</label>
            <input type="number" id="new-units-{key}" name="units_{key}" placeholder="留空表示不限" min="0">
        </div>
    </div>"#,
            title = escape_html(descriptor.label),
            key = descriptor.key,
            unit_label = descriptor.unit_label,
        ));
    }

    let new_group_section = format!(
        r##"<section class="admin">
    <h2>创建新额度组</h2>
    <button class="btn-primary" onclick="openCreateGroupModal()" type="button">+ 创建额度组</button>
    <div id="create-group-modal" class="modal">
        <div class="modal-content modal-large">
            <div class="modal-header">
                <h3>新额度组</h3>
            </div>
            <form method="post" action="/dashboard/usage-groups">
                <input type="hidden" name="group_id" value="">
                <div class="field">
                    <label for="new-group-name">组名称</label>
                    <input type="text" id="new-group-name" name="name" required>
                </div>
                <div class="field">
                    <label for="new-group-desc">描述</label>
                    <input type="text" id="new-group-desc" name="description" placeholder="可选">
                </div>
                {new_group_fields}
                <div class="modal-actions">
                    <button type="button" class="btn-sm" onclick="closeCreateGroupModal()">取消</button>
                    <button type="submit" class="btn-primary">保存额度</button>
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
        * {{ box-sizing: border-box; }}
        body {{ font-family: "Helvetica Neue", Arial, sans-serif; margin: 0; background: #f8fafc; color: #0f172a; line-height: 1.6; }}
        header {{ background: #ffffff; padding: 2rem 1.5rem; border-bottom: 1px solid #e2e8f0; box-shadow: 0 1px 3px rgba(0, 0, 0, 0.05); }}
        .header-bar {{ display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 1rem; }}
        .back-link {{ display: inline-flex; align-items: center; gap: 0.4rem; color: #1d4ed8; text-decoration: none; font-weight: 600; background: #e0f2fe; padding: 0.5rem 0.95rem; border-radius: 999px; border: 1px solid #bfdbfe; transition: all 0.2s ease; }}
        .back-link:hover {{ background: #bfdbfe; border-color: #93c5fd; }}
        .back-link:focus {{ outline: none; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.2); }}
        main {{ padding: 2rem 1.5rem; max-width: 1200px; margin: 0 auto; }}

        /* Table styles with responsive wrapper */
        .table-wrapper {{ width: 100%; overflow-x: auto; -webkit-overflow-scrolling: touch; background: #ffffff; border-radius: 12px; box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06); }}
        table {{ width: 100%; min-width: 800px; border-collapse: collapse; }}
        thead {{ background: linear-gradient(to bottom, #f8fafc, #f1f5f9); }}
        th {{ padding: 1rem 1rem; border-bottom: 2px solid #e2e8f0; text-align: left; font-weight: 700; color: #475569; font-size: 0.875rem; text-transform: uppercase; letter-spacing: 0.05em; }}
        td {{ padding: 1rem 1rem; border-bottom: 1px solid #e2e8f0; }}
        tr.user-row {{ cursor: pointer; transition: all 0.2s ease; }}
        tr.user-row:hover {{ background: #f8fafc; }}
        tr.user-row.expanded {{ background: #dbeafe; }}
        tr.user-row.current-user {{ border-left: 4px solid #2563eb; }}
        tr.user-detail-row td {{ background: #f8fafc; padding: 1.5rem; }}
        .expand-icon {{ display: inline-block; transition: transform 0.2s ease; font-size: 0.75rem; color: #64748b; }}
        tr.user-row.expanded .expand-icon {{ transform: rotate(90deg); }}
        .usage-summary {{ font-weight: 600; color: #1e293b; }}
        .usage-grid {{ display: grid; gap: 0.75rem; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); }}
        .usage-chip {{ background: linear-gradient(to bottom, #ffffff, #f8fafc); border: 1px solid #e2e8f0; border-radius: 8px; padding: 1rem; display: flex; flex-direction: column; gap: 0.5rem; transition: all 0.2s ease; }}
        .usage-chip:hover {{ border-color: #cbd5e1; box-shadow: 0 2px 4px rgba(0, 0, 0, 0.05); }}
        .usage-chip .chip-title {{ font-weight: 600; color: #1d4ed8; font-size: 0.875rem; }}

        /* Button styles */
        button {{ font-family: inherit; cursor: pointer; transition: all 0.2s ease; border: none; font-size: 0.9375rem; }}
        button:disabled {{ opacity: 0.5; cursor: not-allowed; }}
        .btn-primary {{ padding: 0.75rem 1.5rem; border-radius: 8px; background: #2563eb; color: #ffffff; font-weight: 600; box-shadow: 0 1px 2px rgba(0, 0, 0, 0.05); }}
        .btn-primary:hover:not(:disabled) {{ background: #1d4ed8; box-shadow: 0 4px 6px -1px rgba(37, 99, 235, 0.3); }}
        .btn-primary:focus {{ outline: none; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.2); }}
        .btn-sm {{ padding: 0.5rem 1rem; border-radius: 6px; background: #64748b; color: #ffffff; font-weight: 500; font-size: 0.875rem; }}
        .btn-sm:hover:not(:disabled) {{ background: #475569; }}
        .btn-sm:focus {{ outline: none; box-shadow: 0 0 0 3px rgba(100, 116, 139, 0.2); }}
        .btn-warning {{ background: #f59e0b; color: #ffffff; padding: 0.5rem 1rem; border-radius: 6px; font-weight: 500; font-size: 0.875rem; }}
        .btn-warning:hover:not(:disabled) {{ background: #d97706; box-shadow: 0 2px 4px rgba(245, 158, 11, 0.3); }}
        .btn-warning:focus {{ outline: none; box-shadow: 0 0 0 3px rgba(245, 158, 11, 0.2); }}

        /* Form styles */
        .field {{ display: flex; flex-direction: column; gap: 0.5rem; margin-bottom: 1rem; }}
        .field label {{ font-weight: 600; color: #0f172a; font-size: 0.9375rem; }}
        .field input[type="text"],
        .field input[type="password"],
        .field input[type="number"],
        .field input:not([type]) {{ padding: 0.75rem 1rem; border-radius: 8px; border: 1px solid #cbd5e1; background: #ffffff; color: #0f172a; font-size: 1rem; transition: all 0.2s ease; width: 100%; }}
        .field input[type="text"]:hover,
        .field input[type="password"]:hover,
        .field input[type="number"]:hover,
        .field input:not([type]):hover {{ border-color: #94a3b8; }}
        .field input[type="text"]:focus,
        .field input[type="password"]:focus,
        .field input[type="number"]:focus,
        .field input:not([type]):focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        .field input[type="text"]::placeholder,
        .field input[type="password"]::placeholder,
        .field input[type="number"]::placeholder,
        .field input:not([type])::placeholder {{ color: #94a3b8; }}
        .field input[type="number"]::-webkit-inner-spin-button,
        .field input[type="number"]::-webkit-outer-spin-button {{ opacity: 0.6; }}
        .field input[type="number"]:hover::-webkit-inner-spin-button,
        .field input[type="number"]:hover::-webkit-outer-spin-button {{ opacity: 1; }}
        .field select {{ padding: 0.75rem 1rem; border-radius: 8px; border: 1px solid #cbd5e1; background: #ffffff; color: #0f172a; font-size: 1rem; cursor: pointer; transition: all 0.2s ease; width: 100%; }}
        .field select:hover {{ border-color: #94a3b8; }}
        .field select:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}
        .field.checkbox {{ flex-direction: row; align-items: center; gap: 0.75rem; }}
        .field.checkbox label {{ margin: 0; font-weight: 500; }}
        .field input[type="checkbox"] {{ width: 1.25rem; height: 1.25rem; cursor: pointer; border: 2px solid #cbd5e1; border-radius: 4px; }}
        .field input[type="checkbox"]:focus {{ outline: none; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.2); }}
        .field-set {{ padding: 1.5rem; border: 1px solid #e2e8f0; border-radius: 12px; background: #f8fafc; margin-bottom: 1.5rem; }}
        .field-set h3 {{ margin: 0 0 1rem 0; color: #1e293b; font-size: 1.125rem; font-weight: 600; }}
        .dual-inputs {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(250px, 1fr)); gap: 1rem; }}
        .inline-form {{ margin: 0; display: inline; }}
        .inline-select {{ padding: 0.5rem 0.75rem; border-radius: 6px; border: 1px solid #cbd5e1; background: #ffffff; color: #0f172a; font-size: 0.875rem; cursor: pointer; transition: all 0.2s ease; }}
        .inline-select:hover {{ border-color: #94a3b8; }}
        .inline-select:focus {{ outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12); }}

        /* Admin section styles */
        .admin {{ margin-top: 2rem; padding: 2rem; background: #ffffff; border-radius: 12px; box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06); border: 1px solid #e2e8f0; }}
        .admin h2 {{ margin-top: 0; margin-bottom: 1.5rem; color: #1e293b; font-size: 1.5rem; font-weight: 700; }}
        .collapsible-section {{ border: none; padding: 0; }}
        .section-header {{ margin: 0; padding: 1rem 1.25rem; background: #f1f5f9; border-bottom: 1px solid #e2e8f0; cursor: pointer; display: flex; align-items: center; gap: 0.75rem; font-size: 1.05rem; transition: all 0.2s ease; }}
        .section-header:hover {{ background: #e2e8f0; }}
        .section-header:focus {{ outline: 2px solid #2563eb; outline-offset: -2px; }}
        .toggle-icon {{ font-size: 0.85rem; color: #475569; transition: transform 0.2s ease; }}
        .section-content {{ padding: 1.5rem 1.25rem; display: none; flex-direction: column; gap: 1.5rem; }}
        .section-content.collapsed {{ display: none; }}
        .section-content:not(.collapsed) {{ display: flex; }}
        .meta-note {{ margin-bottom: 0.75rem; color: #64748b; font-size: 0.9375rem; }}
        .group-panel {{ margin-top: 1rem; margin-bottom: 1rem; border: 1px solid #e2e8f0; border-radius: 12px; background: #ffffff; overflow: hidden; }}
        .group-panel .section-header {{ border-radius: 0; margin: 0; }}
        .group-panel .section-content {{ border-radius: 0; }}
        .section-title {{ color: #1d4ed8; margin-top: 3rem; margin-bottom: 1rem; font-size: 1.5rem; font-weight: 700; border-bottom: 2px solid #e2e8f0; padding-bottom: 0.5rem; }}
        .action-stack {{ display: flex; gap: 0.75rem; margin-top: 1.5rem; }}
        .actions {{ display: flex; gap: 0.5rem; justify-content: flex-end; align-items: center; }}
        .admin-actions {{ margin: 1.5rem 0; display: flex; gap: 0.75rem; flex-wrap: wrap; }}

        /* Modal styles */
        .modal {{ display: none; position: fixed; z-index: 1000; left: 0; top: 0; width: 100%; height: 100%; background: rgba(15, 23, 42, 0.6); backdrop-filter: blur(2px); animation: fadeIn 0.2s ease; }}
        @keyframes fadeIn {{ from {{ opacity: 0; }} to {{ opacity: 1; }} }}
        .modal-content {{ background: #ffffff; margin: 5% auto; padding: 0; border-radius: 12px; max-width: 480px; box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.1), 0 10px 10px -5px rgba(0, 0, 0, 0.04); animation: slideUp 0.3s ease; }}
        @keyframes slideUp {{ from {{ transform: translateY(20px); opacity: 0; }} to {{ transform: translateY(0); opacity: 1; }} }}
        .modal-content.modal-large {{ max-width: 700px; max-height: 85vh; overflow-y: auto; }}
        .modal-content.modal-large::-webkit-scrollbar {{ width: 8px; }}
        .modal-content.modal-large::-webkit-scrollbar-track {{ background: #f1f5f9; }}
        .modal-content.modal-large::-webkit-scrollbar-thumb {{ background: #cbd5e1; border-radius: 4px; }}
        .modal-content.modal-large::-webkit-scrollbar-thumb:hover {{ background: #94a3b8; }}
        .modal-header {{ padding: 1.5rem 2rem; border-bottom: 1px solid #e2e8f0; }}
        .modal-header h3 {{ margin: 0; color: #0f172a; font-size: 1.25rem; font-weight: 700; }}
        .modal form {{ padding: 2rem; }}
        .modal-actions {{ display: flex; gap: 0.75rem; justify-content: flex-end; margin-top: 1.5rem; padding-top: 1.5rem; border-top: 1px solid #e2e8f0; }}

        /* Footer */
        .app-footer {{ margin-top: 4rem; padding: 2rem 0; text-align: center; font-size: 0.875rem; color: #94a3b8; }}

        /* Responsive design */
        @media (max-width: 768px) {{
            main {{ padding: 1rem; }}
            .header-bar {{ flex-direction: column; align-items: flex-start; }}
            .dual-inputs {{ grid-template-columns: 1fr; }}
            .usage-grid {{ grid-template-columns: 1fr; }}
            .modal-content {{ margin: 10% 1rem; max-width: calc(100% - 2rem); }}
            .admin {{ padding: 1rem; }}
            th, td {{ padding: 0.75rem 0.5rem; font-size: 0.875rem; }}
        }}
    </style>
</head>
<body>
    <header>
        <div class="header-bar">
            <h1>使用情况仪表盘</h1>
            <a class="back-link" href="/">← 返回首页</a>
        </div>
        <p>管理账号额度，并进入各模块的配置页面。</p>
    </header>
    <main>
        <p data-user-id="{auth_id}">当前登录：<strong>{username}</strong>。</p>
        {message_block}
        <div class="table-wrapper">
            <table>
                <thead>
                    <tr><th>用户名</th><th>额度组</th><th>角色</th><th>近 7 日使用（摘要）</th><th>操作</th></tr>
                </thead>
                <tbody>
                    {table_rows}
                </tbody>
            </table>
        </div>
        {user_controls}
        <section class="admin collapsible-section">
            <h2 class="section-header" onclick="toggleSection('group-management')">
                <span class="toggle-icon" id="icon-group-management">▶</span> 管理额度组
            </h2>
            <div class="section-content collapsed" id="content-group-management">
                {group_sections}
                {new_group}
            </div>
        </section>
        <div id="password-modal" class="modal">
            <div class="modal-content">
                <div class="modal-header">
                    <h3>重置密码</h3>
                </div>
                <form id="password-reset-form" method="post" action="/dashboard/users/password">
                    <input type="hidden" name="username" value="">
                    <p>为用户 <strong id="reset-username-display"></strong> 设置新密码：</p>
                    <div class="field">
                        <label for="modal-password-input">新密码</label>
                        <input id="modal-password-input" type="password" name="password" required>
                    </div>
                    <div class="modal-actions">
                        <button type="button" class="btn-sm" onclick="closeModal()">取消</button>
                        <button type="submit" class="btn-primary">确认重置</button>
                    </div>
                </form>
            </div>
        </div>
        {footer}
    </main>
    <script>
        function toggleUserDetails(userId) {{
            const detailRow = document.getElementById('detail-' + userId);
            const userRow = document.querySelector('tr.user-row[data-user-id="' + userId + '"]');

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
                if (icon) {{
                    icon.textContent = '▼';
                }}
            }} else {{
                content.classList.add('collapsed');
                if (icon) {{
                    icon.textContent = '▶';
                }}
            }}
        }}

        function resetPassword(buttonElement) {{
            const username = buttonElement.getAttribute('data-username');
            const modal = document.getElementById('password-modal');
            const usernameSpan = document.getElementById('reset-username-display');
            const passwordInput = document.getElementById('modal-password-input');
            const form = document.getElementById('password-reset-form');

            usernameSpan.textContent = username;
            form.querySelector('input[name="username"]').value = username;
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
    token_limit: Option<i64>,
    unit_limits: HashMap<String, Option<i64>>,
}

#[derive(sqlx::FromRow)]
struct UsageGroupRow {
    id: Uuid,
    name: String,
    description: Option<String>,
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
            let token_limit = limit_map
                .get(&group.id)
                .and_then(|limits| limits.token_limit);

            let unit_limits = limit_map
                .get(&group.id)
                .map(|limits| {
                    limits
                        .module_limits
                        .iter()
                        .map(|(module, snapshot)| (module.clone(), snapshot.unit_limit))
                        .collect::<HashMap<_, _>>()
                })
                .unwrap_or_default();

            UsageGroupDisplay {
                id: group.id,
                name: group.name,
                description: group.description,
                token_limit,
                unit_limits,
            }
        })
        .collect();

    Ok(displays)
}
