import re

with open('src/agent/mod.rs', 'r') as f:
    text = f.read()

replacement = """
                if request.messages.len() > 20 {
                    let mut keep = Vec::new();
                    // Keep the first 5 messages (like initial prompt)
                    keep.extend(request.messages.drain(0..5.min(request.messages.len())));
                    // Keep the last 10 messages
                    let remaining = request.messages.len();
                    if remaining > 10 {
                        request.messages.drain(0..(remaining - 10));
                    }
                    keep.extend(request.messages.drain(..));
                    request.messages = keep;
                }
                let turn = tokio::select! {
"""

text = text.replace("                let turn = tokio::select! {", replacement.strip("\n"))

with open('src/agent/mod.rs', 'w') as f:
    f.write(text)
