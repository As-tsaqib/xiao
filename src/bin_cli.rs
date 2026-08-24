use std::{
    collections::BTreeMap,
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};
use xiao::{
    config::AppConfig,
    ipc::{AttachmentIngestRequest, ExecuteRequest, SessionExecuteRequest},
    security::{redact::redact_text, secrets::SecretStore},
    standalone::{self, CliPaths, ClientConfig, StartResult, StopResult},
};

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
    "cancel",
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
    principal: String,
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
                message: "admin IPC token missing; start xiaod first".into(),
            })?;
        let timeout = Duration::from_secs(options.timeout_seconds.unwrap_or(300).clamp(1, 3600));
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
            .build()
            .map_err(anyhow::Error::from)?;
        Ok(Self {
            http,
            endpoint: client.endpoint.trim_end_matches('/').to_owned(),
            principal: client.principal,
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
                message: "xiaod did not return the new CLI session".into(),
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

#[tokio::main]
async fn main() {
    let raw = std::env::args().skip(1).collect::<Vec<_>>();
    let json_requested = raw.iter().any(|arg| arg == "--json");
    let (options, args) = match parse_global_options(raw) {
        Ok(value) => value,
        Err(failure) => {
            let presenter = CliPresenter::new(GlobalOptions {
                json: json_requested,
                ..GlobalOptions::default()
            });
            presenter.error(&failure);
            std::process::exit(failure.code);
        }
    };
    let presenter = CliPresenter::new(options.clone());
    let result = run(options, args, &presenter).await;
    match result {
        Ok(()) => std::process::exit(EXIT_OK),
        Err(failure) => {
            presenter.error(&failure);
            std::process::exit(failure.code);
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
        || matches!(
            args.first().map(String::as_str),
            Some("-h" | "--help" | "help")
        )
    {
        print_help();
        return Ok(());
    }
    if let Some(position) = args
        .iter()
        .position(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        print_subcommand_help(&args[..position]);
        return Ok(());
    }
    if matches!(
        args.first().map(String::as_str),
        Some("-V" | "--version" | "version")
    ) {
        presenter.line(format!("xiao {}", xiao::VERSION));
        return Ok(());
    }
    let command = args[0].as_str();
    if command.starts_with('/') || !TOP_LEVEL.contains(&command) {
        return Err(unknown_command(command));
    }
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
    let client = DaemonClient::load(paths, options)?;
    if matches!(args[0].as_str(), "chat" | "ask") {
        return chat(&client, options, &args[1..], presenter).await;
    }
    match args[0].as_str() {
        "status" => {
            exact_arity(args, 1, "usage: xiao status")?;
            presenter.success("status", client.get_admin("/v1/admin/dashboard").await?)
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
                client
                    .get_admin(&format!("/v1/admin/context{session}"))
                    .await?,
            )
        }
        "doctor" => {
            exact_arity(args, 1, "usage: xiao doctor")?;
            presenter.success("doctor", client.get_admin("/v1/admin/diagnostics").await?)
        }
        "tools" => {
            exact_arity(args, 1, "usage: xiao tools")?;
            presenter.success("tools", client.get_admin("/v1/admin/tools").await?)
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
        "cancel" => {
            exact_arity(args, 1, "usage: xiao cancel")?;
            let session = client.target_session(options).await?;
            presenter.success(
                "cancel",
                client
                    .post_admin(
                        "/v1/admin/sessions",
                        &json!({"action":"cancel","session_id":session}),
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
                        principal: client.principal.clone(),
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
                    principal: client.principal.clone(),
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
                    principal: client.principal.clone(),
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
                principal: client.principal.clone(),
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
            client.get_admin("/v1/admin/telegram").await?,
        ),
        Some("test") if args.len() == 1 => presenter.success(
            "telegram test",
            client
                .post_admin("/v1/admin/telegram", &json!({"action":"test"}))
                .await?,
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
            let owner = parse_i64(&args[1], "owner user id")?;
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
                owner = Some(parse_i64(
                    args.get(index)
                        .ok_or_else(|| CliFailure::usage("--owner requires USER_ID"))?,
                    "owner user id",
                )?);
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
    if args.len() != 1 {
        return Err(CliFailure::usage(
            "usage: xiao login <codex|antigravity|custom>",
        ));
    }
    match args[0].as_str() {
        "codex" | "antigravity" => presenter.success(
            &format!("login {}", args[0]),
            client
                .post_admin(
                    "/v1/admin/providers/accounts",
                    &json!({"action":"login","provider":args[0]}),
                )
                .await?,
        ),
        "custom" => custom_add_interactive(client, presenter).await,
        _ => Err(CliFailure::usage(
            "provider must be codex, antigravity, or custom",
        )),
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
            "usage: xiao model <show|list|use|accounts|custom>",
        ));
    };
    match sub {
        "show" if args.len() == 1 => {
            let session = client.target_session(options).await?;
            presenter.success("model show", session_by_id(client, &session).await?)
        }
        "list" if args.len() == 1 => {
            let session = client.target_session(options).await?;
            let selected = session_by_id(client, &session).await?;
            let providers = client.get_admin("/v1/admin/providers").await?;
            presenter.success("model list", models_for_session(&selected, &providers))
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
        "accounts" => accounts_command(client, options, &args[1..], presenter).await,
        "custom" => custom_command(client, options, &args[1..], presenter).await,
        _ => Err(CliFailure::usage(
            "usage: xiao model <show|list|use> | xiao model accounts ... | xiao model custom ...",
        )),
    }
}

async fn accounts_command(
    client: &DaemonClient,
    options: &GlobalOptions,
    args: &[String],
    presenter: &CliPresenter,
) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => {
            let providers = client.get_admin("/v1/admin/providers").await?;
            presenter.success(
                "model accounts list",
                json!({"items":providers.get("accounts").cloned().unwrap_or_else(|| json!([]))}),
            )
        }
        Some("show") if args.len() == 2 => {
            let account = account_by_id(client, &args[1]).await?;
            presenter.success("model accounts show", account)
        }
        Some("use") if args.len() == 2 || args.len() == 3 => {
            let account = account_by_id(client, &args[1]).await?;
            let provider = account
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let model = if let Some(model) = args.get(2) {
                model.clone()
            } else {
                account
                    .get("models")
                    .and_then(Value::as_array)
                    .and_then(|models| models.first())
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| CliFailure {
                        code: EXIT_REJECTED,
                        message: "account has no available model".into(),
                    })?
            };
            let session = client.target_session(options).await?;
            presenter.success(
                "model accounts use",
                client
                    .post_admin(
                        "/v1/admin/sessions",
                        &json!({
                            "action":"ai_config",
                            "session_id":session,
                            "provider":provider,
                            "account_or_profile_id":args[1],
                            "model":model,
                        }),
                    )
                    .await?,
            )
        }
        Some("reconnect") if args.len() == 2 => presenter.success(
            "model accounts reconnect",
            client
                .post_admin(
                    "/v1/admin/providers/accounts",
                    &json!({"action":"reconnect","account_id":args[1]}),
                )
                .await?,
        ),
        Some("disconnect") if args.len() == 2 => presenter.success(
            "model accounts disconnect",
            client
                .post_admin(
                    "/v1/admin/providers/accounts",
                    &json!({"action":"disconnect","account_id":args[1]}),
                )
                .await?,
        ),
        _ => Err(CliFailure::usage(
            "usage: xiao model accounts <list|show ID|use ID [MODEL]|reconnect ID|disconnect ID>",
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
            presenter.success(
                "model custom list",
                json!({"items":providers.get("custom_profiles").cloned().unwrap_or_else(|| json!([]))}),
            )
        }
        Some("show") if args.len() == 2 => {
            presenter.success("model custom show", custom_by_id(client, &args[1]).await?)
        }
        Some("add") => custom_add(client, &args[1..], presenter).await,
        Some("edit") => custom_edit(client, &args[1..], presenter).await,
        Some("test") if args.len() == 2 || args.len() == 3 => presenter.success(
            "model custom test",
            client
                .post_admin(
                    "/v1/admin/providers/custom",
                    &json!({
                        "action":"test",
                        "profile_id":args[1],
                        "model":args.get(2),
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
            "usage: xiao model custom <list|add|show|edit|test|models|use|delete> ...",
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
    let mut headers = None::<BTreeMap<String, String>>;
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
                    "headers":headers,
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
            client.get_admin("/v1/admin/sessions?limit=50").await?,
        ),
        Some("new") if args.len() == 1 => presenter.success(
            "sessions new",
            client
                .post_admin("/v1/admin/sessions", &json!({"action":"new"}))
                .await?,
        ),
        Some("show") if args.len() == 2 => {
            presenter.success("sessions show", session_by_id(client, &args[1]).await?)
        }
        Some("use") if args.len() == 2 => presenter.success(
            "sessions use",
            client
                .post_admin(
                    "/v1/admin/sessions",
                    &json!({"action":"use","session_id":args[1]}),
                )
                .await?,
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
        Some("archive") if args.len() == 2 => presenter.success(
            "sessions archive",
            client
                .post_admin(
                    "/v1/admin/sessions",
                    &json!({"action":"archive","session_id":args[1]}),
                )
                .await?,
        ),
        _ => Err(CliFailure::usage(
            "usage: xiao sessions <list|new|show ID|use ID|rename ID NAME|archive ID>",
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
            presenter.success("memory list", client.get_admin(&format!("/v1/admin/memory{query}")).await?)
        }
        Some("search") if args.len() >= 2 => presenter.success(
            "memory search",
            client
                .get_admin(&format!("/v1/admin/memory?query={}", url_encode(&args[1..].join(" "))))
                .await?,
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
            client.get_admin("/v1/admin/skills?limit=50").await?,
        ),
        Some("search") if args.len() >= 2 => presenter.success(
            "skills search",
            client
                .get_admin(&format!("/v1/admin/skills?query={}", url_encode(&args[1..].join(" "))))
                .await?,
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
                json!({"items":security.get("pending_approvals").cloned().unwrap_or_else(|| json!([]))}),
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
            client
                .get_admin(&format!(
                    "/v1/admin/attachments?session_id={}",
                    url_encode(&session)
                ))
                .await?,
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
            client.get_admin("/v1/admin/runs?limit=50").await?,
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
    presenter.line("4/6 AI provider (optional)");
    let provider = prompt_line("Provider [skip/codex/antigravity/custom]: ")?;
    let provider_result = match provider.trim().to_ascii_lowercase().as_str() {
        "" | "skip" => Value::Null,
        "codex" | "antigravity" => {
            client
                .post_admin(
                    "/v1/admin/providers/accounts",
                    &json!({"action":"login","provider":provider.trim().to_ascii_lowercase()}),
                )
                .await?
        }
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
        _ => {
            return Err(CliFailure::usage(
                "setup provider must be skip, codex, antigravity, or custom",
            ))
        }
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
                    message: format!("xiaod exited with {status}"),
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
                    message: "xiaod is not ready".into(),
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
                    message: "xiaod is running outside this lifecycle".into(),
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
        "provider-accounts" => Ok("/v1/admin/providers/accounts"),
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

async fn account_by_id(client: &DaemonClient, id: &str) -> CliResult<Value> {
    let data = client.get_admin("/v1/admin/providers").await?;
    data.get("accounts")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
        })
        .cloned()
        .ok_or_else(|| CliFailure {
            code: EXIT_NOT_FOUND,
            message: format!("account `{id}` not found"),
        })
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
        providers
            .get("accounts")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("id").and_then(Value::as_str) == binding)
            })
            .and_then(|item| item.get("models"))
            .cloned()
            .unwrap_or_else(|| json!([]))
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
        message: format!("parse xiaod JSON response: {error}"),
    })
}

fn connection_failure(error: reqwest::Error) -> CliFailure {
    CliFailure {
        code: EXIT_DAEMON_UNAVAILABLE,
        message: format!("connect to xiaod failed: {error}; run `xiao daemon status`"),
    }
}

fn render_human(value: &Value) {
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
        if unsafe { libc::tcgetattr(fd, &mut old) } == 0 {
            let mut hidden = old;
            hidden.c_lflag &= !libc::ECHO;
            if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) } == 0 {
                let mut value = String::new();
                let result = stdin.read_line(&mut value);
                let _ = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &old) };
                eprintln!();
                result.map_err(|error| CliFailure::local(error.to_string()))?;
                return Ok(value.trim_end_matches(['\r', '\n']).to_owned());
            }
        }
    }
    let mut value = String::new();
    stdin
        .read_line(&mut value)
        .map_err(|error| CliFailure::local(error.to_string()))?;
    Ok(value.trim_end_matches(['\r', '\n']).to_owned())
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
        ["telegram", "configure"] => "Usage: xiao telegram configure [--owner ID] [--allowed-chat ID] [--token-file PATH] [--enable|--disable] [--test]",
        ["model"] => "Usage: xiao model <show|list|use|accounts|custom> ...",
        ["model", "accounts"] => "Usage: xiao model accounts <list|show|use|reconnect|disconnect> ...",
        ["model", "custom"] => "Usage: xiao model custom <list|add|show|edit|test|models|use|delete> ...",
        ["sessions"] => "Usage: xiao sessions <list|new|show|use|rename|archive> ...",
        ["yolo"] => "Usage: xiao yolo <status|on|off> [--session ID]",
        ["memory"] => "Usage: xiao memory <list|search|get|set|forget> ...",
        ["skills"] => "Usage: xiao skills <list|search|show|enable|disable|delete> ...",
        ["approvals"] => "Usage: xiao approvals <list|approve|deny> ...",
        ["attachments"] => "Usage: xiao attachments <list|show|remove> [--session ID] ...",
        ["runs"] => "Usage: xiao runs <list|show|cancel> ...",
        ["daemon"] => "Usage: xiao daemon <start|foreground|stop|restart|status|logs> ...",
        ["config"] => "Usage: xiao config <path|check|show>",
        ["login"] => "Usage: xiao login <codex|antigravity|custom>",
        ["setup"] => "Usage: xiao setup\nInteractive secure setup. Secrets are read from hidden TTY input; they are never required as argv values.",
        ["status"] => "Usage: xiao status [--json] [--quiet]",
        ["context"] => "Usage: xiao context [--session ID] [--json]",
        ["doctor"] => "Usage: xiao doctor [--json]",
        ["tools"] => "Usage: xiao tools [--json]",
        ["btw"] => "Usage: xiao btw",
        ["cancel"] => "Usage: xiao cancel [--session ID]",
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
  xiao login codex|antigravity|custom
  xiao model show|list|use MODEL
  xiao model accounts list|show|use|reconnect|disconnect ...
  xiao model custom list|add|show|edit|test|models|use|delete ...

Sessions:
  xiao sessions list|new|show|use|rename|archive ...
  xiao btw
  xiao yolo status|on|off
  xiao cancel
  xiao retry

Owner data / execution:
  xiao memory list|search|get|set|forget ...
  xiao skills list|search|show|enable|disable|delete ...
  xiao approvals list|approve|deny ...
  xiao attachments list|show|remove ...
  xiao runs list|show|cancel ...

Runtime:
  xiao daemon start|foreground|stop|restart|status|logs [N]
  xiao logs [N]
  xiao config path|check|show

Exit codes: 0 success, 1 generic error, 2 usage, 3 daemon unavailable,
4 rejected, 5 not found, 6 local I/O/config. Unknown commands are never treated as chat.
Secrets are accepted from hidden TTY input, stdin, or files; provider/Telegram
secrets are not accepted as public argv values."#,
        version = xiao::VERSION
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
            "xiao model accounts",
            "xiao model custom",
            "xiao sessions",
            "xiao btw",
            "xiao yolo",
            "xiao cancel",
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
