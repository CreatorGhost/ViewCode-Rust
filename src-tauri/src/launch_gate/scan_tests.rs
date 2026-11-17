//! Source scan: a harness binary can only be started through the gated builders in
//! `providers/launch.rs`. Every other place the app builds a process is listed here with
//! what it runs; a new one fails this test until it is either routed through
//! `providers::provider_command` / `command` / `pty_command` (an agent) or added below
//! with its reason (anything else).
use std::path::{Path, PathBuf};

/// Non-test code that builds a process directly: file, how many times, and what it runs.
/// None of these may be handed a harness binary.
const DIRECT: &[(&str, usize, &str)] = &[
    ("agent_updates_install.rs", 6, "copies a gated harness command; node/npm, brew and uv for an enabled harness's installer"),
    ("checks.rs", 2, "the project's own check commands"),
    ("cloud/aws.rs", 1, "aws CLI"),
    ("cloud/cloudflare.rs", 3, "docker, npm, wrangler"),
    ("cloud/deploy.rs", 8, "gcloud, vercel, wrangler, railway, aws and npx for a deploy"),
    ("cloud/docker.rs", 1, "docker"),
    ("cloud/modal.rs", 1, "python for the Modal helper"),
    ("cloud/ssh.rs", 4, "ssh-keygen, ssh, scp"),
    ("cloud_agents/credentials.rs", 3, "security, gh, git"),
    ("cloud_agents/sync.rs", 3, "git"),
    ("cloud_agents/tasks.rs", 1, "git"),
    ("cloud_agents/transport.rs", 2, "a placeholder program for a cloud machine's harness (never spawned here) and this app's own bridge; the turn is gated in run_with_tools"),
    ("code_search.rs", 1, "codedb"),
    ("computer_use/driver.rs", 1, "this app's computer-use helper"),
    ("computer_use/service.rs", 1, "ps"),
    ("github.rs", 1, "gh"),
    ("github_hub.rs", 1, "git"),
    ("jev.rs", 1, "uv/python for the Jev browser runner"),
    ("jj.rs", 1, "jj"),
    ("lib.rs", 10, "open, xdg-open, explorer, notepad, rundll32, ps"),
    ("local_servers.rs", 1, "lsof/netstat-style port listing"),
    ("local_signing.rs", 1, "codesign"),
    ("model_routing.rs", 1, "sw_vers"),
    ("prompt_attachments.rs", 1, "a document-to-text tool"),
    ("providers.rs", 2, "re-runs an already gated Claude command with --help; taskkill for a PID this app started"),
    ("providers/launch.rs", 3, "the gated builders (two harness, one tool) and the PTY builder"),
    ("quick_connect/process.rs", 1, "wrangler/npx for the user's own relay"),
    ("remote/bench.rs", 3, "gzip and git in the Connect benchmark"),
    ("remote/host.rs", 1, "git"),
    ("remote/start.rs", 3, "git, gh, taskkill for a PID this app started"),
    ("run_script.rs", 2, "the project's run script in a shell"),
    ("simulator.rs", 4, "xcrun, plutil, sips"),
    ("simulator/android.rs", 3, "adb, emulator"),
    ("simulator/android_setup.rs", 3, "brew, sdkmanager"),
    ("simulator/duo.rs", 1, "this app's simulator helper"),
    ("simulator/stream.rs", 1, "axe"),
    ("skill_manager/links.rs", 1, "cmd.exe mklink"),
    ("skills/adapter.rs", 1, "git"),
    ("terminal.rs", 5, "the user's shell, ps and PowerShell; a harness console goes through pty_command"),
    ("token_saver.rs", 2, "rtk, brew"),
    ("usage_keychain.rs", 2, "this app's own helper and /usr/bin/security"),
    ("vm/setup.rs", 1, "VM setup tools"),
    ("vm/tart.rs", 1, "tart"),
    ("voice/local/worker.rs", 1, "this app's speech worker"),
    ("workspace.rs", 2, "git and taskkill for a PID this app started"),
];

/// Files that may ask for an ungated tool command (`providers::tool_command`).
const TOOL_COMMANDS: &[(&str, usize, &str)] = &[
    ("cloud/deploy.rs", 2, "netlify, or npx to fetch it"),
    ("providers/launch.rs", 1, "its definition"),
    ("skill_manager/cli.rs", 1, "npx for the skills CLI"),
];

fn sources(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            sources(&path, root, out);
            continue;
        }
        let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if !relative.ends_with(".rs") || relative.ends_with("tests.rs") || format!("/{relative}").contains("/tests/") {
            continue;
        }
        out.push((relative, production(&std::fs::read_to_string(&path).unwrap())));
    }
}

/// The text without items under `#[cfg(test)]` / `#[cfg(all(test, ...))]`: the item ends
/// at its closing brace, or at a `;` before any brace.
fn production(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut position = 0;
    while position < bytes.len() {
        let end = text[position..].find('\n').map_or(bytes.len(), |offset| position + offset);
        let line = text[position..end].trim();
        if line.starts_with("#[cfg(test)]") || line.starts_with("#[cfg(all(test") {
            let (mut cursor, mut depth) = (end, 0usize);
            while cursor < bytes.len() {
                match bytes[cursor] {
                    b';' if depth == 0 => {
                        cursor += 1;
                        break;
                    }
                    b'{' => depth += 1,
                    b'}' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            cursor += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                cursor += 1;
            }
            position = cursor;
            continue;
        }
        out.push_str(&text[position..(end + 1).min(bytes.len())]);
        position = end + 1;
    }
    out
}

fn count(text: &str, needles: &[&str]) -> usize {
    needles.iter().map(|needle| text.matches(needle).count()).sum()
}

fn all() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    sources(&root, &root, &mut out);
    out
}

fn compare(mut found: Vec<(String, usize)>, listed: &[(&str, usize, &str)], what: &str) {
    let mut listed: Vec<(String, usize)> = listed.iter().map(|(file, sites, _)| ((*file).to_owned(), *sites)).collect();
    found.sort();
    listed.sort();
    assert_eq!(
        found, listed,
        "\n{what} changed. A harness binary must be started through providers::provider_command, command or pty_command, which ask the launch gate. Anything else: update the list in launch_gate/scan_tests.rs with what it runs."
    );
}

#[test]
fn every_direct_process_is_listed_and_none_is_a_harness() {
    let sources = all();
    let found: Vec<(String, usize)> = sources.iter().map(|(file, text)| (file.clone(), count(text, &["Command::new(", "CommandBuilder::new("]))).filter(|(_, sites)| *sites > 0).collect();
    compare(found, DIRECT, "The places that build a process directly");
    // No production code names a harness binary as the program, whatever the roster holds.
    for (id, _) in crate::providers::PROVIDERS {
        for name in [id, crate::providers::binary_name(id)] {
            for (file, text) in &sources {
                for builder in ["Command::new", "CommandBuilder::new"] {
                    assert!(!text.contains(&format!("{builder}(\"{name}\"")), "{file} starts `{name}` without the launch gate");
                }
            }
        }
    }
}

#[test]
fn only_listed_callers_get_an_ungated_command() {
    let sources = all();
    let found: Vec<(String, usize)> = sources.iter().map(|(file, text)| (file.clone(), count(text, &["tool_command("]))).filter(|(_, sites)| *sites > 0).collect();
    compare(found, TOOL_COMMANDS, "The callers of providers::tool_command");
    for (file, text) in &sources {
        assert!(file == "providers/launch.rs" || !text.contains("ungated_command("), "{file} reaches past the gate");
    }
}

#[test]
fn the_three_builders_ask_the_gate_before_anything_else() {
    let launch = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/providers/launch.rs")).unwrap();
    for signature in [
        "pub(crate) fn command(harness: &str, path: &Path) -> Result<tokio::process::Command, String> {",
        "pub(super) fn std_command(harness: &str, path: &Path) -> Result<std::process::Command, String> {",
        "pub(crate) fn pty_command(harness: &str, path: &Path) -> Result<portable_pty::CommandBuilder, String> {",
    ] {
        let body = launch.split_once(signature).unwrap_or_else(|| panic!("{signature} moved")).1;
        assert!(body.trim_start().starts_with("crate::launch_gate::allow(harness)?;"), "{signature} must ask the launch gate first");
    }
    // Resolving a launcher (which may name `node` and a script) stays inside launch.rs.
    for (file, text) in all() {
        assert!(file == "providers/launch.rs" || !text.contains("launch::resolve("), "{file} resolves a launcher itself");
    }
}

#[test]
fn keeping_awake_starts_no_helper() {
    // The power assertion is taken through IOKit in this process: no `caffeinate` child.
    let source = production(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/keep_awake.rs")).unwrap());
    assert_eq!(count(&source, &["Command::new(", "CommandBuilder::new(", ".spawn()"]), 0);
    assert!(source.contains("IOPMAssertionCreateWithName") && source.contains("IOPMAssertionRelease"));
}

/// Callers of `providers::locate`, which finds a harness binary without asking the gate.
/// It is for showing where a binary is; nothing may start what it returns.
const LOCATE: &[(&str, usize, &str)] = &[
    ("launch_gate.rs", 4, "the state the UI shows: installed or not, and where"),
    ("providers.rs", 3, "its definition, `executable` after the gate, and discovery's `present`/`path`"),
];

#[test]
fn a_harness_path_is_only_resolved_after_the_gate() {
    let sources = all();
    let providers = &sources.iter().find(|(file, _)| file == "providers.rs").unwrap().1;
    let body = providers.split_once("pub(crate) fn executable(name: &str) -> Option<PathBuf> {").expect("providers::executable").1;
    assert!(body.trim_start().starts_with("if !crate::launch_gate::enabled(name) {\n        return None;\n    }"), "the gate is asked before a saved override is read");
    let found: Vec<(String, usize)> = sources.iter().map(|(file, text)| (file.clone(), count(text, &["providers::locate(", " locate("]))).filter(|(_, sites)| *sites > 0).collect();
    compare(found, LOCATE, "The callers of providers::locate");
    // `executable` is for harness ids: any other tool would be refused as "not enabled".
    for (file, text) in &sources {
        for (at, _) in text.match_indices("executable(\"") {
            // `validated_executable("…")` and the like are other functions.
            if text[..at].ends_with(|before: char| before.is_alphanumeric() || before == '_') {
                continue;
            }
            let name = text[at + "executable(\"".len()..].split('"').next().unwrap_or_default();
            assert!(crate::providers::PROVIDERS.iter().any(|(id, _)| *id == name), "{file} asks providers::executable for `{name}`, which is not a harness; use tool_path");
        }
    }
}

#[test]
fn test_items_are_left_out_of_the_scan() {
    let text = "fn real() { Command::new(\"git\"); }\n#[cfg(test)]\nmod tests {\n    fn t() { Command::new(\"sh\"); }\n}\n#[cfg(test)]\nuse std::process::Command;\n    #[cfg(all(test, unix))]\n    fn helper() -> u8 { if true { 1 } else { 2 } }\nfn after() {}\n";
    assert_eq!(production(text), "fn real() { Command::new(\"git\"); }\n\n\n\nfn after() {}\n");
}

#[test]
fn every_way_a_turn_starts_passes_the_gated_admission() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let read = |file: &str| production(&std::fs::read_to_string(root.join(file)).unwrap());
    let lib = read("lib.rs");
    let admission = lib.split_once("fn prepare_task_run_in_project(").expect("the admission function").1;
    assert!(admission.split_once("\nfn ").unwrap().0.contains("launch_gate::allow(&session.provider)?;"), "a turn is admitted only for an enabled harness");
    // Schedules send a prompt; the prompt starts a turn through the same admission.
    assert!(read("schedules/firing.rs").contains("crate::send_prompt("));
    assert!(read("providers.rs").split_once("pub async fn run_with_tools(").unwrap().1.contains("crate::launch_gate::allow(&config.provider)"));
}
