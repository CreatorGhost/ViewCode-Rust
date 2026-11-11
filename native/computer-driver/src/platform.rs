//! The native desktop behind the driver: agent-desktop's macOS adapter, or an
//! answer that this platform is not supported yet.

#[cfg(target_os = "macos")]
pub use mac::NativeDesktop;
#[cfg(not(target_os = "macos"))]
pub use unsupported::NativeDesktop;

#[cfg(target_os = "macos")]
mod mac {
    use std::path::Path;
    use std::process::Command;

    use agent_desktop_core::{
        AccessibilityNode, Action, ActionRequest, AdapterError, Deadline, DeliverySemantics,
        DragParams, ErrorCode, InputOps, InteractionLease, InteractionPolicy, KeyCombo, Modifier,
        MouseButton, MouseEvent, MouseEventKind, NativeHandle, ObservationOps, PermissionState,
        RefEntry, SystemOps, WindowFilter, WindowInfo, execute_verified_action, tree::state,
        tree::tree_options::TreeOptions,
    };
    use agent_desktop_macos::MacOSAdapter;

    use crate::driver::{
        Desktop, Dispatched, Failure, ListedWindow, Live, Point, Status, native_ref,
    };

    /// Leaves room under the server's 10 s list and 15 s observe timeouts.
    const LIST_TIMEOUT_MS: u64 = 8_000;
    /// One window, one element or one input: the server allows 20 s per input op.
    const QUICK_TIMEOUT_MS: u64 = 5_000;
    const INPUT_TIMEOUT_MS: u64 = 8_000;
    const SCREENSHOT_TIMEOUT_MS: u64 = 15_000;
    /// agent-desktop skips anonymous web wrappers, so this is logical depth.
    const OBSERVE_MAX_DEPTH: u8 = 40;
    const DRAG_DURATION_MS: u64 = 500;

    const NO_ACCESSIBILITY: &str = "Accessibility permission is not granted to ViewCode. Grant it in System Settings > Privacy & Security > Accessibility.";
    const NO_SCREEN_RECORDING: &str = "Screen Recording permission is not granted to ViewCode. Grant it in System Settings > Privacy & Security > Screen Recording.";
    const NOT_CAPTURED: &str = "Could not capture this window by itself. It may be on another desktop or in full screen (focus --window N switches to it), minimized, or sharing its place with another window of the app.";

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    }
    /// The hardware event state: real mouse and keyboard use, whoever is in front.
    const HID_SYSTEM_STATE: i32 = 1;
    const ANY_INPUT_EVENT: u32 = u32::MAX;

    pub struct NativeDesktop {
        adapter: MacOSAdapter,
    }

    impl NativeDesktop {
        pub fn new() -> Self {
            Self {
                adapter: MacOSAdapter::new(),
            }
        }

        fn lease(&self, timeout_ms: u64) -> Result<InteractionLease, Failure> {
            InteractionLease::guarded(deadline(timeout_ms)?, ()).map_err(failure)
        }

        fn handle(&self, entry: &RefEntry) -> Result<NativeHandle, Failure> {
            self.adapter
                .resolve_element_strict(entry, deadline(QUICK_TIMEOUT_MS)?)
                .map_err(|error| match error.code {
                    ErrorCode::StaleRef
                    | ErrorCode::ElementNotFound
                    | ErrorCode::AmbiguousTarget
                    | ErrorCode::WindowNotFound
                    | ErrorCode::AppNotFound => {
                        Failure::new("stale", "The observed element is gone; observe again.")
                    }
                    _ => failure(error),
                })
        }

        /// One action on the exact element. A failure may have reached the app
        /// unless agent-desktop says nothing was delivered.
        fn act(&self, handle: &NativeHandle, request: ActionRequest) -> Result<(), Failure> {
            let lease = self.lease(INPUT_TIMEOUT_MS)?;
            execute_verified_action(&self.adapter, handle, request, &lease)
                .map(|_| ())
                .map_err(input_failure)
        }
    }

    fn deadline(timeout_ms: u64) -> Result<Deadline, Failure> {
        Deadline::after(timeout_ms).map_err(failure)
    }

    /// Adapter messages can name apps and controls, so ours stay generic.
    fn failure(error: AdapterError) -> Failure {
        match error.code {
            ErrorCode::PermDenied => Failure::new("permission-accessibility", NO_ACCESSIBILITY),
            ErrorCode::WindowNotFound
            | ErrorCode::AppNotFound
            | ErrorCode::StaleRef
            | ErrorCode::AmbiguousTarget
            | ErrorCode::ElementNotFound => {
                Failure::new("stale", "The element or window no longer exists.")
            }
            ErrorCode::Timeout | ErrorCode::AppUnresponsive => {
                Failure::new("timeout", "The OS did not respond in time.")
            }
            ErrorCode::PlatformNotSupported => Failure::new(
                "unavailable",
                "The accessibility service is not available on this machine.",
            ),
            ErrorCode::ActionNotSupported | ErrorCode::PolicyDenied => Failure {
                unsupported: true,
                ..Failure::new("failed", "The element does not support this action.")
            },
            _ => Failure::new("failed", "The OS refused or failed the action."),
        }
    }

    fn input_failure(error: AdapterError) -> Failure {
        let delivered = error.disposition != DeliverySemantics::not_delivered();
        // An accessibility action that never reached the app can be retried by other means.
        let exhausted = !delivered && error.code == ErrorCode::ActionFailed;
        let mut mapped = failure(error);
        mapped.unsupported = mapped.unsupported || exhausted;
        if delivered {
            mapped.dispatched = Dispatched::Unknown;
        }
        mapped
    }

    /// The executable path, which the server's denylist reads `.app` bundles from.
    fn executable_path(pid: u32) -> Option<String> {
        let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let pid = i32::try_from(pid).ok()?;
        // SAFETY: the buffer is writable for the length passed.
        let length =
            unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        if length <= 0 {
            return None;
        }
        buffer.truncate(length as usize);
        String::from_utf8(buffer).ok()
    }

    fn core_point(point: Point) -> agent_desktop_core::Point {
        agent_desktop_core::Point {
            x: point.x,
            y: point.y,
        }
    }

    /// The deepest node holding keyboard focus that the OS gave an identity.
    fn focused_node(node: &AccessibilityNode) -> Option<&AccessibilityNode> {
        node.children.iter().find_map(focused_node).or_else(|| {
            (node.identity.retained_object.is_some()
                && node
                    .presentation
                    .states
                    .iter()
                    .any(|token| token == state::FOCUSED))
            .then_some(node)
        })
    }

    impl Desktop for NativeDesktop {
        fn status(&self) -> Status {
            let report = deadline(LIST_TIMEOUT_MS)
                .and_then(|deadline| self.adapter.permission_report(deadline).map_err(failure));
            match report {
                Ok(report) => match report.accessibility {
                    PermissionState::Granted => Status {
                        available: true,
                        accessibility: "granted",
                        reason: None,
                    },
                    PermissionState::Denied { .. } => Status {
                        available: true,
                        accessibility: "denied",
                        reason: Some(NO_ACCESSIBILITY.into()),
                    },
                    PermissionState::NotRequired | PermissionState::Unknown => Status {
                        available: true,
                        accessibility: "unknown",
                        reason: None,
                    },
                },
                Err(error) => Status {
                    available: false,
                    accessibility: "unknown",
                    reason: Some(error.message),
                },
            }
        }

        fn list_windows(&self) -> Result<Vec<ListedWindow>, Failure> {
            let filter = WindowFilter {
                focused_only: false,
                app: None,
            };
            let windows = self
                .adapter
                .list_windows(&filter, deadline(LIST_TIMEOUT_MS)?)
                .map_err(failure)?;
            Ok(windows
                .into_iter()
                .filter(|window| !window.is_process_owner())
                .map(|info| ListedWindow {
                    app_identifier: executable_path(info.pid.get()),
                    info,
                })
                .collect())
        }

        fn tree(&self, window: &WindowInfo, timeout_ms: u64) -> Result<AccessibilityNode, Failure> {
            let options = TreeOptions {
                max_depth: OBSERVE_MAX_DEPTH,
                include_bounds: true,
                ..TreeOptions::default()
            };
            self.adapter
                .get_tree(window, &options, deadline(timeout_ms)?)
                .map_err(failure)
        }

        fn refresh(&self, window: &WindowInfo) -> Result<WindowInfo, Failure> {
            self.adapter
                .resolve_window_strict(window, deadline(QUICK_TIMEOUT_MS)?)
                .map_err(|error| match error.code {
                    ErrorCode::WindowNotFound
                    | ErrorCode::AppNotFound
                    | ErrorCode::StaleRef
                    | ErrorCode::AmbiguousTarget => Failure::new(
                        "stale",
                        "The window has closed or can no longer be told apart.",
                    ),
                    _ => failure(error),
                })
        }

        fn is_front(&self, window: &WindowInfo) -> bool {
            deadline(QUICK_TIMEOUT_MS)
                .ok()
                .and_then(|deadline| self.adapter.focused_window(deadline).ok().flatten())
                .is_some_and(|front| front.id == window.id && front.pid == window.pid)
        }

        fn focus_window(&self, window: &WindowInfo) -> Result<(), Failure> {
            let lease = self.lease(QUICK_TIMEOUT_MS)?;
            self.adapter.focus_window(window, &lease).map_err(failure)
        }

        fn idle_seconds(&self) -> Option<f64> {
            // SAFETY: a read of the HID event state; no pointers.
            let seconds = unsafe {
                CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT)
            };
            (seconds.is_finite() && seconds >= 0.0).then_some(seconds)
        }

        fn sleep(&self, milliseconds: u64) {
            std::thread::sleep(std::time::Duration::from_millis(milliseconds));
        }

        fn screenshot(&self, window: &WindowInfo) -> Result<Vec<u8>, Failure> {
            let denied = || Failure::new("permission-screen", NO_SCREEN_RECORDING);
            let report = self
                .adapter
                .permission_report(deadline(QUICK_TIMEOUT_MS)?)
                .map_err(failure)?;
            if matches!(report.screen_recording, PermissionState::Denied { .. }) {
                return Err(denied());
            }
            self.adapter
                .screenshot_window_frame(window, deadline(SCREENSHOT_TIMEOUT_MS)?)
                .map(|image| image.data)
                .map_err(|error| match error.code {
                    ErrorCode::PermDenied => denied(),
                    ErrorCode::Timeout => {
                        Failure::new("timeout", "The OS did not respond in time.")
                    }
                    _ => Failure::new("failed", NOT_CAPTURED),
                })
        }

        fn shrink_png(&self, source: &Path, output: &Path, max_size: u32) -> Result<(), Failure> {
            let shrunk = Command::new("/usr/bin/sips")
                .arg("-Z")
                .arg(max_size.to_string())
                .arg(source)
                .arg("--out")
                .arg(output)
                .output()
                .is_ok_and(|result| result.status.success());
            if shrunk {
                Ok(())
            } else {
                Err(Failure::new(
                    "failed",
                    "The screenshot could not be resized.",
                ))
            }
        }

        fn live(&self, entry: &RefEntry) -> Result<Live, Failure> {
            let handle = self.handle(entry)?;
            let element = self
                .adapter
                .get_live_element(&handle, deadline(QUICK_TIMEOUT_MS)?)
                .map_err(failure)?;
            let label = element
                .identity
                .name
                .known()
                .or_else(|| element.identity.description.known())
                .filter(|text| !text.trim().is_empty())
                .cloned();
            Ok(Live {
                role: element.state.role.to_string(),
                label,
                bounds: element.bounds,
            })
        }

        fn press(&self, entry: &RefEntry) -> Result<(), Failure> {
            let handle = self.handle(entry)?;
            self.act(&handle, ActionRequest::headless(Action::Click))
        }

        fn set_value(&self, entry: &RefEntry, value: &str) -> Result<(), Failure> {
            let handle = self.handle(entry)?;
            self.act(
                &handle,
                ActionRequest::headless(Action::SetValue(value.to_string())),
            )
        }

        fn insert_text(&self, entry: &RefEntry, text: &str) -> Result<(), Failure> {
            let handle = self.handle(entry)?;
            self.act(
                &handle,
                ActionRequest::headless(Action::TypeText(text.to_string())),
            )
        }

        fn type_into(&self, entry: &RefEntry, text: &str, replace: bool) -> Result<(), Failure> {
            let handle = self.handle(entry)?;
            self.act(&handle, ActionRequest::focus_fallback(Action::SetFocus))?;
            if replace {
                let select_all = KeyCombo {
                    key: "a".into(),
                    modifiers: vec![Modifier::Meta],
                };
                self.act(&handle, ActionRequest::headed(Action::PressKey(select_all)))?;
            }
            self.act(
                &handle,
                ActionRequest::headed(Action::TypeText(text.to_string())),
            )
        }

        fn type_focused(&self, window: &WindowInfo, text: &str) -> Result<(), Failure> {
            let tree = self.tree(window, QUICK_TIMEOUT_MS)?;
            let entry = focused_node(&tree)
                .and_then(|node| native_ref(window, node))
                .ok_or_else(|| {
                    Failure::new(
                        "failed",
                        "Nothing in the window has keyboard focus to type into. Click the field first.",
                    )
                })?;
            let handle = self.handle(&entry)?;
            self.act(
                &handle,
                ActionRequest::headed(Action::TypeText(text.to_string())),
            )
        }

        fn key(&self, window: &WindowInfo, combo: &KeyCombo) -> Result<(), Failure> {
            let lease = self.lease(INPUT_TIMEOUT_MS)?;
            self.adapter
                .press_key_for_window(window, combo, InteractionPolicy::headed(), &lease)
                .map(|_| ())
                .map_err(input_failure)
        }

        fn click(&self, point: Point, button: MouseButton, count: u32) -> Result<(), Failure> {
            let lease = self.lease(INPUT_TIMEOUT_MS)?;
            let event = MouseEvent {
                kind: MouseEventKind::Click { count },
                point: core_point(point),
                button,
                modifiers: Vec::new(),
            };
            self.adapter
                .mouse_event(event, &lease)
                .map(|_| ())
                .map_err(input_failure)
        }

        fn drag(&self, from: Point, to: Point) -> Result<(), Failure> {
            let lease = self.lease(INPUT_TIMEOUT_MS)?;
            let params = DragParams {
                from: core_point(from),
                to: core_point(to),
                duration_ms: Some(DRAG_DURATION_MS),
                drop_delay_ms: None,
            };
            self.adapter
                .drag(params, &lease)
                .map(|_| ())
                .map_err(input_failure)
        }

        fn move_to(&self, point: Point) -> Result<(), Failure> {
            let lease = self.lease(INPUT_TIMEOUT_MS)?;
            let event = MouseEvent {
                kind: MouseEventKind::Move,
                point: core_point(point),
                button: MouseButton::Left,
                modifiers: Vec::new(),
            };
            self.adapter
                .mouse_event(event, &lease)
                .map(|_| ())
                .map_err(input_failure)
        }

        fn wheel(&self, point: Point, dx: i32, dy: i32) -> Result<(), Failure> {
            let lease = self.lease(INPUT_TIMEOUT_MS)?;
            // The wheel counts lines with up as positive; the protocol's `dy`
            // is positive for scrolling the content down.
            let event = MouseEvent {
                kind: MouseEventKind::Wheel {
                    delta_x: f64::from(dx),
                    delta_y: -f64::from(dy),
                },
                point: core_point(point),
                button: MouseButton::Left,
                modifiers: Vec::new(),
            };
            self.adapter
                .mouse_event(event, &lease)
                .map(|_| ())
                .map_err(input_failure)
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod unsupported {
    use std::path::Path;

    use agent_desktop_core::{AccessibilityNode, KeyCombo, MouseButton, RefEntry, WindowInfo};

    use crate::driver::{Desktop, Failure, ListedWindow, Live, Point, Status};

    const REASON: &str = "Computer use supports macOS only so far.";

    pub struct NativeDesktop;

    impl NativeDesktop {
        pub fn new() -> Self {
            Self
        }
    }

    fn unavailable<T>() -> Result<T, Failure> {
        Err(Failure::new("unavailable", REASON))
    }

    impl Desktop for NativeDesktop {
        fn status(&self) -> Status {
            Status {
                available: false,
                accessibility: "unknown",
                reason: Some(REASON.into()),
            }
        }
        fn list_windows(&self) -> Result<Vec<ListedWindow>, Failure> {
            unavailable()
        }
        fn tree(&self, _: &WindowInfo, _: u64) -> Result<AccessibilityNode, Failure> {
            unavailable()
        }
        fn refresh(&self, _: &WindowInfo) -> Result<WindowInfo, Failure> {
            unavailable()
        }
        fn is_front(&self, _: &WindowInfo) -> bool {
            false
        }
        fn focus_window(&self, _: &WindowInfo) -> Result<(), Failure> {
            unavailable()
        }
        fn idle_seconds(&self) -> Option<f64> {
            None
        }
        fn sleep(&self, _: u64) {}
        fn screenshot(&self, _: &WindowInfo) -> Result<Vec<u8>, Failure> {
            unavailable()
        }
        fn shrink_png(&self, _: &Path, _: &Path, _: u32) -> Result<(), Failure> {
            unavailable()
        }
        fn live(&self, _: &RefEntry) -> Result<Live, Failure> {
            unavailable()
        }
        fn press(&self, _: &RefEntry) -> Result<(), Failure> {
            unavailable()
        }
        fn set_value(&self, _: &RefEntry, _: &str) -> Result<(), Failure> {
            unavailable()
        }
        fn insert_text(&self, _: &RefEntry, _: &str) -> Result<(), Failure> {
            unavailable()
        }
        fn type_into(&self, _: &RefEntry, _: &str, _: bool) -> Result<(), Failure> {
            unavailable()
        }
        fn type_focused(&self, _: &WindowInfo, _: &str) -> Result<(), Failure> {
            unavailable()
        }
        fn key(&self, _: &WindowInfo, _: &KeyCombo) -> Result<(), Failure> {
            unavailable()
        }
        fn click(&self, _: Point, _: MouseButton, _: u32) -> Result<(), Failure> {
            unavailable()
        }
        fn drag(&self, _: Point, _: Point) -> Result<(), Failure> {
            unavailable()
        }
        fn move_to(&self, _: Point) -> Result<(), Failure> {
            unavailable()
        }
        fn wheel(&self, _: Point, _: i32, _: i32) -> Result<(), Failure> {
            unavailable()
        }
    }
}
