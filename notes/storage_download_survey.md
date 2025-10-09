## Storage & Download Handling Survey (2025-10-11)

### Summarizer (`src/modules/summarizer/mod.rs`)
- **Storage root:** `ensure_storage_root()` wraps `tokio_fs::create_dir_all(STORAGE_ROOT)` before uploads and workers.
- **Download guard:** `download_combined_output` validates ownership/admin, checks `files_purged_at`, ensures requested variant exists, and returns `ApiMessage` errors.

### DOCX Translator (`src/modules/translatedocx/mod.rs`)
- **Storage root:** same `ensure_storage_root()` pattern.
- **Download guard:** `download_document_output` verifies variant, ownership/admin, purge status, and translated path before streaming via `serve_docx_file`.

### Info Extract (`src/modules/info_extract/mod.rs`)
- **Storage root:** `ensure_storage_root()` handles doc/spec uploads.
- **Download guard:** `download_result` enforces ownership/admin, `files_purged_at`, and result path, then assembles XLSX headers.

### Grader (`src/modules/grader/mod.rs`)
- **Storage root:** `ensure_storage_root()` exists, but no direct download endpoint (JSON-only output).

### Reviewer (`src/modules/reviewer/mod.rs`)
- **Storage creation:** manual temp → final dir workflow (`tokio_fs::create_dir_all(&final_dir)`), no shared helper.
- **Download guard:** `download_review` checks ownership/admin and purge state, locates round-specific file, and streams DOCX.

### Common Patterns & Gaps
- Ownership checks consistently compare `record.user_id` with `user.id` plus admin override.
- `files_purged_at` gating with `410 Gone` in summarizer, translator, info_extract, reviewer.
- Error payloads unified via `ApiMessage` after recent refactor.
- Storage root creation duplicated per module; reviewer is bespoke.
- Download handlers repeat query → ownership → purge → path retrieval logic with module-specific response shaping.
