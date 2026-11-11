//! The helper's JSON lines, replayed through the protocol types and the gate's decisions
//! without a process. The shapes are the ones `native/computer-driver` reads and writes
//! (`src/driver.rs`, `src/driver_tests.rs`) and ViewCode's client decodes.

use super::policy::*;
use super::protocol::*;
use super::targets::NewShot;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::collections::HashSet;

const NONCE: &str = "n0nce";

/// One session with the helper. `>` is a line the gate writes, `<` one the helper writes.
const RECORDED: &str = r#"
> {"id":1,"nonce":"n0nce","request":{"op":"status"}}
< {"id":1,"ok":true,"result":{"available":true,"accessibility":"granted"}}
> {"id":2,"nonce":"n0nce","request":{"op":"listWindows"}}
< {"id":2,"ok":true,"result":[{"handle":"wab12.1","app":"Notes","pid":70,"title":"Shopping","focused":true,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0},"appIdentifier":"/System/Applications/Notes.app/Contents/MacOS/Notes"},{"handle":"wab12.2","app":"1Password","pid":71,"title":"Vault","focused":false,"bounds":{"x":1000.0,"y":50.0,"width":400.0,"height":300.0},"appIdentifier":"/Applications/1Password.app/Contents/MacOS/1Password"}]}
> {"id":3,"nonce":"n0nce","request":{"op":"observe","window":"wab12.1","maxElements":300}}
< {"id":3,"ok":true,"result":{"elements":[{"handle":"eab12.3","role":"button","label":"Done","enabled":true,"focused":false},{"handle":"eab12.4","role":"text field","label":"Title","value":"Milk","section":"Lists","enabled":true,"focused":true}],"truncated":false}}
> {"id":4,"nonce":"n0nce","request":{"op":"press","element":"eab12.3","expect":{"role":"button","label":"Done"}}}
< {"id":4,"authorize":{"window":{"handle":"wab12.1","app":"Notes","pid":70,"title":"Shopping","focused":true,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0},"appIdentifier":"/System/Applications/Notes.app/Contents/MacOS/Notes"},"element":{"role":"button","label":"Done"}},"phase":"prepare"}
> {"id":4,"nonce":"n0nce","decision":null}
< {"id":4,"authorize":{"window":{"handle":"wab12.1","app":"Notes","pid":70,"title":"Shopping","focused":true,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0},"appIdentifier":"/System/Applications/Notes.app/Contents/MacOS/Notes"},"element":{"role":"button","label":"Done"}},"phase":"dispatch"}
> {"id":4,"nonce":"n0nce","decision":null}
< {"id":4,"ok":true,"result":{"tookFocus":false}}
> {"id":5,"nonce":"n0nce","request":{"op":"typeText","element":"eab12.4","expect":{"role":"text field","label":"Title"},"text":"Eggs"}}
< {"id":5,"authorize":{"window":{"handle":"wab12.1","app":"Notes","pid":70,"title":"Shopping","focused":true,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0},"appIdentifier":"/System/Applications/Notes.app/Contents/MacOS/Notes"},"element":{"role":"text field","label":"Title"}},"phase":"prepare"}
> {"id":5,"nonce":"n0nce","decision":{"code":"CU-CON-008","message":"Input is paused while an approval waits for the user; wait for it to be answered, then observe again.","effect":"not-dispatched"}}
< {"id":5,"ok":false,"error":{"kind":"policy","message":"Input is paused while an approval waits for the user; wait for it to be answered, then observe again.","dispatched":"no","code":"CU-CON-008"}}
> {"id":6,"nonce":"n0nce","request":{"op":"listWindows"}}
< {"id":6,"ok":true,"result":[{"handle":"wab12.1","app":"Notes","pid":70,"title":"Shopping","focused":true,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0},"appIdentifier":"/System/Applications/Notes.app/Contents/MacOS/Notes"},{"handle":"wab12.2","app":"1Password","pid":71,"title":"Vault","focused":false,"bounds":{"x":1000.0,"y":50.0,"width":400.0,"height":300.0},"appIdentifier":"/Applications/1Password.app/Contents/MacOS/1Password"}]}
> {"id":7,"nonce":"n0nce","request":{"op":"screenshot","window":"wab12.1","outputPath":"shots/1.png","maxSize":1568}}
< {"id":7,"ok":true,"result":{"width":400,"height":300,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0}}}
> {"id":8,"nonce":"n0nce","request":{"op":"elementAt","window":"wab12.1","point":{"x":121.0,"y":71.0}}}
< {"id":8,"ok":true,"result":null}
> {"id":9,"nonce":"n0nce","request":{"op":"click","window":"wab12.1","expectBounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0},"point":{"x":121.0,"y":71.0},"button":"left","count":1}}
< {"id":9,"authorize":{"window":{"handle":"wab12.1","app":"Notes","pid":70,"title":"Shopping","focused":true,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0},"appIdentifier":"/System/Applications/Notes.app/Contents/MacOS/Notes"},"foreground":true},"phase":"prepare"}
> {"id":9,"nonce":"n0nce","decision":null}
< {"id":9,"authorize":{"window":{"handle":"wab12.1","app":"Notes","pid":70,"title":"Shopping","focused":true,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0},"appIdentifier":"/System/Applications/Notes.app/Contents/MacOS/Notes"},"element":{"role":"button","label":"Delete list"},"point":{"x":121.0,"y":71.0}},"phase":"dispatch"}
> {"id":9,"nonce":"n0nce","decision":{"code":"CU-CON-007","message":"The target now looks destructive; look again and request approval.","effect":"not-dispatched"}}
< {"id":9,"ok":false,"error":{"kind":"policy","message":"The target now looks destructive; look again and request approval.","dispatched":"no","code":"CU-CON-007"}}
> {"id":10,"nonce":"n0nce","request":{"op":"key","window":"wab12.1","keys":"cmd+s"}}
< {"id":10,"authorize":{"window":{"handle":"wab12.1","app":"Notes","pid":70,"title":"Shopping","focused":true,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0},"appIdentifier":"/System/Applications/Notes.app/Contents/MacOS/Notes"},"foreground":true},"phase":"prepare"}
> {"id":10,"nonce":"n0nce","decision":null}
< {"id":10,"authorize":{"window":{"handle":"wab12.1","app":"Notes","pid":70,"title":"Shopping","focused":true,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0},"appIdentifier":"/System/Applications/Notes.app/Contents/MacOS/Notes"}},"phase":"dispatch"}
> {"id":10,"nonce":"n0nce","decision":null}
< {"id":11,"ok":true,"result":{"tookFocus":true}}
> {"id":11,"nonce":"n0nce","request":{"op":"focus","window":"wab12.1"}}
< {"id":11,"ok":false,"error":{"kind":"stale","message":"The computer-use driver restarted; list windows again.","dispatched":"no","reason":"restarted"}}
"#;

/// Walks the recording in order: a `>` line must be exactly what the gate builds, a `<`
/// line is read the way the client reads it.
struct Replay {
    lines: std::vec::IntoIter<(char, &'static str)>,
    next_id: u64,
}

impl Replay {
    fn new() -> Self {
        let lines: Vec<_> = RECORDED.lines().filter(|line| !line.is_empty()).map(|line| (line.chars().next().unwrap(), &line[2..])).collect();
        Self { lines: lines.into_iter(), next_id: 1 }
    }

    fn written(&mut self, built: &str) {
        let (direction, recorded) = self.lines.next().expect("a recorded line");
        assert_eq!(direction, '>', "{recorded}");
        assert_eq!(serde_json::from_str::<Value>(built).unwrap(), serde_json::from_str::<Value>(recorded).unwrap());
    }

    fn read(&mut self, id: u64) -> DriverLine {
        let (direction, recorded) = self.lines.next().expect("a recorded line");
        assert_eq!(direction, '<', "{recorded}");
        read_line(recorded, id)
    }

    /// Sends a request and returns its id. The recorded line must also read back as the
    /// same request, the way the helper's own decoder would see it.
    fn send(&mut self, request: DriverRequest) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let line = RequestLine { id, nonce: NONCE.into(), request };
        self.written(&line.to_line());
        assert_eq!(serde_json::from_str::<RequestLine>(&line.to_line()).unwrap(), line);
        id
    }

    fn reply<T: DeserializeOwned>(&mut self, id: u64, input: bool, authorized: bool) -> Result<T, DriverFailure> {
        match self.read(id) {
            DriverLine::Reply(result) => decode_result(result?, input, authorized),
            DriverLine::Malformed => Err(CallOutcome::Malformed.failure(input, authorized)),
            DriverLine::Authorize { .. } => panic!("expected a reply"),
        }
    }

    /// Reads an authorize check, decides it and writes the decision.
    fn authorize(&mut self, id: u64, expected: Phase, decide: impl FnOnce(&DispatchTarget) -> Result<(), ComputerUseError>) -> Result<(), ComputerUseError> {
        let DriverLine::Authorize { target, phase } = self.read(id) else { panic!("expected an authorize check") };
        assert_eq!(phase, expected);
        let decision = decide(&target);
        self.written(&DecisionLine::new(id, NONCE, decision.clone()).to_line());
        decision
    }
}

#[test]
fn a_recorded_session_replays_through_the_protocol_and_the_gate() {
    let mut replay = Replay::new();
    let own = HashSet::from([4242]);
    let turn = TurnContext { title: "Tidy notes".into(), turn_id: "turn-1".into(), full_access: true };
    let mut pending = PendingApprovals::default();
    let mut gate = ThreadGate::starting_at(100);
    let live = |pending: &PendingApprovals| Live { mode: Mode::Control, approvals: Approvals::Thread, screen: Screen::Allow, turn: Some(&turn), approval_pending: pending.any_pending(Some(false)), own_pids: &own, background_input: false };

    // Status, a listing and an observation: nothing asks, and the agent sees small numbers.
    let id = replay.send(DriverRequest::Status);
    assert_eq!(replay.reply::<DriverStatus>(id, false, false), Ok(DriverStatus { available: true, accessibility: Accessibility::Granted, reason: None }));
    let id = replay.send(DriverRequest::ListWindows);
    let listed: Vec<DriverWindow> = replay.reply(id, false, false).unwrap();
    let records = gate.targets.record_windows(&listed);
    assert_eq!(records.iter().map(|window| (window.id, window.app.as_str())).collect::<Vec<_>>(), [(100, "Notes"), (101, "1Password")]);
    // The password manager is listed, and that is all an agent may do with it.
    assert_eq!(observable_window(&gate.targets, 101, &own).unwrap_err().code, ErrorCode::ProtectedApp);
    let notes = observable_window(&gate.targets, 100, &own).unwrap();
    let id = replay.send(DriverRequest::Observe { window: notes.handle.clone(), max_elements: MAX_OBSERVED_ELEMENTS, query: None });
    let observation: Observation = replay.reply(id, false, false).unwrap();
    assert_eq!(observation.elements[1].value.as_deref(), Some("Milk"));
    assert_eq!(gate.targets.record_observation(notes.id, &observation.elements), [100, 101]);

    // A press by ref in a Full access thread: allowed without asking, checked at the
    // request and then for the helper's own target, before it prepares and before input.
    let Request::Input(press) = admit(Mode::Control, &json!({ "command": "press", "ref": 100 })).unwrap() else { panic!("input") };
    let (resolved, step) = begin_input(&live(&pending), &gate, &press).unwrap();
    assert_eq!(step, ScreenStep::Proceed);
    let decision = decide_input(Approvals::Thread, &gate, &turn, &press, &resolved, &Hit::NotApplicable);
    assert_eq!(decision, InputDecision::Allow { destructive: false });
    let input = CheckedInput { request: &press, turn_id: "turn-1", resolved: &resolved, asked: false, destructive: false };
    assert_eq!(final_check(&live(&pending), &gate, &input, None), Ok(()));
    let id = replay.send(driver_request(&press, &resolved).unwrap());
    for phase in [Phase::Prepare, Phase::Dispatch] {
        assert_eq!(replay.authorize(id, phase, |native| final_check(&live(&pending), &gate, &input, Some(native))), Ok(()));
    }
    assert_eq!(replay.reply::<InputResult>(id, true, true), Ok(InputResult { took_focus: false }));

    // Typing by ref starts the same way, but another thread's provider asks for approval
    // before the helper's first check: the input is refused and the helper sends nothing.
    let Request::Input(typing) = admit(Mode::Control, &json!({ "command": "type", "ref": 101, "text": "Eggs" })).unwrap() else { panic!("input") };
    let (resolved, _) = begin_input(&live(&pending), &gate, &typing).unwrap();
    let input = CheckedInput { request: &typing, turn_id: "turn-1", resolved: &resolved, asked: false, destructive: false };
    let id = replay.send(driver_request(&typing, &resolved).unwrap());
    pending.track_provider("thread-b", "req-7", Some("turn-9"), true, false);
    let refused = replay.authorize(id, Phase::Prepare, |native| final_check(&live(&pending), &gate, &input, Some(native))).unwrap_err();
    assert_eq!(refused.code, ErrorCode::ApprovalPending);
    let failure = replay.reply::<InputResult>(id, true, false).unwrap_err();
    assert_eq!((failure.kind, failure.dispatched, failure.code), (FailureKind::Policy, Dispatched::No, Some(ErrorCode::ApprovalPending)));
    // The agent reads the gate's own refusal, carried back through the helper.
    assert_eq!(input_failure(&mut gate.targets, &resolved, &failure), refused);
    assert_eq!(begin_input(&live(&pending), &gate, &typing).unwrap_err(), refused);
    pending.track_provider("thread-b", "req-7", Some("turn-9"), false, false);

    // A screenshot: refused while a protected window overlaps; here 1Password sits beside Notes.
    let id = replay.send(DriverRequest::ListWindows);
    let listed: Vec<DriverWindow> = replay.reply(id, false, false).unwrap();
    gate.targets.record_windows(&listed);
    assert!(!protected_overlay(&listed, &notes.handle, &own));
    let id = replay.send(DriverRequest::Screenshot { window: notes.handle.clone(), output_path: "shots/1.png".into(), max_size: DEFAULT_SCREENSHOT_SIZE });
    let image: Screenshot = replay.reply(id, false, false).unwrap();
    let shot = gate.targets.record_shot(notes.id, NewShot { handle: notes.handle.clone(), width: image.width, height: image.height, bounds: image.bounds, max_size: DEFAULT_SCREENSHOT_SIZE });

    // A click in that screenshot. Nothing labelled was under the point when the gate
    // looked, so it did not ask; at dispatch the helper names a destructive control there.
    let Request::Input(click) = admit(Mode::Control, &json!({ "command": "click", "shot": shot.shot, "x": 10, "y": 10 })).unwrap() else { panic!("input") };
    let (resolved, _) = begin_input(&live(&pending), &gate, &click).unwrap();
    let id = replay.send(DriverRequest::ElementAt { window: shot.handle.clone(), point: hit_point(&click, &resolved).unwrap() });
    let hit = match replay.reply::<Option<ElementIdentity>>(id, false, false) {
        Ok(Some(element)) => Hit::Element(element),
        _ => Hit::Nothing,
    };
    assert_eq!(hit, Hit::Nothing);
    assert_eq!(decide_input(Approvals::Thread, &gate, &turn, &click, &resolved, &hit), InputDecision::Allow { destructive: false });
    let input = CheckedInput { request: &click, turn_id: "turn-1", resolved: &resolved, asked: false, destructive: false };
    let id = replay.send(driver_request(&click, &resolved).unwrap());
    assert_eq!(replay.authorize(id, Phase::Prepare, |native| final_check(&live(&pending), &gate, &input, Some(native))), Ok(()));
    let refused = replay.authorize(id, Phase::Dispatch, |native| final_check(&live(&pending), &gate, &input, Some(native))).unwrap_err();
    assert_eq!(refused.code, ErrorCode::StaleShot);
    let failure = replay.reply::<InputResult>(id, true, false).unwrap_err();
    assert_eq!(input_failure(&mut gate.targets, &resolved, &failure), refused);

    // A shortcut the helper was allowed to send, then a line for another request: the
    // stream is out of step, and the input may have landed.
    let Request::Input(key) = admit(Mode::Control, &json!({ "command": "key", "window": 100, "keys": "cmd+s" })).unwrap() else { panic!("input") };
    let (resolved, _) = begin_input(&live(&pending), &gate, &key).unwrap();
    let input = CheckedInput { request: &key, turn_id: "turn-1", resolved: &resolved, asked: false, destructive: false };
    let id = replay.send(driver_request(&key, &resolved).unwrap());
    for phase in [Phase::Prepare, Phase::Dispatch] {
        assert_eq!(replay.authorize(id, phase, |native| final_check(&live(&pending), &gate, &input, Some(native))), Ok(()));
    }
    let failure = replay.reply::<InputResult>(id, true, true).unwrap_err();
    assert_eq!((failure.kind, failure.dispatched), (FailureKind::Malformed, Dispatched::Unknown));
    let reported = input_failure(&mut gate.targets, &resolved, &failure);
    assert_eq!((reported.code, reported.effect), (ErrorCode::DriverMalformed, Some(Effect::DispatchedUnknown)));

    // The replacement helper knows none of the old handles: the window is retired and
    // the agent is told to list again.
    let Request::Input(focus) = admit(Mode::Control, &json!({ "command": "focus", "window": 100 })).unwrap() else { panic!("input") };
    let (resolved, _) = begin_input(&live(&pending), &gate, &focus).unwrap();
    let id = replay.send(driver_request(&focus, &resolved).unwrap());
    let failure = replay.reply::<InputResult>(id, true, false).unwrap_err();
    assert_eq!(failure.reason, Some(FailureReason::Restarted));
    let reported = input_failure(&mut gate.targets, &resolved, &failure);
    assert_eq!((reported.code, reported.message.as_str(), reported.effect), (ErrorCode::UnknownWindow, "The computer-use driver restarted; run `viewcode-computer list-windows` again.", Some(Effect::NotDispatched)));
    assert_eq!(begin_input(&live(&pending), &gate, &focus).unwrap_err().code, ErrorCode::UnknownWindow);
    assert!(replay.lines.next().is_none(), "every recorded line was replayed");
}

#[test]
fn every_request_has_the_shape_the_helper_reads() {
    let expect = || ElementIdentity { role: "button".into(), label: "Done".into() };
    let bounds = Rect { x: 100.0, y: 50.0, width: 400.0, height: 300.0 };
    let point = Point { x: 150.0, y: 80.0 };
    // The request shapes of `native/computer-driver/src/driver_tests.rs` and ViewCode's `DriverRequest`.
    let cases = [
        (DriverRequest::Status, json!({ "op": "status" }), false, 10_000),
        (DriverRequest::ListWindows, json!({ "op": "listWindows" }), false, 10_000),
        (DriverRequest::ReleaseMouse, json!({ "op": "releaseMouse" }), false, 5_000),
        (DriverRequest::Observe { window: "wab12.1".into(), max_elements: 10, query: None }, json!({ "op": "observe", "window": "wab12.1", "maxElements": 10 }), false, 15_000),
        (DriverRequest::Observe { window: "wab12.1".into(), max_elements: 10, query: Some("save".into()) }, json!({ "op": "observe", "window": "wab12.1", "maxElements": 10, "query": "save" }), false, 15_000),
        (DriverRequest::Screenshot { window: "wab12.1".into(), output_path: "out.png".into(), max_size: 1568 }, json!({ "op": "screenshot", "window": "wab12.1", "outputPath": "out.png", "maxSize": 1568 }), false, 30_000),
        (DriverRequest::ElementAt { window: "wab12.1".into(), point }, json!({ "op": "elementAt", "window": "wab12.1", "point": { "x": 150.0, "y": 80.0 } }), false, 8_000),
        (DriverRequest::Press { element: "eab12.2".into(), expect: expect() }, json!({ "op": "press", "element": "eab12.2", "expect": { "role": "button", "label": "Done" } }), true, 20_000),
        (DriverRequest::SetValue { element: "eab12.2".into(), expect: expect(), value: "v".into() }, json!({ "op": "setValue", "element": "eab12.2", "expect": { "role": "button", "label": "Done" }, "value": "v" }), true, 30_000),
        (DriverRequest::TypeText { element: "eab12.2".into(), expect: expect(), text: "milk".into() }, json!({ "op": "typeText", "element": "eab12.2", "expect": { "role": "button", "label": "Done" }, "text": "milk" }), true, 30_000),
        (DriverRequest::Scroll { element: "eab12.2".into(), expect: expect(), dx: 0, dy: -3 }, json!({ "op": "scroll", "element": "eab12.2", "expect": { "role": "button", "label": "Done" }, "dx": 0, "dy": -3 }), true, 20_000),
        (DriverRequest::Key { window: "wab12.1".into(), keys: "cmd+s".into() }, json!({ "op": "key", "window": "wab12.1", "keys": "cmd+s" }), true, 20_000),
        (DriverRequest::TypeFocused { window: "wab12.1".into(), text: "hello".into() }, json!({ "op": "typeFocused", "window": "wab12.1", "text": "hello" }), true, 30_000),
        (DriverRequest::Focus { window: "wab12.1".into() }, json!({ "op": "focus", "window": "wab12.1" }), true, 20_000),
        (DriverRequest::Click { window: "wab12.1".into(), expect_bounds: bounds, point, button: MouseButton::Left, count: 2 }, json!({ "op": "click", "window": "wab12.1", "expectBounds": { "x": 100.0, "y": 50.0, "width": 400.0, "height": 300.0 }, "point": { "x": 150.0, "y": 80.0 }, "button": "left", "count": 2 }), true, 20_000),
        (DriverRequest::Drag { window: "wab12.1".into(), expect_bounds: bounds, from: point, to: Point { x: 160.5, y: 90.5 } }, json!({ "op": "drag", "window": "wab12.1", "expectBounds": { "x": 100.0, "y": 50.0, "width": 400.0, "height": 300.0 }, "from": { "x": 150.0, "y": 80.0 }, "to": { "x": 160.5, "y": 90.5 } }), true, 20_000),
        (DriverRequest::Move { window: "wab12.1".into(), expect_bounds: bounds, point }, json!({ "op": "move", "window": "wab12.1", "expectBounds": { "x": 100.0, "y": 50.0, "width": 400.0, "height": 300.0 }, "point": { "x": 150.0, "y": 80.0 } }), true, 20_000),
        (DriverRequest::ScrollAt { window: "wab12.1".into(), expect_bounds: bounds, point, dx: 1, dy: 2 }, json!({ "op": "scrollAt", "window": "wab12.1", "expectBounds": { "x": 100.0, "y": 50.0, "width": 400.0, "height": 300.0 }, "point": { "x": 150.0, "y": 80.0 }, "dx": 1, "dy": 2 }), true, 20_000),
    ];
    for (request, wire, input, timeout_ms) in cases {
        assert_eq!(serde_json::to_value(&request).unwrap(), wire);
        assert_eq!(serde_json::from_value::<DriverRequest>(wire.clone()).unwrap(), request);
        assert_eq!(request.op(), wire["op"]);
        assert_eq!((request.is_input(), request.timeout_ms()), (input, timeout_ms), "{}", request.op());
        assert_eq!(request.holds_mouse_button(), matches!(request.op(), "click" | "drag"));
        // The envelope the helper's loop reads: id, nonce, request.
        let line = RequestLine { id: 3, nonce: NONCE.into(), request };
        assert_eq!(serde_json::from_str::<Value>(&line.to_line()).unwrap(), json!({ "id": 3, "nonce": NONCE, "request": wire }));
    }
    assert!(serde_json::from_value::<DriverRequest>(json!({ "op": "teleport" })).is_err());
    let typed = RequestLine { id: 1, nonce: NONCE.into(), request: DriverRequest::TypeFocused { window: "wab12.1".into(), text: "hunter2".into() } };
    assert_eq!(format!("{:?}", typed.request), "DriverRequest(typeFocused)");
    assert!(!format!("{typed:?}").contains("hunter2"));
    assert_eq!((DRIVER_NONCE_ENV, DRIVER_EPOCH_ENV, DEFAULT_RECYCLE_AFTER), ("VIEWCODE_COMPUTER_DRIVER_NONCE", "VIEWCODE_COMPUTER_DRIVER_EPOCH", 500));
}

#[test]
fn replies_decode_or_are_malformed_never_a_partial_success() {
    let reply = |line: Value| read_line(&line.to_string(), 7);
    let result = |line: Value| match reply(line) {
        DriverLine::Reply(Ok(result)) => result,
        other => panic!("{other:?}"),
    };
    // Each reply shape the helper writes.
    assert_eq!(decode_result::<DriverStatus>(result(json!({ "id": 7, "ok": true, "result": { "available": true, "accessibility": "denied", "reason": "Grant Accessibility." } })), false, false), Ok(DriverStatus { available: true, accessibility: Accessibility::Denied, reason: Some("Grant Accessibility.".into()) }));
    let windows: Vec<DriverWindow> = decode_result(result(json!({ "id": 7, "ok": true, "result": [{ "handle": "w1", "app": "Notes", "pid": 1, "title": "t", "focused": true }, { "handle": "w2", "app": "Notes", "pid": 1, "title": "File", "focused": false, "kind": "menu-bar" }], "diagnostics": { "windowEnumerationFailures": 2 } })), false, false).unwrap();
    assert_eq!((windows[0].bounds, windows[0].app_identifier.as_deref(), windows[1].kind), (None, None, Some(WindowKind::MenuBar)));
    assert_eq!(serde_json::to_value(&windows[0]).unwrap(), json!({ "handle": "w1", "app": "Notes", "pid": 1, "title": "t", "focused": true }));
    assert_eq!(decode_result::<Option<ElementIdentity>>(result(json!({ "id": 7, "ok": true, "result": null })), false, false), Ok(None));
    assert_eq!(decode_result::<Option<ElementIdentity>>(result(json!({ "id": 7, "ok": true, "result": { "role": "button", "label": "Done" } })), false, false), Ok(Some(ElementIdentity { role: "button".into(), label: "Done".into() })));
    assert_eq!(decode_result::<()>(result(json!({ "id": 7, "ok": true, "result": null })), false, false), Ok(()));
    assert_eq!(decode_result::<InputResult>(result(json!({ "id": 7, "ok": true, "result": { "tookFocus": true } })), true, true), Ok(InputResult { took_focus: true }));

    // A refusal passes through, with what the helper said about dispatch and why.
    let refusal = |error: Value| match reply(json!({ "id": 7, "ok": false, "error": error })) {
        DriverLine::Reply(Err(failure)) => failure,
        other => panic!("{other:?}"),
    };
    assert_eq!(refusal(json!({ "kind": "stale", "message": "changed", "dispatched": "no" })), DriverFailure::new(FailureKind::Stale, "changed", Dispatched::No));
    assert_eq!(refusal(json!({ "kind": "stale", "message": "restarted", "dispatched": "no", "reason": "restarted" })).reason, Some(FailureReason::Restarted));
    assert_eq!(refusal(json!({ "kind": "policy", "message": "An approval is waiting.", "dispatched": "no", "code": "CU-CON-008" })).code, Some(ErrorCode::ApprovalPending));
    assert_eq!(refusal(json!({ "kind": "failed", "message": "The target window lost focus before the input.", "dispatched": "unknown" })).dispatched, Dispatched::Unknown);
    for kind in ["policy", "unavailable", "permission-accessibility", "permission-screen", "stale", "failed", "timeout", "malformed"] {
        let failure = refusal(json!({ "kind": kind, "message": "m", "dispatched": "yes" }));
        assert_eq!(serde_json::to_value(&failure).unwrap(), json!({ "kind": kind, "message": "m", "dispatched": "yes" }));
    }

    // A reply of the wrong shape or for another request is malformed.
    assert_eq!(decode_result::<Vec<DriverWindow>>(result(json!({ "id": 7, "ok": true, "result": [{ "handle": 7 }] })), false, false).unwrap_err().kind, FailureKind::Malformed);
    assert_eq!(decode_result::<Screenshot>(result(json!({ "id": 7, "ok": true, "result": { "width": 0, "height": 300, "bounds": { "x": 0.0, "y": 0.0, "width": 1.0, "height": 1.0 } } })), false, false).unwrap_err().kind, FailureKind::Malformed);
    for line in [json!({ "id": 8, "ok": true, "result": { "elements": [], "truncated": false } }), json!({ "id": 7, "ok": true }), json!({ "id": 7, "ok": false, "error": { "kind": "exploded", "message": "m", "dispatched": "no" } }), json!({ "id": 7, "ok": false, "error": { "kind": "policy", "message": "m", "dispatched": "no", "code": "CU-XXX-999" } }), json!({ "id": 7, "result": 1 }), json!({ "id": 7, "authorize": { "window": "w1" }, "phase": "dispatch" }), json!([7]), json!({ "id": "7", "ok": true, "result": null })] {
        assert_eq!(reply(line.clone()), DriverLine::Malformed, "{line}");
    }
    assert_eq!(read_line("not json", 7), DriverLine::Malformed);
    assert_eq!(read_line("", 7), DriverLine::Malformed);
}

#[test]
fn a_broken_call_reports_only_what_can_have_been_delivered() {
    let failure = |outcome: CallOutcome, input, authorized| {
        let failure = outcome.failure(input, authorized);
        (failure.kind, failure.message, failure.dispatched)
    };
    // Before an allowed dispatch nothing was sent; after one the input may have landed.
    assert_eq!(failure(CallOutcome::Timeout, true, true), (FailureKind::Timeout, "The computer-use driver timed out.".into(), Dispatched::Unknown));
    assert_eq!(failure(CallOutcome::Timeout, true, false).2, Dispatched::No);
    assert_eq!(failure(CallOutcome::Exited, true, true), (FailureKind::Failed, "The computer-use driver exited.".into(), Dispatched::Unknown));
    assert_eq!(failure(CallOutcome::Exited, false, true).2, Dispatched::No);
    assert_eq!(failure(CallOutcome::Malformed, true, true), (FailureKind::Malformed, "The computer-use driver answered with an invalid reply.".into(), Dispatched::Unknown));
    assert_eq!(failure(CallOutcome::NotSent, true, true), (FailureKind::Unavailable, "The computer-use driver could not start.".into(), Dispatched::No));
    assert_eq!(DriverStatus::unavailable("why"), DriverStatus { available: false, accessibility: Accessibility::Unknown, reason: Some("why".into()) });
    assert!(platform_supported("macos") && !platform_supported("linux") && !platform_supported("windows"));
    assert_eq!([unsupported_reason("windows"), unsupported_reason("linux"), unsupported_reason("freebsd")], ["Computer use is not supported on Windows yet.", "Computer use is not supported on Linux yet.", "Computer use is not supported on this platform."]);
}

#[test]
fn the_authorize_handshake_matches_the_helper() {
    // What the helper writes for a press (`a_press_asks_the_server_twice_and_stays_in_the_background`).
    let asked = json!({ "id": 3, "authorize": { "window": { "handle": "wab12.1", "app": "Notes", "pid": 70, "title": "Shopping", "focused": true, "bounds": { "x": 100.0, "y": 50.0, "width": 400.0, "height": 300.0 } }, "element": { "role": "button", "label": "Done" } }, "phase": "dispatch" });
    let DriverLine::Authorize { target, phase } = read_line(&asked.to_string(), 3) else { panic!("an authorize check") };
    assert_eq!(phase, Phase::Dispatch);
    assert_eq!((target.window.handle.as_str(), target.window.app.as_str(), target.element.as_ref().map(|element| element.label.as_str()), target.point, target.foreground), ("wab12.1", "Notes", Some("Done"), None, false));
    assert_eq!(serde_json::to_value(&target).unwrap(), asked["authorize"]);
    // A coordinate check: the screen first, then the point and what it hits.
    let prepare = json!({ "id": 3, "authorize": { "window": { "handle": "wab12.1", "app": "Notes", "pid": 70, "title": "Shopping", "focused": false }, "foreground": true }, "phase": "prepare" });
    let DriverLine::Authorize { target, phase } = read_line(&prepare.to_string(), 3) else { panic!("an authorize check") };
    assert_eq!((phase, target.foreground, target.point, target.element.clone()), (Phase::Prepare, true, None, None));
    assert_eq!(serde_json::to_value(&target).unwrap(), prepare["authorize"]);
    // Anything but an explicit prepare is the stricter final check; another request's check is out of step.
    let mut unnamed = prepare.clone();
    unnamed.as_object_mut().unwrap().remove("phase");
    assert!(matches!(read_line(&unnamed.to_string(), 3), DriverLine::Authorize { phase: Phase::Dispatch, .. }));
    assert_eq!(read_line(&prepare.to_string(), 4), DriverLine::Malformed);
    assert_eq!(serde_json::to_value([Phase::Prepare, Phase::Dispatch]).unwrap(), json!(["prepare", "dispatch"]));

    // The answer the helper waits for: its id, the nonce, and a decision that is always present.
    let allowed = DecisionLine::new(3, NONCE, Ok(()));
    assert_eq!(allowed.to_line(), r#"{"id":3,"nonce":"n0nce","decision":null}"#);
    let refused = DecisionLine::new(3, NONCE, Err(input_paused()));
    assert_eq!(serde_json::from_str::<Value>(&refused.to_line()).unwrap(), json!({ "id": 3, "nonce": NONCE, "decision": { "code": "CU-CON-008", "message": "Input is paused while an approval waits for the user; wait for it to be answered, then observe again.", "effect": "not-dispatched" } }));
    assert_eq!(serde_json::from_str::<DecisionLine>(&refused.to_line()).unwrap(), refused);
    assert_eq!(serde_json::from_str::<DecisionLine>(&allowed.to_line()).unwrap(), allowed);
    // Every code is written the way the helper copies it into its refusal.
    let codes = ["CU-VAL-001", "CU-VAL-002", "CU-VAL-003", "CU-VAL-004", "CU-NOT-001", "CU-NOT-002", "CU-NOT-003", "CU-CON-001", "CU-CON-002", "CU-CON-003", "CU-CON-004", "CU-CON-005", "CU-CON-006", "CU-CON-007", "CU-CON-008", "CU-CON-009", "CU-EXT-001", "CU-EXT-002", "CU-EXT-003", "CU-EXT-004", "CU-EXT-005", "CU-EXT-006", "CU-INT-001"];
    let parsed: Vec<ErrorCode> = serde_json::from_value(json!(codes)).unwrap();
    assert_eq!(parsed.iter().collect::<HashSet<_>>().len(), codes.len());
    assert_eq!(serde_json::to_value(&parsed).unwrap(), json!(codes));
    assert_eq!(serde_json::to_value([Effect::Dispatched, Effect::NotDispatched, Effect::DispatchedUnknown]).unwrap(), json!(["dispatched", "not-dispatched", "dispatched-unknown"]));
}
