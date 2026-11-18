//! Keeps the computer awake while an agent turn runs, and only then (docs/managed-mode.md).
//!
//! A Mac that goes to sleep when its screen locks cuts an agent's connection mid-turn (a
//! Cursor turn ends with `RetriableError: [unavailable] PING timed out`). While at least one
//! turn runs, this holds one macOS power assertion (`PreventUserIdleSystemSleep`, what
//! `caffeinate -i` holds) through IOKit, in this process: no helper is started. It is
//! released when the last turn ends, however it ends, and by the system if the app dies.
//!
//! The limit: this stops idle sleep, not forced sleep. Closing the lid on battery, a low
//! battery or the Sleep menu still put the Mac to sleep, and the display may still turn off.
//!
//! Other platforms hold nothing; the counting is the same everywhere and is what the tests
//! exercise with a fake.
use crate::Result;
use serde::Serialize;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const FILE: &str = "keep-awake.json";
pub(crate) const CHANGED: &str = "keep-awake-changed";

/// The platform's way of holding the computer awake.
pub(crate) trait Assertion: Send {
    /// Starts holding; true when the computer is now kept awake. Called only while not holding.
    fn hold(&mut self) -> bool;
    /// Stops holding. Called only after a successful `hold`.
    fn release(&mut self);
    /// Whether this platform can hold anything at all.
    fn supported(&self) -> bool;
}

/// Which turns are running and whether the assertion is held for them.
pub(crate) struct KeepAwake<A: Assertion> {
    assertion: A,
    /// Running turns by task id. A set, so ending a turn twice (a stop and then the run's
    /// own end) cannot release an assertion another turn still needs.
    turns: HashSet<String>,
    held: bool,
    /// The user's setting.
    enabled: bool,
    /// The organization's policy (`keepAwake.allowed`).
    permitted: bool,
}

impl<A: Assertion> KeepAwake<A> {
    pub(crate) fn new(assertion: A, enabled: bool, permitted: bool) -> Self {
        Self { assertion, turns: HashSet::new(), held: false, enabled, permitted }
    }

    /// Holds exactly while a turn runs and both the user and the policy allow it.
    fn settle(&mut self) {
        let wanted = self.enabled && self.permitted && !self.turns.is_empty();
        if wanted && !self.held {
            self.held = self.assertion.hold();
        } else if !wanted && self.held {
            self.assertion.release();
            self.held = false;
        }
    }

    pub(crate) fn begin(&mut self, turn: &str) {
        self.turns.insert(turn.to_owned());
        self.settle();
    }

    pub(crate) fn end(&mut self, turn: &str) {
        self.turns.remove(turn);
        self.settle();
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.settle();
    }

    /// Forgets every turn and releases: the app is exiting.
    pub(crate) fn clear(&mut self) {
        self.turns.clear();
        self.settle();
    }

    pub(crate) fn state(&self) -> State {
        State { enabled: self.enabled, permitted: self.permitted, supported: self.assertion.supported(), active: self.held, turns: self.turns.len() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct State {
    /// The user's setting (on by default).
    enabled: bool,
    /// False when the organization's policy turns keep-awake off; the setting then has no effect.
    permitted: bool,
    /// False where this platform has nothing to hold.
    supported: bool,
    /// The computer is being kept awake right now.
    active: bool,
    /// Turns running now.
    turns: usize,
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{c_char, c_void, CStr};

    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const LEVEL_ON: u32 = 255;
    const SUCCESS: i32 = 0;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(allocator: *const c_void, text: *const c_char, encoding: u32) -> CFStringRef;
        fn CFRelease(value: *const c_void);
    }
    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(kind: CFStringRef, level: u32, name: CFStringRef, id: *mut u32) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    /// One IOKit power assertion, visible to the user and to IT in `pmset -g assertions`
    /// under this app's name.
    pub(super) struct Power(Option<u32>);

    impl Power {
        pub(super) fn new() -> Self {
            Self(None)
        }
    }

    fn string(text: &CStr) -> CFStringRef {
        // SAFETY: `text` is a NUL-terminated UTF-8 string; a null allocator is the default one.
        unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) }
    }

    impl super::Assertion for Power {
        fn hold(&mut self) -> bool {
            let (kind, name) = (string(c"PreventUserIdleSystemSleep"), string(c"An agent turn is running"));
            let mut id = 0u32;
            // SAFETY: both strings are valid or null (checked first), `id` outlives the call,
            // and each created string is released exactly once.
            let created = unsafe {
                let created = !kind.is_null() && !name.is_null() && IOPMAssertionCreateWithName(kind, LEVEL_ON, name, &mut id) == SUCCESS;
                for value in [kind, name] {
                    if !value.is_null() {
                        CFRelease(value);
                    }
                }
                created
            };
            self.0 = created.then_some(id);
            created
        }

        fn release(&mut self) {
            if let Some(id) = self.0.take() {
                // SAFETY: `id` came from IOPMAssertionCreateWithName and is released once.
                unsafe { IOPMAssertionRelease(id) };
            }
        }

        fn supported(&self) -> bool {
            true
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    /// Nothing to hold off macOS.
    pub(super) struct Power;

    impl Power {
        pub(super) fn new() -> Self {
            Self
        }
    }

    impl super::Assertion for Power {
        fn hold(&mut self) -> bool {
            false
        }
        fn release(&mut self) {}
        fn supported(&self) -> bool {
            false
        }
    }
}

struct Runtime {
    path: Option<PathBuf>,
    keeper: KeepAwake<platform::Power>,
}

static RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();

fn runtime() -> std::sync::MutexGuard<'static, Runtime> {
    RUNTIME
        .get_or_init(|| Mutex::new(Runtime { path: None, keeper: KeepAwake::new(platform::Power::new(), true, true) }))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The saved setting; on unless the file says `{"enabled": false}`.
fn saved(path: &Path) -> bool {
    std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok()).and_then(|value| value["enabled"].as_bool()).unwrap_or(true)
}

/// Loads the setting and the policy. Call after `launch_gate::setup`.
pub(crate) fn setup(dir: &Path) {
    let path = dir.join(FILE);
    let mut runtime = runtime();
    runtime.keeper.enabled = saved(&path);
    runtime.keeper.permitted = crate::launch_gate::keep_awake_permitted();
    runtime.path = Some(path);
    runtime.keeper.settle();
}

/// A running turn. The computer is kept awake until every one of these is dropped: at the
/// turn's end, on Stop, on an error, on a panic and when the run's task is cancelled.
pub(crate) struct Turn(String);

impl Drop for Turn {
    fn drop(&mut self) {
        runtime().keeper.end(&self.0);
    }
}

pub(crate) fn turn(id: &str) -> Turn {
    runtime().keeper.begin(id);
    Turn(id.to_owned())
}

/// Releases at app exit. The system would release it with the process anyway.
pub(crate) fn shutdown() {
    runtime().keeper.clear();
}

pub(crate) fn state() -> State {
    runtime().keeper.state()
}

fn save(enabled: bool) -> Result<State> {
    let mut runtime = runtime();
    if let Some(path) = &runtime.path {
        std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({ "enabled": enabled })).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    }
    runtime.keeper.set_enabled(enabled);
    Ok(runtime.keeper.state())
}

#[tauri::command]
pub(crate) async fn keep_awake_state() -> Result<State> {
    Ok(state())
}

#[tauri::command]
pub(crate) async fn keep_awake_set_enabled(app: tauri::AppHandle, enabled: bool) -> Result<State> {
    use tauri::Emitter;
    let next = crate::blocking(move || save(enabled)).await?;
    let _ = app.emit(CHANGED, &next);
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};

    /// Counts holds and releases; `available` false is a platform that refuses.
    #[derive(Clone, Default)]
    struct Fake {
        holds: Arc<AtomicUsize>,
        releases: Arc<AtomicUsize>,
        unavailable: bool,
    }
    impl Assertion for Fake {
        fn hold(&mut self) -> bool {
            self.holds.fetch_add(1, Ordering::SeqCst);
            !self.unavailable
        }
        fn release(&mut self) {
            self.releases.fetch_add(1, Ordering::SeqCst);
        }
        fn supported(&self) -> bool {
            !self.unavailable
        }
    }
    fn counts(fake: &Fake) -> (usize, usize) {
        (fake.holds.load(Ordering::SeqCst), fake.releases.load(Ordering::SeqCst))
    }

    #[test]
    fn the_computer_is_kept_awake_only_while_a_turn_runs() {
        let fake = Fake::default();
        let mut keeper = KeepAwake::new(fake.clone(), true, true);
        assert_eq!(counts(&fake), (0, 0), "nothing is held while idle");
        assert!(!keeper.state().active);
        keeper.begin("a");
        assert_eq!(counts(&fake), (1, 0));
        assert!(keeper.state().active);
        // More turns share the one assertion.
        keeper.begin("b");
        keeper.begin("c");
        assert_eq!(counts(&fake), (1, 0));
        assert_eq!(keeper.state().turns, 3);
        keeper.end("a");
        keeper.end("c");
        assert_eq!(counts(&fake), (1, 0), "still held for the turn that is left");
        keeper.end("b");
        assert_eq!(counts(&fake), (1, 1), "released when the last turn ends");
        assert!(!keeper.state().active);
        // The next turn holds again.
        keeper.begin("a");
        keeper.end("a");
        assert_eq!(counts(&fake), (2, 2));
    }

    #[test]
    fn ending_a_turn_twice_or_one_that_never_began_releases_nothing_extra() {
        let fake = Fake::default();
        let mut keeper = KeepAwake::new(fake.clone(), true, true);
        keeper.begin("a");
        keeper.begin("b");
        // Stop and then the run's own end both report the same turn.
        keeper.end("a");
        keeper.end("a");
        keeper.end("never-started");
        assert_eq!(counts(&fake), (1, 0), "b still holds the computer awake");
        // The same task starting again while its old run unwinds is one turn, not two.
        keeper.begin("b");
        keeper.end("b");
        assert_eq!(counts(&fake), (1, 1));
        keeper.end("b");
        assert_eq!(counts(&fake), (1, 1));
    }

    #[test]
    fn the_setting_and_the_policy_both_have_to_allow_it() {
        let fake = Fake::default();
        let mut keeper = KeepAwake::new(fake.clone(), false, true);
        keeper.begin("a");
        assert_eq!(counts(&fake), (0, 0), "off: never held");
        // Turned on mid-turn: held from now; turned off mid-turn: released at once.
        keeper.set_enabled(true);
        assert_eq!(counts(&fake), (1, 0));
        keeper.set_enabled(false);
        assert_eq!(counts(&fake), (1, 1));
        keeper.end("a");
        assert_eq!(counts(&fake), (1, 1));

        let fake = Fake::default();
        let mut keeper = KeepAwake::new(fake.clone(), true, false);
        keeper.begin("a");
        keeper.set_enabled(true);
        assert_eq!(counts(&fake), (0, 0), "the organization's policy outranks the setting");
        assert_eq!(keeper.state(), State { enabled: true, permitted: false, supported: true, active: false, turns: 1 });
    }

    #[test]
    fn app_exit_releases_whatever_is_held() {
        let fake = Fake::default();
        let mut keeper = KeepAwake::new(fake.clone(), true, true);
        keeper.begin("a");
        keeper.begin("b");
        keeper.clear();
        assert_eq!(counts(&fake), (1, 1));
        assert_eq!(keeper.state().turns, 0);
        keeper.clear();
        assert_eq!(counts(&fake), (1, 1));
    }

    #[test]
    fn a_platform_that_cannot_hold_is_asked_again_for_the_next_turn_and_never_released() {
        let fake = Fake { unavailable: true, ..Fake::default() };
        let mut keeper = KeepAwake::new(fake.clone(), true, true);
        keeper.begin("a");
        assert!(!keeper.state().active);
        assert!(!keeper.state().supported);
        keeper.end("a");
        assert_eq!(counts(&fake).1, 0, "nothing was held, so nothing is released");
    }

    #[test]
    fn a_dropped_turn_guard_ends_its_turn_even_when_the_run_panics() {
        // The process-wide keeper, with whatever this platform holds (nothing on Linux).
        let id = format!("keep-awake-test-{}", uuid::Uuid::new_v4());
        let run = std::panic::catch_unwind(|| {
            let _turn = turn(&id);
            assert!(state().turns > 0);
            panic!("the run crashed");
        });
        assert!(run.is_err());
        // Other tests' turns may come and go; this one's is gone.
        assert!(!runtime().keeper.turns.contains(&id));
        drop(turn(&id));
        assert!(!runtime().keeper.turns.contains(&id));
    }

    #[test]
    fn the_setting_is_on_unless_saved_off() {
        let dir = std::env::temp_dir().join(format!("pandacode-keep-awake-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        assert!(saved(&path), "no file: on");
        std::fs::write(&path, r#"{"enabled": false}"#).unwrap();
        assert!(!saved(&path));
        std::fs::write(&path, "not json").unwrap();
        assert!(saved(&path), "an unreadable file is the default");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
