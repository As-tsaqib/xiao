import re

with open('src/tools/builtin/terminal.rs', 'r') as f:
    text = f.read()

# I will find the TermuxJobTool execute method.
pattern = r'(match self\.terminal\.execute\(context, call\)\.await \{\n\s+Ok\(output\).*?\n\s+\}\n\s+\})'
replacement = """
            if context.cancellation.is_cancelled() {
                results.push(json!({"index":index,"id":step.id,"status":"cancelled","error":"job cancelled"}));
                break;
            }
            match self.terminal.execute(context, call).await {
                Ok(output) => results.push(
                    json!({"index":index,"id":step.id,"status":"succeeded","summary":output}),
                ),
                Err(error) => {
                    results.push(json!({"index":index,"id":step.id,"status":"failed","error":error.to_string()}));
                    if !step.continue_on_error {
                        break;
                    }
                }
            }"""

new_text = re.sub(pattern, replacement.strip(), text, flags=re.DOTALL)

with open('src/tools/builtin/terminal.rs', 'w') as f:
    f.write(new_text)

