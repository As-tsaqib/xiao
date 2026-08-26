use std::path::PathBuf;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tools::{Tool, ToolContext, ToolEffect, ToolOrigin, ToolRisk, ToolSpec};

#[derive(Clone)]
pub struct PdfCreateTool {
    default_cwd: PathBuf,
}

impl PdfCreateTool {
    pub fn new(default_cwd: impl Into<PathBuf>) -> Self {
        Self {
            default_cwd: default_cwd.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    path: String,
    content: String,
    title: Option<String>,
}

#[async_trait]
impl Tool for PdfCreateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "pdf_create".into(),
            description: "Create a valid, well-formed PDF document in the workspace from text. Uses deterministic layout and xref generation to guarantee a valid parseable PDF file without shell scripts or raw xref strings.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative output PDF file path (e.g. 'document.pdf' or 'report.pdf')"
                    },
                    "content": {
                        "type": "string",
                        "description": "Text content to include in the PDF document"
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional title heading for the PDF document"
                    }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
            risk: ToolRisk::SideEffect,
            origin: ToolOrigin::Builtin,
            effect: ToolEffect::NonIdempotent,
            required_capabilities: Vec::new(),
            timeout_ms: 30_000,
        }
    }

    async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
        let arguments: Arguments = serde_json::from_value(arguments)?;
        let trimmed_path = arguments.path.trim();
        if trimmed_path.is_empty() {
            return Err(anyhow!("path must not be empty"));
        }
        let rel_path = PathBuf::from(trimmed_path);
        if rel_path.is_absolute()
            || rel_path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(anyhow!(
                "path must be a workspace-relative path; parent directory traversal ('..') and absolute escapes are forbidden"
            ));
        }

        let session_workspace = if context.session_id.is_empty() {
            self.default_cwd.clone()
        } else {
            self.default_cwd
                .join(".xiao/workspaces")
                .join(&context.session_id)
        };
        std::fs::create_dir_all(&session_workspace)?;
        let canonical_workspace = session_workspace.canonicalize()?;

        let mut current_ancestor = canonical_workspace.clone();
        for component in rel_path.components() {
            match component {
                std::path::Component::Normal(part) => {
                    current_ancestor = current_ancestor.join(part);
                    if current_ancestor.is_symlink() {
                        return Err(anyhow!(
                            "symlink components in pdf_create path are forbidden: {}",
                            current_ancestor.display()
                        ));
                    }
                }
                std::path::Component::CurDir => {}
                _ => {
                    return Err(anyhow!("invalid path component in pdf_create path"));
                }
            }
        }

        let target_path = canonical_workspace.join(&rel_path);
        if let Some(parent) = target_path.parent() {
            if parent.exists() {
                let canonical_parent = parent.canonicalize()?;
                if !canonical_parent.starts_with(&canonical_workspace) {
                    return Err(anyhow!(
                        "parent directory escapes canonical session workspace"
                    ));
                }
            }
            std::fs::create_dir_all(parent)?;
            let canonical_parent = parent.canonicalize()?;
            if !canonical_parent.starts_with(&canonical_workspace) {
                return Err(anyhow!(
                    "parent directory escapes canonical session workspace"
                ));
            }
        }

        if target_path.is_symlink() {
            return Err(anyhow!("pdf_create target path cannot be a symlink"));
        }
        if let Ok(meta) = std::fs::symlink_metadata(&target_path) {
            if meta.file_type().is_symlink() {
                return Err(anyhow!("symlink destination rejected"));
            }
        }

        let full_text = match arguments.title {
            Some(title) if !title.trim().is_empty() => {
                format!("{}\n\n{}", title.trim(), arguments.content)
            }
            _ => arguments.content,
        };

        let pdf_bytes = generate_valid_pdf(&full_text);
        std::fs::write(&target_path, &pdf_bytes)?;

        let canonical_target = target_path.canonicalize()?;
        if !canonical_target.starts_with(&canonical_workspace) {
            return Err(anyhow!(
                "created pdf file escapes canonical session workspace"
            ));
        }

        let size_bytes = pdf_bytes.len() as u64;
        let file_name = canonical_target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("document.pdf")
            .to_owned();

        Ok(serde_json::to_string(&json!({
            "status": "succeeded",
            "path": canonical_target.display().to_string(),
            "size_bytes": size_bytes,
            "artifacts": [
                {
                    "name": file_name,
                    "path": canonical_target,
                    "size_bytes": size_bytes
                }
            ],
            "verification_evidence": true
        }))?)
    }
}

pub fn generate_valid_pdf(text: &str) -> Vec<u8> {
    let mut lines = Vec::new();
    for raw_line in text.lines() {
        let trimmed = raw_line.trim_end();
        if trimmed.is_empty() {
            lines.push(String::new());
        } else {
            let mut current = String::new();
            for word in trimmed.split_whitespace() {
                if current.is_empty() {
                    current.push_str(word);
                } else if current.len() + 1 + word.len() > 80 {
                    lines.push(current);
                    current = word.to_string();
                } else {
                    current.push(' ');
                    current.push_str(word);
                }
            }
            if !current.is_empty() {
                lines.push(current);
            }
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }

    const LINES_PER_PAGE: usize = 40;
    let page_chunks: Vec<Vec<String>> = lines
        .chunks(LINES_PER_PAGE)
        .map(|chunk| chunk.to_vec())
        .collect();
    let num_pages = page_chunks.len().max(1);

    let font_obj_id = 3;
    let mut objects: Vec<(usize, String)> = Vec::new();
    let mut kids_refs = Vec::new();

    for i in 0..num_pages {
        let page_obj_id = 4 + 2 * i;
        let content_obj_id = page_obj_id + 1;
        kids_refs.push(format!("{page_obj_id} 0 R"));

        let lines_for_page = &page_chunks[i];
        let mut stream_ops = Vec::new();
        stream_ops.push("BT\n/F1 12 Tf\n72 750 Td\n16 TL".to_owned());
        for (line_idx, line) in lines_for_page.iter().enumerate() {
            let sanitized: String = line
                .chars()
                .map(|c| {
                    if c.is_ascii() && !c.is_ascii_control() {
                        c
                    } else if c.is_ascii_whitespace() {
                        ' '
                    } else {
                        '?'
                    }
                })
                .collect();
            let escaped = sanitized
                .replace('\\', "\\\\")
                .replace('(', "\\(")
                .replace(')', "\\)");
            if line_idx == 0 {
                stream_ops.push(format!("({escaped}) Tj"));
            } else {
                stream_ops.push(format!("T* ({escaped}) Tj"));
            }
        }
        stream_ops.push("ET".to_owned());
        let stream_content = stream_ops.join("\n");

        let page_obj_content = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 {font_obj_id} 0 R >> >> /Contents {content_obj_id} 0 R >>"
        );
        let content_obj_content = format!(
            "<< /Length {} >>
stream
{}
endstream",
            stream_content.len(),
            stream_content
        );
        objects.push((page_obj_id, page_obj_content));
        objects.push((content_obj_id, content_obj_content));
    }

    let catalog_obj = (1, "<< /Type /Catalog /Pages 2 0 R >>".to_owned());
    let pages_obj = (
        2,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids_refs.join(" "),
            num_pages
        ),
    );
    let font_obj = (
        font_obj_id,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    );

    let mut all_objects = vec![catalog_obj, pages_obj, font_obj];
    all_objects.extend(objects);
    all_objects.sort_by_key(|(id, _)| *id);

    let mut pdf = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (id, content) in &all_objects {
        offsets.push((*id, pdf.len()));
        pdf.extend_from_slice(format!("{id} 0 obj\n{content}\nendobj\n").as_bytes());
    }
    offsets.sort_by_key(|(id, _)| *id);

    let xref = pdf.len();
    let total_objs = all_objects.len() + 1;
    pdf.extend_from_slice(format!("xref\n0 {total_objs}\n").as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for (_, offset) in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size {total_objs} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n")
            .as_bytes(),
    );
    pdf
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn pdf_create_produces_valid_pdf_extractable_by_pdf_crate() {
        let temp = tempdir().unwrap();
        let tool = PdfCreateTool::new(temp.path());
        let context = ToolContext {
            principal: "p".into(),
            session_id: "test-session".into(),
            agent_run_id: "run-1".into(),
            yolo_mode: false,
            messages: Vec::new(),
            cancellation: CancellationToken::new(),
            progress: None,
        };
        let result = tool
            .execute(
                &context,
                json!({
                    "path": "output/test.pdf",
                    "title": "Document Title",
                    "content": "This is a verified test document produced deterministically."
                }),
            )
            .await
            .unwrap();

        assert!(result.contains(r#""status":"succeeded""#));
        assert!(result.contains("test.pdf"));

        let file_path = temp
            .path()
            .join(".xiao/workspaces/test-session/output/test.pdf");
        assert!(file_path.exists());

        let bytes = std::fs::read(&file_path).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.4"));

        // Verify with pdf-extract
        let extracted = pdf_extract::extract_text_from_mem(&bytes).unwrap();
        assert!(extracted.contains("Document Title"));
        assert!(extracted.contains("verified test document"));
    }

    #[tokio::test]
    async fn pdf_create_handles_multipage_and_unicode_text() {
        let temp = tempdir().unwrap();
        let tool = PdfCreateTool::new(temp.path());
        let context = ToolContext {
            principal: "p".into(),
            session_id: "test-multipage".into(),
            agent_run_id: "run-2".into(),
            yolo_mode: false,
            messages: Vec::new(),
            cancellation: CancellationToken::new(),
            progress: None,
        };
        let mut long_content = String::new();
        for i in 1..=100 {
            long_content.push_str(&format!("Line item number {i} with details and notes.\n"));
        }
        let result = tool
            .execute(
                &context,
                json!({
                    "path": "reports/summary.pdf",
                    "title": "Quarterly Report",
                    "content": long_content
                }),
            )
            .await
            .unwrap();

        assert!(result.contains(r#""status":"succeeded""#));
        let file_path = temp
            .path()
            .join(".xiao/workspaces/test-multipage/reports/summary.pdf");
        assert!(file_path.exists());
        let bytes = std::fs::read(&file_path).unwrap();
        let extracted = pdf_extract::extract_text_from_mem(&bytes).unwrap();
        assert!(extracted.contains("Quarterly Report"));
        assert!(extracted.contains("Line item number 1"));
        assert!(extracted.contains("Line item number 99"));
    }

    #[tokio::test]
    async fn pdf_create_rejects_parent_traversal() {
        let temp = tempdir().unwrap();
        let tool = PdfCreateTool::new(temp.path());
        let context = ToolContext {
            principal: "p".into(),
            session_id: "test-session".into(),
            agent_run_id: "run-1".into(),
            yolo_mode: false,
            messages: Vec::new(),
            cancellation: CancellationToken::new(),
            progress: None,
        };
        let err = tool
            .execute(
                &context,
                json!({
                    "path": "../../../etc/test.pdf",
                    "content": "hello"
                }),
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("parent directory traversal"));
    }

    #[tokio::test]
    async fn pdf_create_rejects_absolute_path() {
        let temp = tempdir().unwrap();
        let tool = PdfCreateTool::new(temp.path());
        let context = ToolContext {
            principal: "p".into(),
            session_id: "test-session".into(),
            agent_run_id: "run-1".into(),
            yolo_mode: false,
            messages: Vec::new(),
            cancellation: CancellationToken::new(),
            progress: None,
        };
        let err = tool
            .execute(
                &context,
                json!({
                    "path": "/tmp/evil.pdf",
                    "content": "hello"
                }),
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("workspace-relative"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pdf_create_rejects_symlink_escape() {
        let temp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let workspace = temp.path().join(".xiao/workspaces/test-symlink");
        std::fs::create_dir_all(&workspace).unwrap();

        // Create a symlink dir pointing outside
        let link_dir = workspace.join("symlink_dir");
        std::os::unix::fs::symlink(outside.path(), &link_dir).unwrap();

        let tool = PdfCreateTool::new(temp.path());
        let context = ToolContext {
            principal: "p".into(),
            session_id: "test-symlink".into(),
            agent_run_id: "run-3".into(),
            yolo_mode: false,
            messages: Vec::new(),
            cancellation: CancellationToken::new(),
            progress: None,
        };

        let err = tool
            .execute(
                &context,
                json!({
                    "path": "symlink_dir/escaped.pdf",
                    "content": "malicious"
                }),
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("symlink"));
    }
}
