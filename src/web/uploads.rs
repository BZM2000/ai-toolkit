#![allow(dead_code)]

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use axum::extract::Multipart;
use tokio::{fs::File, io::AsyncWriteExt};

/// Result type used by the shared upload helpers.
pub type UploadResult<T> = Result<T, UploadError>;

/// Error returned when validating or persisting uploaded files.
#[derive(Debug)]
pub struct UploadError {
    message: String,
}

impl UploadError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for UploadError {}

/// Describes how stored filenames should be generated for a field.
#[derive(Debug, Clone, Copy)]
pub enum FileNaming<'a> {
    /// Keep the sanitized original filename with no prefixing.
    PreserveOriginal,
    /// Prefix the sanitized original filename (no index).
    PrefixOnly { prefix: &'a str },
    /// Prefix with an incrementing index (`prefix_000_original.ext`).
    Indexed { prefix: &'a str, pad_width: usize },
}

impl<'a> FileNaming<'a> {
    fn build_name(&self, index: usize, sanitized_original: &str) -> String {
        match self {
            FileNaming::PreserveOriginal => sanitized_original.to_string(),
            FileNaming::PrefixOnly { prefix } => {
                format!("{}{}", prefix, sanitized_original)
            }
            FileNaming::Indexed { prefix, pad_width } => {
                format!(
                    "{}{:0width$}_{}",
                    prefix,
                    index,
                    sanitized_original,
                    width = *pad_width
                )
            }
        }
    }
}

/// Configuration describing the expectations for a single multipart file field.
#[derive(Debug, Clone, Copy)]
pub struct FileFieldConfig<'a> {
    pub field_name: &'a str,
    pub allowed_extensions: &'a [&'a str],
    pub max_files: usize,
    pub min_files: usize,
    pub naming: FileNaming<'a>,
}

impl<'a> FileFieldConfig<'a> {
    pub fn new(
        field_name: &'a str,
        allowed_extensions: &'a [&'a str],
        max_files: usize,
        naming: FileNaming<'a>,
    ) -> Self {
        Self {
            field_name,
            allowed_extensions,
            max_files,
            min_files: if max_files == 0 { 0 } else { 1 },
            naming,
        }
    }

    pub fn with_min_files(mut self, min_files: usize) -> Self {
        self.min_files = min_files;
        self
    }
}

/// Metadata describing a stored upload on disk.
#[derive(Debug, Clone)]
pub struct SavedFile {
    pub field_name: String,
    pub original_name: String,
    pub stored_name: String,
    pub stored_path: PathBuf,
    pub file_size: u64,
}

/// Aggregated output of the shared upload processor.
#[derive(Debug, Default)]
pub struct UploadOutcome {
    pub files: Vec<SavedFile>,
    pub text_fields: HashMap<String, Vec<String>>,
}

impl UploadOutcome {
    pub fn files_for<'a>(&'a self, field_name: &str) -> impl Iterator<Item = &'a SavedFile> {
        self.files
            .iter()
            .filter(move |file| file.field_name == field_name)
    }

    pub fn first_file_for(&self, field_name: &str) -> Option<&SavedFile> {
        self.files_for(field_name).next()
    }

    pub fn text_values(&self, field_name: &str) -> Option<&[String]> {
        self.text_fields
            .get(field_name)
            .map(|values| values.as_slice())
    }

    pub fn first_text(&self, field_name: &str) -> Option<&str> {
        self.text_values(field_name)
            .and_then(|values| values.first().map(|s| s.as_str()))
    }
}

/// Ensures the destination directory exists.
pub async fn ensure_directory(path: &Path) -> UploadResult<()> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|err| UploadError::new(format!("无法创建上传目录: {err}")))
}

/// Parses multipart form data, persisting files according to the provided configuration.
///
/// The caller is responsible for creating a unique destination directory (e.g. per job).
pub async fn process_upload_form(
    mut multipart: Multipart,
    dest_dir: &Path,
    field_configs: &[FileFieldConfig<'_>],
) -> UploadResult<UploadOutcome> {
    ensure_directory(dest_dir).await?;

    let mut field_states = HashMap::new();
    for config in field_configs {
        if config.max_files == 0 {
            return Err(UploadError::new(format!(
                "字段 `{}` 的 max_files 必须大于 0",
                config.field_name
            )));
        }
        if config.min_files > config.max_files {
            return Err(UploadError::new(format!(
                "字段 `{}` 的 min_files 不能大于 max_files",
                config.field_name
            )));
        }
        field_states.insert(
            config.field_name.to_string(),
            FieldState {
                config: *config,
                count: 0,
            },
        );
    }

    let allowed_lookup: HashMap<&str, HashSet<String>> = field_configs
        .iter()
        .map(|config| {
            let set = config
                .allowed_extensions
                .iter()
                .map(|ext| ext.to_ascii_lowercase())
                .collect();
            (config.field_name, set)
        })
        .collect();

    let mut text_fields: HashMap<String, Vec<String>> = HashMap::new();
    let mut saved_files: Vec<SavedFile> = Vec::new();
    let mut used_names: HashSet<String> = HashSet::new();

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|err| UploadError::new(format!("解析上传表单失败: {err}")))?
    {
        let field_name = field.name().unwrap_or("").to_string();

        if field.file_name().is_none() {
            let value = field
                .text()
                .await
                .map_err(|err| UploadError::new(format!("读取字段 `{field_name}` 失败: {err}")))?;
            text_fields
                .entry(field_name.clone())
                .or_default()
                .push(value);
            continue;
        }

        let Some(state) = field_states.get_mut(field_name.as_str()) else {
            return Err(UploadError::new(format!(
                "不支持的文件字段: `{field_name}`"
            )));
        };

        if state.count >= state.config.max_files {
            return Err(UploadError::new(format!(
                "字段 `{}` 上传文件数量超过限制 (最多 {})",
                state.config.field_name, state.config.max_files
            )));
        }

        let file_name = field.file_name().unwrap_or("upload.bin").to_string();
        let mut sanitized = sanitize_filename::sanitize(&file_name);
        let extension = Path::new(&file_name)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .unwrap_or_default();

        if sanitized.is_empty() {
            sanitized = if extension.is_empty() {
                format!("file_{}", state.count)
            } else {
                format!("file_{}.{}", state.count, extension)
            };
        }

        let allowed = allowed_lookup
            .get(state.config.field_name)
            .expect("allowed lookup should exist");

        if !allowed.is_empty() && !allowed.contains(&extension) {
            return Err(UploadError::new(format!(
                "字段 `{}` 不支持 `{extension}` 文件类型",
                state.config.field_name
            )));
        }

        let stored_name = unique_name(
            state.config.naming.build_name(state.count, &sanitized),
            &mut used_names,
        );
        let stored_path = dest_dir.join(&stored_name);
        let mut file = File::create(&stored_path)
            .await
            .map_err(|err| UploadError::new(format!("保存文件失败: {err}")))?;

        let mut total_bytes: u64 = 0;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|err| UploadError::new(format!("读取上传数据失败: {err}")))?
        {
            total_bytes += chunk.len() as u64;
            file.write_all(&chunk)
                .await
                .map_err(|err| UploadError::new(format!("写入文件失败: {err}")))?;
        }
        file.flush()
            .await
            .map_err(|err| UploadError::new(format!("刷新文件失败: {err}")))?;

        saved_files.push(SavedFile {
            field_name: state.config.field_name.to_string(),
            original_name: file_name,
            stored_name,
            stored_path,
            file_size: total_bytes,
        });

        state.count += 1;
    }

    // Validate minimum counts.
    for state in field_states.values() {
        if state.count < state.config.min_files {
            return Err(UploadError::new(format!(
                "字段 `{}` 至少需要上传 {} 个文件",
                state.config.field_name, state.config.min_files
            )));
        }
    }

    Ok(UploadOutcome {
        files: saved_files,
        text_fields,
    })
}

#[derive(Clone, Copy, Debug)]
struct FieldState<'a> {
    config: FileFieldConfig<'a>,
    count: usize,
}

fn unique_name(candidate: String, used: &mut HashSet<String>) -> String {
    if used.insert(candidate.clone()) {
        return candidate;
    }

    let (stem, extension) = split_name(&candidate);
    let mut counter = 1usize;
    loop {
        let attempt = if extension.is_empty() {
            format!("{}_{}", stem, counter)
        } else {
            format!("{}_{}.{}", stem, counter, extension)
        };
        if used.insert(attempt.clone()) {
            return attempt;
        }
        counter += 1;
    }
}

fn split_name(name: &str) -> (String, String) {
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string();
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    (stem, extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_preserve_original() {
        let naming = FileNaming::PreserveOriginal;
        assert_eq!(naming.build_name(0, "sample.pdf"), "sample.pdf".to_string());
    }

    #[test]
    fn naming_prefix_only() {
        let naming = FileNaming::PrefixOnly { prefix: "source_" };
        assert_eq!(
            naming.build_name(0, "doc.txt"),
            "source_doc.txt".to_string()
        );
    }

    #[test]
    fn naming_indexed() {
        let naming = FileNaming::Indexed {
            prefix: "file_",
            pad_width: 3,
        };
        assert_eq!(naming.build_name(5, "upload.txt"), "file_005_upload.txt");
    }

    #[test]
    fn unique_name_appends_counter() {
        let mut used = HashSet::new();
        let first = unique_name("file.pdf".to_string(), &mut used);
        let second = unique_name("file.pdf".to_string(), &mut used);
        assert_eq!(first, "file.pdf");
        assert_eq!(second, "file_1.pdf");
    }

    #[test]
    fn split_name_handles_extension() {
        let (stem, ext) = split_name("report.final.docx");
        assert_eq!(stem, "report.final");
        assert_eq!(ext, "docx");
    }
}
