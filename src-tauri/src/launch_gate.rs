//! The launch gate: the one decision on whether an agent CLI may be started
//! (docs/managed-mode.md).
//!
//! Security software on company-managed computers can react to an agent binary merely
//! being run, so nothing here is about how a harness is started: it is about not starting
//! it. A harness runs only when the user turned it on and no organization policy forbids
//! it. Every command for a harness binary is built by `providers/launch.rs`, whose builders
//! call [`allow`] first; `launch_gate/scan_tests.rs` fails when a new spawn site appears
//! outside that path.
//!
//! The gate knows no harness by name. It is keyed by whatever id the caller passes, so a
//! harness added later is covered (and off) without a change here.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::RwLock,
};

pub(crate) mod claude;
mod policy;
#[cfg(test)]
mod scan_tests;
#[cfg(test)]
mod tests;

pub(crate) use policy::Policy;

const FILE: &str = "launch-gate.json";
const MAX_FILE: u64 = 64 * 1024;
/// Emitted with the new [`GateState`] whenever the enabled set or the first-run choice changes.
pub(crate) const CHANGED: &str = "launch-gate-changed";

/// Why a harness was not started. `Display` is the user-facing sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The gate has not loaded its state: nothing may start.
    NotReady,
    /// A policy file exists but could not be read, so what it forbids is unknown.
    PolicyUnreadable,
    /// The organization's policy names this harness (or the binary it runs) as not allowed.
    Blocked,
    /// The organization's policy lists the allowed harnesses and this is not one.
    NotInAllowedSet,
    /// The user has not turned this harness on.
    NotEnabled,
}

impl Refused {
    /// The sentence shown for `name` (a harness's display name).
    pub(crate) fn message(&self, name: &str) -> String {
        match self {
            Self::NotReady => format!("{name} was not started: agent settings have not loaded yet."),
            Self::PolicyUnreadable => format!("{name} was not started: your organization's policy file could not be read, so no agent runs until it is fixed."),
            Self::Blocked => format!("{name} is blocked by your organization, so it was not started."),
            Self::NotInAllowedSet => format!("{name} is not one of the agents your organization allows, so it was not started."),
            Self::NotEnabled => format!("{name} is not enabled. Turn it on in Settings → Providers before using it."),
        }
    }

    /// A stable word for the UI.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::NotReady => "notReady",
            Self::PolicyUnreadable => "policyUnreadable",
            Self::Blocked => "blocked",
            Self::NotInAllowedSet => "notAllowed",
            Self::NotEnabled => "notEnabled",
        }
    }
}

/// What the user chose, as saved in `launch-gate.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Saved {
    #[serde(default)]
    pub(crate) version: u32,
    /// Harness ids the user turned on.
    #[serde(default)]
    pub(crate) enabled: BTreeSet<String>,
    /// The first-run choice has not been made: the welcome step still has to ask.
    #[serde(default)]
    pub(crate) first_run_pending: bool,
    /// Harnesses this profile already used when the gate first loaded (threads or saved
    /// settings). On a strict machine they are only suggested, never turned on.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) suggested: BTreeSet<String>,
}

/// The decision data: the user's choice under the organization's policy. Pure, so every
/// rule is tested without a process or a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Gate {
    pub(crate) saved: Saved,
    pub(crate) policy: Policy,
    /// Evidence that this computer is managed by an organization (macOS only).
    pub(crate) managed: Vec<String>,
}

impl Gate {
    /// Strict mode: nothing is turned on for the user, not even what the profile used
    /// before. The default on a managed computer; a policy can set it either way.
    pub(crate) fn strict(&self) -> bool {
        self.policy.strict.unwrap_or(!self.managed.is_empty() || self.policy.source.is_some())
    }

    /// Whether the policy lets the user turn `harness` on. `binary` is the executable name
    /// the harness runs, so blocking `claude` also blocks the relays that run its binary.
    pub(crate) fn permitted(&self, harness: &str, binary: &str) -> std::result::Result<(), Refused> {
        if self.policy.problem.is_some() {
            return Err(Refused::PolicyUnreadable);
        }
        if self.policy.blocked.contains(harness) || self.policy.blocked.contains(binary) {
            return Err(Refused::Blocked);
        }
        if self.policy.allowed.as_ref().is_some_and(|allowed| !allowed.contains(harness)) {
            return Err(Refused::NotInAllowedSet);
        }
        Ok(())
    }

    /// The launch decision. Policy first: a saved choice never outranks it.
    pub(crate) fn check(&self, harness: &str, binary: &str) -> std::result::Result<(), Refused> {
        self.permitted(harness, binary)?;
        if !self.saved.enabled.contains(harness) {
            return Err(Refused::NotEnabled);
        }
        Ok(())
    }

    /// The first state of a profile that has no saved choice. `used` is what the profile
    /// already has threads or settings for.
    ///
    /// The rule: off a strict machine, every used harness the policy permits starts on and
    /// everything else off; the first-run choice is pending only when nothing was used (a
    /// fresh profile). On a strict machine nothing starts on, the choice is pending, and the
    /// used harnesses are only remembered as suggestions.
    pub(crate) fn initial(policy: Policy, managed: Vec<String>, used: BTreeSet<String>, binary: impl Fn(&str) -> String) -> Self {
        let mut gate = Self { saved: Saved { version: 1, ..Saved::default() }, policy, managed };
        let permitted: BTreeSet<String> = used.into_iter().filter(|id| gate.permitted(id, &binary(id)).is_ok()).collect();
        if gate.strict() {
            gate.saved.first_run_pending = true;
        } else {
            gate.saved.first_run_pending = permitted.is_empty();
            gate.saved.enabled = permitted.clone();
        }
        gate.saved.suggested = permitted;
        gate
    }
}

struct Runtime {
    /// None when the state lives in memory only (tests).
    path: Option<PathBuf>,
    gate: Gate,
    /// Re-reads the policy when its files change. None in tests.
    watch: Option<policy::Watch>,
}

static RUNTIME: RwLock<Option<Runtime>> = RwLock::new(None);

fn read<T>(f: impl FnOnce(Option<&Runtime>) -> T) -> T {
    let runtime = RUNTIME.read().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(runtime.as_ref())
}

/// A harness's display name from the roster, else its id.
fn name(harness: &str) -> String {
    crate::providers::PROVIDERS.iter().find(|(id, _)| *id == harness).map_or_else(|| harness.to_owned(), |(_, name)| (*name).to_owned())
}

/// The launch decision for `harness`, read at the moment of the launch. Fails closed: with
/// no loaded state nothing starts. (Unit tests that never install a gate are not about it
/// and build commands freely; `providers/launch_gate_tests.rs` installs one in a process of
/// its own.)
pub(crate) fn check(harness: &str) -> std::result::Result<(), Refused> {
    let binary = crate::providers::binary_name(harness);
    read(|runtime| match runtime {
        Some(runtime) => runtime.gate.check(harness, binary),
        None if cfg!(test) => Ok(()),
        None => Err(Refused::NotReady),
    })
}

/// [`check`] with the user-facing sentence, for the many callers that return `String` errors.
pub(crate) fn allow(harness: &str) -> std::result::Result<(), String> {
    check(harness).map_err(|refused| refused.message(&name(harness)))
}

/// Whether `harness` may be started right now. For loops that skip instead of failing.
pub(crate) fn enabled(harness: &str) -> bool {
    check(harness).is_ok()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HarnessState {
    id: String,
    name: String,
    /// From a file lookup only; the binary is never run to answer this.
    installed: bool,
    path: Option<String>,
    enabled: bool,
    /// Whether the user may change `enabled`. False when the organization decides.
    changeable: bool,
    /// Why it cannot be turned on: `blocked`, `notAllowed` or `policyUnreadable`.
    #[serde(skip_serializing_if = "Option::is_none")]
    locked: Option<&'static str>,
    /// The profile used this harness before the gate existed.
    suggested: bool,
}

/// Everything the UI needs to show and change the gate.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GateState {
    /// Harness ids that may be started now (turned on and permitted).
    enabled: Vec<String>,
    first_run_pending: bool,
    /// This computer looks managed by an organization, or a policy is installed.
    managed: bool,
    managed_evidence: Vec<String>,
    strict: bool,
    /// The policy file in force, if any.
    policy_source: Option<String>,
    policy_problem: Option<String>,
    /// The policy's allow list, when it pins one: nothing else can be turned on.
    pinned: Option<Vec<String>>,
    blocked: Vec<String>,
    harnesses: Vec<HarnessState>,
}

fn state_of(gate: &Gate, locate: &dyn Fn(&str) -> Option<PathBuf>) -> GateState {
    let harnesses: Vec<HarnessState> = crate::providers::PROVIDERS
        .iter()
        .map(|(id, name)| {
            let permitted = gate.permitted(id, crate::providers::binary_name(id));
            let path = locate(id);
            HarnessState {
                id: (*id).into(),
                name: (*name).into(),
                installed: path.is_some(),
                path: path.map(|path| path.to_string_lossy().into_owned()),
                enabled: permitted.is_ok() && gate.saved.enabled.contains(*id),
                changeable: permitted.is_ok(),
                locked: permitted.err().map(|refused| refused.code()),
                suggested: gate.saved.suggested.contains(*id),
            }
        })
        .collect();
    GateState {
        enabled: harnesses.iter().filter(|harness| harness.enabled).map(|harness| harness.id.clone()).collect(),
        first_run_pending: gate.saved.first_run_pending,
        managed: !gate.managed.is_empty() || gate.policy.source.is_some(),
        managed_evidence: gate.managed.clone(),
        strict: gate.strict(),
        policy_source: gate.policy.source.clone(),
        policy_problem: gate.policy.problem.clone(),
        pinned: gate.policy.allowed.as_ref().map(|allowed| allowed.iter().cloned().collect()),
        blocked: gate.policy.blocked.iter().cloned().collect(),
        harnesses,
    }
}

/// The gate as the UI sees it. Looks files up; runs nothing.
pub(crate) fn state() -> Result<GateState> {
    read(|runtime| runtime.map(|runtime| state_of(&runtime.gate, &|id| crate::providers::locate(id))).ok_or_else(|| "Agent settings have not loaded yet.".into()))
}

fn load(path: &Path) -> std::result::Result<Option<Saved>, String> {
    let Ok(metadata) = std::fs::metadata(path) else { return Ok(None) };
    if metadata.len() > MAX_FILE {
        return Err("The saved agent choice is too large to read.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map(Some).map_err(|_| "The saved agent choice could not be read.".into())
}

fn write(path: &Path, saved: &Saved) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(saved).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("json.tmp-{}", uuid::Uuid::new_v4().simple()));
    std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        e.to_string()
    })
}

/// Builds the gate for a profile folder: the saved choice, else the upgrade rule
/// ([`Gate::initial`]) saved once. A choice that cannot be read or saved leaves every
/// harness off for this run, and the unreadable file is kept as it is.
fn open(dir: &Path, policy: Policy, managed: Vec<String>, used: impl FnOnce() -> BTreeSet<String>) -> Runtime {
    let path = dir.join(FILE);
    let off = |policy: Policy, managed: Vec<String>, path: Option<PathBuf>| Runtime {
        path,
        watch: None,
        gate: Gate { saved: Saved { version: 1, first_run_pending: true, ..Saved::default() }, policy, managed },
    };
    match load(&path) {
        Ok(Some(saved)) => Runtime { path: Some(path), watch: None, gate: Gate { saved, policy, managed } },
        Ok(None) => {
            let gate = Gate::initial(policy.clone(), managed.clone(), used(), |id| crate::providers::binary_name(id).to_owned());
            match write(&path, &gate.saved) {
                Ok(()) => Runtime { path: Some(path), watch: None, gate },
                Err(error) => {
                    eprintln!("Could not save the agent choice, so no agent is enabled for this run: {error}");
                    off(policy, managed, Some(path))
                }
            }
        }
        Err(error) => {
            eprintln!("{error} No agent is enabled until one is turned on in Settings.");
            off(policy, managed, Some(path))
        }
    }
}

/// Loads the gate at startup. Reads files only: the policy, the saved choice and, for a
/// profile without one, which harnesses it already has threads or settings for.
pub(crate) fn setup(dir: &Path, store: &crate::storage::Store) {
    let watch = policy::Watch::new(policy::paths(), now_ms());
    let policy = watch.policy();
    if let Some(problem) = &policy.problem {
        eprintln!("{problem}");
    }
    let mut runtime = open(dir, policy, policy::managed_evidence(), || used_harnesses(store));
    runtime.watch = Some(watch);
    *RUNTIME.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(runtime);
}

/// Milliseconds on a clock that only moves forward, for the policy watch.
fn now_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// Applies a policy file that changed since it was last read. `now` is the watch's clock;
/// the files are looked at no more often than `policy::INTERVAL_MS`. Returns the new state
/// when the policy changed.
fn refresh_policy(now: u64) -> Option<GateState> {
    let mut runtime = RUNTIME.write().unwrap_or_else(|poisoned| poisoned.into_inner());
    let runtime = runtime.as_mut()?;
    let policy = runtime.watch.as_mut()?.poll(now)?;
    if policy == runtime.gate.policy {
        return None;
    }
    if let Some(problem) = &policy.problem {
        eprintln!("{problem}");
    }
    runtime.gate.policy = policy;
    Some(state_of(&runtime.gate, &|id| crate::providers::locate(id)))
}

/// Keeps the policy current while the app runs: a block an administrator installs takes
/// effect within a minute, stops what it forbids and tells the UI.
pub(crate) fn watch(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(policy::INTERVAL_MS));
        loop {
            tick.tick().await;
            let handle = app.clone();
            let next = crate::blocking(move || {
                let next = refresh_policy(now_ms());
                if next.is_some() {
                    use tauri::Manager;
                    stop_disabled(&handle.state::<crate::AppState>());
                }
                Ok(next)
            })
            .await;
            if let Ok(Some(next)) = next {
                changed(&app, &next);
            }
        }
    });
}

/// Harnesses this profile already uses: a thread with at least one message, or anything
/// saved for it in provider settings or a named connection. An empty draft is not a choice.
fn used_harnesses(store: &crate::storage::Store) -> BTreeSet<String> {
    let mut used: BTreeSet<String> = store
        .db
        .prepare("SELECT DISTINCT provider FROM sessions s WHERE EXISTS(SELECT 1 FROM messages m WHERE m.session_id=s.id)")
        .and_then(|mut statement| statement.query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<_>>())
        .unwrap_or_default();
    used.extend(crate::provider_settings::configured_providers());
    used.extend(crate::connections::providers_in_use());
    used.retain(|id| crate::providers::PROVIDERS.iter().any(|(known, _)| known == id));
    used
}

fn known(harness: &str) -> Result<()> {
    if crate::providers::PROVIDERS.iter().any(|(id, _)| *id == harness) {
        Ok(())
    } else {
        Err("Unknown agent.".into())
    }
}

/// Applies `change` to the saved choice and persists it; memory changes only once the file did.
fn update(change: impl FnOnce(&Gate) -> Result<Saved>) -> Result<GateState> {
    let mut runtime = RUNTIME.write().unwrap_or_else(|poisoned| poisoned.into_inner());
    let runtime = runtime.as_mut().ok_or("Agent settings have not loaded yet.")?;
    let next = change(&runtime.gate)?;
    if let Some(path) = &runtime.path {
        write(path, &next)?;
    }
    runtime.gate.saved = next;
    Ok(state_of(&runtime.gate, &|id| crate::providers::locate(id)))
}

/// One harness turned on or off. Turning one on while the first-run choice is pending is
/// that choice: the rest stay off. A harness the policy forbids cannot be turned on.
fn chosen(gate: &Gate, harness: &str, enabled: bool) -> Result<Saved> {
    known(harness)?;
    let mut next = gate.saved.clone();
    if enabled {
        gate.permitted(harness, crate::providers::binary_name(harness)).map_err(|refused| refused.message(&name(harness)))?;
        next.enabled.insert(harness.to_owned());
    } else {
        next.enabled.remove(harness);
    }
    next.first_run_pending = false;
    Ok(next)
}

/// The first-run choice: exactly these harnesses on, everything else off. Refused once a
/// choice exists, so a stale welcome screen cannot undo later changes.
fn first_run(gate: &Gate, enabled: &[String]) -> Result<Saved> {
    if !gate.saved.first_run_pending {
        return Err("The agents were already chosen. Change them in Settings → Providers.".into());
    }
    for harness in enabled {
        known(harness)?;
        gate.permitted(harness, crate::providers::binary_name(harness)).map_err(|refused| refused.message(&name(harness)))?;
    }
    Ok(Saved { version: 1, enabled: enabled.iter().cloned().collect(), first_run_pending: false, suggested: gate.saved.suggested.clone() })
}

pub(crate) fn set_enabled(harness: &str, enabled: bool) -> Result<GateState> {
    update(|gate| chosen(gate, harness, enabled))
}

pub(crate) fn complete_first_run(enabled: &[String]) -> Result<GateState> {
    update(|gate| first_run(gate, enabled))
}

/// Replaces the gate for this process. Only `providers/launch_gate_tests.rs` uses it, in a child
/// process, so no other test sees a strict gate.
#[cfg(test)]
pub(crate) fn install_for_test(gate: Gate, dir: Option<&Path>) {
    *RUNTIME.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Runtime { path: dir.map(|dir| dir.join(FILE)), gate, watch: None });
}

/// Everything already running for harnesses that just became unavailable is stopped: warm
/// agents and consoles are killed (processes this app started), running turns get Stop.
/// Stopping is the safer side of "finish or stop": a turn left running would keep spawning
/// the binary the user or the organization just turned off.
fn stop_disabled(state: &crate::AppState) {
    crate::providers::discard_disabled_warm_agents();
    let disabled_task = |id: &str| {
        let provider = state.store.lock().ok().and_then(|store| store.session(id).ok()).map(|task| task.provider);
        // A task that cannot be read is stopped too: the gate fails closed.
        provider.is_none_or(|provider| !enabled(&provider))
    };
    let running: Vec<String> = state.agents.lock().map(|agents| agents.keys().cloned().collect()).unwrap_or_default();
    for id in running.into_iter().filter(|id| !crate::cloud_agents::transport::is_cloud(id)).filter(|id| disabled_task(id)) {
        if let Ok(store) = state.store.lock() {
            let _ = store.request_stop(&id);
        }
        if let Some(agent) = state.agents.lock().ok().and_then(|agents| agents.get(&id).map(|agent| agent.input.clone())) {
            let _ = agent.send(crate::providers::RunInput::Stop);
        }
    }
    for (terminal, _) in crate::terminal::harness_consoles(&state.terminals).into_iter().filter(|(_, session)| disabled_task(session)) {
        let _ = crate::terminal::close(&state.terminals, &terminal);
    }
}

fn changed(app: &tauri::AppHandle, state: &GateState) {
    use tauri::Emitter;
    // Cached answers about a harness describe the state before the change.
    crate::agent_updates::forget();
    for (id, _) in crate::providers::PROVIDERS {
        crate::agent_usage::invalidate(id);
    }
    let _ = app.emit(CHANGED, state);
}

#[tauri::command]
pub(crate) async fn launch_gate_state() -> Result<GateState> {
    crate::blocking(state).await
}

#[tauri::command]
pub(crate) async fn launch_gate_set_enabled(app: tauri::AppHandle, harness: String, enabled: bool) -> Result<GateState> {
    let handle = app.clone();
    let next = crate::blocking(move || {
        let next = set_enabled(&harness, enabled)?;
        if !enabled {
            use tauri::Manager;
            stop_disabled(&handle.state::<crate::AppState>());
        }
        Ok(next)
    })
    .await?;
    changed(&app, &next);
    Ok(next)
}

#[tauri::command]
pub(crate) async fn launch_gate_complete_first_run(app: tauri::AppHandle, enabled: Vec<String>) -> Result<GateState> {
    let next = crate::blocking(move || complete_first_run(&enabled)).await?;
    changed(&app, &next);
    Ok(next)
}
