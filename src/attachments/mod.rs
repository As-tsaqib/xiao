use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use image::GenericImageView;
use quick_xml::{events::Event, Reader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zip::ZipArchive;

use crate::{
    config::AttachmentConfig,
    security::redact::redact_text,
    storage::{AttachmentChunkRecord, AttachmentRecord, NewAttachmentRecord, Storage},
};

const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

#[derive(Debug, Clone)]
pub struct AttachmentIngest {
    pub owner_id: String,
    pub session_id: String,
    pub telegram_file_id: Option<String>,
    pub telegram_unique_id: Option<String>,
    pub original_name: String,
    pub declared_mime: Option<String>,
    pub expected_kind: AttachmentKind,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    Document,
}

impl AttachmentKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Document => "document",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NormalizedImage {
    pub attachment_id: String,
    pub mime_type: String,
    #[serde(skip)]
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub caption: String,
}

#[derive(Clone)]
pub struct AttachmentManager {
    storage: Arc<Storage>,
    root: Arc<PathBuf>,
    config: AttachmentConfig,
}

impl AttachmentManager {
    pub fn new(
        storage: Arc<Storage>,
        data_root: impl Into<PathBuf>,
        config: AttachmentConfig,
    ) -> Result<Self> {
        let root = data_root.into().join("data/attachments");
        create_private_dir(&root)?;
        Ok(Self {
            storage,
            root: Arc::new(root),
            config,
        })
    }

    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    pub fn max_download_bytes(&self, kind: AttachmentKind) -> u64 {
        match kind {
            AttachmentKind::Image => self.config.max_image_bytes,
            AttachmentKind::Document => self.config.max_document_bytes,
        }
    }

    pub fn processing_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.config.processing_timeout_seconds)
    }

    pub fn ingest(&self, input: AttachmentIngest) -> Result<AttachmentRecord> {
        if input.bytes.is_empty() {
            return Err(anyhow!("Telegram attachment is empty"));
        }
        if let Some(unique_id) = input.telegram_unique_id.as_deref() {
            if let Some(existing) = self.storage.attachment_by_telegram_unique(
                &input.owner_id,
                &input.session_id,
                unique_id,
            )? {
                return Ok(existing);
            }
        }
        let byte_limit = self.max_download_bytes(input.expected_kind);
        if input.bytes.len() as u64 > byte_limit {
            return Err(anyhow!("attachment exceeds the {} byte limit", byte_limit));
        }
        let current = self
            .storage
            .session_attachment_bytes(&input.owner_id, &input.session_id)?;
        if current.saturating_add(input.bytes.len() as u64) > self.config.max_session_bytes {
            return Err(anyhow!("attachment would exceed the session storage quota"));
        }

        let detection = detect_content(&input.bytes, &input.original_name)?;
        let actual_kind = if detection.mime.starts_with("image/") {
            AttachmentKind::Image
        } else {
            AttachmentKind::Document
        };
        if input.expected_kind == AttachmentKind::Image && actual_kind != AttachmentKind::Image {
            return Err(anyhow!(
                "Telegram photo content is not a supported image ({})",
                detection.mime
            ));
        }
        if actual_kind == AttachmentKind::Image
            && input.bytes.len() as u64 > self.config.max_image_bytes
        {
            return Err(anyhow!("image exceeds the configured image limit"));
        }

        let attachment_id = Uuid::new_v4().to_string();
        let owner_dir = self.root.join(short_hash(&input.owner_id));
        let session_dir = owner_dir.join(short_hash(&input.session_id));
        create_private_dir(&owner_dir)?;
        create_private_dir(&session_dir)?;
        let final_path = session_dir.join(format!("{attachment_id}.bin"));
        atomic_private_write(&final_path, &input.bytes)?;

        let sha256 = format!("{:x}", Sha256::digest(&input.bytes));
        let original_name = safe_filename(&input.original_name);
        let local_path = final_path
            .to_str()
            .ok_or_else(|| anyhow!("attachment path is not valid UTF-8"))?
            .to_owned();
        let insert = self.storage.insert_attachment(NewAttachmentRecord {
            attachment_id: &attachment_id,
            owner_id: &input.owner_id,
            session_id: &input.session_id,
            telegram_file_id: input.telegram_file_id.as_deref(),
            telegram_unique_id: input.telegram_unique_id.as_deref(),
            original_name: &original_name,
            declared_mime: input.declared_mime.as_deref(),
            detected_mime: &detection.mime,
            kind: actual_kind.as_str(),
            size_bytes: input.bytes.len() as u64,
            sha256: &sha256,
            local_path: &local_path,
        });
        if let Err(error) = insert {
            let _ = fs::remove_file(&final_path);
            if let Some(unique_id) = input.telegram_unique_id.as_deref() {
                if let Some(existing) = self.storage.attachment_by_telegram_unique(
                    &input.owner_id,
                    &input.session_id,
                    unique_id,
                )? {
                    return Ok(existing);
                }
            }
            return Err(error);
        }

        let processing = self.process(&input.owner_id, &attachment_id, &input.bytes, &detection);
        if let Err(error) = processing {
            let safe = bound(&redact_text(&error.to_string()), 1_000);
            self.storage.set_attachment_status(
                &input.owner_id,
                &attachment_id,
                "failed",
                None,
                Some(&safe),
            )?;
            return Err(anyhow!(safe));
        }
        self.storage
            .attachment(&input.owner_id, &attachment_id)?
            .ok_or_else(|| anyhow!("ingested attachment metadata disappeared"))
    }

    fn process(
        &self,
        owner: &str,
        attachment_id: &str,
        bytes: &[u8],
        detection: &DetectedContent,
    ) -> Result<()> {
        self.storage
            .set_attachment_status(owner, attachment_id, "processing", None, None)?;
        if detection.mime.starts_with("image/") {
            let (width, height) = validate_image(bytes, self.config.max_image_pixels)?;
            let summary = format!(
                "Validated {} image, {}×{} pixels, {} bytes",
                detection.mime,
                width,
                height,
                bytes.len()
            );
            return self.storage.set_attachment_status(
                owner,
                attachment_id,
                "ready",
                Some(&summary),
                None,
            );
        }

        let extracted =
            extract_document(bytes, &detection.mime, self.config.max_extracted_text_chars)?;
        let normalized = normalize_text(&extracted.text);
        if normalized
            .chars()
            .filter(|character| !character.is_whitespace())
            .count()
            < 8
        {
            if detection.mime == "application/pdf" {
                return self.storage.set_attachment_status(
                    owner,
                    attachment_id,
                    "needs_ocr",
                    Some("PDF has no meaningful embedded text; OCR or a vision-capable model is required"),
                    None,
                );
            }
            return Err(anyhow!("document contains no meaningful extractable text"));
        }
        let chunks = chunk_text(attachment_id, &normalized, self.config.chunk_chars);
        self.storage
            .replace_attachment_chunks(owner, attachment_id, &chunks)?;
        let summary = format!(
            "{} document; {} characters indexed in {} chunks. Preview: {}",
            detection.mime,
            normalized.chars().count(),
            chunks.len(),
            bound(&normalized, 240)
        );
        self.storage
            .set_attachment_status(owner, attachment_id, "ready", Some(&summary), None)
    }

    pub fn recent_for_prompt(
        &self,
        owner: &str,
        session_id: &str,
        prompt: &str,
        limit: usize,
    ) -> Result<Vec<AttachmentRecord>> {
        if !references_attachment(prompt) {
            return Ok(Vec::new());
        }
        let mut recent = self
            .storage
            .recent_attachments(owner, session_id, limit.clamp(1, 10))?;
        if let Some(position) = requested_ordinal(prompt) {
            recent.reverse();
            return Ok(recent.into_iter().nth(position).into_iter().collect());
        }
        recent.truncate(limit);
        Ok(recent)
    }

    pub fn normalized_images(
        &self,
        owner: &str,
        session_id: &str,
        prompt: &str,
    ) -> Result<Vec<NormalizedImage>> {
        self.recent_for_prompt(owner, session_id, prompt, 3)?
            .into_iter()
            .filter(|attachment| attachment.kind == "image")
            .map(|attachment| {
                if attachment.processing_status != "ready" {
                    return Err(anyhow!("referenced image is not ready for provider input"));
                }
                let path = Path::new(&attachment.local_path);
                if !path.starts_with(self.root.as_path()) {
                    return Err(anyhow!("attachment path escaped the controlled store"));
                }
                let bytes = fs::read(path)?;
                if bytes.len() as u64 != attachment.size_bytes
                    || format!("{:x}", Sha256::digest(&bytes)) != attachment.sha256
                {
                    return Err(anyhow!("attachment integrity verification failed"));
                }
                let (width, height) = validate_image(&bytes, self.config.max_image_pixels)?;
                Ok(NormalizedImage {
                    attachment_id: attachment.attachment_id,
                    mime_type: attachment.detected_mime,
                    bytes,
                    width,
                    height,
                    caption: bound(prompt, 2_000),
                })
            })
            .collect()
    }

    pub fn context_block(
        &self,
        owner: &str,
        session_id: &str,
        prompt: &str,
    ) -> Result<Option<String>> {
        let recent = self.recent_for_prompt(owner, session_id, prompt, 4)?;
        if recent.is_empty() {
            return Ok(None);
        }
        let mut rows = Vec::new();
        for attachment in &recent {
            rows.push(format!(
                "- id={} name={} type={} status={} summary={}",
                attachment.attachment_id,
                attachment.original_name,
                attachment.detected_mime,
                attachment.processing_status,
                attachment.summary.as_deref().unwrap_or("none")
            ));
        }
        let chunks = self.storage.search_attachment_chunks(
            owner,
            session_id,
            prompt,
            self.config.retrieval_chunks,
        )?;
        if chunks.is_empty() && recent.len() == 1 && recent[0].kind == "document" {
            for chunk in self
                .storage
                .attachment_chunks(owner, &recent[0].attachment_id, 2)?
            {
                rows.push(format!(
                    "DOCUMENT_EXCERPT attachment={} chunk={}: {}",
                    chunk.attachment_id,
                    chunk.chunk_no,
                    bound(&chunk.text, self.config.chunk_chars)
                ));
            }
        } else {
            for chunk in chunks {
                rows.push(format!(
                    "RELEVANT_EXCERPT attachment={} chunk={}: {}",
                    chunk.attachment_id,
                    chunk.chunk_no,
                    bound(&chunk.text, self.config.chunk_chars)
                ));
            }
        }
        Ok(Some(format!(
            "<SESSION_ATTACHMENTS verified_runtime_data=true>\n{}\n</SESSION_ATTACHMENTS>",
            rows.join("\n")
        )))
    }

    pub fn health(&self) -> Result<String> {
        create_private_dir(self.root.as_path())?;
        self.storage.with_conn(|connection| {
            connection.query_row("SELECT COUNT(*) FROM attachment_fts", [], |_| Ok(()))?;
            Ok(())
        })?;
        Ok(format!("store={} FTS5 readable", self.root.display()))
    }
}

#[derive(Debug)]
struct DetectedContent {
    mime: String,
}

fn detect_content(bytes: &[u8], original_name: &str) -> Result<DetectedContent> {
    if bytes.starts_with(b"%PDF-") {
        return Ok(DetectedContent {
            mime: "application/pdf".into(),
        });
    }
    if is_docx(bytes) {
        return Ok(DetectedContent {
            mime: DOCX_MIME.into(),
        });
    }
    if let Some(kind) = infer::get(bytes) {
        let mime = kind.mime_type();
        if mime.starts_with("image/") {
            return Ok(DetectedContent { mime: mime.into() });
        }
        return Err(anyhow!("unsupported binary attachment type {mime}"));
    }
    let text = std::str::from_utf8(bytes)
        .context("document is neither supported binary nor UTF-8 text")?;
    if text.contains('\0') {
        return Err(anyhow!("plain-text document contains NUL bytes"));
    }
    let extension = Path::new(original_name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = if serde_json::from_str::<serde_json::Value>(text).is_ok() {
        "application/json"
    } else if extension == "csv" {
        "text/csv"
    } else if extension == "md" || extension == "markdown" {
        "text/markdown"
    } else {
        "text/plain"
    };
    Ok(DetectedContent { mime: mime.into() })
}

fn is_docx(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"PK") {
        return false;
    }
    ZipArchive::new(Cursor::new(bytes))
        .ok()
        .is_some_and(|mut archive| archive.by_name("word/document.xml").is_ok())
}

struct ExtractedDocument {
    text: String,
}

fn extract_document(bytes: &[u8], mime: &str, limit: usize) -> Result<ExtractedDocument> {
    let text = match mime {
        "application/pdf" => std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes))
            .map_err(|_| anyhow!("PDF extractor rejected malformed content"))??,
        DOCX_MIME => extract_docx(bytes, limit)?,
        "application/json" | "text/csv" | "text/markdown" | "text/plain" => {
            std::str::from_utf8(bytes)?.to_owned()
        }
        other => return Err(anyhow!("unsupported document type {other}")),
    };
    if text.chars().count() > limit {
        return Err(anyhow!("extracted document text exceeds configured limit"));
    }
    Ok(ExtractedDocument { text })
}

fn extract_docx(bytes: &[u8], limit: usize) -> Result<String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).context("open DOCX container")?;
    let document = archive
        .by_name("word/document.xml")
        .context("DOCX is missing word/document.xml")?;
    if document.size() > (limit.saturating_mul(8)) as u64 {
        return Err(anyhow!("DOCX XML exceeds safe extraction bound"));
    }
    let mut xml = String::new();
    document
        .take((limit.saturating_mul(8).saturating_add(1)) as u64)
        .read_to_string(&mut xml)?;
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(false);
    let mut output = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Text(text)) => {
                output.push_str(&text.decode().context("decode DOCX text")?);
            }
            Ok(Event::End(end))
                if matches!(end.local_name().as_ref(), b"p" | b"tr" | b"tab" | b"br") =>
            {
                output.push('\n');
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(anyhow!("malformed DOCX XML: {error}")),
        }
        if output.chars().count() > limit {
            return Err(anyhow!("DOCX extracted text exceeds configured limit"));
        }
    }
    Ok(output)
}

fn validate_image(bytes: &[u8], max_pixels: u64) -> Result<(u32, u32)> {
    let format = image::guess_format(bytes).context("unsupported image encoding")?;
    if !matches!(
        format,
        image::ImageFormat::Png
            | image::ImageFormat::Jpeg
            | image::ImageFormat::Gif
            | image::ImageFormat::WebP
    ) {
        return Err(anyhow!("unsupported image format {format:?}"));
    }
    let image = image::load_from_memory_with_format(bytes, format).context("decode image")?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 || u64::from(width).saturating_mul(u64::from(height)) > max_pixels
    {
        return Err(anyhow!(
            "image dimensions exceed the configured pixel limit"
        ));
    }
    Ok((width, height))
}

fn chunk_text(attachment_id: &str, text: &str, max_chars: usize) -> Vec<AttachmentChunkRecord> {
    let characters = text.chars().collect::<Vec<_>>();
    let overlap = (max_chars / 10).clamp(32, 512);
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < characters.len() {
        let mut end = (start + max_chars).min(characters.len());
        if end < characters.len() {
            let floor = (end.saturating_sub(max_chars / 4)).max(start + 1);
            if let Some(boundary) = (floor..end)
                .rev()
                .find(|index| characters[*index].is_whitespace())
            {
                end = boundary;
            }
        }
        let content = characters[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_owned();
        if !content.is_empty() {
            chunks.push(AttachmentChunkRecord {
                attachment_id: attachment_id.into(),
                chunk_no: chunks.len(),
                page_no: None,
                start_offset: Some(start),
                end_offset: Some(end),
                text: content,
            });
        }
        if end >= characters.len() {
            break;
        }
        start = end.saturating_sub(overlap).max(start + 1);
    }
    chunks
}

fn normalize_text(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .split("\n\n\n")
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_owned()
}

fn references_attachment(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    [
        "attachment received",
        "file tadi",
        "file ini",
        "gambar ini",
        "gambar tadi",
        "dokumen ini",
        "dokumen tadi",
        "foto ini",
        "screenshot",
        "this image",
        "this file",
        "this document",
        "attached",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn requested_ordinal(prompt: &str) -> Option<usize> {
    let lower = prompt.to_lowercase();
    [
        ("pertama", 0),
        ("first", 0),
        ("kedua", 1),
        ("second", 1),
        ("ketiga", 2),
        ("third", 2),
    ]
    .into_iter()
    .find_map(|(marker, value)| lower.contains(marker).then_some(value))
}

fn safe_filename(value: &str) -> String {
    let basename = Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment");
    let safe = basename
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| {
            if matches!(character, '/' | '\\' | ':' | '\0') {
                '_'
            } else {
                character
            }
        })
        .take(180)
        .collect::<String>();
    if safe.trim().is_empty() {
        "attachment".into()
    } else {
        safe
    }
}

fn short_hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..24].into()
}

fn bound(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_owned()
    } else {
        value.chars().take(max_chars).collect::<String>() + "…"
    }
}

fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn atomic_private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("attachment path has no parent"))?;
    create_private_dir(parent)?;
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4().simple()));
    fs::write(&temporary, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn text_pdf(text: &str) -> Vec<u8> {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        let stream = format!("BT /F1 14 Tf 72 760 Td ({escaped}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        ];
        let mut pdf = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }

    fn docx_with_macro_marker(text: &str) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        archive
            .start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        write!(
            archive,
            r#"<?xml version="1.0"?><w:document xmlns:w="urn:test"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#
        )
        .unwrap();
        archive
            .start_file(
                "word/vbaProject.bin",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(b"MACRO_MUST_NEVER_EXECUTE_OR_INDEX")
            .unwrap();
        archive.finish().unwrap().into_inner()
    }

    fn manager() -> (AttachmentManager, tempfile::TempDir, Arc<Storage>, String) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open_memory().unwrap());
        let session = storage
            .create_session("owner:test", "test", "custom", None, "m", false, None)
            .unwrap();
        let manager = AttachmentManager::new(
            storage.clone(),
            directory.path(),
            AttachmentConfig::default(),
        )
        .unwrap();
        (manager, directory, storage, session.id)
    }

    #[test]
    fn wrong_txt_extension_cannot_override_pdf_magic_and_empty_pdf_requires_ocr() {
        let (manager, _directory, _storage, session) = manager();
        let pdf = b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF".to_vec();
        let result = manager.ingest(AttachmentIngest {
            owner_id: "owner:test".into(),
            session_id: session,
            telegram_file_id: None,
            telegram_unique_id: None,
            original_name: "misleading.txt".into(),
            declared_mime: Some("text/plain".into()),
            expected_kind: AttachmentKind::Document,
            bytes: pdf,
        });
        // The minimal fixture may be rejected by the PDF parser, but it must
        // never be treated as text based on its extension.
        match result {
            Ok(record) => {
                assert_eq!(record.detected_mime, "application/pdf");
                assert_eq!(record.processing_status, "needs_ocr");
            }
            Err(error) => assert!(error.to_string().contains("PDF")),
        }
    }

    #[test]
    fn malicious_name_stays_inside_private_store_and_oversize_is_rejected() {
        let (mut manager, _directory, _storage, session) = manager();
        manager.config.max_document_bytes = 64 * 1024;
        assert!(manager
            .ingest(AttachmentIngest {
                owner_id: "owner:test".into(),
                session_id: session.clone(),
                telegram_file_id: None,
                telegram_unique_id: None,
                original_name: "../../escape.txt".into(),
                declared_mime: Some("text/plain".into()),
                expected_kind: AttachmentKind::Document,
                bytes: vec![b'x'; 64 * 1024 + 1],
            })
            .is_err());
        let record = manager
            .ingest(AttachmentIngest {
                owner_id: "owner:test".into(),
                session_id: session,
                telegram_file_id: None,
                telegram_unique_id: None,
                original_name: "../../escape.txt".into(),
                declared_mime: Some("text/plain".into()),
                expected_kind: AttachmentKind::Document,
                bytes: b"safe document content for path validation".to_vec(),
            })
            .unwrap();
        assert_eq!(record.original_name, "escape.txt");
        assert!(Path::new(&record.local_path).starts_with(manager.root()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&record.local_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn plain_document_chunks_are_searchable_and_retrieved_relevantly() {
        let (manager, _directory, storage, session) = manager();
        let record = manager
            .ingest(AttachmentIngest {
                owner_id: "owner:test".into(),
                session_id: session.clone(),
                telegram_file_id: None,
                telegram_unique_id: None,
                original_name: "notes.md".into(),
                declared_mime: Some("text/plain".into()),
                expected_kind: AttachmentKind::Document,
                bytes: format!(
                    "{}\nThe release codename is silver-orchid and verification is complete.\n{}",
                    "unrelated preface ".repeat(300),
                    "unrelated appendix ".repeat(300)
                )
                .into_bytes(),
            })
            .unwrap();
        assert_eq!(record.processing_status, "ready");
        let hits = storage
            .search_attachment_chunks("owner:test", &session, "silver orchid", 2)
            .unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].text.contains("silver-orchid"));
        assert!(hits.len() <= 2);
    }

    #[test]
    fn text_pdf_and_docx_extract_into_fts_without_macro_content() {
        let (manager, _directory, storage, session) = manager();
        let pdf = manager
            .ingest(AttachmentIngest {
                owner_id: "owner:test".into(),
                session_id: session.clone(),
                telegram_file_id: None,
                telegram_unique_id: Some("pdf-unique".into()),
                original_name: "release.pdf".into(),
                declared_mime: Some("application/pdf".into()),
                expected_kind: AttachmentKind::Document,
                bytes: text_pdf("The verified launch phrase is cobalt riverstone"),
            })
            .unwrap();
        assert_eq!(pdf.processing_status, "ready");
        assert!(storage
            .search_attachment_chunks("owner:test", &session, "cobalt riverstone", 2)
            .unwrap()
            .iter()
            .any(|chunk| chunk.text.contains("cobalt riverstone")));

        let docx = manager
            .ingest(AttachmentIngest {
                owner_id: "owner:test".into(),
                session_id: session.clone(),
                telegram_file_id: None,
                telegram_unique_id: Some("docx-unique".into()),
                original_name: "notes.docx".into(),
                declared_mime: Some(DOCX_MIME.into()),
                expected_kind: AttachmentKind::Document,
                bytes: docx_with_macro_marker("DOCX safe procedure alpine compass"),
            })
            .unwrap();
        assert_eq!(docx.processing_status, "ready");
        let chunks = storage
            .attachment_chunks("owner:test", &docx.attachment_id, 20)
            .unwrap();
        let indexed = chunks
            .iter()
            .map(|chunk| chunk.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(indexed.contains("alpine compass"));
        assert!(!indexed.contains("MACRO_MUST_NEVER"));
    }

    #[test]
    fn telegram_unique_id_makes_attachment_retry_idempotent() {
        let (manager, _directory, storage, session) = manager();
        let input = AttachmentIngest {
            owner_id: "owner:test".into(),
            session_id: session.clone(),
            telegram_file_id: Some("file-id".into()),
            telegram_unique_id: Some("stable-telegram-id".into()),
            original_name: "retry.txt".into(),
            declared_mime: Some("text/plain".into()),
            expected_kind: AttachmentKind::Document,
            bytes: b"Idempotent Telegram attachment retry content".to_vec(),
        };
        let first = manager.ingest(input.clone()).unwrap();
        let second = manager.ingest(input).unwrap();
        assert_eq!(first.attachment_id, second.attachment_id);
        assert_eq!(
            storage
                .recent_attachments("owner:test", &session, 10)
                .unwrap()
                .len(),
            1
        );
    }
}
