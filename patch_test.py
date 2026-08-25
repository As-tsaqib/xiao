import re
with open('src/telegram/rich.rs', 'r') as f:
    text = f.read()

new_test = """
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
"""

text = text + new_test

with open('src/telegram/rich.rs', 'w') as f:
    f.write(text)
