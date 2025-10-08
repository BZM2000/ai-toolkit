use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tokio::task;

pub async fn convert_docx_to_pdf(docx_path: &Path) -> Result<PathBuf> {
    let output_dir = docx_path
        .parent()
        .ok_or_else(|| anyhow!("Invalid DOCX path: missing parent directory"))?;

    let docx_path_owned = docx_path.to_path_buf();
    let output_dir_owned = output_dir.to_path_buf();

    let command_result = task::spawn_blocking(move || {
        Command::new("libreoffice")
            .args([
                "--headless",
                "--convert-to",
                "pdf:writer_pdf_Export",
                "--outdir",
                &output_dir_owned.to_string_lossy(),
                &docx_path_owned.to_string_lossy(),
            ])
            .output()
    })
    .await
    .context("LibreOffice conversion task failed")?;

    let output = command_result.context("Failed to execute libreoffice command")?;

    if !output.status.success() {
        return Err(anyhow!(
            "LibreOffice conversion failed with status {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let pdf_filename = format!(
        "{}.pdf",
        docx_path
            .file_stem()
            .ok_or_else(|| anyhow!("Invalid DOCX filename"))?
            .to_string_lossy()
    );

    let pdf_path = output_dir.join(pdf_filename);

    if !pdf_path.exists() {
        return Err(anyhow!(
            "PDF file was not created at expected path: {}",
            pdf_path.display()
        ));
    }

    Ok(pdf_path)
}
