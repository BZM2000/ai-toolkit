pub mod admin;
pub mod admin_utils;
pub mod auth;
pub mod data;
pub mod history;
pub mod history_ui;
pub mod landing;
pub mod models;
pub mod responses;
pub mod router;
pub mod state;
pub mod templates;
pub mod upload_ui;
pub mod uploads;

pub use auth::{AuthUser, SESSION_COOKIE, SESSION_TTL_DAYS};
pub use data::{
    fetch_glossary_terms, fetch_journal_references, fetch_journal_topic_scores,
    fetch_journal_topics,
};
pub use models::{GlossaryTermRow, JournalReferenceRow, JournalTopicRow, JournalTopicScoreRow};
pub use responses::{ApiMessage, JobSubmission, json_error};
pub use state::AppState;
pub use templates::{
    ToolAdminLink, ToolPageLayout, escape_html, render_footer, render_login_page, render_tool_page,
};
#[allow(unused_imports)]
pub use upload_ui::{
    UPLOAD_WIDGET_SCRIPT, UPLOAD_WIDGET_STYLES, UploadWidgetConfig, render_upload_widget,
};
#[allow(unused_imports)]
pub use uploads::{
    FileFieldConfig, FileNaming, SavedFile, UploadError, UploadOutcome, UploadResult,
    ensure_directory as ensure_upload_directory, process_upload_form,
};
