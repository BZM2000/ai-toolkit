#![allow(dead_code)]

use crate::web::templates::escape_html;

/// Shared CSS snippet for the Zhang Group upload widget.
pub const UPLOAD_WIDGET_STYLES: &str = r#"
.zg-upload-widget { display: flex; flex-direction: column; gap: 0.75rem; }
.zg-upload-widget__label { font-weight: 600; color: #0f172a; }
.zg-upload-widget__description { color: #475569; font-size: 0.95rem; margin: 0; }
.zg-upload-dropzone { border: 2px dashed #cbd5f5; border-radius: 12px; padding: 2rem; text-align: center; background: #f8fafc; color: #475569; transition: border-color 0.2s ease, background 0.2s ease; cursor: pointer; }
.zg-upload-dropzone strong { color: #1d4ed8; }
.zg-upload-dropzone[data-state="dragover"] { border-color: #2563eb; background: #e0f2fe; }
.zg-upload-note { color: #475569; font-size: 0.9rem; margin: 0.5rem 0 0; }
.zg-upload-browse { color: #2563eb; text-decoration: underline; cursor: pointer; }
.zg-upload-input { display: none; }
.zg-upload-status { min-height: 1.5rem; font-size: 0.95rem; color: #2563eb; }
.zg-upload-list { display: flex; flex-direction: column; gap: 0.5rem; }
.zg-upload-item { display: flex; justify-content: space-between; align-items: center; gap: 0.5rem; padding: 0.5rem 0.75rem; border: 1px solid #e2e8f0; border-radius: 8px; background: #ffffff; color: #0f172a; }
.zg-upload-name { flex: 1; min-width: 0; word-break: break-all; }
.zg-upload-remove { background: #dc2626; color: #ffffff; border: none; padding: 0.35rem 0.75rem; border-radius: 6px; font-size: 0.85rem; cursor: pointer; }
.zg-upload-remove:hover { background: #b91c1c; }
@media (max-width: 768px) {
    .zg-upload-dropzone { padding: 1.5rem 1rem; }
    .zg-upload-item { flex-direction: column; align-items: flex-start; }
    .zg-upload-remove { align-self: flex-end; }
}
"#;

/// Shared JavaScript bundle (vanilla) for the upload widget.
pub const UPLOAD_WIDGET_SCRIPT: &str = r#"<script>
(function() {
    function initWidget(widget) {
        if (widget.dataset.initialized === 'true') {
            return;
        }
        widget.dataset.initialized = 'true';

        const input = widget.querySelector('input[type="file"]');
        const dropzone = widget.querySelector('[data-dropzone]');
        const statusBox = widget.querySelector('[data-upload-status]');
        const listEl = widget.querySelector('[data-upload-list]');
        const browseEl = widget.querySelector('[data-upload-browse]');
        const multiple = widget.dataset.multiple === 'true';
        const maxFiles = parseInt(widget.dataset.maxFiles || '0', 10);

        if (!input || !dropzone) {
            return;
        }

        function setFiles(files) {
            const dt = new DataTransfer();
            files.forEach(file => dt.items.add(file));
            input.files = dt.files;
            renderList();
        }

        function removeAt(index) {
            const current = Array.from(input.files);
            current.splice(index, 1);
            setFiles(current);
        }

        function handleFiles(incoming) {
            const selected = Array.from(input.files);
            for (const file of incoming) {
                if (maxFiles > 0 && selected.length >= maxFiles) {
                    break;
                }
                selected.push(file);
            }
            setFiles(selected);
        }

        function renderList() {
            if (!listEl) {
                return;
            }

            const files = Array.from(input.files);
            if (files.length === 0) {
                listEl.innerHTML = '';
                if (statusBox) {
                    statusBox.textContent = '';
                }
                return;
            }

            if (statusBox) {
                if (maxFiles > 0) {
                    statusBox.textContent = `已选择 ${files.length} 个文件，最多 ${maxFiles} 个。`;
                } else {
                    statusBox.textContent = `已选择 ${files.length} 个文件。`;
                }
            }

            listEl.innerHTML = files.map((file, index) => {
                if (!multiple) {
                    return `<div class="zg-upload-item"><span class="zg-upload-name">${escapeHtml(file.name)}</span></div>`;
                }
                return `<div class="zg-upload-item"><span class="zg-upload-name">${escapeHtml(file.name)}</span><button type="button" class="zg-upload-remove" data-index="${index}">移除</button></div>`;
            }).join('');

            if (multiple) {
                listEl.querySelectorAll('.zg-upload-remove').forEach(btn => {
                    btn.addEventListener('click', (event) => {
                        const idx = Number(event.currentTarget.dataset.index);
                        removeAt(idx);
                    });
                });
            }
        }

        function escapeHtml(value) {
            return value
                .replace(/&/g, '&amp;')
                .replace(/</g, '&lt;')
                .replace(/>/g, '&gt;')
                .replace(/"/g, '&quot;')
                .replace(/'/g, '&#39;');
        }

        input.addEventListener('change', () => {
            if (!multiple && input.files.length > 1) {
                setFiles([input.files[0]]);
                return;
            }
            if (maxFiles > 0 && input.files.length > maxFiles) {
                setFiles(Array.from(input.files).slice(0, maxFiles));
                return;
            }
            renderList();
        });

        const activateDrag = () => dropzone.dataset.state = 'dragover';
        const deactivateDrag = () => delete dropzone.dataset.state;

        dropzone.addEventListener('click', () => input.click());
        dropzone.addEventListener('dragenter', (event) => {
            event.preventDefault();
            activateDrag();
        });
        dropzone.addEventListener('dragover', (event) => {
            event.preventDefault();
        });
        dropzone.addEventListener('dragleave', (event) => {
            event.preventDefault();
            if (!dropzone.contains(event.relatedTarget)) {
                deactivateDrag();
            }
        });
        dropzone.addEventListener('drop', (event) => {
            event.preventDefault();
            deactivateDrag();
            handleFiles(event.dataTransfer.files);
        });

        if (browseEl) {
            browseEl.addEventListener('click', (event) => {
                event.preventDefault();
                input.click();
            });
        }

        renderList();
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', () => {
            document.querySelectorAll('.zg-upload-widget').forEach(initWidget);
        });
    } else {
        document.querySelectorAll('.zg-upload-widget').forEach(initWidget);
    }
})();
</script>"#;

/// Declarative configuration for rendering the upload widget snippet.
#[derive(Debug, Clone)]
pub struct UploadWidgetConfig<'a> {
    pub widget_id: &'a str,
    pub input_id: &'a str,
    pub field_name: &'a str,
    pub label: &'a str,
    pub description: Option<&'a str>,
    pub note: Option<&'a str>,
    pub accept: Option<&'a str>,
    pub multiple: bool,
    pub max_files: Option<usize>,
}

impl<'a> UploadWidgetConfig<'a> {
    pub fn new(widget_id: &'a str, input_id: &'a str, field_name: &'a str, label: &'a str) -> Self {
        Self {
            widget_id,
            input_id,
            field_name,
            label,
            description: None,
            note: None,
            accept: None,
            multiple: false,
            max_files: None,
        }
    }

    pub fn with_multiple(mut self, max_files: Option<usize>) -> Self {
        self.multiple = true;
        self.max_files = max_files;
        self
    }

    pub fn with_description(mut self, text: &'a str) -> Self {
        self.description = Some(text);
        self
    }

    pub fn with_note(mut self, text: &'a str) -> Self {
        self.note = Some(text);
        self
    }

    pub fn with_accept(mut self, accept: &'a str) -> Self {
        self.accept = Some(accept);
        self
    }
}

pub fn render_upload_widget(config: &UploadWidgetConfig<'_>) -> String {
    let description = config
        .description
        .map(|text| {
            format!(
                "<p class=\"zg-upload-widget__description\">{}</p>",
                escape_html(text)
            )
        })
        .unwrap_or_default();

    let note = config
        .note
        .map(|text| format!("<p class=\"zg-upload-note\">{}</p>", escape_html(text)))
        .unwrap_or_default();

    let accept_attr = config
        .accept
        .map(|value| format!(" accept=\"{}\"", escape_html(value)))
        .unwrap_or_default();

    let multiple_attr = if config.multiple { " multiple" } else { "" };
    let max_files_attr = config
        .max_files
        .map(|count| count.to_string())
        .unwrap_or_else(|| "".to_string());

    let browse_label = if config.multiple {
        "点击选择多个文件"
    } else {
        "点击选择文件"
    };

    format!(
        r#"<div class="zg-upload-widget" id="{id}" data-multiple="{multiple}" data-max-files="{max_files}">
    <label class="zg-upload-widget__label" for="{input_id}">{label}</label>
    {description}
    <div class="zg-upload-dropzone" data-dropzone>
        <p><strong>拖拽文件</strong>到此处，或<span class="zg-upload-browse" data-upload-browse>{browse}</span></p>
        {note}
        <input class="zg-upload-input" id="{input_id}" name="{field_name}" type="file"{multiple_attr}{accept_attr}>
    </div>
    <div class="zg-upload-status" data-upload-status></div>
    <div class="zg-upload-list" data-upload-list></div>
</div>"#,
        id = escape_html(config.widget_id),
        multiple = if config.multiple { "true" } else { "false" },
        max_files = escape_html(&max_files_attr),
        input_id = escape_html(config.input_id),
        label = escape_html(config.label),
        description = description,
        browse = browse_label,
        note = note,
        field_name = escape_html(config.field_name),
        multiple_attr = multiple_attr,
        accept_attr = accept_attr,
    )
}
