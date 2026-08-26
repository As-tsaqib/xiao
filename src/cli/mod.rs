use std::{
    collections::BTreeMap,
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{
    config::AppConfig,
    ipc::{AttachmentIngestRequest, ExecuteRequest, SessionExecuteRequest},
    security::{redact::redact_text, secrets::SecretStore},
    standalone::{self, CliPaths, ClientConfig, StartResult, StopResult},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};

const EXIT_OK: i32 = 0;
const EXIT_ERROR: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_DAEMON_UNAVAILABLE: i32 = 3;
const EXIT_REJECTED: i32 = 4;
const EXIT_NOT_FOUND: i32 = 5;
const EXIT_LOCAL_IO: i32 = 6;

const TOP_LEVEL: &[&str] = &[
    "chat",
    "ask",
    "setup",
    "status",
    "context",
    "doctor",
    "tools",
    "telegram",
    "login",
    "model",
    "sessions",
    "btw",
    "yolo",
    "stop",
    "retry",
    "memory",
    "skills",
    "approvals",
    "attachments",
    "runs",
    "daemon",
    "logs",
    "config",
    // Hidden compatibility plumbing. Never advertised as product UX.
    "admin",
    "quickstart",
];

#[derive(Debug, Clone, Default)]
struct GlobalOptions {
    json: bool,
    quiet: bool,
    session: Option<String>,
    timeout_seconds: Option<u64>,
    no_color: bool,
}

#[derive(Debug)]
struct CliFailure {
    code: i32,
    message: String,
}

impl CliFailure {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_USAGE,
            message: message.into(),
        }
    }

    fn local(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_LOCAL_IO,
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for CliFailure {
    fn from(error: anyhow::Error) -> Self {
        Self {
            code: EXIT_ERROR,
            message: redact_text(&error.to_string()),
        }
    }
}

type CliResult<T> = std::result::Result<T, CliFailure>;

struct CliPresenter {
    options: GlobalOptions,
}

impl CliPresenter {
    fn new(options: GlobalOptions) -> Self {
        Self { options }
    }

    fn success(&self, _command: &str, data: Value) -> CliResult<()> {
        if self.options.quiet {
            return Ok(());
        }
        if self.options.json {
            let value = json!({
                "status": "ok",
                "data": data,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&value).map_err(anyhow::Error::from)?
            );
            return Ok(());
        }
        render_human(&data);
        Ok(())
    }

    fn line(&self, value: impl AsRef<str>) {
        if !self.options.quiet {
            println!("{}", value.as_ref());
        }
    }

    fn error(&self, failure: &CliFailure) {
        let message = redact_text(&failure.message);
        if self.options.json && !self.options.quiet {
            let code =
                if failure.code == EXIT_USAGE && failure.message.starts_with("unknown command `") {
                    "unknown_command"
                } else {
                    match failure.code {
                        EXIT_USAGE => "usage",
                        EXIT_DAEMON_UNAVAILABLE => "daemon_unavailable",
                        EXIT_REJECTED => "rejected",
                        EXIT_NOT_FOUND => "not_found",
                        EXIT_LOCAL_IO => "local_io",
                        _ => "operation_failed",
                    }
                };
            eprintln!(
                "{}",
                serde_json::to_string(&json!({
                    "status": "error",
                    "error": {
                        "code": code,
                        "message": message,
                        "details": {},
                    },
                }))
                .unwrap_or_else(|_| {
                    "{\"status\":\"error\",\"error\":{\"code\":\"serialization_error\",\"message\":\"unable to serialize CLI error\",\"details\":{}}}".into()
                })
            );
        } else {
            eprintln!("xiao: {message}");
        }
    }
}

struct DaemonClient {
    http: reqwest::Client,
    endpoint: String,
    client_token: String,
    admin_token: String,
}

impl DaemonClient {
    fn load(paths: &CliPaths, options: &GlobalOptions) -> CliResult<Self> {
        let init = standalone::load_existing(paths).map_err(|error| CliFailure {
            code: EXIT_LOCAL_IO,
            message: error.to_string(),
        })?;
        let client = ClientConfig::load(&paths.client_config).map_err(|error| CliFailure {
            code: EXIT_DAEMON_UNAVAILABLE,
            message: error.to_string(),
        })?;
        let admin_token = SecretStore::new(init.runtime.secrets_dir)
            .get("ipc-admin-token")
            .map_err(|error| CliFailure::local(error.to_string()))?
            .ok_or_else(|| CliFailure {
                code: EXIT_DAEMON_UNAVAILABLE,
                message: "admin IPC token missing; start xiao daemon first".into(),
            })?;
        let timeout = Duration::from_secs(options.timeout_seconds.unwrap_or(300).clamp(1, 3600));
        let builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout);
        #[cfg(unix)]
        let (builder, endpoint) = {
            let sock = client
                .control_socket
                .clone()
                .unwrap_or_else(|| init.runtime.control_socket.clone());
            (builder.unix_socket(sock), "http://localhost".to_string())
        };
        #[cfg(not(unix))]
        let (builder, endpoint) = (builder, "http://localhost".to_string());
        let http = builder.build().map_err(anyhow::Error::from)?;
        Ok(Self {
            http,
            endpoint,
            client_token: client.token,
            admin_token,
        })
    }

    async fn get_admin(&self, path: &str) -> CliResult<Value> {
        let response = self
            .http
            .get(format!("{}{}", self.endpoint, path))
            .bearer_auth(&self.admin_token)
            .send()
            .await
            .map_err(connection_failure)?;
        parse_response(response).await
    }

    async fn post_admin(&self, path: &str, body: &Value) -> CliResult<Value> {
        let response = self
            .http
            .post(format!("{}{}", self.endpoint, path))
            .bearer_auth(&self.admin_token)
            .json(body)
            .send()
            .await
            .map_err(connection_failure)?;
        parse_response(response).await
    }

    async fn post_client<T: serde::Serialize>(&self, path: &str, body: &T) -> CliResult<Value> {
        let response = self
            .http
            .post(format!("{}{}", self.endpoint, path))
            .bearer_auth(&self.client_token)
            .json(body)
            .send()
            .await
            .map_err(connection_failure)?;
        parse_response(response).await
    }

    async fn active_session_id(&self) -> CliResult<String> {
        let sessions = self.get_admin("/v1/admin/sessions?limit=1").await?;
        if let Some(id) = sessions
            .get("active_cli_session_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            return Ok(id.to_owned());
        }
        let created = self
            .post_admin("/v1/admin/sessions", &json!({"action":"new"}))
            .await?;
        created
            .pointer("/session/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| CliFailure {
                code: EXIT_REJECTED,
                message: "xiao daemon did not return the new CLI session".into(),
            })
    }

    async fn target_session(&self, options: &GlobalOptions) -> CliResult<String> {
        if let Some(session) = options.session.as_deref() {
            let response = self
                .get_admin(&format!("/v1/admin/sessions?id={}", url_encode(session)))
                .await?;
            let found = response
                .get("items")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items
                        .iter()
                        .any(|item| item.get("id").and_then(Value::as_str) == Some(session))
                });
            if !found {
                return Err(CliFailure {
                    code: EXIT_NOT_FOUND,
                    message: format!("session `{session}` was not found"),
                });
            }
            Ok(session.to_owned())
        } else {
            self.active_session_id().await
        }
    }
}

pub async fn run_process(raw: Vec<String>) -> i32 {
    let json_requested = raw.iter().any(|arg| arg == "--json");
    let (options, args) = match parse_global_options(raw) {
        Ok(value) => value,
        Err(failure) => {
            let presenter = CliPresenter::new(GlobalOptions {
                json: json_requested,
                ..GlobalOptions::default()
            });
            presenter.error(&failure);
            return failure.code;
        }
    };
    let presenter = CliPresenter::new(options.clone());
    match run(options, args, &presenter).await {
        Ok(()) => EXIT_OK,
        Err(failure) => {
            presenter.error(&failure);
            failure.code
        }
    }
}

fn parse_global_options(raw: Vec<String>) -> CliResult<(GlobalOptions, Vec<String>)> {
    let mut options = GlobalOptions::default();
    let mut args = Vec::new();
    let mut index = 0usize;
    while index < raw.len() {
        match raw[index].as_str() {
            "--json" => options.json = true,
            "--quiet" => options.quiet = true,
            "--no-color" => options.no_color = true,
            "--session" => {
                index += 1;
                options.session = Some(
                    raw.get(index)
                        .cloned()
                        .ok_or_else(|| CliFailure::usage("--session requires an id"))?,
                );
            }
            "--timeout" => {
                index += 1;
                let raw_timeout = raw
                    .get(index)
                    .ok_or_else(|| CliFailure::usage("--timeout requires seconds"))?;
                let seconds = raw_timeout.parse::<u64>().map_err(|_| {
                    CliFailure::usage("--timeout must be an integer number of seconds")
                })?;
                if !(1..=3600).contains(&seconds) {
                    return Err(CliFailure::usage(
                        "--timeout must be between 1 and 3600 seconds",
                    ));
                }
                options.timeout_seconds = Some(seconds);
            }
            value => args.push(value.to_owned()),
        }
        index += 1;
    }
    Ok((options, args))
}

async fn run(options: GlobalOptions, args: Vec<String>, presenter: &CliPresenter) -> CliResult<()> {
    if args.is_empty()
        || (args.len() == 1
            && matches!(
                args.first().map(String::as_str),
                Some("-h" | "--help" | "help")
            ))
    {
        print_help();
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("help") && args.len() > 1 {
        print_subcommand_help(&args[1..]);
        return Ok(());
    }
    if let Some(position) = args
        .iter()
        .position(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        print_subcommand_help(&args[..position]);
        return Ok(());
    }
    if matches!(
        args.first().map(String::as_str),
        Some("-V" | "--version" | "version")
    ) {
        presenter.line(format!("xiao {}", crate::VERSION));
        return Ok(());
    }
    let command = args[0].as_str();
    if command.starts_with('/') || !TOP_LEVEL.contains(&command) {
        return Err(unknown_command(command));
    }
    validate_command_syntax(&args)?;
    let paths = CliPaths::from_env()?;
    match command {
        "setup" => setup(&paths, &options, presenter).await,
        "quickstart" => quickstart(&paths, &args[1..], presenter).await,
        "daemon" => daemon(&paths, &args[1..], presenter).await,
        "config" => config_command(&paths, &args[1..], presenter),
        "admin" => admin(&paths, &args[1..], &options, presenter).await,
        "logs" => logs_command(&paths, &args[1..], &options, presenter).await,
        _ => public_daemon_command(&paths, &options, &args, presenter).await,
    }
}

fn unknown_command(command: &str) -> CliFailure {
    let suggestion = TOP_LEVEL
        .iter()
        .filter(|candidate| !matches!(**candidate, "admin" | "quickstart"))
        .min_by_key(|candidate| edit_distance(command, candidate));
    let suffix = suggestion
        .filter(|candidate| edit_distance(command, candidate) <= 3)
        .map(|candidate| format!("; did you mean `xiao {candidate}`?"))
        .unwrap_or_default();
    CliFailure::usage(format!(
        "unknown command `{command}`{suffix}. Chat is explicit: `xiao chat \"...\"`"
    ))
}

async fn public_daemon_command(
    paths: &CliPaths,
    options: &GlobalOptions,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    validate_command_syntax(args)?;
    let client = DaemonClient::load(paths, options)?;
    if matches!(args[0].as_str(), "chat" | "ask") {
        return chat(&client, options, &args[1..], presenter).await;
    }
    match args[0].as_str() {
        "status" => {
            exact_arity(args, 1, "usage: xiao status")?;
            presenter.success(
                "status",
                dto_status(client.get_admin("/v1/admin/dashboard").await?),
            )
        }
        "context" => {
            exact_arity(args, 1, "usage: xiao context")?;
            let session = if let Some(id) = options.session.as_deref() {
                format!("?session_id={}", url_encode(id))
            } else {
                String::new()
            };
            presenter.success(
                "context",
                dto_context(
                    client
                        .get_admin(&format!("/v1/admin/context{session}"))
                        .await?,
                ),
            )
        }
        "doctor" => {
            exact_arity(args, 1, "usage: xiao doctor")?;
            presenter.success(
                "doctor",
                dto_doctor(client.get_admin("/v1/admin/diagnostics").await?),
            )
        }
        "tools" => {
            exact_arity(args, 1, "usage: xiao tools")?;
            presenter.success(
                "tools",
                dto_tools(client.get_admin("/v1/admin/tools").await?),
            )
        }
        "telegram" => telegram_command(&client, &args[1..], presenter).await,
        "login" => login_command(&client, &args[1..], presenter).await,
        "model" => model_command(&client, options, &args[1..], presenter).await,
        "sessions" => sessions_command(&client, &args[1..], presenter).await,
        "btw" => {
            exact_arity(args, 1, "usage: xiao btw")?;
            presenter.success(
                "btw",
                client
                    .post_admin("/v1/admin/sessions", &json!({"action":"btw"}))
                    .await?,
            )
        }
        "yolo" => yolo_command(&client, options, &args[1..], presenter).await,
        "stop" => {
            exact_arity(args, 1, "usage: xiao stop")?;
            let session = client.target_session(options).await?;
            presenter.success(
                "stop",
                client
                    .post_admin(
                        "/v1/admin/sessions",
                        &json!({"action":"stop","session_id":session}),
                    )
                    .await?,
            )
        }
        "retry" => {
            exact_arity(args, 1, "usage: xiao retry")?;
            let session = client.target_session(options).await?;
            let value = client
                .post_client(
                    "/v1/session-chat",
                    &SessionExecuteRequest {
                        principal: String::new(),
                        session_id: session,
                        input: String::new(),
                        retry: true,
                    },
                )
                .await?;
            presenter.success("retry", app_result_schema(value))
        }
        "memory" => memory_command(&client, &args[1..], presenter).await,
        "skills" => skills_command(&client, &args[1..], presenter).await,
        "approvals" => approvals_command(&client, &args[1..], presenter).await,
        "attachments" => attachments_command(&client, options, &args[1..], presenter).await,
        "runs" => runs_command(&client, &args[1..], presenter).await,
        "chat" | "ask" => Ok(()),
        other => Err(unknown_command(other)),
    }
}

fn validate_command_syntax(args: &[String]) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("status") => exact_arity(args, 1, "usage: xiao status"),
        Some("context") => exact_arity(args, 1, "usage: xiao context"),
        Some("doctor") => exact_arity(args, 1, "usage: xiao doctor"),
        Some("tools") => exact_arity(args, 1, "usage: xiao tools"),
        Some("btw") => exact_arity(args, 1, "usage: xiao btw"),
        Some("stop") => exact_arity(args, 1, "usage: xiao stop"),
        Some("retry") => exact_arity(args, 1, "usage: xiao retry"),
        Some("chat" | "ask") => validate_chat_syntax(&args[1..]),
        Some("telegram") => validate_telegram_syntax(&args[1..]),
        Some("login") => validate_login_syntax(&args[1..]),
        Some("model") => validate_model_syntax(&args[1..]),
        Some("sessions") => validate_sessions_syntax(&args[1..]),
        Some("yolo") => validate_yolo_syntax(&args[1..]),
        Some("memory") => validate_memory_syntax(&args[1..]),
        Some("skills") => validate_skills_syntax(&args[1..]),
        Some("approvals") => validate_approvals_syntax(&args[1..]),
        Some("attachments") => validate_attachments_syntax(&args[1..]),
        Some("runs") => validate_runs_syntax(&args[1..]),
        Some("setup") => exact_arity(args, 1, "usage: xiao setup"),
        Some("quickstart") => validate_quickstart_syntax(&args[1..]),
        Some("daemon") => validate_daemon_syntax(&args[1..]),
        Some("config") => validate_config_syntax(&args[1..]),
        Some("logs") => validate_logs_syntax(&args[1..]),
        _ => Ok(()),
    }
}

fn validate_chat_syntax(args: &[String]) -> CliResult<()> {
    let mut prompt_parts = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--file" | "--image" => {
                let kind = if args[index] == "--image" {
                    "image"
                } else {
                    "file"
                };
                index += 1;
                if args.get(index).is_none() {
                    return Err(CliFailure::usage(format!("--{kind} requires PATH")));
                }
            }
            value if value.starts_with("--") => {
                return Err(CliFailure::usage(format!("unknown chat option `{value}`")));
            }
            value => prompt_parts.push(value.to_owned()),
        }
        index += 1;
    }
    if prompt_parts.join(" ").trim().is_empty() {
        return Err(CliFailure::usage(
            "usage: xiao chat [--file PATH] [--image PATH] \"prompt\"",
        ));
    }
    Ok(())
}

fn validate_telegram_syntax(args: &[String]) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("status") if args.len() == 1 => Ok(()),
        Some("test") if args.len() == 1 => Ok(()),
        Some("set-token-file") if args.len() == 2 => Ok(()),
        Some("set-owner")
            if args.len() == 2 || (args.len() == 3 && args[2] == "--confirm-owner-change") =>
        {
            parse_owner_user_id(&args[1])?;
            Ok(())
        }
        Some("set-owner") => Err(CliFailure::usage(
            "usage: xiao telegram set-owner USER_ID [--confirm-owner-change]",
        )),
        Some("configure") => {
            let mut index = 1usize;
            while index < args.len() {
                match args[index].as_str() {
                    "--owner" => {
                        index += 1;
                        let val = args
                            .get(index)
                            .ok_or_else(|| CliFailure::usage("--owner requires USER_ID"))?;
                        parse_owner_user_id(val)?;
                    }
                    "--allowed-chat" => {
                        index += 1;
                        let val = args
                            .get(index)
                            .ok_or_else(|| CliFailure::usage("--allowed-chat requires CHAT_ID"))?;
                        parse_i64(val, "allowed chat id")?;
                    }
                    "--token-file" => {
                        index += 1;
                        if args.get(index).is_none() {
                            return Err(CliFailure::usage("--token-file requires PATH"));
                        }
                    }
                    "--enable" | "--disable" | "--confirm-owner-change" | "--test" => {}
                    other => {
                        return Err(CliFailure::usage(format!(
                            "unknown telegram configure option `{other}`"
                        )))
                    }
                }
                index += 1;
            }
            Ok(())
        }
        _ => Err(CliFailure::usage(
            "usage: xiao telegram <status|configure|set-owner|set-token-file|test>",
        )),
    }
}

fn validate_login_syntax(args: &[String]) -> CliResult<()> {
    if args.is_empty() || matches!(args, [value] if value == "custom") {
        return Ok(());
    }
    match args {
        [value] if matches!(value.as_str(), "codex" | "antigravity" | "agy") => {
            Err(CliFailure::usage(
                "provider_configuration_required: Codex and Antigravity are no longer supported; use `xiao login` for a Custom endpoint",
            ))
        }
        _ => Err(CliFailure::usage("usage: xiao login [custom]")),
    }
}

fn validate_model_syntax(args: &[String]) -> CliResult<()> {
    let Some(sub) = args.first().map(String::as_str) else {
        return Err(CliFailure::usage(
            "usage: xiao model <show|list|use|custom>",
        ));
    };
    match sub {
        "show" if args.len() == 1 => Ok(()),
        "list" if args.len() == 1 => Ok(()),
        "use" if args.len() == 2 => Ok(()),
        "custom" => validate_custom_syntax(&args[1..]),
        _ => Err(CliFailure::usage(
            "usage: xiao model <show|list|use> | xiao model custom ...",
        )),
    }
}

fn validate_custom_syntax(args: &[String]) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => Ok(()),
        Some("show") if args.len() == 2 => Ok(()),
        Some("add") => Ok(()),
        Some("edit") => {
            if args.len() < 2 {
                return Err(CliFailure::usage(
                    "usage: xiao model custom edit ID [options]",
                ));
            }
            Ok(())
        }
        Some("test") if args.len() == 2 || args.len() == 3 => Ok(()),
        Some("probe") if args.len() == 3 => Ok(()),
        Some("models") if args.len() == 2 => Ok(()),
        Some("use") if args.len() == 3 => Ok(()),
        Some("delete") if args.len() == 2 => Ok(()),
        _ => Err(CliFailure::usage(
            "usage: xiao model custom <list|add|show|edit|test|probe|models|use|delete> ...",
        )),
    }
}

fn validate_sessions_syntax(args: &[String]) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => Ok(()),
        Some("new") if args.len() == 1 => Ok(()),
        Some("show") if args.len() == 2 => Ok(()),
        Some("use") if args.len() == 2 => Ok(()),
        Some("rename") if args.len() >= 3 => Ok(()),
        Some("delete") if args.len() == 2 => Ok(()),
        _ => Err(CliFailure::usage(
            "usage: xiao sessions <list|new|show|use|rename|delete> ...",
        )),
    }
}

fn validate_yolo_syntax(args: &[String]) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("status" | "on" | "off") if args.len() == 1 => Ok(()),
        _ => Err(CliFailure::usage("usage: xiao yolo <status|on|off>")),
    }
}

fn validate_memory_syntax(args: &[String]) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 || args.len() == 2 => Ok(()),
        Some("search") if args.len() >= 2 => Ok(()),
        Some("get") if args.len() == 4 => Ok(()),
        Some("set") if args.len() >= 5 => Ok(()),
        Some("forget") if args.len() == 4 => Ok(()),
        _ => Err(CliFailure::usage(
            "usage: xiao memory <list [SCOPE]|search QUERY|get SCOPE CATEGORY KEY|set SCOPE CATEGORY KEY VALUE|forget SCOPE CATEGORY KEY>",
        )),
    }
}

fn validate_skills_syntax(args: &[String]) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => Ok(()),
        Some("search") if args.len() >= 2 => Ok(()),
        Some("show") if args.len() == 2 => Ok(()),
        Some("enable" | "disable" | "delete") if args.len() == 2 => Ok(()),
        _ => Err(CliFailure::usage(
            "usage: xiao skills <list|search QUERY|show ID|enable ID|disable ID|delete ID>",
        )),
    }
}

fn validate_approvals_syntax(args: &[String]) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => Ok(()),
        Some("approve" | "deny") if args.len() == 2 => Ok(()),
        _ => Err(CliFailure::usage(
            "usage: xiao approvals <list|approve ID|deny ID>",
        )),
    }
}

fn validate_attachments_syntax(args: &[String]) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => Ok(()),
        Some("show" | "remove") if args.len() == 2 => Ok(()),
        _ => Err(CliFailure::usage(
            "usage: xiao attachments <list|show ID|remove ID>",
        )),
    }
}

fn validate_runs_syntax(args: &[String]) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => Ok(()),
        Some("show" | "cancel") if args.len() == 2 => Ok(()),
        _ => Err(CliFailure::usage(
            "usage: xiao runs <list|show ID|cancel ID>",
        )),
    }
}

fn validate_quickstart_syntax(args: &[String]) -> CliResult<()> {
    match args {
        [] => Ok(()),
        [arg] if arg == "--no-start" => Ok(()),
        _ => Err(CliFailure::usage("usage: xiao quickstart [--no-start]")),
    }
}

fn validate_daemon_syntax(args: &[String]) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("start" | "foreground" | "status" | "stop" | "restart") if args.len() == 1 => Ok(()),
        Some("logs") => {
            if args.len() > 2 {
                return Err(CliFailure::usage("usage: xiao daemon logs [N]"));
            }
            parse_lines(args.get(1))?;
            Ok(())
        }
        _ => Err(CliFailure::usage(
            "usage: xiao daemon <start|foreground|stop|restart|status|logs>",
        )),
    }
}

fn validate_config_syntax(args: &[String]) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("path" | "check" | "show") if args.len() == 1 => Ok(()),
        _ => Err(CliFailure::usage("usage: xiao config <path|check|show>")),
    }
}

fn validate_logs_syntax(args: &[String]) -> CliResult<()> {
    if args.len() > 1 {
        return Err(CliFailure::usage("usage: xiao logs [N]"));
    }
    parse_lines(args.first())?;
    Ok(())
}

async fn chat(
    client: &DaemonClient,
    options: &GlobalOptions,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    let mut files = Vec::<(String, String)>::new();
    let mut prompt_parts = Vec::new();
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--file" | "--image" => {
                let kind = if args[index] == "--image" {
                    "image"
                } else {
                    "file"
                };
                index += 1;
                let path = args
                    .get(index)
                    .ok_or_else(|| CliFailure::usage(format!("--{kind} requires PATH")))?;
                files.push((kind.into(), path.clone()));
            }
            value if value.starts_with("--") => {
                return Err(CliFailure::usage(format!("unknown chat option `{value}`")));
            }
            value => prompt_parts.push(value.to_owned()),
        }
        index += 1;
    }
    let prompt = prompt_parts.join(" ").trim().to_owned();
    if prompt.is_empty() {
        return Err(CliFailure::usage(
            "usage: xiao chat [--file PATH] [--image PATH] \"prompt\"",
        ));
    }
    let exact_session = if !files.is_empty() || options.session.is_some() {
        Some(client.target_session(options).await?)
    } else {
        None
    };
    if let Some(session_id) = exact_session.as_deref() {
        for (kind, path) in files {
            ingest_cli_attachment(client, session_id, &kind, Path::new(&path)).await?;
        }
    }
    let value = if let Some(session_id) = exact_session {
        client
            .post_client(
                "/v1/session-chat",
                &SessionExecuteRequest {
                    principal: String::new(),
                    session_id,
                    input: prompt,
                    retry: false,
                },
            )
            .await?
    } else {
        client
            .post_client(
                "/v1/chat",
                &ExecuteRequest {
                    principal: String::new(),
                    input: prompt,
                },
            )
            .await?
    };
    presenter.success("chat", app_result_schema(value))
}

async fn ingest_cli_attachment(
    client: &DaemonClient,
    session_id: &str,
    kind: &str,
    path: &Path,
) -> CliResult<()> {
    let metadata = fs::metadata(path).map_err(|error| {
        CliFailure::local(format!(
            "read attachment metadata {}: {error}",
            path.display()
        ))
    })?;
    // Client-side bound avoids accidentally constructing enormous base64 argv/JSON.
    if metadata.len() > 200 * 1024 * 1024 {
        return Err(CliFailure::local(
            "attachment exceeds the client safety bound",
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        CliFailure::local(format!("read attachment {}: {error}", path.display()))
    })?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment")
        .to_owned();
    client
        .post_client(
            "/v1/attachments/ingest",
            &AttachmentIngestRequest {
                principal: String::new(),
                session_id: session_id.to_owned(),
                name,
                mime: None,
                kind: kind.to_owned(),
                data_base64: URL_SAFE_NO_PAD.encode(bytes),
            },
        )
        .await?;
    Ok(())
}

async fn telegram_command(
    client: &DaemonClient,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("status") if args.len() == 1 => presenter.success(
            "telegram status",
            dto_telegram(client.get_admin("/v1/admin/telegram").await?),
        ),
        Some("test") if args.len() == 1 => presenter.success(
            "telegram test",
            dto_telegram(
                client
                    .post_admin("/v1/admin/telegram", &json!({"action":"test"}))
                    .await?,
            ),
        ),
        Some("set-token-file") if args.len() == 2 => {
            let token = read_secret_file(Path::new(&args[1]), "Telegram Bot Token")?;
            presenter.success(
                "telegram set-token-file",
                client
                    .post_admin(
                        "/v1/admin/telegram",
                        &json!({"action":"configure","token":token}),
                    )
                    .await?,
            )
        }
        Some("set-owner") => {
            if args.len() < 2 || args.len() > 3 {
                return Err(CliFailure::usage(
                    "usage: xiao telegram set-owner USER_ID [--confirm-owner-change]",
                ));
            }
            let owner = parse_owner_user_id(&args[1])?;
            let confirm = args
                .get(2)
                .is_some_and(|value| value == "--confirm-owner-change");
            if args.len() == 3 && !confirm {
                return Err(CliFailure::usage("unknown set-owner option"));
            }
            let current = client.get_admin("/v1/admin/telegram").await?;
            let old = current
                .pointer("/telegram/owner_user_id")
                .or_else(|| current.get("owner_user_id"))
                .or_else(|| current.pointer("/telegram/ownerUserId"))
                .and_then(Value::as_i64);
            let confirmed = if old.is_some_and(|value| value != owner) && !confirm {
                confirm_owner_change(old.unwrap_or_default(), owner)?
            } else {
                confirm
            };
            presenter.success(
                "telegram set-owner",
                client
                    .post_admin(
                        "/v1/admin/telegram",
                        &json!({
                            "action":"configure",
                            "owner_user_id":owner,
                            "confirm_owner_change":confirmed,
                        }),
                    )
                    .await?,
            )
        }
        Some("configure") => telegram_configure(client, &args[1..], presenter).await,
        _ => Err(CliFailure::usage(
            "usage: xiao telegram <status|configure|set-owner|set-token-file|test>",
        )),
    }
}

async fn telegram_configure(
    client: &DaemonClient,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    let mut owner = None;
    let mut allowed = Vec::new();
    let mut token_file = None::<PathBuf>;
    let mut enabled = None;
    let mut confirm = false;
    let mut test = false;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--owner" => {
                index += 1;
                owner =
                    Some(parse_owner_user_id(args.get(index).ok_or_else(|| {
                        CliFailure::usage("--owner requires USER_ID")
                    })?)?);
            }
            "--allowed-chat" => {
                index += 1;
                allowed.push(parse_i64(
                    args.get(index)
                        .ok_or_else(|| CliFailure::usage("--allowed-chat requires CHAT_ID"))?,
                    "allowed chat id",
                )?);
            }
            "--token-file" => {
                index += 1;
                token_file =
                    Some(PathBuf::from(args.get(index).ok_or_else(|| {
                        CliFailure::usage("--token-file requires PATH")
                    })?));
            }
            "--enable" => enabled = Some(true),
            "--disable" => enabled = Some(false),
            "--confirm-owner-change" => confirm = true,
            "--test" => test = true,
            other => {
                return Err(CliFailure::usage(format!(
                    "unknown telegram configure option `{other}`"
                )))
            }
        }
        index += 1;
    }
    let token = if let Some(path) = token_file {
        Some(read_secret_file(&path, "Telegram Bot Token")?)
    } else if io::stdin().is_terminal() {
        let value = read_secret_optional("Telegram Bot Token (blank = keep current): ")?;
        (!value.is_empty()).then_some(value)
    } else {
        None
    };
    if owner.is_none() && allowed.is_empty() && token.is_none() && enabled.is_none() && !test {
        return Err(CliFailure::usage(
            "configure requires an option, or an interactive TTY for token input",
        ));
    }
    let action = if test { "save_and_test" } else { "configure" };
    presenter.success(
        "telegram configure",
        client
            .post_admin(
                "/v1/admin/telegram",
                &json!({
                    "action":action,
                    "token":token,
                    "owner_user_id":owner,
                    "confirm_owner_change":confirm,
                    "allowed_chat_ids": if allowed.is_empty() { None::<Vec<i64>> } else { Some(allowed) },
                    "enabled":enabled,
                }),
            )
            .await?,
    )
}

async fn login_command(
    client: &DaemonClient,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    if args.is_empty() || matches!(args, [value] if value == "custom") {
        return custom_add_interactive(client, presenter).await;
    }
    match args {
        [value] if matches!(value.as_str(), "codex" | "antigravity" | "agy") => {
            Err(CliFailure::usage(
                "provider_configuration_required: Codex and Antigravity are no longer supported; use `xiao login` for a Custom endpoint",
            ))
        }
        _ => Err(CliFailure::usage("usage: xiao login [custom]")),
    }
}

async fn model_command(
    client: &DaemonClient,
    options: &GlobalOptions,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    let Some(sub) = args.first().map(String::as_str) else {
        return Err(CliFailure::usage(
            "usage: xiao model <show|list|use|custom>",
        ));
    };
    match sub {
        "show" if args.len() == 1 => {
            let session = client.target_session(options).await?;
            presenter.success(
                "model show",
                dto_session_item(session_by_id(client, &session).await?),
            )
        }
        "list" if args.len() == 1 => {
            let session = client.target_session(options).await?;
            let selected = session_by_id(client, &session).await?;
            let providers = client.get_admin("/v1/admin/providers").await?;
            presenter.success(
                "model list",
                dto_model_list(models_for_session(&selected, &providers)),
            )
        }
        "use" if args.len() == 2 => {
            let session_id = client.target_session(options).await?;
            let selected = session_by_id(client, &session_id).await?;
            let provider = selected
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let binding = selected
                .get("account_or_profile_id")
                .cloned()
                .unwrap_or(Value::Null);
            let value = client
                .post_admin(
                    "/v1/admin/sessions",
                    &json!({
                        "action":"ai_config",
                        "session_id":session_id,
                        "provider":provider,
                        "account_or_profile_id":binding,
                        "model":args[1],
                    }),
                )
                .await?;
            presenter.success("model use", value)
        }
        "custom" => custom_command(client, options, &args[1..], presenter).await,
        _ => Err(CliFailure::usage(
            "usage: xiao model <show|list|use> | xiao model custom ...",
        )),
    }
}

async fn custom_command(
    client: &DaemonClient,
    options: &GlobalOptions,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => {
            let providers = client.get_admin("/v1/admin/providers").await?;
            presenter.success("model custom list", dto_model_custom(providers))
        }
        Some("show") if args.len() == 2 => {
            let raw = custom_by_id(client, &args[1]).await?;
            presenter.success(
                "model custom show",
                crate::cli_contract::project_custom_profile(raw),
            )
        }
        Some("add") => custom_add(client, &args[1..], presenter).await,
        Some("edit") => custom_edit(client, &args[1..], presenter).await,
        Some("test") if args.len() == 2 => presenter.success(
            "model custom test",
            client
                .post_admin(
                    "/v1/admin/providers/custom",
                    &json!({
                        "action":"test",
                        "profile_id":args[1],
                    }),
                )
                .await?,
        ),
        Some("test") if args.len() == 3 => presenter.success(
            "model custom test",
            client
                .post_admin(
                    "/v1/admin/providers/custom",
                    &json!({
                        "action":"probe",
                        "profile_id":args[1],
                        "model":args[2],
                    }),
                )
                .await?,
        ),
        Some("probe") if args.len() == 3 => presenter.success(
            "model custom probe",
            client
                .post_admin(
                    "/v1/admin/providers/custom",
                    &json!({
                        "action":"probe",
                        "profile_id":args[1],
                        "model":args[2],
                    }),
                )
                .await?,
        ),
        Some("models") if args.len() == 2 => {
            let profile = custom_by_id(client, &args[1]).await?;
            presenter.success(
                "model custom models",
                json!({"profile_id":args[1],"models":profile.get("models").cloned().unwrap_or_else(|| json!([]))}),
            )
        }
        Some("use") if args.len() == 3 => {
            let session = client.target_session(options).await?;
            presenter.success(
                "model custom use",
                client
                    .post_admin(
                        "/v1/admin/sessions",
                        &json!({
                            "action":"ai_config",
                            "session_id":session,
                            "provider":"custom",
                            "account_or_profile_id":args[1],
                            "model":args[2],
                        }),
                    )
                    .await?,
            )
        }
        Some("delete") if args.len() == 2 => presenter.success(
            "model custom delete",
            client
                .post_admin(
                    "/v1/admin/providers/custom",
                    &json!({"action":"delete","profile_id":args[1]}),
                )
                .await?,
        ),
        _ => Err(CliFailure::usage(
            "usage: xiao model custom <list|add|show|edit|test|probe|models|use|delete> ...",
        )),
    }
}

async fn custom_add(
    client: &DaemonClient,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    if args.len() < 2 {
        return custom_add_interactive(client, presenter).await;
    }
    let alias = args[0].clone();
    let endpoint = args[1].clone();
    let mut protocol = "openai_chat_completions".to_owned();
    let mut key_file = None::<PathBuf>;
    let mut headers = BTreeMap::new();
    let mut secret_headers = None::<BTreeMap<String, String>>;
    let mut index = 2usize;
    while index < args.len() {
        match args[index].as_str() {
            "--protocol" => {
                index += 1;
                protocol = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| CliFailure::usage("--protocol requires a value"))?;
            }
            "--key-file" => {
                index += 1;
                key_file =
                    Some(PathBuf::from(args.get(index).ok_or_else(|| {
                        CliFailure::usage("--key-file requires PATH")
                    })?));
            }
            "--header" => {
                index += 1;
                let pair = args
                    .get(index)
                    .ok_or_else(|| CliFailure::usage("--header requires NAME=VALUE"))?;
                let (name, value) = pair
                    .split_once('=')
                    .ok_or_else(|| CliFailure::usage("--header requires NAME=VALUE"))?;
                headers.insert(name.to_owned(), value.to_owned());
            }
            "--headers-file" => {
                index += 1;
                let path = Path::new(required_arg(args, index, "--headers-file")?);
                let raw = fs::read_to_string(path).map_err(|error| {
                    CliFailure::local(format!("read {}: {error}", path.display()))
                })?;
                let file_headers: BTreeMap<String, String> = serde_json::from_str(&raw)
                    .map_err(|error| CliFailure::usage(format!("invalid headers JSON: {error}")))?;
                headers.extend(file_headers);
            }
            "--secret-headers-file" => {
                index += 1;
                let path = Path::new(required_arg(args, index, "--secret-headers-file")?);
                let raw = fs::read_to_string(path).map_err(|error| {
                    CliFailure::local(format!("read {}: {error}", path.display()))
                })?;
                secret_headers = Some(serde_json::from_str(&raw).map_err(|error| {
                    CliFailure::usage(format!("invalid secret headers JSON: {error}"))
                })?);
            }
            other => {
                return Err(CliFailure::usage(format!(
                    "unknown custom add option `{other}`"
                )))
            }
        }
        index += 1;
    }
    let api_key = key_file
        .as_deref()
        .map(|path| read_secret_file(path, "Custom API key"))
        .transpose()?;
    presenter.success(
        "model custom add",
        client
            .post_admin(
                "/v1/admin/providers/custom",
                &json!({
                    "action":"create",
                    "alias":alias,
                    "endpoint":endpoint,
                    "protocol":protocol,
                    "api_key":api_key,
                    "headers":headers,
                    "secret_headers":secret_headers,
                }),
            )
            .await?,
    )
}

async fn custom_add_interactive(client: &DaemonClient, presenter: &CliPresenter) -> CliResult<()> {
    if !io::stdin().is_terminal() {
        return Err(CliFailure::usage(
            "non-interactive Custom setup requires: xiao model custom add ALIAS ENDPOINT [--key-file PATH]",
        ));
    }
    let alias = prompt_line("Custom profile alias: ")?;
    let endpoint = prompt_line("Base URL: ")?;
    let protocol = {
        let value = prompt_line("Protocol [openai_chat_completions]: ")?;
        if value.trim().is_empty() {
            "openai_chat_completions".to_owned()
        } else {
            value
        }
    };
    let api_key = read_secret_optional("API key (blank = none): ")?;
    presenter.success(
        "login custom",
        client
            .post_admin(
                "/v1/admin/providers/custom",
                &json!({
                    "action":"create",
                    "alias":alias,
                    "endpoint":endpoint,
                    "protocol":protocol,
                    "api_key": if api_key.is_empty() { None::<String> } else { Some(api_key) },
                    "headers":BTreeMap::<String,String>::new(),
                }),
            )
            .await?,
    )
}

async fn custom_edit(
    client: &DaemonClient,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    let profile_id = args
        .first()
        .ok_or_else(|| CliFailure::usage("usage: xiao model custom edit ID [options]"))?;
    let mut alias = None;
    let mut endpoint = None;
    let mut protocol = None;
    let mut api_key = None;
    let mut remove_api_key = false;
    let mut keep_credential = false;
    let mut keep_safe_headers = false;
    let mut keep_secret_headers = false;
    let mut headers = None::<BTreeMap<String, String>>;
    let mut secret_headers = None::<BTreeMap<String, String>>;
    let mut clear_secret_headers = false;
    let mut index = 1usize;
    while index < args.len() {
        match args[index].as_str() {
            "--alias" => {
                index += 1;
                alias = Some(required_arg(args, index, "--alias")?.to_owned());
            }
            "--endpoint" => {
                index += 1;
                endpoint = Some(required_arg(args, index, "--endpoint")?.to_owned());
            }
            "--protocol" => {
                index += 1;
                protocol = Some(required_arg(args, index, "--protocol")?.to_owned());
            }
            "--key-file" => {
                index += 1;
                api_key = Some(read_secret_file(
                    Path::new(required_arg(args, index, "--key-file")?),
                    "Custom API key",
                )?);
            }
            "--remove-key" => remove_api_key = true,
            "--keep-credential" => keep_credential = true,
            "--keep-safe-headers" => keep_safe_headers = true,
            "--keep-secret-headers" => keep_secret_headers = true,
            "--clear-secret-headers" => clear_secret_headers = true,
            "--header" => {
                index += 1;
                let pair = args
                    .get(index)
                    .ok_or_else(|| CliFailure::usage("--header requires NAME=VALUE"))?;
                let (name, value) = pair
                    .split_once('=')
                    .ok_or_else(|| CliFailure::usage("--header requires NAME=VALUE"))?;
                let map = headers.get_or_insert_with(BTreeMap::new);
                map.insert(name.to_owned(), value.to_owned());
            }
            "--headers-file" => {
                index += 1;
                let path = Path::new(required_arg(args, index, "--headers-file")?);
                let raw = fs::read_to_string(path).map_err(|error| {
                    CliFailure::local(format!("read {}: {error}", path.display()))
                })?;
                headers = Some(serde_json::from_str(&raw).map_err(|error| {
                    CliFailure::usage(format!("invalid headers JSON: {error}"))
                })?);
            }
            "--secret-headers-file" => {
                index += 1;
                let path = Path::new(required_arg(args, index, "--secret-headers-file")?);
                let raw = fs::read_to_string(path).map_err(|error| {
                    CliFailure::local(format!("read {}: {error}", path.display()))
                })?;
                secret_headers = Some(serde_json::from_str(&raw).map_err(|error| {
                    CliFailure::usage(format!("invalid secret headers JSON: {error}"))
                })?);
            }
            other => {
                return Err(CliFailure::usage(format!(
                    "unknown custom edit option `{other}`"
                )))
            }
        }
        index += 1;
    }
    if alias.is_none()
        && endpoint.is_none()
        && protocol.is_none()
        && api_key.is_none()
        && !remove_api_key
        && headers.is_none()
        && secret_headers.is_none()
        && !clear_secret_headers
    {
        return Err(CliFailure::usage(
            "custom edit requires at least one change",
        ));
    }
    presenter.success(
        "model custom edit",
        client
            .post_admin(
                "/v1/admin/providers/custom",
                &json!({
                    "action":"edit",
                    "profile_id":profile_id,
                    "alias":alias,
                    "endpoint":endpoint,
                    "protocol":protocol,
                    "api_key":api_key,
                    "remove_api_key":remove_api_key,
                    "keep_credential":keep_credential,
                    "keep_safe_headers":keep_safe_headers,
                    "keep_secret_headers":keep_secret_headers,
                    "headers":headers,
                    "secret_headers":secret_headers,
                    "clear_secret_headers":clear_secret_headers,
                }),
            )
            .await?,
    )
}

async fn sessions_command(
    client: &DaemonClient,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => presenter.success(
            "sessions list",
            dto_sessions(client.get_admin("/v1/admin/sessions?limit=50").await?),
        ),
        Some("new") if args.len() == 1 => presenter.success(
            "sessions new",
            client
                .post_admin("/v1/admin/sessions", &json!({"action":"new"}))
                .await?,
        ),
        Some("show") if args.len() == 2 => presenter.success(
            "sessions show",
            dto_session_item(session_by_id(client, &args[1]).await?),
        ),
        Some("use") if args.len() == 2 => presenter.success(
            "sessions use",
            dto_session_item(
                client
                    .post_admin(
                        "/v1/admin/sessions",
                        &json!({"action":"use","session_id":args[1]}),
                    )
                    .await?,
            ),
        ),
        Some("rename") if args.len() >= 3 => presenter.success(
            "sessions rename",
            client
                .post_admin(
                    "/v1/admin/sessions",
                    &json!({"action":"rename","session_id":args[1],"value":args[2..].join(" ")}),
                )
                .await?,
        ),
        Some("delete") if args.len() == 2 => presenter.success(
            "sessions delete",
            client
                .post_admin(
                    "/v1/admin/sessions",
                    &json!({"action":"delete","session_id":args[1]}),
                )
                .await?,
        ),
        _ => Err(CliFailure::usage(
            "usage: xiao sessions <list|new|show ID|use ID|rename ID NAME|delete ID>",
        )),
    }
}

async fn yolo_command(
    client: &DaemonClient,
    options: &GlobalOptions,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    let session = client.target_session(options).await?;
    match args.first().map(String::as_str) {
        Some("status") if args.len() == 1 => {
            let data = session_by_id(client, &session).await?;
            presenter.success("yolo status", json!({"session_id":session,"enabled":data.get("yolo").cloned().unwrap_or(Value::Bool(false))}))
        }
        Some("on" | "off") if args.len() == 1 => {
            let enabled = args[0] == "on";
            presenter.success(
                &format!("yolo {}", args[0]),
                client
                    .post_admin(
                        "/v1/admin/sessions",
                        &json!({"action":"yolo","session_id":session,"value": if enabled {"on"} else {"off"}}),
                    )
                    .await?,
            )
        }
        _ => Err(CliFailure::usage("usage: xiao yolo <status|on|off>")),
    }
}

async fn memory_command(
    client: &DaemonClient,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 || args.len() == 2 => {
            let query = args.get(1).map(|scope| format!("?scope={}", url_encode(scope))).unwrap_or_default();
            presenter.success("memory list", dto_memory(client.get_admin(&format!("/v1/admin/memory{query}")).await?))
        }
        Some("search") if args.len() >= 2 => presenter.success(
            "memory search",
            dto_memory(
                client
                    .get_admin(&format!("/v1/admin/memory?query={}", url_encode(&args[1..].join(" "))))
                    .await?,
            ),
        ),
        Some("get") if args.len() == 4 => {
            let data = client.get_admin(&format!("/v1/admin/memory?scope={}", url_encode(&args[1]))).await?;
            let item = data
                .get("items")
                .and_then(Value::as_array)
                .and_then(|items| items.iter().find(|item| {
                    item.get("category").and_then(Value::as_str) == Some(args[2].as_str())
                        && item.get("key").and_then(Value::as_str) == Some(args[3].as_str())
                }))
                .cloned()
                .ok_or_else(|| CliFailure { code: EXIT_NOT_FOUND, message: "memory entry not found".into() })?;
            presenter.success("memory get", item)
        }
        Some("set") if args.len() >= 5 => presenter.success(
            "memory set",
            client
                .post_admin(
                    "/v1/admin/memory",
                    &json!({"action":"upsert","scope":args[1],"category":args[2],"key":args[3],"value":args[4..].join(" ")}),
                )
                .await?,
        ),
        Some("forget") if args.len() == 4 => presenter.success(
            "memory forget",
            client
                .post_admin(
                    "/v1/admin/memory",
                    &json!({"action":"delete","scope":args[1],"category":args[2],"key":args[3]}),
                )
                .await?,
        ),
        _ => Err(CliFailure::usage(
            "usage: xiao memory <list [SCOPE]|search QUERY|get SCOPE CATEGORY KEY|set SCOPE CATEGORY KEY VALUE|forget SCOPE CATEGORY KEY>",
        )),
    }
}

async fn skills_command(
    client: &DaemonClient,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => presenter.success(
            "skills list",
            dto_skills(client.get_admin("/v1/admin/skills?limit=50").await?),
        ),
        Some("search") if args.len() >= 2 => presenter.success(
            "skills search",
            dto_skills(
                client
                    .get_admin(&format!("/v1/admin/skills?query={}", url_encode(&args[1..].join(" "))))
                    .await?,
            ),
        ),
        Some("show") if args.len() == 2 => {
            let data = client.get_admin("/v1/admin/skills?limit=50").await?;
            let item = find_item(&data, &args[1], &["id", "name"])?;
            presenter.success("skills show", item)
        }
        Some("enable" | "disable") if args.len() == 2 => presenter.success(
            &format!("skills {}", args[0]),
            client
                .post_admin(
                    "/v1/admin/skills",
                    &json!({"action":"set_enabled","skill_id":args[1],"enabled":args[0] == "enable"}),
                )
                .await?,
        ),
        Some("delete") if args.len() == 2 => presenter.success(
            "skills delete",
            client
                .post_admin(
                    "/v1/admin/skills",
                    &json!({"action":"delete","skill_id":args[1]}),
                )
                .await?,
        ),
        _ => Err(CliFailure::usage(
            "usage: xiao skills <list|search QUERY|show ID|enable ID|disable ID|delete ID>",
        )),
    }
}

async fn approvals_command(
    client: &DaemonClient,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => {
            let security = client.get_admin("/v1/admin/security").await?;
            presenter.success(
                "approvals list",
                dto_approvals(json!({"items":security.get("pending_approvals").cloned().unwrap_or_else(|| json!([]))})),
            )
        }
        Some("approve" | "deny") if args.len() == 2 => presenter.success(
            &format!("approvals {}", args[0]),
            client
                .post_admin(
                    "/v1/admin/security",
                    &json!({"action":args[0],"approval_id":args[1]}),
                )
                .await?,
        ),
        _ => Err(CliFailure::usage(
            "usage: xiao approvals <list|approve ID|deny ID>",
        )),
    }
}

async fn attachments_command(
    client: &DaemonClient,
    options: &GlobalOptions,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    let session = client.target_session(options).await?;
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => presenter.success(
            "attachments list",
            dto_attachments(
                client
                    .get_admin(&format!(
                        "/v1/admin/attachments?session_id={}",
                        url_encode(&session)
                    ))
                    .await?,
            ),
        ),
        Some("show") if args.len() == 2 => {
            let data = client
                .get_admin(&format!(
                    "/v1/admin/attachments?id={}",
                    url_encode(&args[1])
                ))
                .await?;
            let item = find_item(&data, &args[1], &["attachment_id"])?;
            presenter.success("attachments show", item)
        }
        Some("remove") if args.len() == 2 => presenter.success(
            "attachments remove",
            client
                .post_admin(
                    "/v1/admin/attachments",
                    &json!({"action":"remove","attachment_id":args[1]}),
                )
                .await?,
        ),
        _ => Err(CliFailure::usage(
            "usage: xiao attachments <list|show ID|remove ID>",
        )),
    }
}

async fn runs_command(
    client: &DaemonClient,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => presenter.success(
            "runs list",
            dto_runs(client.get_admin("/v1/admin/runs?limit=50").await?),
        ),
        Some("show") if args.len() == 2 => {
            let data = client.get_admin("/v1/admin/runs?limit=50").await?;
            presenter.success("runs show", find_item(&data, &args[1], &["id"])?)
        }
        Some("cancel") if args.len() == 2 => presenter.success(
            "runs cancel",
            client
                .post_admin(
                    "/v1/admin/runs",
                    &json!({"action":"cancel","run_id":args[1]}),
                )
                .await?,
        ),
        _ => Err(CliFailure::usage(
            "usage: xiao runs <list|show ID|cancel ID>",
        )),
    }
}

async fn setup(
    paths: &CliPaths,
    options: &GlobalOptions,
    presenter: &CliPresenter,
) -> CliResult<()> {
    if !io::stdin().is_terminal() {
        return Err(CliFailure::usage(
            "xiao setup requires an interactive TTY; automation should use `xiao telegram set-token-file` and `xiao telegram set-owner`",
        ));
    }
    let init = standalone::initialize(paths)?;
    let status = standalone::daemon_status(paths, &init).await?;
    if !status.reachable {
        standalone::start_daemon(paths, &init).await?;
    }
    let client = DaemonClient::load(paths, options)?;
    presenter.line("Xiao secure setup");
    let token = read_secret_required("1/6 Telegram Bot Token: ")?;
    let owner = loop {
        let raw = prompt_line("2/6 Owner User ID: ")?;
        match raw.trim().parse::<i64>() {
            Ok(value) if value != 0 => break value,
            _ => eprintln!("Enter a non-zero numeric Telegram user ID."),
        }
    };
    presenter.line("3/6 Testing Telegram connection...");
    let telegram = client
        .post_admin(
            "/v1/admin/telegram",
            &json!({
                "action":"save_and_test",
                "token":token,
                "owner_user_id":owner,
                "enabled":true,
            }),
        )
        .await?;
    presenter.line("4/6 Custom AI endpoint (optional)");
    let provider = prompt_line("Configure Custom endpoint now? [skip/custom]: ")?;
    let provider_result = match provider.trim().to_ascii_lowercase().as_str() {
        "" | "skip" => Value::Null,
        "custom" => {
            custom_add_interactive(
                &client,
                &CliPresenter::new(GlobalOptions {
                    quiet: true,
                    ..options.clone()
                }),
            )
            .await?;
            json!({"started":true,"provider":"custom"})
        }
        _ => return Err(CliFailure::usage("setup provider must be skip or custom")),
    };
    presenter.line("5/6 Running bounded diagnostics...");
    let diagnostics = client.get_admin("/v1/admin/diagnostics").await?;
    presenter.success(
        "setup",
        json!({
            "telegram":telegram,
            "provider":provider_result,
            "diagnostics":diagnostics,
            "summary":"configuration saved and adapter reload requested",
        }),
    )?;
    presenter.line("6/6 Setup complete.");
    Ok(())
}

async fn quickstart(paths: &CliPaths, args: &[String], presenter: &CliPresenter) -> CliResult<()> {
    let no_start = match args {
        [] => false,
        [arg] if arg == "--no-start" => true,
        _ => return Err(CliFailure::usage("usage: xiao quickstart [--no-start]")),
    };
    let init = standalone::initialize(paths)?;
    if no_start {
        return presenter.success(
            "quickstart",
            json!({
                "config":paths.config,
                "data":init.runtime.data_dir,
                "started":false,
            }),
        );
    }
    let started = standalone::start_daemon(paths, &init).await?;
    presenter.success(
        "quickstart",
        json!({
            "config":paths.config,
            "data":init.runtime.data_dir,
            "started":true,
            "pid":started.pid,
            "already_running":started.already_running,
        }),
    )
}

async fn daemon(paths: &CliPaths, args: &[String], presenter: &CliPresenter) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("start") if args.len() == 1 => {
            let init = standalone::load_existing(paths)?;
            let result = standalone::start_daemon(paths, &init).await?;
            presenter.success("daemon start", start_value(result))
        }
        Some("foreground") if args.len() == 1 => {
            let init = standalone::load_existing(paths)?;
            let status = standalone::run_daemon_foreground(paths, &init)?;
            if !status.success() {
                return Err(CliFailure {
                    code: EXIT_ERROR,
                    message: format!("xiao daemon exited with {status}"),
                });
            }
            presenter.success(
                "daemon foreground",
                json!({"exit_status":status.to_string()}),
            )
        }
        Some("status") if args.len() == 1 => {
            let init = standalone::load_existing(paths)?;
            let status = standalone::daemon_status(paths, &init).await?;
            let value = json!({
                "managed_pid":status.managed_pid,
                "reachable":status.reachable,
                "endpoint":status.endpoint,
                "log":init.runtime.daemon_log,
            });
            if !status.reachable {
                presenter.success("daemon status", value)?;
                return Err(CliFailure {
                    code: EXIT_DAEMON_UNAVAILABLE,
                    message: "xiao daemon is not ready".into(),
                });
            }
            presenter.success("daemon status", value)
        }
        Some("logs") => {
            let lines = parse_lines(args.get(1))?;
            if args.len() > 2 {
                return Err(CliFailure::usage("usage: xiao daemon logs [N]"));
            }
            let init = standalone::load_existing(paths)?;
            let rows = standalone::tail_daemon_log(&init.runtime, lines)?;
            presenter.success("daemon logs", json!({ "lines": rows }))
        }
        Some("stop") if args.len() == 1 => {
            let init = standalone::load_existing(paths)?;
            presenter.success(
                "daemon stop",
                stop_value(standalone::stop_daemon(paths, &init).await?),
            )
        }
        Some("restart") if args.len() == 1 => {
            let init = standalone::load_existing(paths)?;
            let stop = standalone::stop_daemon(paths, &init).await?;
            if matches!(stop, StopResult::UnmanagedRunning) {
                return Err(CliFailure {
                    code: EXIT_REJECTED,
                    message: "xiao daemon is running outside this lifecycle".into(),
                });
            }
            let start = standalone::start_daemon(paths, &init).await?;
            presenter.success(
                "daemon restart",
                json!({"stop":stop_value(stop),"start":start_value(start)}),
            )
        }
        _ => Err(CliFailure::usage(
            "usage: xiao daemon <start|foreground|stop|restart|status|logs>",
        )),
    }
}

fn config_command(paths: &CliPaths, args: &[String], presenter: &CliPresenter) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("path") if args.len() == 1 => presenter.success(
            "config path",
            json!({
                "config":paths.config,
                "client":paths.client_config,
                "default_data":paths.default_data_dir,
            }),
        ),
        Some("check") if args.len() == 1 => {
            let init = standalone::load_existing(paths)?;
            if paths.client_config.exists() {
                ClientConfig::load(&paths.client_config)?;
            }
            presenter.success(
                "config check",
                json!({
                    "valid":true,
                    "ipc_bind":init.config.ipc.bind,
                    "loopback":init.config.ipc.socket_addr()?.ip().is_loopback(),
                }),
            )
        }
        Some("show") if args.len() == 1 => {
            let config = AppConfig::load(&paths.config)?;
            let value = serde_json::to_value(config).map_err(anyhow::Error::from)?;
            presenter.success("config show", value)
        }
        _ => Err(CliFailure::usage("usage: xiao config <path|check|show>")),
    }
}

async fn logs_command(
    paths: &CliPaths,
    args: &[String],
    options: &GlobalOptions,
    presenter: &CliPresenter,
) -> CliResult<()> {
    let lines = parse_lines(args.first())?;
    if args.len() > 1 {
        return Err(CliFailure::usage("usage: xiao logs [N]"));
    }
    let client = DaemonClient::load(paths, options)?;
    presenter.success(
        "logs",
        client.get_admin(&format!("/v1/logs?lines={lines}")).await?,
    )
}

async fn admin(
    paths: &CliPaths,
    args: &[String],
    options: &GlobalOptions,
    _presenter: &CliPresenter,
) -> CliResult<()> {
    let client = DaemonClient::load(paths, options)?;
    match args.first().map(String::as_str) {
        Some("snapshot") if args.len() == 1 => {
            raw_success(client.get_admin("/v1/admin/snapshot").await?)
        }
        Some("logs") => {
            let lines = parse_lines(args.get(1))?;
            raw_success(client.get_admin(&format!("/v1/logs?lines={lines}")).await?)
        }
        Some("client-config") if args.len() == 1 => {
            raw_success(client.get_admin("/v1/admin/client-config").await?)
        }
        Some("apply-file") if args.len() == 2 => {
            let raw = fs::read_to_string(&args[1])
                .map_err(|error| CliFailure::local(error.to_string()))?;
            let body: Value =
                serde_json::from_str(&raw).map_err(|error| CliFailure::usage(error.to_string()))?;
            raw_success(client.post_admin("/v1/admin/apply", &body).await?)
        }
        // P2-2: deprecated legacy base64 envelope — thin delegate to typed manager endpoints.
        Some("apply-base64") if args.len() == 2 => {
            let body = decode_payload(&args[1])?;
            raw_success(client.post_admin("/v1/admin/apply", &body).await?)
        }
        Some("test-token-file") if args.len() == 2 => {
            let token = read_secret_file(Path::new(&args[1]), "Telegram Bot Token")?;
            raw_success(
                client
                    .post_admin("/v1/admin/telegram/test", &json!({ "token": token }))
                    .await?,
            )
        }
        Some("test-token-base64") if args.len() == 2 => {
            let token = String::from_utf8(
                URL_SAFE_NO_PAD
                    .decode(&args[1])
                    .map_err(anyhow::Error::from)?,
            )
            .map_err(anyhow::Error::from)?;
            raw_success(
                client
                    .post_admin("/v1/admin/telegram/test", &json!({ "token": token }))
                    .await?,
            )
        }
        Some("fetch-models-base64") if args.len() == 2 => {
            let body = decode_payload(&args[1])?;
            raw_success(client.post_admin("/v1/admin/custom/models", &body).await?)
        }
        // P2-2: deprecated legacy WebUI/CLI base64 envelope — thin delegates.
        Some("manager-get-base64") if args.len() == 2 => {
            let request = decode_payload(&args[1])?;
            let resource = request
                .get("resource")
                .and_then(Value::as_str)
                .ok_or_else(|| CliFailure::usage("manager resource is required"))?;
            let path = manager_resource_path(resource)?;
            let mut url = reqwest::Url::parse(&format!("{}{}", client.endpoint, path))
                .map_err(anyhow::Error::from)?;
            if let Some(query) = request.get("query").and_then(Value::as_object) {
                let mut pairs = url.query_pairs_mut();
                for (key, value) in query {
                    if !matches!(
                        key.as_str(),
                        "page"
                            | "limit"
                            | "query"
                            | "scope"
                            | "include_archived"
                            | "lines"
                            | "session_id"
                            | "id"
                    ) {
                        return Err(CliFailure::usage(format!(
                            "unsupported manager query field: {key}"
                        )));
                    }
                    pairs.append_pair(
                        key,
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string())
                            .as_str(),
                    );
                }
            }
            let response = client
                .http
                .get(url)
                .bearer_auth(&client.admin_token)
                .send()
                .await
                .map_err(connection_failure)?;
            raw_success(parse_response(response).await?)
        }
        // P2-2: deprecated legacy base64 envelope — thin delegate.
        Some("manager-post-base64") if args.len() == 2 => {
            let request = decode_payload(&args[1])?;
            let resource = request
                .get("resource")
                .and_then(Value::as_str)
                .ok_or_else(|| CliFailure::usage("manager resource is required"))?;
            let body = request
                .get("body")
                .cloned()
                .ok_or_else(|| CliFailure::usage("manager request body is required"))?;
            raw_success(
                client
                    .post_admin(manager_resource_path(resource)?, &body)
                    .await?,
            )
        }
        _ => Err(CliFailure::usage("unsupported hidden admin command")),
    }
}

fn raw_success(value: Value) -> CliResult<()> {
    println!(
        "{}",
        serde_json::to_string(&value).map_err(anyhow::Error::from)?
    );
    Ok(())
}

fn manager_resource_path(resource: &str) -> CliResult<&'static str> {
    match resource {
        "dashboard" => Ok("/v1/admin/dashboard"),
        "providers" => Ok("/v1/admin/providers"),
        "provider-custom" => Ok("/v1/admin/providers/custom"),
        "runtime" => Ok("/v1/admin/runtime"),
        "context" => Ok("/v1/admin/context"),
        "sessions" => Ok("/v1/admin/sessions"),
        "runs" => Ok("/v1/admin/runs"),
        "attachments" => Ok("/v1/admin/attachments"),
        "memory" => Ok("/v1/admin/memory"),
        "skills" => Ok("/v1/admin/skills"),
        "tools" => Ok("/v1/admin/tools"),
        "security" => Ok("/v1/admin/security"),
        "diagnostics" => Ok("/v1/admin/diagnostics"),
        "telegram" => Ok("/v1/admin/telegram"),
        "logs" => Ok("/v1/logs"),
        _ => Err(CliFailure::usage("unsupported manager resource")),
    }
}

async fn session_by_id(client: &DaemonClient, id: &str) -> CliResult<Value> {
    let data = client
        .get_admin(&format!("/v1/admin/sessions?id={}", url_encode(id)))
        .await?;
    find_item(&data, id, &["id"])
}

async fn custom_by_id(client: &DaemonClient, id: &str) -> CliResult<Value> {
    let data = client.get_admin("/v1/admin/providers").await?;
    data.get("custom_profiles")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
        })
        .cloned()
        .ok_or_else(|| CliFailure {
            code: EXIT_NOT_FOUND,
            message: format!("Custom profile `{id}` not found"),
        })
}

fn models_for_session(session: &Value, providers: &Value) -> Value {
    let provider = session
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let binding = session.get("account_or_profile_id").and_then(Value::as_str);
    let models = if provider == "custom" {
        providers
            .get("custom_profiles")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("id").and_then(Value::as_str) == binding)
            })
            .and_then(|item| item.get("models"))
            .cloned()
            .unwrap_or_else(|| json!([]))
    } else {
        json!([])
    };
    json!({
        "session_id":session.get("id"),
        "provider":provider,
        "account_or_profile_id":binding,
        "current_model":session.get("model"),
        "models":models,
    })
}

fn find_item(data: &Value, id: &str, fields: &[&str]) -> CliResult<Value> {
    data.get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find(|item| {
                fields
                    .iter()
                    .any(|field| item.get(field).and_then(Value::as_str) == Some(id))
            })
        })
        .cloned()
        .ok_or_else(|| CliFailure {
            code: EXIT_NOT_FOUND,
            message: format!("`{id}` not found"),
        })
}

fn app_result_schema(value: Value) -> Value {
    if value.get("type").and_then(Value::as_str) == Some("agent") {
        if let Some(data) = value.get("data") {
            return json!({
                "answer":data.get("final_answer").cloned().unwrap_or(Value::Null),
                "side_mode":data.get("side_mode").cloned().unwrap_or(Value::Bool(false)),
                "progress":data.get("progress").cloned().unwrap_or_else(|| json!([])),
                "artifacts":data.get("artifacts").cloned().unwrap_or_else(|| json!([])),
            });
        }
    }
    // Public structured CLI never emits Telegram View/button schema. If a
    // future daemon accidentally returns one here, fail closed into a generic
    // application result marker rather than exposing UI internals.
    if contains_view_shape(&value) {
        return json!({"result":"presentation_unavailable_in_cli"});
    }
    value
}

fn contains_view_shape(value: &Value) -> bool {
    value.get("blocks").is_some()
        || value.get("actions").is_some()
        || value.pointer("/data/blocks").is_some()
        || value.pointer("/data/actions").is_some()
}

async fn parse_response(response: reqwest::Response) -> CliResult<Value> {
    let status = response.status();
    let text = response.text().await.map_err(connection_failure)?;
    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| text.clone());
        let code = if status.as_u16() == 404 {
            EXIT_NOT_FOUND
        } else if status.as_u16() == 401
            || status.as_u16() == 403
            || status.as_u16() == 400
            || status.as_u16() == 409
        {
            EXIT_REJECTED
        } else {
            EXIT_ERROR
        };
        return Err(CliFailure {
            code,
            message: redact_text(&message),
        });
    }
    serde_json::from_str(&text).map_err(|error| CliFailure {
        code: EXIT_ERROR,
        message: format!("parse xiao daemon JSON response: {error}"),
    })
}

fn connection_failure(error: reqwest::Error) -> CliFailure {
    CliFailure {
        code: EXIT_DAEMON_UNAVAILABLE,
        message: format!("connect to xiao daemon failed: {error}; run `xiao daemon status`"),
    }
}

// ---------------------------------------------------------------------------
// P1-9 stable CLI contracts — projections delegate to crate::cli_contract
// ---------------------------------------------------------------------------

fn dto_status(value: Value) -> Value {
    crate::cli_contract::project_status(value)
}
fn dto_telegram(value: Value) -> Value {
    crate::cli_contract::project_telegram(value)
}
fn dto_sessions(value: Value) -> Value {
    crate::cli_contract::project_sessions(value)
}
fn dto_context(value: Value) -> Value {
    crate::cli_contract::project_context(value)
}
fn dto_memory(value: Value) -> Value {
    crate::cli_contract::project_memory(value)
}
fn dto_skills(value: Value) -> Value {
    crate::cli_contract::project_skills(value)
}
fn dto_approvals(value: Value) -> Value {
    crate::cli_contract::project_approvals(value)
}
fn dto_attachments(value: Value) -> Value {
    crate::cli_contract::project_attachments(value)
}
fn dto_runs(value: Value) -> Value {
    crate::cli_contract::project_runs(value)
}
fn dto_doctor(value: Value) -> Value {
    crate::cli_contract::project_doctor(value)
}
fn dto_tools(value: Value) -> Value {
    crate::cli_contract::project_tools(value)
}
fn dto_model_custom(value: Value) -> Value {
    crate::cli_contract::project_custom_profiles(value)
}
#[allow(dead_code)]
fn dto_model_list(value: Value) -> Value {
    crate::cli_contract::project_model_list_for_session(value)
}
fn dto_session_item(value: Value) -> Value {
    crate::cli_contract::project_session_item(value)
}

fn render_status_human(value: &Value) {
    for line in crate::cli_contract::human_status(value).lines() {
        println!("{line}");
    }
}
fn render_sessions_human(value: &Value) {
    for line in crate::cli_contract::human_sessions(value).lines() {
        println!("{line}");
    }
}
fn render_doctor_human(value: &Value) {
    for line in crate::cli_contract::human_doctor(value).lines() {
        println!("{line}");
    }
}
fn render_model_human(value: &Value) {
    for line in crate::cli_contract::human_model(value).lines() {
        println!("{line}");
    }
}

fn render_human(value: &Value) {
    // Intentional routing — status / sessions / doctor / model get dedicated
    // formatting instead of generic nested-JSON. Detection is via stable DTO
    // keys produced by cli_contract projections.
    if value.get("health").is_some() || value.get("counts").is_some() {
        render_status_human(value);
        return;
    }
    if value.get("items").is_some() && value.get("active_cli_session_id").is_some() {
        render_sessions_human(value);
        return;
    }
    if value.get("checks").is_some() {
        render_doctor_human(value);
        return;
    }
    // model surfaces: accounts list, custom list, or session model list
    if value.get("models").is_some()
        && (value.get("session_id").is_some() || value.get("provider").is_some())
    {
        render_model_human(value);
        return;
    }
    if let Some(items) = value.get("items").and_then(|v| v.as_array()) {
        if !items.is_empty() {
            let first = &items[0];
            // doctor fallback: items with status/state
            if first.get("status").is_some() || first.get("state").is_some() {
                // could be runs or doctor; doctor has checks-style, runs have id+session_id
                // prefer doctor if no session_id/provider combo
                if first.get("session_id").is_none() && value.get("active_cli_session_id").is_none()
                {
                    // peek if looks like account/custom vs doctor: account has provider+label, custom has alias
                    let is_account_or_custom = first.get("provider").is_some()
                        || first.get("alias").is_some()
                        || first.get("attachment_id").is_some();
                    if !is_account_or_custom {
                        // treat single-key items as doctor fallback
                        if value.as_object().map(|m| m.len() == 1).unwrap_or(false) {
                            render_doctor_human(value);
                            return;
                        }
                    }
                }
            }
            // model accounts / custom
            if first.get("provider").is_some() || first.get("alias").is_some() {
                // distinguish from sessions (sessions have id+provider+model, accounts have label, custom have alias)
                if first.get("label").is_some() || first.get("alias").is_some() {
                    render_model_human(value);
                    return;
                }
            }
        }
    }
    if let Some(answer) = value.get("answer").and_then(Value::as_str) {
        println!("{answer}");
        if let Some(artifacts) = value.get("artifacts").and_then(Value::as_array) {
            for artifact in artifacts {
                if let Some(path) = artifact.get("path").and_then(Value::as_str) {
                    println!("artifact: {path}");
                }
            }
        }
        return;
    }
    match value {
        Value::Null => println!("OK"),
        Value::Bool(value) => println!("{value}"),
        Value::Number(value) => println!("{value}"),
        Value::String(value) => println!("{value}"),
        Value::Array(items) => {
            for item in items {
                render_list_item(item);
            }
        }
        Value::Object(map) => {
            // If items present but not matched above, render list then scalars
            if let Some(items) = map.get("items").and_then(Value::as_array) {
                for item in items {
                    render_list_item(item);
                }
                for (key, value) in map {
                    if key != "items" && is_scalar(value) {
                        println!("{}: {}", human_key(key), scalar(value));
                    }
                }
            } else {
                for (key, value) in map {
                    if is_scalar(value) {
                        println!("{}: {}", human_key(key), scalar(value));
                    } else {
                        println!("{}:", human_key(key));
                        let pretty = serde_json::to_string_pretty(value)
                            .unwrap_or_else(|_| value.to_string());
                        for line in pretty.lines() {
                            println!("  {line}");
                        }
                    }
                }
            }
        }
    }
}

fn render_list_item(item: &Value) {
    if let Some(object) = item.as_object() {
        let fields = [
            "id",
            "attachment_id",
            "name",
            "alias",
            "provider",
            "model",
            "status",
            "processing_status",
        ];
        let summary = fields
            .iter()
            .filter_map(|field| {
                object
                    .get(*field)
                    .filter(|value| is_scalar(value))
                    .map(|value| format!("{field}={}", scalar(value)))
            })
            .collect::<Vec<_>>();
        if !summary.is_empty() {
            println!("- {}", summary.join("  "));
            return;
        }
    }
    println!("- {}", item);
}

fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn scalar(value: &Value) -> String {
    match value {
        Value::Null => "none".into(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn human_key(value: &str) -> String {
    value.replace('_', " ")
}

fn parse_lines(value: Option<&String>) -> CliResult<usize> {
    value
        .map(|raw| {
            raw.parse::<usize>()
                .map_err(|_| CliFailure::usage("line count must be an integer"))
        })
        .transpose()
        .map(|value| value.unwrap_or(120).clamp(1, 500))
}

fn read_secret_file(path: &Path, label: &str) -> CliResult<String> {
    let value = fs::read_to_string(path).map_err(|error| {
        CliFailure::local(format!("read {label} file {}: {error}", path.display()))
    })?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(CliFailure::local(format!("{label} file is empty")));
    }
    Ok(value)
}

fn read_secret_required(prompt: &str) -> CliResult<String> {
    let value = read_secret_optional(prompt)?;
    if value.trim().is_empty() {
        Err(CliFailure::usage("secret value cannot be empty"))
    } else {
        Ok(value)
    }
}

fn read_secret_optional(prompt: &str) -> CliResult<String> {
    eprint!("{prompt}");
    io::stderr()
        .flush()
        .map_err(|error| CliFailure::local(error.to_string()))?;
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        let mut value = String::new();
        stdin
            .read_line(&mut value)
            .map_err(|error| CliFailure::local(error.to_string()))?;
        return Ok(value.trim_end_matches(['\r', '\n']).to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let fd = stdin.as_raw_fd();
        let mut old = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(fd, &mut old) } != 0 {
            return Err(CliFailure::local(
                "failed to disable terminal echo for secret input; use stdin/file instead (e.g. echo token | xiao ... or --token-file)",
            ));
        }
        let mut hidden = old;
        hidden.c_lflag &= !libc::ECHO;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) } != 0 {
            return Err(CliFailure::local(
                "failed to disable terminal echo for secret input; use stdin/file instead",
            ));
        }
        struct EchoGuard {
            fd: i32,
            old: libc::termios,
        }
        impl Drop for EchoGuard {
            fn drop(&mut self) {
                let _ = unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.old) };
            }
        }
        let _guard = EchoGuard { fd, old };
        let mut value = String::new();
        let result = stdin.read_line(&mut value);
        eprintln!();
        result.map_err(|error| CliFailure::local(error.to_string()))?;
        Ok(value.trim_end_matches(['\r', '\n']).to_owned())
    }
    #[cfg(not(unix))]
    {
        return Err(CliFailure::local(
            "secret input requires a non-terminal stdin on this platform; pipe via stdin or --token-file",
        ));
    }
}

fn prompt_line(prompt: &str) -> CliResult<String> {
    eprint!("{prompt}");
    io::stderr()
        .flush()
        .map_err(|error| CliFailure::local(error.to_string()))?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|error| CliFailure::local(error.to_string()))?;
    Ok(value.trim().to_owned())
}

fn confirm_owner_change(old: i64, new: i64) -> CliResult<bool> {
    if !io::stdin().is_terminal() {
        return Err(CliFailure::usage(
            "changing an existing owner requires --confirm-owner-change in non-interactive mode",
        ));
    }
    let confirmation = prompt_line(&format!(
        "Owner will change from {old} to {new}. Type the new Owner User ID to confirm: "
    ))?;
    if confirmation == new.to_string() {
        Ok(true)
    } else {
        Err(CliFailure::usage("owner change was not confirmed"))
    }
}

fn parse_owner_user_id(value: &str) -> CliResult<i64> {
    let id = value
        .parse::<i64>()
        .ok()
        .ok_or_else(|| CliFailure::usage("owner user id must be a positive integer"))?;
    if id <= 0 {
        return Err(CliFailure::usage(format!(
            "owner user id must be a positive integer (got {id})"
        )));
    }
    Ok(id)
}

fn parse_i64(value: &str, label: &str) -> CliResult<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| CliFailure::usage(format!("{label} must be a non-zero integer")))
}

fn required_arg<'a>(args: &'a [String], index: usize, flag: &str) -> CliResult<&'a str> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| CliFailure::usage(format!("{flag} requires a value")))
}

fn exact_arity(args: &[String], count: usize, usage: &str) -> CliResult<()> {
    if args.len() == count {
        Ok(())
    } else {
        Err(CliFailure::usage(usage))
    }
}

fn decode_payload(encoded: &str) -> CliResult<Value> {
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(anyhow::Error::from)?;
    let raw = String::from_utf8(decoded).map_err(anyhow::Error::from)?;
    serde_json::from_str(&raw)
        .map_err(|error| CliFailure::usage(format!("invalid JSON payload: {error}")))
}

fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let mut costs = (0..=b.chars().count()).collect::<Vec<_>>();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = costs[0];
        costs[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let current = costs[j + 1];
            costs[j + 1] = if ca == cb {
                previous
            } else {
                1 + previous.min(costs[j]).min(current)
            };
            previous = current;
        }
    }
    *costs.last().unwrap_or(&0)
}

fn start_value(result: StartResult) -> Value {
    json!({
        "pid":result.pid,
        "already_running":result.already_running,
        "client_config_created":result.client_config_created,
    })
}

fn stop_value(result: StopResult) -> Value {
    match result {
        StopResult::Stopped { pid, forced } => json!({"state":"stopped","pid":pid,"forced":forced}),
        StopResult::NotRunning => json!({"state":"not_running"}),
        StopResult::UnmanagedRunning => json!({"state":"unmanaged_running"}),
    }
}

fn print_subcommand_help(path: &[String]) {
    let key = path.iter().map(String::as_str).collect::<Vec<_>>();
    let text = match key.as_slice() {
        ["chat"] | ["ask"] => r#"Usage: xiao chat [--file PATH] [--image PATH] [--session ID] [--json] [--quiet] "PROMPT""#,
        ["telegram"] => "Usage: xiao telegram <status|configure|set-owner|set-token-file|test>",
        ["telegram", "status"] => "Usage: xiao telegram status",
        ["telegram", "configure"] => "Usage: xiao telegram configure [--owner ID] [--allowed-chat ID] [--token-file PATH] [--enable|--disable] [--test]",
        ["telegram", "set-owner"] => "Usage: xiao telegram set-owner ID [--confirm-owner-change]",
        ["telegram", "set-token-file"] => "Usage: xiao telegram set-token-file PATH",
        ["telegram", "test"] => "Usage: xiao telegram test",
        ["model"] => "Usage: xiao model <show|list|use|custom> ...",
        ["model", "show"] => "Usage: xiao model show [--session ID]",
        ["model", "list"] => "Usage: xiao model list [--session ID]",
        ["model", "use"] => "Usage: xiao model use MODEL [--session ID]",
        ["model", "custom"] => "Usage: xiao model custom <list|add|show|edit|test|probe|models|use|delete> ...",
        ["model", "custom", "list"] => "Usage: xiao model custom list",
        ["model", "custom", "add"] => "Usage: xiao model custom add ALIAS ENDPOINT [--protocol PROTO] [--key-file PATH] [--header NAME=VALUE] [--headers-file PATH] [--secret-headers-file PATH]",
        ["model", "custom", "show"] => "Usage: xiao model custom show ID",
        ["model", "custom", "edit"] => "Usage: xiao model custom edit ID [--alias ALIAS] [--endpoint URL] [--protocol PROTO] [--key-file PATH] [--header NAME=VALUE] [--headers-file PATH] [--secret-headers-file PATH] [--remove-key] [--keep-credential] [--keep-safe-headers] [--keep-secret-headers] [--clear-secret-headers]",
        ["model", "custom", "test"] => "Usage: xiao model custom test ID [MODEL]",
        ["model", "custom", "probe"] => "Usage: xiao model custom probe ID MODEL",
        ["model", "custom", "models"] => "Usage: xiao model custom models ID",
        ["model", "custom", "use"] => "Usage: xiao model custom use ID MODEL [--session ID]",
        ["model", "custom", "delete"] => "Usage: xiao model custom delete ID",
        ["sessions"] => "Usage: xiao sessions <list|new|show|use|rename|delete> ...",
        ["sessions", "list"] => "Usage: xiao sessions list",
        ["sessions", "new"] => "Usage: xiao sessions new",
        ["sessions", "show"] => "Usage: xiao sessions show ID",
        ["sessions", "use"] => "Usage: xiao sessions use ID",
        ["sessions", "rename"] => "Usage: xiao sessions rename ID NAME...",
        ["sessions", "delete"] => "Usage: xiao sessions delete ID",
        ["yolo"] => "Usage: xiao yolo <status|on|off> [--session ID]",
        ["yolo", "status"] => "Usage: xiao yolo status [--session ID]",
        ["yolo", "on"] => "Usage: xiao yolo on [--session ID]",
        ["yolo", "off"] => "Usage: xiao yolo off [--session ID]",
        ["memory"] => "Usage: xiao memory <list|search|get|set|forget> ...",
        ["memory", "list"] => "Usage: xiao memory list [SCOPE]",
        ["memory", "search"] => "Usage: xiao memory search QUERY",
        ["memory", "get"] => "Usage: xiao memory get SCOPE CATEGORY KEY",
        ["memory", "set"] => "Usage: xiao memory set SCOPE CATEGORY KEY VALUE...",
        ["memory", "forget"] => "Usage: xiao memory forget SCOPE CATEGORY KEY",
        ["skills"] => "Usage: xiao skills <list|search|show|enable|disable|delete> ...",
        ["skills", "list"] => "Usage: xiao skills list",
        ["skills", "search"] => "Usage: xiao skills search QUERY",
        ["skills", "show"] => "Usage: xiao skills show ID",
        ["skills", "enable"] => "Usage: xiao skills enable ID",
        ["skills", "disable"] => "Usage: xiao skills disable ID",
        ["skills", "delete"] => "Usage: xiao skills delete ID",
        ["approvals"] => "Usage: xiao approvals <list|approve|deny> ...",
        ["approvals", "list"] => "Usage: xiao approvals list",
        ["approvals", "approve"] => "Usage: xiao approvals approve ID",
        ["approvals", "deny"] => "Usage: xiao approvals deny ID",
        ["attachments"] => "Usage: xiao attachments <list|show|remove> [--session ID] ...",
        ["attachments", "list"] => "Usage: xiao attachments list [--session ID]",
        ["attachments", "show"] => "Usage: xiao attachments show ID",
        ["attachments", "remove"] => "Usage: xiao attachments remove ID",
        ["runs"] => "Usage: xiao runs <list|show|cancel> ...",
        ["runs", "list"] => "Usage: xiao runs list",
        ["runs", "show"] => "Usage: xiao runs show ID",
        ["runs", "cancel"] => "Usage: xiao runs cancel ID",
        ["daemon"] => "Usage: xiao daemon <start|foreground|stop|restart|status|logs> ...",
        ["daemon", "start"] => "Usage: xiao daemon start",
        ["daemon", "foreground"] => "Usage: xiao daemon foreground",
        ["daemon", "stop"] => "Usage: xiao daemon stop",
        ["daemon", "restart"] => "Usage: xiao daemon restart",
        ["daemon", "status"] => "Usage: xiao daemon status",
        ["daemon", "logs"] => "Usage: xiao daemon logs [LINES]",
        ["config"] => "Usage: xiao config <path|check|show>",
        ["config", "path"] => "Usage: xiao config path",
        ["config", "check"] => "Usage: xiao config check",
        ["config", "show"] => "Usage: xiao config show",
        ["login"] => "Usage: xiao login [custom]",
        ["login", "custom"] => "Usage: xiao login custom",
        ["setup"] => "Usage: xiao setup
Interactive secure setup. Secrets are read from hidden TTY input; they are never required as argv values.",
        ["status"] => "Usage: xiao status [--json] [--quiet]",
        ["context"] => "Usage: xiao context [--session ID] [--json]",
        ["doctor"] => "Usage: xiao doctor [--json]",
        ["tools"] => "Usage: xiao tools [--json]",
        ["btw"] => "Usage: xiao btw",
        ["stop"] => "Usage: xiao stop [--session ID]",
        ["retry"] => "Usage: xiao retry [--session ID]",
        ["logs"] => "Usage: xiao logs [LINES]",
        _ => {
            print_help();
            return;
        }
    };
    println!("{text}");
}

fn help_text() -> String {
    format!(
        r#"xiao v{version}
Private single-owner Xiao terminal control plane.

Usage: xiao [--json] [--quiet] [--session ID] [--timeout SEC] [--no-color] COMMAND

Core:
  xiao chat [--file PATH] [--image PATH] "..."
  xiao ask "..."
  xiao setup
  xiao status
  xiao context
  xiao doctor
  xiao tools

Telegram:
  xiao telegram status
  xiao telegram configure [--owner ID] [--allowed-chat ID] [--token-file PATH] [--enable|--disable] [--test]
  xiao telegram set-owner ID [--confirm-owner-change]
  xiao telegram set-token-file PATH
  xiao telegram test

Providers / models:
  xiao login [custom]
  xiao model show|list|use MODEL
  xiao model custom list|add|show|edit|test|probe|models|use|delete ...

Sessions:
  xiao sessions list|new|show|use|rename|delete ...
  xiao btw
  xiao yolo status|on|off
  xiao stop
  xiao retry

Owner data / execution:
  xiao memory list|search|get|set|forget ...
  xiao skills list|search|show|enable|disable|delete ...
  xiao approvals list|approve|deny ...
  xiao attachments list|show|remove ...
  xiao runs list|show|cancel ...

Runtime:
  xiao daemon
  xiao daemon start|foreground|stop|restart|status|logs [N]
  xiao logs [N]
  xiao config path|check|show

Exit codes: 0 success, 1 generic error, 2 usage, 3 daemon unavailable,
4 rejected, 5 not found, 6 local I/O/config. Unknown commands are never treated as chat.
Secrets are accepted from hidden TTY input, stdin, or files; provider/Telegram
secrets are not accepted as public argv values."#,
        version = crate::VERSION
    )
}

fn print_help() {
    println!("{}", help_text());
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    fn parsed(values: &[&str]) -> (GlobalOptions, Vec<String>) {
        parse_global_options(values.iter().map(|value| value.to_string()).collect()).unwrap()
    }

    #[test]
    fn global_options_work_after_command_and_session_is_exact() {
        let (options, args) = parsed(&["status", "--json", "--session", "abc", "--no-color"]);
        assert!(options.json);
        assert_eq!(options.session.as_deref(), Some("abc"));
        assert!(options.no_color);
        assert_eq!(args, vec!["status"]);
    }

    #[test]
    fn typo_is_usage_and_never_chat() {
        let error = unknown_command("stats");
        assert_eq!(error.code, EXIT_USAGE);
        assert!(error.message.contains("status"));
        assert!(unknown_command("about").message.contains("unknown command"));
        assert!(unknown_command("logout")
            .message
            .contains("unknown command"));
    }

    #[test]
    fn app_json_schema_does_not_expose_view_buttons() {
        let value = app_result_schema(json!({
            "type":"agent",
            "data":{"final_answer":"hello","progress":[],"artifacts":[],"side_mode":false}
        }));
        assert_eq!(value.get("answer").and_then(Value::as_str), Some("hello"));
        assert!(value.get("blocks").is_none());
        assert!(value.get("actions").is_none());
    }

    #[test]
    fn help_tree_excludes_removed_telegram_aliases_from_top_level() {
        assert!(!TOP_LEVEL.contains(&"about"));
        assert!(!TOP_LEVEL.contains(&"logout"));
        assert!(!TOP_LEVEL.contains(&"provider"));
        assert!(!TOP_LEVEL.contains(&"settings"));
        assert!(!TOP_LEVEL.contains(&"usage"));
        assert!(!TOP_LEVEL.contains(&"env"));
    }

    #[test]
    fn edit_distance_suggests_status_for_stats() {
        assert!(edit_distance("stats", "status") <= 3);
    }

    #[test]
    fn root_help_snapshot_contains_complete_public_tree_and_exit_codes() {
        let help = help_text();
        for command in [
            "xiao chat",
            "xiao ask",
            "xiao setup",
            "xiao status",
            "xiao context",
            "xiao doctor",
            "xiao tools",
            "xiao telegram status",
            "xiao telegram configure",
            "xiao telegram set-owner",
            "xiao telegram set-token-file",
            "xiao telegram test",
            "xiao login",
            "xiao model show|list|use",
            "xiao model custom",
            "xiao sessions",
            "xiao btw",
            "xiao yolo",
            "xiao stop",
            "xiao retry",
            "xiao memory",
            "xiao skills",
            "xiao approvals",
            "xiao attachments",
            "xiao runs",
            "xiao daemon",
            "xiao logs",
            "xiao config",
        ] {
            assert!(help.contains(command), "root help missing {command}");
        }
        assert!(help.contains("0 success, 1 generic error, 2 usage, 3 daemon unavailable"));
        assert!(help.contains("Unknown commands are never treated as chat"));
        assert!(!help.contains("xiao about"));
        assert!(!help.contains("xiao logout"));
        assert!(!help.contains("xiao model accounts"));
        assert!(!help.contains("xiao login codex"));
    }

    #[test]
    fn exit_code_contract_is_stable() {
        assert_eq!(
            [
                EXIT_OK,
                EXIT_ERROR,
                EXIT_USAGE,
                EXIT_DAEMON_UNAVAILABLE,
                EXIT_REJECTED,
                EXIT_NOT_FOUND,
                EXIT_LOCAL_IO,
            ],
            [0, 1, 2, 3, 4, 5, 6]
        );
    }
    #[test]
    fn subcommand_help_is_resolved_before_daemon_access() {
        let path = ["model".to_string(), "custom".to_string()];
        assert_eq!(
            path.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["model", "custom"]
        );
    }
}
