use super::policy::{managed_evidence_in, Policy, BUNDLE_ID};
use super::*;
use std::collections::BTreeSet;

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn binary(harness: &str) -> String {
    crate::providers::binary_name(harness).to_owned()
}

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/launch_gate/fixtures").join(name)
}

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("pandacode-launch-gate-unit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn gate(enabled: &[&str], policy: Policy) -> Gate {
    Gate { saved: Saved { version: 1, enabled: set(enabled), first_run_pending: false, suggested: BTreeSet::new() }, policy, managed: Vec::new() }
}

#[test]
fn a_fresh_profile_enables_nothing_and_waits_for_the_first_run_choice() {
    let gate = Gate::initial(Policy::default(), Vec::new(), BTreeSet::new(), binary);
    assert!(gate.saved.enabled.is_empty());
    assert!(gate.saved.first_run_pending);
    assert!(!gate.strict());
    for (id, _) in crate::providers::PROVIDERS {
        assert_eq!(gate.check(id, &binary(id)), Err(Refused::NotEnabled), "{id}");
    }
    // An id no roster knows yet is refused the same way: the gate has no list of its own.
    assert_eq!(gate.check("a-harness-added-later", "later"), Err(Refused::NotEnabled));
}

#[test]
fn an_upgraded_profile_keeps_the_harnesses_it_used_and_nothing_else() {
    let gate = Gate::initial(Policy::default(), Vec::new(), set(&["claude", "codex"]), binary);
    assert_eq!(gate.saved.enabled, set(&["claude", "codex"]));
    assert!(!gate.saved.first_run_pending, "an upgraded profile is not asked again");
    assert_eq!(gate.check("claude", "claude"), Ok(()));
    assert_eq!(gate.check("codex", "codex"), Ok(()));
    assert_eq!(gate.check("opencode", "opencode"), Err(Refused::NotEnabled));
    // Installed is not used: GLM runs the same binary as Claude and still stays off.
    assert_eq!(gate.check("glm", "claude"), Err(Refused::NotEnabled));
}

#[test]
fn a_managed_computer_is_strict_and_turns_nothing_on_by_itself() {
    let evidence = vec!["Managed preferences are installed.".to_owned()];
    let gate = Gate::initial(Policy::default(), evidence.clone(), set(&["claude", "codex"]), binary);
    assert!(gate.strict());
    assert!(gate.saved.enabled.is_empty(), "a used harness is only suggested on a managed computer");
    assert!(gate.saved.first_run_pending);
    assert_eq!(gate.saved.suggested, set(&["claude", "codex"]));
    assert_eq!(gate.check("codex", "codex"), Err(Refused::NotEnabled));
    // A policy can say this managed computer is not strict.
    let relaxed = Gate::initial(Policy { strict: Some(false), ..Policy::default() }, evidence, set(&["claude"]), binary);
    assert!(!relaxed.strict());
    assert_eq!(relaxed.saved.enabled, set(&["claude"]));
    // And an unmanaged computer can be made strict by a policy file alone.
    let by_policy = Gate::initial(Policy { source: Some("/etc/viewcode/policy.json".into()), ..Policy::default() }, Vec::new(), set(&["claude"]), binary);
    assert!(by_policy.strict());
    assert!(by_policy.saved.enabled.is_empty());
}

#[test]
fn a_policy_outranks_the_saved_choice() {
    let blocked = Policy { blocked: set(&["codex", "claude"]), ..Policy::default() };
    let gate = gate(&["codex", "claude", "glm", "opencode"], blocked);
    assert_eq!(gate.check("codex", "codex"), Err(Refused::Blocked));
    // Blocking a binary by name blocks every harness that runs it.
    assert_eq!(gate.check("glm", "claude"), Err(Refused::Blocked));
    assert_eq!(gate.check("mimo", "claude"), Err(Refused::Blocked));
    assert_eq!(gate.check("opencode", "opencode"), Ok(()));
    assert_eq!(gate.permitted("codex", "codex"), Err(Refused::Blocked));

    let pinned = Policy { allowed: Some(set(&["claude"])), ..Policy::default() };
    let gate = self::gate(&["codex", "claude"], pinned);
    assert_eq!(gate.check("claude", "claude"), Ok(()));
    assert_eq!(gate.check("codex", "codex"), Err(Refused::NotInAllowedSet), "enabled before the pin, refused after");
    assert_eq!(gate.check("a-harness-added-later", "later"), Err(Refused::NotInAllowedSet));
    // Pinned is "may be turned on", not "is on".
    assert_eq!(self::gate(&[], Policy { allowed: Some(set(&["claude"])), ..Policy::default() }).check("claude", "claude"), Err(Refused::NotEnabled));

    let unreadable = Policy { problem: Some("could not be read".into()), ..Policy::default() };
    assert_eq!(self::gate(&["claude"], unreadable).check("claude", "claude"), Err(Refused::PolicyUnreadable));
}

#[test]
fn the_upgrade_rule_never_enables_what_the_policy_forbids() {
    let policy = Policy { strict: Some(false), blocked: set(&["codex"]), allowed: Some(set(&["claude", "codex"])), ..Policy::default() };
    let gate = Gate::initial(policy, Vec::new(), set(&["claude", "codex", "opencode"]), binary);
    assert_eq!(gate.saved.enabled, set(&["claude"]));
    assert_eq!(gate.saved.suggested, set(&["claude"]));
}

#[test]
fn refusals_say_what_to_do() {
    assert_eq!(Refused::NotEnabled.message("Codex"), "Codex is not enabled. Turn it on in Settings → Providers before using it.");
    assert!(Refused::Blocked.message("Codex").contains("blocked by your organization"));
    assert!(Refused::NotInAllowedSet.message("Codex").contains("not one of the agents your organization allows"));
    assert!(Refused::PolicyUnreadable.message("Codex").contains("could not be read"));
    assert!(Refused::NotReady.message("Codex").contains("not loaded"));
}

#[test]
fn a_policy_reads_the_same_from_json_and_from_either_kind_of_plist() {
    let expected = Policy {
        source: None,
        strict: Some(true),
        blocked: set(&["codex", "grok"]),
        allowed: Some(set(&["claude", "cursor", "commandcode"])),
        keep_awake: false,
        problem: None,
    };
    for name in ["policy.json", "policy.xml.plist", "policy.binary.plist"] {
        let path = fixture(name);
        let policy = Policy::read(&path).expect(name);
        assert_eq!(policy.source.as_deref(), Some(path.to_string_lossy().as_ref()), "{name}");
        assert_eq!(Policy { source: None, ..policy }, expected, "{name}");
    }
}

#[test]
fn a_block_list_alone_pins_nothing_and_unknown_keys_are_ignored() {
    let policy = Policy::read(&fixture("blocklist.json")).unwrap();
    assert_eq!(policy.blocked, set(&["codex"]));
    assert_eq!(policy.allowed, None);
    assert_eq!(policy.strict, None);
    assert!(policy.keep_awake);
    assert_eq!(policy.problem, None);
}

#[test]
fn a_policy_that_cannot_be_applied_stops_every_agent_and_a_missing_one_changes_nothing() {
    for name in ["malformed.json", "truncated.plist"] {
        let policy = Policy::read(&fixture(name)).expect(name);
        let problem = policy.problem.clone().expect(name);
        assert!(problem.contains("could not be applied"), "{problem}");
        assert!(policy.blocked.is_empty() && policy.allowed.is_none());
        let gate = gate(&["claude"], policy);
        assert_eq!(gate.check("claude", "claude"), Err(Refused::PolicyUnreadable), "{name}");
        assert!(gate.strict(), "{name}");
    }
    assert_eq!(Policy::read(&fixture("no-such-policy.json")), None);
    assert_eq!(Policy::merged(&[fixture("no-such-policy.json")]), Policy::default());
}

#[test]
fn of_several_policy_sources_the_stricter_wins() {
    let dir = Temp::new();
    let write = |name: &str, text: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    let file = write("policy.json", r#"{"providers":{"codex":{"allowed":false}},"allowedProviders":["claude","opencode","codex"],"strict":false}"#);
    let mdm = write("mdm.json", r#"{"providers":{"opencode":{"allowed":false}},"allowedProviders":["claude","kimi"],"strict":true,"keepAwake":{"allowed":false}}"#);
    let merged = Policy::merged(&[file.clone(), dir.path().join("absent.json"), mdm.clone()]);
    assert_eq!(merged.blocked, set(&["codex", "opencode"]), "a block in either source blocks");
    assert_eq!(merged.allowed, Some(set(&["claude"])), "only what every allow list names");
    assert_eq!(merged.strict, Some(true));
    assert!(!merged.keep_awake);
    assert_eq!(merged.problem, None);
    assert!(merged.source.as_deref().is_some_and(|source| source.contains("policy.json") && source.contains("mdm.json")));
    // One source alone keeps its own answer.
    assert_eq!(Policy::merged(std::slice::from_ref(&file)).allowed, Some(set(&["claude", "opencode", "codex"])));
    // One unreadable source stops everything, whatever the other says.
    let broken = write("broken.json", "{ not json");
    let gate = gate(&["claude"], Policy::merged(&[file, broken]));
    assert_eq!(gate.check("claude", "claude"), Err(Refused::PolicyUnreadable));
}

#[test]
fn a_policy_file_is_read_again_when_it_changes_on_the_watch_clock() {
    let dir = Temp::new();
    let path = dir.path().join("policy.json");
    let interval = super::policy::INTERVAL_MS;
    let mut watch = super::policy::Watch::new(vec![path.clone()], 1_000);
    assert_eq!(watch.policy(), Policy::default(), "no file at launch: no policy");
    // An administrator installs a block while the app runs.
    std::fs::write(&path, r#"{"providers":{"codex":{"allowed":false}}}"#).unwrap();
    assert_eq!(watch.poll(1_000 + interval - 1), None, "not looked at before the interval has passed");
    let installed = watch.poll(1_000 + interval).expect("the new file is noticed");
    assert_eq!(installed.blocked, set(&["codex"]));
    assert_eq!(watch.poll(1_000 + 2 * interval), None, "an unchanged file is not read again");
    // It is edited, then broken, then removed.
    std::fs::write(&path, r#"{"providers":{"codex":{"allowed":false},"opencode":{"allowed":false}}}"#).unwrap();
    assert_eq!(watch.poll(1_000 + 3 * interval).unwrap().blocked, set(&["codex", "opencode"]));
    std::fs::write(&path, "{").unwrap();
    assert!(watch.poll(1_000 + 4 * interval).unwrap().problem.is_some());
    std::fs::remove_file(&path).unwrap();
    assert_eq!(watch.poll(1_000 + 5 * interval), Some(Policy::default()));
}

#[test]
fn the_policy_is_read_for_this_apps_bundle_identifier() {
    let conf: serde_json::Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
    assert_eq!(conf["identifier"], BUNDLE_ID);
}

#[test]
fn managed_preferences_or_claude_administrator_files_mark_a_mac_as_managed() {
    let root = Temp::new();
    assert!(managed_evidence_in(root.path()).is_empty());
    let preferences = root.path().join("Library/Managed Preferences");
    std::fs::create_dir_all(&preferences).unwrap();
    std::fs::write(preferences.join(".DS_Store"), "").unwrap();
    assert!(managed_evidence_in(root.path()).is_empty(), "an empty folder is not evidence");
    std::fs::write(preferences.join("com.example.vpn.plist"), "").unwrap();
    assert_eq!(managed_evidence_in(root.path()).len(), 1);
    let claude = root.path().join("Library/Application Support/ClaudeCode");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join("managed-mcp.json"), "{}").unwrap();
    let evidence = managed_evidence_in(root.path());
    assert_eq!(evidence.len(), 2);
    assert!(evidence[1].contains("managed-mcp.json"));
}

#[test]
fn claude_administrator_files_change_how_claude_is_started() {
    let dir = Temp::new();
    let dirs = vec![dir.path().to_owned()];
    assert!(!claude::mcp_managed_in(&dirs));
    assert!(!claude::bypass_disabled_in(&dirs));
    std::fs::write(dir.path().join("managed-mcp.json"), "{}").unwrap();
    assert!(claude::mcp_managed_in(&dirs));
    for (settings, disabled) in [
        (r#"{"permissions":{"disableBypassPermissionsMode":"disable"}}"#, true),
        (r#"{"disableBypassPermissionsMode":"disable"}"#, true),
        (r#"{"permissions":{"disableBypassPermissionsMode":"enable"}}"#, false),
        (r#"{"allowManagedHooksOnly":true}"#, false),
        ("not json", false),
    ] {
        std::fs::write(dir.path().join("managed-settings.json"), settings).unwrap();
        assert_eq!(claude::bypass_disabled_in(&dirs), disabled, "{settings}");
    }
    // The mapping the task run and the console share.
    claude::DIRECTORIES.with(|directories| *directories.borrow_mut() = Some(Vec::new()));
    assert_eq!(claude::permission_mode("full"), "bypassPermissions");
    assert!(claude::allows_bypass("full"));
    assert!(!claude::allows_bypass("auto"));
    std::fs::write(dir.path().join("managed-settings.json"), r#"{"permissions":{"disableBypassPermissionsMode":"disable"}}"#).unwrap();
    claude::DIRECTORIES.with(|directories| *directories.borrow_mut() = Some(dirs));
    assert_eq!(claude::permission_mode("full"), "acceptEdits");
    assert!(!claude::allows_bypass("full"));
    for (mode, expected) in [("auto", "acceptEdits"), ("review", "auto"), ("plan", "plan"), ("ask", "default")] {
        assert_eq!(claude::permission_mode(mode), expected);
    }
    assert!(claude::mcp_managed());
    claude::DIRECTORIES.with(|directories| *directories.borrow_mut() = None);
}

#[test]
fn a_profile_without_a_saved_choice_gets_the_upgrade_rule_once_and_keeps_it() {
    let dir = Temp::new();
    let opened = open(dir.path(), Policy::default(), Vec::new(), || set(&["claude"]));
    assert_eq!(opened.gate.saved.enabled, set(&["claude"]));
    assert!(!opened.gate.saved.first_run_pending);
    // The decision is saved: a thread created later in another harness does not turn it on.
    let again = open(dir.path(), Policy::default(), Vec::new(), || panic!("the upgrade rule runs once"));
    assert_eq!(again.gate.saved, opened.gate.saved);
    // A fresh profile stays pending across restarts until a choice is made.
    let fresh = Temp::new();
    assert!(open(fresh.path(), Policy::default(), Vec::new(), BTreeSet::new).gate.saved.first_run_pending);
    assert!(open(fresh.path(), Policy::default(), Vec::new(), || set(&["codex"])).gate.saved.first_run_pending);
    assert!(open(fresh.path(), Policy::default(), Vec::new(), || set(&["codex"])).gate.saved.enabled.is_empty());
}

#[test]
fn an_unreadable_or_unsavable_choice_leaves_everything_off() {
    let dir = Temp::new();
    let file = dir.path().join(FILE);
    std::fs::write(&file, "{ not json").unwrap();
    let opened = open(dir.path(), Policy::default(), Vec::new(), || set(&["claude"]));
    assert!(opened.gate.saved.enabled.is_empty());
    assert!(opened.gate.saved.first_run_pending);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json", "the unreadable file is left for inspection");
    // A profile folder that cannot be written: the upgrade rule's answer is not trusted for the run.
    let missing = dir.path().join("no-such-folder");
    let opened = open(&missing, Policy::default(), Vec::new(), || set(&["claude"]));
    assert!(opened.gate.saved.enabled.is_empty());
    assert!(opened.gate.saved.first_run_pending);
}

#[test]
fn threads_with_messages_and_saved_settings_count_as_used() {
    let store = crate::storage::Store::open(std::path::Path::new(":memory:")).unwrap();
    let project = store.add_project(&std::env::temp_dir()).unwrap();
    let used = store.create_session(&project.id, "codex", "default", "ask").unwrap();
    store.message(&used.id, "user", "hello").unwrap();
    // A thread nobody wrote in is not a choice.
    store.create_session(&project.id, "opencode", "default", "ask").unwrap();
    // Provider settings and connections are process-wide and other tests may have loaded
    // some: only the two harnesses of this store are asserted.
    let used = used_harnesses(&store);
    assert!(used.contains("codex") && !used.contains("opencode"), "{used:?}");
}

#[test]
fn the_state_lists_every_harness_with_what_the_user_may_change() {
    let policy = Policy { source: Some("/etc/viewcode/policy.json".into()), blocked: set(&["codex"]), allowed: Some(set(&["claude", "codex"])), ..Policy::default() };
    let mut gate = gate(&["claude", "codex", "opencode"], policy);
    gate.saved.first_run_pending = true;
    gate.saved.suggested = set(&["claude"]);
    let state = state_of(&gate, &|id| (id == "claude").then(|| "/fake/bin/claude".into()));
    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(json["enabled"], serde_json::json!(["claude"]));
    assert_eq!(json["firstRunPending"], true);
    assert_eq!(json["managed"], true);
    assert_eq!(json["strict"], true);
    assert_eq!(json["policySource"], "/etc/viewcode/policy.json");
    assert_eq!(json["pinned"], serde_json::json!(["claude", "codex"]));
    assert_eq!(json["blocked"], serde_json::json!(["codex"]));
    let harness = |id: &str| json["harnesses"].as_array().unwrap().iter().find(|harness| harness["id"] == id).unwrap().clone();
    assert_eq!(json["harnesses"].as_array().unwrap().len(), crate::providers::PROVIDERS.len());
    assert_eq!(harness("claude"), serde_json::json!({"id":"claude","name":"Claude Code","installed":true,"path":"/fake/bin/claude","enabled":true,"changeable":true,"suggested":true}));
    assert_eq!(harness("codex")["locked"], "blocked");
    assert_eq!(harness("codex")["enabled"], false);
    assert_eq!(harness("opencode")["locked"], "notAllowed");
    assert_eq!(harness("opencode")["installed"], false);
    assert_eq!(harness("opencode")["changeable"], false);
}

#[test]
fn the_first_run_choice_is_made_once_and_a_settings_switch_counts_as_it() {
    let mut pending = gate(&[], Policy::default());
    pending.saved.first_run_pending = true;
    let chose = first_run(&pending, &["claude".into(), "opencode".into()]).unwrap();
    assert_eq!(chose.enabled, set(&["claude", "opencode"]));
    assert!(!chose.first_run_pending);
    // "Continue without agents" is a choice too.
    let none = first_run(&pending, &[]).unwrap();
    assert!(none.enabled.is_empty() && !none.first_run_pending);
    assert_eq!(first_run(&pending, &["not-a-harness".into()]).unwrap_err(), "Unknown agent.");
    // A second welcome screen cannot undo what Settings changed since.
    let decided = Gate { saved: chose, ..pending.clone() };
    assert!(first_run(&decided, &["codex".into()]).unwrap_err().contains("already chosen"));
    // One switch in Settings while pending: that harness on, the rest off, no longer pending.
    let switched = chosen(&pending, "codex", true).unwrap();
    assert_eq!(switched.enabled, set(&["codex"]));
    assert!(!switched.first_run_pending);
    let off = chosen(&Gate { saved: switched, ..pending.clone() }, "codex", false).unwrap();
    assert!(off.enabled.is_empty());
}

#[test]
fn the_user_cannot_enable_beyond_the_policy() {
    let mut pinned = gate(&[], Policy { allowed: Some(set(&["claude"])), blocked: set(&["grok"]), ..Policy::default() });
    pinned.saved.first_run_pending = true;
    assert!(chosen(&pinned, "codex", true).unwrap_err().contains("not one of the agents your organization allows"));
    assert!(chosen(&pinned, "grok", true).unwrap_err().contains("blocked by your organization"));
    assert!(first_run(&pinned, &["claude".into(), "codex".into()]).is_err(), "one forbidden pick refuses the whole choice");
    assert_eq!(chosen(&pinned, "claude", true).unwrap().enabled, set(&["claude"]));
    // Turning a forbidden harness off is always possible.
    let stale = gate(&["codex"], Policy { allowed: Some(set(&["claude"])), ..Policy::default() });
    assert!(chosen(&stale, "codex", false).unwrap().enabled.is_empty());
    assert_eq!(chosen(&pinned, "not-a-harness", true).unwrap_err(), "Unknown agent.");
}
