use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::Path;

use agent_desktop_core::{ProcessId, WindowState};

use super::*;

const WINDOW_BOUNDS: Rect = Rect {
    x: 100.0,
    y: 50.0,
    width: 400.0,
    height: 300.0,
};

/// A desktop with one Notes window holding a "Done" button, which records
/// every native call so tests can assert what was, and was not, sent.
struct FakeDesktop {
    calls: RefCell<Vec<String>>,
    front: Cell<bool>,
    idle: Cell<Option<f64>>,
    bounds: Cell<Rect>,
    live_label: RefCell<Option<String>>,
    press: Cell<Option<&'static str>>,
    click_fails: Cell<bool>,
    listed: Cell<bool>,
    focus_works: Cell<bool>,
    image: Cell<(u32, u32)>,
    with_identity: Cell<bool>,
}

impl FakeDesktop {
    fn new() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            front: Cell::new(false),
            idle: Cell::new(None),
            bounds: Cell::new(WINDOW_BOUNDS),
            live_label: RefCell::new(Some("Done".into())),
            press: Cell::new(None),
            click_fails: Cell::new(false),
            listed: Cell::new(true),
            focus_works: Cell::new(true),
            image: Cell::new((800, 600)),
            with_identity: Cell::new(true),
        }
    }

    fn log(&self, call: impl Into<String>) {
        self.calls.borrow_mut().push(call.into());
    }

    fn info(&self) -> WindowInfo {
        WindowInfo {
            id: "w-77".into(),
            title: "Shopping list".into(),
            app: "Notes".into(),
            pid: ProcessId::from(42),
            process_instance: Some("boot:1".into()),
            bounds: Some(self.bounds.get()),
            state: WindowState {
                is_focused: self.front.get(),
                ..WindowState::default()
            },
        }
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    bytes
}

fn native_failure(kind: &'static str) -> Failure {
    match kind {
        "unsupported" => Failure {
            unsupported: true,
            ..Failure::new("failed", "The element does not support this action.")
        },
        _ => Failure {
            dispatched: Dispatched::Unknown,
            ..Failure::new("failed", "The OS refused or failed the action.")
        },
    }
}

impl Desktop for &FakeDesktop {
    fn status(&self) -> Status {
        Status {
            available: true,
            accessibility: "granted",
            reason: None,
        }
    }
    fn list_windows(&self) -> Result<Vec<ListedWindow>, Failure> {
        Ok(if self.listed.get() {
            vec![ListedWindow {
                info: self.info(),
                app_identifier: Some("/System/Applications/Notes.app/Contents/MacOS/Notes".into()),
            }]
        } else {
            Vec::new()
        })
    }
    fn tree(&self, _: &WindowInfo, _: u64) -> Result<AccessibilityNode, Failure> {
        let mut button = AccessibilityNode {
            ref_id: None,
            role: "button".into(),
            identity: Default::default(),
            presentation: Default::default(),
            children_count: None,
            subtree_truncated: false,
            children: vec![],
        };
        button.identity.name = Some("Done".into());
        if self.with_identity.get() {
            button.identity.retained_object = Some("axtok1:done".into());
        }
        button.presentation.bounds = Some(Rect {
            x: 120.0,
            y: 70.0,
            width: 60.0,
            height: 20.0,
        });
        let mut root = button.clone();
        root.role = "window".into();
        root.identity.name = Some("Shopping list".into());
        root.presentation.bounds = Some(self.bounds.get());
        root.children = vec![button];
        Ok(root)
    }
    fn refresh(&self, _: &WindowInfo) -> Result<WindowInfo, Failure> {
        if self.listed.get() {
            Ok(self.info())
        } else {
            Err(Failure::new(
                "stale",
                "The window has closed or can no longer be told apart.",
            ))
        }
    }
    fn is_front(&self, _: &WindowInfo) -> bool {
        self.front.get()
    }
    fn focus_window(&self, _: &WindowInfo) -> Result<(), Failure> {
        self.log("focus_window");
        if self.focus_works.get() {
            self.front.set(true);
        }
        Ok(())
    }
    fn idle_seconds(&self) -> Option<f64> {
        self.idle.get()
    }
    fn sleep(&self, _: u64) {}
    fn screenshot(&self, _: &WindowInfo) -> Result<Vec<u8>, Failure> {
        let (width, height) = self.image.get();
        Ok(png(width, height))
    }
    fn shrink_png(&self, _: &Path, output: &Path, max_size: u32) -> Result<(), Failure> {
        self.log(format!("shrink {max_size}"));
        std::fs::write(output, png(max_size, max_size * 3 / 4)).unwrap();
        Ok(())
    }
    fn live(&self, _: &RefEntry) -> Result<Live, Failure> {
        Ok(Live {
            role: "button".into(),
            label: self.live_label.borrow().clone(),
            bounds: Some(Rect {
                x: 120.0,
                y: 70.0,
                width: 60.0,
                height: 20.0,
            }),
        })
    }
    fn press(&self, entry: &RefEntry) -> Result<(), Failure> {
        self.log(format!(
            "press {}",
            entry.identity.retained_object.as_deref().unwrap_or("")
        ));
        self.press
            .get()
            .map_or(Ok(()), |kind| Err(native_failure(kind)))
    }
    fn set_value(&self, _: &RefEntry, value: &str) -> Result<(), Failure> {
        self.log(format!("set_value {value}"));
        Ok(())
    }
    fn insert_text(&self, _: &RefEntry, text: &str) -> Result<(), Failure> {
        self.log(format!("insert_text {text}"));
        Err(native_failure("unsupported"))
    }
    fn type_into(&self, _: &RefEntry, text: &str, replace: bool) -> Result<(), Failure> {
        self.log(format!("type_into {text} {replace}"));
        Ok(())
    }
    fn type_focused(&self, _: &WindowInfo, text: &str) -> Result<(), Failure> {
        self.log(format!("type_focused {text}"));
        Ok(())
    }
    fn key(&self, _: &WindowInfo, combo: &KeyCombo) -> Result<(), Failure> {
        self.log(format!("key {} {}", combo.key, combo.modifiers.len()));
        Ok(())
    }
    fn click(&self, point: Point, _: MouseButton, count: u32) -> Result<(), Failure> {
        self.log(format!("click {} {} x{count}", point.x, point.y));
        if self.click_fails.get() {
            Err(native_failure("os"))
        } else {
            Ok(())
        }
    }
    fn drag(&self, from: Point, to: Point) -> Result<(), Failure> {
        self.log(format!("drag {} {} -> {} {}", from.x, from.y, to.x, to.y));
        Ok(())
    }
    fn move_to(&self, point: Point) -> Result<(), Failure> {
        self.log(format!("move {} {}", point.x, point.y));
        Ok(())
    }
    fn wheel(&self, point: Point, dx: i32, dy: i32) -> Result<(), Failure> {
        self.log(format!("wheel {} {} {dx} {dy}", point.x, point.y));
        Ok(())
    }
}

/// The server's side: answers each dispatch check from a script and keeps what it was asked.
struct Server {
    decisions: VecDeque<Value>,
    asked: Vec<Value>,
    nonce: &'static str,
}

impl Server {
    fn allowing() -> Self {
        Self {
            decisions: VecDeque::new(),
            asked: Vec::new(),
            nonce: "n0nce",
        }
    }

    fn answering(decisions: Vec<Value>) -> Self {
        Self {
            decisions: decisions.into(),
            ..Self::allowing()
        }
    }

    fn phases(&self) -> Vec<&str> {
        self.asked
            .iter()
            .map(|ask| ask["phase"].as_str().unwrap())
            .collect()
    }
}

impl Channel for Server {
    fn ask(&mut self, message: &Value) -> Option<String> {
        self.asked.push(message.clone());
        let decision = self.decisions.pop_front().unwrap_or(Value::Null);
        Some(json!({ "id": message["id"], "nonce": self.nonce, "decision": decision }).to_string())
    }
}

fn send(driver: &mut Driver<&FakeDesktop>, server: &mut Server, id: u64, request: Value) -> Value {
    let line = json!({ "id": id, "nonce": "n0nce", "request": request }).to_string();
    match driver.handle_line(&line, server) {
        Some(Incoming::Reply(reply)) => serde_json::from_str(&reply).unwrap(),
        _ => panic!("expected a reply"),
    }
}

/// Lists and observes, returning the window and button handles.
fn observed(driver: &mut Driver<&FakeDesktop>) -> (String, String) {
    let mut server = Server::allowing();
    let windows = send(driver, &mut server, 1, json!({ "op": "listWindows" }));
    let window = windows["result"][0]["handle"].as_str().unwrap().to_string();
    let seen = send(
        driver,
        &mut server,
        2,
        json!({ "op": "observe", "window": window, "maxElements": 10 }),
    );
    let element = seen["result"]["elements"][0]["handle"]
        .as_str()
        .unwrap()
        .to_string();
    (window, element)
}

fn driver(desktop: &FakeDesktop) -> Driver<&FakeDesktop> {
    Driver::new(desktop, "n0nce".into(), "ab12".into())
}

const DONE: &str = r#"{ "role": "button", "label": "Done" }"#;

fn expect() -> Value {
    serde_json::from_str(DONE).unwrap()
}

fn shot_bounds() -> Value {
    rect_json(&WINDOW_BOUNDS)
}

#[test]
fn lists_windows_with_what_the_denylist_needs() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let windows = send(
        &mut driver,
        &mut Server::allowing(),
        1,
        json!({ "op": "listWindows" }),
    );
    let window = &windows["result"][0];
    assert_eq!(window["handle"], "wab12.1");
    assert_eq!(window["pid"], 42);
    assert_eq!(window["bounds"]["width"], 400.0);
    assert_eq!(
        window["appIdentifier"],
        "/System/Applications/Notes.app/Contents/MacOS/Notes"
    );
}

#[test]
fn a_window_keeps_its_handle_across_listings_and_loses_its_controls_when_it_closes() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let (window, element) = observed(&mut driver);
    let mut server = Server::allowing();
    let again = send(&mut driver, &mut server, 3, json!({ "op": "listWindows" }));
    assert_eq!(again["result"][0]["handle"], window.as_str());

    desktop.listed.set(false);
    send(&mut driver, &mut server, 4, json!({ "op": "listWindows" }));
    desktop.listed.set(true);
    let press = send(
        &mut driver,
        &mut server,
        5,
        json!({ "op": "press", "element": element, "expect": expect() }),
    );
    assert_eq!(press["error"]["kind"], "stale");
    assert!(server.asked.is_empty());
}

#[test]
fn refuses_unknown_and_restarted_handles() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let mut server = Server::allowing();
    let unknown = send(
        &mut driver,
        &mut server,
        1,
        json!({ "op": "observe", "window": "wab12.9", "maxElements": 10 }),
    );
    assert_eq!(unknown["error"]["kind"], "stale");
    assert!(unknown["error"].get("reason").is_none());
    let restarted = send(
        &mut driver,
        &mut server,
        2,
        json!({ "op": "press", "element": "effff.1", "expect": expect() }),
    );
    assert_eq!(restarted["error"]["reason"], "restarted");
}

#[test]
fn a_press_asks_the_server_twice_and_stays_in_the_background() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let (window, element) = observed(&mut driver);
    let mut server = Server::allowing();
    let reply = send(
        &mut driver,
        &mut server,
        3,
        json!({ "op": "press", "element": element, "expect": expect() }),
    );
    assert_eq!(reply["result"]["tookFocus"], false);
    assert_eq!(server.phases(), vec!["prepare", "dispatch"]);
    let asked = &server.asked[1];
    assert_eq!(asked["id"], 3);
    assert_eq!(asked["authorize"]["window"]["handle"], window.as_str());
    assert_eq!(asked["authorize"]["window"]["app"], "Notes");
    assert_eq!(asked["authorize"]["element"]["label"], "Done");
    assert!(asked["authorize"].get("foreground").is_none());
    assert_eq!(*desktop.calls.borrow(), vec!["press axtok1:done"]);
}

#[test]
fn a_refused_check_sends_nothing_and_carries_the_servers_code() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let (_, element) = observed(&mut driver);
    let mut server = Server::answering(vec![
        json!({ "code": "CU-CON-008", "message": "An approval is waiting." }),
    ]);
    let reply = send(
        &mut driver,
        &mut server,
        3,
        json!({ "op": "press", "element": element, "expect": expect() }),
    );
    assert_eq!(reply["error"]["kind"], "policy");
    assert_eq!(reply["error"]["code"], "CU-CON-008");
    assert_eq!(reply["error"]["dispatched"], "no");
    assert!(desktop.calls.borrow().is_empty());
}

#[test]
fn an_answer_with_the_wrong_nonce_is_never_permission() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let (_, element) = observed(&mut driver);
    let mut server = Server {
        nonce: "someone-else",
        ..Server::allowing()
    };
    let reply = send(
        &mut driver,
        &mut server,
        3,
        json!({ "op": "press", "element": element, "expect": expect() }),
    );
    assert_eq!(reply["error"]["kind"], "policy");
    assert!(desktop.calls.borrow().is_empty());
}

#[test]
fn a_control_whose_label_changed_is_stale_before_anything_is_asked() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let (_, element) = observed(&mut driver);
    *desktop.live_label.borrow_mut() = Some("Delete".into());
    let mut server = Server::allowing();
    let reply = send(
        &mut driver,
        &mut server,
        3,
        json!({ "op": "press", "element": element, "expect": expect() }),
    );
    assert_eq!(reply["error"]["kind"], "stale");
    assert!(server.asked.is_empty());
    assert!(desktop.calls.borrow().is_empty());
}

#[test]
fn a_control_without_a_native_identity_cannot_be_pressed_by_ref() {
    let desktop = FakeDesktop::new();
    desktop.with_identity.set(false);
    let mut driver = driver(&desktop);
    let (_, element) = observed(&mut driver);
    let mut server = Server::allowing();
    let reply = send(
        &mut driver,
        &mut server,
        3,
        json!({ "op": "press", "element": element, "expect": expect() }),
    );
    assert_eq!(reply["error"]["kind"], "failed");
    assert!(server.asked.is_empty());
}

#[test]
fn a_control_with_no_accessibility_press_is_clicked_after_fresh_checks() {
    let desktop = FakeDesktop::new();
    desktop.press.set(Some("unsupported"));
    let mut driver = driver(&desktop);
    let (_, element) = observed(&mut driver);
    let mut server = Server::allowing();
    let reply = send(
        &mut driver,
        &mut server,
        3,
        json!({ "op": "press", "element": element, "expect": expect() }),
    );
    assert_eq!(reply["result"]["tookFocus"], true);
    assert_eq!(
        server.phases(),
        vec!["prepare", "dispatch", "prepare", "dispatch"]
    );
    assert_eq!(server.asked[2]["authorize"]["foreground"], true);
    assert_eq!(
        *desktop.calls.borrow(),
        vec!["press axtok1:done", "focus_window", "click 150 80 x1"]
    );
}

#[test]
fn a_native_failure_after_the_server_allowed_it_may_have_landed() {
    let desktop = FakeDesktop::new();
    desktop.press.set(Some("os"));
    let mut driver = driver(&desktop);
    let (_, element) = observed(&mut driver);
    let reply = send(
        &mut driver,
        &mut Server::allowing(),
        3,
        json!({ "op": "press", "element": element, "expect": expect() }),
    );
    assert_eq!(reply["error"]["dispatched"], "unknown");
}

#[test]
fn typing_falls_back_to_keys_in_the_front_window() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let (_, element) = observed(&mut driver);
    let mut server = Server::allowing();
    let reply = send(
        &mut driver,
        &mut server,
        3,
        json!({ "op": "typeText", "element": element, "expect": expect(), "text": "milk" }),
    );
    assert_eq!(reply["result"]["tookFocus"], true);
    assert_eq!(
        *desktop.calls.borrow(),
        vec!["insert_text milk", "focus_window", "type_into milk false"]
    );
}

#[test]
fn a_click_needs_the_window_where_the_screenshot_saw_it() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let (window, _) = observed(&mut driver);
    let click = |x: f64, y: f64| {
        json!({
            "op": "click",
            "window": window,
            "expectBounds": shot_bounds(),
            "point": { "x": x, "y": y },
            "button": "left",
            "count": 2,
        })
    };

    let mut server = Server::allowing();
    let reply = send(&mut driver, &mut server, 3, click(150.0, 80.0));
    assert_eq!(reply["result"]["tookFocus"], true);
    assert_eq!(server.phases(), vec!["prepare", "dispatch"]);
    assert_eq!(server.asked[0]["authorize"]["foreground"], true);
    assert!(server.asked[0]["authorize"].get("point").is_none());
    assert_eq!(server.asked[1]["authorize"]["point"]["x"], 150.0);
    // The dispatch check names what the point hits, for approvals.
    assert_eq!(server.asked[1]["authorize"]["element"]["label"], "Done");
    assert_eq!(
        *desktop.calls.borrow(),
        vec!["focus_window", "click 150 80 x2"]
    );

    desktop.calls.borrow_mut().clear();
    let outside = send(&mut driver, &mut Server::allowing(), 4, click(900.0, 80.0));
    assert_eq!(
        outside["error"]["message"],
        "The point is outside the window."
    );

    desktop.bounds.set(Rect {
        x: 130.0,
        ..WINDOW_BOUNDS
    });
    let moved = send(&mut driver, &mut Server::allowing(), 5, click(150.0, 80.0));
    assert_eq!(moved["error"]["kind"], "stale");
    assert!(desktop.calls.borrow().is_empty());
}

#[test]
fn input_that_takes_the_screen_waits_while_the_user_is_active() {
    let desktop = FakeDesktop::new();
    desktop.idle.set(Some(0.2));
    let mut driver = driver(&desktop);
    let (window, element) = observed(&mut driver);
    let mut server = Server::allowing();
    let key = send(
        &mut driver,
        &mut server,
        3,
        json!({ "op": "key", "window": window, "keys": "cmd+s" }),
    );
    assert_eq!(key["error"]["code"], "CU-CON-009");
    assert_eq!(key["error"]["kind"], "policy");
    assert!(desktop.calls.borrow().is_empty());

    // A background press by ref does not take the screen, so it goes ahead.
    let press = send(
        &mut driver,
        &mut server,
        4,
        json!({ "op": "press", "element": element, "expect": expect() }),
    );
    assert_eq!(press["result"]["tookFocus"], false);
}

#[test]
fn a_window_that_will_not_come_forward_gets_no_input() {
    let desktop = FakeDesktop::new();
    desktop.focus_works.set(false);
    let mut driver = driver(&desktop);
    let (window, _) = observed(&mut driver);
    let mut server = Server::allowing();
    let reply = send(
        &mut driver,
        &mut server,
        3,
        json!({ "op": "typeFocused", "window": window, "text": "hello" }),
    );
    assert_eq!(
        reply["error"]["message"],
        "Could not bring the target window to the front."
    );
    assert_eq!(server.phases(), vec!["prepare"]);
    assert_eq!(*desktop.calls.borrow(), vec!["focus_window"]);
}

#[test]
fn keys_go_to_the_front_window_after_both_checks() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let (window, _) = observed(&mut driver);
    let mut server = Server::allowing();
    let reply = send(
        &mut driver,
        &mut server,
        3,
        json!({ "op": "key", "window": window, "keys": "cmd+shift+z" }),
    );
    assert_eq!(reply["result"]["tookFocus"], true);
    assert_eq!(server.phases(), vec!["prepare", "dispatch"]);
    assert_eq!(*desktop.calls.borrow(), vec!["focus_window", "key z 2"]);

    let unsupported = send(
        &mut driver,
        &mut server,
        4,
        json!({ "op": "key", "window": window, "keys": "f13" }),
    );
    assert_eq!(unsupported["error"]["message"], "Unsupported key chord.");
}

#[test]
fn focus_only_brings_the_window_forward() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let (window, _) = observed(&mut driver);
    let mut server = Server::allowing();
    let reply = send(
        &mut driver,
        &mut server,
        3,
        json!({ "op": "focus", "window": window }),
    );
    assert_eq!(reply["result"]["tookFocus"], true);
    assert_eq!(server.phases(), vec!["prepare"]);
    assert_eq!(*desktop.calls.borrow(), vec!["focus_window"]);
}

#[test]
fn translates_key_chords() {
    let combo = parse_chord("cmd+shift+cmd+backspace").unwrap();
    assert_eq!(combo.key, "delete");
    assert_eq!(combo.modifiers.len(), 2);
    assert_eq!(parse_chord("delete").unwrap().key, "forwarddelete");
    assert_eq!(parse_chord("enter").unwrap().key, "return");
    assert_eq!(parse_chord("f12").unwrap().key, "f12");
    assert!(parse_chord("f13").is_none());
    assert!(parse_chord("hyper+a").is_none());
    assert!(parse_chord("cmd+").is_none());
}

#[test]
fn names_the_control_under_a_point() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let (window, _) = observed(&mut driver);
    let mut server = Server::allowing();
    let hit = send(
        &mut driver,
        &mut server,
        3,
        json!({ "op": "elementAt", "window": window, "point": { "x": 150.0, "y": 80.0 } }),
    );
    assert_eq!(hit["result"]["label"], "Done");
    let miss = send(
        &mut driver,
        &mut server,
        4,
        json!({ "op": "elementAt", "window": window, "point": { "x": 5.0, "y": 5.0 } }),
    );
    assert!(miss["result"].is_null());
}

#[test]
fn screenshots_report_the_window_bounds_and_shrink_large_images() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let (window, _) = observed(&mut driver);
    let directory = std::env::temp_dir().join(format!("viewcode-driver-{}", std::process::id()));
    let output = directory.join("shot.png");
    let shoot = |max: u32| json!({ "op": "screenshot", "window": window, "outputPath": output, "maxSize": max });
    let mut server = Server::allowing();

    let full = send(&mut driver, &mut server, 3, shoot(1_000));
    assert_eq!(full["result"]["width"], 800);
    assert_eq!(full["result"]["height"], 600);
    assert_eq!(full["result"]["bounds"], shot_bounds());
    assert_eq!(std::fs::read(&output).unwrap(), png(800, 600));

    let small = send(&mut driver, &mut server, 4, shoot(400));
    assert_eq!(small["result"]["width"], 400);
    assert_eq!(small["result"]["height"], 300);
    assert_eq!(*desktop.calls.borrow(), vec!["shrink 400"]);
    assert!(!directory.join("shot.window.png").exists());

    // An image that is not the window alone would send clicks to the wrong place.
    desktop.image.set((800, 200));
    let wrong = send(&mut driver, &mut server, 5, shoot(1_000));
    assert_eq!(wrong["error"]["kind"], "failed");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn rejects_a_wrong_nonce_and_ignores_garbage() {
    let desktop = FakeDesktop::new();
    let mut driver = driver(&desktop);
    let mut server = Server::allowing();
    let line = json!({ "id": 1, "nonce": "other", "request": { "op": "status" } }).to_string();
    assert!(matches!(
        driver.handle_line(&line, &mut server),
        Some(Incoming::Reject)
    ));
    assert!(driver.handle_line("not json", &mut server).is_none());
    let unknown = send(&mut driver, &mut server, 2, json!({ "op": "teleport" }));
    assert_eq!(unknown["error"]["kind"], "malformed");
}
