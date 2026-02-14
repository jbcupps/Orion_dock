//! File ingestion capability for chat attachments.
//!
//! This module parses uploaded files into safe, read-only context the LLM can
//! understand. It never executes uploaded file contents.

use flate2::read::GzDecoder;
use image::ImageReader;
use serde::{Deserialize, Serialize};
use std::cmp::min;
use std::io::{Cursor, Read};
use std::path::Path;
use std::process::Command;
use tar::Archive;
use zip::ZipArchive;

#[derive(Debug, Clone)]
pub struct FileIngestionLimits {
    pub max_extract_chars: usize,
    pub max_archive_entries: usize,
    pub max_nested_read_bytes: usize,
    pub max_context_chars: usize,
}

impl Default for FileIngestionLimits {
    fn default() -> Self {
        Self {
            max_extract_chars: 30_000,
            max_archive_entries: 200,
            max_nested_read_bytes: 4 * 1024 * 1024,
            max_context_chars: 16_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileIngestionResult {
    pub detected_mime: String,
    pub detected_kind: String,
    pub parse_status: String,
    pub summary: String,
    pub llm_context: String,
    pub warnings: Vec<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
struct ParseOutcome {
    parse_status: String,
    summary: String,
    llm_context: String,
    warnings: Vec<String>,
    metadata: serde_json::Value,
}

pub fn ingest_file_bytes(
    file_name: Option<&str>,
    declared_content_type: Option<&str>,
    bytes: &[u8],
    limits: &FileIngestionLimits,
) -> FileIngestionResult {
    let detected_mime = detect_mime(file_name, declared_content_type, bytes);
    let detected_kind = detect_kind(file_name, &detected_mime);

    let parsed = match detected_kind.as_str() {
        "text" => parse_text(bytes, &detected_mime, limits),
        "pdf" => parse_pdf(bytes, limits),
        "docx" => parse_docx(bytes, limits),
        "xlsx" => parse_xlsx(bytes, limits),
        "archive" => parse_archive(file_name, bytes, limits),
        "image" => parse_image(file_name, bytes, limits),
        _ => parse_binary(bytes, limits),
    };

    let outcome = match parsed {
        Ok(o) => o,
        Err(err) => {
            let mut fallback = parse_binary(bytes, limits).unwrap_or_else(|_| ParseOutcome {
                parse_status: "fallback".to_string(),
                summary: "Unable to parse file".to_string(),
                llm_context: "No readable content extracted.".to_string(),
                warnings: vec![],
                metadata: serde_json::json!({}),
            });
            fallback.warnings.push(format!(
                "Type-specific parser failed; fell back to binary summary: {}",
                err
            ));
            fallback
        }
    };

    FileIngestionResult {
        detected_mime,
        detected_kind,
        parse_status: outcome.parse_status,
        summary: outcome.summary,
        llm_context: truncate_chars(&outcome.llm_context, limits.max_context_chars),
        warnings: outcome.warnings,
        metadata: outcome.metadata,
    }
}

fn detect_mime(
    file_name: Option<&str>,
    declared_content_type: Option<&str>,
    bytes: &[u8],
) -> String {
    if let Some(kind) = infer::get(bytes) {
        return kind.mime_type().to_string();
    }

    if let Some(ct) = declared_content_type {
        let trimmed = ct.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let ext = file_name
        .and_then(|n| Path::new(n).extension().and_then(|e| e.to_str()))
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("txt") | Some("md") | Some("rs") | Some("ts") | Some("tsx") | Some("js")
        | Some("jsx") | Some("py") | Some("go") | Some("java") | Some("c") | Some("cpp")
        | Some("toml") | Some("ini") | Some("conf") | Some("csv") | Some("log") | Some("yaml")
        | Some("yml") => "text/plain".to_string(),
        Some("json") => "application/json".to_string(),
        Some("xml") => "application/xml".to_string(),
        Some("pdf") => "application/pdf".to_string(),
        Some("docx") => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string()
        }
        Some("xlsx") => {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string()
        }
        Some("zip") => "application/zip".to_string(),
        Some("tar") => "application/x-tar".to_string(),
        Some("gz") => "application/gzip".to_string(),
        Some("png") => "image/png".to_string(),
        Some("jpg") | Some("jpeg") => "image/jpeg".to_string(),
        Some("gif") => "image/gif".to_string(),
        Some("webp") => "image/webp".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

fn detect_kind(file_name: Option<&str>, mime: &str) -> String {
    let ext = file_name
        .and_then(|n| Path::new(n).extension().and_then(|e| e.to_str()))
        .map(|e| e.to_ascii_lowercase());
    if mime.starts_with("text/")
        || matches!(
            mime,
            "application/json"
                | "application/xml"
                | "application/x-yaml"
                | "text/yaml"
                | "text/csv"
        )
        || matches!(
            ext.as_deref(),
            Some("txt")
                | Some("md")
                | Some("json")
                | Some("yaml")
                | Some("yml")
                | Some("xml")
                | Some("csv")
                | Some("rs")
                | Some("ts")
                | Some("tsx")
                | Some("js")
                | Some("jsx")
                | Some("py")
                | Some("go")
                | Some("java")
                | Some("c")
                | Some("cpp")
                | Some("toml")
                | Some("log")
                | Some("conf")
        )
    {
        return "text".to_string();
    }

    if mime == "application/pdf" || matches!(ext.as_deref(), Some("pdf")) {
        return "pdf".to_string();
    }
    if matches!(ext.as_deref(), Some("docx")) {
        return "docx".to_string();
    }
    if matches!(ext.as_deref(), Some("xlsx")) {
        return "xlsx".to_string();
    }
    if mime.starts_with("image/")
        || matches!(
            ext.as_deref(),
            Some("png") | Some("jpg") | Some("jpeg") | Some("gif") | Some("bmp") | Some("webp")
        )
    {
        return "image".to_string();
    }
    if mime == "application/zip"
        || mime == "application/x-tar"
        || mime == "application/gzip"
        || matches!(
            ext.as_deref(),
            Some("zip") | Some("tar") | Some("gz") | Some("tgz")
        )
    {
        return "archive".to_string();
    }
    "binary".to_string()
}

fn parse_text(
    bytes: &[u8],
    mime: &str,
    limits: &FileIngestionLimits,
) -> Result<ParseOutcome, String> {
    let mut warnings = Vec::new();
    let mut text = String::from_utf8_lossy(bytes).to_string();
    if text.chars().count() > limits.max_extract_chars {
        text = truncate_chars(&text, limits.max_extract_chars);
        warnings.push(format!(
            "Text content truncated to {} chars",
            limits.max_extract_chars
        ));
    }
    let line_count = text.lines().count();
    let summary = format!(
        "Text content parsed (mime: {}, {} chars, {} lines).",
        mime,
        text.chars().count(),
        line_count
    );
    Ok(ParseOutcome {
        parse_status: "parsed".to_string(),
        summary,
        llm_context: text.clone(),
        warnings,
        metadata: serde_json::json!({
            "line_count": line_count,
            "char_count": text.chars().count()
        }),
    })
}

fn parse_pdf(bytes: &[u8], limits: &FileIngestionLimits) -> Result<ParseOutcome, String> {
    let extracted = pdf_extract::extract_text_from_mem(bytes).map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    let text = if extracted.chars().count() > limits.max_extract_chars {
        warnings.push(format!(
            "PDF text truncated to {} chars",
            limits.max_extract_chars
        ));
        truncate_chars(&extracted, limits.max_extract_chars)
    } else {
        extracted
    };
    Ok(ParseOutcome {
        parse_status: "parsed".to_string(),
        summary: format!("PDF text extracted ({} chars).", text.chars().count()),
        llm_context: text.clone(),
        warnings,
        metadata: serde_json::json!({
            "char_count": text.chars().count()
        }),
    })
}

fn parse_docx(bytes: &[u8], limits: &FileIngestionLimits) -> Result<ParseOutcome, String> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    let mut chunks = Vec::new();
    let mut found_docs = 0usize;

    for path in [
        "word/document.xml",
        "word/footnotes.xml",
        "word/endnotes.xml",
        "word/comments.xml",
    ] {
        if let Ok(mut file) = zip.by_name(path) {
            found_docs += 1;
            let mut xml = String::new();
            file.read_to_string(&mut xml).map_err(|e| e.to_string())?;
            let plain = collapse_ws(&strip_xml_tags(&xml));
            if !plain.is_empty() {
                chunks.push(plain);
            }
        }
    }

    if chunks.is_empty() {
        warnings.push("No readable DOCX text segments found".to_string());
    }

    let joined = chunks.join("\n\n");
    let text = truncate_chars(&joined, limits.max_extract_chars);
    if joined.chars().count() > text.chars().count() {
        warnings.push(format!(
            "DOCX text truncated to {} chars",
            limits.max_extract_chars
        ));
    }

    Ok(ParseOutcome {
        parse_status: if chunks.is_empty() {
            "partial".to_string()
        } else {
            "parsed".to_string()
        },
        summary: format!(
            "DOCX parsed ({} document XML sections, {} chars extracted).",
            found_docs,
            text.chars().count()
        ),
        llm_context: if text.trim().is_empty() {
            "No readable DOCX text extracted.".to_string()
        } else {
            text.clone()
        },
        warnings,
        metadata: serde_json::json!({
            "document_sections": found_docs,
            "char_count": text.chars().count()
        }),
    })
}

fn parse_xlsx(bytes: &[u8], limits: &FileIngestionLimits) -> Result<ParseOutcome, String> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    let mut shared_strings: Vec<String> = Vec::new();
    let mut sheets: Vec<(String, String)> = Vec::new();

    if let Ok(mut ss) = zip.by_name("xl/sharedStrings.xml") {
        let mut xml = String::new();
        ss.read_to_string(&mut xml).map_err(|e| e.to_string())?;
        shared_strings = extract_tag_values(&xml, "t")
            .into_iter()
            .map(|v| collapse_ws(&decode_xml_entities(&v)))
            .filter(|v| !v.is_empty())
            .collect();
    }

    let mut sheet_names: Vec<String> = Vec::new();
    for i in 0..zip.len() {
        let name = {
            let f = zip.by_index(i).map_err(|e| e.to_string())?;
            f.name().to_string()
        };
        if name.starts_with("xl/worksheets/") && name.ends_with(".xml") {
            sheet_names.push(name);
        }
    }
    sheet_names.sort();
    if sheet_names.len() > 12 {
        warnings.push(format!(
            "Workbook has {} sheets; only first 12 were sampled",
            sheet_names.len()
        ));
        sheet_names.truncate(12);
    }

    for name in &sheet_names {
        if let Ok(mut file) = zip.by_name(name) {
            let mut xml = String::new();
            file.read_to_string(&mut xml).map_err(|e| e.to_string())?;
            let stripped = collapse_ws(&strip_xml_tags(&xml));
            let preview = truncate_chars(&stripped, 1200);
            sheets.push((name.clone(), preview));
        }
    }

    let mut context = String::new();
    if !shared_strings.is_empty() {
        context.push_str("Shared strings sample:\n");
        for s in shared_strings.iter().take(60) {
            context.push_str("- ");
            context.push_str(s);
            context.push('\n');
        }
    }
    if !sheets.is_empty() {
        if !context.is_empty() {
            context.push('\n');
        }
        context.push_str("Sheet text samples:\n");
        for (sheet, preview) in &sheets {
            context.push_str(&format!("--- {} ---\n{}\n\n", sheet, preview));
        }
    }
    if context.is_empty() {
        context = "No readable XLSX text extracted.".to_string();
        warnings.push("Workbook content appears empty or non-textual".to_string());
    }
    context = truncate_chars(&context, limits.max_extract_chars);

    Ok(ParseOutcome {
        parse_status: if context.contains("No readable XLSX") {
            "partial".to_string()
        } else {
            "parsed".to_string()
        },
        summary: format!(
            "XLSX parsed ({} sheet samples, {} shared strings).",
            sheets.len(),
            shared_strings.len()
        ),
        llm_context: context,
        warnings,
        metadata: serde_json::json!({
            "sheet_count_sampled": sheets.len(),
            "shared_strings_count": shared_strings.len()
        }),
    })
}

fn parse_archive(
    file_name: Option<&str>,
    bytes: &[u8],
    limits: &FileIngestionLimits,
) -> Result<ParseOutcome, String> {
    let ext = file_name
        .and_then(|n| Path::new(n).extension().and_then(|e| e.to_str()))
        .map(|e| e.to_ascii_lowercase());

    if matches!(ext.as_deref(), Some("zip")) || looks_like_zip(bytes) {
        return parse_zip_archive(bytes, limits);
    }
    if matches!(ext.as_deref(), Some("tar")) {
        return parse_tar_archive(bytes, limits);
    }
    if matches!(ext.as_deref(), Some("gz") | Some("tgz")) {
        return parse_gzip_archive(bytes, limits);
    }

    // Fall back: try zip first, then tar, then gzip.
    parse_zip_archive(bytes, limits)
        .or_else(|_| parse_tar_archive(bytes, limits))
        .or_else(|_| parse_gzip_archive(bytes, limits))
}

fn looks_like_zip(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[..4] == [0x50, 0x4b, 0x03, 0x04]
}

fn parse_zip_archive(bytes: &[u8], limits: &FileIngestionLimits) -> Result<ParseOutcome, String> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let total_entries = zip.len();
    let mut warnings = Vec::new();
    let mut entries_meta: Vec<serde_json::Value> = Vec::new();
    let mut context_snippets: Vec<String> = Vec::new();
    let mut nested_read = 0usize;
    let cap = min(total_entries, limits.max_archive_entries);

    for idx in 0..cap {
        let mut file = zip.by_index(idx).map_err(|e| e.to_string())?;
        let name = file.name().to_string();
        let is_dir = file.is_dir();
        let size = file.size();
        let compressed = file.compressed_size();
        entries_meta.push(serde_json::json!({
            "name": name,
            "is_dir": is_dir,
            "size_bytes": size,
            "compressed_size_bytes": compressed
        }));

        if !is_dir
            && context_snippets.len() < 4
            && size <= 256 * 1024
            && nested_read < limits.max_nested_read_bytes
        {
            let room = limits.max_nested_read_bytes.saturating_sub(nested_read);
            let mut data = Vec::new();
            let mut take = (&mut file).take(room as u64);
            take.read_to_end(&mut data).map_err(|e| e.to_string())?;
            nested_read += data.len();
            if is_probably_text(&data) {
                let snippet = truncate_chars(&collapse_ws(&String::from_utf8_lossy(&data)), 1200);
                if !snippet.trim().is_empty() {
                    context_snippets.push(format!("{}:\n{}", file.name(), snippet));
                }
            }
        }
    }

    if total_entries > cap {
        warnings.push(format!(
            "Archive contains {} entries; only first {} were inspected",
            total_entries, cap
        ));
    }

    let mut context = String::new();
    context.push_str("Archive entry listing:\n");
    for item in entries_meta.iter().take(60) {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
        let size = item.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
        let marker = if item
            .get("is_dir")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            "dir"
        } else {
            "file"
        };
        context.push_str(&format!("- {} ({}, {} bytes)\n", name, marker, size));
    }
    if !context_snippets.is_empty() {
        context.push_str("\nText samples from archive:\n");
        for snippet in context_snippets {
            context.push_str("---\n");
            context.push_str(&snippet);
            context.push('\n');
        }
    }

    Ok(ParseOutcome {
        parse_status: "parsed".to_string(),
        summary: format!(
            "ZIP archive inspected ({} entries scanned of {}).",
            cap, total_entries
        ),
        llm_context: truncate_chars(&context, limits.max_extract_chars),
        warnings,
        metadata: serde_json::json!({
            "entries_total": total_entries,
            "entries_scanned": cap,
            "entries": entries_meta
        }),
    })
}

fn parse_tar_archive(bytes: &[u8], limits: &FileIngestionLimits) -> Result<ParseOutcome, String> {
    let mut archive = Archive::new(Cursor::new(bytes));
    let mut warnings = Vec::new();
    let mut entries_meta: Vec<serde_json::Value> = Vec::new();
    let mut context_snippets: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut nested_read = 0usize;

    let entries = archive.entries().map_err(|e| e.to_string())?;
    for entry in entries {
        let mut entry = entry.map_err(|e| e.to_string())?;
        if scanned >= limits.max_archive_entries {
            break;
        }
        scanned += 1;
        let path = entry
            .path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string();
        let size = entry.header().size().unwrap_or(0);
        entries_meta.push(serde_json::json!({
            "name": path,
            "size_bytes": size
        }));

        if context_snippets.len() < 4
            && size <= 256 * 1024
            && nested_read < limits.max_nested_read_bytes
        {
            let room = limits.max_nested_read_bytes.saturating_sub(nested_read);
            let mut data = Vec::new();
            let mut take = (&mut entry).take(room as u64);
            take.read_to_end(&mut data).map_err(|e| e.to_string())?;
            nested_read += data.len();
            if is_probably_text(&data) {
                let snippet = truncate_chars(&collapse_ws(&String::from_utf8_lossy(&data)), 1200);
                if !snippet.trim().is_empty() {
                    context_snippets.push(format!("{}:\n{}", path, snippet));
                }
            }
        }
    }

    if scanned >= limits.max_archive_entries {
        warnings.push(format!(
            "TAR archive scanning capped at {} entries",
            limits.max_archive_entries
        ));
    }

    let mut context = String::new();
    context.push_str("Archive entry listing:\n");
    for item in entries_meta.iter().take(60) {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
        let size = item.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
        context.push_str(&format!("- {} ({} bytes)\n", name, size));
    }
    if !context_snippets.is_empty() {
        context.push_str("\nText samples from archive:\n");
        for snippet in context_snippets {
            context.push_str("---\n");
            context.push_str(&snippet);
            context.push('\n');
        }
    }

    Ok(ParseOutcome {
        parse_status: "parsed".to_string(),
        summary: format!("TAR archive inspected ({} entries scanned).", scanned),
        llm_context: truncate_chars(&context, limits.max_extract_chars),
        warnings,
        metadata: serde_json::json!({
            "entries_scanned": scanned,
            "entries": entries_meta
        }),
    })
}

fn parse_gzip_archive(bytes: &[u8], limits: &FileIngestionLimits) -> Result<ParseOutcome, String> {
    let mut warnings = Vec::new();
    let mut decoder = GzDecoder::new(Cursor::new(bytes));
    let mut decompressed = Vec::new();
    let mut take = (&mut decoder).take(limits.max_nested_read_bytes as u64);
    take.read_to_end(&mut decompressed)
        .map_err(|e| e.to_string())?;

    if decompressed.is_empty() {
        return Err("GZIP payload decompressed to empty content".to_string());
    }
    if looks_like_tar(&decompressed) {
        let mut parsed = parse_tar_archive(&decompressed, limits)?;
        parsed
            .warnings
            .push("GZIP was expanded and interpreted as TAR".to_string());
        return Ok(parsed);
    }

    let context = if is_probably_text(&decompressed) {
        truncate_chars(
            &String::from_utf8_lossy(&decompressed),
            limits.max_extract_chars,
        )
    } else {
        warnings.push("GZIP payload appears binary after decompression".to_string());
        binary_preview(&decompressed)
    };

    Ok(ParseOutcome {
        parse_status: "parsed".to_string(),
        summary: format!("GZIP payload decompressed ({} bytes).", decompressed.len()),
        llm_context: context,
        warnings,
        metadata: serde_json::json!({
            "decompressed_bytes": decompressed.len()
        }),
    })
}

fn parse_image(
    file_name: Option<&str>,
    bytes: &[u8],
    limits: &FileIngestionLimits,
) -> Result<ParseOutcome, String> {
    let mut warnings = Vec::new();
    let mut reader = ImageReader::new(Cursor::new(bytes));
    reader = reader.with_guessed_format().map_err(|e| e.to_string())?;
    let format = reader.format().map(|f| format!("{:?}", f));
    let image = reader.decode().map_err(|e| e.to_string())?;
    let width = image.width();
    let height = image.height();
    let color = format!("{:?}", image.color());

    let ext = file_name
        .and_then(|n| Path::new(n).extension().and_then(|e| e.to_str()))
        .unwrap_or("png");
    let ocr_text = match try_ocr_with_tesseract(bytes, ext, limits.max_extract_chars) {
        Ok(text) if !text.trim().is_empty() => Some(text),
        Ok(_) => {
            warnings.push("OCR produced no text".to_string());
            None
        }
        Err(reason) => {
            warnings.push(format!("OCR unavailable: {}", reason));
            None
        }
    };

    let mut context = format!(
        "Image metadata:\n- Dimensions: {}x{}\n- Color model: {}\n- Format: {}\n",
        width,
        height,
        color,
        format.clone().unwrap_or_else(|| "unknown".to_string())
    );
    if let Some(ref text) = ocr_text {
        context.push_str("\nOCR text:\n");
        context.push_str(&truncate_chars(text, limits.max_extract_chars));
    } else {
        context.push_str("\nOCR text not available in this runtime.");
    }

    Ok(ParseOutcome {
        parse_status: if ocr_text.is_some() {
            "parsed".to_string()
        } else {
            "partial".to_string()
        },
        summary: format!(
            "Image analyzed ({}x{}, {}, OCR {}).",
            width,
            height,
            color,
            if ocr_text.is_some() {
                "available"
            } else {
                "unavailable"
            }
        ),
        llm_context: truncate_chars(&context, limits.max_extract_chars),
        warnings,
        metadata: serde_json::json!({
            "width": width,
            "height": height,
            "color": color,
            "format": format,
            "ocr_available": ocr_text.is_some(),
        }),
    })
}

fn parse_binary(bytes: &[u8], limits: &FileIngestionLimits) -> Result<ParseOutcome, String> {
    let entropy = shannon_entropy(bytes);
    let preview = binary_preview(bytes);
    Ok(ParseOutcome {
        parse_status: "fallback".to_string(),
        summary: format!(
            "Binary fallback summary ({} bytes, entropy {:.3}).",
            bytes.len(),
            entropy
        ),
        llm_context: truncate_chars(&preview, limits.max_extract_chars),
        warnings: vec![],
        metadata: serde_json::json!({
            "byte_len": bytes.len(),
            "entropy": entropy,
            "hex_prefix": hex_prefix(bytes, 64),
        }),
    })
}

fn strip_xml_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_xml_entities(&out)
}

fn extract_tag_values(xml: &str, tag_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    let open = format!("<{}>", tag_name);
    let close = format!("</{}>", tag_name);
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        if let Some(end) = rest.find(&close) {
            out.push(rest[..end].to_string());
            rest = &rest[end + close.len()..];
        } else {
            break;
        }
    }
    out
}

fn decode_xml_entities(input: &str) -> String {
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn collapse_ws(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut out = String::new();
    for ch in input.chars().take(max_chars) {
        out.push(ch);
    }
    out.push_str(" ...[truncated]");
    out
}

fn is_probably_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    if std::str::from_utf8(bytes).is_ok() {
        return true;
    }
    let printable = bytes
        .iter()
        .filter(|b| {
            b.is_ascii_graphic() || **b == b'\n' || **b == b'\r' || **b == b'\t' || **b == b' '
        })
        .count();
    printable * 100 / bytes.len() > 85
}

fn hex_prefix(bytes: &[u8], max: usize) -> String {
    bytes
        .iter()
        .take(max)
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

fn binary_preview(bytes: &[u8]) -> String {
    let hex = hex_prefix(bytes, 96);
    let ascii = bytes
        .iter()
        .take(200)
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '.'
            }
        })
        .collect::<String>();
    format!("Hex preview:\n{}\n\nASCII preview:\n{}", hex, ascii)
}

fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for b in bytes {
        counts[*b as usize] += 1;
    }
    let len = bytes.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

fn looks_like_tar(bytes: &[u8]) -> bool {
    bytes.len() > 262 && &bytes[257..262] == b"ustar"
}

fn try_ocr_with_tesseract(bytes: &[u8], ext: &str, max_chars: usize) -> Result<String, String> {
    if Command::new("tesseract").arg("--version").output().is_err() {
        return Err("tesseract binary not found in PATH".to_string());
    }

    let unique = format!(
        "orion-ocr-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    );
    let path = std::env::temp_dir().join(format!("{}.{}", unique, ext));
    std::fs::write(&path, bytes).map_err(|e| format!("write temp image: {}", e))?;
    let output = Command::new("tesseract")
        .arg(&path)
        .arg("stdout")
        .arg("--dpi")
        .arg("150")
        .output()
        .map_err(|e| format!("run tesseract: {}", e));
    let _ = std::fs::remove_file(&path);
    let output = output?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("tesseract exited with {}", output.status)
        } else {
            stderr
        });
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(truncate_chars(text.trim(), max_chars))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_plain_text() {
        let limits = FileIngestionLimits::default();
        let result = ingest_file_bytes(
            Some("notes.txt"),
            Some("text/plain"),
            b"hello\nworld\nfrom orion",
            &limits,
        );
        assert_eq!(result.detected_kind, "text");
        assert_eq!(result.parse_status, "parsed");
        assert!(result.llm_context.contains("hello"));
    }

    #[test]
    fn malformed_pdf_falls_back_safely() {
        let limits = FileIngestionLimits::default();
        let result = ingest_file_bytes(
            Some("broken.pdf"),
            Some("application/pdf"),
            b"%PDF-not-a-real-pdf-content",
            &limits,
        );
        assert_eq!(result.detected_kind, "pdf");
        assert_eq!(result.parse_status, "fallback");
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn binary_blob_has_entropy_and_hex() {
        let mut blob = Vec::new();
        for i in 0..1024 {
            blob.push((i % 251) as u8);
        }
        let limits = FileIngestionLimits::default();
        let result = ingest_file_bytes(Some("blob.bin"), None, &blob, &limits);
        assert_eq!(result.detected_kind, "binary");
        assert!(result.llm_context.contains("Hex preview"));
        assert!(
            result
                .metadata
                .get("entropy")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                > 0.0
        );
    }

    #[test]
    fn zip_entry_limit_applies() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::<u8>::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for i in 0..12 {
            writer
                .start_file(format!("f-{}.txt", i), opts)
                .expect("start_file");
            writer.write_all(b"hello archive").expect("write");
        }
        let cursor = writer.finish().expect("finish");
        let bytes = cursor.into_inner();

        let limits = FileIngestionLimits {
            max_archive_entries: 5,
            ..FileIngestionLimits::default()
        };
        let result =
            ingest_file_bytes(Some("sample.zip"), Some("application/zip"), &bytes, &limits);
        assert_eq!(result.detected_kind, "archive");
        assert!(result.warnings.iter().any(|w| w.contains("only first 5")));
    }
}
