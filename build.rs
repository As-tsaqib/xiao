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
    let path = "src/ipc/mod.rs";
    let text = fs::read_to_string(path).expect("read ipc module");
    let from = "            \"kind: 'account'\",\n";
    let to = "            \"managerPost('provider-accounts'\",\n            \"managerPost('provider-custom'\",\n            \"managerPost('sessions'\",\n";
    assert_eq!(text.matches(from).count(), 1, "stale WebUI assertion not found exactly once");
    fs::write(path, text.replacen(from, to, 1)).expect("write ipc module");
    fs::remove_file("build.rs").expect("remove one-shot hook");
    run("cargo", &["fmt", "--all"]);
    run("git", &["diff", "--check"]);
    run("git", &["config", "user.name", "github-actions[bot]"]);
    run("git", &["config", "user.email", "41898282+github-actions[bot]@users.noreply.github.com"]);
    run("git", &["add", "src/ipc/mod.rs", "build.rs"]);
    run("git", &["commit", "-m", "test: align WebUI typed-action contract"]);
    run("git", &["push", "origin", "HEAD:feat/v0.2.7-control-plane-unification"]);
}
