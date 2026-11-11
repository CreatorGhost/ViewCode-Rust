//! The rules of the computer-use gate, with no I/O: request shape checks, the protected
//! apps, the destructive-target heuristic, when input asks, approval wording, the final
//! check before input and how a helper failure is reported. The service applies them;
//! nothing here touches the helper or the screen.
//!
//! The gate must stay safe when the agent ignores every instruction and sends raw
//! requests itself, so each decision takes what is true now ([`Live`]) and is asked again
//! at every step: when the request arrives, before the helper prepares a target, and
//! immediately before native input.
//!
//! The protected-apps rule and the destructive heuristic are name matching, not a
//! detector. They exist so an agent cannot quietly drive a password manager or the app's
//! own approval buttons, and so obviously risky clicks always reach the user, but a
//! renamed control or an unknown app is not caught. What actually stops an agent
//! answering an approval for itself is the pause: no input while any approval waits.

use super::protocol::{Dispatched, DispatchTarget, DriverElement, DriverFailure, DriverRequest, DriverWindow, ElementIdentity, FailureKind, FailureReason, MouseButton, Point, Rect};
use super::targets::{RefLookup, RefRecord, RefRetired, ShotLookup, ShotRecord, ShotRetired, ThreadTargets, WindowRecord};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The app's name in every message that names the app itself, and in the rule that keeps
/// agents away from its own windows. The one place to change when the app is renamed.
pub(crate) const APP_NAME: &str = "PandaCode";
/// The command an agent runs, as refusals name it when they say what to do next. Kept as
/// ViewCode's so its agent instructions and the helper's `VIEWCODE_COMPUTER_*` names agree.
pub(crate) const LAUNCHER_NAME: &str = "viewcode-computer";
/// The app name shown on an approval card.
pub(crate) const APPROVAL_APP_NAME: &str = "Computer use";
/// Ids of the gate's own approvals, to tell them from a provider's.
pub(crate) const REQUEST_ID_PREFIX: &str = "computer-use:";
/// No answer within this long counts as a decline.
pub(crate) const APPROVAL_TIMEOUT_SECS: u64 = 600;
pub(crate) const MAX_OBSERVED_ELEMENTS: u32 = 300;
/// Longest edge of a returned image. Vision models read about 1568px best.
pub(crate) const DEFAULT_SCREENSHOT_SIZE: u32 = 1568;
/// Recent screenshots are the agent's working memory in coordinate mode.
pub(crate) const SCREENSHOTS_KEPT: usize = 8;
/// How long input gets to take effect before the follow-up screenshot.
pub(crate) const SETTLE_DELAY_MS: u64 = 350;
/// The helper clips every label and value it reports to this many characters.
const VALUE_MAX_CHARS: usize = 200;

// ── Settings ────────────────────────────────────────────────────────────────

/// `Off`: nothing is offered. `Observe` (look only): windows, element trees and
/// screenshots. `Control`: also input, each action decided by the rules below.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode {
    /// Also what an unreadable setting counts as: it must never turn computer use on.
    #[default]
    Off,
    Observe,
    Control,
}

/// When input asks the user first. `Thread`: follow the thread's permission mode (Full
/// access skips routine approvals). `Risky`: only destructive-looking actions ask, in any
/// mode. `Never`: nothing asks. The protected apps and the pause while any approval waits
/// apply in every case.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Approvals {
    /// Also what an unreadable setting counts as: the strictest behaviour.
    #[default]
    Thread,
    Risky,
    Never,
}

/// Whether input may bring windows to the front. `Allow`: whenever it needs to. `Ask`:
/// the first such action in a turn asks once whether to show the task on screen or keep
/// it in the background (actions by reference only).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Screen {
    Allow,
    Ask,
}

impl Screen {
    /// What an unreadable setting counts as. The setting itself defaults to `Allow`.
    pub(crate) const UNREADABLE: Screen = Screen::Ask;
}

// ── Errors ──────────────────────────────────────────────────────────────────

/// `CU-VAL-*` invalid request, `CU-NOT-*` unknown target, `CU-CON-*` refused by policy or
/// state, `CU-EXT-*` helper or OS, `CU-INT-*` a bug in the app.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ErrorCode {
    /// Unknown command.
    #[serde(rename = "CU-VAL-001")]
    UnknownCommand,
    /// Required argument missing.
    #[serde(rename = "CU-VAL-002")]
    MissingArgument,
    /// Argument has the wrong type or is out of range.
    #[serde(rename = "CU-VAL-003")]
    InvalidArgument,
    /// Argument not accepted by this command.
    #[serde(rename = "CU-VAL-004")]
    UnexpectedArgument,
    /// Window id unknown to this thread (never listed, or closed).
    #[serde(rename = "CU-NOT-001")]
    UnknownWindow,
    /// Element ref unknown to this thread.
    #[serde(rename = "CU-NOT-002")]
    UnknownRef,
    /// Screenshot (shot) id unknown to this thread.
    #[serde(rename = "CU-NOT-003")]
    UnknownShot,
    /// Computer use is off, or not granted to this session.
    #[serde(rename = "CU-CON-001")]
    Off,
    /// Input requested while only observation is allowed.
    #[serde(rename = "CU-CON-002")]
    ObserveOnly,
    /// Ref is from an older observation, or the element changed since.
    #[serde(rename = "CU-CON-003")]
    StaleRef,
    /// The user declined the action, or no answer arrived.
    #[serde(rename = "CU-CON-004")]
    Declined,
    /// The target app is never controllable (password managers, the app itself, ...).
    #[serde(rename = "CU-CON-005")]
    ProtectedApp,
    /// No turn is running in this thread; computer actions belong to a turn.
    #[serde(rename = "CU-CON-006")]
    NoTurn,
    /// Coordinates come from a screenshot that is no longer the window's newest, or the
    /// window moved, resized or closed since it was taken.
    #[serde(rename = "CU-CON-007")]
    StaleShot,
    /// Input is paused while any approval waits for the user, so an agent can never click
    /// an approval (in the app, a browser tab or a mirrored screen) for itself or another agent.
    #[serde(rename = "CU-CON-008")]
    ApprovalPending,
    /// The user is using the mouse or keyboard, so input that would take the screen waits
    /// until they pause instead of fighting them for it.
    #[serde(rename = "CU-CON-009")]
    UserActive,
    /// Platform unsupported or the helper could not start.
    #[serde(rename = "CU-EXT-001")]
    DriverUnavailable,
    /// Accessibility permission is missing for the app.
    #[serde(rename = "CU-EXT-002")]
    AccessibilityMissing,
    /// Screen Recording permission is missing (screenshots only).
    #[serde(rename = "CU-EXT-003")]
    ScreenRecordingMissing,
    /// The helper failed or timed out performing the action.
    #[serde(rename = "CU-EXT-004")]
    DriverFailed,
    /// The helper answered with something that is not a valid result.
    #[serde(rename = "CU-EXT-005")]
    DriverMalformed,
    /// The launcher could not reach the app.
    #[serde(rename = "CU-EXT-006")]
    Unreachable,
    /// Unexpected internal failure.
    #[serde(rename = "CU-INT-001")]
    Internal,
}

/// Input actions only. `Dispatched`: the OS accepted it; that is not proof of an effect,
/// so the agent re-observes. `DispatchedUnknown` must never be replayed blindly.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Effect {
    Dispatched,
    NotDispatched,
    DispatchedUnknown,
}

/// A refusal, as the agent reads it and as the helper is answered. The message never
/// carries typed text, values or window contents.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComputerUseError {
    pub(crate) code: ErrorCode,
    pub(crate) message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) effect: Option<Effect>,
}

fn error(code: ErrorCode, message: impl Into<String>) -> ComputerUseError {
    ComputerUseError { code, message: message.into(), effect: None }
}

/// A refusal of input that was certainly not sent.
fn not_dispatched(code: ErrorCode, message: impl Into<String>) -> ComputerUseError {
    ComputerUseError { code, message: message.into(), effect: Some(Effect::NotDispatched) }
}

pub(crate) fn turned_off() -> ComputerUseError {
    error(ErrorCode::Off, format!("Computer use is turned off in {APP_NAME} settings."))
}

pub(crate) fn observe_only() -> ComputerUseError {
    not_dispatched(ErrorCode::ObserveOnly, format!("Computer use is set to observe only; input actions are unavailable. Ask the user to switch it to control in {APP_NAME} settings."))
}

pub(crate) fn internal_error() -> ComputerUseError {
    error(ErrorCode::Internal, format!("{APP_NAME} hit an internal error running this command."))
}

fn denied_app(effect: Option<Effect>) -> ComputerUseError {
    ComputerUseError { code: ErrorCode::ProtectedApp, message: format!("This app can never be observed or controlled through computer use (password managers, system settings and {APP_NAME} itself). Ask the user to do this step."), effect }
}

fn unknown_window(window_id: u64, effect: Option<Effect>) -> ComputerUseError {
    ComputerUseError { code: ErrorCode::UnknownWindow, message: format!("Window {window_id} is not known to this thread (never listed, or closed). Run `{LAUNCHER_NAME} list-windows`."), effect }
}

pub(crate) fn input_paused() -> ComputerUseError {
    not_dispatched(ErrorCode::ApprovalPending, "Input is paused while an approval waits for the user; wait for it to be answered, then observe again.")
}

/// A screenshot is refused while a protected window overlaps the target.
pub(crate) fn overlaid() -> ComputerUseError {
    error(ErrorCode::ProtectedApp, "A protected window overlaps this window; move it away and try again.")
}

/// The helper's answer when no check is installed, or its caller gave up: never permission.
pub(crate) fn cancelled_before_dispatch() -> ComputerUseError {
    not_dispatched(ErrorCode::Declined, "The action was cancelled before dispatch.")
}

const KEPT_IN_BACKGROUND: &str = "The user chose to keep this task in the background, so nothing may bring a window to the front until the turn ends. Act on controls by reference (press, set-value, type --ref) and use screenshots to look; if the task cannot be done that way, tell the user.";

// ── Requests ────────────────────────────────────────────────────────────────

/// One command from an agent, already validated. `Debug` names the command only, so a
/// request that reaches a log never carries what the agent typed.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Request {
    Status,
    ListWindows { app: Option<String> },
    /// `query`: case-insensitive substring over label and value; narrows a large tree.
    Observe { window: u64, query: Option<String> },
    Screenshot { window: u64, max_size: Option<u32> },
    Input(InputRequest),
}

/// Commands that act. Ref commands name an observed control. Coordinate commands give
/// pixels in screenshot `shot`, which must be the newest screenshot of its window; they
/// bring the window to the front first, because synthetic pointer input lands on
/// whatever is on top.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum InputRequest {
    Press { reference: u64 },
    SetValue { reference: u64, value: String },
    Type { reference: u64, text: String },
    Key { window: u64, keys: String },
    Scroll { reference: u64, dx: i32, dy: i32 },
    Click { shot: u64, x: u64, y: u64, button: Option<MouseButton>, count: Option<u32> },
    Drag { shot: u64, from_x: u64, from_y: u64, to_x: u64, to_y: u64 },
    Move { shot: u64, x: u64, y: u64 },
    ScrollAt { shot: u64, x: u64, y: u64, dx: i32, dy: i32 },
    /// Types into whatever has keyboard focus in the window (after bringing it to the front).
    TypeFocused { window: u64, text: String },
    /// Brings the window to the front and nothing else.
    Focus { window: u64 },
}

impl Request {
    pub(crate) fn command(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::ListWindows { .. } => "list-windows",
            Self::Observe { .. } => "observe",
            Self::Screenshot { .. } => "screenshot",
            Self::Input(input) => input.command(),
        }
    }
}

impl InputRequest {
    pub(crate) fn command(&self) -> &'static str {
        match self {
            Self::Press { .. } => "press",
            Self::SetValue { .. } => "set-value",
            Self::Type { .. } => "type",
            Self::Key { .. } => "key",
            Self::Scroll { .. } => "scroll",
            Self::Click { .. } => "click",
            Self::Drag { .. } => "drag",
            Self::Move { .. } => "move",
            Self::ScrollAt { .. } => "scroll-at",
            Self::TypeFocused { .. } => "type-focused",
            Self::Focus { .. } => "focus",
        }
    }

    /// The image pixels a coordinate command names, in order.
    fn pixels(&self) -> Vec<(u64, u64)> {
        match *self {
            Self::Click { x, y, .. } | Self::Move { x, y, .. } | Self::ScrollAt { x, y, .. } => vec![(x, y)],
            Self::Drag { from_x, from_y, to_x, to_y, .. } => vec![(from_x, from_y), (to_x, to_y)],
            _ => Vec::new(),
        }
    }
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Request({})", self.command())
    }
}

impl std::fmt::Debug for InputRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InputRequest({})", self.command())
    }
}

/// `enter`, `cmd+shift+z`: modifiers joined by `+`, then exactly one key.
static KEY_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:(?:cmd|ctrl|alt|shift|meta|option|super)\+){0,4}(?:[a-z0-9]|f(?:[1-9]|1[0-9]|2[0-4])|enter|return|tab|escape|space|backspace|delete|up|down|left|right|home|end|pageup|pagedown)$").expect("key pattern"));

/// Arguments each command takes besides `command`: required, then optional.
fn command_arguments(command: &str) -> Option<(&'static [&'static str], &'static [&'static str])> {
    Some(match command {
        "status" => (&[], &[]),
        "list-windows" => (&[], &["app"]),
        "observe" => (&["window"], &["query"]),
        "screenshot" => (&["window"], &["maxSize"]),
        "press" => (&["ref"], &[]),
        "set-value" => (&["ref", "value"], &[]),
        "type" => (&["ref", "text"], &[]),
        "key" => (&["window", "keys"], &[]),
        "scroll" => (&["ref", "dx", "dy"], &[]),
        "click" => (&["shot", "x", "y"], &["button", "count"]),
        "drag" => (&["shot", "fromX", "fromY", "toX", "toY"], &[]),
        "move" => (&["shot", "x", "y"], &[]),
        "scroll-at" => (&["shot", "x", "y", "dx", "dy"], &[]),
        "type-focused" => (&["window", "text"], &[]),
        "focus" => (&["window"], &[]),
        _ => return None,
    })
}

/// A JSON number that is a whole number (`3` and `3.0` alike, as the agent's JSON may say either).
fn integer(value: &Value) -> Option<i64> {
    const SAFE: f64 = 9_007_199_254_740_991.0;
    value.as_i64().or_else(|| value.as_f64().filter(|number| number.fract() == 0.0 && number.abs() <= SAFE).map(|number| number as i64))
}

fn integer_between(value: &Value, minimum: i64, maximum: i64) -> bool {
    integer(value).is_some_and(|number| (minimum..=maximum).contains(&number))
}

/// String length as the contract counts it (UTF-16 units), so both ends agree on a limit.
fn text_between(value: &Value, minimum: usize, maximum: usize) -> bool {
    value.as_str().is_some_and(|text| (minimum..=maximum).contains(&text.encode_utf16().count()))
}

/// Whether `value` is acceptable for the argument `name`. Every argument name means the
/// same thing in every command that takes it.
fn valid_argument(name: &str, value: &Value) -> bool {
    match name {
        "window" | "ref" | "shot" => integer_between(value, 1, i64::MAX),
        "app" | "query" => text_between(value, 0, 200),
        // Typed and set text. Bounded so a runaway agent cannot paste a novel.
        "value" | "text" => text_between(value, 1, 10_000),
        "keys" => value.as_str().is_some_and(|keys| KEY_PATTERN.is_match(keys)),
        "dx" | "dy" => integer_between(value, -50, 50),
        // Pixel in the screenshot image, from its top-left corner. Bounds are checked against the shot.
        "x" | "y" | "fromX" | "fromY" | "toX" | "toY" => integer_between(value, 0, i64::MAX),
        "maxSize" => integer_between(value, 256, 2560),
        "button" => matches!(value.as_str(), Some("left" | "right" | "middle")),
        "count" => integer_between(value, 1, 3),
        _ => false,
    }
}

fn clip_name(name: &str) -> String {
    if name.chars().count() > 40 {
        format!("{}…", name.chars().take(40).collect::<String>())
    } else {
        name.to_owned()
    }
}

/// Validates an untrusted request body. Messages name arguments, never their values: a
/// rejected `type` request must not echo the text back into logs.
pub(crate) fn validate_request(body: &Value) -> Result<Request, ComputerUseError> {
    let Value::Object(body) = body else { return Err(error(ErrorCode::UnknownCommand, "The request must be a JSON object with a command.")) };
    let Some((command, (required, optional))) = body.get("command").and_then(Value::as_str).and_then(|command| Some((command, command_arguments(command)?))) else {
        return Err(error(ErrorCode::UnknownCommand, format!("Unknown command. Run `{LAUNCHER_NAME} help` for the list of commands.")));
    };
    let unexpected: Vec<String> = body.keys().filter(|key| *key != "command" && !required.contains(&key.as_str()) && !optional.contains(&key.as_str())).map(|key| clip_name(key)).collect();
    if !unexpected.is_empty() {
        return Err(error(ErrorCode::UnexpectedArgument, format!("`{command}` does not accept: {}.", unexpected.join(", "))));
    }
    let missing: Vec<&str> = required.iter().copied().filter(|key| !body.contains_key(*key)).collect();
    if !missing.is_empty() {
        return Err(error(ErrorCode::MissingArgument, format!("`{command}` requires: {}.", missing.join(", "))));
    }
    let invalid: Vec<&str> = required.iter().chain(optional).copied().filter(|key| body.get(*key).is_some_and(|value| !valid_argument(key, value))).collect();
    if !invalid.is_empty() {
        return Err(error(ErrorCode::InvalidArgument, format!("`{command}` has an invalid {}. Run `{LAUNCHER_NAME} help` for the accepted values.", invalid.join(", "))));
    }
    // Everything present was checked above, so these reads cannot fail.
    let id = |name: &str| body.get(name).and_then(integer).unwrap_or_default() as u64;
    let delta = |name: &str| body.get(name).and_then(integer).unwrap_or_default() as i32;
    let text = |name: &str| body.get(name).and_then(Value::as_str).unwrap_or_default().to_owned();
    let optional_text = |name: &str| body.get(name).and_then(Value::as_str).map(str::to_owned);
    let small = |name: &str| body.get(name).and_then(integer).map(|number| number as u32);
    Ok(match command {
        "status" => Request::Status,
        "list-windows" => Request::ListWindows { app: optional_text("app") },
        "observe" => Request::Observe { window: id("window"), query: optional_text("query") },
        "screenshot" => Request::Screenshot { window: id("window"), max_size: small("maxSize") },
        "press" => Request::Input(InputRequest::Press { reference: id("ref") }),
        "set-value" => Request::Input(InputRequest::SetValue { reference: id("ref"), value: text("value") }),
        "type" => Request::Input(InputRequest::Type { reference: id("ref"), text: text("text") }),
        "key" => Request::Input(InputRequest::Key { window: id("window"), keys: text("keys") }),
        "scroll" => Request::Input(InputRequest::Scroll { reference: id("ref"), dx: delta("dx"), dy: delta("dy") }),
        "click" => Request::Input(InputRequest::Click {
            shot: id("shot"),
            x: id("x"),
            y: id("y"),
            button: body.get("button").and_then(|button| serde_json::from_value(button.clone()).ok()),
            count: small("count"),
        }),
        "drag" => Request::Input(InputRequest::Drag { shot: id("shot"), from_x: id("fromX"), from_y: id("fromY"), to_x: id("toX"), to_y: id("toY") }),
        "move" => Request::Input(InputRequest::Move { shot: id("shot"), x: id("x"), y: id("y") }),
        "scroll-at" => Request::Input(InputRequest::ScrollAt { shot: id("shot"), x: id("x"), y: id("y"), dx: delta("dx"), dy: delta("dy") }),
        "type-focused" => Request::Input(InputRequest::TypeFocused { window: id("window"), text: text("text") }),
        _ => Request::Input(InputRequest::Focus { window: id("window") }),
    })
}

/// The first decision for every request: the setting, the request's shape, and that look
/// only never takes input. The mode is read fresh for each request.
pub(crate) fn admit(mode: Mode, body: &Value) -> Result<Request, ComputerUseError> {
    if mode == Mode::Off {
        return Err(turned_off());
    }
    let request = validate_request(body)?;
    if matches!(request, Request::Input(_)) && mode != Mode::Control {
        return Err(observe_only());
    }
    Ok(request)
}

/// Where a thread's screenshots are written. Providers that take directory grants are
/// given this folder, so the agent can open its own screenshots without a read prompt;
/// other threads' screenshots stay ungranted. Thread ids are opaque, so they are kept
/// from becoming path syntax.
pub(crate) fn screenshots_dir(state_dir: &Path, thread_id: &str) -> PathBuf {
    let safe: String = thread_id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
    state_dir.join("computer-use").join("screenshots").join(safe)
}

/// Whether an observed control matches the agent's `query` (label or value, any case).
/// Applied again on this side so a ref is only minted for what the agent is shown.
pub(crate) fn matches_query(element: &DriverElement, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    needle.is_empty() || element.label.to_lowercase().contains(&needle) || element.value.as_ref().is_some_and(|value| value.to_lowercase().contains(&needle))
}

// ── Protected apps ──────────────────────────────────────────────────────────

/// Lowercase words of an app name, without parenthesized channel suffixes and a trailing
/// "helper", so "PandaCode (Alpha)", "PandaCode Helper (Renderer)" and "pandacode" all
/// read "pandacode".
fn normalize(value: &str) -> String {
    static PARENTHESIZED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\([^)]*\)").expect("pattern"));
    static SEPARATORS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").expect("pattern"));
    let lower = value.to_lowercase();
    let words = PARENTHESIZED.replace_all(&lower, " ");
    let words = SEPARATORS.replace_all(&words, " ");
    let words = words.trim();
    let words = if words == "helper" { "" } else { words.strip_suffix(" helper").unwrap_or(words) };
    words.trim().to_owned()
}

/// Apps whose windows hold secrets or system security controls. Never observed or
/// controlled, and a screenshot is refused while one overlaps the target, because a
/// region capture shows whatever is on top.
const PROTECTED_APP_NAMES: [&str; 16] = ["1password", "1password 7", "1password 8", "nordpass", "bitwarden", "lastpass", "dashlane", "keeper", "keeper password manager", "keepassxc", "passwords", "keychain access", "system settings", "system preferences", "securityagent", "loginwindow"];
/// Executable file names (normalized, without extension) that mark a protected app.
const PROTECTED_EXECUTABLES: [&str; 10] = ["1password", "nordpass", "bitwarden", "lastpass", "dashlane", "keeper", "keeperpasswordmanager", "keepassxc", "securityagent", "loginwindow"];
/// Names this app and the apps it descends from ship under, after `normalize` drops the
/// channel suffix: ViewCode (which this port follows) and T3 Code. Each shows approval
/// buttons an agent must never reach. [`APP_NAME`] is matched on top of these.
const RELATED_APP_NAMES: [&str; 3] = ["viewcode", "t3 code", "t3code"];

fn is_protected_name(name: &str) -> bool {
    PROTECTED_APP_NAMES.contains(&name)
}

fn is_protected_executable(name: &str) -> bool {
    PROTECTED_EXECUTABLES.contains(&name)
}

/// The app's own name, however it is spaced, or a related app's.
fn is_own_name(name: &str) -> bool {
    let own = normalize(APP_NAME);
    name == own || name.replace(' ', "") == own.replace(' ', "") || RELATED_APP_NAMES.contains(&name)
}

/// Whether the window's app is one of a set, by its own name or by any `<Name>.app`
/// bundle on its executable path (`app_identifier` is the path the helper reports, never
/// a bundle id). The bundle match catches helpers inside `PandaCode.app` whose executable
/// has another name.
fn app_matches(app: &str, app_identifier: Option<&str>, names: fn(&str) -> bool, executables: fn(&str) -> bool) -> bool {
    if names(&normalize(app)) {
        return true;
    }
    let Some(identifier) = app_identifier.map(str::trim).filter(|identifier| !identifier.is_empty()) else { return false };
    let segments: Vec<&str> = identifier.split(['/', '\\']).filter(|segment| !segment.is_empty()).collect();
    // `<Name>.app`, any case: the name without the extension.
    let bundle_name = |segment: &str| {
        let split = segment.len().checked_sub(4).filter(|split| segment.is_char_boundary(*split))?;
        segment[split..].eq_ignore_ascii_case(".app").then(|| normalize(&segment[..split]))
    };
    if segments.iter().filter_map(|segment| bundle_name(segment)).any(|name| names(&name)) {
        return true;
    }
    segments.last().is_some_and(|executable| {
        let lower = executable.to_ascii_lowercase();
        let stem = [".exe", ".appimage"].iter().find(|extension| lower.ends_with(*extension)).map_or(*executable, |extension| &executable[..executable.len() - extension.len()]);
        executables(&normalize(stem))
    })
}

/// The parts of a window the protected-apps rule reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AppWindow<'a> {
    pub(crate) app: &'a str,
    pub(crate) app_identifier: Option<&'a str>,
    pub(crate) title: Option<&'a str>,
    pub(crate) pid: Option<u32>,
}

impl<'a> From<&'a DriverWindow> for AppWindow<'a> {
    fn from(window: &'a DriverWindow) -> Self {
        Self { app: &window.app, app_identifier: window.app_identifier.as_deref(), title: Some(&window.title), pid: Some(window.pid) }
    }
}

impl<'a> From<&'a WindowRecord> for AppWindow<'a> {
    fn from(window: &'a WindowRecord) -> Self {
        Self { app: &window.app, app_identifier: window.app_identifier.as_deref(), title: Some(&window.title), pid: Some(window.pid) }
    }
}

/// Password managers and system security UI.
pub(crate) fn is_protected_app(window: AppWindow<'_>) -> bool {
    app_matches(window.app, window.app_identifier, is_protected_name, is_protected_executable)
}

const BROWSER_APP_NAMES: [&str; 15] = ["safari", "safari technology preview", "google chrome", "google chrome canary", "chromium", "arc", "brave browser", "microsoft edge", "firefox", "firefox developer edition", "opera", "vivaldi", "orion", "zen", "zen browser"];

/// The app's web page as a browser window title: "PandaCode", "PandaCode (Alpha) - Google Chrome", …
static OWN_PAGE_TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"(?i)^\s*(?:{}|viewcode|t3 code)(?:\s*\([^)]*\))?(?:\s*[-\x{{2013}}\x{{2014}}|\x{{00b7}}].*)?\s*$", regex::escape(APP_NAME))).expect("title pattern"));

/// Defense in depth only: a browser tab showing the app is recognised by its title, which
/// a tab switch or a missing title defeats. The real guarantee against an agent answering
/// its own approval is the input pause while any approval waits.
fn is_own_browser_window(window: AppWindow<'_>) -> bool {
    window.title.is_some_and(|title| BROWSER_APP_NAMES.contains(&window.app.trim().to_lowercase().as_str()) && OWN_PAGE_TITLE.is_match(title))
}

/// Whether every command but `list-windows` is refused on this window's app by name:
/// protected apps, the app itself and a browser tab showing it.
pub(crate) fn is_denylisted_app(window: AppWindow<'_>) -> bool {
    is_protected_app(window) || app_matches(window.app, window.app_identifier, is_own_name, is_own_name) || is_own_browser_window(window)
}

/// The least the app knows about its own process ancestry without asking the OS: itself
/// and its parent. A window owned by any process in the ancestry is the app (or the
/// terminal that started it) whatever its name says; the service extends this set with
/// the remaining ancestors once at startup.
pub(crate) fn own_process_floor() -> HashSet<u32> {
    let mut pids = HashSet::from([std::process::id()]);
    #[cfg(unix)]
    {
        let parent = std::os::unix::process::parent_id();
        if parent > 1 {
            pids.insert(parent);
        }
    }
    pids
}

/// The whole protected-apps rule: by name, or a window of the app's own process ancestry.
pub(crate) fn is_denied(window: AppWindow<'_>, own_pids: &HashSet<u32>) -> bool {
    is_denylisted_app(window) || window.pid.is_some_and(|pid| own_pids.contains(&pid))
}

/// Whether two screen rects share any area.
pub(crate) fn rects_intersect(left: &Rect, right: &Rect) -> bool {
    left.x < right.x + right.width && right.x < left.x + left.width && left.y < right.y + right.height && right.y < left.y + left.height
}

/// Whether a screenshot of the window with `handle` must be refused, given a fresh
/// listing: the window itself is denied now, or a password manager or security window
/// overlaps it. A region capture shows whatever is on top, so it would leak that window.
/// Unknown bounds count as overlapping. Asked before the capture and again after it,
/// because a protected window may have appeared while the capture ran.
pub(crate) fn protected_overlay(listed: &[DriverWindow], handle: &str, own_pids: &HashSet<u32>) -> bool {
    let live = listed.iter().find(|entry| entry.handle == handle);
    if live.is_some_and(|live| is_denied(live.into(), own_pids)) {
        return true;
    }
    let target = live.and_then(|live| live.bounds.as_ref());
    listed.iter().any(|entry| entry.handle != handle && is_protected_app(entry.into()) && !matches!((target, entry.bounds.as_ref()), (Some(target), Some(bounds)) if !rects_intersect(target, bounds)))
}

// ── Destructive heuristic ───────────────────────────────────────────────────

// `(?-u:\b)`: word boundaries as ASCII, as the labels were matched in ViewCode.
static DESTRUCTIVE_WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)(delete|remove|erase|discard|uninstall|purchase|buy|pay|checkout|check out|place order|transfer|send|submit|sign out|signout|log out|logout|reset)(?-u:\b)").expect("pattern"));
static FORMAT_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)format(?-u:\b)").expect("pattern"));
// Freeform describes its canvas using this sentence, which is not a format command.
// Other group labels, including "Format disk", remain conservative.
static CANVAS_FORMAT_DESCRIPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^format,\s*move,\s*and resize items within the canvas$").expect("pattern"));

/// A label or role that looks like it commits, pays, sends or destroys something.
pub(crate) fn is_destructive_target(role: &str, label: &str) -> bool {
    let role = role.to_lowercase();
    let role = role.trim();
    let description = format!("{label} {role}").to_lowercase();
    let describes_canvas = role == "group" && CANVAS_FORMAT_DESCRIPTION.is_match(label.trim());
    DESTRUCTIVE_WORDS.is_match(&description) || (FORMAT_WORD.is_match(&description) && !describes_canvas)
}

/// Quit, close and delete chords: cmd/ctrl+q|w, alt+f4, cmd/ctrl+delete|backspace.
pub(crate) fn is_destructive_chord(keys: &str) -> bool {
    let lower = keys.to_lowercase();
    let mut parts: Vec<&str> = lower.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    let has = |modifier: &str| parts.contains(&modifier);
    let command = has("cmd") || has("ctrl") || has("meta") || has("super");
    let alt = has("alt") || has("option");
    (command && matches!(key, "q" | "w" | "delete" | "backspace")) || (alt && key == "f4")
}

/// Whether an input must be approved by the user under the approvals setting. `granted`
/// is Full access on thread and session, or "allow for the rest of this turn".
pub(crate) fn input_needs_approval(approvals: Approvals, destructive: bool, granted: bool) -> bool {
    match approvals {
        Approvals::Never => false,
        Approvals::Risky => destructive,
        Approvals::Thread => destructive || !granted,
    }
}

// ── Approval wording ────────────────────────────────────────────────────────

fn single_line(value: &str, max: usize) -> String {
    let line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max - 1).collect::<String>())
    } else {
        line
    }
}

fn quoted(value: &str) -> String {
    let line = single_line(value, 80);
    if line.is_empty() {
        "(no label)".to_owned()
    } else {
        format!("\"{line}\"")
    }
}

fn or_else(value: String, fallback: &str) -> String {
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value
    }
}

fn describe_element(element: &ElementIdentity) -> String {
    format!("{} {}", or_else(single_line(&element.role, 30), "element"), quoted(&element.label))
}

/// What a coordinate command's first point lands on, read (best effort) before asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Hit {
    /// Not a coordinate command.
    NotApplicable,
    /// Nothing labelled is there, the app exposes no tree, or the read failed.
    Nothing,
    Element(ElementIdentity),
}

/// What an approval names besides the action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApprovalTarget<'a> {
    pub(crate) app: &'a str,
    pub(crate) window_title: &'a str,
    /// The ref's element for ref commands.
    pub(crate) element: Option<ElementIdentity>,
    pub(crate) hit: Hit,
}

/// Typed and set text is shown only as this: the user approves that something is typed
/// and where, and the text itself never reaches a card, an event or a log.
fn character_count(text: &str) -> String {
    let count = text.chars().count();
    format!("{count} character{}", if count == 1 { "" } else { "s" })
}

/// One line for the approval card: action, element role and label, app and window title.
/// Typed and set text appears only as a character count. Coordinate commands give image
/// pixels and say the window comes to the front.
pub(crate) fn describe_input_for_approval(request: &InputRequest, target: &ApprovalTarget<'_>) -> String {
    let element = target.element.as_ref().map(describe_element).unwrap_or_default();
    let app = or_else(single_line(target.app, 40), "an app");
    let location = format!("in {app} — {}", quoted(target.window_title));
    let hit = match &target.hit {
        Hit::NotApplicable => String::new(),
        Hit::Nothing => " (no labelled control there)".to_owned(),
        Hit::Element(hit) => format!(" (on {})", describe_element(hit)),
    };
    let front = "; brings the window to the front";
    match request {
        InputRequest::Press { .. } => format!("Press {element} {location}"),
        InputRequest::SetValue { value, .. } => format!("Set {element} to {} {location}", character_count(value)),
        InputRequest::Type { text, .. } => format!("Type {} into {element} {location}", character_count(text)),
        InputRequest::Key { keys, .. } => format!("Press {keys} {location}"),
        InputRequest::Scroll { dx, dy, .. } => format!("Scroll {element} by ({dx}, {dy}) {location}"),
        InputRequest::Click { x, y, button, count, .. } => {
            let verb = match (button.unwrap_or(MouseButton::Left), count.unwrap_or(1)) {
                (MouseButton::Right, _) => "Right-click",
                (MouseButton::Middle, _) => "Middle-click",
                (MouseButton::Left, 2) => "Double-click",
                (MouseButton::Left, 3) => "Triple-click",
                (MouseButton::Left, _) => "Click",
            };
            format!("{verb} at ({x}, {y}) {location}{hit}{front}")
        }
        InputRequest::Drag { from_x, from_y, to_x, to_y, .. } => format!("Drag from ({from_x}, {from_y}) to ({to_x}, {to_y}) {location}{hit}{front}"),
        InputRequest::Move { x, y, .. } => format!("Move the pointer to ({x}, {y}) {location}{hit}{front}"),
        InputRequest::ScrollAt { x, y, dx, dy, .. } => format!("Scroll by ({dx}, {dy}) at ({x}, {y}) {location}{hit}{front}"),
        InputRequest::TypeFocused { text, .. } => format!("Type {} into the focused field {location}{front}", character_count(text)),
        InputRequest::Focus { .. } => format!("Bring {} in {app} to the front, switching desktops if needed", quoted(target.window_title)),
    }
}

/// The user's answer to an approval card.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ApprovalAnswer {
    Accept,
    AcceptForSession,
    AcceptAlways,
    Decline,
    /// No answer: the wait timed out, the turn ended or the caller gave up.
    Cancel,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ApprovalOption {
    pub(crate) decision: ApprovalAnswer,
    pub(crate) label: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) warning: Option<&'static str>,
}

const ROUTINE_OPTIONS: [ApprovalOption; 3] = [
    ApprovalOption { decision: ApprovalAnswer::Accept, label: "Allow once", warning: None },
    ApprovalOption { decision: ApprovalAnswer::AcceptForSession, label: "Allow for the rest of this turn", warning: None },
    ApprovalOption { decision: ApprovalAnswer::Decline, label: "Decline", warning: None },
];
/// A destructive-looking action is never covered by "the rest of this turn".
const DESTRUCTIVE_OPTIONS: [ApprovalOption; 2] = [
    ApprovalOption { decision: ApprovalAnswer::Accept, label: "Allow once", warning: Some("Looks destructive, so this always asks") },
    ApprovalOption { decision: ApprovalAnswer::Decline, label: "Decline", warning: None },
];
const SCREEN_OPTIONS: [ApprovalOption; 2] = [
    ApprovalOption { decision: ApprovalAnswer::AcceptForSession, label: "Show on screen for this task", warning: None },
    ApprovalOption { decision: ApprovalAnswer::Decline, label: "Keep it in the background", warning: None },
];

/// What an approval card shows. `detail` is the only free text, and never typed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApprovalPrompt {
    pub(crate) detail: String,
    pub(crate) options: &'static [ApprovalOption],
}

/// How asking the user ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AskOutcome {
    /// Another approval was already waiting, so this one was never shown.
    Paused,
    Answered(ApprovalAnswer),
}

// ── Pending approvals ───────────────────────────────────────────────────────

/// How a client's answer to a request id was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalResponse {
    /// A pending computer-use approval took the decision.
    Handled,
    /// A computer-use request id that is no longer pending (answered, cancelled, or from
    /// before a restart).
    Stale,
    /// Not a computer-use request; route it to the provider adapter.
    NotOwned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Owner {
    thread_id: String,
    turn_id: Option<String>,
}

/// Every approval waiting for the user, anywhere in the app: the gate's own cards (any
/// thread) and provider requests, tracked here before storage shows them. While one is
/// open no input runs, so nothing can click an approval card, wherever it is shown.
/// The service changes this only while it holds the dispatch lock, so no card can open
/// between the final check and the input landing.
#[derive(Debug, Default)]
pub(crate) struct PendingApprovals {
    own: HashMap<String, Owner>,
    provider: HashMap<(String, String), Owner>,
}

impl PendingApprovals {
    /// `stored` is whether storage shows another approval waiting in a live thread (user
    /// questions excluded); `None` when it could not be read, which counts as pending.
    pub(crate) fn any_pending(&self, stored: Option<bool>) -> bool {
        !self.own.is_empty() || !self.provider.is_empty() || stored.unwrap_or(true)
    }

    /// Opens one of the gate's own approvals and returns its request id, or `None` when
    /// another approval is already waiting: two cards are never open at once.
    pub(crate) fn open(&mut self, thread_id: &str, turn_id: &str, stored: Option<bool>) -> Option<String> {
        if self.any_pending(stored) {
            return None;
        }
        let request_id = format!("{REQUEST_ID_PREFIX}{}", uuid::Uuid::new_v4());
        self.own.insert(request_id.clone(), Owner { thread_id: thread_id.to_owned(), turn_id: Some(turn_id.to_owned()) });
        Some(request_id)
    }

    /// Closes one of the gate's own approvals; false when it was already closed, so a
    /// card is resolved exactly once.
    pub(crate) fn close(&mut self, request_id: &str) -> bool {
        self.own.remove(request_id).is_some()
    }

    /// Takes a client's answer. The second value is what the waiting action receives:
    /// the answer, a cancel when the turn it belonged to is no longer the running one,
    /// or nothing when the request was not pending here.
    pub(crate) fn respond(&mut self, thread_id: &str, request_id: &str, running_turn: Option<&str>, answer: ApprovalAnswer) -> (ApprovalResponse, Option<ApprovalAnswer>) {
        if !request_id.starts_with(REQUEST_ID_PREFIX) {
            return (ApprovalResponse::NotOwned, None);
        }
        let Some(owner) = self.own.get(request_id).filter(|owner| owner.thread_id == thread_id) else { return (ApprovalResponse::Stale, None) };
        let current = running_turn.is_some() && owner.turn_id.as_deref() == running_turn;
        self.own.remove(request_id);
        if current {
            (ApprovalResponse::Handled, Some(answer))
        } else {
            (ApprovalResponse::Stale, Some(ApprovalAnswer::Cancel))
        }
    }

    /// Records a provider's request before clients see it. Everything clients render as
    /// an approval card counts; a question for the user (`user_input`) does not.
    pub(crate) fn track_provider(&mut self, thread_id: &str, request_id: &str, turn_id: Option<&str>, pending: bool, user_input: bool) {
        if request_id.starts_with(REQUEST_ID_PREFIX) {
            return;
        }
        let key = (thread_id.to_owned(), request_id.to_owned());
        if pending && !user_input {
            self.provider.insert(key, Owner { thread_id: thread_id.to_owned(), turn_id: turn_id.map(str::to_owned) });
        } else {
            self.provider.remove(&key);
        }
    }

    fn cancel_own(&mut self, cancelled: impl Fn(&Owner) -> bool) -> Vec<String> {
        let mut ids: Vec<String> = self.own.iter().filter(|(_, owner)| cancelled(owner)).map(|(id, _)| id.clone()).collect();
        ids.sort();
        for id in &ids {
            self.own.remove(id);
        }
        ids
    }

    /// A turn ended or was interrupted (`turn_id` unknown): returns the gate's own
    /// approvals to cancel. A provider request still tracked for an ended turn will never
    /// be answered, so it stops pausing input.
    pub(crate) fn end_turn(&mut self, thread_id: &str, turn_id: Option<&str>) -> Vec<String> {
        self.provider.retain(|_, owner| !(owner.thread_id == thread_id && (turn_id.is_none() || owner.turn_id.is_none() || owner.turn_id.as_deref() == turn_id)));
        self.cancel_own(|owner| owner.thread_id == thread_id && (turn_id.is_none() || owner.turn_id.as_deref() == turn_id))
    }

    /// The thread's session stopped: returns the gate's own approvals to cancel.
    pub(crate) fn release_thread(&mut self, thread_id: &str) -> Vec<String> {
        self.provider.retain(|_, owner| owner.thread_id != thread_id);
        self.cancel_own(|owner| owner.thread_id == thread_id)
    }

    pub(crate) fn release_all(&mut self) -> Vec<String> {
        self.provider.clear();
        self.cancel_own(|_| true)
    }
}

// ── Per-thread state ────────────────────────────────────────────────────────

/// The thread's running turn as the app sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnContext {
    pub(crate) title: String,
    pub(crate) turn_id: String,
    /// Full access on the thread and on the run that is executing: a newly loosened
    /// thread setting must not upgrade a turn that started stricter.
    pub(crate) full_access: bool,
}

/// What the gate remembers for one thread: its targets, "allow for the rest of this
/// turn", and the answer to the show-on-screen question. In memory only; a restart
/// forgets it, and the turn-scoped parts end with the turn.
#[derive(Debug, Default)]
pub(crate) struct ThreadGate {
    pub(crate) targets: ThreadTargets,
    turn_grant: Option<String>,
    screen_choice: Option<(String, bool)>,
    last_running_turn: Option<String>,
    ended_turn: Option<String>,
}

impl ThreadGate {
    /// A gate whose numbers start at `first_id` instead of this run's random base.
    #[cfg(test)]
    pub(crate) fn starting_at(first_id: u64) -> Self {
        Self { targets: ThreadTargets::starting_at(first_id), ..Self::default() }
    }

    /// The running turn, unless it is one this gate was already told ended: turn-end
    /// notices stop input at once, without waiting for storage to catch up.
    pub(crate) fn running_turn(&mut self, turn: Option<TurnContext>) -> Option<TurnContext> {
        let turn = turn.filter(|turn| self.ended_turn.as_deref() != Some(turn.turn_id.as_str()))?;
        self.last_running_turn = Some(turn.turn_id.clone());
        Some(turn)
    }

    /// Whether routine input in this turn needs no approval under "follow thread permissions".
    pub(crate) fn granted(&self, turn: &TurnContext) -> bool {
        turn.full_access || self.turn_grant.as_deref() == Some(turn.turn_id.as_str())
    }

    /// Whether this turn may bring windows to the front: always under `Allow`; under
    /// `Ask` only once the user chose to show it on screen. `None` while the user has
    /// not been asked yet.
    pub(crate) fn screen_allowed(&self, screen: Screen, turn_id: &str) -> Option<bool> {
        if screen == Screen::Allow {
            return Some(true);
        }
        self.screen_choice.as_ref().filter(|(turn, _)| turn == turn_id).map(|(_, on_screen)| *on_screen)
    }

    /// Records the answer to the show-on-screen question; it holds for the rest of the
    /// turn. `Err` when the input may not take the screen.
    pub(crate) fn answer_screen(&mut self, turn_id: &str, outcome: AskOutcome) -> Result<(), ComputerUseError> {
        let on_screen = match outcome {
            AskOutcome::Paused => return Err(input_paused()),
            AskOutcome::Answered(ApprovalAnswer::Cancel) => return Err(not_dispatched(ErrorCode::Declined, "No answer arrived about showing this task on screen.")),
            AskOutcome::Answered(answer) => answer != ApprovalAnswer::Decline,
        };
        self.screen_choice = Some((turn_id.to_owned(), on_screen));
        if on_screen {
            Ok(())
        } else {
            Err(not_dispatched(ErrorCode::Declined, KEPT_IN_BACKGROUND))
        }
    }

    /// Records the answer to an input approval. "Allow for the rest of this turn" covers
    /// routine input until the turn ends, and never a destructive-looking action.
    pub(crate) fn answer_approval(&mut self, turn_id: &str, destructive: bool, outcome: AskOutcome) -> Result<(), ComputerUseError> {
        match outcome {
            AskOutcome::Paused => Err(input_paused()),
            AskOutcome::Answered(ApprovalAnswer::Decline | ApprovalAnswer::Cancel) => Err(not_dispatched(ErrorCode::Declined, "The user declined this action, or no answer arrived.")),
            AskOutcome::Answered(answer) => {
                if !destructive && matches!(answer, ApprovalAnswer::AcceptForSession | ApprovalAnswer::AcceptAlways) {
                    self.turn_grant = Some(turn_id.to_owned());
                }
                Ok(())
            }
        }
    }

    /// A turn ended or was interrupted: drop its grant and refuse its remaining input.
    /// Without an id (an interrupt), the turn is the one storage shows as running
    /// (`active`), or the last one seen here.
    pub(crate) fn end_turn(&mut self, turn_id: Option<&str>, active: Option<&str>) {
        if let Some(ended) = turn_id.or(active).map(str::to_owned).or_else(|| self.last_running_turn.clone()) {
            self.ended_turn = Some(ended);
        }
        if turn_id.is_none() || self.turn_grant.as_deref() == turn_id {
            self.turn_grant = None;
        }
    }

    /// The thread's session stopped: forget its windows, refs, shots and grant. The
    /// counters stay, so numbers are never reused.
    pub(crate) fn release(&mut self) {
        self.targets.reset();
        self.turn_grant = None;
    }
}

// ── Targets ─────────────────────────────────────────────────────────────────

/// The window an observe or screenshot names, unless it is unknown or protected.
pub(crate) fn observable_window(targets: &ThreadTargets, window_id: u64, own_pids: &HashSet<u32>) -> Result<WindowRecord, ComputerUseError> {
    let window = targets.window(window_id).ok_or_else(|| unknown_window(window_id, None))?;
    if is_denied(window.into(), own_pids) {
        return Err(denied_app(None));
    }
    Ok(window.clone())
}

/// What an input request acts on, resolved through the thread's tables.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ResolvedTarget {
    Element { window: WindowRecord, element: RefRecord },
    Window { window: WindowRecord },
    Shot { window: WindowRecord, shot: ShotRecord },
}

impl ResolvedTarget {
    pub(crate) fn window(&self) -> &WindowRecord {
        match self {
            Self::Element { window, .. } | Self::Window { window } | Self::Shot { window, .. } => window,
        }
    }

    /// Whether two resolutions name the same window, and the same ref or shot.
    fn same_as(&self, other: &Self) -> bool {
        let same_window = self.window().id == other.window().id && self.window().handle == other.window().handle;
        same_window
            && match (self, other) {
                (Self::Element { element: left, .. }, Self::Element { element: right, .. }) => left.reference == right.reference,
                (Self::Window { .. }, Self::Window { .. }) => true,
                (Self::Shot { shot: left, .. }, Self::Shot { shot: right, .. }) => left.shot == right.shot,
                _ => false,
            }
    }

    fn stale_code(&self) -> ErrorCode {
        if matches!(self, Self::Shot { .. }) {
            ErrorCode::StaleShot
        } else {
            ErrorCode::StaleRef
        }
    }
}

/// Resolves what an input request names, or says why it cannot be acted on: an unknown
/// number, a ref from an older observation, a shot that is not its window's newest, a
/// closed window, a protected app, or a point outside the image.
pub(crate) fn resolve_input_target(targets: &ThreadTargets, request: &InputRequest, own_pids: &HashSet<u32>) -> Result<ResolvedTarget, ComputerUseError> {
    let denied = |window: &WindowRecord| is_denied(window.into(), own_pids);
    match *request {
        InputRequest::Key { window, .. } | InputRequest::TypeFocused { window, .. } | InputRequest::Focus { window } => {
            let record = targets.window(window).ok_or_else(|| unknown_window(window, Some(Effect::NotDispatched)))?;
            if denied(record) {
                return Err(denied_app(Some(Effect::NotDispatched)));
            }
            Ok(ResolvedTarget::Window { window: record.clone() })
        }
        InputRequest::Click { shot, .. } | InputRequest::Drag { shot, .. } | InputRequest::Move { shot, .. } | InputRequest::ScrollAt { shot, .. } => match targets.lookup_shot(shot) {
            ShotLookup::Unknown => Err(not_dispatched(ErrorCode::UnknownShot, format!("Shot {shot} is not known to this thread. Take a screenshot first."))),
            ShotLookup::Retired(ShotRetired::WindowClosed) => Err(not_dispatched(ErrorCode::StaleShot, format!("Shot {shot} shows a window that closed. Run `{LAUNCHER_NAME} list-windows`."))),
            ShotLookup::Retired(ShotRetired::NewerShot) => Err(not_dispatched(ErrorCode::StaleShot, format!("Shot {shot} is not the newest screenshot of its window; use the newest shot (every input returns one) or take a new screenshot."))),
            ShotLookup::Found { shot: record, window } => {
                if denied(window) {
                    return Err(denied_app(Some(Effect::NotDispatched)));
                }
                if request.pixels().iter().any(|(x, y)| *x >= u64::from(record.width) || *y >= u64::from(record.height)) {
                    return Err(not_dispatched(ErrorCode::InvalidArgument, format!("A point is outside shot {shot}, which is {}×{} pixels.", record.width, record.height)));
                }
                Ok(ResolvedTarget::Shot { window: window.clone(), shot: record.clone() })
            }
        },
        InputRequest::Press { reference } | InputRequest::SetValue { reference, .. } | InputRequest::Type { reference, .. } | InputRequest::Scroll { reference, .. } => match targets.lookup_ref(reference) {
            RefLookup::Unknown => Err(not_dispatched(ErrorCode::UnknownRef, format!("Ref {reference} is not known to this thread. Observe the window to get refs."))),
            RefLookup::Retired(RefRetired::WindowClosed) => Err(not_dispatched(ErrorCode::StaleRef, format!("Ref {reference} belongs to a window that closed. Run `{LAUNCHER_NAME} list-windows`."))),
            RefLookup::Retired(RefRetired::NewerObservation) => Err(not_dispatched(ErrorCode::StaleRef, format!("Ref {reference} is from an older observation; observe again."))),
            RefLookup::Found { reference: element, window } => {
                if denied(window) {
                    return Err(denied_app(Some(Effect::NotDispatched)));
                }
                Ok(ResolvedTarget::Element { window: window.clone(), element: element.clone() })
            }
        },
    }
}

/// Pixel centre → logical screen point, through the screen rect the image covers.
pub(crate) fn to_screen_point(shot: &ShotRecord, x: u64, y: u64) -> Point {
    Point { x: shot.bounds.x + ((x as f64 + 0.5) * shot.bounds.width) / f64::from(shot.width), y: shot.bounds.y + ((y as f64 + 0.5) * shot.bounds.height) / f64::from(shot.height) }
}

/// The screen point whose control an approval should name: a coordinate command's first
/// point. `None` for everything else, which is [`Hit::NotApplicable`].
pub(crate) fn hit_point(request: &InputRequest, target: &ResolvedTarget) -> Option<Point> {
    let ResolvedTarget::Shot { shot, .. } = target else { return None };
    request.pixels().first().map(|(x, y)| to_screen_point(shot, *x, *y))
}

/// The helper request that carries out a resolved input. It names the helper's handles
/// and what this side last saw of the target (a control's role and label, the bounds a
/// shot covered), which the helper compares with the live ones at dispatch. `None` when
/// the request and target kinds do not belong together.
pub(crate) fn driver_request(request: &InputRequest, target: &ResolvedTarget) -> Option<DriverRequest> {
    Some(match (request, target) {
        (InputRequest::Key { keys, .. }, ResolvedTarget::Window { window }) => DriverRequest::Key { window: window.handle.clone(), keys: keys.clone() },
        (InputRequest::TypeFocused { text, .. }, ResolvedTarget::Window { window }) => DriverRequest::TypeFocused { window: window.handle.clone(), text: text.clone() },
        (InputRequest::Focus { .. }, ResolvedTarget::Window { window }) => DriverRequest::Focus { window: window.handle.clone() },
        (InputRequest::Click { x, y, button, count, .. }, ResolvedTarget::Shot { shot, .. }) => DriverRequest::Click { window: shot.handle.clone(), expect_bounds: shot.bounds, point: to_screen_point(shot, *x, *y), button: button.unwrap_or(MouseButton::Left), count: count.unwrap_or(1) },
        (InputRequest::Drag { from_x, from_y, to_x, to_y, .. }, ResolvedTarget::Shot { shot, .. }) => DriverRequest::Drag { window: shot.handle.clone(), expect_bounds: shot.bounds, from: to_screen_point(shot, *from_x, *from_y), to: to_screen_point(shot, *to_x, *to_y) },
        (InputRequest::Move { x, y, .. }, ResolvedTarget::Shot { shot, .. }) => DriverRequest::Move { window: shot.handle.clone(), expect_bounds: shot.bounds, point: to_screen_point(shot, *x, *y) },
        (InputRequest::ScrollAt { x, y, dx, dy, .. }, ResolvedTarget::Shot { shot, .. }) => DriverRequest::ScrollAt { window: shot.handle.clone(), expect_bounds: shot.bounds, point: to_screen_point(shot, *x, *y), dx: *dx, dy: *dy },
        (InputRequest::Press { .. }, ResolvedTarget::Element { element, .. }) => DriverRequest::Press { element: element.handle.clone(), expect: element.identity() },
        (InputRequest::SetValue { value, .. }, ResolvedTarget::Element { element, .. }) => DriverRequest::SetValue { element: element.handle.clone(), expect: element.identity(), value: value.clone() },
        (InputRequest::Type { text, .. }, ResolvedTarget::Element { element, .. }) => DriverRequest::TypeText { element: element.handle.clone(), expect: element.identity(), text: text.clone() },
        (InputRequest::Scroll { dx, dy, .. }, ResolvedTarget::Element { element, .. }) => DriverRequest::Scroll { element: element.handle.clone(), expect: element.identity(), dx: *dx, dy: *dy },
        _ => return None,
    })
}

// ── Decisions ───────────────────────────────────────────────────────────────

/// Everything a decision reads, read fresh by the caller for that decision and never
/// cached across one: the settings, the running turn, and whether any approval waits.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Live<'a> {
    pub(crate) mode: Mode,
    pub(crate) approvals: Approvals,
    pub(crate) screen: Screen,
    /// The thread's running turn, already through [`ThreadGate::running_turn`].
    pub(crate) turn: Option<&'a TurnContext>,
    /// [`PendingApprovals::any_pending`].
    pub(crate) approval_pending: bool,
    /// The app's own process ancestry.
    pub(crate) own_pids: &'a HashSet<u32>,
    /// The helper delivers synthetic input to windows that are not in front (opt-in).
    pub(crate) background_input: bool,
}

/// Whether the show-on-screen question comes before anything else.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ScreenStep {
    Proceed,
    /// Ask, then record the answer with [`ThreadGate::answer_screen`].
    Ask(ApprovalPrompt),
}

/// An input request as it arrives: a turn must be running, no approval may be waiting,
/// the target must resolve, and a task kept in the background may not take the screen.
pub(crate) fn begin_input(live: &Live<'_>, gate: &ThreadGate, request: &InputRequest) -> Result<(ResolvedTarget, ScreenStep), ComputerUseError> {
    let Some(turn) = live.turn else {
        return Err(not_dispatched(ErrorCode::NoTurn, "No turn is running in this thread; computer actions only happen while you are working on the user's request."));
    };
    if live.approval_pending {
        return Err(input_paused());
    }
    let resolved = resolve_input_target(&gate.targets, request, live.own_pids)?;
    // Everything but press, set-value and type by ref brings the window to the front;
    // those only do when their fallback needs it (asked at the helper's prepare check).
    let background_candidate = live.background_input && !resolved.window().focused && matches!(request, InputRequest::Click { .. } | InputRequest::Key { .. } | InputRequest::Scroll { .. } | InputRequest::ScrollAt { .. } | InputRequest::Drag { .. } | InputRequest::Move { .. } | InputRequest::TypeFocused { .. });
    let takes_screen = !matches!(resolved, ResolvedTarget::Element { .. }) || matches!(request, InputRequest::Scroll { .. });
    if background_candidate || !takes_screen {
        return Ok((resolved, ScreenStep::Proceed));
    }
    match gate.screen_allowed(live.screen, &turn.turn_id) {
        Some(true) => Ok((resolved, ScreenStep::Proceed)),
        Some(false) => Err(not_dispatched(ErrorCode::Declined, KEPT_IN_BACKGROUND)),
        None => {
            let detail = format!("Show this on your screen? To work in {}, the agent needs to bring its windows to the front and use the mouse and keyboard until this response ends. In the background it can only act on controls directly.", resolved.window().app);
            Ok((resolved, ScreenStep::Ask(ApprovalPrompt { detail, options: &SCREEN_OPTIONS })))
        }
    }
}

/// Whether an input goes ahead to the final check, or asks the user first.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum InputDecision {
    Allow { destructive: bool },
    /// Ask, then record the answer with [`ThreadGate::answer_approval`].
    Ask { prompt: ApprovalPrompt, destructive: bool },
}

impl InputDecision {
    pub(crate) fn asked(&self) -> bool {
        matches!(self, Self::Ask { .. })
    }

    pub(crate) fn destructive(&self) -> bool {
        match self {
            Self::Allow { destructive } | Self::Ask { destructive, .. } => *destructive,
        }
    }
}

/// Decides whether a resolved input asks. `approvals` is the setting as read with the
/// turn, before the hit test; the final check reads it again. Destructive-looking means
/// the ref's control, a quit or close chord, or what a click lands on: a move or scroll
/// over a "Delete" button is not asked about.
pub(crate) fn decide_input(approvals: Approvals, gate: &ThreadGate, turn: &TurnContext, request: &InputRequest, resolved: &ResolvedTarget, hit: &Hit) -> InputDecision {
    let destructive = match (resolved, request, hit) {
        (ResolvedTarget::Element { element, .. }, _, _) => is_destructive_target(&element.role, &element.label),
        (_, InputRequest::Key { keys, .. }, _) => is_destructive_chord(keys),
        (_, InputRequest::Click { .. }, Hit::Element(hit)) => is_destructive_target(&hit.role, &hit.label),
        _ => false,
    };
    if !input_needs_approval(approvals, destructive, gate.granted(turn)) {
        return InputDecision::Allow { destructive };
    }
    let element = match resolved {
        ResolvedTarget::Element { element, .. } => Some(element.identity()),
        _ => None,
    };
    let detail = describe_input_for_approval(request, &ApprovalTarget { app: &resolved.window().app, window_title: &resolved.window().title, element, hit: hit.clone() });
    InputDecision::Ask { prompt: ApprovalPrompt { detail, options: if destructive { &DESTRUCTIVE_OPTIONS } else { &ROUTINE_OPTIONS } }, destructive }
}

/// An input the gate went on with: what was resolved and decided up front, to be
/// confirmed at each later check.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CheckedInput<'a> {
    pub(crate) request: &'a InputRequest,
    /// The turn the request arrived in.
    pub(crate) turn_id: &'a str,
    pub(crate) resolved: &'a ResolvedTarget,
    /// The user approved this very action.
    pub(crate) asked: bool,
    pub(crate) destructive: bool,
}

fn clip_value(value: &str) -> String {
    if value.chars().count() <= VALUE_MAX_CHARS {
        value.to_owned()
    } else {
        format!("{}…", value.chars().take(VALUE_MAX_CHARS - 1).collect::<String>())
    }
}

/// The last gate before the desktop, on every input path (asked or auto-allowed): the
/// setting, the same running turn, no approval open, and the target still current. Run
/// once without `native` to fail fast, then for the exact native target the helper
/// reports: before it prepares anything and again immediately before input, that last
/// time under the dispatch lock. `Ok` allows; an `Err` is the helper's refusal.
pub(crate) fn final_check(live: &Live<'_>, gate: &ThreadGate, input: &CheckedInput<'_>, native: Option<&DispatchTarget>) -> Result<(), ComputerUseError> {
    if live.mode != Mode::Control {
        return Err(not_dispatched(ErrorCode::ObserveOnly, "Computer use no longer allows input."));
    }
    let Some(turn) = live.turn.filter(|turn| turn.turn_id == input.turn_id) else {
        return Err(not_dispatched(if input.asked { ErrorCode::Declined } else { ErrorCode::NoTurn }, "The turn ended before the action ran."));
    };
    if live.approval_pending {
        return Err(input_paused());
    }
    // Fresh, so switching to a stricter setting (or leaving Full access) mid-action never
    // lets an input through that would now ask.
    if !input.asked && input_needs_approval(live.approvals, input.destructive, gate.granted(turn)) {
        return Err(not_dispatched(ErrorCode::Declined, "The thread now requires approval; request the action again."));
    }
    let fresh = resolve_input_target(&gate.targets, input.request, live.own_pids)?;
    if !input.resolved.same_as(&fresh) {
        return Err(not_dispatched(fresh.stale_code(), "The target changed since this action was checked; look again."));
    }
    let Some(native) = native else { return Ok(()) };
    if native.foreground && gate.screen_allowed(live.screen, &turn.turn_id) != Some(true) {
        return Err(not_dispatched(ErrorCode::Declined, "This control can only be operated with its window in front, which this task may not do. Act on other controls by reference, or ask the user."));
    }
    if native.window.handle != fresh.window().handle {
        return Err(not_dispatched(ErrorCode::Declined, "The native target changed before dispatch."));
    }
    if is_denied((&native.window).into(), live.own_pids) {
        return Err(denied_app(Some(Effect::NotDispatched)));
    }
    // Only element targets and clicks were checked for destructiveness up front. The
    // label is compared as the agent and the up-front check saw it: clipped.
    let destructive_applies = matches!(fresh, ResolvedTarget::Element { .. }) || matches!(input.request, InputRequest::Click { .. });
    if live.approvals != Approvals::Never && !input.destructive && destructive_applies && native.element.as_ref().is_some_and(|element| is_destructive_target(&element.role, &clip_value(&element.label))) {
        return Err(not_dispatched(fresh.stale_code(), "The target now looks destructive; look again and request approval."));
    }
    Ok(())
}

// ── Helper failures ─────────────────────────────────────────────────────────

fn effect_of(dispatched: Dispatched) -> Effect {
    match dispatched {
        Dispatched::No => Effect::NotDispatched,
        Dispatched::Unknown => Effect::DispatchedUnknown,
        Dispatched::Yes => Effect::Dispatched,
    }
}

/// Maps a helper failure; input commands carry what the OS may have received.
pub(crate) fn driver_error_to_computer_use_error(failure: &DriverFailure, input: bool) -> ComputerUseError {
    let effect = input.then(|| effect_of(failure.dispatched));
    let (code, message) = match failure.kind {
        FailureKind::Policy => (failure.code.unwrap_or(ErrorCode::Declined), failure.message.clone()),
        FailureKind::Unavailable => (ErrorCode::DriverUnavailable, failure.message.clone()),
        FailureKind::PermissionAccessibility => (ErrorCode::AccessibilityMissing, failure.message.clone()),
        FailureKind::PermissionScreen => (ErrorCode::ScreenRecordingMissing, failure.message.clone()),
        FailureKind::Stale => (ErrorCode::StaleRef, "The element changed since it was observed; observe the window again.".to_owned()),
        FailureKind::Malformed => (ErrorCode::DriverMalformed, failure.message.clone()),
        FailureKind::Failed | FailureKind::Timeout if effect == Some(Effect::DispatchedUnknown) => (ErrorCode::DriverFailed, format!("{} The input may have been delivered; observe before retrying.", failure.message)),
        FailureKind::Failed | FailureKind::Timeout => (ErrorCode::DriverFailed, failure.message.clone()),
    };
    ComputerUseError { code, message, effect }
}

/// A stale window handle does not prove that the app window closed. Retire it and ask
/// for rediscovery; a helper restart gets its own explanation.
pub(crate) fn window_gone(targets: &mut ThreadTargets, window: &WindowRecord, failure: &DriverFailure, effect: Option<Effect>) -> ComputerUseError {
    targets.close_window(window.id);
    let message = if failure.reason == Some(FailureReason::Restarted) {
        format!("The computer-use driver restarted; run `{LAUNCHER_NAME} list-windows` again.")
    } else {
        format!("The driver can no longer identify window {}. Run `{LAUNCHER_NAME} list-windows`.", window.id)
    };
    ComputerUseError { code: ErrorCode::UnknownWindow, message, effect }
}

/// How a failed observe or screenshot is reported: a stale handle retires the window.
pub(crate) fn observation_failure(targets: &mut ThreadTargets, window: &WindowRecord, failure: &DriverFailure) -> ComputerUseError {
    if failure.kind == FailureKind::Stale {
        window_gone(targets, window, failure, None)
    } else {
        driver_error_to_computer_use_error(failure, false)
    }
}

/// How a failed input is reported, and what it retires: after a helper restart every
/// handle of the old one is gone; a stale coordinate target means the window moved, so
/// its shot stops accepting coordinates.
pub(crate) fn input_failure(targets: &mut ThreadTargets, target: &ResolvedTarget, failure: &DriverFailure) -> ComputerUseError {
    let effect = Some(effect_of(failure.dispatched));
    if failure.kind != FailureKind::Stale {
        return driver_error_to_computer_use_error(failure, true);
    }
    let restarted = failure.reason == Some(FailureReason::Restarted);
    match target {
        ResolvedTarget::Element { window, .. } if restarted => {
            targets.close_window(window.id);
            ComputerUseError { code: ErrorCode::StaleRef, message: format!("The computer-use driver restarted; run `{LAUNCHER_NAME} list-windows`, then observe again."), effect }
        }
        ResolvedTarget::Shot { shot, .. } if !restarted => {
            targets.retire_shot(shot.shot);
            ComputerUseError { code: ErrorCode::StaleShot, message: format!("The window moved, resized or closed since shot {}; take a new screenshot.", shot.shot), effect }
        }
        ResolvedTarget::Element { .. } => driver_error_to_computer_use_error(failure, true),
        ResolvedTarget::Window { window } | ResolvedTarget::Shot { window, .. } => window_gone(targets, window, failure, effect),
    }
}
