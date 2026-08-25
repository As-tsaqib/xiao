import re
with open('src/telegram/rich.rs', 'r') as f:
    text = f.read()

# Replace the thinking emoji line
text = text.replace('(ProgressIcon::Thinking, AI_ACTION_THINKING, "💭"),', '(ProgressIcon::Thinking, None, "💭"),')

# Replace the other AI action lines
def replacer(m):
    return f"(ProgressIcon::{m.group(1)}, Some({m.group(2)}), {m.group(3)}),"

text = re.sub(r'\(ProgressIcon::(\w+),\s*([A-Z_]+),\s*("[^"]+")\s*\),', replacer, text)

# Replace the custom_emoji_id assignment
text = text.replace('custom_emoji_id: Some(id.to_owned()),', 'custom_emoji_id: id.map(str::to_owned),')

with open('src/telegram/rich.rs', 'w') as f:
    f.write(text)
