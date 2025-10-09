use axum::Json;
use axum::http::StatusCode;
use serde::Serialize;
use uuid::Uuid;

/// Canonical JSON payload for error responses.
#[derive(Debug, Serialize, Clone)]
pub struct ApiMessage {
    pub message: String,
}

impl ApiMessage {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Standard job submission response used by async modules.
#[derive(Debug, Serialize, Clone)]
pub struct JobSubmission {
    pub job_id: Uuid,
    pub status_url: String,
}

impl JobSubmission {
    pub fn new(job_id: Uuid, status_url: impl Into<String>) -> Self {
        Self {
            job_id,
            status_url: status_url.into(),
        }
    }
}

/// Helper for controllers that need to return `(StatusCode, Json<ApiMessage>)`.
pub fn json_error(
    status: StatusCode,
    message: impl Into<String>,
) -> (StatusCode, Json<ApiMessage>) {
    (status, Json(ApiMessage::new(message)))
}
