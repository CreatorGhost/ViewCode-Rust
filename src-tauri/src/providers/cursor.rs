//! Cursor's own CLI over the shared ACP transport: `cursor-agent [--force] acp`.
//!
//! What the CLI does, as ViewCode's `cursor-agent` runtime records it:
//! - It keeps its own sign-in (`cursor-agent login`). `authenticate` with
//!   `cursor_login` goes out right after `initialize`, before any session call.
//! - With the `parameterizedModelPicker` client capability, the
//!   `cursor/list_available_models` extension lists base models with their own
//!   config options (reasoning, context, fast, thinking).
//! - Approvals are fixed at launch: `--force` approves everything, and without
//!   it every tool call asks through `session/request_permission`.
//! - Session modes are `agent`, `plan` and `ask` (read-only).
//! - `cursor/ask_question` and `cursor/create_plan` are extension requests.
use super::*;
use crate::slash_commands::claude::Pipe;

/// Where ViewCode sends people to install the CLI.
pub(crate) const INSTALL_DOCS: &str = "https://cursor.com/docs/cli/installation";
/// Shown when the CLI has no usable sign-in.
pub(super) const SIGN_IN: &str = "The Cursor CLI is not signed in. Run `cursor-agent login` in a terminal, then send again.";
/// A turn the stall watch ended.
pub(super) const STALLED: &str = "Cursor stopped responding; send it again.";
/// The process did not speak ACP: a CLI from before `cursor-agent acp`.
pub(super) const NOT_ACP: &str = "The Cursor CLI did not start its ACP server. It is probably too old: update it with `cursor-agent update` (or reinstall from https://cursor.com/docs/cli/installation), then try again.";
const AUTH_METHOD: &str = "cursor_login";
/// Awake time without a frame before a turn is ended. Model output streams, so
/// a working agent is rarely silent this long; a running tool or an unanswered
/// request pauses the watch.
pub(super) const STALL_TIMEOUT: Duration = Duration::from_secs(600);
const STARTUP: Duration = Duration::from_secs(60);
const ABOUT_TIMEOUT: Duration = Duration::from_secs(8);
/// Request ids for a pick's per-model options, clear of startup (1-8),
/// cancel (900), steering (2000+) and goals (10000+).
pub(super) const OPTION_IDS: std::ops::Range<u64> = 20..40;
/// Listed variants per model before only single changes from the default are listed.
const VARIANT_LIMIT: usize = 8;

#[derive(Default)]
pub(super) struct State {
    /// When the agent last wrote a frame, on the monotonic clock (it does not
    /// advance while the computer sleeps, so the sleep itself never counts).
    pub(super) last_frame: Option<tokio::time::Instant>,
}

pub(super) fn configure(command: &mut Command, config: &RunConfig) {
    // Full access is the CLI's own `--force`; every other mode asks, and the
    // approval policy in `acp.rs` answers for the selected mode.
    if config.mode == "full" {
        command.arg("--force");
    }
    command.arg("acp");
}

/// `initialize` for a Cursor session. The capability turns on the base-model
/// list with per-model options.
pub(super) fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"_meta":{"parameterizedModelPicker":true}},"clientInfo":{"name":"pandacode","version":"0.1.0"}}})
}

pub(super) fn authenticate() -> Value {
    json!({"jsonrpc":"2.0","id":7,"method":"authenticate","params":{"methodId":AUTH_METHOD}})
}

/// Outside Plan a turn runs in `agent`: `ask` is Cursor's read-only mode.
pub(super) fn mode_id(mode: &str) -> &'static str {
    if mode == "plan" {
        "plan"
    } else {
        "agent"
    }
}

/// Why the CLI could not use its sign-in. Only `SignedOut` means the user must
/// log in; the others clear once the Keychain is unlocked or the network is back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthProblem {
    SignedOut,
    CredentialsUnreadable,
    Network,
}

pub(crate) fn classify(text: &str) -> Option<AuthProblem> {
    let text = text.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| text.contains(word));
    if has(&["keychain", "errsec", "user interaction is not allowed", "-25308", "secitem"]) {
        return Some(AuthProblem::CredentialsUnreadable);
    }
    if has(&["enotfound", "econnrefused", "econnreset", "etimedout", "eai_again", "enetunreach", "enetdown", "getaddrinfo", "fetch failed", "network", "socket hang up", "offline"]) {
        return Some(AuthProblem::Network);
    }
    if has(&["not logged in", "not signed in", "login required", "log in required", "authentication required", "authenticate required", "unauthenticated", "unauthorized", "unauthorised", "agent login"]) {
        return Some(AuthProblem::SignedOut);
    }
    None
}

fn problem_message(problem: AuthProblem, detail: &str) -> String {
    let detail = model_text(Some(detail.lines().next().unwrap_or_default()), "", 300);
    let quoted = if detail.is_empty() { String::new() } else { format!(" (Cursor CLI: \"{detail}\")") };
    // Each message starts a new `cursor-agent`, which reads the saved sign-in again.
    match problem {
        AuthProblem::CredentialsUnreadable => format!("The Cursor CLI could not read its saved sign-in{quoted}. On a Mac this usually means the login Keychain is locked, for example after sleep. Unlock the Mac and send the message again."),
        AuthProblem::Network => format!("The Cursor CLI could not reach Cursor{quoted}, which often happens right after sleep or a network change. Send the message again once you are online."),
        AuthProblem::SignedOut => format!("The Cursor CLI reported that it is not signed in{quoted}. Send the message again; if it fails again, run `cursor-agent status` in a terminal, and `cursor-agent login` only if that says you are signed out."),
    }
}

/// The message for a failed JSON-RPC request, when Cursor's reason is one the
/// user can act on. Other failures keep the shared ACP wording.
pub(super) fn request_failure(frame: &Value) -> Option<String> {
    let error = &frame["error"];
    let data = &error["data"];
    let text = [error["message"].as_str(), data.as_str(), data["message"].as_str(), data["details"].as_str()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(problem) = classify(&text) {
        return Some(problem_message(problem, &text));
    }
    (frame["id"] == 7 || error["code"] == -32000).then(|| SIGN_IN.to_owned())
}

/// A run that ended without an answer: an old CLI, or one that lost its sign-in.
pub(super) fn run_failure(error: &str, diagnostic: &str) -> Option<String> {
    // An old CLI takes `acp` for a prompt and prints text, or rejects the command and exits.
    if error.starts_with("Invalid provider JSON") {
        return Some(NOT_ACP.into());
    }
    if !error.starts_with("Agent exited") {
        return None;
    }
    let lower = diagnostic.to_ascii_lowercase();
    if lower.contains("unknown command") || lower.contains("unknown option") {
        return Some(NOT_ACP.into());
    }
    classify(diagnostic).map(|problem| problem_message(problem, diagnostic.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default()))
}

/// What `cursor-agent about` says: the CLI version and whose sign-in it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct About {
    pub(crate) version: Option<String>,
    pub(crate) email: Option<String>,
    /// The subscription tier as Cursor names it (`pro`, `ultra`…), title-cased.
    pub(crate) plan: Option<String>,
    pub(crate) signed_in: Option<bool>,
}

fn logged_out(email: &str) -> bool {
    let lower = email.to_ascii_lowercase();
    lower == "not logged in" || lower.contains("login required") || lower.contains("authentication required")
}

fn plan_label(tier: &str) -> Option<String> {
    let words: Vec<String> = tier
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
            format!("{first}{}", chars.as_str().to_lowercase())
        })
        .collect();
    (!words.is_empty()).then(|| model_text(Some(&words.join(" ")), "", 80))
}

/// `about --format json` (`cliVersion`, `userEmail`, `subscriptionTier`), or
/// the older text form with `CLI Version` and `User Email` rows.
pub(crate) fn parse_about(stdout: &str, stderr: &str) -> About {
    let text = |value: &Value| value.as_str().map(str::trim).filter(|text| !text.is_empty()).map(|text| model_text(Some(text), "", 200));
    let from_email = |version: Option<String>, email: Option<String>, plan: Option<String>| match email {
        Some(email) if logged_out(&email) => About { version, email: None, plan: None, signed_in: Some(false) },
        Some(email) => About { version, email: Some(email), plan, signed_in: Some(true) },
        None => About { version, email: None, plan: None, signed_in: None },
    };
    if let Ok(Value::Object(record)) = serde_json::from_str::<Value>(stdout.trim()) {
        let version = record.get("cliVersion").and_then(text);
        let plan = record.get("subscriptionTier").and_then(Value::as_str).and_then(plan_label);
        if record.get("userEmail").is_some_and(Value::is_null) {
            return from_email(version, Some("not logged in".into()), plan);
        }
        return from_email(version, record.get("userEmail").and_then(text), plan);
    }
    // Strip ANSI colour codes, then read the two-column rows.
    let mut plain = String::new();
    let mut escape = false;
    for ch in format!("{stdout}\n{stderr}").chars() {
        if escape {
            escape = !ch.is_ascii_alphabetic();
        } else if ch == '\u{1b}' {
            escape = true;
        } else {
            plain.push(ch);
        }
    }
    let row = |key: &str| {
        plain.lines().find_map(|line| {
            let rest = line.trim_start().strip_prefix(key)?;
            (rest.starts_with("  ") || rest.starts_with('\t')).then(|| rest.trim()).filter(|value| !value.is_empty()).map(|value| model_text(Some(value), "", 200))
        })
    };
    from_email(row("CLI Version"), row("User Email"), None)
}

/// Runs `cursor-agent about`. Blocking: call from a worker thread.
pub(crate) fn about(path: &Path) -> Result<(About, String), String> {
    let run = |args: &[&str]| {
        let mut command = launch::std_command(path)?;
        crate::provider_settings::apply_environment("cursor", &mut command);
        command.args(args);
        let output = crate::workspace::run_process(command, "Cursor CLI", 64 * 1024, ABOUT_TIMEOUT)
            .map_err(|_| "The Cursor CLI did not answer `cursor-agent about`.".to_owned())?;
        Ok::<_, String>((String::from_utf8_lossy(&output.stdout.bytes).into_owned(), String::from_utf8_lossy(&output.stderr.bytes).into_owned()))
    };
    let (mut stdout, mut stderr) = run(&["about", "--format", "json"])?;
    // CLIs from before `--format` reject the flag.
    if format!("{stdout}\n{stderr}").to_ascii_lowercase().contains("'--format'") {
        (stdout, stderr) = run(&["about"])?;
    }
    let about = parse_about(&stdout, &stderr);
    Ok((about, format!("{stdout}\n{stderr}")))
}

/// Whether the CLI's config still names a signed-in account. Only the account
/// id in `~/.cursor/cli-config.json` is read; the token stays in the Keychain.
fn saved_login(home: Option<PathBuf>) -> bool {
    let Some(path) = home.map(|home| home.join(".cursor").join("cli-config.json")) else { return false };
    let Ok(metadata) = path.metadata() else { return false };
    if metadata.len() > 1024 * 1024 {
        return false;
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|config| config["authInfo"]["authId"].as_str().is_some_and(|id| !id.trim().is_empty()))
}

/// The Cursor card of the Usage view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Account {
    /// A usage status: `unsupported` (signed in, no limits to show), `signedOut`, `unavailable` or `error`.
    pub(crate) status: &'static str,
    pub(crate) message: String,
    pub(crate) email: Option<String>,
    pub(crate) plan: Option<String>,
    /// The CLI version, shown as where the account came from.
    pub(crate) version: Option<String>,
}

/// The sign-in state for the Usage view.
/// "Not logged in" is also what the CLI says when it cannot read its token (a
/// locked Keychain after sleep) or reach Cursor, so only a CLI with no saved
/// login and no stated reason counts as signed out.
pub(crate) fn account(about: &About, output: &str, saved_login: bool) -> Account {
    const NO_LIMITS: &str = "Cursor shows plan usage on its dashboard; the CLI's ACP server does not report limits.";
    let state = |status, message: &str| Account { status, message: message.into(), version: about.version.clone(), ..Account::default() };
    match about.signed_in {
        Some(true) => Account { email: about.email.clone(), plan: about.plan.clone(), ..state("unsupported", NO_LIMITS) },
        Some(false) => {
            let problem = classify(output).filter(|problem| *problem != AuthProblem::SignedOut);
            if problem.is_some() || saved_login {
                state("unavailable", "The Cursor CLI has a saved sign-in but could not confirm it just now. The Mac's login Keychain may be locked (for example after sleep) or the network may be down. If `cursor-agent status` in a terminal says you are signed out, run `cursor-agent login`.")
            } else {
                state("signedOut", "The Cursor CLI is not signed in. Run `cursor-agent login` in a terminal.")
            }
        }
        None => state("unavailable", "Could not verify the Cursor CLI sign-in."),
    }
}

/// Blocking: the Usage view's Cursor card, from `cursor-agent about`.
pub(crate) fn account_status() -> Account {
    let Some(path) = executable("cursor") else {
        return Account { status: "unavailable", message: format!("The Cursor CLI (`cursor-agent`) was not found. Install it ({INSTALL_DOCS}) or set its location in Settings."), ..Account::default() };
    };
    match about(&path) {
        Ok((about, output)) => account(&about, &output, saved_login(crate::agent_usage_providers::home())),
        Err(message) => Account { status: "error", message, ..Account::default() },
    }
}

/// ACP v1 names a config option's id `id`, later drafts `configId`; the shared
/// code reads `id`, so give every option one.
pub(super) fn normalize(config: &mut Value) {
    for option in config["configOptions"].as_array_mut().into_iter().flatten().take(64) {
        if option["id"].is_null() && option["configId"].is_string() {
            option["id"] = option["configId"].clone();
        }
    }
}

/// A pick such as `gpt-5.4[context=1m,fast=true]` names its base model first,
/// then the per-model options to set. `default` is Cursor's Auto.
pub(crate) fn split_model(model: &str) -> (&str, Vec<(&str, &str)>) {
    let model = model.trim();
    let model = if model.is_empty() { "default" } else { model };
    let Some((base, rest)) = model.split_once('[') else { return (model, Vec::new()) };
    let options = rest
        .trim_end_matches(']')
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .filter(|(key, value)| !key.is_empty() && !value.is_empty())
        .take(OPTION_IDS.end as usize - OPTION_IDS.start as usize)
        .collect();
    (base.trim(), options)
}

/// One per-model option that is not the reasoning effort: a select or a switch.
struct Knob {
    id: String,
    name: String,
    default: String,
    /// `(value, label)`; the label is what a variant's name gains.
    values: Vec<(String, String)>,
}

fn plain(text: &str) -> bool {
    !text.is_empty() && text.len() <= 64 && !text.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '[' | ']' | ',' | '='))
}

fn knob(option: &Value) -> Option<Knob> {
    let id = option["id"].as_str().or_else(|| option["configId"].as_str()).filter(|id| plain(id))?;
    let name = model_text(option["name"].as_str(), id, 80);
    if option["type"] == "boolean" {
        let default = option["currentValue"].as_bool()?;
        return Some(Knob { id: id.into(), default: default.to_string(), values: vec![("true".into(), name.clone()), ("false".into(), format!("{name} off"))], name });
    }
    if option["type"] != "select" {
        return None;
    }
    let default = option["currentValue"].as_str().filter(|value| plain(value))?;
    let values: Vec<(String, String)> = acp::choices(option)
        .into_iter()
        .filter_map(|choice| {
            let value = choice["value"].as_str().filter(|value| plain(value))?;
            let label = model_text(choice["name"].as_str(), value, 80);
            // "Off" or "None" alone says nothing next to a model name.
            let generic = ["off", "on", "none", "default", "true", "false"].contains(&label.to_ascii_lowercase().as_str());
            Some((value.to_owned(), if generic { format!("{name} {}", label.to_lowercase()) } else { label }))
        })
        .collect();
    // A long list (or one the default is missing from) stays as the session has it.
    (values.len() > 1 && values.len() <= 6 && values.iter().any(|(value, _)| value == default)).then(|| Knob { id: id.into(), name, default: default.into(), values })
}

/// `272k`, `1m` or a plain token count.
fn context_tokens(value: &str) -> Option<u64> {
    let lower = value.to_ascii_lowercase();
    let (digits, scale) = match lower.strip_suffix('k') {
        Some(digits) => (digits, 1_000.0),
        None => match lower.strip_suffix('m') {
            Some(digits) => (digits, 1_000_000.0),
            None => (lower.as_str(), 1.0),
        },
    };
    let tokens = digits.parse::<f64>().ok()? * scale;
    (1_000.0..=1_000_000_000.0).contains(&tokens).then_some(tokens as u64)
}

fn is_effort(option: &Value) -> bool {
    option["type"] == "select" && (option["category"] == "thought_level" || matches!(option["id"].as_str().or_else(|| option["configId"].as_str()), Some("reasoning" | "effort")))
}

/// The picker catalog from a `cursor/list_available_models` result. A model
/// with options of its own (context, fast, thinking) is listed once per
/// combination, each naming every option, so a pick never depends on what an
/// older session left set.
pub(super) fn catalog(response: &Value) -> Vec<ModelInfo> {
    let mut models = vec![ModelInfo { name: "Auto".into(), description: "Cursor picks the model".into(), ..ModelInfo::default_choice() }];
    let mut seen = HashSet::new();
    for entry in response["models"].as_array().into_iter().flatten().take(MODEL_CATALOG_LIMIT) {
        let slug = field(entry, "value").trim();
        let name = model_text(entry["name"].as_str(), "", 160);
        if !plain(slug) || name.is_empty() || !seen.insert(slug.to_owned()) {
            continue;
        }
        let options: Vec<&Value> = entry["configOptions"]
            .as_array()
            .into_iter()
            .flatten()
            .take(32)
            // The session mode follows the task's mode, not a per-model choice.
            .filter(|option| !matches!(option["category"].as_str(), Some("mode" | "model")))
            .collect();
        let effort = options.iter().find(|option| is_effort(option));
        let reasoning_efforts: Vec<ReasoningEffortInfo> = effort
            .map(|option| acp::choices(option))
            .unwrap_or_default()
            .into_iter()
            .take(16)
            .filter_map(|choice| {
                let id = choice["value"].as_str()?;
                validate_reasoning_effort("cursor", Some(id)).ok()?;
                Some(ReasoningEffortInfo { id: id.into(), name: model_text(choice["name"].as_str(), id, 80), description: model_text(choice["description"].as_str(), "", 500) })
            })
            .collect();
        let default_reasoning_effort = effort
            .and_then(|option| option["currentValue"].as_str())
            .filter(|value| reasoning_efforts.iter().any(|effort| effort.id == *value))
            .map(str::to_owned);
        if slug == "default" {
            models[0].name = name;
            models[0].reasoning_efforts = reasoning_efforts;
            models[0].default_reasoning_effort = default_reasoning_effort;
            continue;
        }
        let knobs: Vec<Knob> = options.iter().filter(|option| !is_effort(option)).filter_map(|option| knob(option)).take(6).collect();
        for values in combinations(&knobs) {
            let id = if knobs.is_empty() {
                slug.to_owned()
            } else {
                format!("{slug}[{}]", knobs.iter().zip(&values).map(|(knob, (value, _))| format!("{}={value}", knob.id)).collect::<Vec<_>>().join(","))
            };
            if !valid_model_id(&id) || models.len() > MODEL_CATALOG_LIMIT {
                continue;
            }
            let changed: Vec<&str> = knobs.iter().zip(&values).filter(|(knob, (value, _))| knob.default != *value).map(|(_, (_, label))| label.as_str()).collect();
            let context = knobs.iter().zip(&values).find(|(knob, _)| knob.id == "context").and_then(|(_, (value, _))| context_tokens(value));
            models.push(ModelInfo {
                id,
                name: std::iter::once(name.as_str()).chain(changed).collect::<Vec<_>>().join(" · "),
                description: knobs.iter().zip(&values).map(|(knob, (value, _))| format!("{}: {value}", knob.name)).collect::<Vec<_>>().join(", "),
                reasoning_efforts: reasoning_efforts.clone(),
                default_reasoning_effort: default_reasoning_effort.clone(),
                context_window: context,
            });
        }
    }
    models
}

/// Every combination of the knobs' values, defaults first; past
/// `VARIANT_LIMIT` only the default and each single change from it.
fn combinations(knobs: &[Knob]) -> Vec<Vec<(String, String)>> {
    let pick = |knob: &Knob, value: &str| knob.values.iter().find(|(candidate, _)| candidate == value).cloned().unwrap_or_else(|| (value.to_owned(), value.to_owned()));
    let defaults: Vec<(String, String)> = knobs.iter().map(|knob| pick(knob, &knob.default)).collect();
    let total: usize = knobs.iter().map(|knob| knob.values.len()).product();
    let mut all = vec![defaults.clone()];
    if total <= VARIANT_LIMIT {
        let mut product: Vec<Vec<(String, String)>> = vec![Vec::new()];
        for knob in knobs {
            product = product.into_iter().flat_map(|prefix| knob.values.iter().map(move |value| { let mut next = prefix.clone(); next.push(value.clone()); next })).collect();
        }
        all.extend(product.into_iter().filter(|values| *values != defaults));
    } else {
        for (index, knob) in knobs.iter().enumerate() {
            for value in knob.values.iter().filter(|(value, _)| *value != knob.default) {
                let mut next = defaults.clone();
                next[index] = value.clone();
                all.push(next);
            }
        }
    }
    all
}

fn transport(pipe: &Pipe) -> String {
    if pipe.timed_out() {
        acp::ACP_STARTUP_TIMEOUT.into()
    } else {
        NOT_ACP.into()
    }
}

fn command(path: &Path) -> Result<Command, String> {
    let mut command = super::provider_command("cursor", path)?;
    command.arg("acp");
    Ok(command)
}

/// What one discovery session is opened for.
enum Probe {
    Models,
    Commands,
}

/// `initialize`, `authenticate`, `session/new`, then either the model list or
/// the session's command list. No prompt is sent and no agent request is run.
async fn probe(path: &Path, cwd: &Path, want: Probe) -> Result<Value, String> {
    let mut pipe = Pipe::spawn(command(path)?, cwd, STARTUP).map_err(|_| "Could not start the Cursor CLI. Check the selected executable.".to_owned())?;
    let result = async {
        pipe.send(&initialize()).await.map_err(|_| transport(&pipe))?;
        let mut session = String::new();
        let mut commands = None;
        loop {
            let frame = match pipe.read().await {
                Ok(frame) => frame,
                // The command list is optional once the session is open.
                Err(()) if matches!(want, Probe::Commands) && !session.is_empty() => return Ok(Value::Null),
                Err(()) => return Err(transport(&pipe)),
            };
            if frame.get("method").is_some() && frame.get("id").is_some() {
                let _ = pipe.send(&json!({"jsonrpc":"2.0","id":frame["id"],"error":{"code":-32601,"message":"Metadata discovery does not execute requests"}})).await;
            } else if frame["method"] == "session/update" && frame["params"]["update"]["sessionUpdate"] == "available_commands_update" {
                commands = Some(frame);
            } else if frame.get("error").is_some() && matches!(frame["id"].as_u64(), Some(1 | 2 | 3 | 7)) {
                return Err(request_failure(&frame).unwrap_or_else(|| acp::rpc_error(&frame, "discovery")));
            } else if frame["id"] == 1 {
                if frame["result"]["protocolVersion"] != 1 {
                    return Err("This Cursor CLI does not support ACP protocol v1. Update it with `cursor-agent update`.".into());
                }
                pipe.send(&authenticate()).await.map_err(|_| transport(&pipe))?;
            } else if frame["id"] == 7 {
                pipe.send(&json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":launch::external_path(cwd),"mcpServers":[]}})).await.map_err(|_| transport(&pipe))?;
            } else if frame["id"] == 2 {
                session = field(&frame["result"], "sessionId").to_owned();
                if session.is_empty() || session.len() > 256 {
                    return Err("The Cursor CLI did not return a session ID.".into());
                }
                match want {
                    Probe::Models => pipe.send(&json!({"jsonrpc":"2.0","id":3,"method":"cursor/list_available_models","params":{}})).await.map_err(|_| transport(&pipe))?,
                    Probe::Commands => pipe.set_deadline_after(Duration::from_secs(1)),
                }
            } else if frame["id"] == 3 {
                return Ok(frame["result"].clone());
            }
            if matches!(want, Probe::Commands) && !session.is_empty() {
                if let Some(frame) = commands.take_if(|frame| frame["params"]["sessionId"] == session.as_str()) {
                    return Ok(frame);
                }
            }
        }
    }
    .await;
    pipe.finish().await;
    result.map_err(|error| super::diagnostics::failure(&error, pipe.diagnostic()))
}

/// An empty folder for discovery sessions, so the CLI indexes nothing.
fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join("pandacode-cursor-discovery");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Base models with their options. `session/new` plus the extension call is a
/// full CLI start, so the list is cached until the binary changes or 5 minutes pass.
pub(super) async fn models(path: &Path) -> Result<Vec<ModelInfo>, String> {
    if let Some(models) = acp_catalog::get("cursor", path) {
        return Ok(models);
    }
    let _refresh = acp_catalog::refresh_lock("cursor", path).await;
    if let Some(models) = acp_catalog::get("cursor", path) {
        return Ok(models);
    }
    let discovery = acp_catalog::begin("cursor", path);
    let response = probe(path, &scratch(), Probe::Models).await?;
    let models = catalog(&response);
    if models.len() <= 1 {
        return Err("Couldn't reach Cursor to list models. Check `cursor-agent status`, then refresh.".into());
    }
    acp_catalog::store_models(discovery, models.clone());
    Ok(models)
}

/// The `available_commands_update` frame for a project folder, if Cursor sends one.
pub(crate) async fn commands(path: &Path, cwd: &Path) -> Result<Option<Value>, String> {
    Ok(Some(probe(path, cwd, Probe::Commands).await?).filter(|frame| !frame.is_null()))
}

impl Protocol {
    /// When a silent turn is given up on, if nothing explains the silence.
    pub(super) fn cursor_stall_at(&self) -> Option<tokio::time::Instant> {
        if self.config.provider != "cursor" || self.phase != 1 {
            return None;
        }
        let busy = !self.pending.is_empty() || !self.inputs.is_empty() || self.tools.values().any(|tool| tool.status == "running");
        if busy {
            return None;
        }
        self.cursor.last_frame.map(|at| at + STALL_TIMEOUT)
    }

    /// Sets the base model, `default` (Cursor's Auto) included, so a task that
    /// goes back to Auto leaves the model an earlier turn chose.
    pub(super) fn cursor_model(&mut self, step: &mut Step) -> Result<(), String> {
        let model = self.config.model.clone();
        let (base, _) = split_model(&model);
        if let Some(option) = acp::options(&self.acp_config, "model") {
            let offered = acp::choices(option).iter().any(|choice| choice["value"] == base);
            if option["currentValue"] == base || (base == "default" && !offered) {
                return self.cursor_options(0, step);
            }
            if !offered {
                return Err(format!("Cursor does not offer the model \"{base}\" to this account. Refresh the model list and choose another."));
            }
            step.write.push(json!({"jsonrpc":"2.0","id":5,"method":"session/set_config_option","params":{"sessionId":self.remote_id,"configId":option["id"],"value":base}}));
            return Ok(());
        }
        let models = &self.acp_config["models"];
        if let Some(available) = models["availableModels"].as_array() {
            if models["currentModelId"] == base {
                return self.cursor_options(0, step);
            }
            if available.iter().any(|model| model["modelId"] == base) {
                step.write.push(json!({"jsonrpc":"2.0","id":5,"method":"session/set_model","params":{"sessionId":self.remote_id,"modelId":base}}));
                return Ok(());
            }
        }
        if base == "default" {
            return self.cursor_options(0, step);
        }
        // A restored session may not list its models again; Cursor still takes the option.
        step.write.push(json!({"jsonrpc":"2.0","id":5,"method":"session/set_config_option","params":{"sessionId":self.remote_id,"configId":"model","value":base}}));
        Ok(())
    }

    /// Sets the pick's per-model options one at a time, from index `from`, then
    /// the reasoning effort. An option already at its value is skipped.
    pub(super) fn cursor_options(&mut self, from: usize, step: &mut Step) -> Result<(), String> {
        let model = self.config.model.clone();
        let (_, wanted) = split_model(&model);
        for (index, (key, value)) in wanted.into_iter().enumerate().skip(from) {
            let option = self.acp_config["configOptions"].as_array().into_iter().flatten().take(64).find(|option| option["id"] == key);
            let Some(option) = option else {
                return Err(format!("Cursor does not offer the \"{key}\" setting for this model. Refresh the model list and choose the model again."));
            };
            let id = OPTION_IDS.start + index as u64;
            let mut params = json!({"sessionId":self.remote_id,"configId":key});
            if option["type"] == "boolean" {
                let on = value == "true";
                if option["currentValue"] == on {
                    continue;
                }
                params["type"] = json!("boolean");
                params["value"] = json!(on);
            } else {
                if option["currentValue"] == value {
                    continue;
                }
                if !acp::choices(option).iter().any(|choice| choice["value"] == value) {
                    return Err(format!("Cursor does not offer \"{value}\" for the \"{key}\" setting of this model. Refresh the model list and choose the model again."));
                }
                params["value"] = json!(value);
            }
            step.write.push(json!({"jsonrpc":"2.0","id":id,"method":"session/set_config_option","params":params}));
            return Ok(());
        }
        self.acp_effort(step)
    }

    /// `cursor/ask_question` becomes the shared question form.
    pub(super) fn cursor_question(&mut self, value: &Value, step: &mut Step) {
        let params = &value["params"];
        let header = model_text(params["title"].as_str(), "Question", 120);
        let questions: Vec<Value> = params["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|question| {
                let mut options: Vec<Value> = question["options"].as_array().into_iter().flatten().map(|option| json!({"label":option["label"],"description":""})).collect();
                if options.is_empty() {
                    options.push(json!({"label":"OK","description":"Continue"}));
                }
                json!({"id":question["id"],"header":header,"question":question["prompt"],"isOther":true,"isSecret":false,"options":options})
            })
            .collect();
        let before = step.write.len();
        self.input_request(json!({"id":value["id"],"method":"item/tool/requestUserInput","params":{"questions":questions}}), step);
        if let Some(refused) = step.write.get_mut(before) {
            // The form was refused; the JSON-RPC error still has to be well formed.
            refused["jsonrpc"] = json!("2.0");
            return;
        }
        let multiple: HashSet<&str> = params["questions"].as_array().into_iter().flatten().filter(|question| question["allowMultiple"] == true).filter_map(|question| question["id"].as_str()).collect();
        let Some(event) = step.events.last_mut().filter(|event| event.kind == "input_request") else { return };
        let id = event.request_id.clone().unwrap_or_default();
        let mark = |questions: &mut Value| {
            for question in questions.as_array_mut().into_iter().flatten() {
                if question["id"].as_str().is_some_and(|id| multiple.contains(id)) {
                    question["multiSelect"] = json!(true);
                    question["minSelections"] = json!(1);
                }
            }
        };
        if let Some(data) = event.data.as_mut() {
            mark(&mut data["questions"]);
        }
        if let Some(stored) = self.inputs.get_mut(&id) {
            mark(&mut stored["params"]["questions"]);
        }
    }

    /// Answers `cursor/ask_question` from the form's `{answers:{id:[text, …]}}`:
    /// one text per question, or a list where the question allows several. An
    /// empty response answers with no answers, as a dismissed question does.
    pub(super) fn cursor_user_input(&mut self, id: &str, response: Value) -> Result<Step, String> {
        let request = self.inputs.get(id).ok_or("This input request is no longer active")?;
        let answers = response
            .as_object()
            .filter(|object| object.len() == 1)
            .and_then(|object| object.get("answers")?.as_object())
            .ok_or("Invalid answer format")?;
        let questions = request["params"]["questions"].as_array().cloned().unwrap_or_default();
        if !answers.is_empty() && answers.len() != questions.len() {
            return Err("Answer every question before continuing".into());
        }
        let mut output = serde_json::Map::new();
        for (question_id, values) in answers {
            let question = questions.iter().find(|question| question["id"] == question_id.as_str()).ok_or("Response contains an unknown question")?;
            let values: Vec<&str> = values
                .as_array()
                .ok_or("Invalid answer format")?
                .iter()
                .map(|value| value.as_str().filter(|text| !text.trim().is_empty() && text.len() <= 8192).ok_or("Invalid answer"))
                .collect::<Result<_, _>>()?;
            if question["multiSelect"] == true {
                if values.is_empty() || values.len() > 20 {
                    return Err("Choose at least one answer".into());
                }
                output.insert(question_id.clone(), json!(values));
            } else {
                let [answer] = values[..] else { return Err("Each question takes one answer".into()) };
                output.insert(question_id.clone(), json!(answer));
            }
        }
        let request = self.inputs.remove(id).unwrap();
        let mut event = self.event("input_cancelled", "", None);
        event.request_id = Some(id.into());
        Ok(Step { write: vec![json!({"jsonrpc":"2.0","id":request["id"],"result":{"answers":output}})], events: vec![event], ..Step::default() })
    }

    /// `cursor/create_plan` carries the plan Cursor proposes in Plan mode. It is
    /// shown as the turn's answer and acknowledged; implementing it is the next
    /// message, in a mode that allows changes.
    pub(super) fn cursor_plan(&mut self, value: &Value, step: &mut Step) {
        let plan = value["params"]["plan"].as_str().filter(|plan| !plan.trim().is_empty()).unwrap_or("# Plan\n\n(Cursor did not supply plan text.)");
        let id = self.next_text_id();
        self.text_delta(&id, None, plan, step);
        // Whatever Cursor says next is a new message.
        self.kimi_message_id.clear();
        step.write.push(json!({"jsonrpc":"2.0","id":value["id"],"result":{"accepted":true}}));
    }
}
