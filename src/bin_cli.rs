use std::{fs, path::Path};

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use xiao::{
    ipc::ExecuteRequest,
    security::{redact::redact_text, secrets::SecretStore},
    standalone::{self, CliPaths, ClientConfig, StartResult, StopResult},
};

#[tokio::main]
async fn main() -> Result<()> {
    run(std::env::args().skip(1).collect()).await
}

async fn run(args: Vec<String>) -> Result<()> {
    if args.is_empty() || matches!(args.first().map(String::as_str), Some("-h" | "--help")) {
        print_help();
        return Ok(());
    }
    if matches!(
        args.first().map(String::as_str),
        Some("-V" | "--version" | "version")
    ) {
        println!("xiao {}", xiao::VERSION);
        return Ok(());
    }

    let paths = CliPaths::from_env()?;
    match args[0].as_str() {
        "quickstart" => quickstart(&paths, &args[1..]).await,
        "daemon" => daemon(&paths, &args[1..]).await,
        "config" => config_command(&paths, &args[1..]),
        "admin" => admin(&paths, &args[1..]).await,
        _ => semantic_command(&paths, &args).await,
    }
}

async fn quickstart(paths: &CliPaths, args: &[String]) -> Result<()> {
    let no_start = match args {
        [] => false,
        [arg] if arg == "--no-start" => true,
        _ => bail!("usage: xiao quickstart [--no-start]"),
    };
    let init = standalone::initialize(paths)?;
    println!(
        "Config: {} ({})",
        paths.config.display(),
        if init.config_created {
            "created"
        } else {
            "preserved"
        }
    );
    println!("Data:   {}", init.runtime.data_dir.display());

    if no_start {
        let client_state = if paths.client_config.exists() {
            "preserved"
        } else {
            "pending until xiaod starts"
        };
        println!("Client: {} ({client_state})", paths.client_config.display());
        println!("Setup complete without starting xiaod.");
        println!("Next: xiao daemon start");
        return Ok(());
    }

    let started = standalone::start_daemon(paths, &init).await?;
    print_start_result(started);
    println!(
        "Client: {} ({})",
        paths.client_config.display(),
        if started.client_config_created {
            "created privately"
        } else {
            "preserved"
        }
    );
    println!("xiao quickstart complete.");
    println!("Next: xiao status");
    println!("      xiao doctor");
    println!("      xiao login codex");
    println!("      xiao chat \"hello\"");
    Ok(())
}

async fn daemon(paths: &CliPaths, args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("start") if args.len() == 1 => {
            let init = standalone::load_existing(paths)?;
            let started = standalone::start_daemon(paths, &init).await?;
            print_start_result(started);
            if started.client_config_created {
                println!(
                    "Client config created privately at {}",
                    paths.client_config.display()
                );
            }
            Ok(())
        }
        Some("foreground") if args.len() == 1 => {
            let init = standalone::load_existing(paths)?;
            let status = standalone::run_daemon_foreground(paths, &init)?;
            if status.success() {
                Ok(())
            } else {
                bail!("xiaod exited with {status}")
            }
        }
        Some("status") if args.len() == 1 => {
            let init = standalone::load_existing(paths)?;
            let status = standalone::daemon_status(paths, &init).await?;
            match status.managed_pid {
                Some(pid) => println!("Process: managed, PID {pid}"),
                None if status.reachable => println!("Process: running outside xiao lifecycle"),
                None => println!("Process: not running"),
            }
            println!(
                "IPC:     {} ({})",
                if status.reachable {
                    "ready"
                } else {
                    "unreachable"
                },
                status.endpoint
            );
            println!("Log:     {}", init.runtime.daemon_log.display());
            if status.reachable {
                Ok(())
            } else {
                bail!("xiaod is not ready")
            }
        }
        Some("logs") => {
            let lines = parse_lines(args.get(1))?;
            if args.len() > 2 {
                bail!("usage: xiao daemon logs [N]");
            }
            let init = standalone::load_existing(paths)?;
            for line in standalone::tail_daemon_log(&init.runtime, lines)? {
                println!("{line}");
            }
            Ok(())
        }
        Some("stop") if args.len() == 1 => {
            let init = standalone::load_existing(paths)?;
            print_stop_result(standalone::stop_daemon(paths, &init).await?)
        }
        Some("restart") if args.len() == 1 => {
            let init = standalone::load_existing(paths)?;
            match standalone::stop_daemon(paths, &init).await? {
                StopResult::Stopped { pid, forced } => println!(
                    "Stopped xiaod PID {pid}{}.",
                    if forced { " (forced)" } else { "" }
                ),
                StopResult::NotRunning => println!("xiaod was not running."),
                StopResult::UnmanagedRunning => {
                    bail!("xiaod is running outside this lifecycle; stop it manually first")
                }
            }
            print_start_result(standalone::start_daemon(paths, &init).await?);
            Ok(())
        }
        _ => bail!("daemon usage: xiao daemon <start|foreground|status|logs [N]|stop|restart>"),
    }
}

fn config_command(paths: &CliPaths, args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("path") if args.len() == 1 => {
            println!("Config: {}", paths.config.display());
            println!("Client: {}", paths.client_config.display());
            if paths.config.exists() {
                let init = standalone::load_existing(paths)?;
                println!("Data:   {}", init.runtime.data_dir.display());
                println!("DB:     {}", init.runtime.database.display());
                println!("Logs:   {}", init.runtime.logs_dir.display());
                println!("Secrets:{}", init.runtime.secrets_dir.display());
            } else {
                println!("Data:   {}", paths.default_data_dir.display());
            }
            Ok(())
        }
        Some("check") if args.len() == 1 => {
            let init = standalone::load_existing(paths)?;
            println!("Daemon config: valid ({})", paths.config.display());
            if paths.client_config.exists() {
                ClientConfig::load(&paths.client_config)?;
                println!("Client config: valid ({})", paths.client_config.display());
            } else {
                println!("Client config: not created yet");
            }
            println!("IPC bind: loopback ({})", init.config.ipc.bind);
            Ok(())
        }
        _ => bail!("config usage: xiao config <path|check>"),
    }
}

async fn semantic_command(paths: &CliPaths, args: &[String]) -> Result<()> {
    let config = ClientConfig::load(&paths.client_config)?;
    let client = reqwest::Client::new();
    if args.first().map(String::as_str) == Some("logs") {
        let lines = parse_lines(args.get(1))?;
        if args.len() > 2 {
            bail!("usage: xiao logs [N]");
        }
        let response = client
            .get(format!(
                "{}/v1/logs?lines={lines}",
                config.endpoint.trim_end_matches('/')
            ))
            .bearer_auth(&config.token)
            .send()
            .await
            .context("connect to xiaod; run `xiao daemon status`")?;
        let value = response_json(response).await?;
        for line in value
            .get("lines")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str())
        {
            println!("{line}");
        }
        return Ok(());
    }

    let input = normalize_cli(args);
    let response = client
        .post(format!(
            "{}/v1/command",
            config.endpoint.trim_end_matches('/')
        ))
        .bearer_auth(&config.token)
        .json(&ExecuteRequest {
            principal: config.principal,
            input,
        })
        .send()
        .await
        .context("connect to xiaod; run `xiao daemon status`")?;
    let value = response_json(response).await?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn normalize_cli(args: &[String]) -> String {
    if args.is_empty() {
        return String::new();
    }
    if args[0] == "chat" {
        return args[1..].join(" ");
    }
    if args[0].starts_with('/') {
        return args.join(" ");
    }
    const ALIASES: &[&str] = &[
        "start",
        "help",
        "login",
        "logout",
        "model",
        "new",
        "sessions",
        "session",
        "btw",
        "status",
        "context",
        "cancel",
        "stop",
        "retry",
        "yolo",
        "memory",
        "skills",
        "tools",
        "doctor",
        "about",
        "approvals",
        "account",
    ];
    if ALIASES.contains(&args[0].as_str()) {
        return format!("/{}", args.join(" "));
    }
    args.join(" ")
}

async fn admin(paths: &CliPaths, args: &[String]) -> Result<()> {
    let init = standalone::load_existing(paths)?;
    let token = SecretStore::new(init.runtime.secrets_dir)
        .get("ipc-admin-token")?
        .ok_or_else(|| anyhow!("admin IPC token missing; xiaod must start once"))?;
    let endpoint = format!("http://{}", init.config.ipc.bind);
    let client = reqwest::Client::new();
    match args.first().map(String::as_str) {
        Some("snapshot") => {
            print_response(
                client
                    .get(format!("{endpoint}/v1/admin/snapshot"))
                    .bearer_auth(&token)
                    .send()
                    .await?,
            )
            .await?;
        }
        Some("logs") => {
            let lines = parse_lines(args.get(1))?;
            print_response(
                client
                    .get(format!("{endpoint}/v1/logs?lines={lines}"))
                    .bearer_auth(&token)
                    .send()
                    .await?,
            )
            .await?;
        }
        Some("client-config") => {
            eprintln!("Warning: this explicit admin output contains the client credential.");
            print_response(
                client
                    .get(format!("{endpoint}/v1/admin/client-config"))
                    .bearer_auth(&token)
                    .send()
                    .await?,
            )
            .await?;
        }
        Some("apply-base64") => {
            let encoded = args
                .get(1)
                .ok_or_else(|| anyhow!("base64 payload required"))?;
            let payload = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded)?)?;
            apply_admin_json(&client, &endpoint, &token, &payload).await?;
        }
        Some("apply-file") => {
            let path = required_path(args.get(1), "JSON payload file required")?;
            let payload = fs::read_to_string(path)
                .with_context(|| format!("read admin payload {}", path.display()))?;
            apply_admin_json(&client, &endpoint, &token, &payload).await?;
        }
        Some("test-token-base64") => {
            let encoded = args
                .get(1)
                .ok_or_else(|| anyhow!("base64 token required"))?;
            let token_value = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded)?)?;
            test_telegram_token(&client, &endpoint, &token, token_value.trim()).await?;
        }
        Some("test-token-file") => {
            let path = required_path(args.get(1), "token file required")?;
            let token_value = fs::read_to_string(path)
                .with_context(|| format!("read token file {}", path.display()))?;
            test_telegram_token(&client, &endpoint, &token, token_value.trim()).await?;
        }
        Some("fetch-models-base64") => {
            let encoded = args
                .get(1)
                .ok_or_else(|| anyhow!("base64 model-discovery payload required"))?;
            let payload = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded)?)?;
            post_admin_json(
                &client,
                &endpoint,
                &token,
                "/v1/admin/custom/models",
                &payload,
            )
            .await?;
        }
        _ => bail!(concat!(
            "admin usage: snapshot | logs [N] | client-config | apply-file JSON | ",
            "apply-base64 PAYLOAD | test-token-file FILE | test-token-base64 TOKEN | ",
            "fetch-models-base64 PAYLOAD"
        )),
    }
    Ok(())
}

async fn apply_admin_json(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    payload: &str,
) -> Result<()> {
    post_admin_json(client, endpoint, token, "/v1/admin/apply", payload).await
}

async fn post_admin_json(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
    path: &str,
    payload: &str,
) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(payload)?;
    print_response(
        client
            .post(format!("{endpoint}{path}"))
            .bearer_auth(token)
            .json(&value)
            .send()
            .await?,
    )
    .await
}

async fn test_telegram_token(
    client: &reqwest::Client,
    endpoint: &str,
    admin_token: &str,
    telegram_token: &str,
) -> Result<()> {
    print_response(
        client
            .post(format!("{endpoint}/v1/admin/telegram/test"))
            .bearer_auth(admin_token)
            .json(&serde_json::json!({"token":telegram_token}))
            .send()
            .await?,
    )
    .await
}

async fn print_response(response: reqwest::Response) -> Result<()> {
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        bail!("xiaod request failed ({status}): {}", response_error(&text));
    }
    println!("{text}");
    Ok(())
}

async fn response_json(response: reqwest::Response) -> Result<serde_json::Value> {
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        bail!("xiaod request failed ({status}): {}", response_error(&text));
    }
    serde_json::from_str(&text).context("parse xiaod JSON response")
}

fn response_error(text: &str) -> String {
    let message = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.to_owned());
    redact_text(&message)
}

fn parse_lines(value: Option<&String>) -> Result<usize> {
    value
        .map(|raw| {
            raw.parse::<usize>()
                .context("line count must be an integer")
        })
        .transpose()
        .map(|value| value.unwrap_or(120).clamp(1, 500))
}

fn required_path<'a>(value: Option<&'a String>, message: &str) -> Result<&'a Path> {
    value
        .map(Path::new)
        .ok_or_else(|| anyhow!(message.to_owned()))
}

fn print_start_result(result: StartResult) {
    match (result.already_running, result.pid) {
        (true, Some(pid)) => println!("xiaod is already running (managed PID {pid})."),
        (true, None) => println!("xiaod is already running outside this lifecycle."),
        (false, Some(pid)) => println!("Started xiaod (PID {pid})."),
        (false, None) => println!("Started xiaod."),
    }
}

fn print_stop_result(result: StopResult) -> Result<()> {
    match result {
        StopResult::Stopped { pid, forced } => println!(
            "Stopped xiaod PID {pid}{}.",
            if forced { " (forced)" } else { "" }
        ),
        StopResult::NotRunning => println!("xiaod is not running."),
        StopResult::UnmanagedRunning => {
            bail!("xiaod is running outside this lifecycle; stop its foreground process manually")
        }
    }
    Ok(())
}

fn print_help() {
    println!(
        "xiao v{}\n\
Standalone Termux CLI for the xiao daemon and shared Command Core.\n\n\
First run:\n  xiao quickstart [--no-start]\n\n\
Daemon:\n  xiao daemon start|foreground|status|logs [N]|stop|restart\n\n\
Configuration:\n  xiao config path|check\n\n\
Agent and session commands:\n  xiao status\n  xiao doctor\n  xiao login [PROVIDER]\n  xiao model [NAME]\n  xiao account [ID]\n  xiao new | sessions [ARGS] | btw | context | cancel | retry\n  xiao chat TEXT\n  xiao /command [ARGS]\n  xiao logs [N]\n\n\
Environment overrides:\n  XIAO_CONFIG, XIAO_CLIENT_CONFIG, XIAO_HOME, XIAOD_BIN\n",
        xiao::VERSION
    );
}

#[cfg(test)]
mod cli_tests {
    use super::normalize_cli;

    fn n(values: &[&str]) -> String {
        normalize_cli(
            &values
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn command_aliases_keep_arguments() {
        assert_eq!(n(&["status"]), "/status");
        assert_eq!(n(&["login", "codex"]), "/login codex");
        assert_eq!(n(&["model", "gpt-5.6-sol"]), "/model gpt-5.6-sol");
        assert_eq!(n(&["provider", "codex"]), "provider codex");
        assert_eq!(n(&["account", "abc"]), "/account abc");
        assert_eq!(n(&["help", "session"]), "/help session");
    }

    #[test]
    fn multiple_command_arguments_are_preserved() {
        assert_eq!(
            n(&["session", "rename", "01HXYZ", "New", "Name"]),
            "/session rename 01HXYZ New Name"
        );
    }

    #[test]
    fn slash_and_chat_are_preserved() {
        assert_eq!(n(&["/login", "codex"]), "/login codex");
        assert_eq!(
            n(&["chat", "explain", "this", "architecture"]),
            "explain this architecture"
        );
        assert_eq!(
            n(&["explain this architecture"]),
            "explain this architecture"
        );
        assert_eq!(n(&[]), "");
    }
}
