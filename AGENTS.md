# Zhang Group AI Toolkit Notes

## Vision
- Build the Zhang Group AI Toolkit: a modular platform that invokes LLM APIs behind the scenes to execute various tasks.
- Provide up to 10 distinct tools/features accessible from a central landing page.

## Core Requirements
- Include user login with credentials provisioned by an administrator (no public self-signup).
- Offer an admin-facing user management dashboard to track usage counts/limits per user.
- Support file upload/download, multi-step task progress reporting, and text input for tasks.
- Ensure architecture supports background calls to LLM providers.

## Technical Preferences
- Implement backend and services in Rust for performance and reliability. Reference rust docs if necessary: https://github.com/rust-lang/book
- Design front-end landing page that links to each tool/module and consistently brands the experience as "Zhang Group AI Toolkit."
- Deploy: Railway (consider container-friendly setup, environment variables for config) through Railway CLI.
- Plan for modular expansion up to ~10 tools, each encapsulated for maintainability. There will be shared utilities for things like LLM API calling.
- Add observability in admin dashboard (logging, metrics) suitable for Railway deployment.
- Implement rate limiting per user based on usage metrics (requests/tokens) stored in persistent storage.

## Shared LLM Utility
- Module: `src/llm/mod.rs` exposes the reusable `LlmClient` plus request/response types.
- Configure API keys via `OPENROUTER_API_KEY` and `POE_API_KEY`; optional `OPENROUTER_HTTP_REFERER` and `OPENROUTER_X_TITLE` headers can be set for OpenRouter analytics.
- Instantiate a client with `let client = LlmClient::from_env()?;` and create a request using provider-prefixed models like `openrouter/openai/gpt-4o` or `poe/claude-3-haiku`.
- Build chat turns with `ChatMessage::new(MessageRole::User, "prompt")`; attach files using `FileAttachment::new` (OpenRouter only supports `AttachmentKind::Image | Audio | Pdf`).
- Call `client.execute(request).await?` to receive `LlmResponse` containing assistant text, provider info, raw JSON, and token counts (approximate when providers omit them).

## Model Configuration
- All tool model selections live in `config/models.yaml`; override path with `MODELS_CONFIG_PATH` env var if needed.
- Example schema:
  ```yaml
  modules:
    summarizer:
      summary_model: "openrouter/anthropic/claude-3-haiku"
      translation_model: "openrouter/openai/gpt-4o-mini"
    translate_docx:
      translation_model: "openrouter/openai/gpt-4o-mini"
  ```
- `AppState` loads this once at startup; modules clone the config via `state.models_config()`.
- When adding new modules, extend the `modules` map with a section matching the module name and any required model identifiers; keep keys snake_case to match Rust struct fields.
- Configuration re-load requires application restart—there is no hot reload yet.

## Prompt Configuration
- Prompt copy for modules is stored in `config/prompts.yaml`; override the path with `PROMPTS_CONFIG_PATH` if you need an alternate config per environment.
- The config mirrors the models file: add a section under `modules` matching the module name and define named prompt strings (use multi-line blocks with `|-` for readability).
- Summarizer translation copy lives under `translation` and must include the placeholder `{{GLOSSARY}}`, which is replaced at runtime with one EN -> CN pair per line. Omit the placeholder only if the prompt already explains how to reference a glossary.
- DOCX translator prompts expose `en_to_cn` and `cn_to_en` strings under `modules.translate_docx`; each must include `{{GLOSSARY}}` and `{{PARAGRAPH_SEPARATOR}}` so the runtime can inject glossary lines and the paragraph boundary marker used for chunking.
- Keep prompts trimmed of Markdown unless the downstream module explicitly renders Markdown; summarizer currently treats prompts as plain text.
- As with the models config, any prompt changes require restarting the server so `AppState` reloads the YAML.

## Summarizer Module
- Routes mounted under `/tools/summarizer` (HTML form) and `/api/summarizer` (JSON/download endpoints).
- Authenticated users can upload up to 10 `.pdf`, `.docx`, or `.txt` files per job, select document type, and toggle translation; background worker writes outputs to `storage/summarizer/<job_id>/`.
- Progress and downloads:
  - `POST /tools/summarizer/jobs` → returns `job_id`.
  - `GET /api/summarizer/jobs/{job_id}` → JSON status (per-document links, combined outputs, error info).
  - `GET /api/summarizer/jobs/{job_id}/documents/{doc_id}/download/{summary|translation}` → authenticated file stream.
  - `GET /api/summarizer/jobs/{job_id}/combined/{summary|translation}` → combined text downloads.
- Glossary terms are now persisted in `glossary_terms` as EN -> CN pairs; admins manage them from the dashboard, and translation prompts incorporate the local glossary (no external fetch).
- Usage accounting: `users.usage_count` increments by successfully processed documents; request is rejected if projected usage would exceed `usage_limit`.

## DOCX Translator Module
- Routes mounted under `/tools/translatedocx` (HTML form) and `/api/translatedocx` (status/download endpoints).
- Accepts a single `.docx` file per job, with a user-facing toggle for EN → CN or CN → EN translation; glossary substitutions and the paragraph separator marker are honored in both directions.
- Background worker rewrites the uploaded file into a fresh DOCX stored at `storage/translatedocx/<job_id>/translated_1.docx` and exposes a direct download once complete.
- `docx_jobs` and `docx_documents` tables capture job and document state (including the persisted `translation_direction`); token usage and chunk counts are recorded for auditability.
- Translated downloads live at `/api/translatedocx/jobs/{job}/{doc}/download/translated`.
- Usage counting mirrors the summarizer: each successful document increments `users.usage_count`, and the job aborts if account limits would be exceeded.

## Database
- `migrations/0002_glossary.sql` creates `glossary_terms` with case-insensitive uniqueness on `source_term`.
- `migrations/0003_summarizer.sql` adds `summary_jobs` and `summary_documents` for async processing metadata; indexes support job history lookups.

## File System
- Runtime artifacts persist under `storage/summarizer/`; `.gitignore` ignores this directory by default.
- Each job directory contains `summary_n.txt`, optional `translation_n.txt`, and combined outputs created with Markdown-style headings for readability.

## Testing & Verification
- Unit tests (`cargo test`) cover translation prompt assembly and DOCX text extraction helpers.
- For manual end-to-end checks: run `cargo run`, log in as an admin, add glossary entries, submit a summarizer job, watch `/api/summarizer/jobs/{id}` poll results, and verify downloads.
