//! Fake-process proof of the launch gate (docs/managed-mode.md): with a stand-in for every
//! harness binary first on PATH, each recording that it was executed, an agent that is off
//! or blocked by the organization is never run, and one that is on still works.
//!
//! The gate and PATH are process-wide, so the checks run in a child test process: no other
//! test sees the strict gate, and no real agent binary on this computer can be reached
//! (every roster binary name resolves to a stand-in before anything is run).
//!
//! Nothing here waits on a clock. A process having started or ended is observed through a
//! FIFO it holds open, and the timed parts of the app (the update cache, the policy
//! re-read) take their time as an argument and are tested where they live.
#![cfg(unix)]
use super::*;
use crate::launch_gate::{Gate, Policy, Saved};
use std::os::unix::fs::PermissionsExt;

const FIXTURE: &str = "PANDACODE_LAUNCH_GATE_FIXTURE";
const NOT_ENABLED: &str = "is not enabled. Turn it on in Settings → Providers before using it.";
const BLOCKED: &str = "is blocked by your organization, so it was not started.";

/// A stand-in that appends its arguments to `markers/<name>` and answers the few
/// questions a harness is asked without a conversation.
fn stand_in(root: &Path, name: &str) {
    let markers = root.join("markers");
    let body = if name == "claude" {
        // Anything but a question is a session. It holds the FIFO open for writing until it
        // is killed, so a reader sees it start (the open returns) and end (end of file).
        format!("case \"$1\" in --version) echo '9.9.9 (Claude Code)'; exit 0;; --help) echo usage; exit 0;; esac\nexec 3>'{}'\ncat >/dev/null\n", root.join("alive").display())
    } else {
        "case \"$1\" in --version) echo '9.9.9'; exit 0;; models) echo 'anthropic/claude-test'; exit 0;; esac\nexit 0\n".to_owned()
    };
    let path = root.join("bin").join(name);
    std::fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n{body}", markers.join(name).display())).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn an_agent_that_is_off_or_blocked_is_never_executed() {
    let root = std::env::temp_dir().join(format!("pandacode-launch-gate-{}", uuid::Uuid::new_v4()));
    for folder in ["bin", "markers", "home"] {
        std::fs::create_dir_all(root.join(folder)).unwrap();
    }
    // Every binary name the roster can resolve, so no lookup ever lands on a real agent.
    for (id, _) in PROVIDERS {
        stand_in(&root, executable_name(id));
    }
    stand_in(&root, "agy");
    let fifo = std::ffi::CString::new(root.join("alive").to_string_lossy().as_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path in a folder this test owns.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["providers::launch_gate_tests::gated_child", "--exact", "--ignored", "--nocapture"])
        .env(FIXTURE, &root)
        .env("PATH", format!("{}:/usr/bin:/bin", root.join("bin").display()))
        .env("HOME", root.join("home"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn install(enabled: &[&str], blocked: &[&str]) {
    let saved = Saved { enabled: enabled.iter().map(|id| (*id).to_owned()).collect(), ..Saved::default() };
    let policy = Policy { blocked: blocked.iter().map(|id| (*id).to_owned()).collect(), ..Policy::default() };
    crate::launch_gate::install_for_test(Gate { saved, policy, ..Gate::default() }, None);
}

/// The harness binaries executed so far, by stand-in name.
fn executed(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root.join("markers")).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

fn arguments(root: &Path, name: &str) -> String {
    std::fs::read_to_string(root.join("markers").join(name)).unwrap_or_default()
}

async fn turn(provider: &str, session: &str) -> (Result<(), String>, Vec<String>) {
    let events: Arc<Mutex<Vec<String>>> = Default::default();
    let seen = events.clone();
    let sink: Sink = Arc::new(move |event| seen.lock().unwrap().push(format!("{}: {}", event.kind, event.text)));
    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let config = RunConfig { session_id: session.into(), cwd: std::env::temp_dir(), ..tests::config(provider) };
    // A bound on a hang, not a wait: the run returns as soon as the harness answers or exits.
    let result = tokio::time::timeout(Duration::from_secs(120), run(config, sink, rx)).await.expect("the turn ends");
    let events = events.lock().unwrap().clone();
    (result, events)
}

/// A task of `provider` asked to start a turn the way every caller does: the composer, a
/// schedule (`schedules::firing::deliver` → `send_prompt`), a bot, a team role, `serve`
/// and another computer all pass `prepare_task_run`.
fn admission(provider: &str) -> Result<(), String> {
    let (state, project) = crate::concurrent_task_tests::test_state();
    let task = state.store.lock().unwrap().create_session(&project, provider, "default", "ask").unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    crate::prepare_task_run(&state, task.id, Some("Run the nightly check".into()), Vec::new(), tx, Arc::new(std::sync::atomic::AtomicBool::new(false))).map(|_| ())
}

fn warm_config() -> RunConfig {
    RunConfig { session_id: "gate-warm".into(), ..tests::config("claude") }
}

/// Everything the app can do with one harness, each of which must refuse with `reason`.
/// The caller then checks that no stand-in was executed.
async fn never_starts(id: &str, reason: &str) {
    let name = PROVIDERS.iter().find(|(known, _)| *known == id).unwrap().1;
    let refused = format!("{name} {reason}");
    let path = locate(id).unwrap();
    // The one resolver every caller uses: nothing to start, whatever an override says.
    assert_eq!(executable(id), None, "{id}");
    // The command builders, for a caller that brought its own path (a named connection).
    assert_eq!(provider_command(id, &path).err(), Some(refused.clone()));
    assert_eq!(pty_command(id, &path).err(), Some(refused.clone()), "the harness console");
    assert!(version(id, &path).is_none());
    assert_eq!(run_provider(id, &["--version"], Duration::from_secs(5)), None);
    // The model picker.
    assert_eq!(list_model_catalog(id).await.unwrap_err(), refused);
    assert_eq!(list_models(id).await.unwrap_err(), refused);
    // The update check.
    assert!(crate::agent_updates::version_for_test(id, &path).is_err());
    assert!(crate::agent_updates::checked_for_test().iter().all(|(harnesses, _)| !harnesses.iter().any(|harness| harness == id)));
    // Usage.
    crate::agent_usage::invalidate(id);
    if let Ok(usage) = crate::agent_usage::query_usage(id.into(), true).await {
        let usage = serde_json::to_value(usage).unwrap();
        assert_eq!((usage["status"].as_str(), usage["message"].as_str()), (Some("unavailable"), Some(refused.as_str())), "{id}");
    }
    // A turn; and the admission behind schedules, bots, team roles, `serve` and remote starts.
    let (result, events) = turn(id, &format!("gate-{id}")).await;
    assert_eq!(result.unwrap_err(), refused);
    assert!(events.contains(&format!("error: {refused}")), "{events:?}");
    assert_eq!(admission(id).unwrap_err(), refused);
    // A turn started from another computer.
    assert!(crate::remote::require_installed(id).await.unwrap_err().starts_with(&refused));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "started by an_agent_that_is_off_or_blocked_is_never_executed"]
async fn gated_child() {
    let Some(root) = std::env::var_os(FIXTURE).map(PathBuf::from) else { return };
    let bin = root.join("bin");
    for (id, _) in PROVIDERS {
        let path = locate(id).unwrap_or_else(|| panic!("{id} resolves"));
        assert!(path.starts_with(&bin), "{id} resolved outside the fixture: {}", path.display());
    }
    let cwd = std::env::temp_dir();

    // A new install: every agent is off.
    install(&[], &[]);
    // App launch (and `serve`, which shares it): discovery finds the files and runs none.
    let found = crate::local_providers(false).await;
    assert_eq!(found.len(), PROVIDERS.len());
    for info in &found {
        assert!(info.present && !info.enabled && !info.installed, "{}: found on disk, not enabled, not offered", info.id);
        assert_eq!(info.version, if info.id == "antigravity" { "ACP" } else { "" }, "{}", info.id);
        assert!(info.path.is_some());
    }
    assert!(crate::agent_updates::checked_for_test().is_empty(), "no installation is checked for updates");
    for (id, _) in PROVIDERS {
        never_starts(id, NOT_ENABLED).await;
    }
    // Skills and integrations are read by running Codex.
    assert!(crate::integrations::discover(cwd.clone()).await.is_err());
    assert!(crate::integrations::discover_skills(&cwd).await.is_err());
    // Warm-up.
    assert!(prewarm_agent(warm_config(), Vec::new(), HashSet::new(), warm_epoch("gate-warm")).await.unwrap_err().ends_with(NOT_ENABLED));
    assert!(!warm::waiting("gate-warm"));
    assert_eq!(executed(&root), Vec::<String>::new(), "nothing was executed while every agent was off");

    // The user turns on Claude Code and OpenCode. Codex stays off.
    install(&["claude", "opencode"], &[]);
    for info in crate::local_providers(false).await {
        let on = matches!(info.id.as_str(), "claude" | "opencode");
        assert_eq!((info.enabled, info.installed, info.present), (on, on, true), "{}", info.id);
        if on {
            assert!(info.version.starts_with("9.9.9"), "{}: {}", info.id, info.version);
        }
    }
    assert_eq!(executed(&root), ["claude", "opencode"], "an agent's checks start once its switch is on");
    assert!(executable("opencode").is_some_and(|path| path.starts_with(&bin)));
    assert!(list_model_catalog("opencode").await.is_ok());
    assert!(arguments(&root, "opencode").contains("models"), "{}", arguments(&root, "opencode"));
    let mut checked = crate::agent_updates::checked_for_test();
    checked.sort();
    assert_eq!(checked, [(vec!["claude".to_owned()], true), (vec!["opencode".to_owned()], true)]);
    assert!(admission("claude").is_ok());
    let before = arguments(&root, "opencode").lines().count();
    let (result, _) = turn("opencode", "gate-opencode").await;
    assert!(result.is_err(), "the stand-in is not an agent");
    assert!(arguments(&root, "opencode").lines().count() > before, "an enabled agent is started for its turn");
    never_starts("codex", NOT_ENABLED).await;
    assert!(crate::integrations::discover_skills(&cwd).await.is_err());

    // A warm agent starts for an agent that is on, and is killed when it is turned off.
    prewarm_agent(warm_config(), Vec::new(), HashSet::new(), warm_epoch("gate-warm")).await.unwrap();
    assert!(warm::waiting("gate-warm"));
    let alive = root.join("alive");
    // Returns once the warm stand-in has opened its end: it is running.
    let mut reader = tokio::task::spawn_blocking(move || std::fs::File::open(alive).unwrap()).await.unwrap();
    install(&["opencode"], &[]);
    discard_disabled_warm_agents();
    assert!(!warm::waiting("gate-warm"));
    // Returns at end of file, which is when every process holding the other end is gone.
    let read = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        reader.read_to_end(&mut Vec::new())
    });
    let read = tokio::time::timeout(Duration::from_secs(60), read).await.expect("the warm process was killed").unwrap();
    assert_eq!(read.unwrap(), 0);
    assert!(prewarm_agent(warm_config(), Vec::new(), HashSet::new(), warm_epoch("gate-warm")).await.is_err());
    assert_eq!(executed(&root), ["claude", "opencode"]);

    // The organization blocks Codex and Claude Code; both switches are on. Blocking the
    // `claude` binary also blocks the relays that run it, although their own switch is on.
    install(&["codex", "claude", "glm", "opencode"], &["codex", "claude"]);
    let runs_before = (arguments(&root, "claude"), arguments(&root, "opencode"));
    for info in crate::local_providers(false).await {
        let on = info.id == "opencode";
        assert_eq!((info.enabled, info.installed, info.present), (on, on, true), "{}", info.id);
    }
    for id in ["codex", "claude", "glm"] {
        never_starts(id, BLOCKED).await;
    }
    assert!(crate::integrations::discover(cwd.clone()).await.is_err());
    assert!(prewarm_agent(warm_config(), Vec::new(), HashSet::new(), warm_epoch("gate-warm")).await.unwrap_err().ends_with(BLOCKED));
    assert_eq!(arguments(&root, "claude"), runs_before.0, "a blocked agent is not run even with its switch on");
    assert!(arguments(&root, "opencode").len() > runs_before.1.len(), "what the policy leaves alone still works");

    assert_eq!(executed(&root), ["claude", "opencode"], "codex was never executed, off or blocked");
}
