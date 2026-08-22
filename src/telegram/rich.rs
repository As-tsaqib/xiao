use crate::presentation::{Block, ProgressState, RichText, View};
use serde_json::{json, Value};

pub fn render(view: &View, draft: bool) -> Value {
    let mut blocks = Vec::new();
    if view.side_mode {
        blocks.push(json!({"type":"heading","text":"SIDE CHAT SESSION","size":3}));
    }
    if let Some(title) = &view.title {
        blocks.push(json!({"type":"heading","text":title,"size":2}));
    }
    for block in &view.blocks {
        blocks.extend(render_block(block, draft));
    }
    json!({"blocks":blocks})
}
fn render_block(block: &Block, draft: bool) -> Vec<Value> {
    match block {
        Block::Heading { level, text } => {
            vec![json!({"type":"heading","text":text,"size":(*level).clamp(1,6)})]
        }
        Block::Paragraph { text } => vec![json!({"type":"paragraph","text":text})],
        Block::RichHeading { level, content } => {
            vec![json!({"type":"heading","text":rich_text(content),"size":(*level).clamp(1,6)})]
        }
        Block::RichParagraph { content } => {
            vec![json!({"type":"paragraph","text":rich_text(content)})]
        }
        Block::Code { language, content } => {
            let mut v = json!({"type":"pre","text":content});
            if let Some(l) = language {
                v["language"] = json!(l);
            }
            vec![v]
        }
        Block::Table { headers, rows } => {
            let mut cells = Vec::new();
            cells.push(headers.iter().map(|x| cell(x, true)).collect::<Vec<_>>());
            for row in rows {
                cells.push(row.iter().map(|x| cell(x, false)).collect())
            }
            vec![json!({"type":"table","cells":cells,"is_bordered":true,"is_striped":true})]
        }
        Block::RichTable { headers, rows } => {
            let mut cells = Vec::new();
            cells.push(
                headers
                    .iter()
                    .map(|x| rich_cell(x, true))
                    .collect::<Vec<_>>(),
            );
            for row in rows {
                cells.push(row.iter().map(|x| rich_cell(x, false)).collect())
            }
            vec![json!({"type":"table","cells":cells,"is_bordered":true,"is_striped":true})]
        }
        Block::Quote { text } => {
            vec![json!({"type":"blockquote","blocks":[{"type":"paragraph","text":text}]})]
        }
        Block::RichQuote { content } => vec![
            json!({"type":"blockquote","blocks":[{"type":"paragraph","text":rich_text(content)}]}),
        ],
        Block::List { ordered, items } => vec![
            json!({"type":"list","items":items.iter().map(|x|{if *ordered{json!({"blocks":[{"type":"paragraph","text":x}],"type":"1"})}else{json!({"blocks":[{"type":"paragraph","text":x}]})}}).collect::<Vec<_>>()}),
        ],
        Block::RichList { ordered, items } => vec![
            json!({"type":"list","items":items.iter().map(|x|{if *ordered{json!({"blocks":[{"type":"paragraph","text":rich_text(x)}],"type":"1"})}else{json!({"blocks":[{"type":"paragraph","text":rich_text(x)}]})}}).collect::<Vec<_>>()}),
        ],
        Block::Details { title, blocks } => vec![
            json!({"type":"details","summary":title,"blocks":blocks.iter().flat_map(|b|render_block(b,draft)).collect::<Vec<_>>()}),
        ],
        Block::Progress { items } => {
            if draft {
                vec![
                    json!({"type":"thinking","text":items.iter().map(|i|format!("{} {}",icon(&i.state),i.label)).collect::<Vec<_>>().join("\n")}),
                ]
            } else {
                vec![]
            }
        }
        Block::Divider => vec![json!({"type":"divider"})],
    }
}
fn cell(text: &str, header: bool) -> Value {
    if header {
        json!({"text":text,"is_header":true,"align":"left","valign":"middle"})
    } else {
        json!({"text":text,"align":"left","valign":"middle"})
    }
}
fn rich_cell(text: &[RichText], header: bool) -> Value {
    if header {
        json!({"text":rich_text(text),"is_header":true,"align":"left","valign":"middle"})
    } else {
        json!({"text":rich_text(text),"align":"left","valign":"middle"})
    }
}
fn rich_text(items: &[RichText]) -> Value {
    let values = items
        .iter()
        .map(|item| match item {
            RichText::Text { text } => json!(text),
            RichText::Bold { content } => json!({"type":"bold","text":rich_text(content)}),
            RichText::Italic { content } => json!({"type":"italic","text":rich_text(content)}),
            RichText::Code { text } => json!({"type":"code","text":text}),
            RichText::Link { content, url } => {
                json!({"type":"url","text":rich_text(content),"url":url})
            }
        })
        .collect::<Vec<_>>();
    if values.len() == 1 {
        values.into_iter().next().unwrap()
    } else {
        Value::Array(values)
    }
}
fn icon(s: &ProgressState) -> &'static str {
    match s {
        ProgressState::Pending => "○",
        ProgressState::Active => "◉",
        ProgressState::Done => "✓",
        ProgressState::Failed => "✗",
    }
}

pub fn plain(view: &View) -> String {
    let mut out = Vec::new();
    if view.side_mode {
        out.push("SIDE CHAT SESSION".into());
    }
    if let Some(t) = &view.title {
        out.push(t.clone());
    }
    for b in &view.blocks {
        match b {
            Block::Heading { text, .. } | Block::Paragraph { text } | Block::Quote { text } => {
                out.push(text.clone())
            }
            Block::RichHeading { content, .. }
            | Block::RichParagraph { content }
            | Block::RichQuote { content } => {
                out.push(content.iter().map(RichText::plain).collect())
            }
            Block::Code { content, .. } => out.push(content.clone()),
            Block::Table { headers, rows } => {
                out.push(headers.join(" | "));
                for r in rows {
                    out.push(r.join(" | "));
                }
            }
            Block::RichTable { headers, rows } => {
                out.push(
                    headers
                        .iter()
                        .map(|c| c.iter().map(RichText::plain).collect::<String>())
                        .collect::<Vec<_>>()
                        .join(" | "),
                );
                for r in rows {
                    out.push(
                        r.iter()
                            .map(|c| c.iter().map(RichText::plain).collect::<String>())
                            .collect::<Vec<_>>()
                            .join(" | "),
                    );
                }
            }
            Block::List { ordered, items } => {
                for (i, x) in items.iter().enumerate() {
                    out.push(if *ordered {
                        format!("{}. {x}", i + 1)
                    } else {
                        format!("• {x}")
                    })
                }
            }
            Block::RichList { ordered, items } => {
                for (i, x) in items.iter().enumerate() {
                    let t = x.iter().map(RichText::plain).collect::<String>();
                    out.push(if *ordered {
                        format!("{}. {t}", i + 1)
                    } else {
                        format!("• {t}")
                    })
                }
            }
            Block::Details { title, blocks } => {
                out.push(title.clone());
                for x in blocks {
                    out.push(plain(&View {
                        title: None,
                        blocks: vec![x.clone()],
                        actions: vec![],
                        side_mode: false,
                    }));
                }
            }
            Block::Progress { .. } => {}
            Block::Divider => out.push("────────".into()),
        }
    }
    out.join("\n\n")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn side_marker_is_native_heading() {
        let mut v = View::info("X", "Y");
        v.side_mode = true;
        let j = render(&v, false);
        assert_eq!(j["blocks"][0]["text"], "SIDE CHAT SESSION");
    }
    #[test]
    fn markdown_final_becomes_native_rich_blocks() {
        let v=View::from_markdown("## Result\n\n| File | Status |\n|---|---|\n| `auth.rs` | **Fixed** |\n\n```rust\nfn main() {}\n```\n\n- one\n- two\n\n> quote",false);
        let j = render(&v, false);
        let blocks = j["blocks"].as_array().unwrap();
        assert!(blocks.iter().any(|b| b["type"] == "heading"));
        assert!(blocks.iter().any(|b| b["type"] == "table"));
        assert!(blocks
            .iter()
            .any(|b| b["type"] == "pre" && b["language"] == "rust"));
        assert!(blocks.iter().any(|b| b["type"] == "list"));
        assert!(blocks.iter().any(|b| b["type"] == "blockquote"));
        assert!(!j.to_string().contains("thinking"));
    }
}
