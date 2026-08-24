use std::process::Command;

fn xiao() -> Command {
    Command::new(env!("CARGO_BIN_EXE_xiao"))
}

#[test]
fn root_help_matches_snapshot() {
    let output = xiao().arg("--help").output().expect("run xiao --help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help utf8");
    let (version, body) = stdout.split_once('\n').expect("help first line");
    assert!(version.starts_with("xiao v"));
    assert_eq!(body, include_str!("snapshots/cli_help_body.txt"));
}

#[test]
fn typo_is_usage_error_and_never_falls_through_to_chat() {
    let output = xiao().arg("stats").output().expect("run typo");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
    assert!(stderr.contains("unknown command `stats`"));
    assert!(stderr.contains("did you mean `xiao status`"));
    assert!(stderr.contains("Chat is explicit"));
}

#[test]
fn removed_aliases_remain_usage_errors() {
    for alias in ["about", "logout"] {
        let output = xiao().arg(alias).output().expect("run removed alias");
        assert_eq!(output.status.code(), Some(2), "{alias}");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf8");
        assert!(stderr.contains("unknown command"), "{alias}: {stderr}");
    }
}

#[test]
fn json_usage_error_has_stable_application_envelope() {
    let output = xiao()
        .args(["stats", "--json"])
        .output()
        .expect("run json typo");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).expect("json error");
    assert_eq!(value.get("status").and_then(|v| v.as_str()), Some("error"));
    assert_eq!(
        value.pointer("/error/code").and_then(|v| v.as_str()),
        Some("unknown_command")
    );
    assert!(value
        .pointer("/error/details")
        .is_some_and(|v| v.is_object()));
    assert!(value.get("ok").is_none());
    let message = value
        .pointer("/error/message")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(message.contains("unknown command `stats`"));
    assert!(value.get("view").is_none());
    assert!(value.get("actions").is_none());
    assert!(value.get("buttons").is_none());
}

#[test]
fn subcommand_help_is_terminal_native_and_does_not_require_daemon() {
    let output = xiao()
        .args(["model", "custom", "--help"])
        .output()
        .expect("run subcommand help");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("help utf8").trim(),
        "Usage: xiao model custom <list|add|show|edit|test|probe|models|use|delete> ..."
    );
}
