use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    GlossaryTermRow, JournalReferenceRow, JournalTopicRow, JournalTopicScoreRow, escape_html,
};

pub const MODULE_ADMIN_SHARED_STYLES: &str = r#"
        section.admin {
            margin-bottom: 2rem;
            padding: 1.5rem;
            border-radius: 12px;
            background: #ffffff;
            border: 1px solid #e2e8f0;
            box-shadow: 0 18px 40px rgba(15, 23, 42, 0.08);
        }
        section.admin h2 {
            margin-top: 0;
            color: #1d4ed8;
        }
        section.admin h3 {
            margin-top: 0;
            color: #0f172a;
        }
        .section-note {
            color: #475569;
            font-size: 0.95rem;
            margin-bottom: 1rem;
        }
        .stack {
            display: flex;
            flex-direction: column;
            gap: 1.5rem;
        }
        .glossary-forms {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
            gap: 1.5rem;
        }
        .glossary-forms form {
            background: #f8fafc;
            border: 1px solid #e2e8f0;
            border-radius: 10px;
            padding: 1.25rem;
        }
        .glossary-forms h3 {
            margin-bottom: 0.75rem;
        }
        .field {
            margin-bottom: 1rem;
        }
        .field label {
            display: block;
            margin-bottom: 0.4rem;
            font-weight: 600;
            color: #0f172a;
        }
        .field input,
        .field select,
        .field textarea {
            width: 100%;
            padding: 0.75rem;
            border-radius: 8px;
            border: 1px solid #cbd5f5;
            background: #f8fafc;
            color: #0f172a;
            box-sizing: border-box;
        }
        .field input:focus,
        .field select:focus,
        .field textarea:focus {
            outline: none;
            border-color: #2563eb;
            box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.12);
        }
        .glossary table {
            width: 100%;
            border-collapse: collapse;
            background: #ffffff;
            border-radius: 12px;
            overflow: hidden;
        }
        .glossary thead {
            background: #f1f5f9;
        }
        .glossary th,
        .glossary td {
            padding: 0.75rem 1rem;
            border-bottom: 1px solid #e2e8f0;
            text-align: left;
        }
        .modal {
            display: none;
            position: fixed;
            z-index: 1000;
            padding-top: 100px;
            left: 0;
            top: 0;
            width: 100%;
            height: 100%;
            overflow: auto;
            background-color: rgba(15, 23, 42, 0.45);
        }
        .modal-content {
            background-color: #fefefe;
            margin: auto;
            padding: 20px;
            border: 1px solid #e2e8f0;
            width: 480px;
            border-radius: 12px;
            box-shadow: 0 18px 40px rgba(15, 23, 42, 0.2);
        }
        .modal-content.modal-large {
            width: min(820px, 90%);
        }
        .modal-header {
            display: flex;
            align-items: center;
            justify-content: space-between;
            margin-bottom: 1rem;
        }
        .modal-actions {
            margin-top: 1.5rem;
            display: flex;
            justify-content: flex-end;
            gap: 0.75rem;
        }
        .btn-sm {
            padding: 0.5rem 0.85rem;
            border-radius: 8px;
            border: none;
            background: #2563eb;
            color: #ffffff;
            cursor: pointer;
            font-weight: 600;
        }
        .btn-sm.btn-warning {
            background: #f97316;
        }
        .btn-sm.btn-warning:hover {
            background: #ea580c;
        }
        .btn-primary {
            padding: 0.75rem 1.25rem;
            border-radius: 8px;
            background: #2563eb;
            color: #ffffff;
            border: none;
            font-weight: 600;
            cursor: pointer;
            transition: background 0.15s ease;
        }
        .btn-primary:hover {
            background: #1d4ed8;
        }
        button.danger {
            background: #ef4444;
            border: none;
            padding: 0.45rem 0.85rem;
            border-radius: 8px;
            color: #ffffff;
            cursor: pointer;
        }
        button.danger:hover {
            background: #dc2626;
        }
        table.glossary tbody tr:nth-child(even) {
            background: #f8fafc;
        }
        .topic-picker.activemed {
            border-color: #2563eb;
        }
"#;

pub fn render_glossary_section(terms: &[GlossaryTermRow], redirect: &str) -> String {
    let mut rows = String::new();
    let mut select_options = String::new();

    if terms.is_empty() {
        rows.push_str(r#"<tr><td colspan="4">尚未添加术语。</td></tr>"#);
    } else {
        for term in terms {
            rows.push_str(&format!(
                r#"<tr>
    <td>{source}</td>
    <td>{target}</td>
    <td>{notes}</td>
    <td>
        <form method="post" action="/dashboard/glossary/delete" onsubmit="return confirm('确认删除该术语吗？');">
            <input type="hidden" name="id" value="{id}">
            <input type="hidden" name="redirect" value="{redirect}">
            <button type="submit" class="danger">删除</button>
        </form>
    </td>
</tr>"#,
                source = escape_html(&term.source_term),
                target = escape_html(&term.target_term),
                notes = term
                    .notes
                    .as_ref()
                    .map(|n| escape_html(n))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "—".to_string()),
                id = term.id,
                redirect = redirect,
            ));

            select_options.push_str(&format!(
                r#"<option value="{id}">{label}</option>"#,
                id = term.id,
                label = escape_html(&term.source_term)
            ));
        }
    }

    let disabled_attr = if terms.is_empty() { " disabled" } else { "" };

    format!(
        r##"<section class="admin">
    <h2>术语表管理</h2>
    <p class="section-note">该术语表同时用于摘要与 DOCX 翻译模块。</p>
    <div class="stack">
        <table class="glossary">
            <thead>
                <tr><th>英文</th><th>中文</th><th>备注</th><th>操作</th></tr>
            </thead>
            <tbody>
                {rows}
            </tbody>
        </table>
        <div class="glossary-forms">
            <form method="post" action="/dashboard/glossary">
                <h3>新增术语</h3>
                <input type="hidden" name="redirect" value="{redirect}">
                <div class="field">
                    <label for="glossary-source">英文术语</label>
                    <input id="glossary-source" name="source_term" required>
                </div>
                <div class="field">\n                    <label for="glossary-target">中文术语</label>
                    <input id="glossary-target" name="target_term" required>
                </div>
                <div class="field">
                    <label for="glossary-notes">备注（可选）</label>
                    <input id="glossary-notes" name="notes" placeholder="填写上下文或使用说明">
                </div>
                <button type="submit">保存术语</button>
            </form>
            <form method="post" action="/dashboard/glossary/update">
                <h3>更新术语</h3>
                <input type="hidden" name="redirect" value="{redirect}">
                <div class="field">
                    <label for="glossary-update-id">选择术语</label>
                    <select id="glossary-update-id" name="id" required{disabled_attr}>
                        {select_options}
                    </select>
                </div>
                <div class="field">
                    <label for="glossary-update-source">更新后的英文</label>
                    <input id="glossary-update-source" name="source_term" required{disabled_attr}>
                </div>
                <div class="field">
                    <label for="glossary-update-target">更新后的中文</label>
                    <input id="glossary-update-target" name="target_term" required{disabled_attr}>
                </div>
                <div class="field">
                    <label for="glossary-update-notes">备注（可选）</label>
                    <input id="glossary-update-notes" name="notes" placeholder="填写上下文或使用说明"{disabled_attr}>
                </div>
                <button type="submit"{disabled_attr}>保存修改</button>
            </form>
        </div>
    </div>
</section>"##,
        rows = rows,
        select_options = select_options,
        disabled_attr = disabled_attr,
        redirect = redirect,
    )
}

pub fn render_topic_section(topics: &[JournalTopicRow], redirect: &str) -> String {
    let mut rows = String::new();

    if topics.is_empty() {
        rows.push_str(r#"<tr><td colspan="4">尚未添加主题。</td></tr>"#);
    } else {
        for topic in topics {
            let description = topic
                .description
                .as_ref()
                .map(|d| escape_html(d))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "—".to_string());
            let created = topic.created_at.format("%Y-%m-%d");
            rows.push_str(&format!(
                r#"<tr><td>{name}</td><td>{description}</td><td>{created}</td><td>
    <form method="post" action="/dashboard/journal-topics/delete" onsubmit="return confirm('确定删除该主题？');">
        <input type="hidden" name="id" value="{id}">
        <input type="hidden" name="redirect" value="{redirect}">
        <button type="submit" class="danger">删除</button>
    </form>
</td></tr>"#,
                name = escape_html(&topic.name),
                description = description,
                created = created,
                id = topic.id,
                redirect = redirect,
            ));
        }
    }

    format!(
        r##"<section class="admin">
    <h2>主题管理</h2>
    <p class="section-note">主题用于稿件关键词识别，可重复提交同名主题覆盖描述。</p>
    <table>
        <thead>
            <tr><th>主题名称</th><th>描述</th><th>创建时间</th><th>操作</th></tr>
        </thead>
        <tbody>
            {rows}
        </tbody>
    </table>
    <form method="post" action="/dashboard/journal-topics">
        <input type="hidden" name="redirect" value="{redirect}">
        <h3>新增或更新主题</h3>
        <div class="field">
            <label for="topic-name">主题名称</label>
            <input id="topic-name" name="name" required>
        </div>
        <div class="field">
            <label for="topic-description">描述（可选）</label>
            <input id="topic-description" name="description" placeholder="例如：用于描述简要范围">
        </div>
        <button type="submit">保存主题</button>
    </form>
</section>"##,
        rows = rows,
        redirect = redirect,
    )
}

pub fn render_journal_section(
    references: &[JournalReferenceRow],
    topics: &[JournalTopicRow],
    scores: &[JournalTopicScoreRow],
    redirect: &str,
) -> String {
    let mut name_lookup: HashMap<Uuid, String> = HashMap::new();
    for topic in topics {
        name_lookup.insert(topic.id, topic.name.clone());
    }

    let valid_ids: HashSet<Uuid> = references.iter().map(|r| r.id).collect();
    let mut scores_map: HashMap<Uuid, Vec<(Uuid, String, i16)>> = HashMap::new();
    for score in scores {
        if !valid_ids.contains(&score.journal_id) {
            continue;
        }
        if let Some(name) = name_lookup.get(&score.topic_id) {
            scores_map.entry(score.journal_id).or_default().push((
                score.topic_id,
                name.clone(),
                score.score,
            ));
        }
    }

    for values in scores_map.values_mut() {
        values.sort_by(|a, b| a.1.cmp(&b.1));
    }

    let mut rows = String::new();
    if references.is_empty() {
        rows.push_str(r#"<tr><td colspan="6">尚未添加期刊参考。</td></tr>"#);
    } else {
        for reference in references {
            let mark = reference
                .reference_mark
                .as_ref()
                .map(|m| escape_html(m))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "—".to_string());
            let notes = reference
                .notes
                .as_ref()
                .map(|n| escape_html(n))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "—".to_string());
            let score_display = scores_map
                .get(&reference.id)
                .map(|entries| {
                    if entries.is_empty() {
                        "—".to_string()
                    } else {
                        entries
                            .iter()
                            .map(|(_, name, score)| format!("{}：{}", escape_html(name), score))
                            .collect::<Vec<_>>()
                            .join("<br>")
                    }
                })
                .unwrap_or_else(|| "—".to_string());

            let mut score_payload = serde_json::Map::new();
            if let Some(entries) = scores_map.get(&reference.id) {
                for (topic_id, _name, score) in entries {
                    score_payload.insert(topic_id.to_string(), json!(score));
                }
            }

            let payload_value = json!({
                "journal_name": &reference.journal_name,
                "reference_mark": &reference.reference_mark,
                "low_bound": reference.low_bound,
                "notes": &reference.notes,
                "scores": Value::Object(score_payload),
            });
            let payload_attr = escape_html(&payload_value.to_string());

            rows.push_str(&format!(
                r#"<tr><td>{name}</td><td>{mark}</td><td>{low:.2}</td><td>{notes}</td><td>{scores}</td><td>
    <div class="action-stack">
        <button type="button" class="secondary" data-load-journal="{payload}">载入表单</button>
        <form method="post" action="/dashboard/journal-references/delete" onsubmit="return confirm('确定删除该期刊参考？');">
            <input type="hidden" name="id" value="{id}">
            <input type="hidden" name="redirect" value="{redirect}">
            <button type="submit" class="danger">删除</button>
        </form>
    </div>
</td></tr>"#,
                name = escape_html(&reference.journal_name),
                mark = mark,
                low = reference.low_bound,
                notes = notes,
                scores = score_display,
                id = reference.id,
                redirect = redirect,
                payload = payload_attr,
            ));
        }
    }

    let score_inputs = if topics.is_empty() {
        r#"<p class="section-note">暂无主题，请先添加主题后再录入分值。</p>"#.to_string()
    } else {
        let fields = topics
            .iter()
            .map(|topic| {
                let mut options = String::new();
                for value in 0..=2 {
                    options.push_str(&format!(
                        r#"<option value="{value}"{selected}>{value}</option>"#,
                        value = value,
                        selected = if value == 0 { " selected" } else { "" },
                    ));
                }
                format!(
                    r#"<div class="topic-picker" data-topic="{id}"><label for="score-{id}">{name}</label><select id="score-{id}" name="score_{id}" data-topic-select="{id}">{options}</select></div>"#,
                    id = topic.id,
                    name = escape_html(&topic.name),
                    options = options,
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(r#"<div class="topic-grid">{fields}</div>"#, fields = fields,)
    };

    let mut section_html = format!(
        r##"<section class="admin">
    <h2>期刊参考</h2>
    <p class="section-note">该列表支撑稿件评估模块的期刊推荐逻辑。</p>
    <table>
        <thead>
            <tr><th>期刊名称</th><th>参考标记</th><th>低区间阈值</th><th>备注</th><th>主题分值</th><th>操作</th></tr>
        </thead>
        <tbody>
            {rows}
        </tbody>
    </table>
    <form id="journal-form" method="post" action="/dashboard/journal-references">
        <input type="hidden" name="redirect" value="{redirect}">
        <h3>新增或更新期刊</h3>
        <div class="field">
            <label for="journal-name">期刊名称</label>
            <input id="journal-name" name="journal_name" required>
        </div>
        <div class="field">
            <label for="journal-mark">参考标记（可选）</label>
            <input id="journal-mark" name="reference_mark" placeholder="例如：Level 3 或 2/3">
        </div>
        <div class="field">
            <label for="journal-low">低区间阈值</label>
            <input id="journal-low" name="low_bound" required placeholder="例如：37.5">
        </div>
        <div class="field">
            <label for="journal-notes">备注（可选）</label>
            <input id="journal-notes" name="notes" placeholder="简要说明">
        </div>
        {score_inputs}
        <div class="journal-form-actions">
            <button type="submit">保存期刊</button>
            <button type="button" class="secondary" data-clear-journal-form>清空表单</button>
        </div>
    </form>
</section>"##,
        rows = rows,
        score_inputs = score_inputs,
        redirect = redirect,
    );

    let script = r#"
<script>
document.addEventListener('DOMContentLoaded', function () {
    const form = document.getElementById('journal-form');
    if (!form) { return; }
    const selects = Array.from(form.querySelectorAll('[data-topic-select]'));

    function updateHighlight(select) {
        const wrapper = select.closest('.topic-picker');
        if (!wrapper) { return; }
        const value = parseInt(select.value || '0', 10);
        if (!Number.isNaN(value) && value > 0) {
            wrapper.classList.add('active');
        } else {
            wrapper.classList.remove('active');
        }
    }

    selects.forEach((select) => {
        updateHighlight(select);
        select.addEventListener('change', () => updateHighlight(select));
    });

    document.querySelectorAll('[data-load-journal]').forEach((button) => {
        button.addEventListener('click', () => {
            const payloadRaw = button.getAttribute('data-load-journal');
            if (!payloadRaw) { return; }

            let data;
            try {
                data = JSON.parse(payloadRaw);
            } catch (error) {
                console.error('Failed to parse journal payload', error);
                return;
            }

            const nameInput = form.querySelector('#journal-name');
            const markInput = form.querySelector('#journal-mark');
            const lowInput = form.querySelector('#journal-low');
            const notesInput = form.querySelector('#journal-notes');

            if (nameInput) { nameInput.value = data.journal_name ? String(data.journal_name) : ''; }
            if (markInput) { markInput.value = data.reference_mark ? String(data.reference_mark) : ''; }
            if (lowInput) {
                if (data.low_bound === null || data.low_bound === undefined || data.low_bound === '') {
                    lowInput.value = '';
                } else {
                    lowInput.value = String(data.low_bound);
                }
            }
            if (notesInput) { notesInput.value = data.notes ? String(data.notes) : ''; }

            const scoreMap = (data.scores && typeof data.scores === 'object') ? data.scores : {};
            selects.forEach((select) => {
                const topicId = select.getAttribute('data-topic-select');
                const rawValue = topicId && Object.prototype.hasOwnProperty.call(scoreMap, topicId)
                    ? scoreMap[topicId]
                    : 0;
                select.value = String(rawValue ?? 0);
                select.dispatchEvent(new Event('change'));
            });

            if (nameInput) { nameInput.focus(); }
        });
    });

    const resetButton = form.querySelector('[data-clear-journal-form]');
    if (resetButton) {
        resetButton.addEventListener('click', () => {
            form.reset();
            selects.forEach((select) => {
                select.value = '0';
                select.dispatchEvent(new Event('change'));
            });
        });
    }
});
</script>
"#;

    section_html.push_str(script);
    section_html
}
