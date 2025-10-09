## Next Shared Service Candidate (2025-10-11)

### Current State
- Upload handling, auth guards, response helpers, storage/download access, and tool page layout are now unified.
- Module-specific `JobStatusResponse` structs and front-end polling scripts (`translateStatus`, etc.) still repeat the same status-label mapping and boilerplate across modules.
- Reviewer remains bespoke on the front-end (multi-round flow), but the summarizer/translator/info_extract/grader pages share similar JavaScript patterns (FormData submit → poll → render table) with minor variations.

### Potential Targets
1. **Job status modeling**
   - Define a shared `JobStatus` enum and label translator for common statuses (`pending`, `processing`, `completed`, `failed`, `queued`).
   - Impact: consistent JSON + UI mapping, less copy/paste in inline scripts.
   - Complexity: medium; modules have extra fields, but the core status string is identical.

2. **Frontend polling kit**
   - Create `history_ui`-like shared JS for form submission + polling (e.g., `initJobForm`), consumed by summarizer, translator, info_extract, grader.
   - Impact: removes repetitive DOM manipulation scripts.
   - Complexity: medium/high due to module-specific rendering functions.

3. **LLM retry/backoff utilities**
   - Summarizer, info_extract, reviewer contain custom retry logic; centralising would reduce divergence.
   - Complexity: high (different retry counts, jitter, context).

### Recommendation
Focus next on **job status modeling + frontend status translation**:
- Provide a shared `JobStatus` enum in Rust with `from_db` and `label()` helpers, plus a JS translation map exported through `history_ui` or a new shared script.
- Update modules to emit structured statuses and consume the shared translator, reducing inline JS duplication and ensuring future modules inherit the standard labels.

### Next Steps
1. Catalogue how each module maps status strings to labels (already noted via `translateStatus` functions).
2. Design the Rust enum + Serde integration and the corresponding JS map (possibly extending `history_ui`).
3. Plan migration sequence (server → client) ensuring backward compatibility.
