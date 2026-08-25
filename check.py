with open("src/tools/builtin/terminal.rs", "r") as f:
    text = f.read()

count = 0
for i, line in enumerate(text.splitlines(), 1):
    for c in line:
        if c == '{': count += 1
        elif c == '}': count -= 1
    if count < 0:
        print(f"Negative at line {i}: {line}")
