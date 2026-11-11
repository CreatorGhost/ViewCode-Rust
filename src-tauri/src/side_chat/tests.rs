use super::*;
use crate::concurrent_task_tests::test_state;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::mpsc::unbounded_channel;

const HOUR: i64 = 60 * 60 * 1_000;

/// A store with one thread that has said something.
fn fixture() -> (Store, Session) {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let project = store.add_project(&std::env::temp_dir()).unwrap();
    let parent = store.create_session(&project.id, "claude", "default", "ask").unwrap();
    store.message(&parent.id, "user", "Make search instant").unwrap();
    store.message(&parent.id, "tool", "{\"type\":\"tool_activity\"}").unwrap();
    store.message(&parent.id, "assistant", "I am building an index.").unwrap();
    (store, parent)
}

fn contents(store: &Store, sid: &str) -> Vec<(String, String)> {
    let mut query = store.db.prepare("SELECT role,content FROM messages WHERE session_id=?1 ORDER BY id").unwrap();
    let rows = query.query_map([sid], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    rows.map(|row| row.unwrap()).collect()
}

fn idle() -> impl Fn(&str) -> bool {
    |_| false
}

#[test]
fn a_side_chat_is_a_session_with_a_kind_and_a_parent_link() {
    let (store, parent) = fixture();
    assert_eq!((parent.kind.as_str(), parent.parent_session_id.as_deref()), ("thread", None));
    store.db.execute("UPDATE sessions SET reasoning_effort='high' WHERE id=?1", [&parent.id]).unwrap();
    let side = open(&store, &parent.id, "  why an **index**?\nand more ", 1_000).unwrap();
    assert_eq!(side.kind, KIND);
    assert_eq!(side.parent_session_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(side.title, "why an **index**? and more");
    assert_eq!((&side.project_id, &side.provider, &side.model, &side.mode), (&parent.project_id, &parent.provider, &parent.model, &parent.mode));
    assert_eq!(side.reasoning_effort.as_deref(), Some("high"));
    assert_eq!((side.created_at, side.updated_at, side.last_prompt_at, side.last_response_at), (1_000, 1_000, None, None));
    assert_eq!(side.status, "idle");
    let value = serde_json::to_value(&side).unwrap();
    assert_eq!((value["kind"].as_str(), value["parentSessionId"].as_str()), (Some("side"), Some(parent.id.as_str())));
    assert!(serde_json::to_value(store.session(&parent.id).unwrap()).unwrap().get("parentSessionId").is_none());
    assert_eq!(open(&store, &parent.id, "", 2_000).unwrap().title, "Side chat");
    assert_eq!(open(&store, &parent.id, &"x".repeat(60), 2_000).unwrap().title.chars().count(), 48);
}

#[test]
fn a_side_chat_cannot_be_opened_beside_a_side_chat_a_worker_or_a_remote_task() {
    let (store, parent) = fixture();
    let side = open(&store, &parent.id, "q", 1).unwrap();
    assert_eq!(open(&store, &side.id, "q", 2).unwrap_err(), "A side chat cannot open another side chat.");
    assert!(open(&store, "missing", "q", 2).is_err());
    store.db.execute("UPDATE sessions SET remote_device='device-1' WHERE id=?1", [&parent.id]).unwrap();
    assert_eq!(open(&store, &parent.id, "q", 2).unwrap_err(), "Side chats run on this computer, and this task runs elsewhere.");
    assert_eq!(list(&store, &parent.id).unwrap().len(), 1, "a refused open leaves nothing behind");
}

#[test]
fn opening_snapshots_the_parent_conversation_and_never_follows_it() {
    let (store, parent) = fixture();
    let side = open(&store, &parent.id, "why an index?", 1_000).unwrap();
    // Only what was said is copied: no tool rows, in order.
    assert_eq!(contents(&store, &side.id), [("user".to_string(), "Make search instant".to_string()), ("assistant".into(), "I am building an index.".into())]);
    // The parent keeps working; the side chat does not see it.
    store.message(&parent.id, "assistant", "The index is done.").unwrap();
    let (prompt, through) = crate::context_recovery::prepare(&store, &store.session(&side.id).unwrap(), "why an index?").unwrap();
    assert!(prompt.starts_with(SNAPSHOT_NOTICE), "{prompt}");
    assert!(prompt.contains("I am building an index.") && !prompt.contains("The index is done."));
    assert!(prompt.ends_with("Current request:\nwhy an index?"));
    assert_eq!(through, inherited_through(&store, &side.id).unwrap());
    // Once that turn is acknowledged, later handoffs are the side chat's own history.
    store.begin_task_turn(&side.id, "why an index?").unwrap();
    crate::context_recovery::acknowledge(&store, &side.id, through.unwrap()).unwrap();
    store.message(&side.id, "assistant", "Because scans were slow.").unwrap();
    store.finish_task_turn(&side.id, true, false).unwrap();
    let (again, _) = crate::context_recovery::prepare(&store, &store.session(&side.id).unwrap(), "and then?").unwrap();
    assert!(!again.starts_with(SNAPSHOT_NOTICE) && again.contains("Because scans were slow."));
    // A parent with nothing said yet opens an empty side chat with nothing to hand over.
    let empty = store.create_session(&parent.project_id, "claude", "default", "ask").unwrap();
    let blank = open(&store, &empty.id, "q", 1).unwrap();
    assert_eq!(inherited_through(&store, &blank.id).unwrap(), Some(0));
    assert_eq!(crate::context_recovery::prepare(&store, &blank, "q").unwrap(), ("q".into(), None));
}

#[test]
fn the_snapshot_is_bounded_by_rows_user_turns_and_text() {
    let rows = |count: usize, size: usize| (0..count).map(|index| (if index % 2 == 0 { "user" } else { "assistant" }.to_string(), format!("{index:04}{}", "é".repeat(size - 4)), index as i64)).collect::<Vec<Row>>();
    // Eight user turns at most, ending with the newest row.
    let kept = bound(rows(40, 10));
    assert_eq!(kept.iter().filter(|row| row.0 == "user").count(), SNAPSHOT_USER_TURNS);
    assert_eq!((kept.len(), kept.last().unwrap().2, kept[0].2), (16, 39, 24));
    // One long message keeps its end; the total never passes the budget.
    let kept = bound(rows(6, 10_000));
    assert!(kept.iter().all(|row| row.1.chars().count() <= SNAPSHOT_MESSAGE_CHARS));
    // Three fit the budget; the oldest of those is an answer whose question was cut, so it goes too.
    assert_eq!(kept.iter().map(|row| (row.0.as_str(), row.1.chars().count())).collect::<Vec<_>>(), [("user", SNAPSHOT_MESSAGE_CHARS), ("assistant", SNAPSHOT_MESSAGE_CHARS)]);
    assert_eq!(bound(rows(3, 9_000)).iter().map(|row| row.1.chars().count()).sum::<usize>(), SNAPSHOT_CHARS);
    assert!(kept.last().unwrap().1.ends_with('é') && !kept.last().unwrap().1.starts_with("0005"));
    assert_eq!(tail("añb", 2), "ñb");
    // The query itself reads only the newest rows.
    let (store, parent) = fixture();
    for index in 0..100 {
        store.message(&parent.id, "assistant", &format!("reply {index}")).unwrap();
    }
    let copied = snapshot(&store, &parent.id).unwrap();
    assert_eq!((copied.len(), copied[0].1.as_str(), copied.last().unwrap().1.as_str()), (SNAPSHOT_ROWS, "reply 40", "reply 99"));
}

#[test]
fn side_chats_are_hidden_from_lists_and_search() {
    let (store, parent) = fixture();
    let side = open(&store, &parent.id, "why an index?", 1_000).unwrap();
    store.message(&side.id, "user", "a zebra question").unwrap();
    let listed = store.sessions().unwrap().into_iter().map(|task| task.id).collect::<Vec<_>>();
    assert_eq!(listed, std::slice::from_ref(&parent.id));
    // Neither its title, its own messages nor the copied ones are found.
    for query in ["why an index", "zebra"] {
        assert!(store.search_tasks(&parent.project_id, query, 10).unwrap().is_empty(), "{query}");
    }
    let found = store.search_tasks(&parent.project_id, "search instant", 10).unwrap();
    assert_eq!(found.iter().map(|hit| hit["taskId"].as_str().unwrap()).collect::<Vec<_>>(), [parent.id.as_str()]);
    // Only its thread lists it.
    assert_eq!(list(&store, &parent.id).unwrap().into_iter().map(|task| task.id).collect::<Vec<_>>(), std::slice::from_ref(&side.id));
    assert!(list(&store, &side.id).unwrap().is_empty());
    // Opened as a full thread it is an ordinary, listed task that keeps its lineage.
    let full = promote(&store, &side.id, 2_000).unwrap();
    assert_eq!((full.kind.as_str(), full.parent_session_id.as_deref()), ("thread", Some(parent.id.as_str())));
    assert!(store.sessions().unwrap().iter().any(|task| task.id == side.id));
    assert!(list(&store, &parent.id).unwrap().is_empty());
    assert_eq!(promote(&store, &side.id, 3_000).unwrap_err(), "This is not a side chat.");
}

#[test]
fn a_side_chat_shows_only_its_own_messages() {
    let (store, parent) = fixture();
    let side = open(&store, &parent.id, "why an index?", 1_000).unwrap();
    let page = store.message_page(&side.id, None).unwrap();
    assert!(page.messages.is_empty() && !page.has_older);
    store.message(&side.id, "user", "why an index?").unwrap();
    store.message(&side.id, "assistant", "Because scans were slow.").unwrap();
    let page = store.message_page(&side.id, None).unwrap();
    assert_eq!(page.messages.iter().map(|message| message.content.as_str()).collect::<Vec<_>>(), ["why an index?", "Because scans were slow."]);
    assert!(!page.has_older, "the copied rows are not an older page");
    // The parent's transcript is untouched and shows nothing of the side chat.
    assert_eq!(store.message_page(&parent.id, None).unwrap().messages.len(), 3);
    // As a full thread the inherited history shows.
    promote(&store, &side.id, 2_000).unwrap();
    assert_eq!(store.message_page(&side.id, None).unwrap().messages.len(), 4);
}

#[test]
fn an_idle_side_chat_expires_after_24_hours() {
    let (store, parent) = fixture();
    let opened = 1_000_000;
    let side = open(&store, &parent.id, "q", opened).unwrap();
    let none: Vec<(String, String)> = Vec::new();
    assert_eq!(expire(&store, opened + IDLE_EXPIRY_MS - 1, &idle()).unwrap(), none);
    assert_eq!(list(&store, &parent.id).unwrap().len(), 1);
    assert_eq!(expire(&store, opened + IDLE_EXPIRY_MS, &idle()).unwrap(), [(side.id.clone(), parent.id.clone())]);
    assert!(store.session(&side.id).unwrap().archived);
    assert!(list(&store, &parent.id).unwrap().is_empty(), "an expired chat is gone from its thread");
    assert_eq!(expire(&store, opened + 3 * IDLE_EXPIRY_MS, &idle()).unwrap(), none, "archived once");
    // Its rows remain, hidden; ordinary threads never expire.
    assert_eq!(contents(&store, &side.id).len(), 2);
    assert!(!store.session(&parent.id).unwrap().archived);
    assert!(store.sessions().unwrap().iter().all(|task| task.id != side.id));
}

#[test]
fn activity_running_work_and_queued_messages_keep_a_side_chat() {
    let (store, parent) = fixture();
    let opened = 1_000_000;
    let late = opened + IDLE_EXPIRY_MS + HOUR;
    let none: Vec<(String, String)> = Vec::new();
    // A prompt or a reply restarts the day.
    let asked = open(&store, &parent.id, "asked", opened).unwrap();
    store.db.execute("INSERT INTO messages(session_id,role,content,created_at) VALUES (?1,'user','later',?2)", params![asked.id, opened + 2 * HOUR]).unwrap();
    assert_eq!(store.session(&asked.id).unwrap().last_prompt_at, Some(opened + 2 * HOUR));
    assert_eq!(expire(&store, late, &idle()).unwrap(), none);
    store.db.execute("INSERT INTO messages(session_id,role,content,created_at) VALUES (?1,'assistant','answer',?2)", params![asked.id, opened + 3 * HOUR]).unwrap();
    assert_eq!(expire(&store, opened + IDLE_EXPIRY_MS + 3 * HOUR - 1, &idle()).unwrap(), none);
    assert_eq!(expire(&store, opened + IDLE_EXPIRY_MS + 3 * HOUR, &idle()).unwrap().len(), 1);
    // Running, waiting on the user, queued or still holding a process: kept however old.
    let running = open(&store, &parent.id, "running", opened).unwrap();
    store.db.execute("UPDATE sessions SET status='running' WHERE id=?1", [&running.id]).unwrap();
    let waiting = open(&store, &parent.id, "waiting", opened).unwrap();
    store.db.execute("UPDATE sessions SET status='waiting' WHERE id=?1", [&waiting.id]).unwrap();
    let queued = open(&store, &parent.id, "queued", opened).unwrap();
    crate::message_queue::add(&store, &queued.id, "next", &serde_json::json!([])).unwrap();
    let held = open(&store, &parent.id, "held", opened).unwrap();
    let held_id = held.id.clone();
    assert_eq!(expire(&store, late * 2, &move |id| id == held_id).unwrap(), none);
    assert_eq!(list(&store, &parent.id).unwrap().len(), 4);
    // Each expires once its work settles.
    store.db.execute("UPDATE sessions SET status='idle' WHERE id IN (?1,?2)", [&running.id, &waiting.id]).unwrap();
    store.db.execute("DELETE FROM queued_messages", []).unwrap();
    assert_eq!(expire(&store, late * 2, &idle()).unwrap().len(), 4);
}

#[test]
fn closing_ends_an_idle_side_chat_and_refuses_a_running_one() {
    let (store, parent) = fixture();
    let side = open(&store, &parent.id, "q", 1).unwrap();
    store.db.execute("UPDATE sessions SET status='running' WHERE id=?1", [&side.id]).unwrap();
    assert_eq!(close(&store, &side.id).unwrap_err(), "Stop the side chat before closing it.");
    store.db.execute("UPDATE sessions SET status='idle' WHERE id=?1", [&side.id]).unwrap();
    assert!(close(&store, &side.id).unwrap().archived);
    assert!(list(&store, &parent.id).unwrap().is_empty());
    assert_eq!(close(&store, &parent.id).unwrap_err(), "This is not a side chat.");
    assert_eq!(require(&store, &parent.id).unwrap_err(), "This is not a side chat.");
}

#[test]
fn a_side_chat_runs_beside_a_working_parent_without_touching_it() {
    let (state, project_id) = test_state();
    let (parent_tx, mut parent_rx) = unbounded_channel();
    let (parent, before, side) = {
        let store = state.store.lock().unwrap();
        let parent = store.create_session(&project_id, "claude", "default", "ask").unwrap();
        store.message(&parent.id, "user", "Earlier request").unwrap();
        store.message(&parent.id, "assistant", "Earlier answer").unwrap();
        store.resume(&parent.id, "parent-native-thread").unwrap();
        // The parent's agent is mid-turn.
        store.begin_task_turn(&parent.id, "Build the feature").unwrap();
        let before = (serde_json::to_value(store.session(&parent.id).unwrap()).unwrap(), contents(&store, &parent.id));
        let side = open(&store, &parent.id, "what does it cost?", crate::storage::now()).unwrap();
        (parent, before, side)
    };
    state.agents.lock().unwrap().insert(parent.id.clone(), crate::RunningAgent::new(parent_tx, Arc::new(AtomicBool::new(false))));
    let (side_tx, _side_rx) = unbounded_channel();
    let prepared = crate::prepare_task_run(&state, side.id.clone(), Some("what does it cost?".into()), Vec::new(), side_tx, Arc::new(AtomicBool::new(false))).unwrap();
    // Its own session and a fresh agent context that starts from the snapshot.
    assert_eq!(prepared.config.session_id, side.id);
    assert_eq!(prepared.config.resume_id, None);
    assert!(prepared.config.prompt.starts_with(SNAPSHOT_NOTICE));
    assert!(prepared.config.prompt.contains("Earlier answer") && prepared.config.prompt.contains("Build the feature"));
    assert!(prepared.context_handoff.is_some());
    let store = state.store.lock().unwrap();
    // The parent: same row, same messages, still running, and nothing was sent to its process.
    assert_eq!((serde_json::to_value(store.session(&parent.id).unwrap()).unwrap(), contents(&store, &parent.id)), before);
    assert_eq!(before.0["status"], "running");
    assert!(parent_rx.try_recv().is_err(), "the parent's run received no input, steer or stop");
    assert!(state.agents.lock().unwrap().contains_key(&parent.id));
    // The side chat saved only the question; the snapshot is not shown.
    let page = store.message_page(&side.id, None).unwrap();
    assert_eq!(page.messages.iter().map(|message| message.content.as_str()).collect::<Vec<_>>(), ["what does it cost?"]);
    assert_eq!(store.session(&side.id).unwrap().status, "running");
    // A running side chat is not expired by a sweep, however far the clock moved.
    drop(store);
    assert!(sweep(&state, crate::storage::now() + 10 * IDLE_EXPIRY_MS).unwrap().is_empty());
}

#[test]
fn deleting_a_thread_deletes_its_side_chats() {
    let (state, project_id) = test_state();
    let (parent, side, other) = {
        let store = state.store.lock().unwrap();
        let parent = store.create_session(&project_id, "claude", "default", "ask").unwrap();
        let other = store.create_session(&project_id, "claude", "default", "ask").unwrap();
        (parent.clone(), open(&store, &parent.id, "q", 1).unwrap(), open(&store, &other.id, "q", 1).unwrap())
    };
    let deleted = crate::task_deletion::delete_state(&state, &parent.id).unwrap();
    assert_eq!(deleted.deleted_session_ids, [parent.id.clone(), side.id.clone()]);
    let store = state.store.lock().unwrap();
    assert!(store.session(&side.id).is_err());
    assert!(store.session(&other.id).is_ok(), "another thread's side chat stays");
    assert_eq!(store.db.query_row("SELECT COUNT(*) FROM side_chats", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
}

#[test]
fn schema_40_adds_kinds_and_parent_links_to_an_older_database() {
    let path = std::env::temp_dir().join(format!("pandacode-side-chat-{}.db", crate::storage::id()));
    let store = Store::open(&path).unwrap();
    let project = store.add_project(&std::env::temp_dir()).unwrap();
    let old = store.create_session(&project.id, "claude", "default", "ask").unwrap();
    store.db.execute_batch("DROP TABLE side_chats; DROP INDEX sessions_parent; ALTER TABLE sessions DROP COLUMN kind; ALTER TABLE sessions DROP COLUMN parent_session_id; PRAGMA user_version=39;").unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(crate::storage::SCHEMA_VERSION, 40);
    assert_eq!(store.db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(), 40);
    let migrated = store.session(&old.id).unwrap();
    assert_eq!((migrated.kind.as_str(), migrated.parent_session_id), ("thread", None));
    assert_eq!(open(&store, &old.id, "q", 1).unwrap().kind, KIND);
    drop(store);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}
