//! The app's side of the native helper (`native/computer-driver`): starts it as a child
//! process and speaks `protocol.rs` over its stdin and stdout.
//!
//! Why a child process: the OS accessibility API can crash or hang inside a misbehaving
//! app, and that must not take the app down. The app starts the helper itself, so macOS
//! attributes it to the app and it uses the app's Accessibility and Screen Recording
//! grants.
//!
//! Calls are serialized. A helper that dies, times out or answers out of step is killed
//! and a fresh one starts on the next call; its handles die with it, which the service
//! sees as `stale`. For input the helper asks the gate before it prepares a target and
//! again immediately before native input ([`Authorize`]); a crash, timeout or malformed
//! reply after an allowed `dispatch` is reported as `dispatched: unknown`, before one
//! nothing was sent.
//!
//! A call whose caller gave up while it was still queued is never sent. Once sent, it runs
//! to its reply even if the caller is gone: a check still to come is refused, but input
//! already allowed may be landing, and the dispatch lock stays held until it returns.
//!
//! The helper is replaced after `recycle_after` requests so one long-lived process cannot
//! grow without bound, and after a `status` that does not report Accessibility granted,
//! because macOS reads a process's grant once.

use super::policy::{cancelled_before_dispatch, ComputerUseError};
use super::protocol::{
    decode_result, platform_supported, read_line, unsupported_reason, Accessibility, CallOutcome, DecisionLine, DispatchTarget, Dispatched, DriverFailure, DriverLine, DriverRequest, DriverStatus, FailureKind, Phase, RequestLine,
    DEFAULT_RECYCLE_AFTER, DRIVER_EPOCH_ENV, DRIVER_NONCE_ENV,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::Instant;

/// An explicit helper path, for a helper built somewhere else (a source checkout builds
/// it with `cargo build --release` in `native/computer-driver`).
pub(crate) const BINARY_ENV: &str = "VIEWCODE_COMPUTER_DRIVER_BINARY";
/// The helper's file name, as its crate builds it.
pub(crate) const BINARY_NAME: &str = "viewcode-computer-driver";
/// What status says when the helper is not where the app looks for it.
pub(crate) const HELPER_MISSING: &str = "The computer-use helper is not installed with this build.";

/// Where the helper may be, most specific first. A path that was given is the only
/// candidate; otherwise it is looked for in the app's bundled resources and beside the
/// app's own executable.
pub(crate) fn binary_candidates(override_path: Option<&str>, resource_dir: Option<&Path>, executable: Option<&Path>) -> Vec<PathBuf> {
    if let Some(given) = override_path.map(str::trim).filter(|given| !given.is_empty()) {
        return vec![PathBuf::from(given)];
    }
    let mut candidates = Vec::new();
    if let Some(resources) = resource_dir {
        candidates.push(resources.join("computer-driver").join(BINARY_NAME));
    }
    if let Some(directory) = executable.and_then(Path::parent) {
        candidates.push(directory.join(BINARY_NAME));
    }
    candidates
}

/// The first helper that exists.
pub(crate) fn find_binary(candidates: &[PathBuf], exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    candidates.iter().find(|candidate| exists(candidate)).cloned()
}

/// How to start one helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Launch {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
}

/// The gate's lock on the desktop: held from an allowed `dispatch` check until the
/// helper's next line, which it writes only once that input returned.
pub(crate) type DispatchPermit = tokio::sync::OwnedMutexGuard<()>;
/// `Ok` allows; an allowed `dispatch` check carries the permit to hold.
pub(crate) type Decision = Result<Option<DispatchPermit>, ComputerUseError>;
pub(crate) type DecisionFuture = Pin<Box<dyn Future<Output = Decision> + Send>>;
/// The gate's check of the native target the helper just read.
pub(crate) type Authorize = Arc<dyn Fn(DispatchTarget, Phase) -> DecisionFuture + Send + Sync>;

pub(crate) struct DriverOptions {
    /// A `std::env::consts::OS` value; the helper is only ever started on macOS.
    pub(crate) os: &'static str,
    /// Resolved when a helper starts, so an unused driver costs nothing. `None` when the
    /// helper is not installed.
    pub(crate) launch: Box<dyn Fn() -> Option<Launch> + Send + Sync>,
    /// Per-op timeouts in milliseconds, replacing `DriverRequest::timeout_ms`.
    pub(crate) timeouts: HashMap<&'static str, u64>,
    /// Requests one helper serves before it is replaced.
    pub(crate) recycle_after: u32,
}

impl DriverOptions {
    pub(crate) fn new(os: &'static str, launch: impl Fn() -> Option<Launch> + Send + Sync + 'static) -> Self {
        Self { os, launch: Box::new(launch), timeouts: HashMap::new(), recycle_after: DEFAULT_RECYCLE_AFTER }
    }
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    nonce: String,
    served: u32,
}

#[derive(Default)]
struct State {
    worker: Option<Worker>,
    next_id: u64,
    /// The last helper died after it was allowed to press a mouse button.
    release_mouse: bool,
    /// Helpers started so far.
    started: u32,
}

impl State {
    /// Kills the helper and forgets it; its buffered lines are dropped with it, so they
    /// can never answer the next helper's request.
    fn discard(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            let _ = worker.child.start_kill();
            // Reaped in the background; `kill_on_drop` covers a runtime that is going away.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = worker.child.wait().await;
                });
            }
        }
    }
}

pub(crate) struct DriverClient {
    options: DriverOptions,
    /// Held for a whole call: the queue.
    state: Arc<tokio::sync::Mutex<State>>,
    closed: AtomicBool,
}

/// Marks a call as given up when its caller goes away before it finished.
struct GiveUp {
    flag: Arc<AtomicBool>,
    answered: bool,
}

impl Drop for GiveUp {
    fn drop(&mut self) {
        if !self.answered {
            self.flag.store(true, Ordering::SeqCst);
        }
    }
}

fn random_hex(bytes: usize) -> String {
    let mut text = String::new();
    while text.len() < bytes * 2 {
        text.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    text.truncate(bytes * 2);
    text
}

impl DriverClient {
    pub(crate) fn new(options: DriverOptions) -> Arc<Self> {
        Arc::new(Self { options, state: Arc::new(tokio::sync::Mutex::new(State { next_id: 1, ..State::default() })), closed: AtomicBool::new(false) })
    }

    /// Why no helper can run here, without starting one: the platform, or a missing
    /// helper. `None` when a call would start (or reuse) a helper.
    pub(crate) fn unavailable_reason(&self) -> Option<String> {
        if !platform_supported(self.options.os) {
            return Some(unsupported_reason(self.options.os).to_owned());
        }
        match (self.options.launch)() {
            Some(_) => None,
            None => Some(HELPER_MISSING.to_owned()),
        }
    }

    /// How many helpers have been started; a restart shows as a higher number.
    pub(crate) async fn helpers_started(&self) -> u32 {
        self.state.lock().await.started
    }

    /// Stops the helper for good: later calls are answered as unavailable. A call in
    /// flight keeps its helper until it returns; the helper also exits when the app's end
    /// of its stdin closes.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Ok(mut state) = self.state.try_lock() {
            state.discard();
        }
    }

    fn timeout(&self, request: &DriverRequest) -> Duration {
        Duration::from_millis(self.options.timeouts.get(request.op()).copied().unwrap_or_else(|| request.timeout_ms()))
    }

    fn start(&self) -> Option<Worker> {
        let launch = (self.options.launch)()?;
        let nonce = random_hex(32);
        let mut command = Command::new(&launch.program);
        command.args(&launch.args).env(DRIVER_NONCE_ENV, &nonce).env(DRIVER_EPOCH_ENV, random_hex(4)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(true);
        crate::child_env::sanitize(&mut command);
        let mut child = command.spawn().ok()?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.start_kill();
            return None;
        };
        Some(Worker { child, stdin, lines: BufReader::new(stdout).lines(), nonce, served: 0 })
    }

    /// One request on the current helper, starting one if needed. The state lock is held
    /// by the caller for the whole call.
    async fn run(&self, state: &mut State, request: &DriverRequest, authorize: Option<&Authorize>, given_up: &AtomicBool) -> Result<Value, DriverFailure> {
        let input = request.is_input();
        if self.closed.load(Ordering::SeqCst) {
            return Err(CallOutcome::NotSent.failure(input, false));
        }
        if state.worker.is_none() {
            state.worker = self.start();
            if state.worker.is_some() {
                state.started += 1;
            }
        }
        let Some(worker) = state.worker.as_mut() else { return Err(CallOutcome::NotSent.failure(input, false)) };
        let id = state.next_id;
        state.next_id += 1;
        let deadline = Instant::now() + self.timeout(request);
        let line = RequestLine { id, nonce: worker.nonce.clone(), request: request.clone() }.to_line();
        if write_line(&mut worker.stdin, &line).await.is_err() {
            state.discard();
            return Err(CallOutcome::NotSent.failure(input, false));
        }
        // An allowed `dispatch` went to the helper, so input may have run.
        let mut authorized = false;
        // The dispatch lock behind that decision, released at the helper's next line.
        let mut held: Option<DispatchPermit> = None;
        let outcome = loop {
            let read = tokio::time::timeout_at(deadline, worker.lines.next_line()).await;
            drop(held.take());
            let line = match read {
                Err(_) => break CallOutcome::Timeout,
                Ok(Ok(Some(line))) => line,
                Ok(_) => break CallOutcome::Exited,
            };
            match read_line(&line, id) {
                DriverLine::Reply(result) => {
                    worker.served += 1;
                    let recycle = worker.served >= self.options.recycle_after || (matches!(request, DriverRequest::Status) && !reports_granted(&result));
                    if recycle {
                        state.discard();
                    }
                    return result;
                }
                DriverLine::Malformed => break CallOutcome::Malformed,
                DriverLine::Authorize { target, phase } => {
                    let decision = match authorize {
                        Some(authorize) if !given_up.load(Ordering::SeqCst) => match tokio::time::timeout_at(deadline, authorize(target, phase)).await {
                            Ok(decision) => decision,
                            Err(_) => break CallOutcome::Timeout,
                        },
                        _ => Err(cancelled_before_dispatch()),
                    };
                    // The caller may have gone away while the gate was deciding.
                    let decision = if given_up.load(Ordering::SeqCst) { decision.and(Err(cancelled_before_dispatch())) } else { decision };
                    let decision = decision.map(|permit| {
                        if phase == Phase::Dispatch {
                            authorized = true;
                            held = permit;
                        }
                    });
                    if write_line(&mut worker.stdin, &DecisionLine::new(id, &worker.nonce, decision).to_line()).await.is_err() {
                        break CallOutcome::Exited;
                    }
                }
            }
        };
        drop(held);
        state.discard();
        if authorized && request.holds_mouse_button() {
            state.release_mouse = true;
        }
        Err(outcome.failure(input, authorized))
    }

    /// Sends one request and returns the helper's `result`, undecoded.
    pub(crate) async fn call(self: &Arc<Self>, request: DriverRequest, authorize: Option<Authorize>) -> Result<Value, DriverFailure> {
        if !platform_supported(self.options.os) {
            return Err(DriverFailure::new(FailureKind::Unavailable, unsupported_reason(self.options.os), Dispatched::No));
        }
        let input = request.is_input();
        let given_up = Arc::new(AtomicBool::new(false));
        let mut guard = GiveUp { flag: given_up.clone(), answered: false };
        let client = self.clone();
        // Its own task: a request that was sent runs to its reply whatever happens to the caller.
        let task = tokio::spawn(async move {
            let mut state = client.state.clone().lock_owned().await;
            if given_up.load(Ordering::SeqCst) {
                return Err(CallOutcome::NotSent.failure(input, false));
            }
            if state.release_mouse {
                state.release_mouse = false;
                let _ = client.run(&mut state, &DriverRequest::ReleaseMouse, None, &AtomicBool::new(false)).await;
                if given_up.load(Ordering::SeqCst) {
                    return Err(CallOutcome::NotSent.failure(input, false));
                }
            }
            client.run(&mut state, &request, authorize.as_ref(), &given_up).await
        });
        let result = task.await.unwrap_or_else(|_| Err(CallOutcome::Exited.failure(input, false)));
        // Reached only when the caller stayed for the answer.
        guard.answered = true;
        result
    }

    /// Sends one request and decodes its result for the op that was sent.
    pub(crate) async fn request<T: DeserializeOwned>(self: &Arc<Self>, request: DriverRequest, authorize: Option<Authorize>) -> Result<T, DriverFailure> {
        let input = request.is_input();
        let result = self.call(request, authorize).await?;
        // A reply that arrived means the helper finished: a result of the wrong shape
        // after input says the input may have run.
        decode_result(result, input, input)
    }

    /// Never fails: a helper that cannot answer is reported as unavailable, with why.
    pub(crate) async fn status(self: &Arc<Self>) -> DriverStatus {
        if let Some(reason) = self.unavailable_reason() {
            return DriverStatus::unavailable(reason);
        }
        match self.request::<DriverStatus>(DriverRequest::Status, None).await {
            Ok(status) => status,
            Err(failure) => DriverStatus::unavailable(failure.message),
        }
    }
}

async fn write_line(stdin: &mut ChildStdin, line: &str) -> std::io::Result<()> {
    stdin.write_all(line.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await
}

fn reports_granted(result: &Result<Value, DriverFailure>) -> bool {
    result.as_ref().ok().and_then(|result| serde_json::from_value::<DriverStatus>(result.clone()).ok()).is_some_and(|status| status.accessibility == Accessibility::Granted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helper_is_looked_for_where_it_was_given_then_in_the_bundle() {
        let resources = Path::new("/bundle/Resources");
        let executable = Path::new("/bundle/MacOS/app");
        assert_eq!(binary_candidates(Some(" /somewhere/helper "), Some(resources), Some(executable)), [PathBuf::from("/somewhere/helper")]);
        let candidates = binary_candidates(Some("  "), Some(resources), Some(executable));
        assert_eq!(candidates, [resources.join("computer-driver").join(BINARY_NAME), Path::new("/bundle/MacOS").join(BINARY_NAME)]);
        assert_eq!(binary_candidates(None, None, None), Vec::<PathBuf>::new());
        assert_eq!(find_binary(&candidates, |path| path.starts_with("/bundle/MacOS")), Some(candidates[1].clone()));
        assert_eq!(find_binary(&candidates, |_| true), Some(candidates[0].clone()));
        assert_eq!(find_binary(&candidates, |_| false), None);
        assert_eq!((BINARY_ENV, BINARY_NAME), ("VIEWCODE_COMPUTER_DRIVER_BINARY", "viewcode-computer-driver"));
    }

    #[tokio::test]
    async fn nothing_starts_where_the_helper_cannot_run() {
        for (os, reason) in [("linux", "Computer use is not supported on Linux yet."), ("windows", "Computer use is not supported on Windows yet.")] {
            let driver = DriverClient::new(DriverOptions::new(os, || panic!("no helper is looked for on an unsupported platform")));
            assert_eq!(driver.unavailable_reason().as_deref(), Some(reason));
            assert_eq!(driver.status().await, DriverStatus::unavailable(reason));
            let failure = driver.request::<Vec<super::super::protocol::DriverWindow>>(DriverRequest::ListWindows, None).await.unwrap_err();
            assert_eq!((failure.kind, failure.message.as_str(), failure.dispatched), (FailureKind::Unavailable, reason, Dispatched::No));
            assert_eq!(driver.helpers_started().await, 0);
        }
        // On macOS without the helper: unavailable, and a call says it could not start.
        let driver = DriverClient::new(DriverOptions::new("macos", || None));
        assert_eq!(driver.status().await, DriverStatus::unavailable(HELPER_MISSING));
        let failure = driver.call(DriverRequest::Focus { window: "w1".into() }, None).await.unwrap_err();
        assert_eq!((failure.kind, failure.message.as_str(), failure.dispatched), (FailureKind::Unavailable, "The computer-use driver could not start.", Dispatched::No));
        // A helper that is not a program at all.
        let driver = DriverClient::new(DriverOptions::new("macos", || Some(Launch { program: std::env::temp_dir().join("pandacode-no-such-helper"), args: Vec::new() })));
        assert_eq!(driver.unavailable_reason(), None);
        assert_eq!(driver.status().await, DriverStatus::unavailable("The computer-use driver could not start."));
        assert_eq!(driver.helpers_started().await, 0);
        assert_eq!(random_hex(32).len(), 64);
        assert_eq!(random_hex(4).len(), 8);
    }
}
