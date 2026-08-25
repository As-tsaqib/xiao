use xiao::{
    presentation::{Block, ProgressActivity, ProgressIcon, ProgressItem, ProgressState, View},
    telegram::rich::{render, render_with_registry, TelegramEmojiRegistry},
};

#[test]
fn custom_emoji_registry_gracefully_falls_back_when_unvalidated_or_non_numeric() {
    let mut registry = TelegramEmojiRegistry::default();

    // Non-digit emoji ID rejected
    registry.set(ProgressIcon::Tool, Some("invalid-emoji-id"), true);
    let view = View {
        title: None,
        side_mode: false,
        blocks: vec![Block::Progress {
            items: vec![ProgressItem {
                id: 1,
                state: ProgressState::Active,
                activity: ProgressActivity::Tool,
                icon: ProgressIcon::Tool,
                action_key: None,
                correlation_id: None,
                summary: None,
                label: "Searching...".into(),
            }],
        }],
        actions: vec![],
    };
    let rendered = render_with_registry(&view, true, &registry);
    let json_str = rendered.to_string();
    assert!(!json_str.contains("invalid-emoji-id"));

    // Valid numeric emoji ID accepted when validated
    registry.set(ProgressIcon::Tool, Some("5368324170671204113"), true);
    let rendered_valid = render_with_registry(&view, true, &registry);
    let valid_str = rendered_valid.to_string();
    assert!(valid_str.contains("5368324170671204113"));
}

#[test]
fn default_render_uses_quiet_completed_progress() {
    let view = View {
        title: Some("Task Complete".into()),
        side_mode: false,
        blocks: vec![Block::Progress {
            items: vec![ProgressItem {
                id: 2,
                state: ProgressState::Done,
                activity: ProgressActivity::Thinking,
                icon: ProgressIcon::Thinking,
                action_key: None,
                correlation_id: None,
                summary: None,
                label: "Done".into(),
            }],
        }],
        actions: vec![],
    };
    let rendered = render(&view, false);
    assert!(!rendered.to_string().is_empty());
}
