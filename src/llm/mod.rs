use std::{env, fmt, fs, path::Path};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{
    Client,
    multipart::{Form, Part},
};
use serde::Deserialize;

/// Enumerates the supported LLM backends behind the shared utility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LlmProvider {
    OpenRouter,
    Poe,
}

impl fmt::Display for LlmProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmProvider::OpenRouter => write!(f, "openrouter"),
            LlmProvider::Poe => write!(f, "poe"),
        }
    }
}

/// Defines the shape of a chat-style interaction with an LLM.
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub attachments: Vec<FileAttachment>,
}

impl LlmRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            attachments: Vec::new(),
        }
    }

    pub fn with_attachments(mut self, attachments: Vec<FileAttachment>) -> Self {
        self.attachments = attachments;
        self
    }
}

/// Individual chat message, compatible with OpenAI compliant providers.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub text: String,
}

impl ChatMessage {
    pub fn new(role: MessageRole, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
        }
    }
}

/// Supported chat roles passed to providers.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    fn as_str(&self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }
}

/// File attachment descriptor. Modules can construct one directly or through helpers.
#[derive(Debug, Clone)]
pub struct FileAttachment {
    pub filename: String,
    pub content_type: String,
    pub kind: AttachmentKind,
    pub bytes: Vec<u8>,
}

impl FileAttachment {
    pub fn new(
        filename: impl Into<String>,
        content_type: impl Into<String>,
        kind: AttachmentKind,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            filename: filename.into(),
            content_type: content_type.into(),
            kind,
            bytes,
        }
    }

    /// Load a file from disk into an attachment with the provided metadata.
    pub fn from_path(
        path: impl AsRef<Path>,
        content_type: impl Into<String>,
        kind: AttachmentKind,
    ) -> Result<Self> {
        let path = path.as_ref();
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("unable to derive filename from {:?}", path))?;
        let bytes =
            fs::read(path).with_context(|| format!("failed to read attachment from {:?}", path))?;

        Ok(Self::new(
            filename.to_string(),
            content_type.into(),
            kind,
            bytes,
        ))
    }
}

/// Types of attachments supported by the shared utility.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AttachmentKind {
    Image,
    Audio,
    Pdf,
}

impl AttachmentKind {
    fn as_str(&self) -> &'static str {
        match self {
            AttachmentKind::Image => "image",
            AttachmentKind::Audio => "audio",
            AttachmentKind::Pdf => "pdf",
        }
    }
}

/// Captures basic token usage metrics associated with a call.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokenUsage {
    pub prompt_tokens: usize,
    pub response_tokens: usize,
    pub total_tokens: usize,
}

/// Full response surface returned to callers.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub text: String,
    pub token_usage: TokenUsage,
    pub provider: LlmProvider,
    pub model: String,
    pub raw: serde_json::Value,
}

/// Main entry point for invoking providers.
#[derive(Clone)]
pub struct LlmClient {
    http: Client,
    config: LlmConfig,
}

#[derive(Clone, Default)]
struct LlmConfig {
    openrouter_api_key: Option<String>,
    poe_api_key: Option<String>,
    openrouter_referer: Option<String>,
    openrouter_title: Option<String>,
}

impl LlmClient {
    /// Build a client using environment variables.
    pub fn from_env() -> Result<Self> {
        let openrouter_api_key = env::var("OPENROUTER_API_KEY").ok();
        let poe_api_key = env::var("POE_API_KEY").ok();
        let openrouter_referer = env::var("OPENROUTER_HTTP_REFERER").ok();
        let openrouter_title = env::var("OPENROUTER_X_TITLE").ok();

        Ok(Self {
            http: Client::new(),
            config: LlmConfig {
                openrouter_api_key,
                poe_api_key,
                openrouter_referer,
                openrouter_title,
            },
        })
    }

    /// Execute a request against the provider encoded in the model name.
    pub async fn execute(&self, request: LlmRequest) -> Result<LlmResponse> {
        let model = request.model.clone();
        let (provider, provider_model) = parse_model_provider(&model)?;

        match provider {
            LlmProvider::OpenRouter => self.execute_openrouter(provider_model, request).await,
            LlmProvider::Poe => self.execute_poe(provider_model, request).await,
        }
    }

    async fn execute_openrouter(&self, model: &str, request: LlmRequest) -> Result<LlmResponse> {
        let Some(api_key) = self.config.openrouter_api_key.as_ref() else {
            bail!("OPENROUTER_API_KEY is not configured but required for OpenRouter requests");
        };

        let mut input = Vec::new();
        for msg in &request.messages {
            input.push(serde_json::json!({
                "role": msg.role.as_str(),
                "content": [
                    {
                        "type": "input_text",
                        "text": msg.text,
                    }
                ],
            }));
        }

        // Ensure there is a user message to hold attachment references.
        let mut attachment_target_idx = request
            .messages
            .iter()
            .rposition(|m| m.role == MessageRole::User);

        if attachment_target_idx.is_none() && !request.attachments.is_empty() {
            // create empty user entry to pin uploads
            input.push(serde_json::json!({
                "role": "user",
                "content": [],
            }));
            attachment_target_idx = Some(input.len() - 1);
        }

        for attachment in &request.attachments {
            if !matches!(
                attachment.kind,
                AttachmentKind::Image | AttachmentKind::Audio | AttachmentKind::Pdf
            ) {
                bail!(
                    "unsupported attachment kind for OpenRouter: {}",
                    attachment.kind.as_str()
                );
            }

            let file_id = self
                .upload_openrouter_file(api_key, attachment)
                .await
                .with_context(|| {
                    format!("failed to upload {} attachment", attachment.kind.as_str())
                })?;

            if let Some(idx) = attachment_target_idx {
                if let Some(entry) = input.get_mut(idx) {
                    if let Some(content) = entry.get_mut("content") {
                        if let Some(array) = content.as_array_mut() {
                            let attachment_entry = match attachment.kind {
                                AttachmentKind::Image => serde_json::json!({
                                    "type": "input_image",
                                    "image_id": file_id,
                                }),
                                AttachmentKind::Audio => serde_json::json!({
                                    "type": "input_audio",
                                    "audio_id": file_id,
                                }),
                                AttachmentKind::Pdf => serde_json::json!({
                                    "type": "file_path",
                                    "file_path": {"file_id": file_id},
                                }),
                            };
                            array.push(attachment_entry);
                        }
                    }
                }
            }
        }

        let prompt_tokens = approximate_token_count(
            &request
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        );

        let payload = serde_json::json!({
            "model": model,
            "input": input,
        });

        let mut req_builder = self
            .http
            .post("https://openrouter.ai/api/v1/responses")
            .bearer_auth(api_key)
            .json(&payload);

        if let Some(referer) = &self.config.openrouter_referer {
            req_builder = req_builder.header("HTTP-Referer", referer);
        }

        if let Some(title) = &self.config.openrouter_title {
            req_builder = req_builder.header("X-Title", title);
        }

        let response = req_builder.send().await?;
        let status = response.status();
        let response_text = response.text().await.context("failed to read response body")?;
        let body: serde_json::Value = serde_json::from_str(&response_text)
            .with_context(|| {
                let preview = if response_text.len() > 500 {
                    format!("{}...", &response_text[..500])
                } else {
                    response_text.clone()
                };
                format!("failed to parse OpenRouter response as JSON. Response body: {}", preview)
            })?;
        if !status.is_success() {
            bail!("openrouter call failed with status {}: {}", status, body);
        }

        let (text, usage) = extract_text_and_usage(&body)
            .ok_or_else(|| anyhow!("unexpected OpenRouter response payload: {}", body))?;

        let mut token_usage = usage.unwrap_or_else(|| TokenUsage {
            prompt_tokens,
            response_tokens: approximate_token_count(&text),
            total_tokens: prompt_tokens + approximate_token_count(&text),
        });
        if token_usage.prompt_tokens == 0 {
            token_usage.prompt_tokens = prompt_tokens;
        }
        if token_usage.response_tokens == 0 {
            token_usage.response_tokens = approximate_token_count(&text);
        }
        token_usage.total_tokens = token_usage.prompt_tokens + token_usage.response_tokens;

        Ok(LlmResponse {
            text,
            token_usage,
            provider: LlmProvider::OpenRouter,
            model: model.to_string(),
            raw: body,
        })
    }

    async fn execute_poe(&self, model: &str, request: LlmRequest) -> Result<LlmResponse> {
        if !request.attachments.is_empty() {
            bail!("attachments are not currently supported for Poe API calls");
        }

        let Some(api_key) = self.config.poe_api_key.as_ref() else {
            bail!("POE_API_KEY is not configured but required for Poe requests");
        };

        let messages: Vec<_> = request
            .messages
            .iter()
            .map(|msg| {
                serde_json::json!({
                    "role": msg.role.as_str(),
                    "content": msg.text,
                })
            })
            .collect();

        let payload = serde_json::json!({
            "model": model,
            "messages": messages,
        });

        let response = self
            .http
            .post("https://api.poe.com/v1/chat/completions")
            .bearer_auth(api_key)
            .json(&payload)
            .send()
            .await?;

        let status = response.status();
        let response_text = response.text().await.context("failed to read response body")?;
        let body: serde_json::Value = serde_json::from_str(&response_text)
            .with_context(|| {
                let preview = if response_text.len() > 500 {
                    format!("{}...", &response_text[..500])
                } else {
                    response_text.clone()
                };
                format!("failed to parse Poe response as JSON. Response body: {}", preview)
            })?;
        if !status.is_success() {
            bail!("poe call failed with status {}: {}", status, body);
        }

        let (text, usage) = extract_text_and_usage(&body)
            .ok_or_else(|| anyhow!("unexpected Poe response payload: {}", body))?;

        let prompt_tokens = approximate_token_count(
            &request
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let mut token_usage = usage.unwrap_or_else(|| TokenUsage {
            prompt_tokens,
            response_tokens: approximate_token_count(&text),
            total_tokens: prompt_tokens + approximate_token_count(&text),
        });
        if token_usage.prompt_tokens == 0 {
            token_usage.prompt_tokens = prompt_tokens;
        }
        if token_usage.response_tokens == 0 {
            token_usage.response_tokens = approximate_token_count(&text);
        }
        token_usage.total_tokens = token_usage.prompt_tokens + token_usage.response_tokens;

        Ok(LlmResponse {
            text,
            token_usage,
            provider: LlmProvider::Poe,
            model: model.to_string(),
            raw: body,
        })
    }

    async fn upload_openrouter_file(
        &self,
        api_key: &str,
        attachment: &FileAttachment,
    ) -> Result<String> {
        let part = Part::bytes(attachment.bytes.clone())
            .file_name(attachment.filename.clone())
            .mime_str(&attachment.content_type)?;

        let form = Form::new().text("purpose", "assistants").part("file", part);

        let response = self
            .http
            .post("https://openrouter.ai/api/v1/files")
            .bearer_auth(api_key)
            .multipart(form)
            .send()
            .await?;

        let status = response.status();
        let response_text = response.text().await.context("failed to read response body")?;
        let body: serde_json::Value = serde_json::from_str(&response_text)
            .with_context(|| {
                let preview = if response_text.len() > 500 {
                    format!("{}...", &response_text[..500])
                } else {
                    response_text.clone()
                };
                format!("failed to parse OpenRouter file upload response as JSON. Response body: {}", preview)
            })?;
        if !status.is_success() {
            bail!(
                "openrouter file upload failed with status {}: {}",
                status,
                body
            );
        }

        let upload: OpenRouterFileUploadResponse = serde_json::from_value(body.clone())
            .map_err(|_| anyhow!("unable to parse file upload response: {}", body))?;

        Ok(upload.id)
    }
}

/// Extract assistant text and optional usage metrics from either Responses or Chat Completions payloads.
fn extract_text_and_usage(value: &serde_json::Value) -> Option<(String, Option<TokenUsage>)> {
    if let Ok(resp) = serde_json::from_value::<OpenRouterResponsesPayload>(value.clone()) {
        let text = resp
            .output
            .into_iter()
            .filter(|item| item.item_type == "message")
            .flat_map(|item| item.content)
            .find_map(|content| match content.content_type.as_str() {
                "output_text" | "text" => Some(content.text.unwrap_or_default()),
                _ => None,
            })
            .unwrap_or_default();

        let usage = resp.usage.map(|usage| TokenUsage {
            prompt_tokens: usage.prompt_tokens.unwrap_or_default(),
            response_tokens: usage.completion_tokens.unwrap_or_default(),
            total_tokens: usage.total_tokens.unwrap_or_default(),
        });

        return Some((text, usage));
    }

    if let Ok(chat) = serde_json::from_value::<OpenAiChatCompletionPayload>(value.clone()) {
        let text = chat
            .choices
            .into_iter()
            .find_map(|choice| choice.message.content)
            .unwrap_or_default();

        let usage = chat.usage.map(|usage| TokenUsage {
            prompt_tokens: usage.prompt_tokens.unwrap_or_default(),
            response_tokens: usage.completion_tokens.unwrap_or_default(),
            total_tokens: usage.total_tokens.unwrap_or_default(),
        });

        return Some((text, usage));
    }

    None
}

fn parse_model_provider(model: &str) -> Result<(LlmProvider, &str)> {
    let (provider, name) = model.split_once('/').ok_or_else(|| {
        anyhow!("model must be prefixed with provider, e.g. 'openrouter/openai/gpt-4o'")
    })?;

    if name.trim().is_empty() {
        bail!("model name is required after provider prefix");
    }

    match provider {
        "openrouter" => Ok((LlmProvider::OpenRouter, name)),
        "poe" => Ok((LlmProvider::Poe, name)),
        other => bail!("unsupported provider prefix: {other}"),
    }
}

fn approximate_token_count(input: &str) -> usize {
    if input.trim().is_empty() {
        return 0;
    }
    input
        .split_whitespace()
        .filter(|segment| !segment.is_empty())
        .count()
}

#[derive(Debug, Deserialize)]
struct OpenRouterFileUploadResponse {
    id: String,
}

#[derive(Debug, Deserialize)]
struct OpenRouterResponsesPayload {
    #[serde(default)]
    output: Vec<OpenRouterOutputItem>,
    #[serde(default)]
    usage: Option<OpenRouterUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterOutputItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    content: Vec<OpenRouterOutputContent>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterOutputContent {
    #[serde(rename = "type")]
    content_type: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterUsage {
    #[serde(default)]
    prompt_tokens: Option<usize>,
    #[serde(default)]
    completion_tokens: Option<usize>,
    #[serde(default)]
    total_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatCompletionPayload {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiChatMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: Option<usize>,
    #[serde(default)]
    completion_tokens: Option<usize>,
    #[serde(default)]
    total_tokens: Option<usize>,
}
