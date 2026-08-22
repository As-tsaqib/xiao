use crate::presentation::{Block, ProgressActivity, ProgressItem, ProgressState, RichText, View};
use serde_json::{json, Value};

const AI_ACTION_THINKING: &str = "5535034915403333642";
const AI_ACTION_ANALYZING: &str = "5535457114983497745";
const AI_ACTION_SEARCHING: &str = "5537511986251694100";
const AI_ACTION_FETCHING: &str = "5535365052359507996";
const AI_ACTION_TOOL: &str = "5535458420653555733";
const AI_ACTION_CODING: &str = "5537247356136718385";
const AI_ACTION_MEDIA: &str = "5537727026674270220";
const AI_ACTION_WRITING: &str = "5537203062138994712";

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
                vec![json!({"type":"thinking","text":progress_text(items)})]
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

fn progress_text(items: &[ProgressItem]) -> Value {
    let mut text = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            text.push(json!("\n"));
        }
        if item.state == ProgressState::Active {
            text.push(activity_icon(item.activity));
            text.push(json!(format!(" {}", item.label)));
        } else {
            text.push(json!(format!("{} {}", icon(&item.state), item.label)));
        }
    }
    Value::Array(text)
}

fn activity_icon(activity: ProgressActivity) -> Value {
    let (custom_emoji_id, alternative_text) = match activity {
        ProgressActivity::Thinking => (AI_ACTION_THINKING, "💭"),
        ProgressActivity::Analyzing => (AI_ACTION_ANALYZING, "🧠"),
        ProgressActivity::Searching => (AI_ACTION_SEARCHING, "🔎"),
        ProgressActivity::Fetching => (AI_ACTION_FETCHING, "🌐"),
        ProgressActivity::Tool => (AI_ACTION_TOOL, "⚙️"),
        ProgressActivity::Coding => (AI_ACTION_CODING, "💻"),
        ProgressActivity::Media => (AI_ACTION_MEDIA, "🖼️"),
        ProgressActivity::Writing => (AI_ACTION_WRITING, "✨"),
    };
    json!({
        "type": "custom_emoji",
        "custom_emoji_id": custom_emoji_id,
        "alternative_text": alternative_text,
    })
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

    #[test]
    fn active_progress_uses_the_official_ai_actions_emoji() {
        let view = View {
            title: None,
            blocks: vec![Block::Progress {
                items: vec![ProgressItem {
                    state: ProgressState::Active,
                    activity: ProgressActivity::Searching,
                    label: "Searching the web".into(),
                }],
            }],
            actions: vec![],
            side_mode: false,
        };
        let rendered = render(&view, true);
        assert_eq!(rendered["blocks"][0]["type"], "thinking");
        assert_eq!(
            rendered["blocks"][0]["text"][0]["custom_emoji_id"],
            AI_ACTION_SEARCHING
        );
        assert_eq!(rendered["blocks"][0]["text"][0]["alternative_text"], "🔎");
        assert_eq!(rendered["blocks"][0]["text"][1], " Searching the web");
    }

    #[test]
    fn completed_progress_is_quiet_and_not_animated() {
        let view = View {
            title: None,
            blocks: vec![Block::Progress {
                items: vec![ProgressItem {
                    state: ProgressState::Done,
                    activity: ProgressActivity::Fetching,
                    label: "Fetched the page".into(),
                }],
            }],
            actions: vec![],
            side_mode: false,
        };
        let rendered = render(&view, true);
        assert_eq!(rendered["blocks"][0]["text"], json!(["✓ Fetched the page"]));
    }
}
