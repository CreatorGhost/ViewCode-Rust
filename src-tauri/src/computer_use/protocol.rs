//! The wire between the gate and the native helper (`native/computer-driver`): one JSON
//! object per line on the helper's stdin and stdout. These types only say what a line is;
//! nothing here starts a process.
//!
//! A request is `{id, nonce, request}`. The helper answers `{id, ok, result | error}`.
//! In between, for input, it asks `{id, authorize, phase}` about the target it just read
//! and waits for `{id, nonce, decision}`: `null` allows, an error object refuses. It asks
//! twice, `prepare` before it activates, raises or focuses anything and `dispatch`
//! immediately before native input. An allowed `dispatch` holds the gate's dispatch lock
//! until the helper's next line, so the helper writes nothing between that decision and
//! the input returning. A lost, mismatched or unreadable line is never permission.

use super::policy::{ComputerUseError, ErrorCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Env var carrying the per-spawn nonce the helper requires on every request.
pub(crate) const DRIVER_NONCE_ENV: &str = "VIEWCODE_COMPUTER_DRIVER_NONCE";
/// Env var carrying the epoch chosen for this helper; part of every handle it mints.
pub(crate) const DRIVER_EPOCH_ENV: &str = "VIEWCODE_COMPUTER_DRIVER_EPOCH";
/// How long the helper may take to bring a window forward before it refuses the input.
const ACTIVATION_TIMEOUT_MS: u64 = 3_000;
/// Input may resolve its element, activate the window, name a coordinate target (a walk
/// of up to 3 s) and, for an accessibility action the app does not support, activate
/// again for the fallback, all before the input itself.
const INPUT_TIMEOUT_MS: u64 = 2 * ACTIVATION_TIMEOUT_MS + 14_000;
/// Typing long text adds its own time on top.
const TYPING_TIMEOUT_MS: u64 = INPUT_TIMEOUT_MS + 10_000;
/// Requests one helper serves before it is replaced, so it cannot grow without bound.
pub(crate) const DEFAULT_RECYCLE_AFTER: u32 = 500;

/// A rectangle in logical screen coordinates, origin at the top left.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

/// A point in logical screen coordinates.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub(crate) struct Point {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

/// What the gate last saw of a control. The helper re-reads the element at dispatch and
/// refuses with `stale` when role or label no longer match, so an action cannot land on a
/// control that replaced the one the agent saw.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct ElementIdentity {
    pub(crate) role: String,
    pub(crate) label: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MouseButton {
    Left,
    Right,
    Middle,
}

/// One request to the helper. Handles (`window`, `element`) are the opaque strings the
/// same helper process minted; coordinate requests carry the window bounds the screenshot
/// was taken at, which the helper compares with the live ones. `Debug` names the op only:
/// a request carries what the agent types, which must never reach a log.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub(crate) enum DriverRequest {
    Status,
    ListWindows,
    /// Sent first to a replacement helper when the previous one died mid-drag.
    ReleaseMouse,
    Observe {
        window: String,
        max_elements: u32,
        /// Keeps only controls whose label or value contains this (case-insensitive).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query: Option<String>,
    },
    Screenshot { window: String, output_path: String, max_size: u32 },
    ElementAt { window: String, point: Point },
    Press { element: String, expect: ElementIdentity },
    SetValue { element: String, expect: ElementIdentity, value: String },
    TypeText { element: String, expect: ElementIdentity, text: String },
    Scroll { element: String, expect: ElementIdentity, dx: i32, dy: i32 },
    Key { window: String, keys: String },
    TypeFocused { window: String, text: String },
    Focus { window: String },
    Click { window: String, expect_bounds: Rect, point: Point, button: MouseButton, count: u32 },
    Drag { window: String, expect_bounds: Rect, from: Point, to: Point },
    Move { window: String, expect_bounds: Rect, point: Point },
    ScrollAt { window: String, expect_bounds: Rect, point: Point, dx: i32, dy: i32 },
}

impl std::fmt::Debug for DriverRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DriverRequest({})", self.op())
    }
}

impl DriverRequest {
    /// The `op` name, the only part of a request that may be logged.
    pub(crate) fn op(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::ListWindows => "listWindows",
            Self::ReleaseMouse => "releaseMouse",
            Self::Observe { .. } => "observe",
            Self::Screenshot { .. } => "screenshot",
            Self::ElementAt { .. } => "elementAt",
            Self::Press { .. } => "press",
            Self::SetValue { .. } => "setValue",
            Self::TypeText { .. } => "typeText",
            Self::Scroll { .. } => "scroll",
            Self::Key { .. } => "key",
            Self::TypeFocused { .. } => "typeFocused",
            Self::Focus { .. } => "focus",
            Self::Click { .. } => "click",
            Self::Drag { .. } => "drag",
            Self::Move { .. } => "move",
            Self::ScrollAt { .. } => "scrollAt",
        }
    }

    /// Whether the request sends input, so a failure must say what the OS may have received.
    pub(crate) fn is_input(&self) -> bool {
        !matches!(self, Self::Status | Self::ListWindows | Self::ReleaseMouse | Self::Observe { .. } | Self::Screenshot { .. } | Self::ElementAt { .. })
    }

    /// Whether the request presses a mouse button, which a helper dying mid-call could leave held.
    pub(crate) fn holds_mouse_button(&self) -> bool {
        matches!(self, Self::Click { .. } | Self::Drag { .. })
    }

    /// How long the helper gets before it is killed and the call reported as timed out.
    pub(crate) fn timeout_ms(&self) -> u64 {
        match self {
            Self::Status | Self::ListWindows => 10_000,
            Self::ReleaseMouse => 5_000,
            Self::Observe { .. } => 15_000,
            Self::Screenshot { .. } => 30_000,
            Self::ElementAt { .. } => 8_000,
            // An ignored accessibility change falls back to typing the value.
            Self::SetValue { .. } | Self::TypeText { .. } | Self::TypeFocused { .. } => TYPING_TIMEOUT_MS,
            _ => INPUT_TIMEOUT_MS,
        }
    }
}

/// A request as written to the helper's stdin.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct RequestLine {
    pub(crate) id: u64,
    pub(crate) nonce: String,
    pub(crate) request: DriverRequest,
}

impl RequestLine {
    pub(crate) fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// A window as the helper lists it, and as it describes the target of an authorize check.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DriverWindow {
    pub(crate) handle: String,
    pub(crate) app: String,
    pub(crate) pid: u32,
    pub(crate) title: String,
    pub(crate) focused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) bounds: Option<Rect>,
    /// The executable path when the platform reports one (never a bundle id); the
    /// protected-apps rule matches `.app` bundles on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) app_identifier: Option<String>,
    /// An open menu or an app's menu bar: refs and screenshots only, never focus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<WindowKind>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WindowKind {
    Menu,
    MenuBar,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct DriverElement {
    pub(crate) handle: String,
    pub(crate) role: String,
    pub(crate) label: String,
    /// Already clipped by the helper; absent for secure text fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<String>,
    /// The nearest heading before the control in reading order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) section: Option<String>,
    /// "2 of 8" when other controls share its role and label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) instance: Option<String>,
    pub(crate) enabled: bool,
    pub(crate) focused: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Accessibility {
    Granted,
    Denied,
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct DriverStatus {
    pub(crate) available: bool,
    pub(crate) accessibility: Accessibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

impl DriverStatus {
    /// Status never fails: a helper that cannot answer is reported as unavailable, with why.
    pub(crate) fn unavailable(reason: impl Into<String>) -> Self {
        Self { available: false, accessibility: Accessibility::Unknown, reason: Some(reason.into()) }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observation {
    pub(crate) elements: Vec<DriverElement>,
    /// True when the element cap cut the list.
    pub(crate) truncated: bool,
}

/// A written screenshot. `bounds` is the screen region the image covers, which maps image
/// pixels back to the screen.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub(crate) struct Screenshot {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bounds: Rect,
}

impl<'de> Deserialize<'de> for Screenshot {
    /// An image without pixels is not a result: a zero size would divide by zero when a
    /// click is mapped through it.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            width: u32,
            height: u32,
            bounds: Rect,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.width == 0 || wire.height == 0 {
            return Err(serde::de::Error::custom("empty screenshot"));
        }
        Ok(Self { width: wire.width, height: wire.height, bounds: wire.bounds })
    }
}

/// What an input call did besides the input itself.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InputResult {
    /// Whether the helper brought the target app to the front, taking focus from the
    /// user. False for accessibility actions that ran in the background.
    pub(crate) took_focus: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FailureKind {
    Policy,
    /// Platform unsupported, helper missing, or it would not start.
    Unavailable,
    /// Accessibility not granted to the process the helper runs as.
    PermissionAccessibility,
    /// Screen Recording not granted (screenshots).
    PermissionScreen,
    /// Handle unknown to this helper process, or identity no longer matches.
    Stale,
    /// The OS refused or the action is unsupported on the element.
    Failed,
    Timeout,
    /// The helper answered with something that is not a valid reply.
    Malformed,
}

/// Whether the OS may have received the input. `Unknown` when the helper died or timed
/// out after an allowed dispatch: such an action must never be replayed blindly.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Dispatched {
    No,
    Yes,
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FailureReason {
    /// The handle came from a helper that has since restarted, so the target may well
    /// still exist; list or observe again.
    Restarted,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct DriverFailure {
    pub(crate) kind: FailureKind,
    /// Safe to show: never contains typed text, values or window contents.
    pub(crate) message: String,
    pub(crate) dispatched: Dispatched,
    /// A policy code copied from the gate's own refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) code: Option<ErrorCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<FailureReason>,
}

impl DriverFailure {
    pub(crate) fn new(kind: FailureKind, message: impl Into<String>, dispatched: Dispatched) -> Self {
        Self { kind, message: message.into(), dispatched, code: None, reason: None }
    }
}

/// How a call ended when no usable reply arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallOutcome {
    /// The request never reached a helper, or its caller gave up while it was queued.
    NotSent,
    Exited,
    Timeout,
    /// A line arrived that is not a valid reply to this request.
    Malformed,
}

/// What a broken call may have delivered: only input an allowed `dispatch` decision went
/// out for can have run; before one, nothing was sent.
pub(crate) fn dispatched_after(input: bool, authorized: bool) -> Dispatched {
    if input && authorized {
        Dispatched::Unknown
    } else {
        Dispatched::No
    }
}

impl CallOutcome {
    pub(crate) fn failure(self, input: bool, authorized: bool) -> DriverFailure {
        let after_send = dispatched_after(input, authorized);
        match self {
            Self::NotSent => DriverFailure::new(FailureKind::Unavailable, "The computer-use driver could not start.", Dispatched::No),
            Self::Timeout => DriverFailure::new(FailureKind::Timeout, "The computer-use driver timed out.", after_send),
            Self::Exited => DriverFailure::new(FailureKind::Failed, "The computer-use driver exited.", after_send),
            Self::Malformed => DriverFailure::new(FailureKind::Malformed, "The computer-use driver answered with an invalid reply.", after_send),
        }
    }
}

/// The helper drives macOS only so far; elsewhere no process is started.
pub(crate) fn platform_supported(os: &str) -> bool {
    os == "macos"
}

/// Why computer use cannot run on `os` (a `std::env::consts::OS` value).
pub(crate) fn unsupported_reason(os: &str) -> &'static str {
    match os {
        "windows" => "Computer use is not supported on Windows yet.",
        "linux" => "Computer use is not supported on Linux yet.",
        _ => "Computer use is not supported on this platform.",
    }
}

/// When the helper asks: `Prepare` before it activates, raises or focuses anything,
/// `Dispatch` immediately before native input.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Phase {
    Prepare,
    Dispatch,
}

/// The freshly read native target an authorize check is about.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct DispatchTarget {
    pub(crate) window: DriverWindow,
    /// The control acted on, or what a coordinate hits (named on the `dispatch` check).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) element: Option<ElementIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) point: Option<Point>,
    /// Set when the helper is about to bring the window to the front for this input,
    /// including a fallback for a background action.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) foreground: bool,
}

/// The gate's answer to an authorize check. `decision` is always written: `null` allows,
/// and the helper treats a line without the key as no permission.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecisionLine {
    pub(crate) id: u64,
    pub(crate) nonce: String,
    pub(crate) decision: Option<ComputerUseError>,
}

impl DecisionLine {
    pub(crate) fn new(id: u64, nonce: &str, decision: Result<(), ComputerUseError>) -> Self {
        Self { id, nonce: nonce.to_owned(), decision: decision.err() }
    }

    pub(crate) fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// One line read from the helper while request `id` is in flight.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DriverLine {
    /// The helper asks about a target before going on.
    Authorize { target: DispatchTarget, phase: Phase },
    /// The final answer; `result` is decoded by the caller for the op it sent.
    Reply(Result<Value, DriverFailure>),
    /// Not JSON, for another request, or the wrong shape: the stream is out of step, so
    /// the caller answers malformed and starts a fresh helper rather than misattribute it.
    Malformed,
}

/// Reads one helper line as part of request `id`.
pub(crate) fn read_line(line: &str, id: u64) -> DriverLine {
    let Ok(Value::Object(mut message)) = serde_json::from_str::<Value>(line) else { return DriverLine::Malformed };
    if message.get("id").and_then(Value::as_u64) != Some(id) {
        return DriverLine::Malformed;
    }
    if let Some(target) = message.remove("authorize") {
        // Anything but an explicit `prepare` is treated as the stricter final check.
        let phase = if message.get("phase").and_then(Value::as_str) == Some("prepare") { Phase::Prepare } else { Phase::Dispatch };
        return match serde_json::from_value(target) {
            Ok(target) => DriverLine::Authorize { target, phase },
            Err(_) => DriverLine::Malformed,
        };
    }
    match message.get("ok").and_then(Value::as_bool) {
        Some(true) => match message.remove("result") {
            Some(result) => DriverLine::Reply(Ok(result)),
            None => DriverLine::Malformed,
        },
        Some(false) => match message.remove("error").map(serde_json::from_value::<DriverFailure>) {
            Some(Ok(failure)) => DriverLine::Reply(Err(failure)),
            _ => DriverLine::Malformed,
        },
        None => DriverLine::Malformed,
    }
}

/// Decodes a successful reply's `result` for the op that was sent. Anything else is
/// `malformed`, never a partial success.
pub(crate) fn decode_result<T: DeserializeOwned>(result: Value, input: bool, authorized: bool) -> Result<T, DriverFailure> {
    serde_json::from_value(result).map_err(|_| CallOutcome::Malformed.failure(input, authorized))
}
