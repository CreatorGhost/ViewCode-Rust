//! The computer-use tools an agent is handed, and the app behind the gate: where the
//! settings, the running turn and provider approvals are read from.
//!
//! Each tool is one of ViewCode's `viewcode-computer` commands with the same arguments,
//! so a refusal that names `viewcode-computer <command>` means the tool
//! `pandacode_computer_<command>`. Tools are offered only while the setting is not off,
//! on a platform the helper drives, with the helper installed; the service still decides
//! every call from what is true at that moment.

use super::driver::{binary_candidates, find_binary, DriverClient, DriverOptions, Launch, BINARY_ENV};
use super::policy::{ComputerUseError, Effect, Mode, TurnContext};
use super::protocol::platform_supported;
use super::service::{own_process_ancestry, Caller, Host, Service, ServiceOptions};
use super::settings::{self, Settings};
use crate::agent_tools::spec;
use crate::providers::ToolHost;
use crate::AppState;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tauri::{Emitter, Manager};

/// Tool name (after `pandacode_`), the command it runs, and whether it is input.
const TOOLS: [(&str, &str, bool); 15] = [
    ("computer_status", "status", false),
    ("computer_list_windows", "list-windows", false),
    ("computer_observe", "observe", false),
    ("computer_screenshot", "screenshot", false),
    ("computer_press", "press", true),
    ("computer_set_value", "set-value", true),
    ("computer_type", "type", true),
    ("computer_key", "key", true),
    ("computer_scroll", "scroll", true),
    ("computer_click", "click", true),
    ("computer_drag", "drag", true),
    ("computer_move", "move", true),
    ("computer_scroll_at", "scroll-at", true),
    ("computer_type_focused", "type-focused", true),
    ("computer_focus", "focus", true),
];

/// The command a tool runs and whether it is input; `None` for another host's tool.
pub(crate) fn command_for(tool: &str) -> Option<(&'static str, bool)> {
    let short = tool.strip_prefix("pandacode_")?;
    TOOLS.iter().find(|(name, _, _)| *name == short).map(|(_, command, input)| (*command, *input))
}

/// Whether a tool is one of these. A harness need not ask before them: the gate decides
/// every call itself (setting, turn, protected apps, approval) and asks the user in its
/// own words, which never show typed text.
pub(crate) fn self_gated(tool: &str) -> bool {
    command_for(tool).is_some()
}

fn id(what: &str) -> Value {
    json!({"type":"integer","minimum":1,"description":what})
}

fn pixel(what: &str) -> Value {
    json!({"type":"integer","minimum":0,"description":what})
}

fn delta() -> Value {
    json!({"type":"integer","minimum":-50,"maximum":50})
}

fn text(what: &str) -> Value {
    json!({"type":"string","minLength":1,"maxLength":10000,"description":what})
}

/// The tools for a mode: looking only under observe, input as well under control.
pub(crate) fn catalog(mode: Mode) -> Vec<Value> {
    let window = || id("A window id from pandacode_computer_list_windows.");
    let reference = || id("An element ref from the newest pandacode_computer_observe of its window.");
    let shot = || id("The newest screenshot (shot id) of the window.");
    let x = || pixel("Pixels from the left edge of the screenshot image.");
    let y = || pixel("Pixels from the top edge of the screenshot image.");
    let all = vec![
        spec("computer_status", "Computer use on this machine: the mode (observe = look only, control = also input), whether the helper runs and whether Accessibility is granted. A refusal naming `viewcode-computer <command>` means the tool pandacode_computer_<command>.", json!({}), &[]),
        spec("computer_list_windows", "List the windows open on the user's computer with their ids. Run it first, and again after a window opens or closes. Password managers, system settings and PandaCode itself are listed but can never be observed or controlled.", json!({"app":{"type":"string","maxLength":200,"description":"Only windows whose app name contains this."}}), &[]),
        spec("computer_observe", "Read a window's controls (role, label, value) and get a ref for each. A new observation of a window retires its older refs.", json!({"window":window(),"query":{"type":"string","maxLength":200,"description":"Only controls whose label or value contains this."}}), &["window"]),
        spec("computer_screenshot", "Save a screenshot of one window as a PNG and return its path, size and shot id. Coordinates for click, drag, move and scroll_at are pixels in a window's newest shot.", json!({"window":window(),"maxSize":{"type":"integer","minimum":256,"maximum":2560,"description":"Longest edge in pixels (default 1568)."}}), &["window"]),
        spec("computer_press", "Press an observed control (button, checkbox, menu item) by ref, without moving the pointer. Input tools run only during your turn, may ask the user first, and return a fresh screenshot; observe again before the next action.", json!({"ref":reference()}), &["ref"]),
        spec("computer_set_value", "Replace the value of an observed text field or slider by ref.", json!({"ref":reference(),"value":text("The new value.")}), &["ref", "value"]),
        spec("computer_type", "Type text into an observed text field by ref.", json!({"ref":reference(),"text":text("The text to type.")}), &["ref", "text"]),
        spec("computer_key", "Press one key or shortcut in a window, which comes to the front first: modifiers (cmd, ctrl, alt, shift) joined by + and then one key, e.g. enter, escape, cmd+s.", json!({"window":window(),"keys":{"type":"string","maxLength":40}}), &["window", "keys"]),
        spec("computer_scroll", "Scroll an observed control by ref; dx and dy are steps (positive dy scrolls down).", json!({"ref":reference(),"dx":delta(),"dy":delta()}), &["ref", "dx", "dy"]),
        spec("computer_click", "Click a point in a window's newest screenshot; the window comes to the front first. Prefer press by ref when the control was observed.", json!({"shot":shot(),"x":x(),"y":y(),"button":{"type":"string","enum":["left","right","middle"]},"count":{"type":"integer","minimum":1,"maximum":3}}), &["shot", "x", "y"]),
        spec("computer_drag", "Drag from one point to another in a window's newest screenshot.", json!({"shot":shot(),"fromX":x(),"fromY":y(),"toX":x(),"toY":y()}), &["shot", "fromX", "fromY", "toX", "toY"]),
        spec("computer_move", "Move the pointer to a point in a window's newest screenshot, for hover states.", json!({"shot":shot(),"x":x(),"y":y()}), &["shot", "x", "y"]),
        spec("computer_scroll_at", "Scroll at a point in a window's newest screenshot; dx and dy are steps.", json!({"shot":shot(),"x":x(),"y":y(),"dx":delta(),"dy":delta()}), &["shot", "x", "y", "dx", "dy"]),
        spec("computer_type_focused", "Type text into whatever has keyboard focus in a window, which comes to the front first. Prefer type by ref.", json!({"window":window(),"text":text("The text to type.")}), &["window", "text"]),
        spec("computer_focus", "Bring a window to the front and nothing else.", json!({"window":window()}), &["window"]),
    ];
    all.into_iter().filter(|tool| tool["name"].as_str().and_then(command_for).is_some_and(|(_, input)| !input || mode == Mode::Control)).collect()
}

/// A refusal as the agent reads it: the code, the gate's wording and, for input, what
/// the OS may have received.
pub(crate) fn refusal_text(refusal: &ComputerUseError) -> String {
    let effect = match refusal.effect {
        Some(Effect::NotDispatched) => " No input was sent.",
        Some(Effect::DispatchedUnknown) => " The input may have been sent: look before you retry.",
        Some(Effect::Dispatched) => " The input was sent.",
        None => "",
    };
    format!("{}: {}{effect}", serde_json::to_value(refusal.code).ok().and_then(|code| code.as_str().map(str::to_owned)).unwrap_or_default(), refusal.message)
}

/// The request body the gate validates: the tool's arguments under its command.
pub(crate) fn request_body(command: &str, args: &Value) -> Value {
    let mut body = match args {
        Value::Object(args) => args.clone(),
        _ => serde_json::Map::new(),
    };
    body.insert("command".into(), json!(command));
    Value::Object(body)
}

struct AppHost {
    app: tauri::AppHandle,
}

impl Host for AppHost {
    fn settings(&self) -> Option<Settings> {
        let state = self.app.state::<AppState>();
        let store = state.store.lock().ok()?;
        settings::read(&store).ok()
    }

    fn running_turn(&self, thread_id: &str) -> Option<TurnContext> {
        let state = self.app.state::<AppState>();
        running_turn(&state.agents, &state.store, thread_id)
    }

    fn provider_approval_pending(&self) -> Option<bool> {
        provider_approval_pending(&self.app.state::<AppState>().agents)
    }

    fn changed(&self, thread_id: Option<&str>) {
        changed(&self.app, thread_id);
    }
}

/// A turn is running while the thread's agent is registered and not stopped; its run id
/// is the turn's id. `None` when anything cannot be read.
fn running_turn(agents: &crate::Agents, store: &std::sync::Mutex<crate::storage::Store>, thread_id: &str) -> Option<TurnContext> {
    let turn_id = {
        let agents = agents.lock().ok()?;
        let agent = agents.get(thread_id)?;
        if agent.stopped.load(Ordering::SeqCst) {
            return None;
        }
        agent.run_id.clone()
    };
    let session = store.lock().ok()?.session(thread_id).ok()?;
    if session.archived {
        return None;
    }
    Some(TurnContext { title: session.title, turn_id, full_access: session.mode == "full" })
}

/// Whether any running agent has a request waiting that is not a question for the user.
/// `None` when it cannot be read, which the gate counts as pending.
fn provider_approval_pending(agents: &crate::Agents) -> Option<bool> {
    let agents = agents.lock().ok()?;
    for agent in agents.values() {
        let approvals = agent.approvals.lock().ok()?;
        let requests = agent.pending_requests.lock().ok()?;
        if approvals.iter().any(|request_id| requests.get(request_id).is_none_or(|request| request.kind != "input_request")) {
            return Some(true);
        }
    }
    Some(false)
}

pub(crate) fn changed(app: &tauri::AppHandle, thread_id: Option<&str>) {
    let _ = app.emit("computer-use-changed", json!({ "sessionId": thread_id }));
}

static SERVICE: OnceLock<Arc<Service>> = OnceLock::new();

/// The service, started on first use. Its helper starts later still, on the first call
/// that needs it.
pub(crate) fn service(app: &tauri::AppHandle) -> Option<Arc<Service>> {
    if let Some(service) = SERVICE.get() {
        return Some(service.clone());
    }
    let state_dir = app.path().app_data_dir().ok()?;
    let resources = app.path().resource_dir().ok();
    let launch = move || {
        let given = std::env::var(BINARY_ENV).ok();
        let executable = std::env::current_exe().ok();
        let candidates = binary_candidates(given.as_deref(), resources.as_deref(), executable.as_deref());
        find_binary(&candidates, |path| path.is_file()).map(|program| Launch { program, args: Vec::new() })
    };
    let os = std::env::consts::OS;
    // The ancestry only matters where a helper can list windows.
    let own_pids = if platform_supported(os) { own_process_ancestry() } else { super::policy::own_process_floor() };
    let service = Service::new(Arc::new(AppHost { app: app.clone() }), DriverClient::new(DriverOptions::new(os, launch)), os, ServiceOptions::new(state_dir, own_pids));
    Some(SERVICE.get_or_init(|| service).clone())
}

/// The service if anything has used it; ending a turn must not start it.
pub(crate) fn started() -> Option<Arc<Service>> {
    SERVICE.get().cloned()
}

/// Adds the computer-use tools when the setting, the platform and the helper allow;
/// otherwise returns the host unchanged.
pub(crate) fn with_computer_use(base: Option<ToolHost>, app: tauri::AppHandle, session_id: String, stopped: Arc<AtomicBool>) -> Option<ToolHost> {
    if !platform_supported(std::env::consts::OS) {
        return base;
    }
    let read = app.state::<AppState>().store.lock().ok().map(|store| (settings::read(&store).ok(), store.session(&session_id).ok().map(|session| session.mode)));
    let Some((Some(settings), Some(session_mode))) = read else { return base };
    if settings.mode == Mode::Off {
        return base;
    }
    let Some(service) = service(&app).filter(|service| service.driver().unavailable_reason().is_none()) else { return base };
    let caller = Caller { thread_id: session_id, started_full_access: session_mode == "full" };
    // Plan mode is for reading and planning: looking is fine, input is not.
    let plan = session_mode == "plan";
    let mut tools = base.as_ref().map_or_else(Vec::new, |host| host.tools.clone());
    let approval_tools = base.as_ref().map_or_else(Default::default, |host| host.approval_tools.clone());
    tools.extend(catalog(settings.mode));
    Some(ToolHost {
        tools,
        // The gate asks for itself, with wording that never shows typed text, so these
        // tools are not on the harness's ask-first list.
        approval_tools,
        handler: Arc::new(move |name, args| {
            let (base, service, caller, stopped) = (base.clone(), service.clone(), caller.clone(), stopped.clone());
            Box::pin(async move {
                if stopped.load(Ordering::SeqCst) {
                    return Err("Task has stopped.".into());
                }
                let Some((command, input)) = command_for(&name) else {
                    return match base {
                        Some(base) => (base.handler)(name, args).await,
                        None => Err("Unknown PandaCode tool.".into()),
                    };
                };
                if input && plan {
                    return Err("This host action is unavailable in Plan mode".into());
                }
                service.handle(&caller, &request_body(command, &args)).await.map_err(|refusal| refusal_text(&refusal))
            })
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer_use::policy::{validate_request, ErrorCode};

    #[test]
    fn every_command_has_a_tool_the_host_accepts() {
        let control = catalog(Mode::Control);
        assert_eq!(control.len(), TOOLS.len());
        let names: Vec<&str> = control.iter().map(|tool| tool["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOLS.map(|(name, _, _)| format!("pandacode_{name}")).iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(catalog(Mode::Observe).iter().map(|tool| tool["name"].as_str().unwrap()).collect::<Vec<_>>(), ["pandacode_computer_status", "pandacode_computer_list_windows", "pandacode_computer_observe", "pandacode_computer_screenshot"]);
        // The host's own checks: names, schemas and the size of the whole catalog.
        let host = ToolHost { tools: control.clone(), approval_tools: Default::default(), handler: Arc::new(|_, args| Box::pin(async move { Ok(args) })) };
        assert!(crate::providers::host_tools_accept(&host), "the tool host rejects the catalog");
        assert!(serde_json::to_string(&control).unwrap().len() < 16 * 1024);
        assert_eq!(command_for("pandacode_computer_scroll_at"), Some(("scroll-at", true)));
        assert_eq!(command_for("pandacode_computer_observe"), Some(("observe", false)));
        assert_eq!(command_for("computer_observe"), None);
        assert_eq!(command_for("pandacode_preview_file"), None);
        assert!(names.iter().all(|name| self_gated(name)) && !self_gated("pandacode_preview_file") && !self_gated("pandacode_computer_shell"));
    }

    /// A value the argument's schema allows, so every tool's own schema is run through the gate's validation.
    fn sample(schema: &Value) -> Value {
        match schema["type"].as_str() {
            Some("integer") => json!(schema["minimum"].as_i64().unwrap_or(1).max(1)),
            Some("string") if schema["enum"].is_array() => schema["enum"][0].clone(),
            _ => json!("enter"),
        }
    }

    #[test]
    fn tool_arguments_are_the_arguments_the_gate_validates() {
        for tool in catalog(Mode::Control) {
            let name = tool["name"].as_str().unwrap();
            let (command, input) = command_for(name).unwrap();
            let properties = tool["inputSchema"]["properties"].as_object().unwrap();
            let required: Vec<&str> = tool["inputSchema"]["required"].as_array().unwrap().iter().map(|name| name.as_str().unwrap()).collect();
            let all: serde_json::Map<String, Value> = properties.iter().map(|(name, schema)| (name.clone(), sample(schema))).collect();
            let request = validate_request(&request_body(command, &Value::Object(all.clone()))).unwrap_or_else(|refusal| panic!("{name}: {}", refusal.message));
            assert_eq!(request.command(), command);
            assert_eq!(matches!(request, crate::computer_use::policy::Request::Input(_)), input, "{name}");
            // Only what the schema requires is required, and nothing else is accepted.
            let only_required: serde_json::Map<String, Value> = all.iter().filter(|(name, _)| required.contains(&name.as_str())).map(|(name, value)| (name.clone(), value.clone())).collect();
            assert!(validate_request(&request_body(command, &Value::Object(only_required.clone()))).is_ok(), "{name}");
            for missing in &required {
                let mut body = only_required.clone();
                body.remove(*missing);
                assert_eq!(validate_request(&request_body(command, &Value::Object(body))).unwrap_err().code, ErrorCode::MissingArgument, "{name} without {missing}");
            }
            let mut extra = only_required;
            extra.insert("other".into(), json!(1));
            assert_eq!(validate_request(&request_body(command, &Value::Object(extra))).unwrap_err().code, ErrorCode::UnexpectedArgument);
            assert_eq!(tool["inputSchema"]["additionalProperties"], json!(false));
        }
        // The command always comes from the tool, never from the arguments.
        assert_eq!(request_body("status", &json!({"command":"click"})), json!({"command":"status"}));
        assert_eq!(request_body("status", &json!("click")), json!({"command":"status"}));
    }

    #[test]
    fn the_running_turn_and_waiting_approvals_are_read_from_the_agents() {
        let store = std::sync::Mutex::new(crate::storage::Store::open(std::path::Path::new(":memory:")).unwrap());
        let (full, ask) = {
            let store = store.lock().unwrap();
            let project = store.add_project(&std::env::temp_dir()).unwrap();
            (store.create_session(&project.id, "codex", "model", "full").unwrap().id, store.create_session(&project.id, "codex", "model", "ask").unwrap().id)
        };
        let agents: crate::Agents = Default::default();
        assert_eq!(running_turn(&agents, &store, &full), None, "no agent, no turn");
        assert_eq!(provider_approval_pending(&agents), Some(false));
        let agent = |stopped: bool| crate::RunningAgent::new(tokio::sync::mpsc::unbounded_channel().0, Arc::new(AtomicBool::new(stopped)));
        let (running, other) = (agent(false), agent(false));
        let (run_id, approvals, requests) = (running.run_id.clone(), running.approvals.clone(), running.pending_requests.clone());
        agents.lock().unwrap().insert(full.clone(), running);
        agents.lock().unwrap().insert(ask.clone(), other);
        let turn = running_turn(&agents, &store, &full).unwrap();
        assert_eq!((turn.turn_id.as_str(), turn.full_access), (run_id.as_str(), true));
        assert!(!running_turn(&agents, &store, &ask).unwrap().full_access, "only Full access skips routine approvals");
        assert_eq!(running_turn(&agents, &store, "missing"), None);
        // A question for the user does not pause input; an approval does, and so does
        // a waiting request nothing is known about.
        let request = |kind: &str| crate::providers::AgentEvent { session_id: full.clone(), kind: kind.into(), text: String::new(), request_id: None, data: None, tool: None };
        approvals.lock().unwrap().insert("q".into());
        requests.lock().unwrap().insert("q".into(), request("input_request"));
        assert_eq!(provider_approval_pending(&agents), Some(false));
        approvals.lock().unwrap().insert("a".into());
        assert_eq!(provider_approval_pending(&agents), Some(true));
        requests.lock().unwrap().insert("a".into(), request("approval"));
        assert_eq!(provider_approval_pending(&agents), Some(true));
        approvals.lock().unwrap().clear();
        assert_eq!(provider_approval_pending(&agents), Some(false));
        // A stopped agent has no turn, and neither has an archived thread.
        agents.lock().unwrap().get(&full).unwrap().stopped.store(true, Ordering::SeqCst);
        assert_eq!(running_turn(&agents, &store, &full), None);
        store.lock().unwrap().db.execute("UPDATE sessions SET archived=1 WHERE id=?1", [&ask]).unwrap();
        assert_eq!(running_turn(&agents, &store, &ask), None);
    }

    #[test]
    fn a_refusal_reads_with_its_code_and_what_was_sent() {
        let refusal = |effect| ComputerUseError { code: ErrorCode::DriverFailed, message: "The computer-use driver timed out.".into(), effect };
        assert_eq!(refusal_text(&refusal(None)), "CU-EXT-004: The computer-use driver timed out.");
        assert_eq!(refusal_text(&refusal(Some(Effect::NotDispatched))), "CU-EXT-004: The computer-use driver timed out. No input was sent.");
        assert_eq!(refusal_text(&refusal(Some(Effect::DispatchedUnknown))), "CU-EXT-004: The computer-use driver timed out. The input may have been sent: look before you retry.");
        assert_eq!(refusal_text(&refusal(Some(Effect::Dispatched))), "CU-EXT-004: The computer-use driver timed out. The input was sent.");
        assert_eq!(refusal_text(&crate::computer_use::policy::turned_off()), "CU-CON-001: Computer use is turned off in PandaCode settings.");
    }
}
