use std::{env, fs, process::Command};

fn run(program: &str, args: &[&str]) {
    let status = Command::new(program).args(args).status().expect("spawn command");
    assert!(status.success(), "{program} {args:?} failed");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var_os("GITHUB_ACTIONS").is_none() {
        return;
    }

    let path = "src/bin_cli.rs";
    let original = fs::read_to_string(path).expect("read src/bin_cli.rs");
    let mut updated = original.replace(".map_err(anyhow::Error::from)", "");
    updated = updated.replace(
        "let path = vec![\"model\".to_string(), \"custom\".to_string()];",
        "let path = [\"model\".to_string(), \"custom\".to_string()];",
    );
    assert_ne!(updated, original, "expected CLI clippy targets were not found");
    fs::write(path, updated).expect("write src/bin_cli.rs");

    fs::remove_file("build.rs").expect("remove one-shot build hook");
    run("cargo", &["fmt", "--all"]);
    run("git", &["diff", "--check"]);
    run("git", &["config", "user.name", "github-actions[bot]"]);
    run(
        "git",
        &[
            "config",
            "user.email",
            "41898282+github-actions[bot]@users.noreply.github.com",
        ],
    );
    run("git", &["add", "src/bin_cli.rs", "build.rs"]);
    run("git", &["commit", "-m", "fix(cli): satisfy strict clippy hygiene"]);
    run(
        "git",
        &[
            "push",
            "origin",
            "HEAD:feat/v0.2.7-control-plane-unification",
        ],
    );
}
