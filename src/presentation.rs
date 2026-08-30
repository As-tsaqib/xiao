use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph {
        text: String,
    },
    RichHeading {
        level: u8,
        content: Vec<RichText>,
    },
    RichParagraph {
        content: Vec<RichText>,
    },
    Code {
        language: Option<String>,
        content: String,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    RichTable {
        headers: Vec<Vec<RichText>>,
        rows: Vec<Vec<Vec<RichText>>>,
    },
    Quote {
        text: String,
    },
    RichQuote {
        content: Vec<RichText>,
    },
    List {
        ordered: bool,
        items: Vec<String>,
    },
    RichList {
        ordered: bool,
        items: Vec<Vec<RichText>>,
    },
    Details {
        title: String,
        blocks: Vec<Block>,
    },
    Progress {
        items: Vec<ProgressItem>,
    },
    Divider,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RichText {
    Text { text: String },
    Bold { content: Vec<RichText> },
    Italic { content: Vec<RichText> },
    Code { text: String },
    Link { content: Vec<RichText>, url: String },
}

impl RichText {
    pub fn plain(&self) -> String {
        match self {
            Self::Text { text } | Self::Code { text } => text.clone(),
            Self::Bold { content } | Self::Italic { content } => {
                content.iter().map(Self::plain).collect()
            }
            Self::Link { content, .. } => content.iter().map(Self::plain).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgressItem {
    /// Stable only within one live execution timeline. Adapters must not use
    /// Telegram message/custom-emoji identifiers as domain identity.
    #[serde(default)]
    pub id: u64,
    pub state: ProgressState,
    #[serde(default)]
    pub activity: ProgressActivity,
    /// Semantic icon/action classification; presentation adapters map this
    /// to their own visual vocabulary.
    #[serde(default)]
    pub icon: ProgressIcon,
    /// Presentation-safe action key, useful to correlate updates without
    /// exposing provider reasoning or tool arguments.
    #[serde(default)]
    pub action_key: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProgressIcon {
    #[default]
    Thinking,
    Analyzing,
    WebSearch,
    FileSearch,
    Fetching,
    DocumentRead,
    ImageInspect,
    Terminal,
    Coding,
    Editing,
    Installing,
    Testing,
    Tool,
    Writing,
    Audio,
    Video,
}

impl ProgressIcon {
    pub const fn from_activity(activity: ProgressActivity) -> Self {
        match activity {
            ProgressActivity::Thinking => Self::Thinking,
            ProgressActivity::Analyzing => Self::Analyzing,
            ProgressActivity::Searching => Self::WebSearch,
            ProgressActivity::Fetching => Self::Fetching,
            ProgressActivity::Tool => Self::Tool,
            ProgressActivity::Coding => Self::Coding,
            ProgressActivity::Media => Self::ImageInspect,
            ProgressActivity::Writing => Self::Writing,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProgressActivity {
    #[default]
    Thinking,
    Analyzing,
    Searching,
    Fetching,
    Tool,
    Coding,
    Media,
    Writing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProgressState {
    Pending,
    Active,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ActionTarget {
    Command(String),
    Url(String),
    Back,
    Close,
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Action {
    pub label: String,
    pub target: ActionTarget,
}

impl Action {
    pub fn command(label: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            target: ActionTarget::Command(command.into()),
        }
    }
    pub fn url(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            target: ActionTarget::Url(url.into()),
        }
    }
    pub fn back() -> Self {
        Self {
            label: "Back".into(),
            target: ActionTarget::Back,
        }
    }
    pub fn close() -> Self {
        Self {
            label: "Close".into(),
            target: ActionTarget::Close,
        }
    }
    pub fn noop(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            target: ActionTarget::Noop,
        }
    }
    pub fn callback_command(&self) -> Option<&str> {
        match &self.target {
            ActionTarget::Command(v) => Some(v.as_str()),
            ActionTarget::Back => Some("back"),
            ActionTarget::Close => Some("close"),
            ActionTarget::Noop => Some("noop"),
            ActionTarget::Url(_) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct View {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
    pub actions: Vec<Vec<Action>>,
    #[serde(default)]
    pub side_mode: bool,
}

impl View {
    pub fn info(title: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            blocks: vec![Block::Paragraph { text: text.into() }],
            actions: vec![],
            side_mode: false,
        }
    }

    pub fn from_markdown(text: &str, side_mode: bool) -> Self {
        Self {
            title: None,
            blocks: parse_markdown(text),
            actions: vec![],
            side_mode,
        }
    }

    /// Build a draft-only semantic view from an answer that may end in the
    /// middle of an inline Markdown construct. The snapshot balances only
    /// provisional delimiters for rendering; it never mutates the canonical
    /// provider output that is persisted or used for the permanent final.
    pub fn from_streaming_markdown(text: &str, side_mode: bool) -> Self {
        let snapshot = streaming_markdown_snapshot(text);
        Self {
            title: None,
            blocks: parse_markdown(&snapshot),
            actions: vec![],
            side_mode,
        }
    }
}

fn streaming_markdown_snapshot(input: &str) -> String {
    let mut bold_markers = Vec::new();
    let mut code_markers = Vec::new();
    let mut in_fence = false;
    let mut in_inline_code = false;
    let mut index = 0usize;

    while index < input.len() {
        let rest = &input[index..];
        if rest.starts_with("```") {
            in_fence = !in_fence;
            in_inline_code = false;
            index += 3;
            continue;
        }

        let Some(character) = rest.chars().next() else {
            break;
        };
        if in_fence {
            index += character.len_utf8();
            continue;
        }

        if character == '`' {
            code_markers.push(index);
            in_inline_code = !in_inline_code;
            index += 1;
            continue;
        }

        if !in_inline_code && rest.starts_with("**") {
            bold_markers.push(index);
            index += 2;
            continue;
        }

        index += character.len_utf8();
    }

    let mut closers = Vec::new();
    if bold_markers.len() % 2 == 1 {
        closers.push((*bold_markers.last().unwrap(), "**"));
    }
    if code_markers.len() % 2 == 1 {
        closers.push((*code_markers.last().unwrap(), "`"));
    }
    if closers.is_empty() {
        return input.to_owned();
    }

    // Close the most recently opened construct first. This keeps a draft like
    // `**bold \`code` semantically nested without exposing either marker.
    closers.sort_by(|left, right| right.0.cmp(&left.0));
    let mut snapshot = input.to_owned();
    for (_, delimiter) in closers {
        snapshot.push_str(delimiter);
    }
    snapshot
}

pub fn parse_markdown(input: &str) -> Vec<Block> {
    let lines = input.lines().collect::<Vec<_>>();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        if let Some(rest) = line.trim_start().strip_prefix("```") {
            let language = rest.trim();
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                code.push(lines[i]);
                i += 1;
            }
            if i < lines.len() {
                i += 1;
            }
            out.push(Block::Code {
                language: (!language.is_empty()).then(|| language.to_owned()),
                content: code.join("\n"),
            });
            continue;
        }

        let trimmed = line.trim_start();
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed.chars().nth(hashes) == Some(' ') {
            out.push(Block::RichHeading {
                level: hashes as u8,
                content: parse_inline(trimmed[hashes + 1..].trim()),
            });
            i += 1;
            continue;
        }

        if is_divider(line) {
            out.push(Block::Divider);
            i += 1;
            continue;
        }

        if i + 1 < lines.len() && looks_like_table_row(line) && is_table_separator(lines[i + 1]) {
            let headers = table_cells(line)
                .into_iter()
                .map(parse_inline)
                .collect::<Vec<_>>();
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() && !lines[i].trim().is_empty() && looks_like_table_row(lines[i]) {
                rows.push(
                    table_cells(lines[i])
                        .into_iter()
                        .map(parse_inline)
                        .collect(),
                );
                i += 1;
            }
            out.push(Block::RichTable { headers, rows });
            continue;
        }

        if trimmed.starts_with("> ") || trimmed == ">" {
            let mut quote = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim_start();
                if let Some(v) = t.strip_prefix("> ") {
                    quote.push(v);
                    i += 1;
                } else if t == ">" {
                    quote.push("");
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(Block::RichQuote {
                content: parse_inline(&quote.join("\n")),
            });
            continue;
        }

        if bullet_item(trimmed).is_some() {
            let mut items = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim_start();
                if let Some(v) = bullet_item(t) {
                    items.push(parse_inline(v));
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(Block::RichList {
                ordered: false,
                items,
            });
            continue;
        }

        if ordered_item(trimmed).is_some() {
            let mut items = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim_start();
                if let Some(v) = ordered_item(t) {
                    items.push(parse_inline(v));
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(Block::RichList {
                ordered: true,
                items,
            });
            continue;
        }

        let mut paragraph = vec![line.trim_end()];
        i += 1;
        while i < lines.len() && !lines[i].trim().is_empty() && !starts_block(&lines, i) {
            paragraph.push(lines[i].trim_end());
            i += 1;
        }
        out.push(Block::RichParagraph {
            content: parse_inline(&paragraph.join("\n")),
        });
    }
    if out.is_empty() && !input.is_empty() {
        out.push(Block::RichParagraph {
            content: parse_inline(input),
        });
    }
    out
}

pub fn parse_inline(input: &str) -> Vec<RichText> {
    let mut out = Vec::new();
    let mut rest = input;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**") {
                out.push(RichText::Bold {
                    content: parse_inline(&after[..end]),
                });
                rest = &after[end + 2..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                out.push(RichText::Code {
                    text: after[..end].to_owned(),
                });
                rest = &after[end + 1..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('*') {
            if let Some(end) = after.find('*') {
                if end > 0 {
                    out.push(RichText::Italic {
                        content: parse_inline(&after[..end]),
                    });
                    rest = &after[end + 1..];
                    continue;
                }
            }
        }
        if let Some(after) = rest.strip_prefix('_') {
            if let Some(end) = after.find('_') {
                if end > 0 {
                    out.push(RichText::Italic {
                        content: parse_inline(&after[..end]),
                    });
                    rest = &after[end + 1..];
                    continue;
                }
            }
        }
        if rest.starts_with('[') {
            if let Some(close) = rest.find("](") {
                if let Some(end) = rest[close + 2..].find(')') {
                    let url = &rest[close + 2..close + 2 + end];
                    if url.starts_with("https://") || url.starts_with("http://") {
                        out.push(RichText::Link {
                            content: parse_inline(&rest[1..close]),
                            url: url.to_owned(),
                        });
                        rest = &rest[close + 3 + end..];
                        continue;
                    }
                }
            }
        }
        let next = rest
            .char_indices()
            .skip(1)
            .find(|(_, c)| matches!(c, '*' | '_' | '`' | '['))
            .map(|(n, _)| n)
            .unwrap_or(rest.len());
        let take = if next == 0 {
            rest.chars().next().unwrap().len_utf8()
        } else {
            next
        };
        push_text(&mut out, &rest[..take]);
        rest = &rest[take..];
    }
    out
}

fn push_text(out: &mut Vec<RichText>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(RichText::Text { text: existing }) = out.last_mut() {
        existing.push_str(text);
    } else {
        out.push(RichText::Text {
            text: text.to_owned(),
        });
    }
}
fn is_divider(line: &str) -> bool {
    matches!(line.trim(), "---" | "***" | "___")
}
fn looks_like_table_row(line: &str) -> bool {
    line.contains('|') && table_cells(line).len() >= 2
}
fn table_cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect()
}
fn is_table_separator(line: &str) -> bool {
    let cells = table_cells(line);
    cells.len() >= 2
        && cells.iter().all(|c| {
            c.trim_matches(':').chars().all(|ch| ch == '-') && c.trim_matches(':').len() >= 3
        })
}
fn bullet_item(line: &str) -> Option<&str> {
    ["- ", "* ", "+ "].iter().find_map(|p| line.strip_prefix(p))
}
fn ordered_item(line: &str) -> Option<&str> {
    let dot = line.find(". ")?;
    (dot > 0 && line[..dot].chars().all(|c| c.is_ascii_digit())).then(|| &line[dot + 2..])
}
fn starts_block(lines: &[&str], i: usize) -> bool {
    let t = lines[i].trim_start();
    t.starts_with("```")
        || t.starts_with('#')
        || t.starts_with("> ")
        || bullet_item(t).is_some()
        || ordered_item(t).is_some()
        || is_divider(t)
        || (i + 1 < lines.len() && looks_like_table_row(t) && is_table_separator(lines[i + 1]))
}

#[cfg(test)]
mod markdown_tests {
    use super::*;
    #[test]
    fn parses_native_blocks() {
        let blocks=parse_markdown("## Result\n\n| File | Status |\n|---|---|\n| `auth.rs` | **Fixed** |\n\n```rust\nfn main() {}\n```\n\n- one\n- two\n\n1. first\n2. second\n\n> quote");
        assert!(matches!(blocks[0], Block::RichHeading { .. }));
        assert!(blocks.iter().any(|b| matches!(b, Block::RichTable { .. })));
        assert!(blocks
            .iter()
            .any(|b| matches!(b,Block::Code{language:Some(l),..} if l=="rust")));
        assert_eq!(
            blocks
                .iter()
                .filter(|b| matches!(b, Block::RichList { .. }))
                .count(),
            2
        );
        assert!(blocks.iter().any(|b| matches!(b, Block::RichQuote { .. })));
    }
    #[test]
    fn streaming_snapshot_hides_provisional_bold_and_code_markers() {
        let view = View::from_streaming_markdown(
            "Orbit itu **jatuh terus dan `melengkung",
            false,
        );
        let rendered_plain = view
            .blocks
            .iter()
            .map(block_plain)
            .collect::<String>();
        assert_eq!(rendered_plain, "Orbit itu jatuh terus dan melengkung");

        let debug = format!("{:?}", view.blocks);
        assert!(debug.contains("Bold"));
        assert!(debug.contains("Code"));
        assert!(!rendered_plain.contains("**"));
        assert!(!rendered_plain.contains('`'));
    }

    #[test]
    fn streaming_snapshot_does_not_touch_canonical_markdown_parser() {
        let input = "Broken **bold";
        let canonical = parse_markdown(input)
            .iter()
            .map(block_plain)
            .collect::<String>();
        let streaming = View::from_streaming_markdown(input, false)
            .blocks
            .iter()
            .map(block_plain)
            .collect::<String>();

        assert_eq!(canonical, input);
        assert_eq!(streaming, "Broken bold");
    }

    #[test]
    fn malformed_markup_preserves_content() {
        let input = "Broken **bold and `code";
        let plain = parse_markdown(input)
            .iter()
            .map(block_plain)
            .collect::<String>();
        assert_eq!(plain, input);
    }
    #[test]
    fn inline_emphasis_code_and_link() {
        let v = parse_inline("**bold** *ital* `code` [site](https://example.com)");
        assert!(v.iter().any(|x| matches!(x, RichText::Bold { .. })));
        assert!(v.iter().any(|x| matches!(x, RichText::Italic { .. })));
        assert!(v.iter().any(|x| matches!(x, RichText::Code { .. })));
        assert!(v.iter().any(|x| matches!(x, RichText::Link { .. })));
    }
    #[test]
    fn heading_and_paragraph_are_distinct_semantic_blocks() {
        let b = parse_markdown("# Heading\n\nA paragraph.");
        assert!(matches!(
            b.first(),
            Some(Block::RichHeading { level: 1, .. })
        ));
        assert!(matches!(b.get(1), Some(Block::RichParagraph { .. })));
    }
    #[test]
    fn fenced_code_preserves_language_and_body() {
        let b = parse_markdown("```rust\nfn main() {}\n```");
        assert!(
            matches!(b.first(),Some(Block::Code{language:Some(lang),content}) if lang=="rust"&&content=="fn main() {}")
        );
    }
    #[test]
    fn ordered_and_bullet_lists_keep_ordering_mode() {
        let b = parse_markdown("- a\n- b\n\n1. c\n2. d");
        assert!(matches!(
            b.first(),
            Some(Block::RichList { ordered: false, .. })
        ));
        assert!(matches!(
            b.get(1),
            Some(Block::RichList { ordered: true, .. })
        ));
    }
    #[test]
    fn blockquote_is_semantic() {
        let b = parse_markdown("> quoted");
        assert!(matches!(b.first(), Some(Block::RichQuote { .. })));
    }
    fn block_plain(b: &Block) -> String {
        match b {
            Block::RichParagraph { content }
            | Block::RichHeading { content, .. }
            | Block::RichQuote { content } => content.iter().map(RichText::plain).collect(),
            _ => String::new(),
        }
    }
}
