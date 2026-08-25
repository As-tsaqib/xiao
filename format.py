import subprocess
try:
    subprocess.run(["rustfmt", "src/tools/builtin/terminal.rs"], check=True)
except FileNotFoundError:
    pass
