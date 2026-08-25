with open("src/telegram/rich.rs", "r") as f:
    text = f.read()

text = text.replace("use crate::presentation::{Block, ProgressIcon, ProgressItem, ProgressState, ProgressActivity, RichText, View};", "use crate::presentation::{\n    Block, ProgressActivity, ProgressIcon, ProgressItem, ProgressState, RichText, View,\n};")

with open("src/telegram/rich.rs", "w") as f:
    f.write(text)
