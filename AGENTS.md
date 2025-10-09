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
  - `summarizer/`, `translatedocx/`, `grader/`, `reviewer/`: each exports `mod.rs` (tool router, handlers, background orchestration) and `admin.rs` (settings/prompt management pages).
  - `admin_shared.rs`: reusable styles, layout helpers, and widgets for module admin pages.
  - `mod.rs`: registers module routers with the main application and provides shared traits/enums for module discovery.
- `migrations/`: ordered Postgres migrations (`0001_init.sql` … `0010_reviewer.sql`) defining users, glossary, job tracking tables, module configuration storage, and usage limit schema.
- `robots.txt`: served for web crawlers via `web::router`.
- `target/`: Cargo build artifacts (ignored in version control) useful for local compilation caching.
- `storage/`: runtime directory (ignored by Git) where background jobs persist generated files, summaries, and translated documents.

### Current Web Application Layout (2025-xx)
- `src/web/` owns all HTTP-facing logic: `state.rs` (shared `AppState`), `landing.rs`, `auth.rs`, and `admin.rs` (user & usage dashboards), plus `data.rs`, `models.rs`, and `templates.rs` for reusable queries and HTML.
- Module-specific admin pages live alongside each tool (`src/modules/<tool>/admin.rs`) and register their settings routes from the module router; shared styling/widgets sit in `src/modules/admin_shared.rs` and helpers in `src/web/admin_utils.rs`.
- `src/web/router.rs` builds the Axum `Router`, wiring auth, dashboard, and module routes (summarizer/translatedocx/grader/reviewer) and serves `robots.txt`.
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

## DOCX to PDF Conversion
- **Implementation**: DOCX to PDF conversion is handled by a pure-Rust implementation using the `docx-rs` and `printpdf` crates.
- **Location**: The conversion logic is located in the `src/utils/docx_to_pdf.rs` module, making it a shared utility that can be used by any module in the toolkit.
- **Functionality**: The current implementation performs a basic conversion, extracting text from the DOCX and placing it into a PDF. It does not preserve complex formatting, images, or tables.
- **Usage**:
  ```rust
  use crate::utils::docx_to_pdf::convert_docx_to_pdf;
  use std::path::Path;

  async fn example(docx_path: &Path) -> anyhow::Result<()> {
      let pdf_path = convert_docx_to_pdf(docx_path).await?;
      // ...
      Ok(())
  }
  ```
- **Performance**: This pure-Rust approach is significantly faster and more memory-efficient than the previous LibreOffice-based solution, as it avoids the overhead of starting a separate process.

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

## Reviewer Module
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

## File System
- Runtime artifacts persist under `storage/summarizer/`, `storage/translatedocx/`, `storage/grader/`, and `storage/reviewer/`; `.gitignore` ignores the entire `storage/` directory.
- Summarizer job directories contain `summary_n.txt`, optional `translation_n.txt`, and combined outputs with Markdown-style headings.
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
