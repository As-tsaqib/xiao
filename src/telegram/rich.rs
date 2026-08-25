use std::collections::HashMap;

use crate::presentation::{Block, ProgressIcon, ProgressItem, ProgressState, ProgressActivity, RichText, View};
use serde_json::{json, Value};

const AI_ACTION_THINKING: &str = "5535034915403333642";
const AI_ACTION_ANALYZING: &str = "5535457114983497745";
const AI_ACTION_SEARCHING: &str = "5537511986251694100";
const AI_ACTION_FETCHING: &str = "5535365052359507996";
const AI_ACTION_TOOL: &str = "5535458420653555733";
const AI_ACTION_CODING: &str = "5537247356136718385";
const AI_ACTION_MEDIA: &str = "5537727026674270220";
const AI_ACTION_WRITING: &str = "5537203062138994712";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramEmoji {
    pub custom_emoji_id: Option<String>,
    pub fallback: &'static str,
}

#[derive(Debug, Clone)]
pub struct TelegramEmojiRegistry {
    entries: HashMap<ProgressIcon, TelegramEmoji>,
}

impl Default for TelegramEmojiRegistry {
    fn default() -> Self {
        let mut entries = HashMap::new();
        for (icon, id, fallback) in [
            (ProgressIcon::Thinking, None, "💭"),
            (ProgressIcon::Analyzing, Some(AI_ACTION_ANALYZING), "🧠"),
            (ProgressIcon::WebSearch, Some(AI_ACTION_SEARCHING), "🔎"),
            (ProgressIcon::FileSearch, Some(AI_ACTION_SEARCHING), "📁"),
            (ProgressIcon::Fetching, Some(AI_ACTION_FETCHING), "🌐"),
            (ProgressIcon::DocumentRead, Some(AI_ACTION_ANALYZING), "📄"),
            (ProgressIcon::ImageInspect, Some(AI_ACTION_MEDIA), "🖼️"),
            (ProgressIcon::Terminal, Some(AI_ACTION_TOOL), "⌘"),
            (ProgressIcon::Coding, Some(AI_ACTION_CODING), "💻"),
            (ProgressIcon::Editing, Some(AI_ACTION_CODING), "✎"),
            (ProgressIcon::Installing, Some(AI_ACTION_TOOL), "📦"),
            (ProgressIcon::Testing, Some(AI_ACTION_TOOL), "🧪"),
            (ProgressIcon::Tool, Some(AI_ACTION_TOOL), "⚙️"),
            (ProgressIcon::Writing, Some(AI_ACTION_WRITING), "✨"),
            (ProgressIcon::Audio, Some(AI_ACTION_MEDIA), "🔊"),
            (ProgressIcon::Video, Some(AI_ACTION_MEDIA), "🎬"),
        ] {
            entries.insert(
                icon,
                TelegramEmoji {
                    custom_emoji_id: id.map(str::to_owned),
                    fallback,
                },
            );
        }
        Self { entries }
    }
}

impl TelegramEmojiRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, icon: ProgressIcon, custom_emoji_id: Option<&str>, validated: bool) {
        self.set_verified_custom_emoji(icon, custom_emoji_id, validated);
    }

    pub fn set_verified_custom_emoji(
        &mut self,
        icon: ProgressIcon,
        custom_emoji_id: Option<&str>,
        validated: bool,
    ) {
        let fallback = self
            .entries
            .get(&icon)
            .map(|entry| entry.fallback)
            .unwrap_or("•");
        let custom_emoji_id = custom_emoji_id
            .map(str::trim)
            .filter(|id| validated && !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
            .map(str::to_owned);
        self.entries.insert(
            icon,
            TelegramEmoji {
                custom_emoji_id,
                fallback,
            },
        );
    }

    fn get(&self, icon: ProgressIcon) -> TelegramEmoji {
        self.entries.get(&icon).cloned().unwrap_or(TelegramEmoji {
            custom_emoji_id: None,
            fallback: "•",
        })
    }
}

pub fn render(view: &View, draft: bool) -> Value {
    render_with_registry(view, draft, &TelegramEmojiRegistry::default())
}

pub fn render_with_registry(view: &View, draft: bool, registry: &TelegramEmojiRegistry) -> Value {
    let mut blocks = Vec::new();
    if view.side_mode {
        blocks.push(json!({"type":"heading","text":"SIDE CHAT SESSION","size":3}));
    }
    if let Some(title) = &view.title {
        blocks.push(json!({"type":"heading","text":title,"size":2}));
    }
    for block in &view.blocks {
        blocks.extend(render_block(block, draft, registry));
    }
    json!({ "blocks": blocks })
}
fn render_block(block: &Block, draft: bool, registry: &TelegramEmojiRegistry) -> Vec<Value> {
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
            json!({"type":"details","summary":title,"blocks":blocks.iter().flat_map(|b|render_block(b,draft,registry)).collect::<Vec<_>>()}),
        ],
        Block::Progress { items } => {
            if draft {
                vec![json!({"type":"thinking","text":progress_text(items, registry)})]
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

fn progress_text(items: &[ProgressItem], registry: &TelegramEmojiRegistry) -> Value {
    let mut text = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            text.push(json!("\n"));
        }
        if item.state == ProgressState::Active {
            text.push(activity_icon(item.icon, registry));
            text.push(json!(format!(" {}", item.label)));
        } else {
            text.push(json!(format!("{} {}", icon(&item.state), item.label)));
        }
    }
    Value::Array(text)
}

fn activity_icon(icon: ProgressIcon, registry: &TelegramEmojiRegistry) -> Value {
    let emoji = registry.get(icon);
    if let Some(custom_emoji_id) = emoji.custom_emoji_id {
        json!({
            "type": "custom_emoji",
            "custom_emoji_id": custom_emoji_id,
            "alternative_text": emoji.fallback,
        })
    } else {
        json!(emoji.fallback)
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
    use crate::presentation::ProgressActivity;

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
                    id: 1,
                    state: ProgressState::Active,
                    activity: ProgressActivity::Searching,
                    icon: ProgressIcon::WebSearch,
                    action_key: Some("web_search".into()),
                    correlation_id: None,
                    summary: None,
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
                    id: 1,
                    state: ProgressState::Done,
                    activity: ProgressActivity::Fetching,
                    icon: ProgressIcon::Fetching,
                    action_key: Some("web_fetch".into()),
                    correlation_id: None,
                    summary: Some("completed".into()),
                    label: "Fetched the page".into(),
                }],
            }],
            actions: vec![],
            side_mode: false,
        };
        let rendered = render(&view, true);
        assert_eq!(rendered["blocks"][0]["text"], json!(["✓ Fetched the page"]));
    }

    #[test]
    fn invalid_custom_emoji_id_falls_back_to_unicode_without_broken_draft() {
        let mut registry = TelegramEmojiRegistry::new();
        registry.set_verified_custom_emoji(ProgressIcon::WebSearch, Some("not-verified"), true);
        let view = View {
            title: None,
            blocks: vec![Block::Progress {
                items: vec![ProgressItem {
                    id: 1,
                    state: ProgressState::Active,
                    activity: ProgressActivity::Searching,
                    icon: ProgressIcon::WebSearch,
                    action_key: Some("web_search".into()),
                    correlation_id: Some("call-1".into()),
                    summary: None,
                    label: "Searching the web".into(),
                }],
            }],
            actions: vec![],
            side_mode: false,
        };
        let rendered = render_with_registry(&view, true, &registry);
        assert_eq!(rendered["blocks"][0]["text"][0], "🔎");
        assert!(!rendered.to_string().contains("not-verified"));

        registry.set_verified_custom_emoji(
            ProgressIcon::WebSearch,
            Some("5537511986251694100"),
            false,
        );
        let rendered = render_with_registry(&view, true, &registry);
        assert_eq!(rendered["blocks"][0]["text"][0], "🔎");
    }
}

#[test]
fn thinking_emoji_defaults_to_unicode_fallback() {
    let registry = TelegramEmojiRegistry::default();
    let emoji = registry.get(ProgressIcon::Thinking);
    assert_eq!(emoji.custom_emoji_id, None);
    assert_eq!(emoji.fallback, "💭");

    let view = View {
        title: None,
        blocks: vec![Block::Progress {
            items: vec![ProgressItem {
                id: 1,
                state: ProgressState::Active,
                activity: ProgressActivity::Thinking,
                icon: ProgressIcon::Thinking,
                action_key: None,
                correlation_id: None,
                summary: None,
                label: "Thinking".into(),
            }],
        }],
        actions: vec![],
        side_mode: false,
    };
    let rendered = render(&view, true);
    assert_eq!(rendered["blocks"][0]["type"], "thinking");
    assert_eq!(rendered["blocks"][0]["text"][0], "💭");
}
