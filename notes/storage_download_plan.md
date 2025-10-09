## Shared Storage & Download Helpers – Design Plan (2025-10-11)

### Goals
- Eliminate duplicate storage-root creation logic across modules.
- Centralise job ownership / `files_purged_at` checks for download endpoints while letting reviewers keep round-specific lookups.
- Provide ergonomic helpers that return consistent `ApiMessage` errors via `json_error`.

### Proposed Helpers (`src/web/storage.rs`)
1. `pub async fn ensure_storage_root(path: &str) -> Result<()>`
   - Thin wrapper around `tokio_fs::create_dir_all(path)` used by summarizer, translator, info_extract, grader, reviewer.

2. `pub struct JobAccess { pub user_id: Uuid, pub files_purged_at: Option<DateTime<Utc>> }`
   - Represents the common ownership metadata across modules.

3. `pub async fn verify_job_access<T>(pool: &PgPool, query: sqlx::QueryAs<'_, Postgres, T>, requester: &AuthUser) -> Result<T, (StatusCode, Json<ApiMessage>)>`
   - Accepts a prepared query returning `T: JobAccessExt` (trait exposing `user_id`/`files_purged_at`).
   - Checks owner vs requester (with `is_admin` override) and `files_purged_at` (returning `StatusCode::GONE`).
   - Returns the hydrated record for module-specific handling.

4. `pub fn require_path(path: Option<String>, missing_msg: &str) -> Result<String, (StatusCode, Json<ApiMessage>)>`
   - Ensures optional output paths are present (reused by summarizer, translator, info_extract).

5. `pub async fn stream_file(path: &Path, filename: &str, content_type: &str) -> Result<Response, (StatusCode, Json<ApiMessage>)>`
   - Handles reading bytes + setting standard headers. Translator can wrap this for DOCX; info_extract can build custom headers on top (or accept optional override).

### Reviewer Accommodation
- Reviewer can call `ensure_storage_root(STORAGE_ROOT)` during job creation.
- For downloads, run `verify_job_access` on the job row, then perform its round/index query; reuse `require_path` and `stream_file` once the doc path is known.
- Add a small helper `json_response` to convert `(StatusCode, Json<ApiMessage>)` into `Response` (already present).

### Migration Order
1. Implement `web::storage` helpers + trait `JobAccessExt`, add tests.
2. Update summarizer download + ensure root usage.
3. Migrate docx translator (download & ensure root).
4. Migrate info_extract download handler.
5. Update reviewer create/download flows.
6. Remove per-module `ensure_storage_root` functions and duplicate guard code.
7. Update docs (`AGENTS.md`) with new helper usage.

