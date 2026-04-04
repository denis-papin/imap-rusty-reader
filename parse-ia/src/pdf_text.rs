use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use log::{info, warn};
use tempfile::tempdir;

const PDF_TEXT_MAX_CHARS: usize = 80_000;
const OCR_MAX_PAGES: usize = 20;

#[derive(Debug, Clone)]
pub struct PdfTextExtraction {
    pub text: String,
    pub method: &'static str,
    pub truncated: bool,
}

pub fn extract_pdf_text(path: &Path) -> Result<Option<PdfTextExtraction>> {
    if let Some(text) = extract_native_pdf_text(path)
        .with_context(|| format!("unable to extract native PDF text {}", path.display()))?
    {
        return Ok(Some(finalize_extraction(text, "pdf_text")));
    }

    if let Some(text) =
        extract_pdf_text_with_ocr(path).with_context(|| format!("unable to OCR PDF {}", path.display()))?
    {
        return Ok(Some(finalize_extraction(text, "pdf_ocr")));
    }

    Ok(None)
}

fn extract_native_pdf_text(path: &Path) -> Result<Option<String>> {
    let extracted = pdf_extract::extract_text(path)?;
    let normalized = normalize_text(&extracted);
    if normalized.is_empty() {
        Ok(None)
    } else {
        Ok(Some(normalized))
    }
}

fn extract_pdf_text_with_ocr(path: &Path) -> Result<Option<String>> {
    let pdftoppm_available = command_available("pdftoppm");
    let tesseract_available = command_available("tesseract");
    if !pdftoppm_available || !tesseract_available {
        warn!(
            "🧰 OCR PDF skipped [{}] because external tools are unavailable (pdftoppm: {}, tesseract: {})",
            path.display(),
            pdftoppm_available,
            tesseract_available
        );
        return Ok(None);
    }

    let temp_dir = tempdir()?;
    let output_prefix = temp_dir.path().join("page");
    info!(
        "🧰 Launch external tool [pdftoppm] for OCR preprocessing [{}]",
        path.display()
    );
    let status = Command::new("pdftoppm")
        .arg("-png")
        .arg("-f")
        .arg("1")
        .arg("-l")
        .arg(OCR_MAX_PAGES.to_string())
        .arg(path)
        .arg(&output_prefix)
        .status()
        .with_context(|| "unable to launch pdftoppm for OCR fallback")?;
    if !status.success() {
        warn!(
            "💣 External tool failed [pdftoppm] for OCR preprocessing [{}] with status {}",
            path.display(),
            status
        );
        return Ok(None);
    }
    info!(
        "✅ External tool succeeded [pdftoppm] for OCR preprocessing [{}]",
        path.display()
    );

    let mut pages = fs::read_dir(temp_dir.path())?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .map(|value| value.eq_ignore_ascii_case("png"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    pages.sort();

    let mut chunks = Vec::new();
    for page in pages {
        info!(
            "🧰 Launch external tool [tesseract] for OCR [{}]",
            page.display()
        );
        let output = Command::new("tesseract")
            .arg(&page)
            .arg("stdout")
            .arg("-l")
            .arg("fra+eng")
            .output()
            .with_context(|| format!("unable to launch tesseract on {}", page.display()))?;
        if !output.status.success() {
            warn!(
                "💣 External tool failed [tesseract] for OCR [{}] with status {}",
                page.display(),
                output.status
            );
            continue;
        }
        info!(
            "✅ External tool succeeded [tesseract] for OCR [{}]",
            page.display()
        );
        let text = normalize_text(&String::from_utf8_lossy(&output.stdout));
        if !text.is_empty() {
            chunks.push(text);
        }
    }

    if chunks.is_empty() {
        Ok(None)
    } else {
        Ok(Some(chunks.join("\n\n")))
    }
}

fn command_available(name: &str) -> bool {
    Command::new(name)
        .arg("-h")
        .output()
        .map(|output| output.status.success() || !output.stdout.is_empty() || !output.stderr.is_empty())
        .unwrap_or(false)
}

fn normalize_text(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

fn finalize_extraction(text: String, method: &'static str) -> PdfTextExtraction {
    let truncated = text.chars().count() > PDF_TEXT_MAX_CHARS;
    let text = if truncated {
        text.chars().take(PDF_TEXT_MAX_CHARS).collect::<String>()
    } else {
        text
    };
    PdfTextExtraction {
        text,
        method,
        truncated,
    }
}
