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

#[test]
fn subcommand_help_for_all_command_families_is_terminal_native() {
    let cases: &[(&[&str], &str)] = &[
        (&["chat", "--help"], "Usage: xiao chat"),
        (&["ask", "--help"], "Usage: xiao chat"),
        (
            &["telegram", "--help"],
            "Usage: xiao telegram <status|configure|set-owner|set-token-file|test>",
        ),
        (
            &["telegram", "configure", "--help"],
            "Usage: xiao telegram configure",
        ),
        (
            &["telegram", "set-owner", "--help"],
            "Usage: xiao telegram set-owner",
        ),
        (
            &["telegram", "set-token-file", "--help"],
            "Usage: xiao telegram set-token-file",
        ),
        (&["telegram", "test", "--help"], "Usage: xiao telegram test"),
        (
            &["model", "--help"],
            "Usage: xiao model <show|list|use|custom> ...",
        ),
        (
            &["model", "show", "--help"],
            "Usage: xiao model show [--session ID]",
        ),
        (
            &["model", "list", "--help"],
            "Usage: xiao model list [--session ID]",
        ),
        (
            &["model", "use", "--help"],
            "Usage: xiao model use MODEL [--session ID]",
        ),
        (
            &["model", "custom", "--help"],
            "Usage: xiao model custom <list|add|show|edit|test|probe|models|use|delete> ...",
        ),
        (
            &["model", "custom", "list", "--help"],
            "Usage: xiao model custom list",
        ),
        (
            &["model", "custom", "add", "--help"],
            "Usage: xiao model custom add",
        ),
        (
            &["model", "custom", "show", "--help"],
            "Usage: xiao model custom show ID",
        ),
        (
            &["model", "custom", "edit", "--help"],
            "Usage: xiao model custom edit",
        ),
        (
            &["model", "custom", "test", "--help"],
            "Usage: xiao model custom test",
        ),
        (
            &["model", "custom", "probe", "--help"],
            "Usage: xiao model custom probe",
        ),
        (
            &["model", "custom", "models", "--help"],
            "Usage: xiao model custom models",
        ),
        (
            &["model", "custom", "use", "--help"],
            "Usage: xiao model custom use",
        ),
        (
            &["model", "custom", "delete", "--help"],
            "Usage: xiao model custom delete",
        ),
        (
            &["sessions", "--help"],
            "Usage: xiao sessions <list|new|show|use|rename|delete> ...",
        ),
        (&["sessions", "list", "--help"], "Usage: xiao sessions list"),
        (&["sessions", "new", "--help"], "Usage: xiao sessions new"),
        (&["sessions", "show", "--help"], "Usage: xiao sessions show"),
        (&["sessions", "use", "--help"], "Usage: xiao sessions use"),
        (
            &["sessions", "rename", "--help"],
            "Usage: xiao sessions rename",
        ),
        (
            &["sessions", "delete", "--help"],
            "Usage: xiao sessions delete",
        ),
        (
            &["yolo", "--help"],
            "Usage: xiao yolo <status|on|off> [--session ID]",
        ),
        (
            &["memory", "--help"],
            "Usage: xiao memory <list|search|get|set|forget> ...",
        ),
        (
            &["skills", "--help"],
            "Usage: xiao skills <list|search|show|enable|disable|delete> ...",
        ),
        (
            &["approvals", "--help"],
            "Usage: xiao approvals <list|approve|deny> ...",
        ),
        (
            &["attachments", "--help"],
            "Usage: xiao attachments <list|show|remove> [--session ID] ...",
        ),
        (
            &["runs", "--help"],
            "Usage: xiao runs <list|show|cancel> ...",
        ),
        (
            &["daemon", "--help"],
            "Usage: xiao daemon <start|foreground|stop|restart|status|logs> ...",
        ),
        (&["daemon", "start", "--help"], "Usage: xiao daemon start"),
        (
            &["daemon", "foreground", "--help"],
            "Usage: xiao daemon foreground",
        ),
        (&["daemon", "stop", "--help"], "Usage: xiao daemon stop"),
        (
            &["daemon", "restart", "--help"],
            "Usage: xiao daemon restart",
        ),
        (&["daemon", "status", "--help"], "Usage: xiao daemon status"),
        (
            &["daemon", "logs", "--help"],
            "Usage: xiao daemon logs [LINES]",
        ),
        (
            &["config", "--help"],
            "Usage: xiao config <path|check|show>",
        ),
        (&["config", "path", "--help"], "Usage: xiao config path"),
        (&["config", "check", "--help"], "Usage: xiao config check"),
        (&["config", "show", "--help"], "Usage: xiao config show"),
        (&["login", "--help"], "Usage: xiao login [custom]"),
        (&["setup", "--help"], "Usage: xiao setup"),
        (
            &["status", "--help"],
            "Usage: xiao status [--json] [--quiet]",
        ),
        (
            &["context", "--help"],
            "Usage: xiao context [--session ID] [--json]",
        ),
        (&["doctor", "--help"], "Usage: xiao doctor [--json]"),
        (&["tools", "--help"], "Usage: xiao tools [--json]"),
        (&["btw", "--help"], "Usage: xiao btw"),
        (&["stop", "--help"], "Usage: xiao stop [--session ID]"),
        (&["retry", "--help"], "Usage: xiao retry [--session ID]"),
        (&["logs", "--help"], "Usage: xiao logs [LINES]"),
    ];

    for (args, expected_prefix) in cases {
        let output = xiao()
            .args(*args)
            .output()
            .unwrap_or_else(|_| panic!("run {args:?}"));
        assert!(output.status.success(), "failed {args:?}");
        let stdout = String::from_utf8(output.stdout).expect("utf8");
        assert!(
            stdout.starts_with(expected_prefix),
            "args {args:?} expected prefix '{expected_prefix}', got '{stdout}'"
        );
    }
}

#[test]
fn help_subcommand_prefix_works() {
    let cases: &[(&[&str], &str)] = &[
        (&["help", "sessions"], "Usage: xiao sessions"),
        (&["help", "model", "custom"], "Usage: xiao model custom"),
        (&["help", "telegram"], "Usage: xiao telegram"),
        (&["help", "memory"], "Usage: xiao memory"),
        (&["help", "skills"], "Usage: xiao skills"),
        (&["help", "daemon"], "Usage: xiao daemon"),
    ];

    for (args, expected_prefix) in cases {
        let output = xiao()
            .args(*args)
            .output()
            .unwrap_or_else(|_| panic!("run {args:?}"));
        assert!(output.status.success(), "failed {args:?}");
        let stdout = String::from_utf8(output.stdout).expect("utf8");
        assert!(
            stdout.starts_with(expected_prefix),
            "args {args:?} expected prefix '{expected_prefix}', got '{stdout}'"
        );
    }
}

#[test]
fn global_options_validation_contract() {
    let cases: &[(&[&str], &str)] = &[
        (&["status", "--timeout"], "--timeout requires seconds"),
        (
            &["status", "--timeout", "0"],
            "--timeout must be between 1 and 3600 seconds",
        ),
        (
            &["status", "--timeout", "5000"],
            "--timeout must be between 1 and 3600 seconds",
        ),
        (
            &["status", "--timeout", "invalid"],
            "--timeout must be an integer number of seconds",
        ),
        (&["status", "--session"], "--session requires an id"),
    ];

    for (args, expected_error) in cases {
        let output = xiao()
            .args(*args)
            .output()
            .unwrap_or_else(|_| panic!("run {args:?}"));
        assert_eq!(output.status.code(), Some(2), "args {args:?}");
        let stderr = String::from_utf8(output.stderr).expect("utf8");
        assert!(
            stderr.contains(expected_error),
            "args {args:?} expected '{expected_error}' in stderr: '{stderr}'"
        );
    }
}

#[test]
fn subcommand_syntax_and_arity_errors_are_usage_errors() {
    let cases: &[(&[&str], &str)] = &[
        (&["status", "extra"], "usage: xiao status"),
        (&["context", "extra"], "usage: xiao context"),
        (&["doctor", "extra"], "usage: xiao doctor"),
        (&["tools", "extra"], "usage: xiao tools"),
        (&["btw", "extra"], "usage: xiao btw"),
        (&["yolo", "foo"], "usage: xiao yolo <status|on|off>"),
        (
            &["chat", "--unknown-flag"],
            "unknown chat option `--unknown-flag`",
        ),
        (&["memory", "get", "foo"], "usage: xiao memory"),
        (&["skills", "invalid"], "usage: xiao skills"),
        (&["approvals", "invalid"], "usage: xiao approvals"),
        (&["attachments", "invalid"], "usage: xiao attachments"),
        (&["runs", "invalid"], "usage: xiao runs"),
        (
            &["config", "invalid"],
            "usage: xiao config <path|check|show>",
        ),
        (
            &["daemon", "invalid"],
            "usage: xiao daemon <start|foreground|stop|restart|status|logs>",
        ),
        (&["logs", "10", "extra"], "usage: xiao logs [N]"),
    ];

    for (args, expected_error) in cases {
        let output = xiao()
            .args(*args)
            .output()
            .unwrap_or_else(|_| panic!("run {args:?}"));
        assert_eq!(output.status.code(), Some(2), "args {args:?}");
        let stderr = String::from_utf8(output.stderr).expect("utf8");
        assert!(
            stderr.contains(expected_error),
            "args {args:?} expected '{expected_error}' in stderr: '{stderr}'"
        );
    }
}

#[test]
fn version_flag_and_command_works() {
    for flag in ["--version", "-V", "version"] {
        let output = xiao()
            .arg(flag)
            .output()
            .unwrap_or_else(|_| panic!("run {flag}"));
        assert!(output.status.success(), "{flag}");
        let stdout = String::from_utf8(output.stdout).expect("utf8");
        assert!(stdout.starts_with("xiao "), "{flag}: {stdout}");
    }
}

#[test]
fn deprecated_providers_give_helpful_guidance() {
    for provider in ["codex", "antigravity", "agy"] {
        let output = xiao()
            .args(["login", provider])
            .output()
            .unwrap_or_else(|_| panic!("run login {provider}"));
        assert_eq!(output.status.code(), Some(2), "login {provider}");
        let stderr = String::from_utf8(output.stderr).expect("utf8");
        assert!(
            stderr.contains("use `xiao login` for a Custom endpoint"),
            "login {provider}: {stderr}"
        );
    }
}
