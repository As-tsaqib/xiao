with open("src/agent/mod.rs", "r") as f:
    text = f.read()
count = 0
for i, line in enumerate(text.splitlines(), 1):
    for c in line:
        if c == '{': count += 1
        elif c == '}': count -= 1
print("Final Brackets count:", count)
