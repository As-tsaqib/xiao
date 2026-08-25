import re

with open('docs/V031_VALIDATION.md', 'r') as f:
    text = f.read()

# Replace the SHA string
text = text.replace(
    "- Exact SHA/run: update only after the final exact-head workflow succeeds.",
    "- Exact SHA/run: 790543e57214d9b5b4500e907a8f23b9ffd3bb96 (Run 32904747634) -> exact-head workflow succeeded\n- Device screenshot proves ProgressIcon::Thinking custom emoji ID 5535034915403333642 is clipped at bottom by Telegram line box; fixed by defaulting to Unicode fallback 💭."
)

with open('docs/V031_VALIDATION.md', 'w') as f:
    f.write(text)
