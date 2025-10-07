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

### Current Web Application Layout (2025-xx)
- `src/web/` owns all HTTP-facing logic: `state.rs` (shared `AppState`), `landing.rs`, `auth.rs`, and `admin.rs` (user & usage dashboards), plus `data.rs`, `models.rs`, and `templates.rs` for reusable queries and HTML.
- Module-specific admin pages live alongside each tool (`src/modules/<tool>/admin.rs`) and register their settings routes from the module router; shared styling/widgets sit in `src/modules/admin_shared.rs` and helpers in `src/web/admin_utils.rs`.
- `src/web/router.rs` builds the Axum `Router`, wiring auth, dashboard, and module routes (summarizer/translatedocx/grader) and serves `robots.txt`.
- `src/main.rs` is now a thin bootstrap: initialize tracing, create `AppState`, call `web::router::build_router`, and start the server.
- Downstream modules continue to consume shared helpers via re-exports in `src/web/mod.rs` (e.g., glossary/journal fetch helpers, `AppState`, HTML utilities).

### Adding a New Tool Module (quick guide)
1. **Module skeleton**: create `src/modules/<tool>/mod.rs` with a `Router<AppState>` builder (`pub fn router() -> Router<AppState>`) exposing `/tools/<tool>` and any `/api/<tool>` endpoints. Follow the structure used by summarizer/translatedocx/grader (shared auth guards live in `web::auth`).
2. **State/utilities**: use helpers from `AppState` (`state.pool()`/`state.llm_client()`) and shared usage accounting (`crate::usage`). Place module-specific SQL tables/migrations under `migrations/` with incremental numbering.
3. **Configuration**: extend `ModuleSettings` in `src/config.rs` if the tool needs persisted model/prompt data. Seed defaults in `ensure_defaults`, update admin forms, and persist edits via new DB columns.
4. **Admin UI wiring**: add a `modules::<tool>::admin` module to serve settings pages, wire its routes from the tool router, and reuse shared HTML helpers (`modules::admin_shared::MODULE_ADMIN_SHARED_STYLES`). POST handlers should call `state.reload_settings()` after writes.
5. **Usage metering**: register the module in `src/usage.rs` (`REGISTERED_MODULES`) with proper unit/token labels and incorporate limit checks in the module’s request path.
6. **Surface links**: update the landing page cards (`web::landing::render_main_page`) to advertise the new tool and add docs/tests as necessary.

## Shared LLM Utility
- Module: `src/llm/mod.rs` exposes the reusable `LlmClient` plus request/response types.
- Configure API keys via `OPENROUTER_API_KEY` and `POE_API_KEY`; optional `OPENROUTER_HTTP_REFERER` and `OPENROUTER_X_TITLE` headers can be set for OpenRouter analytics.
- Instantiate a client with `let client = LlmClient::from_env()?;` and create a request using provider-prefixed models like `openrouter/openai/gpt-4o` or `poe/claude-3-haiku`.
- Build chat turns with `ChatMessage::new(MessageRole::User, "prompt")`; attach files using `FileAttachment::new` (OpenRouter only supports `AttachmentKind::Image | Audio | Pdf`).
- Call `client.execute(request).await?` to receive `LlmResponse` containing assistant text, provider info, raw JSON, and token counts (approximate when providers omit them).

## Model Configuration
- All module model selections are stored in the `module_configs` table under the `models` JSON column. Administrators manage these values from the dedicated module setting pages inside the dashboard.
- The server seeds defaults on first boot (matching the old YAML values) via `ModuleSettings::ensure_defaults`. Subsequent edits happen through the web UI and persist in Postgres; YAML files now serve only as bootstrap defaults.
- Updating models through the admin UI triggers an in-memory reload so changes take effect without restarting the service.

## Prompt Configuration
- Prompt text shares the same `module_configs` table using the `prompts` JSON column. Each module has a dedicated admin page for editing prompt bodies (e.g. summarizer, DOCX translator, grader). Changes are persisted in Postgres and reloaded without a restart.
- Validation guards remain: summarizer translation prompts must contain `{{GLOSSARY}}`; DOCX prompts must include both `{{GLOSSARY}}` and `{{PARAGRAPH_SEPARATOR}}`; grader keyword prompts must include `{{KEYWORDS}}`.
- The server seeds initial defaults from the legacy YAML file on first run; afterwards only the admin UI controls these values.

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

## Grader Module
- Routes mounted under `/tools/grader` (HTML interface) and `/api/grader` (JSON status endpoint).
- Users upload a single `.pdf`, `.docx`, or `.txt` manuscript; the background worker extracts text, performs up to 30 LLM grading attempts (stopping early once 12 valid runs are collected), and computes an interquartile-mean score with docx-specific penalty.
- Keyword extraction runs on the same LLM (configured in `modules.grader.keyword_model`) and maps results against admin-managed topics to weight journal matches.
- Periodic progress updates are written to `grader_jobs.status_detail`; the UI polls the JSON API until completion or failure. Results include IQM score, justification, keyword summary, and a sorted list of recommended journals.
- Usage counting increments by one per successful job; jobs abort early if the projected usage would exceed a user's limit.
- Admin dashboard提供专题与期刊参考管理表单：提交同名主题或期刊会覆盖原值，期刊分值会自动更新至推荐逻辑。

## Database
- `migrations/0002_glossary.sql` creates `glossary_terms` with case-insensitive uniqueness on `source_term`.
- `migrations/0003_summarizer.sql` adds `summary_jobs` and `summary_documents` for async processing metadata; indexes support job history lookups.
- `migrations/0004_translatedocx.sql` and `0005_docx_direction.sql` track DOCX translation jobs/documents and persist chosen translation direction.
- `migrations/0006_grader.sql` introduces `grader_jobs`, `grader_documents`, `journal_topics`, `journal_reference_entries`, and `journal_topic_scores`. Journal topics and reference rows are editable from the admin dashboard and are used by the grader module for keyword weighting and threshold adjustments.

## File System
- Runtime artifacts persist under `storage/summarizer/`; `.gitignore` ignores this directory by default.
- Each job directory contains `summary_n.txt`, optional `translation_n.txt`, and combined outputs created with Markdown-style headings for readability.

## Testing & Verification
- Unit tests (`cargo test`) cover translation prompt assembly and DOCX text extraction helpers.
- For manual end-to-end checks: run `cargo run`, log in as an admin, add glossary entries, submit a summarizer job, watch `/api/summarizer/jobs/{id}` poll results, and verify downloads.
