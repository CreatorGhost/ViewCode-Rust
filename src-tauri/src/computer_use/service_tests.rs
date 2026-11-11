//! The driver client and the service against a fake helper: a shell script that speaks
//! the helper's JSON lines over stdin and stdout, started as a real child process. The
//! script checks the nonce, mints handles with its epoch the way the helper does, asks
//! the gate before it prepares and before it "dispatches", and records what it did.

use super::driver::{Authorize, DriverClient, DriverOptions, Launch};
use super::policy::{ApprovalAnswer, ApprovalResponse, Approvals, ErrorCode, Effect, Mode, Screen, TurnContext, cancelled_before_dispatch};
use super::protocol::{DispatchTarget, Dispatched, DriverRequest, DriverWindow, ElementIdentity, FailureKind, InputResult, Phase};
use super::service::{Caller, Host, PendingApproval, Service, ServiceOptions, ACTIVITY_ENTRIES_KEPT};
use super::settings::Settings;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `$1` is the scenario, `$2` a folder for the log of what the helper did (`ops`) and
/// for marks that outlive one helper.
const FAKE_HELPER: &str = r#"
scenario="$1"; dir="$2"
nonce="$VIEWCODE_COMPUTER_DRIVER_NONCE"; epoch="$VIEWCODE_COMPUTER_DRIVER_EPOCH"
[ -n "$nonce" ] && [ -n "$epoch" ] || exit 8
printf 'start\n' >> "$dir/ops"
notes='{"handle":"w'$epoch'.1","app":"Notes","pid":70,"title":"Shopping","focused":true,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0},"appIdentifier":"/System/Applications/Notes.app/Contents/MacOS/Notes"}'
vault='{"handle":"w'$epoch'.2","app":"1Password","pid":71,"title":"Vault","focused":false,"bounds":{"x":1000.0,"y":50.0,"width":400.0,"height":300.0}}'
[ "$scenario" = overlap ] && vault='{"handle":"w'$epoch'.2","app":"1Password","pid":71,"title":"Vault","focused":false,"bounds":{"x":500.0,"y":300.0,"width":400.0,"height":300.0}}'
field() { printf '%s\n' "$line" | sed -n 's/.*"'"$1"'":"\([^"]*\)".*/\1/p'; }
reply() { printf '{"id":%s,"ok":true,"result":%s}\n' "$id" "$1"; }
fail() { printf '{"id":%s,"ok":false,"error":{"kind":"%s","message":"%s","dispatched":"no"%s}}\n' "$id" "$1" "$2" "$3"; }
once() { [ -e "$dir/$1" ] && return 1; : > "$dir/$1"; return 0; }
current() { case "$1" in ?"$epoch".*) return 0;; esac; fail stale "The computer-use driver restarted; list windows again." ',"reason":"restarted"'; return 1; }
identity() {
  case "$1" in
    *.11) printf '{"role":"button","label":"Done"}';;
    *.12) printf '{"role":"text field","label":"Title"}';;
    *) printf '{"role":"button","label":"Delete list"}';;
  esac
}
ask() {
  printf '{"id":%s,"authorize":%s,"phase":"%s"}\n' "$id" "$2" "$1"
  IFS= read -r decision || exit 0
  case "$decision" in *'"nonce":"'"$nonce"'","decision":null'*) return 0;; esac
  code=$(printf '%s\n' "$decision" | sed -n 's/.*"code":"\([^"]*\)".*/\1/p')
  message=$(printf '%s\n' "$decision" | sed -n 's/.*"message":"\([^"]*\)".*/\1/p')
  printf 'refused:%s:%s\n' "$1" "$code" >> "$dir/ops"
  printf '{"id":%s,"ok":false,"error":{"kind":"policy","message":"%s","dispatched":"no","code":"%s"}}\n' "$id" "$message" "$code"
  return 1
}
act() {
  ask prepare "$1" || return 0
  [ "$scenario" = hang-prepared ] && exec sleep 30
  dispatch="$1"
  [ -n "$2" ] && dispatch="$2"
  ask dispatch "$dispatch" || return 0
  [ "$scenario" = crash-dispatched ] && once crashed && exit 3
  [ "$scenario" = hang-dispatched ] && exec sleep 30
  printf 'did:%s\n' "$op" >> "$dir/ops"
  reply '{"tookFocus":'"$3"'}'
}
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/^{"id":\([0-9]*\),.*/\1/p')
  case "$line" in *'"nonce":"'"$nonce"'"'*) ;; *) exit 9;; esac
  op=$(field op)
  printf '%s\n' "$op" >> "$dir/ops"
  case "$op" in
    status)
      if [ "$scenario" = denied ]; then reply '{"available":true,"accessibility":"denied","reason":"Grant Accessibility."}'
      else reply '{"available":true,"accessibility":"granted"}'; fi;;
    listWindows) reply "[$notes,$vault]";;
    releaseMouse) reply null;;
    observe)
      current "$(field window)" || continue
      [ "$scenario" = garbage ] && once garbled && { printf 'not json\n'; continue; }
      [ "$scenario" = slow ] && sleep 0.4
      [ "$scenario" = hang-observe ] && exec sleep 30
      reply '{"elements":[{"handle":"e'$epoch'.11","role":"button","label":"Done","enabled":true,"focused":false},{"handle":"e'$epoch'.12","role":"text field","label":"Title","value":"Milk","section":"Lists","enabled":true,"focused":true},{"handle":"e'$epoch'.13","role":"button","label":"Delete list","enabled":true,"focused":false}],"truncated":false}';;
    screenshot)
      current "$(field window)" || continue
      [ "$scenario" = no-screen ] && { fail permission-screen "Screen Recording is not granted." ""; continue; }
      : > "$(field outputPath)"
      reply '{"width":400,"height":300,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0}}';;
    elementAt)
      if [ "$scenario" = delete-under-pointer ]; then reply '{"role":"button","label":"Delete list"}'; else reply null; fi;;
    press|setValue|typeText|scroll)
      [ "$scenario" = crash-early ] && once crashed && exit 3
      element=$(field element)
      current "$element" || continue
      act '{"window":'"$notes"',"element":'"$(identity "$element")"'}' "" false;;
    key|typeFocused|focus|click|drag|move|scrollAt)
      current "$(field window)" || continue
      named=""
      [ "$scenario" = delete-at-dispatch ] && named='{"window":'"$notes"',"element":{"role":"button","label":"Delete list"},"point":{"x":121.0,"y":71.0}}'
      [ "$scenario" = moved ] && { fail stale "The window moved." ""; continue; }
      act '{"window":'"$notes"',"foreground":true}' "${named:-{\"window\":$notes\}}" true;;
    *) fail failed "unknown op" "";;
  esac
done
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("pandacode-computer-use-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const THREAD: &str = "thread-a";

struct FakeHost {
    settings: Mutex<Option<Settings>>,
    turns: Mutex<Vec<(String, TurnContext)>>,
    provider_pending: Mutex<Option<bool>>,
    /// Report a provider approval from this many reads on (0 = never).
    pending_from_read: AtomicUsize,
    reads: AtomicUsize,
    /// Report look only from this many reads of the settings on (0 = never).
    observe_from_read: AtomicUsize,
    settings_reads: AtomicUsize,
    changes: Mutex<Vec<Option<String>>>,
}

impl FakeHost {
    fn set(&self, change: impl FnOnce(&mut Settings)) {
        change(self.settings.lock().unwrap().as_mut().unwrap());
    }

    fn turn(&self, thread_id: &str, turn_id: Option<&str>, full_access: bool) {
        let mut turns = self.turns.lock().unwrap();
        turns.retain(|(thread, _)| thread != thread_id);
        if let Some(turn_id) = turn_id {
            turns.push((thread_id.to_owned(), TurnContext { title: "Tidy notes".into(), turn_id: turn_id.into(), full_access }));
        }
    }

    fn changes(&self) -> usize {
        self.changes.lock().unwrap().len()
    }
}

impl Host for FakeHost {
    fn settings(&self) -> Option<Settings> {
        let read = self.settings_reads.fetch_add(1, Ordering::SeqCst) + 1;
        let from = self.observe_from_read.load(Ordering::SeqCst);
        let settings = *self.settings.lock().unwrap();
        settings.map(|settings| if from != 0 && read >= from { Settings { mode: Mode::Observe, ..settings } } else { settings })
    }

    fn running_turn(&self, thread_id: &str) -> Option<TurnContext> {
        self.turns.lock().unwrap().iter().find(|(thread, _)| thread == thread_id).map(|(_, turn)| turn.clone())
    }

    fn provider_approval_pending(&self) -> Option<bool> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        let from = self.pending_from_read.load(Ordering::SeqCst);
        if from != 0 && read >= from {
            return Some(true);
        }
        *self.provider_pending.lock().unwrap()
    }

    fn changed(&self, thread_id: Option<&str>) {
        self.changes.lock().unwrap().push(thread_id.map(str::to_owned));
    }
}

struct Rig {
    host: Arc<FakeHost>,
    service: Arc<Service>,
    dir: TempDir,
    caller: Caller,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

fn launch(scenario: &str, dir: &Path) -> impl Fn() -> Option<Launch> + Send + Sync + 'static {
    let args = vec!["-c".to_owned(), FAKE_HELPER.to_owned(), "fake-helper".to_owned(), scenario.to_owned(), dir.to_string_lossy().into_owned()];
    move || Some(Launch { program: PathBuf::from("/bin/sh"), args: args.clone() })
}

async fn rig_with(scenario: &str, mode: Mode, full_access: bool, configure: impl FnOnce(&mut DriverOptions, &mut ServiceOptions)) -> Rig {
    let serial = crate::providers::concurrent_process_tests::process_test_lock().lock().await;
    let dir = TempDir::new();
    let host = Arc::new(FakeHost {
        settings: Mutex::new(Some(Settings { mode, approvals: Approvals::Thread, screen: Screen::Allow })),
        turns: Mutex::new(Vec::new()),
        provider_pending: Mutex::new(Some(false)),
        pending_from_read: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
        observe_from_read: AtomicUsize::new(0),
        settings_reads: AtomicUsize::new(0),
        changes: Mutex::new(Vec::new()),
    });
    host.turn(THREAD, Some("turn-1"), full_access);
    // The helper only ever starts on macOS; the fake stands in for it on any Unix.
    let mut driver = DriverOptions::new("macos", launch(scenario, &dir.0));
    let mut options = ServiceOptions::new(dir.0.join("state"), HashSet::from([4242]));
    options.settle_delay = Duration::ZERO;
    options.turn_poll = Duration::from_millis(20);
    configure(&mut driver, &mut options);
    let service = Service::new(host.clone(), DriverClient::new(driver), "macos", options);
    Rig { host, service, dir, caller: Caller { thread_id: THREAD.into(), started_full_access: true }, _serial: serial }
}

async fn rig_for(scenario: &str, mode: Mode, full_access: bool) -> Rig {
    rig_with(scenario, mode, full_access, |_, _| {}).await
}

impl Rig {
    async fn run(&self, body: Value) -> Result<Value, super::policy::ComputerUseError> {
        self.service.handle(&self.caller, &body).await
    }

    async fn ok(&self, body: Value) -> Value {
        self.run(body.clone()).await.unwrap_or_else(|refusal| panic!("{body}: {} {}", serde_json::to_string(&refusal.code).unwrap(), refusal.message))
    }

    async fn refused(&self, body: Value) -> super::policy::ComputerUseError {
        self.run(body.clone()).await.err().unwrap_or_else(|| panic!("{body} was not refused"))
    }

    /// Lists the windows and observes Notes: (window id, refs of Done, Title, Delete list).
    async fn look(&self) -> (u64, [u64; 3]) {
        let windows = self.ok(json!({"command":"list-windows","app":"notes"})).await;
        let window = windows["windows"][0]["id"].as_u64().unwrap();
        let observed = self.ok(json!({"command":"observe","window":window})).await;
        let refs: Vec<u64> = observed["elements"].as_array().unwrap().iter().map(|element| element["ref"].as_u64().unwrap()).collect();
        (window, [refs[0], refs[1], refs[2]])
    }

    /// What the fake helper logged.
    fn ops(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.0.join("ops")).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    fn did(&self, op: &str) -> usize {
        self.ops().iter().filter(|line| **line == format!("did:{op}")).count()
    }

    /// Runs a request in the background, as an agent's tool call does.
    fn start(&self, body: Value) -> tokio::task::JoinHandle<Result<Value, super::policy::ComputerUseError>> {
        let (service, caller) = (self.service.clone(), self.caller.clone());
        tokio::spawn(async move { service.handle(&caller, &body).await })
    }

    /// Waits for the one approval the gate opened.
    async fn approval(&self) -> PendingApproval {
        for _ in 0..500 {
            if let [approval] = self.service.pending_approvals().as_slice() {
                return approval.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no approval opened; the helper did {:?}", self.ops());
    }

    async fn answer(&self, decision: ApprovalAnswer) -> PendingApproval {
        let approval = self.approval().await;
        assert_eq!(self.service.respond_to_approval(THREAD, &approval.request_id, decision), ApprovalResponse::Handled);
        approval
    }
}

fn labels(approval: &PendingApproval) -> Vec<&'static str> {
    approval.options.iter().map(|option| option.label).collect()
}

#[tokio::test]
async fn look_only_lists_observes_and_takes_screenshots_but_never_input() {
    let rig = rig_for("plain", Mode::Observe, true).await;
    let status = rig.ok(json!({"command":"status"})).await;
    assert_eq!(status, json!({"kind":"status","status":{"mode":"observe","platform":"macos","driverAvailable":true,"accessibility":"granted"}}));
    let windows = rig.ok(json!({"command":"list-windows"})).await;
    let listed = windows["windows"].as_array().unwrap();
    assert_eq!(listed.iter().map(|window| window["app"].as_str().unwrap()).collect::<Vec<_>>(), ["Notes", "1Password"]);
    let (notes, vault) = (listed[0]["id"].as_u64().unwrap(), listed[1]["id"].as_u64().unwrap());
    assert_eq!(listed[0], json!({"id":notes,"app":"Notes","pid":70,"title":"Shopping","focused":true,"bounds":{"x":100.0,"y":50.0,"width":800.0,"height":600.0}}), "the helper's handle stays on this side");
    assert_eq!(rig.ok(json!({"command":"list-windows","app":" PASS "})).await["windows"].as_array().unwrap().len(), 1);
    // The password manager is listed, and that is all an agent may do with it.
    assert_eq!(rig.refused(json!({"command":"observe","window":vault})).await.code, ErrorCode::ProtectedApp);
    assert_eq!(rig.refused(json!({"command":"screenshot","window":vault})).await.code, ErrorCode::ProtectedApp);
    assert_eq!(rig.refused(json!({"command":"observe","window":notes + 50})).await.code, ErrorCode::UnknownWindow);
    let observed = rig.ok(json!({"command":"observe","window":notes})).await;
    let first = observed["elements"][0]["ref"].as_u64().unwrap();
    assert_eq!(observed, json!({"kind":"observation","window":notes,"truncated":false,"elements":[
        {"ref":first,"role":"button","label":"Done","enabled":true,"focused":false},
        {"ref":first + 1,"role":"text field","label":"Title","value":"Milk","section":"Lists","enabled":true,"focused":true},
        {"ref":first + 2,"role":"button","label":"Delete list","enabled":true,"focused":false}]}));
    // A query narrows what is shown, and refs are only minted for what is shown.
    let narrowed = rig.ok(json!({"command":"observe","window":notes,"query":"MILK"})).await;
    assert_eq!(narrowed["elements"].as_array().unwrap().iter().map(|element| element["label"].as_str().unwrap()).collect::<Vec<_>>(), ["Title"]);
    assert_eq!(narrowed["elements"][0]["ref"], json!(first + 3));
    let shot = rig.ok(json!({"command":"screenshot","window":notes,"maxSize":800})).await;
    let path = PathBuf::from(shot["path"].as_str().unwrap());
    assert_eq!(path, rig.dir.0.join("state").join("computer-use").join("screenshots").join(THREAD).join("1.png"));
    assert!(path.is_file());
    assert_eq!((shot["kind"].as_str(), shot["window"].as_u64(), shot["width"].as_u64(), shot["height"].as_u64()), (Some("screenshot"), Some(notes), Some(400), Some(300)));
    // Looking never asks, whatever the thread's permissions; input is not available at all.
    let refused = rig.refused(json!({"command":"press","ref":first + 3})).await;
    assert_eq!((refused.code, refused.effect), (ErrorCode::ObserveOnly, Some(Effect::NotDispatched)));
    assert_eq!(rig.refused(json!({"command":"teleport"})).await.code, ErrorCode::UnknownCommand);
    assert_eq!(rig.refused(json!({"command":"observe"})).await.code, ErrorCode::MissingArgument);
    assert_eq!((rig.host.changes(), rig.service.pending_approvals().len()), (0, 0));
    // Off means off for every command, and an unreadable setting is off.
    rig.host.set(|settings| settings.mode = Mode::Off);
    assert_eq!(rig.refused(json!({"command":"status"})).await.code, ErrorCode::Off);
    *rig.host.settings.lock().unwrap() = None;
    assert_eq!(rig.refused(json!({"command":"list-windows"})).await.code, ErrorCode::Off);
    assert_eq!(rig.service.status().await.mode, Mode::Off);
    // The recent-actions list: command, outcome and numbers, newest first.
    let activity = rig.service.recent_activity();
    assert_eq!(activity[0]["command"], "invalid");
    assert_eq!(activity[0]["outcome"], "CU-CON-001");
    let press = activity.iter().find(|entry| entry["command"] == "press").unwrap();
    assert_eq!((press["outcome"].as_str(), press["effect"].as_str(), press["threadId"].as_str()), (Some("CU-CON-002"), Some("not-dispatched"), Some(THREAD)));
    assert!(activity.iter().any(|entry| entry["command"] == "screenshot" && entry["outcome"] == "ok" && entry["durationMs"].is_u64()));
    let text = serde_json::to_string(&activity).unwrap();
    assert!(!text.contains("Shopping") && !text.contains("Milk") && !text.contains("1.png"), "{text}");
    assert_eq!(rig.did("press"), 0);
}

#[tokio::test]
async fn full_access_input_runs_without_asking_and_stale_targets_are_refused() {
    let rig = rig_for("plain", Mode::Control, true).await;
    // Nothing was listed yet: no number means anything.
    assert_eq!(rig.refused(json!({"command":"press","ref":1})).await.code, ErrorCode::UnknownRef);
    assert_eq!(rig.refused(json!({"command":"click","shot":1,"x":1,"y":1})).await.code, ErrorCode::UnknownShot);
    assert_eq!(rig.refused(json!({"command":"key","window":1,"keys":"enter"})).await.code, ErrorCode::UnknownWindow);
    let (window, [done, title, _]) = rig.look().await;
    let pressed = rig.ok(json!({"command":"press","ref":done})).await;
    assert_eq!((pressed["kind"].as_str(), pressed["effect"].as_str(), pressed["tookFocus"].as_bool()), (Some("input"), Some("dispatched"), Some(false)));
    // Every input returns a fresh screenshot, which becomes the window's newest shot.
    let shot = pressed["screenshot"]["shot"].as_u64().unwrap();
    assert!(Path::new(pressed["screenshot"]["path"].as_str().unwrap()).is_file());
    assert_eq!(rig.ok(json!({"command":"type","ref":title,"text":"hunter2"})).await["effect"], "dispatched");
    assert_eq!((rig.did("press"), rig.did("typeText"), rig.host.changes()), (1, 1, 0), "Full access runs routine input without a card");
    // The first shot is no longer the newest: its coordinates are refused.
    let stale = rig.refused(json!({"command":"click","shot":shot,"x":10,"y":10})).await;
    assert_eq!((stale.code, stale.effect), (ErrorCode::StaleShot, Some(Effect::NotDispatched)));
    assert!(stale.message.contains("not the newest screenshot"), "{}", stale.message);
    let newest = rig.ok(json!({"command":"screenshot","window":window})).await["shot"].as_u64().unwrap();
    assert_eq!(rig.refused(json!({"command":"click","shot":newest,"x":400,"y":10})).await.code, ErrorCode::InvalidArgument, "outside the image");
    let clicked = rig.ok(json!({"command":"click","shot":newest,"x":10,"y":10,"count":2})).await;
    assert_eq!(clicked["tookFocus"], true);
    assert_eq!(rig.ok(json!({"command":"key","window":window,"keys":"cmd+s"})).await["effect"], "dispatched");
    // A newer observation retires the older refs.
    rig.ok(json!({"command":"observe","window":window})).await;
    let stale = rig.refused(json!({"command":"press","ref":done})).await;
    assert_eq!((stale.code, stale.message.as_str(), stale.effect), (ErrorCode::StaleRef, format!("Ref {done} is from an older observation; observe again.").as_str(), Some(Effect::NotDispatched)));
    assert_eq!(rig.refused(json!({"command":"press","ref":done + 1000})).await.code, ErrorCode::UnknownRef);
    assert_eq!((rig.did("press"), rig.did("click"), rig.did("key")), (1, 1, 1));
    // Typed text went to the helper and nowhere else.
    let activity = serde_json::to_string(&rig.service.recent_activity()).unwrap();
    assert!(!activity.contains("hunter2"));
    assert!(rig.service.recent_activity().iter().any(|entry| entry["command"] == "click" && entry["effect"] == "dispatched" && entry["tookFocus"] == true));
    // A turn that started stricter is not upgraded by a thread loosened later.
    let stricter = Caller { started_full_access: false, ..rig.caller.clone() };
    let (service, body) = (rig.service.clone(), json!({"command":"observe","window":window}));
    let refs = service.handle(&stricter, &body).await.unwrap();
    let task = tokio::spawn(async move { service.handle(&stricter, &json!({"command":"press","ref":refs["elements"][0]["ref"]})).await });
    assert_eq!(labels(&rig.answer(ApprovalAnswer::Decline).await), ["Allow once", "Allow for the rest of this turn", "Decline"]);
    assert_eq!(task.await.unwrap().unwrap_err().code, ErrorCode::Declined);
    // Without a running turn nothing acts.
    rig.host.turn(THREAD, None, true);
    let refused = rig.refused(json!({"command":"key","window":window,"keys":"enter"})).await;
    assert_eq!((refused.code, refused.effect), (ErrorCode::NoTurn, Some(Effect::NotDispatched)));
}

#[tokio::test]
async fn routine_input_asks_and_runs_when_allowed_and_is_refused_when_declined() {
    let rig = rig_for("plain", Mode::Control, false).await;
    let (_, [done, title, delete]) = rig.look().await;
    // Approved once: the card names the control and the window, and the press runs.
    let press = rig.start(json!({"command":"press","ref":done}));
    let approval = rig.approval().await;
    assert_eq!((approval.thread_id.as_str(), approval.turn_id.as_str(), approval.app_name, approval.detail.as_str()), (THREAD, "turn-1", "Computer use", "Press button \"Done\" in Notes — \"Shopping\""));
    assert!(approval.request_id.starts_with("computer-use:"));
    assert_eq!(labels(&approval), ["Allow once", "Allow for the rest of this turn", "Decline"]);
    assert_eq!(serde_json::to_value(&approval).unwrap()["options"][1], json!({"decision":"acceptForSession","label":"Allow for the rest of this turn"}));
    assert_eq!(rig.did("press"), 0, "nothing reaches the helper before the answer");
    // While the card is open nothing else acts, in this thread or any other.
    let other = Caller { thread_id: "thread-b".into(), started_full_access: true };
    rig.host.turn("thread-b", Some("turn-9"), true);
    let paused = rig.service.handle(&other, &json!({"command":"press","ref":done})).await.unwrap_err();
    assert_eq!((paused.code, paused.effect), (ErrorCode::ApprovalPending, Some(Effect::NotDispatched)));
    assert_eq!(rig.refused(json!({"command":"press","ref":title})).await.code, ErrorCode::ApprovalPending);
    // An answer for another thread, or for something that is not the gate's, changes nothing.
    assert_eq!(rig.service.respond_to_approval("thread-b", &approval.request_id, ApprovalAnswer::Accept), ApprovalResponse::Stale);
    assert_eq!(rig.service.respond_to_approval(THREAD, "provider-7", ApprovalAnswer::Accept), ApprovalResponse::NotOwned);
    assert_eq!(rig.service.respond_to_approval(THREAD, &approval.request_id, ApprovalAnswer::Accept), ApprovalResponse::Handled);
    assert_eq!(rig.service.respond_to_approval(THREAD, &approval.request_id, ApprovalAnswer::Accept), ApprovalResponse::Stale, "a card is answered once");
    assert_eq!(press.await.unwrap().unwrap()["effect"], "dispatched");
    assert_eq!((rig.did("press"), rig.service.pending_approvals().len()), (1, 0));
    assert_eq!(rig.host.changes.lock().unwrap().as_slice(), [Some(THREAD.to_owned()), Some(THREAD.to_owned())], "clients hear the card open and close");

    // Declined: the text is shown as a count only, and nothing is typed.
    let (_, [done, title, delete_now]) = rig.look().await;
    assert!(delete_now > delete);
    let typing = rig.start(json!({"command":"type","ref":title,"text":"hunter2"}));
    let approval = rig.answer(ApprovalAnswer::Decline).await;
    assert_eq!(approval.detail, "Type 7 characters into text field \"Title\" in Notes — \"Shopping\"");
    let declined = typing.await.unwrap().unwrap_err();
    assert_eq!((declined.code, declined.message.as_str(), declined.effect), (ErrorCode::Declined, "The user declined this action, or no answer arrived.", Some(Effect::NotDispatched)));
    assert_eq!(rig.did("typeText"), 0);

    // Allowed for the rest of the turn: routine input stops asking.
    let press = rig.start(json!({"command":"press","ref":done}));
    rig.answer(ApprovalAnswer::AcceptForSession).await;
    press.await.unwrap().unwrap();
    let (_, [done, _, delete]) = rig.look().await;
    rig.ok(json!({"command":"press","ref":done})).await;
    assert_eq!(rig.did("press"), 3);
    // A destructive-looking control still asks, and "the rest of this turn" is not offered.
    let (_, [_, _, delete_again]) = (delete, rig.look().await.1);
    let destroy = rig.start(json!({"command":"press","ref":delete_again}));
    let approval = rig.approval().await;
    assert_eq!(labels(&approval), ["Allow once", "Decline"]);
    assert_eq!(approval.options[0].warning, Some("Looks destructive, so this always asks"));
    // An answer the card did not offer is a decline.
    assert_eq!(rig.service.respond_to_approval(THREAD, &approval.request_id, ApprovalAnswer::AcceptForSession), ApprovalResponse::Handled);
    assert_eq!(destroy.await.unwrap().unwrap_err().code, ErrorCode::Declined);
    let (_, [_, _, delete]) = rig.look().await;
    let destroy = rig.start(json!({"command":"press","ref":delete}));
    rig.answer(ApprovalAnswer::Accept).await;
    destroy.await.unwrap().unwrap();
    assert_eq!(rig.did("press"), 4);
    // The grant ended with its turn: the next turn asks again.
    rig.service.end_turn(THREAD, Some("turn-1"));
    rig.host.turn(THREAD, Some("turn-2"), false);
    let (_, [done, _, _]) = rig.look().await;
    let press = rig.start(json!({"command":"press","ref":done}));
    assert_eq!(rig.approval().await.turn_id, "turn-2");
    rig.answer(ApprovalAnswer::Accept).await;
    press.await.unwrap().unwrap();
    // Risky only: routine input runs, destructive-looking input asks. Never: nothing asks.
    rig.host.set(|settings| settings.approvals = Approvals::Risky);
    let (_, [done, _, delete]) = rig.look().await;
    rig.ok(json!({"command":"press","ref":done})).await;
    let (_, [_, _, delete_fresh]) = (delete, rig.look().await.1);
    let destroy = rig.start(json!({"command":"press","ref":delete_fresh}));
    rig.answer(ApprovalAnswer::Decline).await;
    assert_eq!(destroy.await.unwrap().unwrap_err().code, ErrorCode::Declined);
    rig.host.set(|settings| settings.approvals = Approvals::Never);
    let (_, [_, _, delete]) = rig.look().await;
    rig.ok(json!({"command":"press","ref":delete})).await;
}

#[tokio::test]
async fn an_approval_ends_as_a_decline_when_its_turn_ends_or_nobody_answers() {
    let rig = rig_with("plain", Mode::Control, false, |_, options| options.approval_timeout = Duration::from_millis(1500)).await;
    let (window, [done, _, _]) = rig.look().await;
    // The turn stops while the card is open.
    let press = rig.start(json!({"command":"press","ref":done}));
    rig.approval().await;
    rig.host.turn(THREAD, None, false);
    assert_eq!(press.await.unwrap().unwrap_err().code, ErrorCode::Declined);
    assert_eq!(rig.service.pending_approvals().len(), 0);
    // The app says the turn ended.
    rig.host.turn(THREAD, Some("turn-2"), false);
    let press = rig.start(json!({"command":"press","ref":done}));
    let approval = rig.approval().await;
    rig.service.end_turn(THREAD, None);
    assert_eq!(press.await.unwrap().unwrap_err().code, ErrorCode::Declined);
    assert_eq!(rig.service.respond_to_approval(THREAD, &approval.request_id, ApprovalAnswer::Accept), ApprovalResponse::Stale);
    // The ended turn takes no more input, even while the app still shows it running.
    assert_eq!(rig.refused(json!({"command":"press","ref":done})).await.code, ErrorCode::NoTurn);
    // An answer that arrives for a turn that is no longer the running one is a cancel.
    rig.host.turn(THREAD, Some("turn-3"), false);
    let press = rig.start(json!({"command":"key","window":window,"keys":"enter"}));
    let approval = rig.approval().await;
    assert_eq!(approval.detail, "Press enter in Notes — \"Shopping\"");
    rig.host.turn(THREAD, Some("turn-4"), false);
    assert_eq!(rig.service.respond_to_approval(THREAD, &approval.request_id, ApprovalAnswer::Accept), ApprovalResponse::Stale);
    assert_eq!(press.await.unwrap().unwrap_err().code, ErrorCode::Declined);
    // The caller goes away (its tool call was stopped): the card closes.
    let press = rig.start(json!({"command":"press","ref":done}));
    rig.approval().await;
    press.abort();
    let _ = press.await;
    assert_eq!(rig.service.pending_approvals().len(), 0);
    // The thread is deleted, or computer use is turned off, while a card is open.
    for release in [true, false] {
        let (_, [done, _, _]) = rig.look().await;
        let press = rig.start(json!({"command":"press","ref":done}));
        rig.approval().await;
        if release {
            rig.service.release_thread(THREAD);
        } else {
            rig.service.release_all();
        }
        assert_eq!(press.await.unwrap().unwrap_err().code, ErrorCode::Declined);
        // The thread's targets went with it.
        assert_eq!(rig.refused(json!({"command":"press","ref":done})).await.code, ErrorCode::StaleRef);
        let listed = rig.ok(json!({"command":"list-windows"})).await;
        assert!(listed["windows"][0]["id"].as_u64().unwrap() > window, "numbers are never reused");
    }
    // Nobody answers.
    let (_, [done, _, _]) = rig.look().await;
    let started = std::time::Instant::now();
    assert_eq!(rig.refused(json!({"command":"press","ref":done})).await.code, ErrorCode::Declined);
    assert!(started.elapsed() >= Duration::from_millis(1400));
    assert_eq!((rig.service.pending_approvals().len(), rig.did("press"), rig.did("key")), (0, 0, 0));
}

#[tokio::test]
async fn input_pauses_while_any_approval_waits_anywhere() {
    let rig = rig_for("plain", Mode::Control, true).await;
    let (window, [done, _, _]) = rig.look().await;
    // A provider's approval in some thread, or approvals that cannot be read at all.
    for pending in [Some(true), None] {
        *rig.host.provider_pending.lock().unwrap() = pending;
        let paused = rig.refused(json!({"command":"press","ref":done})).await;
        assert_eq!((paused.code, paused.effect), (ErrorCode::ApprovalPending, Some(Effect::NotDispatched)));
        // Looking is not input.
        rig.ok(json!({"command":"observe","window":window})).await;
    }
    *rig.host.provider_pending.lock().unwrap() = Some(false);
    // An approval that opens after the request was checked is caught by the helper's
    // own questions: before it prepares (the third read) and before it dispatches.
    for (from_read, stage) in [(3, "prepare"), (4, "dispatch")] {
        let (_, [done, _, _]) = rig.look().await;
        rig.host.reads.store(0, Ordering::SeqCst);
        rig.host.pending_from_read.store(from_read, Ordering::SeqCst);
        let paused = rig.refused(json!({"command":"press","ref":done})).await;
        assert_eq!((paused.code, paused.effect), (ErrorCode::ApprovalPending, Some(Effect::NotDispatched)), "{stage}");
        assert!(paused.message.starts_with("Input is paused while an approval waits"));
        assert_eq!(rig.ops().last().map(String::as_str), Some(format!("refused:{stage}:CU-CON-008").as_str()));
        rig.host.pending_from_read.store(0, Ordering::SeqCst);
    }
    // A gate approval cannot open while a provider's waits: the asking input is paused instead.
    rig.host.turn(THREAD, Some("turn-1"), false);
    let (_, [done, _, _]) = rig.look().await;
    rig.host.reads.store(0, Ordering::SeqCst);
    rig.host.pending_from_read.store(2, Ordering::SeqCst);
    assert_eq!(rig.refused(json!({"command":"press","ref":done})).await.code, ErrorCode::ApprovalPending);
    assert_eq!((rig.did("press"), rig.host.changes()), (0, 0));
    // Switching to look only while an action is on its way stops it too: the setting is
    // read for the request, when input begins, at the first check, and then (the fourth
    // read) when the helper asks before it prepares.
    rig.host.pending_from_read.store(0, Ordering::SeqCst);
    rig.host.turn(THREAD, Some("turn-1"), true);
    rig.host.settings_reads.store(0, Ordering::SeqCst);
    rig.host.observe_from_read.store(4, Ordering::SeqCst);
    let stopped = rig.refused(json!({"command":"press","ref":done})).await;
    assert_eq!((stopped.code, stopped.message.as_str(), stopped.effect), (ErrorCode::ObserveOnly, "Computer use no longer allows input.", Some(Effect::NotDispatched)));
    assert_eq!((rig.did("press"), rig.ops().last().map(String::as_str)), (0, Some("refused:prepare:CU-CON-002")));
    // The turn ending on the way stops it as well.
    rig.host.observe_from_read.store(0, Ordering::SeqCst);
    let (_, [done, _, _]) = rig.look().await;
    let press = rig.start(json!({"command":"press","ref":done}));
    rig.service.end_turn(THREAD, Some("turn-1"));
    assert_eq!(press.await.unwrap().unwrap_err().code, ErrorCode::NoTurn);
    assert_eq!(rig.did("press"), 0);
}

#[tokio::test]
async fn showing_a_task_on_screen_asks_once_per_turn() {
    let rig = rig_for("plain", Mode::Control, true).await;
    rig.host.set(|settings| settings.screen = Screen::Ask);
    let (window, [done, _, _]) = rig.look().await;
    // By reference nothing comes to the front, so nothing asks.
    rig.ok(json!({"command":"press","ref":done})).await;
    assert_eq!(rig.host.changes(), 0);
    // A shortcut needs the window in front: kept in the background, it never runs this turn.
    let key = rig.start(json!({"command":"key","window":window,"keys":"enter"}));
    let approval = rig.answer(ApprovalAnswer::Decline).await;
    assert_eq!(labels(&approval), ["Show on screen for this task", "Keep it in the background"]);
    assert!(approval.detail.starts_with("Show this on your screen? To work in Notes,"), "{}", approval.detail);
    let kept = key.await.unwrap().unwrap_err();
    assert_eq!(kept.code, ErrorCode::Declined);
    assert!(kept.message.starts_with("The user chose to keep this task in the background"));
    let changes = rig.host.changes();
    assert_eq!(rig.refused(json!({"command":"focus","window":window})).await.message, kept.message);
    assert_eq!(rig.host.changes(), changes, "the answer holds for the turn without asking again");
    let (_, [done, _, _]) = rig.look().await;
    rig.ok(json!({"command":"press","ref":done})).await;
    // The next turn asks again; shown on screen, input that takes the screen runs.
    rig.host.turn(THREAD, Some("turn-2"), true);
    let key = rig.start(json!({"command":"key","window":window,"keys":"enter"}));
    rig.answer(ApprovalAnswer::AcceptForSession).await;
    assert_eq!(key.await.unwrap().unwrap()["tookFocus"], true);
    rig.ok(json!({"command":"focus","window":window})).await;
    assert_eq!((rig.did("key"), rig.did("focus")), (1, 1));
    // No answer about the screen is not a yes.
    rig.host.turn(THREAD, Some("turn-3"), true);
    let key = rig.start(json!({"command":"type-focused","window":window,"text":"hello"}));
    let approval = rig.approval().await;
    assert_eq!(rig.service.respond_to_approval(THREAD, &approval.request_id, ApprovalAnswer::Cancel), ApprovalResponse::Handled);
    assert_eq!(key.await.unwrap().unwrap_err().message, "No answer arrived about showing this task on screen.");
    assert_eq!(rig.did("typeFocused"), 0);
}

#[tokio::test]
async fn what_the_helper_finds_at_dispatch_is_checked_again() {
    // Nothing labelled was under the pointer when the gate looked; at dispatch the
    // helper names a destructive control there.
    let rig = rig_for("delete-at-dispatch", Mode::Control, true).await;
    let (window, _) = rig.look().await;
    let shot = rig.ok(json!({"command":"screenshot","window":window})).await["shot"].as_u64().unwrap();
    let refused = rig.refused(json!({"command":"click","shot":shot,"x":10,"y":10})).await;
    assert_eq!((refused.code, refused.message.as_str(), refused.effect), (ErrorCode::StaleShot, "The target now looks destructive; look again and request approval.", Some(Effect::NotDispatched)));
    assert_eq!((rig.did("click"), rig.ops().last().map(String::as_str)), (0, Some("refused:dispatch:CU-CON-007")));
    drop(rig);
    // The gate sees the destructive control itself: it asks, and names what the click hits.
    let rig = rig_for("delete-under-pointer", Mode::Control, true).await;
    let (window, _) = rig.look().await;
    let shot = rig.ok(json!({"command":"screenshot","window":window})).await["shot"].as_u64().unwrap();
    let click = rig.start(json!({"command":"click","shot":shot,"x":10,"y":10}));
    let approval = rig.answer(ApprovalAnswer::Accept).await;
    assert_eq!(approval.detail, "Click at (10, 10) in Notes — \"Shopping\" (on button \"Delete list\"); brings the window to the front");
    assert_eq!(labels(&approval), ["Allow once", "Decline"]);
    assert_eq!(click.await.unwrap().unwrap()["effect"], "dispatched");
    // A move over the same control is not a click: it does not ask.
    let shot = rig.ok(json!({"command":"screenshot","window":window})).await["shot"].as_u64().unwrap();
    rig.ok(json!({"command":"move","shot":shot,"x":10,"y":10})).await;
    assert_eq!((rig.did("click"), rig.did("move")), (1, 1));
    drop(rig);
    // The window moved since the screenshot: the shot stops accepting coordinates.
    let rig = rig_for("moved", Mode::Control, true).await;
    let (window, _) = rig.look().await;
    let shot = rig.ok(json!({"command":"screenshot","window":window})).await["shot"].as_u64().unwrap();
    let moved = rig.refused(json!({"command":"drag","shot":shot,"fromX":1,"fromY":1,"toX":9,"toY":9})).await;
    assert_eq!((moved.code, moved.message.as_str()), (ErrorCode::StaleShot, format!("The window moved, resized or closed since shot {shot}; take a new screenshot.").as_str()));
    assert_eq!(rig.refused(json!({"command":"scroll-at","shot":shot,"x":1,"y":1,"dx":0,"dy":3})).await.code, ErrorCode::StaleShot);
    // A stale window handle retires the window.
    assert_eq!(rig.refused(json!({"command":"focus","window":window})).await.code, ErrorCode::UnknownWindow);
    assert_eq!(rig.refused(json!({"command":"observe","window":window})).await.code, ErrorCode::UnknownWindow);
}

#[tokio::test]
async fn a_screenshot_is_refused_while_a_protected_window_overlaps() {
    let rig = rig_for("overlap", Mode::Control, true).await;
    let (window, [done, _, _]) = rig.look().await;
    let refused = rig.refused(json!({"command":"screenshot","window":window})).await;
    assert_eq!((refused.code, refused.message.as_str()), (ErrorCode::ProtectedApp, "A protected window overlaps this window; move it away and try again."));
    assert!(!rig.ops().iter().any(|op| op == "screenshot"), "nothing is captured");
    // Input still runs; it is reported without a picture.
    let pressed = rig.ok(json!({"command":"press","ref":done})).await;
    assert_eq!((pressed["effect"].as_str(), pressed.get("screenshot")), (Some("dispatched"), None));
    drop(rig);
    // Without Screen Recording the same holds, and a screenshot says what is missing.
    let rig = rig_for("no-screen", Mode::Control, true).await;
    let (window, [done, _, _]) = rig.look().await;
    assert_eq!(rig.refused(json!({"command":"screenshot","window":window})).await.code, ErrorCode::ScreenRecordingMissing);
    assert_eq!(rig.ok(json!({"command":"press","ref":done})).await.get("screenshot"), None);
}

#[tokio::test]
async fn a_helper_that_crashes_is_replaced_and_its_handles_are_stale() {
    // It dies after the gate allowed the dispatch: the input may have landed.
    let rig = rig_for("crash-dispatched", Mode::Control, true).await;
    let (window, [done, _, _]) = rig.look().await;
    let shot = rig.ok(json!({"command":"screenshot","window":window})).await["shot"].as_u64().unwrap();
    let crashed = rig.refused(json!({"command":"click","shot":shot,"x":10,"y":10})).await;
    assert_eq!((crashed.code, crashed.message.as_str(), crashed.effect), (ErrorCode::DriverFailed, "The computer-use driver exited. The input may have been delivered; observe before retrying.", Some(Effect::DispatchedUnknown)));
    assert_eq!(rig.service.driver().helpers_started().await, 1);
    // The next call starts a fresh helper, which first lets go of the mouse button the
    // old one may have left held, and knows none of the old handles.
    let stale = rig.refused(json!({"command":"press","ref":done})).await;
    assert_eq!((stale.code, stale.message.as_str(), stale.effect), (ErrorCode::StaleRef, "The computer-use driver restarted; run `viewcode-computer list-windows`, then observe again.", Some(Effect::NotDispatched)));
    assert_eq!(rig.service.driver().helpers_started().await, 2);
    let ops = rig.ops();
    let restart = ops.iter().rposition(|op| op == "start").unwrap();
    assert_eq!(&ops[restart..restart + 3], ["start", "releaseMouse", "press"]);
    assert_eq!(rig.refused(json!({"command":"observe","window":window})).await.code, ErrorCode::UnknownWindow, "the window went with its helper");
    // Listing again finds everything under new numbers, and input works.
    let (again, [done, _, _]) = rig.look().await;
    assert!(again > window);
    assert_eq!(rig.ok(json!({"command":"press","ref":done})).await["effect"], "dispatched");
    assert_eq!((rig.did("click"), rig.did("press"), rig.service.driver().helpers_started().await), (0, 1, 2));
    drop(rig);

    // It dies before it asked anything: nothing was sent.
    let rig = rig_for("crash-early", Mode::Control, true).await;
    let (window, [done, _, _]) = rig.look().await;
    let crashed = rig.refused(json!({"command":"press","ref":done})).await;
    assert_eq!((crashed.code, crashed.message.as_str(), crashed.effect), (ErrorCode::DriverFailed, "The computer-use driver exited.", Some(Effect::NotDispatched)));
    let stale = rig.refused(json!({"command":"key","window":window,"keys":"enter"})).await;
    assert_eq!((stale.code, stale.message.as_str()), (ErrorCode::UnknownWindow, "The computer-use driver restarted; run `viewcode-computer list-windows` again."));
    assert!(!rig.ops().iter().any(|op| op == "releaseMouse"));
    assert_eq!(rig.service.driver().helpers_started().await, 2);
}

#[tokio::test]
async fn a_helper_that_hangs_times_out_and_is_replaced() {
    let short = |driver: &mut DriverOptions, _: &mut ServiceOptions| {
        driver.timeouts = [("key", 400), ("observe", 400), ("press", 400)].into();
    };
    // It hangs before the dispatch was allowed: nothing was sent.
    let rig = rig_with("hang-prepared", Mode::Control, true, short).await;
    let (window, _) = rig.look().await;
    let started = std::time::Instant::now();
    let timed_out = rig.refused(json!({"command":"key","window":window,"keys":"enter"})).await;
    assert_eq!((timed_out.code, timed_out.message.as_str(), timed_out.effect), (ErrorCode::DriverFailed, "The computer-use driver timed out.", Some(Effect::NotDispatched)));
    assert!(started.elapsed() < Duration::from_secs(8), "the helper is killed, not waited for");
    assert_eq!(rig.service.driver().helpers_started().await, 1);
    rig.ok(json!({"command":"list-windows"})).await;
    assert_eq!(rig.service.driver().helpers_started().await, 2);
    drop(rig);
    // It hangs after: the input may have been delivered and must not be replayed blindly.
    let rig = rig_with("hang-dispatched", Mode::Control, true, short).await;
    let (_, [done, _, _]) = rig.look().await;
    let timed_out = rig.refused(json!({"command":"press","ref":done})).await;
    assert_eq!((timed_out.code, timed_out.message.as_str(), timed_out.effect), (ErrorCode::DriverFailed, "The computer-use driver timed out. The input may have been delivered; observe before retrying.", Some(Effect::DispatchedUnknown)));
    assert!(!rig.ops().iter().any(|op| op == "releaseMouse"), "a press holds no mouse button");
    drop(rig);
    // Looking can time out too.
    let rig = rig_with("hang-observe", Mode::Observe, true, short).await;
    let window = rig.ok(json!({"command":"list-windows"})).await["windows"][0]["id"].as_u64().unwrap();
    let timed_out = rig.refused(json!({"command":"observe","window":window})).await;
    assert_eq!((timed_out.code, timed_out.effect), (ErrorCode::DriverFailed, None));
    assert!(rig.service.status().await.driver_available);
}

#[tokio::test]
async fn a_reply_that_is_not_a_reply_is_malformed_and_helpers_are_recycled() {
    let rig = rig_with("garbage", Mode::Observe, true, |driver, _| driver.recycle_after = 4).await;
    let window = rig.ok(json!({"command":"list-windows"})).await["windows"][0]["id"].as_u64().unwrap();
    let malformed = rig.refused(json!({"command":"observe","window":window})).await;
    assert_eq!((malformed.code, malformed.message.as_str()), (ErrorCode::DriverMalformed, "The computer-use driver answered with an invalid reply."));
    // The stream is out of step, so a fresh helper takes over.
    assert_eq!(rig.refused(json!({"command":"observe","window":window})).await.code, ErrorCode::UnknownWindow);
    assert_eq!(rig.service.driver().helpers_started().await, 2);
    // After `recycle_after` requests a helper is replaced, so it cannot grow without bound.
    for _ in 0..4 {
        rig.ok(json!({"command":"list-windows"})).await;
    }
    assert_eq!(rig.service.driver().helpers_started().await, 3);
    // The activity list is bounded as well.
    for _ in 0..ACTIVITY_ENTRIES_KEPT {
        let _ = rig.run(json!({"command":"nothing"})).await;
    }
    assert_eq!(rig.service.recent_activity().len(), ACTIVITY_ENTRIES_KEPT);
    drop(rig);
    // A helper that saw Accessibility missing never sees it granted: each status starts a new one.
    let rig = rig_for("denied", Mode::Observe, true).await;
    let status = rig.service.status().await;
    assert_eq!((status.driver_available, serde_json::to_value(&status).unwrap()["accessibility"].as_str().map(str::to_owned), status.reason.as_deref()), (true, Some("denied".to_owned()), Some("Grant Accessibility.")));
    rig.service.status().await;
    assert_eq!(rig.service.driver().helpers_started().await, 2);
    // Closed for good: nothing starts again.
    rig.service.driver().close();
    let status = rig.service.status().await;
    assert_eq!((status.driver_available, status.reason.as_deref()), (false, Some("The computer-use driver could not start.")));
    assert_eq!(rig.refused(json!({"command":"list-windows"})).await.code, ErrorCode::DriverUnavailable);
    assert_eq!(rig.service.driver().helpers_started().await, 2);
}

#[tokio::test]
async fn a_call_nobody_waits_for_is_never_sent_and_never_allowed() {
    let _serial = crate::providers::concurrent_process_tests::process_test_lock().lock().await;
    let dir = TempDir::new();
    let ops = || std::fs::read_to_string(dir.0.join("ops")).unwrap_or_default();
    let driver = DriverClient::new(DriverOptions::new("macos", launch("slow", &dir.0)));
    let windows: Vec<DriverWindow> = driver.request(DriverRequest::ListWindows, None).await.unwrap();
    let handle = windows[0].handle.clone();
    // Queued behind a slow call and given up on before its turn came: never sent.
    let slow = {
        let (driver, handle) = (driver.clone(), handle.clone());
        tokio::spawn(async move { driver.call(DriverRequest::Observe { window: handle, max_elements: 10, query: None }, None).await })
    };
    while !ops().contains("observe") {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let queued = driver.call(DriverRequest::Focus { window: handle.clone() }, None);
    assert!(tokio::time::timeout(Duration::from_millis(50), queued).await.is_err(), "still queued");
    slow.await.unwrap().unwrap();
    assert_eq!(driver.request::<Vec<DriverWindow>>(DriverRequest::ListWindows, None).await.unwrap().len(), 2);
    assert!(!ops().contains("focus"), "{}", ops());

    // Input without a gate to ask is refused by the client itself.
    let failure = driver.request::<InputResult>(DriverRequest::Focus { window: handle.clone() }, None).await.unwrap_err();
    assert_eq!((failure.kind, failure.message.as_str(), failure.dispatched, failure.code), (FailureKind::Policy, cancelled_before_dispatch().message.as_str(), Dispatched::No, Some(ErrorCode::Declined)));

    // The caller goes away while the gate is deciding: the answer is a refusal, the
    // helper is told so, and it stays in step for the next call.
    let (asked, release) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
    let seen: Arc<Mutex<Vec<(Phase, DispatchTarget)>>> = Arc::default();
    let authorize: Authorize = {
        let (asked, release, seen) = (asked.clone(), release.clone(), seen.clone());
        Arc::new(move |target, phase| {
            let (asked, release, seen) = (asked.clone(), release.clone(), seen.clone());
            Box::pin(async move {
                seen.lock().unwrap().push((phase, target));
                asked.notify_one();
                release.notified().await;
                Ok(None)
            })
        })
    };
    let call = {
        let (driver, handle) = (driver.clone(), handle.clone());
        tokio::spawn(async move { driver.request::<InputResult>(DriverRequest::Press { element: handle.replacen('w', "e", 1).replace(".1", ".11"), expect: ElementIdentity { role: "button".into(), label: "Done".into() } }, Some(authorize)).await })
    };
    asked.notified().await;
    call.abort();
    let _ = call.await;
    release.notify_one();
    assert_eq!(driver.request::<Vec<DriverWindow>>(DriverRequest::ListWindows, None).await.unwrap().len(), 2);
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "the dispatch question never came");
    assert_eq!((seen[0].0, seen[0].1.window.handle.as_str(), seen[0].1.element.as_ref().map(|element| element.label.as_str()), seen[0].1.foreground), (Phase::Prepare, handle.as_str(), Some("Done"), false));
    assert!(ops().contains("refused:prepare:CU-CON-004") && !ops().contains("did:press"), "{}", ops());
    assert_eq!(driver.helpers_started().await, 1, "one helper served all of it");
}
