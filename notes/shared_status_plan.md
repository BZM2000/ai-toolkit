## Shared Job Status Modeling Plan (2025-10-11)

### Current duplication
- **Rust**: every module defines its own `JobStatusResponse` struct with `status: String` and uses bare strings (`pending`, `processing`, `completed`, `failed`, `queued`).
- **JavaScript**: summarizer and translator include identical `translateStatus` maps; info_extract/grader hard-code status strings in the DOM; reviewer uses custom text.
- **History UI** already expects string statuses; shared history panel would benefit from standard labels.

### Proposed server-side changes
1. Introduce `enum JobStatus { Pending, Processing, Completed, Failed, Queued, Unknown(Cow<'static, str>) }` in a new module `web::status`.
2. Implement `impl<'de> Deserialize` (and `Serialize`) so modules can convert DB strings to/from the enum easily.
3. Provide helper `fn label_zh(&self) -> &'static str` for Chinese labels, optionally `fn as_str(&self)` for JSON.
4. Update `JobStatusResponse` structs: replace `status: String` with `status: JobStatus`. Modules that return raw JSON to the front-end (summarizer, translator, info_extract) should either serialize the enum or provide `status_label`.

### Proposed client-side changes
1. Replace per-module `translateStatus` JS with a shared script (extend `history_ui` or create `status_ui`) exporting `window.translateJobStatus(status)`.
2. Modules consuming inline JS call the shared function rather than duplicating maps.
3. Update CSS classes (`status-tag`) to accept canonical string values (e.g., `status-tag completed`).

### Migration steps
1. Implement `web::status` enum + helper functions.
2. Update server JSON responses in summarizer, translator, info_extract to include both `status` (enum) and `status_label` or adjust front-end to map the string.
3. Create shared JS snippet (e.g., `STATUS_SCRIPT` in `history_ui`) and embed it where needed.
4. Remove duplicate `translateStatus` definitions from modules, switching to the shared function.
5. Ensure history panels and admin dashboards use the enum/label consistently.
6. Verify reviewer integration: map its `status` string to the enum or provide a label component as needed.

### Risks
- Enum deserialization must tolerate unexpected DB values (use `Unknown` variant).
- Front-end changes require careful coordination to avoid breaking existing polling; a shared script must load before module scripts.
- Reviewer’s multi-round status may remain custom; we can keep its unique strings while leveraging the shared map for common statuses.
