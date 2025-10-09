pub const HISTORY_STYLES: &str = include_str!("history_styles.css");
pub const HISTORY_SCRIPT: &str = include_str!("history_client.js");

pub fn render_history_panel(module_key: &str) -> String {
    format!(
        r#"<section class="panel history-panel" data-history-module="{module}" data-history-limit="20">
    <h2>历史记录</h2>
    <p class="note">展示最近 24 小时提交的任务，可在后台完成后直接下载结果。</p>
    <div class="history-table-wrapper">
        <table class="history-table">
            <thead>
                <tr>
                    <th>任务</th>
                    <th>状态</th>
                    <th>最近更新</th>
                    <th>操作</th>
                </tr>
            </thead>
            <tbody data-history-body>
                <tr class="history-empty-row"><td colspan="4">正在载入...</td></tr>
            </tbody>
        </table>
    </div>
</section>"#,
        module = html_escape(module_key),
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
