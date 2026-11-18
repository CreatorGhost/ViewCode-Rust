//! A long thread switched to another agent, end to end (docs/handoff.md): the 304-message
//! fixture thread is moved from Codex to a stand-in agent speaking the recorded Claude stream
//! protocol, once on a model with a small context window and once on one with a large window.
//! The stand-in saves the first prompt it is sent; that prompt must be the handoff, within
//! the model's budget, carrying the summary and the pointers. On a model whose window has no
//! room, the turn is refused before anything is saved and the stand-in never runs.
//!
//! The stand-in is a script in a folder of this test's own, started by its path with a PATH
//! that holds only that folder and the system's. Nothing here waits on a clock.
use super::*;
use crate::context_recovery::{self, Options};
use crate::handoff::{self, fixture};
use std::os::unix::fs::PermissionsExt;

const REQUEST: &str = "Now make the ranking stable for three-way ties too.";

/// `$1` is the folder it records in: that it ran, and the prompt frame it was sent.
const STAND_IN: &str = r#"#!/bin/sh
printf 'started\n' >> "$1/started"
IFS= read -r init
printf '%s\n' '{"type":"control_response","response":{"request_id":"pandacode-init","subtype":"success","response":{}}}'
IFS= read -r prompt
printf '%s\n' "$prompt" > "$1/prompt.json"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"stand-in-session"}'
printf '%s\n' '{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Picked up from the handoff."}]}}'
printf '%s\n' '{"type":"result","subtype":"success","result":"Picked up from the handoff."}'
"#;

/// The text the agent was sent: the one string in the frame that holds the request.
fn sent(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if text.contains(REQUEST) => Some(text.clone()),
        Value::Array(items) => items.iter().find_map(sent),
        Value::Object(fields) => fields.values().find_map(sent),
        _ => None,
    }
}

fn started(run: &Path) -> usize {
    std::fs::read_to_string(run.join("started")).map_or(0, |text| text.lines().count())
}

#[tokio::test]
async fn a_long_thread_reaches_a_small_and_a_large_model_within_budget_and_a_tiny_one_is_refused() {
    let _serial = process_test_lock().lock().await;
    let root = std::env::temp_dir().join(format!("pandacode-handoff-run-{}", uuid::Uuid::new_v4()));
    let (bin, run, data) = (root.join("bin"), root.join("run"), root.join("data"));
    for folder in [&bin, &run, &data] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let agent = bin.join("stand-in-agent");
    std::fs::write(&agent, STAND_IN).unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();

    let store = crate::storage::Store::open(&data.join("state.sqlite")).unwrap();
    let project = store.add_project(&root).unwrap();
    let task = store.create_session(&project.id, "codex", "gpt-old", "ask").unwrap();
    let ids = fixture::save(&store, &task.id);
    store.resume(&task.id, "codex-native-session").unwrap();
    let history = data.join("handoffs").join(&task.id).join("history.md");

    // (model, the window its catalog entry reports, the handoff's allowance in bytes)
    let mut carried_whole = Vec::new();
    for (round, (model, tokens, allowance)) in [("stand-in-small", 32_000_u64, 24_000_usize), ("stand-in-large", 200_000, 150_000)].into_iter().enumerate() {
        handoff::note_window("claude", model, tokens);
        let session = store.configure(&task.id, "claude", model, None).unwrap();
        assert_eq!(session.resume_id, None, "another harness never resumes the Codex session");
        let (prompt, through) = context_recovery::prepare_with(&store, &session, REQUEST, &Options::default()).unwrap();
        assert!(through.is_some());

        let mut config = tests::config("claude");
        (config.session_id, config.model, config.cwd, config.prompt) = (task.id.clone(), model.into(), root.clone(), prompt.clone());
        let (input, receiver) = unbounded_channel();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let saved = observed.clone();
        let sink: Sink = Arc::new(move |event: AgentEvent| saved.lock().unwrap().push(event));
        let mut command = Command::new(&agent);
        command.arg(&run).env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        run_process(config, sink, receiver, command, Vec::new()).await.unwrap();
        drop(input);
        assert_eq!(started(&run), round + 1, "one process per turn");
        let reply: String = observed.lock().unwrap().iter().filter(|event| event.kind == "text").map(|event| event.text.as_str()).collect();
        assert_eq!(reply, "Picked up from the handoff.");

        // The first prompt the new agent received is the handoff, byte for byte.
        let frame: Value = serde_json::from_str(&std::fs::read_to_string(run.join("prompt.json")).unwrap()).unwrap();
        let received = sent(&frame).expect("the stand-in was sent the request");
        assert_eq!(received, prompt);
        // Within the budget of this model's window; the request itself is whole, after it.
        let context = received.strip_suffix(&format!("Current request:\n{REQUEST}")).unwrap();
        assert!(context.len() <= allowance, "{model}: {} > {allowance}", context.len());
        assert!(context.contains(&format!("Budget: context window {} tokens; {} bytes for this handoff", handoff::group(tokens as usize), handoff::group(allowance))));
        // It says what it is, who wrote the history, and carries the summary and pointers.
        assert!(context.starts_with(&format!("PandaCode handoff. This thread was written with Codex (model gpt-old) and continues with you, Claude Code (model {model}), from this message on.")));
        for part in ["## Standing instructions from the user", "## Summary of earlier work", "### Goals and requests", "### Decisions and reasons", "### Files created or changed", "## Recent conversation", "full text: history file line "] {
            assert!(context.contains(part), "{model}: {part}");
        }
        // The small model's summary could not hold every older message: the rest are pointers.
        assert_eq!(context.contains("## By reference only"), round == 0, "{model}");
        for instruction in fixture::STANDING {
            assert!(context.contains(&format!("“{instruction}”")));
        }
        assert!(context.contains(&format!("Full history: {} holds every message whole", history.display())));
        // The newest exchange is there in full; the pointers lead into a file that exists.
        let whole = handoff::carried(&received);
        assert_eq!(whole.last().unwrap().2, "Ties now fall back to the note's creation time. Next step: add a test for three-way ties, which is still pending.");
        assert!(whole.iter().any(|(_, role, text)| role == "user" && text == "Now make the ranking stable when two notes tie."));
        let file = std::fs::read_to_string(&history).unwrap();
        assert!(ids.iter().all(|id| file.contains(&format!("\n## #{id} "))));
        carried_whole.push(whole.iter().filter(|(_, role, _)| role == "user" || role == "assistant").count());
    }
    assert!(carried_whole[1] > carried_whole[0] * 3, "the large model gets far more of the thread whole: {carried_whole:?}");
    drop(store);

    // A model whose window has no room: the real turn admission refuses, with the reason,
    // before the request is saved and before any process exists.
    handoff::note_window("claude", "stand-in-tiny", 16_000);
    let (state, project) = crate::concurrent_task_tests::test_state();
    let task = {
        let store = state.store.lock().unwrap();
        let task = store.create_session(&project, "codex", "gpt-old", "ask").unwrap();
        fixture::save(&store, &task.id);
        store.resume(&task.id, "codex-native-session").unwrap();
        store.configure(&task.id, "claude", "stand-in-tiny", None).unwrap()
    };
    let (tx, _rx) = unbounded_channel();
    let refused = crate::prepare_task_run(&state, task.id.clone(), Some(REQUEST.into()), Vec::new(), tx, Arc::new(std::sync::atomic::AtomicBool::new(false))).map(|_| ()).unwrap_err();
    assert_eq!(
        refused,
        "This conversation cannot be handed over to Claude Code (stand-in-tiny): its context window (16,000 tokens) leaves no room for the earlier conversation after your message and the agent's own instructions. Choose a model with a larger context window or start a new thread. Nothing was sent and your message was not shortened."
    );
    assert!(state.agents.lock().unwrap().is_empty(), "no run was admitted");
    assert_eq!(started(&run), 2, "the stand-in was not started for the refused turn");
    let store = state.store.lock().unwrap();
    let saved: i64 = store.db.query_row("SELECT COUNT(*) FROM messages WHERE session_id=?1 AND content=?2", rusqlite::params![task.id, REQUEST], |row| row.get(0)).unwrap();
    assert_eq!(saved, 0, "the refused request was not saved");
    assert_eq!(store.session(&task.id).unwrap().status, "idle");
    assert!(context_recovery::pending(&store, &store.session(&task.id).unwrap()).unwrap(), "the handoff still waits for a model it fits");
    drop(store);
    let _ = std::fs::remove_dir_all(&root);
}
