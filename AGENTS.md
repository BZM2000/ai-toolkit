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

## Project Structure
- `Cargo.toml` / `Cargo.lock`: define crate metadata, dependencies, features, and lock the exact versions used to build the toolkit.
- `src/main.rs`: bootstraps the Axum server, wires tracing, constructs `AppState`, and delegates HTTP wiring to `web::router::build_router`.
- `src/config.rs`: owns persistent configuration (`ModuleSettings`, model/prompt storage hooks) and the `ensure_defaults` logic that seeds module settings on startup.
- `src/usage.rs`: centralizes per-user usage accounting, module registration (`REGISTERED_MODULES`), and rate-limit helpers consumed by every tool.
- `src/llm/mod.rs`: shared OpenRouter/Poe client wrapper (`LlmClient`) plus request/response types, file attachments, and provider abstractions consumed across modules.
- `src/web/`: all HTTP-facing concerns.
  - `state.rs`: defines `AppState` (database pool, LLM client, config cache) and helpers for sharing application resources.
  - `router.rs`: constructs the Axum `Router` by mounting auth, admin, landing, and per-module routes and serving static assets like `robots.txt`.
  - `auth.rs`: middleware and handlers for credential-based login, session management, and guards reused by module routers.
  - `landing.rs`: renders the "Zhang Group AI Toolkit" entry page with navigation cards for every registered tool.
  - `admin_utils.rs`, `data.rs`, `models.rs`, `templates.rs`: shared HTML builders, SQL helpers, and typed query utilities used by dashboard views.
  - `admin/`: feature-specific admin UI submodules (`users.rs`, `usage_groups.rs`, `dashboard.rs`, `glossary.rs`, `journals.rs`, `auth.rs`) plus `types.rs` and `mod.rs` for routing helpers.
- `src/modules/`: encapsulated tool implementations with their own routers and admin surfaces.
  - `summarizer/`, `info_extract/`, `translatedocx/`, `grader/`, `reviewer/`: each exports `mod.rs` (tool router, handlers, background orchestration) and `admin.rs` (settings/prompt management pages).
  - `admin_shared.rs`: reusable styles, layout helpers, and widgets for module admin pages.
  - `mod.rs`: registers module routers with the main application and provides shared traits/enums for module discovery.
- `migrations/`: ordered Postgres migrations (`0001_init.sql` … `0012_info_extract.sql`) defining users, glossary, job tracking tables, module configuration storage, and usage limit schema.
- `robots.txt`: served for web crawlers via `web::router`.
- `target/`: Cargo build artifacts (ignored in version control) useful for local compilation caching.
- `storage/`: runtime directory (ignored by Git) where background jobs persist generated files, summaries, and translated documents.
 
## Shared Services & Utilities

### Application Layout
- `src/web/` owns all HTTP-facing logic: `state.rs` (shared `AppState`), `landing.rs`, `auth.rs`, and `admin.rs` (user & usage dashboards), plus `data.rs`, `models.rs`, and `templates.rs` for reusable queries and HTML.
- Module-specific admin pages live alongside each tool (`src/modules/<tool>/admin.rs`) and register their settings routes from the module router; shared styling/widgets sit in `src/modules/admin_shared.rs` and helpers in `src/web/admin_utils.rs`.
- `src/web/router.rs` builds the Axum `Router`, wiring auth, dashboard, and module routes (summarizer/infoextract/translatedocx/grader/reviewer) and serves `robots.txt`.
- `src/main.rs` is a thin bootstrap: initialize tracing, create `AppState`, call `web::router::build_router`, and start the server.
- Shared helpers are re-exported via `src/web/mod.rs` so downstream modules can pull in `AppState`, HTML utilities, and data access helpers without deep paths.

### Authentication & Sessions
- `web::auth` centralises session handling. Use `current_user` to fetch an `AuthUser`, `require_user_redirect` inside HTML handlers to bounce unauthenticated users, and `current_user_or_json_error` for JSON endpoints that should emit consistent status/message pairs.
- Sessions live in the `sessions` table, backed by the `auth_token` cookie with a 7-day TTL (`SESSION_TTL_DAYS`). `AuthUser::is_admin` flags privileged users for dashboard and download guards.
- Login and logout continue to rely on `process_login`/`logout`, which issue and revoke session rows and cookies.

### LLM Client
- Module: `src/llm/mod.rs` exposes the reusable `LlmClient` plus request/response types.
- Configure API keys via `OPENROUTER_API_KEY` and `POE_API_KEY`; optional `OPENROUTER_HTTP_REFERER` and `OPENROUTER_X_TITLE` headers can be set for OpenRouter analytics.
- Instantiate a client with `let client = LlmClient::from_env()?;` and create a request using provider-prefixed models like `openrouter/openai/gpt-4o` or `poe/claude-3-haiku`.
- Build chat turns with `ChatMessage::new(MessageRole::User, "prompt")`; attach files using `FileAttachment::new` (OpenRouter only supports `AttachmentKind::Image | Audio | Pdf`).
- Call `client.execute(request).await?` to receive `LlmResponse` containing assistant text, provider info, raw JSON, and token counts (approximate when providers omit them).

### Upload Pipeline
- **Backend** (`src/web/uploads.rs`): standardises multipart parsing and disk writes.
  - Describe expected file inputs with `FileFieldConfig::new(field, allowed_exts, max_files, FileNaming::Indexed { prefix: "source_", pad_width: 3 })`; chain `.with_min_files(n)` for required uploads.
  - Create the per-job directory with `ensure_upload_directory(&job_dir).await?`, then call `process_upload_form(multipart, &job_dir, &[config_docs, config_spec]).await?`.
  - The result `UploadOutcome` exposes `files_for("field")` iterators plus `text_fields` for ancillary inputs (`direction`, `translate`, `language`, etc.). Filenames are sanitised and deduplicated (`foo.pdf`, `foo_1.pdf`, …).
  - Example scaffold:
    ```rust
    use crate::web::{
        ensure_upload_directory, process_upload_form, FileFieldConfig, FileNaming,
    };

    ensure_upload_directory(&job_dir).await?;
    let cfg = FileFieldConfig::new(
        "files",
        &["pdf", "docx", "txt"],
        100,
        FileNaming::Indexed { prefix: "source_", pad_width: 3 },
    );

    let uploads = process_upload_form(multipart, &job_dir, &[cfg]).await?;
    for file in uploads.files_for("files") {
        tracing::debug!(?file.stored_path, %file.original_name, "queued upload");
    }
    ```
- **Frontend** (`src/web/upload_ui.rs`): shared drop-zone widget for consistent UX.
  - Embed `UPLOAD_WIDGET_STYLES` in the page `<style>` block and append `UPLOAD_WIDGET_SCRIPT` before `</body>`; the script is idempotent.
  - Render a picker with `render_upload_widget(&UploadWidgetConfig::new("summarizer-files", "summarizer-input", "files", "上传文件").with_multiple(Some(100)).with_note("最多 100 个文件").with_accept(".pdf,.docx,.txt"))`.
  - Multi-file widgets show removable chips and enforce the configured limit; single-file widgets auto-collapse to the last selection while keeping the same visual language across modules.
- Naming strategies:
  - `FileNaming::Indexed` → `prefix_000_original.ext` (recommended for multi-file jobs like summarizer & info_extract).
  - `FileNaming::PrefixOnly` → `prefix_original.ext` (single-file modules that prefer a stable prefix before the sanitized name).
  - `FileNaming::PreserveOriginal` → sanitized filename only (reviewer keeps the user-provided name).
- Adoption checklist per module:
  1. Replace the manual `Multipart` loop with `process_upload_form`, persisting DB rows from the returned `SavedFile` metadata.
  2. Swap the HTML drop-zone for `render_upload_widget`, keeping module-specific controls (checkboxes, selects) outside the widget.
  3. Remove bespoke CSS/JS once the shared widget is embedded; retain module-specific copy via `UploadWidgetConfig::with_note` or surrounding labels.

### Tool Page Layout
- `src/web/templates.rs` exposes `ToolPageLayout` and `ToolAdminLink`; call `render_tool_page` from `/tools/<module>` handlers to inherit the standard header, back link, tab chrome, and footer.
- Populate the layout slots with module-specific markup: pass the new-task panel HTML (typically two `<section class="panel">` blocks) via `new_tab_html` and reuse `history_ui::render_history_panel(MODULE_<TOOL>)` for `history_panel_html`.
- Add optional CSS/JS by pushing strings (wrapped in `.into()` / `Cow::Borrowed`) into `extra_style_blocks` and `body_scripts`. Embed `<script>…</script>` around custom scripts before pushing and reuse shared snippets like `UPLOAD_WIDGET_STYLES`/`UPLOAD_WIDGET_SCRIPT`.
- Provide an `admin_link` when the module has a dashboard settings page so the badge renders automatically; omit it for user-only tools.
- Summarizer, DOCX translator, info_extract, grader, and reviewer demonstrate the pattern—mirror their usage to avoid hand-rolled page scaffolding.

### Module Configuration
- All module model selections are stored in the `module_configs` table under the `models` JSON column. Administrators manage these values from the dedicated module setting pages inside the dashboard.
- The server seeds defaults on first boot (matching the old YAML values) via `ModuleSettings::ensure_defaults`. Subsequent edits happen through the web UI and persist in Postgres; YAML files now serve only as bootstrap defaults.
- Updating models through the admin UI triggers an in-memory reload so changes take effect without restarting the service.

### Prompt Configuration
- Prompt text shares the same `module_configs` table using the `prompts` JSON column. Each module has a dedicated admin page for editing prompt bodies (e.g. summarizer, DOCX translator, grader). Changes persist in Postgres and reload without a restart.
- Validation guards remain: summarizer translation prompts must contain `{{GLOSSARY}}`; DOCX prompts must include both `{{GLOSSARY}}` and `{{PARAGRAPH_SEPARATOR}}`; grader keyword prompts must include `{{KEYWORDS}}`.
- The server seeds initial defaults from the legacy YAML file on first run; afterwards only the admin UI controls these values.

### History & Retention
- Background jobs call `history::record_job_start` to populate `user_job_history` and power the `/api/history` endpoint plus the shared history panels.
- `history_ui` supplies the frontend panels and polling script embedded on each tool page and the `/jobs` overview.
- `maintenance::spawn` enforces the 24-hour retention policy by clearing generated files under `storage/*` and nulling persisted download paths; download handlers return HTTP `410 Gone` once resources expire.
- The retention schema adds `files_purged_at` to module job tables so history surfaces can distinguish expired outputs.

## Building a New Tool Module
1. **Module skeleton**: create `src/modules/<tool>/mod.rs` with a `Router<AppState>` exposing `/tools/<tool>` and `/api/<tool>` endpoints. Use `auth::require_user_redirect` for HTML handlers and `auth::current_user_or_json_error` (or `current_user`) inside API routes to enforce sessions consistently.
2. **Shared page layout**: render the `/tools/<tool>` handler with `render_tool_page(ToolPageLayout { .. })` so the module inherits the standard header/back link/tab shell. Supply your new-task markup via `new_tab_html`, embed `history_ui::render_history_panel(MODULE_<TOOL>)` in `history_panel_html`, and append scripts/CSS through `body_scripts`/`extra_style_blocks` (wrap custom JS in `<script>...</script>`).
3. **State/utilities**: use helpers from `AppState` (`state.pool()`/`state.llm_client()`) and shared usage accounting (`crate::usage`). Place module-specific SQL tables/migrations under `migrations/` with incremental numbering—include a `files_purged_at TIMESTAMPTZ` column on your job table for retention bookkeeping.
4. **Configuration**: extend `ModuleSettings` in `src/config.rs` if the tool needs persisted model/prompt data. Seed defaults in `ensure_defaults`, update admin forms, and persist edits via new DB columns.
5. **Admin UI wiring**: add a `modules::<tool>::admin` module to serve settings pages, wire its routes from the tool router, and reuse shared HTML helpers (`modules::admin_shared::MODULE_ADMIN_SHARED_STYLES`). POST handlers should call `state.reload_settings()` after writes.
6. **Usage metering**: register the module in `src/usage.rs` (`REGISTERED_MODULES`) with proper unit/token labels and incorporate limit checks in the module’s request path.
7. **History & retention hooks**: after inserting a new job, call `history::record_job_start(&pool, MODULE_<TOOL>, user_id, job_id)` so it appears in `/api/history` and the shared panels. Expose status/download endpoints that tolerate missing files and clear stored paths once `files_purged_at` is set.
8. **Surface links**: update the landing page cards (`web::landing::render_main_page`) to advertise the new tool, consider adding a `/jobs` panel card if it requires special messaging, and add docs/tests as necessary.

## Tool Modules

### Summarizer Module
- Routes mounted under `/tools/summarizer` (HTML form) and `/api/summarizer` (JSON/download endpoints).
- Authenticated users can upload up to 10 `.pdf`, `.docx`, or `.txt` files per job, select document type, and toggle translation; background worker writes outputs to `storage/summarizer/<job_id>/`.
- Progress and downloads:
  - `POST /tools/summarizer/jobs` → returns `job_id`.
  - `GET /api/summarizer/jobs/{job_id}` → JSON status (per-document progress, combined outputs, error info).
  - `GET /api/summarizer/jobs/{job_id}/combined/{summary|translation}` → combined text downloads.
- Glossary terms are now persisted in `glossary_terms` as EN -> CN pairs; admins manage them from the dashboard, and translation prompts incorporate the local glossary (no external fetch).
- Usage accounting: `users.usage_count` increments by successfully processed documents; request is rejected if projected usage would exceed `usage_limit`.

### Info Extract Module
- Routes mounted under `/tools/infoextract` (HTML form), `/tools/infoextract/jobs` (job creation), `/api/infoextract/jobs/{job_id}` (status polling), and `/api/infoextract/jobs/{job_id}/download/result` (XLSX download).
- Users upload 1-100 PDF manuscripts plus a required XLSX field-definition template; row 1 supplies field names, row 2 optional descriptions, row 3 optional examples (semicolon separated), and row 4 optional allowed values (mutually exclusive with examples). The template is validated before the job is queued.
- Backend persists metadata in `info_extract_jobs`/`info_extract_documents`, stores uploads under `storage/infoextract/<job_id>/`, and spawns a worker that processes up to five papers concurrently.
- Each document text is truncated to 20,000 characters before calling the configured extraction model (default `openrouter/openai/gpt-4o-mini`) with module-level system and response-guidance prompts. The worker retries failed requests up to three times with incremental 1.5 s delays and parses JSON responses into structured values.
- Successful results are aggregated into `extraction_result.xlsx` with a per-row error column; once generated, the workbook is exposed through the status endpoint for download.
- Usage tracking logs per-document units and total tokens via `usage::record_usage`; submission is rejected if the projected document count exceeds the user's limits.
- Admin settings live at `/dashboard/modules/infoextract`, letting administrators update the extraction model and prompts stored in `ModuleSettings` without restarting the service.

### DOCX Translator Module
- Routes mounted under `/tools/translatedocx` (HTML form) and `/api/translatedocx` (status/download endpoints).
- Accepts a single `.docx` file per job, with a user-facing toggle for EN → CN or CN → EN translation; glossary substitutions and the paragraph separator marker are honored in both directions.
- Background worker rewrites the uploaded file into a fresh DOCX stored at `storage/translatedocx/<job_id>/translated_1.docx` and exposes a direct download once complete.
- `docx_jobs` and `docx_documents` tables capture job and document state (including the persisted `translation_direction`); token usage and chunk counts are recorded for auditability.
- Translated downloads live at `/api/translatedocx/jobs/{job}/{doc}/download/translated`.
- Usage counting mirrors the summarizer: each successful document increments `users.usage_count`, and the job aborts if account limits would be exceeded.

### Grader Module
- Routes mounted under `/tools/grader` (HTML interface) and `/api/grader` (JSON status endpoint).
- Users upload a single `.pdf`, `.docx`, or `.txt` manuscript; the background worker extracts text, performs up to 30 LLM grading attempts (stopping early once 12 valid runs are collected), and computes an interquartile-mean score with docx-specific penalty.
- Keyword extraction runs on the same LLM (configured in `modules.grader.keyword_model`) and maps results against admin-managed topics to weight journal matches.
- Periodic progress updates are written to `grader_jobs.status_detail`; the UI polls the JSON API until completion or failure. Results include IQM score, justification, keyword summary, and a sorted list of recommended journals.
- Usage counting increments by one per successful job; jobs abort early if the projected usage would exceed a user's limit.
- Admin dashboard提供专题与期刊参考管理表单：提交同名主题或期刊会覆盖原值，期刊分值会自动更新至推荐逻辑。

### Reviewer Module
- Routes mounted under `/tools/reviewer` (HTML interface), `/api/reviewer/jobs/{id}` (status endpoint), and `/api/reviewer/jobs/{job_id}/round/{round}/review/{idx}/download` (DOCX download).
- Users upload a single `.pdf` or `.docx` manuscript and select review language (English or Chinese); the background worker orchestrates a three-round review process.
- Workflow:
  - **Round 1**: 8 parallel independent reviews using different LLM models (configured in `round1_model_1` through `round1_model_8`). Each review includes up to 3 retry attempts. Process continues if at least 4 reviews succeed; otherwise job fails.
  - **Round 2**: Meta-review synthesizing all Round 1 reports using `round2_model`, with the manuscript provided as context.
  - **Round 3**: Fact-checking the Round 2 meta-review against the manuscript using `round3_model`.
- DOCX manuscripts are automatically converted to PDF. All review outputs are saved as downloadable DOCX files.
- Configuration: 10 model settings (8 for round 1, 1 each for rounds 2 and 3) and 6 prompts (initial/secondary/final in both English and Chinese) managed through `/dashboard/modules/reviewer`.
- Database: `migrations/0010_reviewer.sql` creates `reviewer_jobs` (job metadata with UUID user_id) and `reviewer_documents` (per-round review storage with file paths).
- Usage counting: increments by 1 per successful job (token usage not tracked for reviewer module).
- Files persist in `storage/reviewer/<job_id>/` with naming convention `round{1-3}_review_{index}.docx`.

## Database
- `migrations/0002_glossary.sql` creates `glossary_terms` with case-insensitive uniqueness on `source_term`.
- `migrations/0003_summarizer.sql` adds `summary_jobs` and `summary_documents` for async processing metadata; indexes support job history lookups.
- `migrations/0004_translatedocx.sql` and `0005_docx_direction.sql` track DOCX translation jobs/documents and persist chosen translation direction.
- `migrations/0006_grader.sql` introduces `grader_jobs`, `grader_documents`, `journal_topics`, `journal_reference_entries`, and `journal_topic_scores`. Journal topics and reference rows are editable from the admin dashboard and are used by the grader module for keyword weighting and threshold adjustments.
- `migrations/0010_reviewer.sql` adds `reviewer_jobs` (with UUID user_id referencing users table) and `reviewer_documents` (tracking per-round reviews with file paths, status, and error messages).
- `migrations/0012_info_extract.sql` creates `info_extract_jobs` (tracking owner, spec metadata, status, aggregate tokens/units) and `info_extract_documents` (per-PDF status, parsed JSON, attempt counts, token usage).

## File System
- Runtime artifacts persist under `storage/summarizer/`, `storage/infoextract/`, `storage/translatedocx/`, `storage/grader/`, and `storage/reviewer/`; `.gitignore` ignores the entire `storage/` directory.
- Summarizer job directories persist only combined outputs (`combined_summary.txt`, optional `combined_translation.txt`) with Markdown-style headings.
- Info Extract job directories cache the uploaded PDFs, the validated XLSX schema, and the generated `extraction_result.xlsx` workbook.
- Reviewer job directories contain DOCX files: `round1_review_{1-8}.docx`, `round2_meta_review.docx`, and `round3_final_report.docx`.

## Docker Deployment
- `Dockerfile` provides a multi-stage build for Railway deployment.
- **Stage 1 (Builder)**: Uses `rust:1.82-slim-bookworm` base, installs build dependencies (pkg-config, libssl-dev), and compiles the release binary using dependency caching.
- **Stage 2 (Runtime)**: Uses `debian:bookworm-slim` base, installs runtime dependencies such as SSL certificates, copies the built binary and migrations, and exposes port 3000.
- `.dockerignore` excludes `target/`, `storage/`, `.git/`, and development files to optimize build performance.
- Railway automatically detects the Dockerfile and builds the container; no `railway.json` configuration needed.
- Required environment variables: `DATABASE_URL`, `OPENROUTER_API_KEY`, `POE_API_KEY` (optional: `OPENROUTER_HTTP_REFERER`, `OPENROUTER_X_TITLE`).

## Testing & Verification
- Unit tests (`cargo test`) cover translation prompt assembly and DOCX text extraction helpers.
- For manual end-to-end checks: run `cargo run`, log in as an admin, add glossary entries, submit a summarizer job, watch `/api/summarizer/jobs/{id}` poll results, and verify downloads.
- Build verification: `cargo build --release` to compile all modules.

## Centralisation Masterplan
- **Unified auth/session helpers**: expose shared `require_user` variants from `web::auth` so modules depend on a single guard implementation (HTML redirect + JSON error adapters) instead of duplicating SQL session checks and `SessionUser` structs.
- **Shared API response scaffolding**: create `web::responses` with reusable `ApiError`, `JobSubmissionResponse`, and `internal_error` helpers to eliminate per-module clones and align error copy.
- **Consistent job status modeling**: define a core `JobStatus` enum + serde helpers and bundle a shared front-end label map, keeping Axum responses and UI tags in sync across modules and the history panel.
- **Storage & download utilities**: extract `storage::ensure_root` and `download_guard` helpers that encapsulate directory creation, ownership checks, and `files_purged_at` handling before streaming outputs.
- **Job poller client kit**: publish a shared JS initializer (e.g., `window.initJobForm`) that wraps FormData submission, status messaging, and polling intervals so each tool only supplies render callbacks.

### Detailed Plan: Unified auth/session helpers
1. **Inventory current guards**
   - Catalogue `require_user` implementations and `SessionUser` structs in all modules (`summarizer`, `translatedocx`, `info_extract`, `grader`, `reviewer`) plus `web::history` to confirm required fields and error handling variants (redirect vs. JSON response).
2. **Design shared interface**
   - Extend `web::auth` with a reusable `SessionUser` (aliasing existing `AuthUser`) and provide helper functions: `require_user_redirect(jar, state)` returning `Result<AuthUser, Redirect>` and `require_user_json(jar, state)` returning `Result<AuthUser, (StatusCode, Json<ApiError>)>`.
   - Allow optional admin enforcement and custom unauthorized messages via parameters so modules avoid bespoke checks.
3. **Implement backend helpers**
   - Refactor `web::auth` to expose the helpers, ensuring they reuse existing `fetch_user_by_session` logic and centralize tracing/error logs.
   - Add targeted unit or integration tests (if feasible) covering valid session, expired session, and admin-only scenarios.
4. **Migrate modules incrementally**
   - Update each module to drop local `SessionUser`/`require_user`, import the shared helper, and adjust call sites (HTML handlers use redirect variant; JSON endpoints map errors into their existing `ApiError`).
   - Remove redundant SQL queries, making sure admin gates still behave correctly.
5. **Cleanup & verification**
   - Run `cargo fmt` + `cargo check` to confirm compilation.
   - Smoke-test at least one HTML and one JSON endpoint per module in dev to ensure redirects and error bodies match expectations.
   - Document the helper usage pattern in `AGENTS.md` or module quick-start notes for future contributors.

## Agent Log
- 2025-10-08 (Codex agent): Ran `cargo test` after recent usage aggregation fixes, resolved new `GlossaryTermRow` field requirements in summarizer and DOCX translator tests, and confirmed test suite now passes.
- 2025-10-08 (Codex agent): Restored OpenRouter audio attachment payload format by adding MIME→format mapping in `src/llm/mod.rs`, verified with `cargo test`.
- 2025-10-08 (Claude Code agent): Implemented complete Reviewer module (审稿助手) with three-round academic review workflow:
  - Created `migrations/0010_reviewer.sql` with UUID user_id foreign key to users table
  - Fixed P0 issues: corrected session authentication using sessions table join, removed UUID casting, proper UUID handling throughout
  - Fixed P1 bug: persisted Round 2 and Round 3 DOCX file paths to `reviewer_documents.file_path` (previously caused 404s on download endpoints)
  - Integrated DOCX→PDF conversion via LibreOffice `--headless` mode
  - Created Dockerfile with multi-stage build including LibreOffice installation for Railway deployment
  - Added .dockerignore for optimized Docker builds
  - Added LibreOffice integration documentation section for future developers
  - All routes registered in router.rs, usage tracking in usage.rs, landing page card added
  - Admin settings page at `/dashboard/modules/reviewer` for managing 10 models and 6 prompts (EN/CN)
- 2025-10-08 (Gemini agent): Replaced the LibreOffice-based DOCX to PDF conversion with a pure-Rust implementation using `docx-rs` and `printpdf`.
  - Created a new shared utility module at `src/utils/docx_to_pdf.rs`.
  - Removed the `libreoffice` dependency from the `Dockerfile`.
  - Updated the `reviewer` module to use the new utility.
  - Updated `AGENTS.md` to reflect the changes.
- 2025-10-09 (Codex agent): Restored LibreOffice-backed DOCX→PDF conversion to fix multi-page truncation.
  - Replaced the pure-Rust renderer with a `spawn_blocking` wrapper around `libreoffice --headless --convert-to pdf:writer_pdf_Export` in `src/utils/docx_to_pdf.rs` and re-exported it for the reviewer module.
  - Removed the unused `printpdf` dependency from `Cargo.toml`/`Cargo.lock` and reinstalled LibreOffice packages in the runtime Docker image.
  - Verified the build with `cargo check` and documented the change here.
- 2025-10-09 (Codex agent): Converted per-module token caps into a single rolling 7-day pool shared across all tools.
  - Added migration `0011_global_token_limit.sql` to move `token_limit` onto `usage_groups` and drop the per-module column.
  - Refactored `src/usage.rs` to enforce the global token window while keeping per-module unit limits and future module extensibility.
  - Updated admin dashboard and usage group forms to manage the shared token budget and display per-user totals.
- 2025-10-09 (Codex agent): Added 信息提取模块 with concurrent schema-driven extraction.
  - Built `src/modules/info_extract` to validate XLSX field definitions, batch ingest up to 100 PDFs, orchestrate five-way parallel processing with three LLM retries, and emit job summaries as Excel downloads.
  - Extended configuration (`InfoExtractModels`/`InfoExtractPrompts`), admin UI (`/dashboard/modules/infoextract`), and landing/router wiring; registered `MODULE_INFO_EXTRACT` for rate limiting and usage accounting.
  - Created `migrations/0012_info_extract.sql`, ensured storage under `storage/infoextract/`, and exposed `/api/infoextract/jobs/{id}` plus `/download/result` endpoints for polling and retrieval.
- 2025-10-10 (Codex agent): Introduced shared upload utilities.
- 2025-10-10 (Codex agent): Migrated summarizer、DOCX translator、grader模块到共享上传工具。
- 2025-10-10 (Codex agent): 信息提取模块接入共享上传工具（多文件论文 + 单 XLSX 字段表）。
- 2025-10-10 (Codex agent): 审稿助手模块改用共享上传组件。
  - `process_upload_form` 负责稿件上传并在任务建好后迁移至最终目录；保留 DOCX→PDF 转换流程。
  - 页面使用 `render_upload_widget`，脚本仅处理语言选择、轮询与状态提示。
  - 后端改用 `process_upload_form` 配置双字段，去掉手写 multipart；字段表仍落盘并即时解析。
  - 前端采用两个 `render_upload_widget`，脚本保留轮询逻辑但移除自定义拖拽 UI。
  - 服务端改用 `process_upload_form`/`FileFieldConfig` 统一校验与存储，清理手写 multipart 循环。
  - 前端页面统一引用 `render_upload_widget` 与共享脚本，移除自定义拖拽样式/逻辑。
- 2025-10-11 (Codex agent): 实现 24 小时任务历史与存储清理机制。
  - 新增迁移 `0013_history_and_retention.sql` 引入 `user_job_history` 并为五个模块的任务记录添加 `files_purged_at` 标记。
  - 构建共享历史服务：任务提交即调用 `record_job_start`，`fetch_recent_jobs` 聚合状态，`/api/history` 对外提供统一 JSON。
  - 新增 `history_ui`（共享样式 + JS），在各工具页加入“历史记录”页签，同时上线 `/jobs` 汇总页与首页导航卡片。
  - 编写 `maintenance::spawn` 定时任务，24 小时后清理 `storage/*` 产物并清空下载路径；下载接口遇到过期资源返回 410 提示。
  - 前端历史面板支持轮询、状态详情与下载链接，过期任务显示“结果已清除”并禁用下载按钮。
  - 各模块作业创建后统一记录历史，成功/失败队列与下载端点均反映清理状态，确保 24 小时后自动失效。
- 2025-10-11 (Codex agent): 统一工具页布局，新增 `ToolPageLayout`/`render_tool_page` 并迁移五个模块以复用共享 header/标签页壳，更新新模块指南与文档说明，`cargo check` 通过。
- 2025-10-11 (Codex agent): Centralised session guards via `web::auth::{current_user, require_user_redirect, current_user_or_json_error}`; removed per-module `SessionUser` structs and aligned history/api handlers to the shared helpers, `cargo fmt` + `cargo check` clean.
