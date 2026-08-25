with open("src/telegram/rich.rs", "r") as f:
    text = f.read()

text = text.replace("use crate::presentation::{\n    Block, ProgressIcon, ProgressItem, ProgressState, RichText, View,\n};", "use crate::presentation::{Block, ProgressIcon, ProgressItem, ProgressState, RichText, View};")
text = text.replace("use serde_json::{json, Value};\n\n\nconst AI_ACTION_ANALYZING:", "use serde_json::{json, Value};\n\nconst AI_ACTION_ANALYZING:")

with open("src/telegram/rich.rs", "w") as f:
    f.write(text)
