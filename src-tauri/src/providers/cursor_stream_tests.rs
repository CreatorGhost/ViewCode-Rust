//! Cursor CLI (`cursor-agent acp`) protocol tests.
//!
//! Where the frames come from:
//! - **From ViewCode's own tests** (`apps/server/src/provider/viewcode/` in
//!   CreatorGhost/ViewCode, branch `dev`): the `cursor-agent about` outputs and
//!   the CLI error strings in `cursorAgentStatus.viewcode.test.ts`, and the
//!   `cli-config.json` shape in `cursorCredentialStore.test.ts`. ViewCode wrote
//!   those from what the CLI prints; the account values are placeholders.
//! - **Synthetic, verify against a live CLI**: every ACP frame below. ViewCode
//!   keeps no recording of `cursor-agent acp` (its `cursor_transcript.ndjson`
//!   fixtures record the SDK runtime, a different protocol), so these frames
//!   are built from the ACP v1 schema and from the shapes ViewCode's mock agent
//!   serves for Cursor (`apps/server/scripts/acp-mock-agent.ts`: the
//!   `cursor/list_available_models` reply, the per-model config options and the
//!   `cursor/ask_question` request). `installed_cursor_*` at the end checks the
//!   same path against a real CLI.
use super::{cursor, tests::config, Protocol, Step};
use serde_json::{json, Value};

const SESSION: &str = "0c6f6f4e-7d0a-4b53-9a51-0d0f2a8f7c11";

// ---- From ViewCode's tests -------------------------------------------------

const ABOUT_SIGNED_IN: &str = r#"{"cliVersion":"2026.10.01","userEmail":"dev@example.test"}"#;
const ABOUT_NOT_LOGGED_IN: &str = r#"{"cliVersion":"2026.10.01","userEmail":null}"#;
const ABOUT_TEXT_FORM: &str = "CLI Version         2026.03.20\nUser Email          Not logged in\n";
const KEYCHAIN_STDERR: &str = "SecItemCopyMatching failed: User interaction is not allowed. (-25308)";
const OFFLINE_STDERR: &str = "getaddrinfo ENOTFOUND api2.cursor.sh";

#[test]
fn about_reads_the_version_and_sign_in_from_json_and_from_the_older_text_form() {
    let about = cursor::parse_about(ABOUT_SIGNED_IN, "");
    assert_eq!(about.version.as_deref(), Some("2026.10.01"));
    assert_eq!(about.email.as_deref(), Some("dev@example.test"));
    assert_eq!(about.signed_in, Some(true));
    let about = cursor::parse_about(ABOUT_NOT_LOGGED_IN, "");
    assert_eq!(about.version.as_deref(), Some("2026.10.01"));
    assert_eq!(about.signed_in, Some(false));
    assert_eq!(about.email, None);
    let about = cursor::parse_about(ABOUT_TEXT_FORM, "");
    assert_eq!(about.version.as_deref(), Some("2026.03.20"));
    assert_eq!(about.signed_in, Some(false));
    // Coloured text output, a signed-in account and a plan.
    let about = cursor::parse_about("\u{1b}[1mCLI Version\u{1b}[0m         2026.03.20\nUser Email          dev@example.test\n", "");
    assert_eq!(about.email.as_deref(), Some("dev@example.test"));
    let about = cursor::parse_about(r#"{"cliVersion":"2026.10.01","userEmail":"dev@example.test","subscriptionTier":"pro_plus"}"#, "");
    assert_eq!(about.plan.as_deref(), Some("Pro Plus"));
    // Nothing recognisable is unknown, never signed out.
    assert_eq!(cursor::parse_about("", "boom").signed_in, None);
}

#[test]
fn a_saved_login_the_cli_could_not_confirm_is_not_called_signed_out() {
    let not_logged_in = cursor::parse_about(ABOUT_NOT_LOGGED_IN, "");
    // No saved login and no stated reason: the user must sign in.
    let account = cursor::account(&not_logged_in, ABOUT_NOT_LOGGED_IN, false);
    assert_eq!(account.status, "signedOut");
    assert!(account.message.contains("cursor-agent login"));
    // After a wake the CLI says "not logged in" while `cli-config.json` still names the account.
    let account = cursor::account(&not_logged_in, ABOUT_NOT_LOGGED_IN, true);
    assert_eq!(account.status, "unavailable");
    assert!(account.message.contains("Keychain"));
    for stderr in [KEYCHAIN_STDERR, OFFLINE_STDERR] {
        let output = format!("{ABOUT_NOT_LOGGED_IN}\n{stderr}");
        assert_eq!(cursor::account(&not_logged_in, &output, false).status, "unavailable", "{stderr}");
    }
    let signed_in = cursor::parse_about(ABOUT_SIGNED_IN, "");
    let account = cursor::account(&signed_in, ABOUT_SIGNED_IN, true);
    assert_eq!((account.status, account.email.as_deref(), account.version.as_deref()), ("unsupported", Some("dev@example.test"), Some("2026.10.01")));
}

#[test]
fn turn_failures_tell_signed_out_unreadable_credentials_and_network_apart() {
    use cursor::AuthProblem::*;
    assert_eq!(cursor::classify("Authentication required. Please run 'cursor-agent login' first."), Some(SignedOut));
    assert_eq!(cursor::classify("Not logged in. Run `agent login`."), Some(SignedOut));
    assert_eq!(cursor::classify("Could not read from the macOS keychain"), Some(CredentialsUnreadable));
    assert_eq!(cursor::classify(KEYCHAIN_STDERR), Some(CredentialsUnreadable));
    assert_eq!(cursor::classify("fetch failed: ECONNRESET"), Some(Network));
    assert_eq!(cursor::classify(OFFLINE_STDERR), Some(Network));
    assert_eq!(cursor::classify("Model gpt-x is not available"), None);

    let failed = |message: &str| json!({"jsonrpc":"2.0","id":3,"error":{"code":-32603,"message":message}});
    let mut p = ready("ask").0;
    let error = p.receive(failed("Authentication required. Please run 'cursor-agent login' first.")).err().unwrap();
    assert!(error.contains("cursor-agent status"), "{error}");
    let error = ready("ask").0.receive(failed("keychain is locked")).err().unwrap();
    assert!(error.contains("Keychain is locked"), "{error}");
    let error = ready("ask").0.receive(failed("fetch failed: ECONNRESET")).err().unwrap();
    assert!(error.contains("could not reach Cursor"), "{error}");
    // Anything else keeps the shared ACP wording.
    let error = ready("ask").0.receive(failed("Model gpt-x is not available")).err().unwrap();
    assert!(error.contains("session/prompt") && error.contains("Model gpt-x is not available"), "{error}");
}

// ---- Synthetic, verify against a live CLI ----------------------------------

fn response(id: u64, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn capabilities() -> Value {
    response(1, json!({"protocolVersion":1,
        "agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":true,"embeddedContext":true},"mcpCapabilities":{"http":true,"sse":true}},
        "authMethods":[{"id":"cursor_login","name":"Cursor Login","description":"Use the login stored by `cursor-agent login`"}]}))
}

fn model_option(current: &str) -> Value {
    json!({"id":"model","name":"Model","category":"model","type":"select","currentValue":current,"options":[
        {"value":"default","name":"Auto"},{"value":"composer-2","name":"Composer 2"},{"value":"gpt-5.4","name":"GPT-5.4"},{"value":"claude-opus-4-6","name":"Opus 4.6"}]})
}

fn session(mode: &str) -> Value {
    json!({"sessionId":SESSION,
        "modes":{"currentModeId":mode,"availableModes":[{"id":"agent","name":"Agent"},{"id":"plan","name":"Plan"},{"id":"ask","name":"Ask"}]},
        "configOptions":[model_option("default")]})
}

/// GPT-5.4's options, keyed `configId` as the later ACP drafts (and ViewCode's mock) spell it.
fn gpt_options(reasoning: &str, context: &str, fast: &str) -> Value {
    json!([
        {"configId":"model","name":"Model","category":"model","type":"select","currentValue":"gpt-5.4","options":[{"value":"default","name":"Auto"},{"value":"gpt-5.4","name":"GPT-5.4"}]},
        {"configId":"reasoning","name":"Reasoning","category":"thought_level","type":"select","currentValue":reasoning,"options":[
            {"value":"none","name":"None"},{"value":"low","name":"Low"},{"value":"medium","name":"Medium"},{"value":"high","name":"High"},{"value":"extra-high","name":"Extra High"}]},
        {"configId":"context","name":"Context","category":"model_config","type":"select","currentValue":context,"options":[{"value":"272k","name":"272K"},{"value":"1m","name":"1M"}]},
        {"configId":"fast","name":"Fast","category":"model_config","type":"select","currentValue":fast,"options":[{"value":"false","name":"Off"},{"value":"true","name":"Fast"}]}
    ])
}

/// The `cursor/list_available_models` reply ViewCode's mock agent serves.
fn model_list() -> Value {
    json!({"models":[
        {"value":"default","name":"Auto","configOptions":[]},
        {"value":"composer-2","name":"Composer 2","configOptions":[
            {"configId":"fast","name":"Fast","category":"model_config","type":"select","currentValue":"false","options":[{"value":"false","name":"Off"},{"value":"true","name":"Fast"}]}]},
        {"value":"gpt-5.4","name":"GPT-5.4","configOptions":gpt_options("medium", "272k", "false").as_array().unwrap()[1..].to_vec()},
        {"value":"claude-opus-4-6","name":"Opus 4.6","configOptions":[
            {"configId":"reasoning","name":"Reasoning","category":"thought_level","type":"select","currentValue":"medium","options":[{"value":"low","name":"Low"},{"value":"medium","name":"Medium"},{"value":"high","name":"High"}]},
            {"configId":"thinking","name":"Thinking","category":"model_config","type":"boolean","currentValue":true}]},
        {"value":"gpt-5.4","name":"Listed twice"},
        {"value":"  ","name":"No slug"}
    ]})
}

fn update(update: Value) -> Value {
    json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":SESSION,"update":update}})
}

fn permission(id: u64, kind: &str, title: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"session/request_permission","params":{"sessionId":SESSION,
        "toolCall":{"toolCallId":"tool-1","title":title,"kind":kind,"status":"pending"},
        "options":[{"optionId":"allow-once","name":"Allow once","kind":"allow_once"},{"optionId":"allow-always","name":"Allow always","kind":"allow_always"},{"optionId":"reject-once","name":"Reject","kind":"reject_once"}]}})
}

/// A Cursor protocol past its handshake with the default model, plus the prompt step.
fn ready(mode: &str) -> (Protocol, Step) {
    let mut cfg = config("cursor");
    cfg.mode = mode.into();
    let mut p = Protocol::new(cfg);
    p.receive(capabilities()).unwrap();
    p.receive(response(7, json!({}))).unwrap();
    let mut step = p.receive(response(2, session("agent"))).unwrap();
    if mode == "plan" {
        assert_eq!(step.write[0]["method"], "session/set_mode");
        step = p.receive(response(4, json!({}))).unwrap();
    }
    assert_eq!(step.write[0]["method"], "session/prompt", "Auto in the right mode goes straight to the prompt");
    (p, step)
}

fn args(mode: &str) -> Vec<String> {
    let mut cfg = config("cursor");
    cfg.mode = mode.into();
    cfg.model = "gpt-5.4[context=1m,fast=true]".into();
    cfg.reasoning_effort = Some("high".into());
    let mut command = tokio::process::Command::new("cursor-agent");
    super::acp::configure(&mut command, &cfg);
    command.as_std().get_args().map(|arg| arg.to_string_lossy().into_owned()).collect()
}

#[test]
fn cursor_is_registered_with_its_cli_name_and_catalog_defined_efforts() {
    assert!(super::PROVIDERS.contains(&("cursor", "Cursor")));
    assert_eq!(super::executable_name("cursor"), "cursor-agent");
    assert!(super::validate_reasoning_effort("cursor", None).is_ok());
    assert!(super::validate_reasoning_effort("cursor", Some("extra-high")).is_ok());
    assert!(super::validate_reasoning_effort("cursor", Some("bad\nflag")).is_err());
}

#[test]
fn launch_flags_follow_the_approval_mode_and_never_carry_the_model() {
    // Only Full access is a launch flag; the model and its options are session settings.
    assert_eq!(args("ask"), ["acp"]);
    assert_eq!(args("auto"), ["acp"]);
    assert_eq!(args("plan"), ["acp"]);
    assert_eq!(args("full"), ["--force", "acp"]);
}

#[test]
fn the_model_list_becomes_base_models_with_efforts_and_explicit_variants() {
    let models = cursor::catalog(&model_list());
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids, [
        "default",
        "composer-2[fast=false]", "composer-2[fast=true]",
        "gpt-5.4[context=272k,fast=false]", "gpt-5.4[context=272k,fast=true]", "gpt-5.4[context=1m,fast=false]", "gpt-5.4[context=1m,fast=true]",
        "claude-opus-4-6[thinking=true]", "claude-opus-4-6[thinking=false]",
    ]);
    assert_eq!(models[0].name, "Auto");
    let named = |id: &str| models.iter().find(|model| model.id == id).unwrap();
    assert_eq!(named("composer-2[fast=false]").name, "Composer 2");
    assert_eq!(named("composer-2[fast=true]").name, "Composer 2 · Fast");
    assert_eq!(named("gpt-5.4[context=1m,fast=true]").name, "GPT-5.4 · 1M · Fast");
    assert_eq!(named("claude-opus-4-6[thinking=false]").name, "Opus 4.6 · Thinking off");
    // Context sizes come from the context option; efforts from the reasoning option.
    assert_eq!(named("gpt-5.4[context=272k,fast=false]").context_window, Some(272_000));
    assert_eq!(named("gpt-5.4[context=1m,fast=false]").context_window, Some(1_000_000));
    assert_eq!(named("composer-2[fast=true]").context_window, None);
    let gpt = named("gpt-5.4[context=272k,fast=false]");
    assert_eq!(gpt.reasoning_efforts.iter().map(|effort| effort.id.as_str()).collect::<Vec<_>>(), ["none", "low", "medium", "high", "extra-high"]);
    assert_eq!(gpt.reasoning_efforts[4].name, "Extra High");
    assert_eq!(gpt.default_reasoning_effort.as_deref(), Some("medium"));
    assert!(named("composer-2[fast=false]").reasoning_efforts.is_empty());
    assert_eq!(named("claude-opus-4-6[thinking=true]").reasoning_efforts.len(), 3);
    // A failed or empty list is only the Auto choice, which discovery reports as a failure.
    assert_eq!(cursor::catalog(&json!({})).len(), 1);
}

#[test]
fn a_pick_splits_into_its_base_model_and_options() {
    assert_eq!(cursor::split_model("gpt-5.4[context=1m,fast=true]"), ("gpt-5.4", vec![("context", "1m"), ("fast", "true")]));
    assert_eq!(cursor::split_model("composer-2"), ("composer-2", vec![]));
    assert_eq!(cursor::split_model(""), ("default", vec![]));
    assert_eq!(cursor::split_model("gpt-5.4[reasoning=high]"), ("gpt-5.4", vec![("reasoning", "high")]));
}

#[test]
fn a_turn_authenticates_opens_a_session_with_host_tools_and_streams() {
    let mut cfg = config("cursor");
    cfg.model = "gpt-5.4[context=1m,fast=true]".into();
    cfg.reasoning_effort = Some("high".into());
    let mut p = Protocol::new(cfg);
    p.mcp_server = Some(json!({"type":"http","name":"pandacode","url":"http://127.0.0.1:4100/mcp","headers":[]}));
    // The capability asks Cursor for base models with per-model options.
    let initialize = p.initial();
    assert_eq!(initialize["method"], "initialize");
    assert_eq!(initialize["params"]["protocolVersion"], 1);
    assert_eq!(initialize["params"]["clientCapabilities"]["_meta"]["parameterizedModelPicker"], true);
    // The CLI's own login is claimed before any session call.
    let step = p.receive(capabilities()).unwrap();
    assert_eq!(step.write.len(), 1);
    assert_eq!(step.write[0]["method"], "authenticate");
    assert_eq!(step.write[0]["params"]["methodId"], "cursor_login");
    let step = p.receive(response(7, json!({}))).unwrap();
    assert_eq!(step.write[0]["method"], "session/new");
    assert_eq!(step.write[0]["params"]["cwd"], "/tmp");
    assert_eq!(step.write[0]["params"]["mcpServers"][0]["name"], "pandacode", "the per-run MCP host reaches Cursor");
    // Already in agent mode, so the model is next: the base model, then each option, then the effort.
    let step = p.receive(response(2, session("agent"))).unwrap();
    assert!(step.events.iter().any(|event| event.kind == "session" && event.text == SESSION));
    assert_eq!(step.write[0]["method"], "session/set_config_option");
    assert_eq!(step.write[0]["params"], json!({"sessionId":SESSION,"configId":"model","value":"gpt-5.4"}));
    let step = p.receive(response(5, json!({"configOptions":gpt_options("medium", "272k", "false")}))).unwrap();
    assert_eq!(step.write[0]["params"], json!({"sessionId":SESSION,"configId":"context","value":"1m"}));
    let step = p.receive(response(20, json!({"configOptions":gpt_options("medium", "1m", "false")}))).unwrap();
    assert_eq!(step.write[0]["params"], json!({"sessionId":SESSION,"configId":"fast","value":"true"}));
    let step = p.receive(response(21, json!({"configOptions":gpt_options("medium", "1m", "true")}))).unwrap();
    assert_eq!(step.write[0]["id"], 6);
    assert_eq!(step.write[0]["params"], json!({"sessionId":SESSION,"configId":"reasoning","value":"high"}));
    let step = p.receive(response(6, json!({"configOptions":gpt_options("high", "1m", "true")}))).unwrap();
    assert_eq!(step.write[0]["method"], "session/prompt");
    assert_eq!(step.write[0]["params"]["prompt"][0]["text"], "Explain this project", "Cursor has its own Plan mode, so no prefix");
    assert_eq!(p.phase, 1);

    // Reasoning, a tool call, text, context usage, then the end of the turn.
    let step = p.receive(update(json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"Reading the manifest"}}))).unwrap();
    assert!(step.events.iter().any(|event| event.kind == "thinking" && event.text == "Reading the manifest"));
    let step = p.receive(update(json!({"sessionUpdate":"tool_call","toolCallId":"tool-1","title":"Read Cargo.toml","kind":"read","status":"in_progress","rawInput":{"path":"Cargo.toml"}}))).unwrap();
    let tool = step.events.iter().find_map(|event| event.tool.as_ref()).expect("a tool event");
    assert_eq!((tool.name.as_str(), tool.status.as_str()), ("Read Cargo.toml", "running"));
    let step = p.receive(update(json!({"sessionUpdate":"tool_call_update","toolCallId":"tool-1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"[package]"}}]}))).unwrap();
    assert_eq!(step.events.iter().find_map(|event| event.tool.as_ref()).unwrap().status, "completed");
    let text: String = [
        update(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"A Rust "}})),
        update(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"workspace."}})),
    ]
    .into_iter()
    .flat_map(|frame| p.receive(frame).unwrap().events)
    .filter(|event| event.kind == "text")
    .map(|event| event.text)
    .collect();
    assert_eq!(text, "A Rust workspace.");
    let step = p.receive(update(json!({"sessionUpdate":"usage_update","used":18240,"size":1000000}))).unwrap();
    let usage = step.events.iter().find(|event| event.kind == "usage").expect("usage");
    assert_eq!(usage.data.as_ref().unwrap()["usedTokens"], 18240);
    assert_eq!(usage.data.as_ref().unwrap()["contextWindow"], 1000000);
    assert!(p.receive(response(3, json!({"stopReason":"end_turn"}))).unwrap().done);
}

#[test]
fn settings_a_restored_session_already_has_are_not_sent_again() {
    let mut cfg = config("cursor");
    cfg.resume_id = Some(SESSION.into());
    cfg.model = "gpt-5.4[context=1m,fast=true]".into();
    cfg.reasoning_effort = Some("high".into());
    let mut p = Protocol::new(cfg);
    p.receive(capabilities()).unwrap();
    // Cursor advertises `loadSession`, so the saved session is loaded by id.
    let step = p.receive(response(7, json!({}))).unwrap();
    assert_eq!(step.write[0]["method"], "session/load");
    assert_eq!(step.write[0]["params"]["sessionId"], SESSION);
    // History replayed before the answer belongs to earlier turns.
    let replay = p.receive(update(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"An earlier answer"}}))).unwrap();
    assert!(replay.events.iter().all(|event| event.kind != "text"));
    let mut restored = session("agent");
    restored.as_object_mut().unwrap().remove("sessionId");
    restored["configOptions"] = gpt_options("high", "1m", "true");
    restored["configOptions"][0]["id"] = json!("model");
    let step = p.receive(response(2, restored)).unwrap();
    assert_eq!(step.write.len(), 1);
    assert_eq!(step.write[0]["method"], "session/prompt", "mode, model, options and effort are all as chosen");
}

#[test]
fn going_back_to_auto_sets_the_default_model_again() {
    let mut cfg = config("cursor");
    cfg.resume_id = Some(SESSION.into());
    let mut p = Protocol::new(cfg);
    p.receive(capabilities()).unwrap();
    p.receive(response(7, json!({}))).unwrap();
    let mut restored = session("ask");
    restored["configOptions"] = json!([model_option("gpt-5.4")]);
    // A session left in Cursor's read-only `ask` mode goes back to `agent` first.
    let step = p.receive(response(2, restored)).unwrap();
    assert_eq!(step.write[0]["method"], "session/set_mode");
    assert_eq!(step.write[0]["params"]["modeId"], "agent");
    let step = p.receive(response(4, json!({}))).unwrap();
    assert_eq!(step.write[0]["params"], json!({"sessionId":SESSION,"configId":"model","value":"default"}));
    let step = p.receive(response(5, json!({"configOptions":[model_option("default")]}))).unwrap();
    assert_eq!(step.write[0]["method"], "session/prompt");
}

#[test]
fn a_model_or_option_cursor_does_not_offer_fails_with_a_reason() {
    let mut cfg = config("cursor");
    cfg.model = "gpt-9".into();
    let mut p = Protocol::new(cfg);
    p.receive(capabilities()).unwrap();
    p.receive(response(7, json!({}))).unwrap();
    let error = p.receive(response(2, session("agent"))).err().unwrap();
    assert!(error.contains("gpt-9") && error.contains("Refresh the model list"), "{error}");

    let mut cfg = config("cursor");
    cfg.model = "gpt-5.4[turbo=on]".into();
    let mut p = Protocol::new(cfg);
    p.receive(capabilities()).unwrap();
    p.receive(response(7, json!({}))).unwrap();
    p.receive(response(2, session("agent"))).unwrap();
    let error = p.receive(response(5, json!({"configOptions":gpt_options("medium", "272k", "false")}))).err().unwrap();
    assert!(error.contains("\"turbo\""), "{error}");
}

#[test]
fn a_cli_that_is_signed_out_or_too_old_says_what_to_do() {
    // `authenticate` refused: no usable login.
    let mut p = Protocol::new(config("cursor"));
    p.receive(capabilities()).unwrap();
    let error = p.receive(json!({"jsonrpc":"2.0","id":7,"error":{"code":-32603,"message":"Internal error"}})).err().unwrap();
    assert_eq!(error, cursor::SIGN_IN);
    // ACP's auth-required code on a session call.
    let mut p = Protocol::new(config("cursor"));
    p.receive(capabilities()).unwrap();
    p.receive(response(7, json!({}))).unwrap();
    let error = p.receive(json!({"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"Authentication required"}})).err().unwrap();
    assert!(error.contains("cursor-agent login"), "{error}");
    // A CLI from before `cursor-agent acp` prints text, or rejects the command.
    assert_eq!(cursor::run_failure("Invalid provider JSON: expected value at line 1 column 1", "").as_deref(), Some(cursor::NOT_ACP));
    assert_eq!(cursor::run_failure("Agent exited before completing its response", "error: unknown command 'acp'").as_deref(), Some(cursor::NOT_ACP));
    assert!(cursor::NOT_ACP.contains("cursor-agent update") && cursor::NOT_ACP.contains("cursor.com/docs/cli/installation"));
    // A process that died on a locked Keychain keeps that reason; other exits keep theirs.
    let error = cursor::run_failure("Agent exited before completing its response", KEYCHAIN_STDERR).unwrap();
    assert!(error.contains("Keychain is locked"), "{error}");
    assert_eq!(cursor::run_failure("Agent exited before completing its response", "panic: boom"), None);
}

#[test]
fn approvals_follow_the_task_mode() {
    // Standard: every request waits for the user, and the answer picks an offered option.
    let (mut p, _) = ready("ask");
    let step = p.receive(permission(11, "edit", "Edit `src/main.rs`")).unwrap();
    assert!(step.write.is_empty());
    let approval = step.events.iter().find(|event| event.kind == "approval").expect("an approval card");
    assert_eq!(approval.text, "Edit `src/main.rs`");
    let id = approval.request_id.clone().unwrap();
    let answer = p.approve(&id, true).unwrap();
    assert_eq!(answer, json!({"jsonrpc":"2.0","id":11,"result":{"outcome":{"outcome":"selected","optionId":"allow-once"}}}));
    let step = p.receive(permission(12, "execute", "Run `cargo test`")).unwrap();
    let id = step.events.iter().find(|event| event.kind == "approval").unwrap().request_id.clone().unwrap();
    assert_eq!(p.approve(&id, false).unwrap()["result"]["outcome"]["optionId"], "reject-once");

    // Accept edits: edits pass, commands still ask.
    let (mut p, _) = ready("auto");
    let step = p.receive(permission(11, "edit", "Edit `src/main.rs`")).unwrap();
    assert_eq!(step.write[0]["result"]["outcome"]["optionId"], "allow-once");
    let step = p.receive(permission(12, "execute", "Run `cargo test`")).unwrap();
    assert!(step.write.is_empty() && step.events.iter().any(|event| event.kind == "approval"));

    // Plan: Cursor's own plan mode, and a change it still asks for is declined, not offered.
    let (mut p, _) = ready("plan");
    let step = p.receive(permission(11, "edit", "Edit `src/main.rs`")).unwrap();
    assert_eq!(step.write[0]["result"]["outcome"]["optionId"], "reject-once");
    assert!(step.events.iter().any(|event| event.kind == "status" && event.text.contains("Plan mode declined")));
    assert!(p.pending.is_empty());
    let step = p.receive(permission(12, "read", "Read `README.md`")).unwrap();
    assert_eq!(step.write[0]["result"]["outcome"]["optionId"], "allow-once");

    // Full access starts the CLI with `--force`; a request that still arrives is allowed.
    let (mut p, _) = ready("full");
    let step = p.receive(permission(11, "execute", "Run `cargo test`")).unwrap();
    assert_eq!(step.write[0]["result"]["outcome"]["optionId"], "allow-once");

    // A request for another session is never the user's to answer.
    let (mut p, _) = ready("full");
    let mut foreign = permission(13, "execute", "Run `rm -rf /`");
    foreign["params"]["sessionId"] = json!("another-session");
    assert_eq!(p.receive(foreign).unwrap().write[0]["result"]["outcome"]["outcome"], "cancelled");
}

#[test]
fn plan_mode_selects_cursors_plan_mode_and_shows_the_plan_it_proposes() {
    let (mut p, step) = ready("plan");
    assert_eq!(step.write[0]["params"]["prompt"][0]["text"], "Explain this project");
    p.receive(update(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Here is my plan."}}))).unwrap();
    let step = p.receive(json!({"jsonrpc":"2.0","id":31,"method":"cursor/create_plan","params":{"toolCallId":"plan-1","plan":"# Plan\n\n1. Add the flag\n2. Test it"}})).unwrap();
    assert_eq!(step.write, [json!({"jsonrpc":"2.0","id":31,"result":{"accepted":true}})]);
    let plan: String = step.events.iter().filter(|event| event.kind == "text").map(|event| event.text.as_str()).collect();
    assert_eq!(plan, "# Plan\n\n1. Add the flag\n2. Test it");
    assert!(step.events.iter().any(|event| event.kind == "message_start"), "the plan is its own message");
    // Missing plan text still gets an answer, so the turn does not wait.
    let step = p.receive(json!({"jsonrpc":"2.0","id":32,"method":"cursor/create_plan","params":{"toolCallId":"plan-2","plan":""}})).unwrap();
    assert_eq!(step.write[0]["result"]["accepted"], true);
}

#[test]
fn a_question_from_cursor_uses_the_shared_form_and_answers_by_question_id() {
    let question = json!({"jsonrpc":"2.0","id":41,"method":"cursor/ask_question","params":{"toolCallId":"ask-question-tool-call-1","title":"Question","questions":[
        {"id":"scope","prompt":"Which scope?","options":[{"id":"workspace","label":"Workspace"},{"id":"session","label":"Session"}]},
        {"id":"targets","prompt":"Which targets?","allowMultiple":true,"options":[{"id":"mac","label":"macOS"},{"id":"win","label":"Windows"}]}]}});
    let (mut p, _) = ready("ask");
    let step = p.receive(question.clone()).unwrap();
    assert!(step.write.is_empty(), "the turn waits for the user");
    let event = step.events.iter().find(|event| event.kind == "input_request").expect("the question form");
    let data = event.data.as_ref().unwrap();
    assert_eq!(data["questions"][0]["question"], "Which scope?");
    assert_eq!(data["questions"][0]["options"][1]["label"], "Session");
    assert_eq!(data["questions"][1]["multiSelect"], true);
    let id = event.request_id.clone().unwrap();
    assert!(p.cursor_user_input(&id, json!({"answers":{"scope":["Workspace"]}})).is_err(), "every question needs an answer");
    assert!(p.cursor_user_input(&id, json!({"answers":{"scope":["Workspace","Session"],"targets":["macOS"]}})).is_err());
    let step = p.cursor_user_input(&id, json!({"answers":{"scope":["Workspace"],"targets":["macOS","Windows"]}})).unwrap();
    assert_eq!(step.write, [json!({"jsonrpc":"2.0","id":41,"result":{"answers":{"scope":"Workspace","targets":["macOS","Windows"]}}})]);
    assert!(p.inputs.is_empty());
    // Stop answers an open question with no answers instead of leaving Cursor waiting.
    let (mut p, _) = ready("ask");
    p.receive(question).unwrap();
    let step = p.cancel_requests();
    assert!(step.write.iter().any(|frame| frame["id"] == 41 && frame["result"]["answers"] == json!({})));
    // Any other request from the agent is refused, not left hanging.
    let step = p.receive(json!({"jsonrpc":"2.0","id":42,"method":"cursor/unknown","params":{}})).unwrap();
    assert_eq!(step.write[0]["error"]["code"], -32601);
}

#[test]
fn stop_cancels_the_session_and_a_cancelled_turn_ends_the_run() {
    let (mut p, _) = ready("ask");
    assert_eq!(p.cancel(), json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":SESSION}}));
    assert!(p.receive(response(3, json!({"stopReason":"cancelled"}))).unwrap().done);
}

#[test]
fn a_silent_turn_is_given_up_on_unless_a_tool_or_a_request_explains_the_silence() {
    let (mut p, _) = ready("ask");
    let last = p.cursor.last_frame.expect("every frame is timed");
    assert_eq!(p.cursor_stall_at(), Some(last + cursor::STALL_TIMEOUT));
    assert_eq!(cursor::STALL_TIMEOUT.as_secs(), 600);
    // A running tool (a build, a subagent) is silent far longer.
    p.receive(update(json!({"sessionUpdate":"tool_call","toolCallId":"tool-1","title":"Run tests","kind":"execute","status":"in_progress"}))).unwrap();
    assert_eq!(p.cursor_stall_at(), None);
    p.receive(update(json!({"sessionUpdate":"tool_call_update","toolCallId":"tool-1","status":"completed"}))).unwrap();
    assert!(p.cursor_stall_at().is_some());
    // So is a request the user has not answered.
    p.receive(permission(11, "execute", "Run `cargo test`")).unwrap();
    assert_eq!(p.cursor_stall_at(), None);
    // Before the prompt is sent the startup limit applies instead.
    let mut p = Protocol::new(config("cursor"));
    p.receive(capabilities()).unwrap();
    assert_eq!(p.cursor_stall_at(), None);
    // Other harnesses are not watched.
    assert_eq!(Protocol::new(config("grok")).cursor_stall_at(), None);
}

#[test]
fn host_tools_are_optional_unless_a_team_needs_them() {
    let without_http = || {
        let mut frame = capabilities();
        frame["result"]["agentCapabilities"]["mcpCapabilities"] = json!({});
        frame
    };
    let mut p = Protocol::new(config("cursor"));
    p.mcp_server = Some(json!({"type":"http","name":"pandacode"}));
    p.mcp_required = false;
    p.receive(without_http()).unwrap();
    let step = p.receive(response(7, json!({}))).unwrap();
    assert_eq!(step.write[0]["params"]["mcpServers"], json!([]));
    let mut p = Protocol::new(config("cursor"));
    p.mcp_server = Some(json!({"type":"http","name":"pandacode"}));
    assert!(p.receive(without_http()).err().unwrap().contains("HTTP MCP"));
}

#[test]
fn images_go_out_as_acp_image_blocks_when_cursor_accepts_them() {
    let mut p = Protocol::new(config("cursor"));
    p.image_blocks.push(json!({"type":"image","mimeType":"image/png","data":"aGk="}));
    p.receive(capabilities()).unwrap();
    p.receive(response(7, json!({}))).unwrap();
    let step = p.receive(response(2, session("agent"))).unwrap();
    assert_eq!(step.write[0]["params"]["prompt"][0]["type"], "image");
    assert_eq!(step.write[0]["params"]["prompt"][1]["text"], "Explain this project");
    let mut p = Protocol::new(config("cursor"));
    p.image_blocks.push(json!({"type":"image"}));
    let mut text_only = capabilities();
    text_only["result"]["agentCapabilities"]["promptCapabilities"]["image"] = json!(false);
    assert!(p.receive(text_only).err().unwrap().contains("image input"));
}

#[test]
fn commands_cursor_advertises_reach_the_composer() {
    let frame = update(json!({"sessionUpdate":"available_commands_update","availableCommands":[{"name":"review","description":"Review the working tree","input":{"hint":"[focus]"}}]}));
    let (mut p, _) = ready("ask");
    assert!(p.receive(frame).unwrap().events.iter().any(|event| event.kind == "commands_changed"));
}

// ---- Live: needs the Cursor CLI ---------------------------------------------

/// Run on a computer with `cursor-agent` installed and signed in:
/// `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib installed_cursor -- --ignored --nocapture`
/// It reads `cursor-agent about`, lists models and negotiates one model with
/// its options and effort. It never sends a prompt.
#[tokio::test]
#[ignore = "Needs an installed and signed-in Cursor CLI; metadata only, never sends a prompt"]
async fn installed_cursor_lists_models_and_negotiates_settings_without_a_prompt() {
    use crate::slash_commands::claude::Pipe;
    use std::time::Duration;
    let path = super::executable("cursor").expect("Install the Cursor CLI first: https://cursor.com/docs/cli/installation");
    let probe = path.clone();
    let (about, _) = tokio::task::spawn_blocking(move || cursor::about(&probe)).await.unwrap().unwrap();
    println!("cursor-agent {} signed in: {:?} plan: {:?}", about.version.as_deref().unwrap_or("?"), about.signed_in, about.plan);
    assert_eq!(about.signed_in, Some(true), "run `cursor-agent login` first");
    let models = cursor::models(&path).await.unwrap();
    for model in &models {
        println!("{} | {} | efforts {:?} | context {:?}", model.id, model.name, model.reasoning_efforts.iter().map(|effort| effort.id.as_str()).collect::<Vec<_>>(), model.context_window);
    }
    assert!(models.len() > 1, "{models:?}");
    // The richest pick: a model with options of its own and, if any has them, efforts.
    let chosen = models.iter().skip(1).max_by_key(|model| (model.id.contains('['), !model.reasoning_efforts.is_empty())).unwrap();
    let mut cfg = config("cursor");
    cfg.model = chosen.id.clone();
    cfg.reasoning_effort = chosen.reasoning_efforts.last().map(|effort| effort.id.clone());
    cfg.cwd = std::env::temp_dir().join(format!("pandacode-cursor-smoke-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&cfg.cwd).unwrap();
    let mut command = super::provider_command("cursor", &path).unwrap();
    super::acp::configure(&mut command, &cfg);
    let mut pipe = Pipe::spawn(command, &cfg.cwd, Duration::from_secs(60)).unwrap();
    let mut protocol = Protocol::new(cfg.clone());
    let result: Result<(), String> = async {
        pipe.send(&protocol.initial()).await.map_err(|_| "Could not initialize Cursor")?;
        loop {
            let frame = pipe.read().await.map_err(|_| "Could not read Cursor metadata")?;
            if frame.get("method").is_some() && frame.get("id").is_some() {
                pipe.send(&json!({"jsonrpc":"2.0","id":frame["id"],"error":{"code":-32601,"message":"Metadata smoke test does not execute requests"}}))
                    .await
                    .map_err(|_| "Could not reject a server request")?;
                continue;
            }
            for request in protocol.receive(frame)?.write {
                println!("-> {} {}", request["method"], request["params"]);
                if request["method"] == "session/prompt" {
                    return Ok(());
                }
                pipe.send(&request).await.map_err(|_| "Could not negotiate Cursor settings")?;
            }
        }
    }
    .await;
    pipe.finish().await;
    let _ = std::fs::remove_dir_all(&cfg.cwd);
    result.unwrap_or_else(|error| panic!("{error}\n{}", pipe.diagnostic()));
}
