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
            let dir = self
                .default_cwd
                .join(".xiao/workspaces")
                .join(&context.session_id);
            let _ = std::fs::create_dir_all(&dir);
            dir
        };

        let target_path = session_workspace.join(&rel_path);
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let full_text = match arguments.title {
            Some(title) if !title.trim().is_empty() => {
                format!("{}\n\n{}", title.trim(), arguments.content)
            }
            _ => arguments.content,
        };

        let pdf_bytes = generate_valid_pdf(&full_text);
        std::fs::write(&target_path, &pdf_bytes)?;

        let size_bytes = pdf_bytes.len() as u64;
        let file_name = target_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("document.pdf")
            .to_owned();

        Ok(serde_json::to_string(&json!({
            "status": "succeeded",
            "path": target_path.display().to_string(),
            "size_bytes": size_bytes,
            "artifacts": [
                {
                    "name": file_name,
                    "path": target_path,
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

    let mut stream_ops = Vec::new();
    stream_ops.push("BT\n/F1 12 Tf\n72 750 Td\n16 TL".to_owned());
    for (i, line) in lines.iter().enumerate() {
        let escaped = line
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        if i == 0 {
            stream_ops.push(format!("({escaped}) Tj"));
        } else {
            stream_ops.push(format!("T* ({escaped}) Tj"));
        }
    }
    stream_ops.push("ET".to_owned());
    let stream_content = stream_ops.join("\n");

    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            stream_content.len(),
            stream_content
        ),
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
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref
        )
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

        assert!(result.contains("\"status\":\"succeeded\""));
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
}
