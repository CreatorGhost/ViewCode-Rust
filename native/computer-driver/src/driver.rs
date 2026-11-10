//! The request loop's state and rules: handles, the wire shapes, and the order
//! of checks around every input. The wire protocol is the one the server's
//! worker client (`apps/server/src/computerUse/ComputerDriverClient.ts`)
//! speaks, as JSON lines on stdio.
//!
//! The server is the only gate. Before the driver brings anything to the front
//! it asks the server a `prepare` check, and immediately before native input a
//! `dispatch` check, each for the freshly resolved target. An allowed
//! `dispatch` holds the server's dispatch lock until this process sends its
//! next line, so nothing is written between that decision and the input
//! returning.
//!
//! Everything native sits behind [`Desktop`], so these rules are tested
//! without a desktop.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::Instant;

use agent_desktop_core::{
    AccessibilityNode, KeyCombo, Modifier, MouseButton, Rect, RefCapabilities, RefEntry,
    RefEntryIdentity, RefGeometry, RefProcess, RefScope, RefSource, WindowInfo,
    parse_png_dimensions,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::observe;

/// How long activation may take before input is refused.
const ACTIVATION_TIMEOUT_MS: u64 = 3_000;
const ACTIVATION_POLL_MS: u64 = 50;
/// Input that takes the screen waits while the user touched the mouse or
/// keyboard this recently.
const USER_ACTIVE_SECONDS: f64 = 1.5;
/// Margin for the driver's own events showing up as the latest input.
const OWN_INPUT_SLACK_SECONDS: f64 = 0.1;
/// Bounds the element table across many windows; oldest handles go first.
const MAX_ELEMENT_HANDLES: usize = 20_000;
/// Window bounds are whole points; this only absorbs float noise.
const BOUNDS_TOLERANCE: f64 = 0.5;
const OBSERVE_TIMEOUT_MS: u64 = 12_000;
/// Naming what a coordinate hits must stay quick.
const ELEMENT_AT_TIMEOUT_MS: u64 = 3_000;

/// A point in logical screen coordinates, origin at the top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// The native side, so the rules can be tested without a desktop.
pub trait Desktop {
    fn status(&self) -> Status;
    fn list_windows(&self) -> Result<Vec<ListedWindow>, Failure>;
    /// The window's accessibility tree, with bounds and native identity on each node.
    fn tree(&self, window: &WindowInfo, timeout_ms: u64) -> Result<AccessibilityNode, Failure>;
    /// A fresh read of one window: live bounds and state, or `stale` when it is gone.
    fn refresh(&self, window: &WindowInfo) -> Result<WindowInfo, Failure>;
    /// Whether this exact window is the one receiving keyboard input.
    fn is_front(&self, window: &WindowInfo) -> bool;
    fn focus_window(&self, window: &WindowInfo) -> Result<(), Failure>;
    /// Seconds since the last real mouse or keyboard event, when the OS can tell.
    fn idle_seconds(&self) -> Option<f64>;
    fn sleep(&self, milliseconds: u64);
    /// PNG bytes of the window alone, without its shadow.
    fn screenshot(&self, window: &WindowInfo) -> Result<Vec<u8>, Failure>;
    /// Writes `source` to `output` with its longest edge at most `max_size` pixels.
    fn shrink_png(&self, source: &Path, output: &Path, max_size: u32) -> Result<(), Failure>;
    /// Resolves the exact observed element and reads it live.
    fn live(&self, entry: &RefEntry) -> Result<Live, Failure>;
    /// Accessibility press, in the background.
    fn press(&self, entry: &RefEntry) -> Result<(), Failure>;
    /// Accessibility value change, in the background.
    fn set_value(&self, entry: &RefEntry, value: &str) -> Result<(), Failure>;
    /// Accessibility text insertion, in the background.
    fn insert_text(&self, entry: &RefEntry, text: &str) -> Result<(), Failure>;
    /// Focuses the element in its front window and types with key events.
    fn type_into(&self, entry: &RefEntry, text: &str, replace: bool) -> Result<(), Failure>;
    /// Types into whatever has keyboard focus in the front window.
    fn type_focused(&self, window: &WindowInfo, text: &str) -> Result<(), Failure>;
    fn key(&self, window: &WindowInfo, combo: &KeyCombo) -> Result<(), Failure>;
    fn click(&self, point: Point, button: MouseButton, count: u32) -> Result<(), Failure>;
    fn drag(&self, from: Point, to: Point) -> Result<(), Failure>;
    fn move_to(&self, point: Point) -> Result<(), Failure>;
    /// `dx` positive scrolls right, `dy` positive scrolls the content down.
    fn wheel(&self, point: Point, dx: i32, dy: i32) -> Result<(), Failure>;
}

pub struct Status {
    pub available: bool,
    /// "granted", "denied" or "unknown".
    pub accessibility: &'static str,
    pub reason: Option<String>,
}

pub struct ListedWindow {
    pub info: WindowInfo,
    /// The executable path; the server's denylist matches `.app` bundles on it.
    pub app_identifier: Option<String>,
}

pub struct Live {
    pub role: String,
    /// The live label, when the OS answered with one.
    pub label: Option<String>,
    pub bounds: Option<Rect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dispatched {
    No,
    /// The OS may have received the input.
    Unknown,
}

#[derive(Debug)]
pub struct Failure {
    /// One of the server's `DriverErrorKind`s.
    pub kind: &'static str,
    /// Safe to show: never window contents.
    pub message: String,
    pub dispatched: Dispatched,
    /// A policy code copied from the server's decision.
    pub code: Option<String>,
    pub restarted: bool,
    /// The element has no such accessibility action, so a synthetic fallback may apply.
    pub unsupported: bool,
}

impl Failure {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            dispatched: Dispatched::No,
            code: None,
            restarted: false,
            unsupported: false,
        }
    }

    fn stale(message: &str) -> Self {
        Self::new("stale", message)
    }

    fn failed(message: &str) -> Self {
        Self::new("failed", message)
    }

    /// A refusal met after the server allowed the input: it may have landed.
    fn after_dispatch(mut self) -> Self {
        self.dispatched = Dispatched::Unknown;
        self
    }
}

/// The parent's side of the stream, for the dispatch checks in the middle of a request.
pub trait Channel {
    /// Sends one line and returns the parent's next line, or `None` when it is gone.
    fn ask(&mut self, message: &Value) -> Option<String>;
}

#[derive(Deserialize)]
struct Expect {
    role: String,
    label: String,
}

#[derive(Deserialize, Clone, Copy)]
struct WirePoint {
    x: f64,
    y: f64,
}

#[derive(Deserialize, Clone, Copy)]
struct WireRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum Request {
    Status,
    ListWindows,
    ReleaseMouse,
    Observe {
        window: String,
        max_elements: usize,
        query: Option<String>,
    },
    Screenshot {
        window: String,
        output_path: String,
        max_size: u32,
    },
    ElementAt {
        window: String,
        point: WirePoint,
    },
    Press {
        element: String,
        expect: Expect,
    },
    SetValue {
        element: String,
        expect: Expect,
        value: String,
    },
    TypeText {
        element: String,
        expect: Expect,
        text: String,
    },
    Scroll {
        element: String,
        expect: Expect,
        dx: i32,
        dy: i32,
    },
    Key {
        window: String,
        keys: String,
    },
    TypeFocused {
        window: String,
        text: String,
    },
    Focus {
        window: String,
    },
    Click {
        window: String,
        expect_bounds: WireRect,
        point: WirePoint,
        button: String,
        count: u32,
    },
    Drag {
        window: String,
        expect_bounds: WireRect,
        from: WirePoint,
        to: WirePoint,
    },
    Move {
        window: String,
        expect_bounds: WireRect,
        point: WirePoint,
    },
    ScrollAt {
        window: String,
        expect_bounds: WireRect,
        point: WirePoint,
        dx: i32,
        dy: i32,
    },
}

#[derive(Deserialize)]
struct Envelope {
    id: u64,
    nonce: String,
    request: Option<Value>,
}

pub enum Incoming {
    Reply(String),
    /// The stream is out of step or the caller is not our server: stop.
    Reject,
}

struct WindowEntry {
    info: WindowInfo,
    app_identifier: Option<String>,
    elements: Vec<String>,
}

struct ElementEntry {
    window: String,
    role: String,
    /// Full label; the agent saw it clipped.
    label: String,
    /// The element's native identity; absent when the OS exposed none.
    native: Option<RefEntry>,
}

/// An observed element, confirmed live.
struct Resolved {
    role: String,
    label: String,
    bounds: Option<Rect>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepare,
    Dispatch,
}

/// What an authorize check is asked about, besides the window.
#[derive(Default)]
struct Target<'a> {
    element: Option<(&'a str, &'a str)>,
    point: Option<Point>,
    foreground: bool,
    /// A background accessibility action: no user-activity pause, no front check.
    background: bool,
}

const WINDOW_RESTARTED: &str = "The computer-use driver restarted; list windows again.";
const ELEMENT_RESTARTED: &str = "The computer-use driver restarted; observe again.";
const USER_ACTIVE: &str =
    "The user is using the mouse or keyboard. Wait a few seconds, then observe again and retry.";
const NO_IDENTITY: &str = "This app's controls cannot be acted on by ref on this macOS version; use a screenshot and click by coordinates.";

fn same_rect(left: Option<&Rect>, right: Option<&Rect>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            (left.x - right.x).abs() <= BOUNDS_TOLERANCE
                && (left.y - right.y).abs() <= BOUNDS_TOLERANCE
                && (left.width - right.width).abs() <= BOUNDS_TOLERANCE
                && (left.height - right.height).abs() <= BOUNDS_TOLERANCE
        }
        _ => false,
    }
}

fn rect_json(rect: &Rect) -> Value {
    json!({ "x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height })
}

fn contains(rect: &Rect, point: Point) -> bool {
    point.x >= rect.x
        && point.x <= rect.x + rect.width
        && point.y >= rect.y
        && point.y <= rect.y + rect.height
}

fn center(rect: &Rect) -> Point {
    Point {
        x: rect.x + rect.width / 2.0,
        y: rect.y + rect.height / 2.0,
    }
}

/// `cmd+shift+z` as a key combo; `None` for a chord the OS layer cannot send.
pub fn parse_chord(chord: &str) -> Option<KeyCombo> {
    let mut parts: Vec<&str> = chord.split('+').collect();
    let last = parts.pop().filter(|key| !key.is_empty())?;
    let mut modifiers: Vec<Modifier> = Vec::new();
    for part in parts {
        let modifier = match part {
            "cmd" | "meta" | "super" => Modifier::Meta,
            "ctrl" => Modifier::Ctrl,
            "alt" | "option" => Modifier::Alt,
            "shift" => Modifier::Shift,
            _ => return None,
        };
        if !modifiers
            .iter()
            .any(|held| std::mem::discriminant(held) == std::mem::discriminant(&modifier))
        {
            modifiers.push(modifier);
        }
    }
    let single = last.len() == 1
        && last
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let function = last
        .strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (1..=12).contains(&number));
    let key = if single || function {
        last
    } else {
        match last {
            "enter" | "return" => "return",
            "tab" => "tab",
            "escape" => "escape",
            "space" => "space",
            "backspace" => "delete",
            "delete" => "forwarddelete",
            "up" | "down" | "left" | "right" | "home" | "end" | "pageup" | "pagedown" => last,
            _ => return None,
        }
    };
    Some(KeyCombo {
        key: key.to_string(),
        modifiers,
    })
}

/// The identity agent-desktop resolves the exact element from. `None` when
/// the OS gave the node no token: it can then only be reached by coordinates.
pub fn native_ref(window: &WindowInfo, node: &AccessibilityNode) -> Option<RefEntry> {
    let token = node.identity.retained_object.clone()?;
    let bounds = node.presentation.bounds;
    Some(RefEntry {
        process: RefProcess {
            pid: window.pid,
            process_instance: window.process_instance.clone(),
        },
        identity: RefEntryIdentity {
            retained_object: Some(token),
            role: node.role.clone(),
            name: node.identity.name.clone(),
            description: node.identity.description.clone(),
            ..RefEntryIdentity::default()
        },
        geometry: RefGeometry {
            bounds,
            bounds_hash: bounds.and_then(|bounds| bounds.bounds_hash()),
            bounds_space: None,
        },
        capabilities: RefCapabilities {
            states: node.presentation.states.clone(),
            available_actions: node.presentation.available_actions.clone(),
        },
        source: RefSource {
            source_app: Some(window.app.clone()),
            source_window_id: Some(window.id.clone()),
            source_window_title: Some(window.title.clone()),
            ..RefSource::default()
        },
        scope: RefScope {
            root_ref: None,
            path_is_absolute: false,
            path: Default::default(),
        },
    })
}

pub struct Driver<D> {
    desktop: D,
    nonce: String,
    epoch: String,
    next_handle: u64,
    windows: HashMap<String, WindowEntry>,
    elements: HashMap<String, ElementEntry>,
    element_order: VecDeque<String>,
    /// When this driver last sent input, so its own events do not read as the user's.
    last_input: Option<Instant>,
    /// The request being answered; authorize lines carry its id.
    active: u64,
}

impl<D: Desktop> Driver<D> {
    pub fn new(desktop: D, nonce: String, epoch: String) -> Self {
        Self {
            desktop,
            nonce,
            epoch,
            next_handle: 1,
            windows: HashMap::new(),
            elements: HashMap::new(),
            element_order: VecDeque::new(),
            last_input: None,
            active: 0,
        }
    }

    fn mint(&mut self, prefix: char) -> String {
        let handle = format!("{prefix}{}.{}", self.epoch, self.next_handle);
        self.next_handle += 1;
        handle
    }

    /// Answers one request line. A line that is not a request for this worker
    /// gets no reply: the server times out and starts a fresh worker.
    pub fn handle_line(&mut self, line: &str, channel: &mut dyn Channel) -> Option<Incoming> {
        let envelope: Envelope = serde_json::from_str(line).ok()?;
        if envelope.nonce != self.nonce {
            return Some(Incoming::Reject);
        }
        let request = envelope.request?;
        self.active = envelope.id;
        let reply = match serde_json::from_value::<Request>(request) {
            Ok(request) => self.answer(request, channel),
            Err(_) => Err(Failure::new("malformed", "The request was not understood.")),
        };
        let message = match reply {
            Ok(result) => json!({ "id": envelope.id, "ok": true, "result": result }),
            Err(failure) => {
                let mut error = json!({
                    "kind": failure.kind,
                    "message": failure.message,
                    "dispatched": match failure.dispatched {
                        Dispatched::No => "no",
                        Dispatched::Unknown => "unknown",
                    },
                });
                if let Some(code) = failure.code {
                    error["code"] = json!(code);
                }
                if failure.restarted {
                    error["reason"] = json!("restarted");
                }
                json!({ "id": envelope.id, "ok": false, "error": error })
            }
        };
        Some(Incoming::Reply(message.to_string()))
    }

    fn answer(&mut self, request: Request, channel: &mut dyn Channel) -> Result<Value, Failure> {
        match request {
            Request::Status => {
                let status = self.desktop.status();
                let mut result = json!({
                    "available": status.available,
                    "accessibility": status.accessibility,
                });
                if let Some(reason) = status.reason {
                    result["reason"] = json!(reason);
                }
                Ok(result)
            }
            Request::ListWindows => self.list_windows(),
            // Clicks and drags release their button from a guard, even on failure.
            Request::ReleaseMouse => Ok(Value::Null),
            Request::Observe {
                window,
                max_elements,
                query,
            } => self.observe(&window, max_elements, query.as_deref()),
            Request::Screenshot {
                window,
                output_path,
                max_size,
            } => self.screenshot(&window, Path::new(&output_path), max_size),
            Request::ElementAt { window, point } => self.element_at(&window, point),
            Request::Press { element, expect } => self.press(&element, &expect, channel),
            Request::SetValue {
                element,
                expect,
                value,
            } => self.set_value(&element, &expect, &value, channel),
            Request::TypeText {
                element,
                expect,
                text,
            } => self.type_text(&element, &expect, &text, channel),
            Request::Scroll {
                element,
                expect,
                dx,
                dy,
            } => self.scroll(&element, &expect, dx, dy, channel),
            Request::Key { window, keys } => self.key(&window, &keys, channel),
            Request::TypeFocused { window, text } => self.type_focused(&window, &text, channel),
            Request::Focus { window } => self.focus(&window, channel),
            Request::Click {
                window,
                expect_bounds,
                point,
                button,
                count,
            } => {
                let button = match button.as_str() {
                    "left" => MouseButton::Left,
                    "right" => MouseButton::Right,
                    "middle" => MouseButton::Middle,
                    _ => return Err(Failure::new("malformed", "Unknown mouse button.")),
                };
                let point = to_point(point);
                self.coordinate(
                    &window,
                    expect_bounds,
                    &[point],
                    point,
                    channel,
                    |desktop| desktop.click(point, button, count.max(1)),
                )
            }
            Request::Drag {
                window,
                expect_bounds,
                from,
                to,
            } => {
                let (from, to) = (to_point(from), to_point(to));
                self.coordinate(
                    &window,
                    expect_bounds,
                    &[from, to],
                    from,
                    channel,
                    |desktop| desktop.drag(from, to),
                )
            }
            Request::Move {
                window,
                expect_bounds,
                point,
            } => {
                let point = to_point(point);
                self.coordinate(
                    &window,
                    expect_bounds,
                    &[point],
                    point,
                    channel,
                    |desktop| desktop.move_to(point),
                )
            }
            Request::ScrollAt {
                window,
                expect_bounds,
                point,
                dx,
                dy,
            } => {
                let point = to_point(point);
                self.coordinate(
                    &window,
                    expect_bounds,
                    &[point],
                    point,
                    channel,
                    |desktop| desktop.wheel(point, dx, dy),
                )
            }
        }
    }

    // --- Observation ---------------------------------------------------------

    fn list_windows(&mut self) -> Result<Value, Failure> {
        let listed = self.desktop.list_windows()?;
        // A window that is still there keeps its handle: the service lists
        // around every screenshot and matches windows by handle.
        let mut known: HashMap<(u32, String), String> = self
            .windows
            .iter()
            .map(|(handle, entry)| {
                (
                    (entry.info.pid.get(), entry.info.id.clone()),
                    handle.clone(),
                )
            })
            .collect();
        let mut previous = std::mem::take(&mut self.windows);
        let mut windows = Vec::with_capacity(listed.len());
        for window in listed {
            let key = (window.info.pid.get(), window.info.id.clone());
            let (handle, elements) = match known.remove(&key) {
                Some(handle) => {
                    let elements = previous
                        .remove(&handle)
                        .map(|entry| entry.elements)
                        .unwrap_or_default();
                    (handle, elements)
                }
                None => (self.mint('w'), Vec::new()),
            };
            let mut entry = json!({
                "handle": handle,
                "app": window.info.app,
                "pid": window.info.pid.get(),
                "title": window.info.title,
                "focused": window.info.state.is_focused,
            });
            if let Some(bounds) = &window.info.bounds {
                entry["bounds"] = rect_json(bounds);
            }
            if let Some(identifier) = &window.app_identifier {
                entry["appIdentifier"] = json!(identifier);
            }
            self.windows.insert(
                handle,
                WindowEntry {
                    info: window.info,
                    app_identifier: window.app_identifier,
                    elements,
                },
            );
            windows.push(entry);
        }
        // Windows that were not listed again are gone, and their controls with them.
        for (_, gone) in previous {
            for element in gone.elements {
                self.elements.remove(&element);
            }
        }
        Ok(Value::Array(windows))
    }

    fn owner_epoch(handle: &str) -> Option<&str> {
        handle
            .rfind('.')
            .filter(|dot| *dot > 1)
            .map(|dot| &handle[1..dot])
    }

    fn restarted(message: &str) -> Failure {
        Failure {
            restarted: true,
            ..Failure::stale(message)
        }
    }

    fn require_window(&self, handle: &str) -> Result<(), Failure> {
        if Self::owner_epoch(handle).is_some_and(|owner| owner != self.epoch) {
            return Err(Self::restarted(WINDOW_RESTARTED));
        }
        if self.windows.contains_key(handle) {
            Ok(())
        } else {
            Err(Failure::stale("Unknown window; list windows again."))
        }
    }

    /// Re-reads the window and adopts its live bounds and state.
    fn refresh(&mut self, handle: &str) -> Result<WindowInfo, Failure> {
        let entry = self
            .windows
            .get_mut(handle)
            .ok_or_else(|| Failure::stale("Unknown window; list windows again."))?;
        let fresh = self.desktop.refresh(&entry.info)?;
        entry.info = fresh.clone();
        Ok(fresh)
    }

    fn observe(
        &mut self,
        handle: &str,
        max_elements: usize,
        query: Option<&str>,
    ) -> Result<Value, Failure> {
        self.require_window(handle)?;
        let window = self.refresh(handle)?;
        let tree = self.desktop.tree(&window, OBSERVE_TIMEOUT_MS)?;
        let observation = observe::flatten(&tree, max_elements, query);

        // A new observation of a window replaces its previous handles.
        let previous = self
            .windows
            .get_mut(handle)
            .map(|entry| std::mem::take(&mut entry.elements))
            .unwrap_or_default();
        for element in previous {
            self.elements.remove(&element);
        }

        let mut minted = Vec::with_capacity(observation.elements.len());
        let mut elements = Vec::with_capacity(observation.elements.len());
        for (element, node) in observation.elements {
            let element_handle = self.mint('e');
            let mut entry = json!({
                "handle": element_handle,
                "role": element.role,
                "label": element.label,
                "enabled": element.enabled,
                "focused": element.focused,
            });
            if let Some(value) = element.value {
                entry["value"] = json!(value);
            }
            if let Some(section) = element.section {
                entry["section"] = json!(section);
            }
            if let Some(instance) = element.instance {
                entry["instance"] = json!(instance);
            }
            self.elements.insert(
                element_handle.clone(),
                ElementEntry {
                    window: handle.to_string(),
                    role: node.role.clone(),
                    label: observe::label_of(node),
                    native: native_ref(&window, node),
                },
            );
            self.element_order.push_back(element_handle.clone());
            minted.push(element_handle);
            elements.push(entry);
        }
        if let Some(entry) = self.windows.get_mut(handle) {
            entry.elements = minted;
        }
        while self.elements.len() > MAX_ELEMENT_HANDLES {
            let Some(oldest) = self.element_order.pop_front() else {
                break;
            };
            self.elements.remove(&oldest);
        }
        Ok(json!({ "elements": elements, "truncated": observation.truncated }))
    }

    fn element_at(&mut self, handle: &str, point: WirePoint) -> Result<Value, Failure> {
        self.require_window(handle)?;
        let window = self.refresh(handle)?;
        Ok(match self.name_at(&window, to_point(point)) {
            Some((role, label)) => json!({ "role": role, "label": label }),
            None => Value::Null,
        })
    }

    /// Best effort: an app with no readable tree simply names nothing.
    fn name_at(&self, window: &WindowInfo, point: Point) -> Option<(String, String)> {
        let tree = self.desktop.tree(window, ELEMENT_AT_TIMEOUT_MS).ok()?;
        observe::labelled_at(&tree, point.x, point.y)
    }

    fn screenshot(&mut self, handle: &str, output: &Path, max_size: u32) -> Result<Value, Failure> {
        self.require_window(handle)?;
        let window = self.refresh(handle)?;
        let bounds = window
            .bounds
            .filter(|bounds| bounds.width > 0.0 && bounds.height > 0.0)
            .ok_or_else(|| Failure::failed("The window is not on screen."))?;
        let png = self.desktop.screenshot(&window)?;
        let (width, height) = parse_png_dimensions(&png)
            .filter(|(width, height)| *width > 0 && *height > 0)
            .ok_or_else(|| Failure::failed("The screenshot could not be read."))?;
        // Coordinates are mapped through the window's bounds, so an image of
        // anything but the window alone would send clicks to the wrong place.
        let scale = f64::from(width) / bounds.width;
        if (f64::from(height) / scale - bounds.height).abs() > 2.0 {
            return Err(Failure::failed(
                "Could not capture this window by itself. It may be on another desktop or in full screen (focus --window N switches to it), minimized, or sharing its place with another window of the app.",
            ));
        }
        let write = |failure: std::io::Error| {
            Failure::failed(&format!(
                "Could not write the screenshot ({}).",
                failure.kind()
            ))
        };
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).map_err(write)?;
        }
        let (width, height) = if width.max(height) > max_size.max(1) {
            let full = output.with_extension("window.png");
            std::fs::write(&full, &png).map_err(write)?;
            let shrunk = self.desktop.shrink_png(&full, output, max_size.max(1));
            let _ = std::fs::remove_file(&full);
            shrunk?;
            let written = std::fs::read(output).map_err(write)?;
            parse_png_dimensions(&written)
                .ok_or_else(|| Failure::failed("The screenshot could not be read."))?
        } else {
            std::fs::write(output, &png).map_err(write)?;
            (width, height)
        };
        Ok(json!({ "width": width, "height": height, "bounds": rect_json(&bounds) }))
    }

    // --- The checks around input ----------------------------------------------

    /// Refuses input that takes the screen while the user is at the mouse or keyboard.
    fn pause_for_user(&self) -> Result<(), Failure> {
        let Some(idle) = self.desktop.idle_seconds() else {
            return Ok(());
        };
        if idle >= USER_ACTIVE_SECONDS {
            return Ok(());
        }
        let own = self.last_input.map(|at| at.elapsed().as_secs_f64());
        if own.is_some_and(|own| idle >= own - OWN_INPUT_SLACK_SECONDS) {
            return Ok(());
        }
        Err(Failure {
            code: Some("CU-CON-009".into()),
            ..Failure::new("policy", USER_ACTIVE)
        })
    }

    /// Brings the exact window to the front, or refuses.
    fn activate(&mut self, handle: &str) -> Result<(), Failure> {
        let window = self.refresh(handle)?;
        if self.desktop.is_front(&window) {
            return Ok(());
        }
        self.pause_for_user()?;
        // The poll below decides; a refused raise is reported there.
        let _ = self.desktop.focus_window(&window);
        let mut waited = 0;
        loop {
            if self.desktop.is_front(&window) {
                self.last_input = Some(Instant::now());
                return Ok(());
            }
            if waited >= ACTIVATION_TIMEOUT_MS {
                return Err(Failure::failed(
                    "Could not bring the target window to the front.",
                ));
            }
            self.desktop.sleep(ACTIVATION_POLL_MS);
            waited += ACTIVATION_POLL_MS;
        }
    }

    /// Asks the server about the freshly read target. An allowed `dispatch`
    /// holds the server's lock until the next line this process writes.
    fn authorize(
        &mut self,
        handle: &str,
        target: Target<'_>,
        phase: Phase,
        channel: &mut dyn Channel,
    ) -> Result<(), Failure> {
        if phase == Phase::Dispatch && !target.background {
            self.pause_for_user()?;
        }
        let before = self.windows.get(handle).and_then(|entry| entry.info.bounds);
        let window = self.refresh(handle)?;
        if before.is_some() && !same_rect(before.as_ref(), window.bounds.as_ref()) {
            return Err(Failure::stale("The window moved before dispatch."));
        }
        let named = match (phase, target.point, target.element) {
            (Phase::Dispatch, Some(point), None) => self.name_at(&window, point),
            _ => None,
        };
        let mut description = json!({
            "handle": handle,
            "app": window.app,
            "pid": window.pid.get(),
            "title": window.title,
            "focused": window.state.is_focused,
        });
        if let Some(bounds) = &window.bounds {
            description["bounds"] = rect_json(bounds);
        }
        if let Some(identifier) = self
            .windows
            .get(handle)
            .and_then(|entry| entry.app_identifier.as_ref())
        {
            description["appIdentifier"] = json!(identifier);
        }
        let mut wire = json!({ "window": description });
        match (target.element, &named) {
            (Some((role, label)), _) => wire["element"] = json!({ "role": role, "label": label }),
            (None, Some((role, label))) => {
                wire["element"] = json!({ "role": role, "label": label })
            }
            (None, None) => {}
        }
        if let Some(point) = target.point {
            wire["point"] = json!({ "x": point.x, "y": point.y });
        }
        if target.foreground {
            wire["foreground"] = json!(true);
        }
        let message = json!({
            "id": self.active,
            "authorize": wire,
            "phase": if phase == Phase::Prepare { "prepare" } else { "dispatch" },
        });
        let decision = channel
            .ask(&message)
            .and_then(|line| serde_json::from_str::<Value>(&line).ok())
            .filter(|answer| {
                answer.get("id").and_then(Value::as_u64) == Some(self.active)
                    && answer.get("nonce").and_then(Value::as_str) == Some(self.nonce.as_str())
                    && answer.get("decision").is_some()
            })
            .map(|answer| answer["decision"].clone());
        // A lost or out-of-step parent never counts as permission.
        let Some(decision) = decision else {
            return Err(Failure::new("policy", "The action was not authorized."));
        };
        if !decision.is_null() {
            return Err(Failure {
                code: decision
                    .get("code")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                ..Failure::new(
                    "policy",
                    decision
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("The action was refused."),
                )
            });
        }
        if phase == Phase::Dispatch {
            let window = self.refresh(handle).map_err(Failure::after_dispatch)?;
            if before.is_some() && !same_rect(before.as_ref(), window.bounds.as_ref()) {
                return Err(Failure::stale("The window moved before dispatch."));
            }
            if !target.background && !self.desktop.is_front(&window) {
                return Err(Failure::failed(
                    "The target window lost focus before the input.",
                ));
            }
        }
        Ok(())
    }

    /// Runs native input the server just allowed. Errors here may have landed.
    fn send<F>(&mut self, native: F) -> Result<(), Failure>
    where
        F: FnOnce(&D) -> Result<(), Failure>,
    {
        self.last_input = Some(Instant::now());
        let outcome = native(&self.desktop);
        self.last_input = Some(Instant::now());
        outcome
    }

    /// The observed element, still the same control: role and label as the
    /// agent saw them, resolved by its native identity.
    fn resolve(
        &mut self,
        handle: &str,
        expect: &Expect,
    ) -> Result<(String, RefEntry, Resolved), Failure> {
        if Self::owner_epoch(handle).is_some_and(|owner| owner != self.epoch) {
            return Err(Self::restarted(ELEMENT_RESTARTED));
        }
        let entry = self
            .elements
            .get(handle)
            .ok_or_else(|| Failure::stale("Unknown element; observe again."))?;
        if entry.role != expect.role || observe::clip(&entry.label) != expect.label {
            return Err(Failure::stale("The element changed since it was observed."));
        }
        let native = entry
            .native
            .clone()
            .ok_or_else(|| Failure::failed(NO_IDENTITY))?;
        let (window, role, label) = (
            entry.window.clone(),
            entry.role.clone(),
            entry.label.clone(),
        );
        self.require_window(&window)?;
        self.refresh(&window)?;
        let live = self.desktop.live(&native)?;
        // The native identity proves it is the same control; a control whose
        // role or label changed is no longer what the agent, or an approval, named.
        if live.role != role || live.label.as_ref().is_some_and(|live| *live != label) {
            return Err(Failure::stale("The element changed since it was observed."));
        }
        Ok((
            window,
            native,
            Resolved {
                role,
                label,
                bounds: live.bounds,
            },
        ))
    }

    fn input_result(took_focus: bool) -> Value {
        json!({ "tookFocus": took_focus })
    }

    // --- Input by ref -----------------------------------------------------------

    /// The two checks every background accessibility action passes.
    fn background_checks(
        &mut self,
        handle: &str,
        expect: &Expect,
        channel: &mut dyn Channel,
    ) -> Result<(String, RefEntry), Failure> {
        let (window, _, live) = self.resolve(handle, expect)?;
        let element = Some((live.role.as_str(), live.label.as_str()));
        self.authorize(
            &window,
            Target {
                element,
                background: true,
                ..Target::default()
            },
            Phase::Prepare,
            channel,
        )?;
        let (window, native, live) = self.resolve(handle, expect)?;
        let element = Some((live.role.as_str(), live.label.as_str()));
        self.authorize(
            &window,
            Target {
                element,
                background: true,
                ..Target::default()
            },
            Phase::Dispatch,
            channel,
        )?;
        Ok((window, native))
    }

    /// The checks for input that needs the element's window in front.
    fn foreground_checks(
        &mut self,
        handle: &str,
        expect: &Expect,
        channel: &mut dyn Channel,
    ) -> Result<(RefEntry, Resolved), Failure> {
        let (window, _, live) = self.resolve(handle, expect)?;
        let element = Some((live.role.as_str(), live.label.as_str()));
        self.authorize(
            &window,
            Target {
                element,
                foreground: true,
                ..Target::default()
            },
            Phase::Prepare,
            channel,
        )?;
        self.activate(&window)?;
        let (window, native, live) = self.resolve(handle, expect)?;
        let element = Some((live.role.as_str(), live.label.as_str()));
        self.authorize(
            &window,
            Target {
                element,
                ..Target::default()
            },
            Phase::Dispatch,
            channel,
        )?;
        Ok((native, live))
    }

    fn press(
        &mut self,
        handle: &str,
        expect: &Expect,
        channel: &mut dyn Channel,
    ) -> Result<Value, Failure> {
        let (_, native) = self.background_checks(handle, expect, channel)?;
        match self.send(|desktop| desktop.press(&native)) {
            Ok(()) => Ok(Self::input_result(false)),
            // No accessibility press on this control: click it instead.
            Err(failure) if failure.unsupported => {
                let (_, live) = self.foreground_checks(handle, expect, channel)?;
                let bounds = live
                    .bounds
                    .ok_or_else(|| Failure::failed("The element cannot be pressed."))?;
                let point = center(&bounds);
                self.send(|desktop| desktop.click(point, MouseButton::Left, 1))
                    .map_err(Failure::after_dispatch)?;
                Ok(Self::input_result(true))
            }
            Err(failure) => Err(failure),
        }
    }

    fn set_value(
        &mut self,
        handle: &str,
        expect: &Expect,
        value: &str,
        channel: &mut dyn Channel,
    ) -> Result<Value, Failure> {
        let (_, native) = self.background_checks(handle, expect, channel)?;
        match self.send(|desktop| desktop.set_value(&native, value)) {
            Ok(()) => Ok(Self::input_result(false)),
            Err(failure) if failure.unsupported => Err(failure),
            // The app ignored the change: type it instead. The first attempt
            // may have had an effect, so nothing here reads as untouched.
            Err(_) => self
                .type_into(handle, expect, value, true, channel)
                .map_err(Failure::after_dispatch),
        }
    }

    fn type_text(
        &mut self,
        handle: &str,
        expect: &Expect,
        text: &str,
        channel: &mut dyn Channel,
    ) -> Result<Value, Failure> {
        let (_, native) = self.background_checks(handle, expect, channel)?;
        match self.send(|desktop| desktop.insert_text(&native, text)) {
            Ok(()) => Ok(Self::input_result(false)),
            Err(failure) if failure.unsupported => {
                self.type_into(handle, expect, text, false, channel)
            }
            Err(_) => self
                .type_into(handle, expect, text, false, channel)
                .map_err(Failure::after_dispatch),
        }
    }

    fn type_into(
        &mut self,
        handle: &str,
        expect: &Expect,
        text: &str,
        replace: bool,
        channel: &mut dyn Channel,
    ) -> Result<Value, Failure> {
        let (native, _) = self.foreground_checks(handle, expect, channel)?;
        self.send(|desktop| desktop.type_into(&native, text, replace))
            .map_err(Failure::after_dispatch)?;
        Ok(Self::input_result(true))
    }

    fn scroll(
        &mut self,
        handle: &str,
        expect: &Expect,
        dx: i32,
        dy: i32,
        channel: &mut dyn Channel,
    ) -> Result<Value, Failure> {
        let (_, live) = self.foreground_checks(handle, expect, channel)?;
        let bounds = live
            .bounds
            .ok_or_else(|| Failure::failed("The element cannot be scrolled."))?;
        let point = center(&bounds);
        self.send(|desktop| desktop.wheel(point, dx, dy))
            .map_err(Failure::after_dispatch)?;
        Ok(Self::input_result(true))
    }

    // --- Input to a window --------------------------------------------------------

    /// `prepare`, bring the window forward, then `dispatch` for the window alone.
    fn window_checks(
        &mut self,
        handle: &str,
        channel: &mut dyn Channel,
    ) -> Result<WindowInfo, Failure> {
        self.authorize(
            handle,
            Target {
                foreground: true,
                ..Target::default()
            },
            Phase::Prepare,
            channel,
        )?;
        self.activate(handle)?;
        self.authorize(handle, Target::default(), Phase::Dispatch, channel)?;
        self.windows
            .get(handle)
            .map(|entry| entry.info.clone())
            .ok_or_else(|| Failure::stale("Unknown window; list windows again."))
    }

    fn key(
        &mut self,
        handle: &str,
        keys: &str,
        channel: &mut dyn Channel,
    ) -> Result<Value, Failure> {
        self.require_window(handle)?;
        let combo = parse_chord(keys).ok_or_else(|| Failure::failed("Unsupported key chord."))?;
        let window = self.window_checks(handle, channel)?;
        self.send(|desktop| desktop.key(&window, &combo))
            .map_err(Failure::after_dispatch)?;
        Ok(Self::input_result(true))
    }

    fn type_focused(
        &mut self,
        handle: &str,
        text: &str,
        channel: &mut dyn Channel,
    ) -> Result<Value, Failure> {
        self.require_window(handle)?;
        let window = self.window_checks(handle, channel)?;
        self.send(|desktop| desktop.type_focused(&window, text))
            .map_err(Failure::after_dispatch)?;
        Ok(Self::input_result(true))
    }

    fn focus(&mut self, handle: &str, channel: &mut dyn Channel) -> Result<Value, Failure> {
        self.require_window(handle)?;
        self.authorize(
            handle,
            Target {
                foreground: true,
                ..Target::default()
            },
            Phase::Prepare,
            channel,
        )?;
        self.activate(handle)?;
        Ok(Self::input_result(true))
    }

    // --- Input by coordinates --------------------------------------------------------

    /// Coordinates come from a screenshot: they are only good while the window
    /// sits exactly where that screenshot saw it.
    fn coordinate<F>(
        &mut self,
        handle: &str,
        expect_bounds: WireRect,
        points: &[Point],
        target: Point,
        channel: &mut dyn Channel,
        native: F,
    ) -> Result<Value, Failure>
    where
        F: FnOnce(&D) -> Result<(), Failure>,
    {
        self.require_window(handle)?;
        self.authorize(
            handle,
            Target {
                foreground: true,
                ..Target::default()
            },
            Phase::Prepare,
            channel,
        )?;
        self.activate(handle)?;
        let window = self.refresh(handle)?;
        let expected = Rect {
            x: expect_bounds.x,
            y: expect_bounds.y,
            width: expect_bounds.width,
            height: expect_bounds.height,
        };
        if !same_rect(Some(&expected), window.bounds.as_ref()) {
            return Err(Failure::stale(
                "The window moved or resized since the screenshot.",
            ));
        }
        if points.iter().any(|point| !contains(&expected, *point)) {
            return Err(Failure::failed("The point is outside the window."));
        }
        self.authorize(
            handle,
            Target {
                point: Some(target),
                ..Target::default()
            },
            Phase::Dispatch,
            channel,
        )?;
        self.send(native).map_err(Failure::after_dispatch)?;
        Ok(Self::input_result(true))
    }
}

fn to_point(point: WirePoint) -> Point {
    Point {
        x: point.x,
        y: point.y,
    }
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;
