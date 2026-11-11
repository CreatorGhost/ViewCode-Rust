//! The gate itself: every computer-use request of an agent lands here, already bound to
//! the thread it came from. The service decides what the request may do (setting, running
//! turn, protected apps, approval, stale targets) with the rules in `policy.rs`, and only
//! then reaches the helper through `driver.rs`. It must stay safe when the agent ignores
//! every instruction: nothing here trusts what the caller checked.
//!
//! State is in memory and per thread: window ids, refs and shots, a routine-input grant
//! for the current turn, the show-on-screen answer and the approvals waiting for the
//! user. A restart forgets it. What the app knows (settings, the running turn, provider
//! approvals) is read through [`Host`], fresh for every decision.

use super::driver::{Authorize, DriverClient};
use super::policy::{
    admit, begin_input, decide_input, driver_error_to_computer_use_error, driver_request, final_check, hit_point, input_failure, internal_error, matches_query, observable_window, observation_failure, overlaid, own_process_floor, Guard,
    protected_overlay, screenshots_dir, validate_request, ApprovalAnswer, ApprovalOption, ApprovalPrompt, ApprovalResponse, AskOutcome, CheckedInput, ComputerUseError, Hit, InputDecision, InputRequest, Live, Mode, PendingApprovals, Request,
    ResolvedTarget, ScreenStep, ThreadGate, TurnContext, APPROVAL_APP_NAME, APPROVAL_TIMEOUT_SECS, DEFAULT_SCREENSHOT_SIZE, MAX_OBSERVED_ELEMENTS, SCREENSHOTS_KEPT, SETTLE_DELAY_MS,
};
use super::protocol::{Accessibility, DispatchTarget, DriverFailure, DriverRequest, DriverWindow, ElementIdentity, InputResult, Observation, Phase, Rect, Screenshot, WindowKind};
use super::settings::Settings;
use super::targets::{NewShot, WindowRecord};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Requests kept for a "recent actions" list.
pub(crate) const ACTIVITY_ENTRIES_KEPT: usize = 100;

/// What the gate reads from the app. Every method is asked again for each decision and
/// must fail closed: `None` means "could not be read".
pub(crate) trait Host: Send + Sync + 'static {
    /// The stored settings; `None` counts as off, strictest approvals, ask for the screen.
    fn settings(&self) -> Option<Settings>;
    /// The thread's running turn, or `None` when no turn is running in it.
    fn running_turn(&self, thread_id: &str) -> Option<TurnContext>;
    /// Whether a provider approval waits for the user in any thread (questions for the
    /// user do not count); `None` counts as pending.
    fn provider_approval_pending(&self) -> Option<bool>;
    /// Approvals or settings changed; clients re-read the state.
    fn changed(&self, thread_id: Option<&str>);
}

/// Who is calling: bound by the app when it hands a thread its tools, never read from
/// the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Caller {
    pub(crate) thread_id: String,
    /// Whether the run that holds these tools started with Full access: a thread
    /// loosened later must not upgrade a turn that started stricter.
    pub(crate) started_full_access: bool,
}

pub(crate) struct ServiceOptions {
    /// Screenshots are written under `computer-use/screenshots/<thread>` in here.
    pub(crate) state_dir: PathBuf,
    /// The app's own process ancestry; its windows are never observed or controlled.
    pub(crate) own_pids: HashSet<u32>,
    /// How long input gets to take effect before the follow-up screenshot.
    pub(crate) settle_delay: Duration,
    /// No answer within this long counts as a decline.
    pub(crate) approval_timeout: Duration,
    /// How often a waiting approval looks whether its turn is still running.
    pub(crate) turn_poll: Duration,
}

impl ServiceOptions {
    pub(crate) fn new(state_dir: PathBuf, own_pids: HashSet<u32>) -> Self {
        Self { state_dir, own_pids, settle_delay: Duration::from_millis(SETTLE_DELAY_MS), approval_timeout: Duration::from_secs(APPROVAL_TIMEOUT_SECS), turn_poll: Duration::from_millis(500) }
    }
}

/// The setting plus what the helper can do right now.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub(crate) mode: Mode,
    pub(crate) platform: &'static str,
    pub(crate) driver_available: bool,
    pub(crate) accessibility: Accessibility,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

/// An approval waiting for the user, as clients list it. `detail` never carries typed text.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingApproval {
    pub(crate) request_id: String,
    pub(crate) thread_id: String,
    pub(crate) turn_id: String,
    pub(crate) app_name: &'static str,
    pub(crate) detail: String,
    pub(crate) options: &'static [ApprovalOption],
    pub(crate) created_at: i64,
}

struct Waiting {
    approval: PendingApproval,
    answer: tokio::sync::oneshot::Sender<ApprovalAnswer>,
}

#[derive(Serialize)]
struct WindowOut<'a> {
    id: u64,
    app: &'a str,
    pid: u32,
    title: &'a str,
    focused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    bounds: Option<Rect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<WindowKind>,
}

/// A written screenshot as the agent is told about it.
#[derive(Serialize, Debug, Clone, PartialEq)]
struct ShotOut {
    shot: u64,
    window: u64,
    path: String,
    width: u32,
    height: u32,
}

enum Capture {
    Captured(ShotOut),
    /// A protected window overlaps the target, or the target itself is protected now.
    Overlaid,
    Failed(DriverFailure),
}

#[derive(Default)]
struct Inner {
    gates: HashMap<String, ThreadGate>,
    pending: PendingApprovals,
    waiting: HashMap<String, Waiting>,
    /// Newest first; command, outcome and numbers only.
    activity: VecDeque<Value>,
    screenshot_counters: HashMap<String, u64>,
}

impl Inner {
    fn gate(&mut self, thread_id: &str) -> &mut ThreadGate {
        self.gates.entry(thread_id.to_owned()).or_default()
    }

    /// Hands waiting actions their cancellation; true when any was waiting.
    fn cancel(&mut self, request_ids: Vec<String>) -> bool {
        let mut any = false;
        for request_id in request_ids {
            if let Some(waiting) = self.waiting.remove(&request_id) {
                let _ = waiting.answer.send(ApprovalAnswer::Cancel);
                any = true;
            }
        }
        any
    }
}

/// What the app said when a decision was made.
struct Snapshot {
    settings: Settings,
    turn: Option<TurnContext>,
    provider_pending: Option<bool>,
}

pub(crate) struct Service {
    host: Arc<dyn Host>,
    driver: Arc<DriverClient>,
    os: &'static str,
    options: ServiceOptions,
    /// Held from the final check of an input until its native action returns, and while
    /// an approval opens, so no approval can open between that check and the input
    /// landing. Not held while a request waits for the helper or prepares its target.
    dispatch_lock: Arc<tokio::sync::Mutex<()>>,
    inner: Mutex<Inner>,
}

/// Closes an approval that is still open when its action stops waiting, whatever the reason.
struct OpenApproval<'a> {
    service: &'a Service,
    thread_id: &'a str,
    request_id: String,
}

impl Drop for OpenApproval<'_> {
    fn drop(&mut self) {
        let mut inner = self.service.inner();
        let closed = inner.pending.close(&self.request_id);
        let waited = inner.waiting.remove(&self.request_id).is_some();
        drop(inner);
        if closed || waited {
            self.service.host.changed(Some(self.thread_id));
        }
    }
}

type Check = Arc<dyn Fn(Option<&DispatchTarget>) -> Result<(), ComputerUseError> + Send + Sync>;

impl Service {
    pub(crate) fn new(host: Arc<dyn Host>, driver: Arc<DriverClient>, os: &'static str, options: ServiceOptions) -> Arc<Self> {
        Arc::new(Self { host, driver, os, options, dispatch_lock: Arc::new(tokio::sync::Mutex::new(())), inner: Mutex::new(Inner::default()) })
    }

    pub(crate) fn driver(&self) -> &Arc<DriverClient> {
        &self.driver
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    /// Read before `inner` is locked: the host takes the app's own locks.
    fn snapshot(&self, caller: &Caller) -> Snapshot {
        let settings = self.host.settings().unwrap_or(Settings::UNREADABLE);
        let turn = self.host.running_turn(&caller.thread_id).map(|turn| TurnContext { full_access: turn.full_access && caller.started_full_access, ..turn });
        Snapshot { settings, turn, provider_pending: self.host.provider_approval_pending() }
    }

    /// Runs one decision against what is true now.
    fn with_live<R>(&self, caller: &Caller, decide: impl FnOnce(&Live<'_>, &mut ThreadGate) -> R) -> R {
        let snapshot = self.snapshot(caller);
        let mut inner = self.inner();
        let approval_pending = inner.pending.any_pending(snapshot.provider_pending);
        let gate = inner.gate(&caller.thread_id);
        let turn = gate.running_turn(snapshot.turn);
        let live = Live { mode: snapshot.settings.mode, approvals: snapshot.settings.approvals, screen: snapshot.settings.screen, turn: turn.as_ref(), approval_pending, own_pids: &self.options.own_pids, background_input: false };
        decide(&live, gate)
    }

    /// The protected-apps rule as the settings have it now; unreadable settings keep it on.
    fn guard(&self) -> Guard<'_> {
        Guard::new(&self.options.own_pids, self.host.settings().unwrap_or(Settings::UNREADABLE).approvals)
    }

    fn with_gate<R>(&self, caller: &Caller, change: impl FnOnce(&mut ThreadGate) -> R) -> R {
        change(self.inner().gate(&caller.thread_id))
    }

    /// Fresh every call: the setting plus what the helper can do right now.
    pub(crate) async fn status(&self) -> Status {
        let mode = self.host.settings().map_or(Mode::Off, |settings| settings.mode);
        let driver = self.driver.status().await;
        Status { mode, platform: self.os, driver_available: driver.available, accessibility: driver.accessibility, reason: driver.reason }
    }

    /// The last requests handled, newest first: command, outcome and numbers only, never
    /// anything an agent typed or a target's details.
    pub(crate) fn recent_activity(&self) -> Vec<Value> {
        self.inner().activity.iter().cloned().collect()
    }

    /// The gate's own approvals waiting for the user, oldest first.
    pub(crate) fn pending_approvals(&self) -> Vec<PendingApproval> {
        let mut approvals: Vec<PendingApproval> = self.inner().waiting.values().map(|waiting| waiting.approval.clone()).collect();
        approvals.sort_by(|left, right| (left.created_at, &left.request_id).cmp(&(right.created_at, &right.request_id)));
        approvals
    }

    /// Runs one request. Refusals and helper failures are the error; nothing else fails.
    pub(crate) async fn handle(self: &Arc<Self>, caller: &Caller, body: &Value) -> Result<Value, ComputerUseError> {
        let started = std::time::Instant::now();
        // Read fresh for each request; an unreadable setting never turns computer use on.
        let mode = self.host.settings().map_or(Mode::Off, |settings| settings.mode);
        let command = if mode == Mode::Off { "invalid" } else { validate_request(body).map_or("invalid", |request| request.command()) };
        let result = match admit(mode, body) {
            Ok(request) => self.run(caller, request).await,
            Err(refusal) => Err(refusal),
        };
        self.record(caller, command, &result, started.elapsed());
        result
    }

    async fn run(self: &Arc<Self>, caller: &Caller, request: Request) -> Result<Value, ComputerUseError> {
        match request {
            Request::Status => Ok(json!({ "kind": "status", "status": self.status().await })),
            Request::ListWindows { app } => self.list_windows(caller, app.as_deref()).await,
            Request::Observe { window, query } => self.observe(caller, window, query).await,
            Request::Screenshot { window, max_size } => self.screenshot(caller, window, max_size).await,
            Request::Input(input) => self.input(caller, input).await,
        }
    }

    /// Command, outcome and numbers only: never text, values, labels, titles, paths or coordinates.
    fn record(&self, caller: &Caller, command: &'static str, result: &Result<Value, ComputerUseError>, elapsed: Duration) {
        let mut entry = json!({ "at": crate::storage::now(), "threadId": caller.thread_id, "command": command, "durationMs": elapsed.as_millis() as u64 });
        match result {
            Ok(result) => {
                entry["outcome"] = json!("ok");
                for field in ["effect", "tookFocus"] {
                    if let Some(value) = result.get(field) {
                        entry[field] = value.clone();
                    }
                }
            }
            Err(refusal) => {
                entry["outcome"] = json!(refusal.code);
                if let Some(effect) = refusal.effect {
                    entry["effect"] = json!(effect);
                }
            }
        }
        let mut inner = self.inner();
        inner.activity.push_front(entry);
        inner.activity.truncate(ACTIVITY_ENTRIES_KEPT);
    }

    async fn listing(&self) -> Result<Vec<DriverWindow>, DriverFailure> {
        self.driver.request(DriverRequest::ListWindows, None).await
    }

    async fn list_windows(&self, caller: &Caller, app: Option<&str>) -> Result<Value, ComputerUseError> {
        let listed = self.listing().await.map_err(|failure| driver_error_to_computer_use_error(&failure, false))?;
        let records = self.with_gate(caller, |gate| gate.targets.record_windows(&listed));
        let filter = app.map(|app| app.trim().to_lowercase()).filter(|app| !app.is_empty());
        let windows: Vec<WindowOut<'_>> = listed
            .iter()
            .zip(&records)
            .filter(|(window, _)| filter.as_ref().is_none_or(|filter| window.app.to_lowercase().contains(filter)))
            .map(|(window, record)| WindowOut { id: record.id, app: &window.app, pid: window.pid, title: &window.title, focused: window.focused, bounds: window.bounds, kind: window.kind })
            .collect();
        Ok(json!({ "kind": "windows", "windows": windows }))
    }

    /// The window an observe or screenshot names, with its title and handle re-read: a
    /// cached record is checked first so a protected app costs no helper call.
    async fn fresh_window(&self, caller: &Caller, window_id: u64) -> Result<WindowRecord, ComputerUseError> {
        let own = self.guard();
        self.with_gate(caller, |gate| observable_window(&gate.targets, window_id, own))?;
        let listed = self.listing().await.map_err(|failure| driver_error_to_computer_use_error(&failure, false))?;
        self.with_gate(caller, |gate| {
            gate.targets.record_windows(&listed);
            observable_window(&gate.targets, window_id, own)
        })
    }

    async fn observe(&self, caller: &Caller, window_id: u64, query: Option<String>) -> Result<Value, ComputerUseError> {
        let window = self.fresh_window(caller, window_id).await?;
        let query = query.filter(|query| !query.trim().is_empty());
        let observed: Result<Observation, DriverFailure> = self.driver.request(DriverRequest::Observe { window: window.handle.clone(), max_elements: MAX_OBSERVED_ELEMENTS, query: query.clone() }, None).await;
        let observation = match observed {
            Ok(observation) => observation,
            Err(failure) => return Err(self.with_gate(caller, |gate| observation_failure(&mut gate.targets, &window, &failure))),
        };
        // Filtered again on this side, so a ref is only minted for what the agent is shown.
        let matching: Vec<_> = observation.elements.into_iter().filter(|element| query.as_deref().is_none_or(|query| matches_query(element, query))).collect();
        let refs = self.with_gate(caller, |gate| gate.targets.record_observation(window.id, &matching));
        let elements: Vec<Value> = matching
            .iter()
            .zip(refs)
            .map(|(element, reference)| {
                let mut out = json!({ "ref": reference, "role": element.role, "label": element.label, "enabled": element.enabled, "focused": element.focused });
                for (name, value) in [("value", &element.value), ("section", &element.section), ("instance", &element.instance)] {
                    if let Some(value) = value {
                        out[name] = json!(value);
                    }
                }
                out
            })
            .collect();
        Ok(json!({ "kind": "observation", "window": window.id, "elements": elements, "truncated": observation.truncated }))
    }

    /// The next numbered file in the thread's screenshot folder. Numbers continue after
    /// files left by an earlier run, so pruning never mistakes a new capture for an old one.
    fn next_screenshot_path(&self, thread_id: &str) -> std::io::Result<(PathBuf, PathBuf)> {
        let directory = screenshots_dir(&self.options.state_dir, thread_id);
        std::fs::create_dir_all(&directory)?;
        let mut inner = self.inner();
        let last = match inner.screenshot_counters.get(thread_id) {
            Some(last) => *last,
            None => numbered_screenshots(&directory).into_iter().max().unwrap_or(0),
        };
        inner.screenshot_counters.insert(thread_id.to_owned(), last + 1);
        let file = directory.join(format!("{}.png", last + 1));
        Ok((directory, file))
    }

    /// Captures a window and makes the image its newest shot. Refused while a protected
    /// window overlaps it, asked before the capture and again after it.
    async fn capture(&self, caller: &Caller, window: &WindowRecord, max_size: u32) -> Result<Capture, ComputerUseError> {
        let own = self.guard();
        match self.listing().await {
            Ok(listed) if protected_overlay(&listed, &window.handle, own) => return Ok(Capture::Overlaid),
            Ok(_) => {}
            Err(failure) => return Ok(Capture::Failed(failure)),
        }
        let (directory, file) = self.next_screenshot_path(&caller.thread_id).map_err(|_| internal_error())?;
        let request = DriverRequest::Screenshot { window: window.handle.clone(), output_path: file.to_string_lossy().into_owned(), max_size };
        let image: Screenshot = match self.driver.request(request, None).await {
            Ok(image) => image,
            Err(failure) => return Ok(Capture::Failed(failure)),
        };
        // A protected window may have appeared while the capture ran.
        let after = match self.listing().await {
            Ok(listed) if protected_overlay(&listed, &window.handle, own) => Some(Capture::Overlaid),
            Ok(_) => None,
            Err(failure) => Some(Capture::Failed(failure)),
        };
        if let Some(after) = after {
            let _ = std::fs::remove_file(&file);
            return Ok(after);
        }
        prune_screenshots(&directory);
        let shot = self.with_gate(caller, |gate| gate.targets.record_shot(window.id, NewShot { handle: window.handle.clone(), width: image.width, height: image.height, bounds: image.bounds, max_size }));
        Ok(Capture::Captured(ShotOut { shot: shot.shot, window: window.id, path: file.to_string_lossy().into_owned(), width: shot.width, height: shot.height }))
    }

    async fn screenshot(&self, caller: &Caller, window_id: u64, max_size: Option<u32>) -> Result<Value, ComputerUseError> {
        let window = self.fresh_window(caller, window_id).await?;
        match self.capture(caller, &window, max_size.unwrap_or(DEFAULT_SCREENSHOT_SIZE)).await? {
            Capture::Captured(shot) => {
                let mut result = serde_json::to_value(shot).map_err(|_| internal_error())?;
                result["kind"] = json!("screenshot");
                Ok(result)
            }
            Capture::Overlaid => Err(overlaid()),
            Capture::Failed(failure) => Err(self.with_gate(caller, |gate| observation_failure(&mut gate.targets, &window, &failure))),
        }
    }

    /// Opens an approval and waits for the user. Ends as a cancel when no answer arrives
    /// in time, the turn ends, or the caller stops waiting.
    async fn ask(&self, caller: &Caller, turn: &TurnContext, prompt: ApprovalPrompt) -> AskOutcome {
        let (request_id, mut answer) = {
            let _dispatch = self.dispatch_lock.lock().await;
            let provider_pending = self.host.provider_approval_pending();
            let mut inner = self.inner();
            let Some(request_id) = inner.pending.open(&caller.thread_id, &turn.turn_id, provider_pending) else { return AskOutcome::Paused };
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let approval = PendingApproval { request_id: request_id.clone(), thread_id: caller.thread_id.clone(), turn_id: turn.turn_id.clone(), app_name: APPROVAL_APP_NAME, detail: prompt.detail, options: prompt.options, created_at: crate::storage::now() };
            inner.waiting.insert(request_id.clone(), Waiting { approval, answer: sender });
            (request_id, receiver)
        };
        let _open = OpenApproval { service: self, thread_id: &caller.thread_id, request_id };
        self.host.changed(Some(&caller.thread_id));
        let deadline = tokio::time::Instant::now() + self.options.approval_timeout;
        loop {
            tokio::select! {
                answered = &mut answer => return AskOutcome::Answered(answered.unwrap_or(ApprovalAnswer::Cancel)),
                () = tokio::time::sleep_until(deadline) => return AskOutcome::Answered(ApprovalAnswer::Cancel),
                () = tokio::time::sleep(self.options.turn_poll) => {
                    if self.host.running_turn(&caller.thread_id).map(|running| running.turn_id).as_deref() != Some(turn.turn_id.as_str()) {
                        return AskOutcome::Answered(ApprovalAnswer::Cancel);
                    }
                }
            }
        }
    }

    /// What a coordinate command's first point lands on; best effort.
    async fn hit(&self, request: &InputRequest, resolved: &ResolvedTarget) -> Hit {
        let (Some(point), ResolvedTarget::Shot { shot, .. }) = (hit_point(request, resolved), resolved) else { return Hit::NotApplicable };
        match self.driver.request::<Option<ElementIdentity>>(DriverRequest::ElementAt { window: shot.handle.clone(), point }, None).await {
            Ok(Some(element)) => Hit::Element(element),
            _ => Hit::Nothing,
        }
    }

    async fn input(self: &Arc<Self>, caller: &Caller, request: InputRequest) -> Result<Value, ComputerUseError> {
        // The turn and the approvals setting are read together, before the hit test; the
        // final check reads both again.
        let (begun, turn, approvals) = self.with_live(caller, |live, gate| (begin_input(live, gate, &request), live.turn.cloned(), live.approvals));
        let (resolved, step) = begun?;
        let Some(turn) = turn else { return Err(internal_error()) };
        if let ScreenStep::Ask(prompt) = step {
            let outcome = self.ask(caller, &turn, prompt).await;
            self.with_gate(caller, |gate| gate.answer_screen(&turn.turn_id, outcome))?;
        }
        let hit = self.hit(&request, &resolved).await;
        let decision = self.with_gate(caller, |gate| decide_input(approvals, gate, &turn, &request, &resolved, &hit));
        let (asked, destructive) = (decision.asked(), decision.destructive());
        if let InputDecision::Ask { prompt, .. } = decision {
            let outcome = self.ask(caller, &turn, prompt).await;
            self.with_gate(caller, |gate| gate.answer_approval(&turn.turn_id, destructive, outcome))?;
        }
        let dispatched = self.check_and_dispatch(caller, &request, &turn, &resolved, asked, destructive).await?;
        let mut result = json!({ "kind": "input", "effect": "dispatched", "tookFocus": dispatched.took_focus });
        if let Some(shot) = self.after_action(caller, resolved.window()).await {
            result["screenshot"] = serde_json::to_value(shot).map_err(|_| internal_error())?;
        }
        Ok(result)
    }

    /// The last gate before the desktop, on every input path (asked or auto-allowed).
    /// Checked once here to fail fast, then for the exact native target the helper
    /// reports: before it prepares anything and again immediately before input. That last
    /// check takes the dispatch lock, and an allowed one keeps it until the native action
    /// returned.
    async fn check_and_dispatch(self: &Arc<Self>, caller: &Caller, request: &InputRequest, turn: &TurnContext, resolved: &ResolvedTarget, asked: bool, destructive: bool) -> Result<InputResult, ComputerUseError> {
        let check: Check = {
            let (service, caller, request, resolved, turn_id) = (self.clone(), caller.clone(), request.clone(), resolved.clone(), turn.turn_id.clone());
            Arc::new(move |native| service.with_live(&caller, |live, gate| final_check(live, gate, &CheckedInput { request: &request, turn_id: &turn_id, resolved: &resolved, asked, destructive }, native)))
        };
        check(None)?;
        let Some(driver_request) = driver_request(request, resolved) else { return Err(internal_error()) };
        let authorize: Authorize = {
            let lock = self.dispatch_lock.clone();
            Arc::new(move |native, phase| {
                let (check, lock) = (check.clone(), lock.clone());
                Box::pin(async move {
                    match phase {
                        Phase::Prepare => check(Some(&native)).map(|()| None),
                        Phase::Dispatch => {
                            let permit = lock.lock_owned().await;
                            check(Some(&native)).map(|()| Some(permit))
                        }
                    }
                })
            })
        };
        self.driver.request::<InputResult>(driver_request, Some(authorize)).await.map_err(|failure| self.with_gate(caller, |gate| input_failure(&mut gate.targets, resolved, &failure)))
    }

    /// The window shortly after an action, so the agent continues from what is there now.
    /// Without Screen Recording permission, or when the window went away, the action
    /// still happened and is reported without a picture.
    async fn after_action(&self, caller: &Caller, window: &WindowRecord) -> Option<ShotOut> {
        if !self.options.settle_delay.is_zero() {
            tokio::time::sleep(self.options.settle_delay).await;
        }
        let max_size = self.with_gate(caller, |gate| gate.targets.latest_shot(window.id).map(|shot| shot.max_size)).unwrap_or(DEFAULT_SCREENSHOT_SIZE);
        match self.capture(caller, window, max_size).await {
            Ok(Capture::Captured(shot)) => Some(shot),
            _ => None,
        }
    }

    /// Takes the user's answer to one of the gate's approvals.
    pub(crate) fn respond_to_approval(&self, thread_id: &str, request_id: &str, answer: ApprovalAnswer) -> ApprovalResponse {
        let running = self.host.running_turn(thread_id);
        let mut inner = self.inner();
        let running = inner.gate(thread_id).running_turn(running);
        // Only an answer the card offered, a decline or a cancel is taken as given.
        let offered = inner.waiting.get(request_id).is_none_or(|waiting| waiting.approval.options.iter().any(|option| option.decision == answer));
        let answer = if offered || answer == ApprovalAnswer::Cancel { answer } else { ApprovalAnswer::Decline };
        let (response, deliver) = inner.pending.respond(thread_id, request_id, running.as_ref().map(|turn| turn.turn_id.as_str()), answer);
        let Some(deliver) = deliver else { return response };
        if let Some(waiting) = inner.waiting.remove(request_id) {
            let _ = waiting.answer.send(deliver);
        }
        drop(inner);
        self.host.changed(Some(thread_id));
        response
    }

    /// A turn ended or was interrupted: cancel its approvals, drop its grant and refuse
    /// its remaining input. Without an id, the turn is the one the app shows as running,
    /// or the last one seen here.
    pub(crate) fn end_turn(&self, thread_id: &str, turn_id: Option<&str>) {
        let active = self.host.running_turn(thread_id).map(|turn| turn.turn_id);
        let mut inner = self.inner();
        inner.gate(thread_id).end_turn(turn_id, active.as_deref());
        let cancelled = inner.pending.end_turn(thread_id, turn_id);
        let any = inner.cancel(cancelled);
        drop(inner);
        if any {
            self.host.changed(Some(thread_id));
        }
    }

    /// The thread is gone: forget its windows, refs, shots, grant and approvals.
    pub(crate) fn release_thread(&self, thread_id: &str) {
        let mut inner = self.inner();
        if let Some(gate) = inner.gates.get_mut(thread_id) {
            gate.release();
        }
        let cancelled = inner.pending.release_thread(thread_id);
        let any = inner.cancel(cancelled);
        drop(inner);
        if any {
            self.host.changed(Some(thread_id));
        }
    }

    /// Computer use was turned off: forget every thread's targets and close every approval.
    pub(crate) fn release_all(&self) {
        let mut inner = self.inner();
        for gate in inner.gates.values_mut() {
            gate.release();
        }
        let cancelled = inner.pending.release_all();
        let any = inner.cancel(cancelled);
        drop(inner);
        if any {
            self.host.changed(None);
        }
    }
}

/// The numbers of the `<n>.png` files in a screenshot folder.
fn numbered_screenshots(directory: &std::path::Path) -> Vec<u64> {
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    entries.flatten().filter_map(|entry| entry.file_name().to_str()?.strip_suffix(".png")?.parse().ok()).collect()
}

/// Keeps a thread's newest screenshots: they are the agent's working memory in coordinate mode.
fn prune_screenshots(directory: &std::path::Path) {
    let mut numbered = numbered_screenshots(directory);
    numbered.sort_unstable_by(|left, right| right.cmp(left));
    for number in numbered.into_iter().skip(SCREENSHOTS_KEPT) {
        let _ = std::fs::remove_file(directory.join(format!("{number}.png")));
    }
}

/// Adds the ancestors of `start` to `pids`, up to init, as `parent_of` reports them.
fn extend_ancestry(pids: &mut HashSet<u32>, start: u32, parent_of: impl Fn(u32) -> Option<u32>) {
    const MAX_DEPTH: usize = 32;
    let mut current = start;
    for _ in 0..MAX_DEPTH {
        if current <= 1 {
            break;
        }
        let Some(parent) = parent_of(current).filter(|parent| *parent > 1 && !pids.contains(parent)) else { break };
        pids.insert(parent);
        current = parent;
    }
}

/// A process's parent as `ps` reports it; `None` when it cannot be read.
fn parent_pid(pid: u32) -> Option<u32> {
    if !cfg!(unix) {
        return None;
    }
    let mut command = std::process::Command::new("ps");
    command.args(["-o", "ppid=", "-p", &pid.to_string()]);
    let output = crate::workspace::run_process(command, "ps", 64, Duration::from_secs(2)).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout.bytes).trim().parse().ok()
}

/// The app's own process ancestry: this process, its parent, and further ancestors up to
/// init. A window owned by any of them is the app (or the terminal that started it)
/// whatever its name says. Read once, best effort.
pub(crate) fn own_process_ancestry() -> HashSet<u32> {
    let mut pids = own_process_floor();
    let own = std::process::id();
    if let Some(parent) = pids.iter().copied().find(|pid| *pid != own) {
        extend_ancestry(&mut pids, parent, parent_pid);
    }
    pids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ancestry_walk_stops_at_init_at_a_loop_and_at_a_process_it_cannot_read() {
        let parents: HashMap<u32, u32> = [(50, 40), (40, 30), (30, 1)].into();
        let walk = |start: u32, parents: &HashMap<u32, u32>| {
            let mut pids = HashSet::from([60, start]);
            extend_ancestry(&mut pids, start, |pid| parents.get(&pid).copied());
            let mut pids: Vec<u32> = pids.into_iter().collect();
            pids.sort_unstable();
            pids
        };
        assert_eq!(walk(50, &parents), [30, 40, 50, 60]);
        assert_eq!(walk(77, &parents), [60, 77], "an unreadable parent ends the walk");
        assert_eq!(walk(1, &parents), [1, 60]);
        let looping: HashMap<u32, u32> = [(50, 40), (40, 50)].into();
        assert_eq!(walk(50, &looping), [40, 50, 60]);
        let endless: HashMap<u32, u32> = (2..200).map(|pid| (pid, pid + 1)).collect();
        assert_eq!(walk(2, &endless).len(), 2 + 32, "the walk is bounded");
        // The real walk always holds this process, and its parent where the OS has one.
        let own = own_process_ancestry();
        assert!(own.contains(&std::process::id()));
        assert!(own_process_floor().is_subset(&own));
    }

    #[test]
    fn only_the_newest_screenshots_are_kept() {
        let directory = std::env::temp_dir().join(format!("pandacode-computer-shots-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&directory).unwrap();
        for name in ["1.png", "2.png", "3.png", "10.png", "11.png", "12.png", "13.png", "14.png", "15.png", "16.png", "notes.txt", "x.png"] {
            std::fs::write(directory.join(name), b"").unwrap();
        }
        let mut numbers = numbered_screenshots(&directory);
        numbers.sort_unstable();
        assert_eq!(numbers, [1, 2, 3, 10, 11, 12, 13, 14, 15, 16]);
        prune_screenshots(&directory);
        let mut kept = numbered_screenshots(&directory);
        kept.sort_unstable();
        assert_eq!(kept, [3, 10, 11, 12, 13, 14, 15, 16]);
        assert!(directory.join("notes.txt").exists() && directory.join("x.png").exists(), "other files are left alone");
        assert_eq!(numbered_screenshots(&directory.join("missing")), Vec::<u64>::new());
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
