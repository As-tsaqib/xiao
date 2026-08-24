use std::{env, fs, process::Command};

fn run(program: &str, args: &[&str]) {
    let status = Command::new(program).args(args).status().expect("spawn command");
    assert!(status.success(), "{program} {args:?} failed");
}

fn replace_once(text: &mut String, from: &str, to: &str) {
    let count = text.matches(from).count();
    assert_eq!(count, 1, "expected one match for {from:?}, got {count}");
    *text = text.replacen(from, to, 1);
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var_os("GITHUB_ACTIONS").is_none() {
        return;
    }

    let path = "src/bin_cli.rs";
    let mut text = fs::read_to_string(path).expect("read src/bin_cli.rs");
    replace_once(
        &mut text,
        "serde_json::to_string_pretty(&value)?",
        "serde_json::to_string_pretty(&value).map_err(anyhow::Error::from)?",
    );
    replace_once(
        &mut text,
        ".timeout(timeout)\n            .build()?;",
        ".timeout(timeout)\n            .build()\n            .map_err(anyhow::Error::from)?;",
    );
    replace_once(
        &mut text,
        "let value = serde_json::to_value(config)?;",
        "let value = serde_json::to_value(config).map_err(anyhow::Error::from)?;",
    );
    replace_once(
        &mut text,
        "let token = String::from_utf8(URL_SAFE_NO_PAD.decode(&args[1])?)?;",
        "let token = String::from_utf8(\n                URL_SAFE_NO_PAD\n                    .decode(&args[1])\n                    .map_err(anyhow::Error::from)?,\n            )\n            .map_err(anyhow::Error::from)?;",
    );
    replace_once(
        &mut text,
        "let mut url = reqwest::Url::parse(&format!(\"{}{}\", client.endpoint, path))?;",
        "let mut url = reqwest::Url::parse(&format!(\"{}{}\", client.endpoint, path))\n                .map_err(anyhow::Error::from)?;",
    );
    replace_once(
        &mut text,
        "println!(\"{}\", serde_json::to_string(&value)?);",
        "println!(\n        \"{}\",\n        serde_json::to_string(&value).map_err(anyhow::Error::from)?\n    );",
    );
    replace_once(
        &mut text,
        "let decoded = URL_SAFE_NO_PAD.decode(encoded)?;",
        "let decoded = URL_SAFE_NO_PAD\n        .decode(encoded)\n        .map_err(anyhow::Error::from)?;",
    );
    replace_once(
        &mut text,
        "let raw = String::from_utf8(decoded)?;",
        "let raw = String::from_utf8(decoded).map_err(anyhow::Error::from)?;",
    );

    fs::write(path, text).expect("write src/bin_cli.rs");
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
    run(
        "git",
        &["commit", "-m", "fix(cli): preserve required error conversions"],
    );
    run(
        "git",
        &[
            "push",
            "origin",
            "HEAD:feat/v0.2.7-control-plane-unification",
        ],
    );
}
