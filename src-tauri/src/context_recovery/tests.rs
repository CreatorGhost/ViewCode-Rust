use super::*;
use crate::handoff::{carried, fixture, CANNOT_FIT};
use std::path::{Path, PathBuf};

struct Disk(PathBuf);
impl Disk {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pandacode-context-recovery-{}",
            crate::storage::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn database(&self) -> PathBuf {
        self.0.join("state.sqlite")
    }
}
impl Drop for Disk {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn task(store: &Store) -> Session {
    let project = store.add_project(&std::env::temp_dir()).unwrap();
    store
        .create_session(&project.id, "codex", "gpt-test", "ask")
        .unwrap()
}

/// `(role, text)` of every message the prompt carries in full, in order.
fn prior_entries(prompt: &str) -> Vec<(String, String)> {
    carried(prompt).into_iter().map(|(_, role, text)| (role, text)).collect()
}

fn window(tokens: u64) -> Options {
    Options { images: 0, window: Some(tokens) }
}

#[test]
fn a_new_task_and_an_ordinary_resumed_task_keep_the_original_prompt() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let task = task(&store);
    assert!(!pending(&store, &task).unwrap());
    assert_eq!(
        prepare(&store, &task, "First request").unwrap(),
        ("First request".into(), None)
    );
    store
        .message(&task.id, "user", "Keep this prior request")
        .unwrap();
    store
        .message(&task.id, "assistant", "Keep the native answer")
        .unwrap();
    store.resume(&task.id, "existing-native-context").unwrap();
    let resumed = store.session(&task.id).unwrap();
    assert!(!pending(&store, &resumed).unwrap());
    assert_eq!(
        prepare(&store, &resumed, "Continue").unwrap(),
        ("Continue".into(), None)
    );
}

#[test]
fn missing_native_context_recovers_the_exact_previous_code_without_rewriting_the_transcript() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let task = task(&store);
    let code = "```cpp\n#include <iostream>\nint main() { std::cout << \"Hello, world!\" << std::endl; return 0; }\n```";
    store
        .message(
            &task.id,
            "user",
            "Write C++ in your response, without making a file",
        )
        .unwrap();
    store.message(&task.id, "assistant", code).unwrap();
    store
        .message(&task.id, "tool", "Legacy tool row\nwith its output")
        .unwrap();
    store
        .message(&task.id, "system", "A system notice")
        .unwrap();
    store
        .message(&task.id, "changes", "A change summary")
        .unwrap();
    let before = store.messages(&task.id, None).unwrap().len();
    let (prompt, through) = prepare(&store, &task, "Put this in a file please").unwrap();
    assert!(through.is_some());
    let entries = prior_entries(&prompt);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].0, "user");
    assert_eq!(entries[1], ("assistant".into(), code.into()));
    assert!(prompt.ends_with("Current request:\nPut this in a file please"));
    assert_eq!(prompt.matches("Put this in a file please").count(), 1);
    // Tool steps and the app's own rows are one line each, never replayed as conversation.
    assert!(prompt.contains(" tool Legacy tool row] with its output → no output\n"));
    assert!(prompt.contains(" notice system] A system notice\n") && prompt.contains(" notice changes] A change summary\n"));
    assert_eq!(store.messages(&task.id, None).unwrap().len(), before);
    // A database that is not a file has no history file; the agent is told, not pointed.
    assert!(prompt.contains("The full history file could not be written"));
}

#[test]
fn receiving_a_new_session_id_does_not_acknowledge_a_failed_or_stopped_handoff() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let task = task(&store);
    store
        .message(&task.id, "assistant", "Important previous response")
        .unwrap();
    queue(&store, &task.id).unwrap();
    let (_, first_boundary) = prepare(&store, &task, "Continue").unwrap();
    // Providers return a session ID before accepting the prompt; startup can fail next.
    store.resume(&task.id, "new-context-before-prompt").unwrap();
    store.message(&task.id, "user", "Continue").unwrap();
    let restarted = store.session(&task.id).unwrap();
    assert!(pending(&store, &restarted).unwrap());
    let (retry_prompt, boundary) = prepare(&store, &restarted, "Try again").unwrap();
    assert!(boundary.unwrap() > first_boundary.unwrap());
    let entries = prior_entries(&retry_prompt);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1], ("user".into(), "Continue".into()));
    assert!(retry_prompt.contains("Important previous response"));
    assert_eq!(retry_prompt.matches("Try again").count(), 1);
    acknowledge(&store, &task.id, first_boundary.unwrap()).unwrap();
    assert!(pending(&store, &restarted).unwrap());
    acknowledge(&store, &task.id, boundary.unwrap()).unwrap();
    assert!(!pending(&store, &restarted).unwrap());
    assert_eq!(
        prepare(&store, &restarted, "Another follow-up").unwrap(),
        ("Another follow-up".into(), None)
    );
}

#[test]
fn acknowledgment_cannot_erase_a_newer_queued_handoff_or_another_task() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let task = task(&store);
    let other = self::task(&store);
    store
        .message(&task.id, "assistant", "Earlier context")
        .unwrap();
    store
        .message(&other.id, "assistant", "Other task context")
        .unwrap();
    queue(&store, &task.id).unwrap();
    queue(&store, &other.id).unwrap();
    let (_, boundary) = prepare(&store, &task, "Continue").unwrap();
    store.resume(&task.id, "native-context").unwrap();
    store
        .message(&task.id, "assistant", "Later context")
        .unwrap();
    queue(&store, &task.id).unwrap();
    let task = store.session(&task.id).unwrap();
    acknowledge(&store, &task.id, boundary.unwrap()).unwrap();
    assert!(pending(&store, &task).unwrap());
    assert!(pending(&store, &other).unwrap());
    let (prompt, newer) = prepare(&store, &task, "Continue again").unwrap();
    assert!(newer.unwrap() > boundary.unwrap());
    assert_eq!(prior_entries(&prompt).len(), 2);
    acknowledge(&store, &task.id, newer.unwrap()).unwrap();
    assert!(!pending(&store, &task).unwrap());
    assert!(pending(&store, &other).unwrap());
}

#[test]
fn large_unicode_and_escaped_history_stays_bounded_and_honest() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let task = task(&store);
    for index in 0..300 {
        store
            .message(&task.id, "user", &format!("Older message {index}"))
            .unwrap();
    }
    let huge = "竹🌱\"\\\n\t\u{0001}".repeat(30_000);
    store.message(&task.id, "assistant", &huge).unwrap();
    store
        .message(&task.id, "user", "Newest meaningful context")
        .unwrap();
    // 32,000 tokens: 24,000 bytes for the handoff, whatever the history weighs.
    let (prompt, _) = prepare_with(&store, &task, "Keep going 🌱", &window(32_000)).unwrap();
    let context = prompt.strip_suffix("Current request:\nKeep going 🌱").unwrap();
    assert!(context.len() <= 24_000, "{}", context.len());
    let entries = prior_entries(&prompt);
    assert_eq!(entries.last().unwrap(), &("user".into(), "Newest meaningful context".into()));
    // The huge reply is a marked excerpt cut on characters, not a silent cut.
    let excerpt = entries.iter().find(|(role, _)| role.starts_with("assistant · excerpt: the first ")).unwrap();
    assert!(excerpt.0.contains(&format!("of {} bytes; full text is not available to you", crate::handoff::group(huge.len()))));
    assert!(excerpt.1.contains(" bytes left out here …]") && excerpt.1.len() < 2_000);
    // Older messages are summarised or said to be out of reach; none just vanish.
    assert!(prompt.contains("## Summary of earlier work") && prompt.contains("## By reference only"));
    assert!(prompt.contains("so ask the user about them"));
}

#[test]
fn a_long_history_of_short_messages_goes_whole_in_order() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let task = task(&store);
    for index in 0..250 {
        store
            .message(&task.id, "user", &format!("message-{index}"))
            .unwrap();
    }
    let (prompt, _) = prepare(&store, &task, "Continue").unwrap();
    // No row limit: 250 short messages fit the default window's 96,000 bytes.
    let entries = prior_entries(&prompt);
    assert_eq!(entries.len(), 250);
    assert!(entries.iter().enumerate().all(|(index, (role, text))| role == "user" && *text == format!("message-{index}")));
    assert!(!prompt.contains("## Summary of earlier work"));
}

#[test]
fn pending_recovery_survives_reopen_and_success_does_not_repeat_after_reopen() {
    let disk = Disk::new();
    let task;
    {
        let store = Store::open(&disk.database()).unwrap();
        task = self::task(&store);
        store
            .message(&task.id, "assistant", "Context before restart")
            .unwrap();
        store.resume(&task.id, "old-context").unwrap();
        queue(&store, &task.id).unwrap();
    }
    {
        let store = Store::open(&disk.database()).unwrap();
        let restarted = store.session(&task.id).unwrap();
        let (prompt, boundary) = prepare(&store, &restarted, "Continue after restart").unwrap();
        assert!(prompt.contains("Context before restart"));
        store.resume(&task.id, "new-context").unwrap();
        acknowledge(&store, &task.id, boundary.unwrap()).unwrap();
    }
    let store = Store::open(&disk.database()).unwrap();
    let restarted = store.session(&task.id).unwrap();
    assert!(!pending(&store, &restarted).unwrap());
    assert_eq!(
        prepare(&store, &restarted, "Continue again").unwrap(),
        ("Continue again".into(), None)
    );
}

#[test]
fn migration_repairs_legacy_architect_resets_once_even_after_a_new_session_was_saved() {
    let disk = Disk::new();
    let (affected, unaffected);
    {
        let store = Store::open(&disk.database()).unwrap();
        affected = task(&store);
        unaffected = task(&store);
        store
            .message(&affected.id, "assistant", "Original C++ code")
            .unwrap();
        store
            .message(&affected.id, "system", LEGACY_NOTICE)
            .unwrap();
        store
            .message(&affected.id, "user", "Put this in a file")
            .unwrap();
        store
            .message(&affected.id, "assistant", "What does this refer to?")
            .unwrap();
        store
            .resume(&affected.id, "context-created-after-loss")
            .unwrap();
        store
            .message(&unaffected.id, "user", LEGACY_NOTICE)
            .unwrap();
        store
            .message(
                &unaffected.id,
                "system",
                &format!("{LEGACY_NOTICE} Different notice"),
            )
            .unwrap();
        store.resume(&unaffected.id, "intact-context").unwrap();
        store
            .db
            .execute_batch("DROP TABLE context_recovery; PRAGMA user_version=13;")
            .unwrap();
    }
    {
        let store = Store::open(&disk.database()).unwrap();
        let affected = store.session(&affected.id).unwrap();
        let (prompt, boundary) = prepare(&store, &affected, "Continue").unwrap();
        let entries = prior_entries(&prompt);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].1, "Original C++ code");
        assert_eq!(entries[2].1, "What does this refer to?");
        assert_eq!(
            affected.resume_id.as_deref(),
            Some("context-created-after-loss")
        );
        assert!(!pending(&store, &store.session(&unaffected.id).unwrap()).unwrap());
        acknowledge(&store, &affected.id, boundary.unwrap()).unwrap();
    }
    let store = Store::open(&disk.database()).unwrap();
    assert!(!pending(&store, &store.session(&affected.id).unwrap()).unwrap());
    assert_eq!(store.messages(&affected.id, None).unwrap().len(), 4);
}

#[test]
fn schema_41_adds_handoff_columns_and_per_account_sessions_to_an_older_database() {
    let disk = Disk::new();
    let task;
    {
        let store = Store::open(&disk.database()).unwrap();
        task = self::task(&store);
        store.message(&task.id, "assistant", "Before the upgrade").unwrap();
        queue(&store, &task.id).unwrap();
        store
            .db
            .execute_batch("DROP TABLE session_resume_ids; ALTER TABLE context_recovery DROP COLUMN after_message_id; ALTER TABLE context_recovery DROP COLUMN native_tokens; ALTER TABLE context_recovery DROP COLUMN from_provider; ALTER TABLE context_recovery DROP COLUMN from_model; PRAGMA user_version=40;")
            .unwrap();
    }
    let store = Store::open(&disk.database()).unwrap();
    assert_eq!(crate::storage::SCHEMA_VERSION, 41);
    assert_eq!(store.db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(), 41);
    // The handoff queued before the upgrade is still whole and still delivered.
    let (prompt, through) = prepare(&store, &store.session(&task.id).unwrap(), "Continue").unwrap();
    assert!(through.is_some() && prompt.contains("Before the upgrade"));
    assert_eq!(store.db.query_row("SELECT COUNT(*) FROM session_resume_ids", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

/// The fixture thread (304 messages, two very long ones) written with Codex, then switched.
fn switched(store: &Store) -> (Session, Vec<i64>) {
    let task = task(store);
    let ids = fixture::save(store, &task.id);
    store.resume(&task.id, "codex-native-session").unwrap();
    let switched = store.configure(&task.id, "claude", "sonnet", None).unwrap();
    (switched, ids)
}

fn handoff_row(store: &Store, sid: &str) -> serde_json::Value {
    let content: String = store.db.query_row("SELECT content FROM messages WHERE session_id=?1 AND role='handoff' ORDER BY id DESC LIMIT 1", [sid], |row| row.get(0)).unwrap();
    serde_json::from_str(&content).unwrap()
}

#[test]
fn a_switched_long_thread_points_into_a_history_file_that_holds_every_message() {
    let disk = Disk::new();
    let store = Store::open(&disk.database()).unwrap();
    let (task, ids) = switched(&store);
    assert_eq!(handoff_row(&store, &task.id)["summary"], crate::handoff::SWITCHED);
    let (prompt, through) = prepare_with(&store, &task, "Continue with the ranking work.", &window(32_000)).unwrap();
    assert!(through.is_some());
    // It says plainly what it is: a handoff, from which agent and model, to which.
    assert!(prompt.starts_with("PandaCode handoff. This thread was written with Codex (model gpt-test) and continues with you, Claude Code (model sonnet), from this message on."), "{}", &prompt[..200]);
    // The file is under the app data folder, for this thread, and holds every message whole.
    let file = disk.0.join("handoffs").join(&task.id).join("history.md");
    assert!(prompt.contains(&format!("Full history: {} holds every message whole", file.display())));
    let history = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<&str> = history.lines().collect();
    let rows = fixture::rows();
    assert_eq!(ids.len(), rows.len());
    for (id, (role, content, tool)) in ids.iter().zip(&rows) {
        assert!(history.contains(&format!("\n## #{id} ")), "message {id} has no heading");
        if tool.is_none() {
            assert!(history.contains(&format!("\n## #{id} {}\n{content}\n", if *role == "status" { "notice status" } else { role })), "message {id} is not whole in the file");
        }
    }
    assert!(history.contains("END-OF-LONG-REPLY") && history.contains("TAIL-OF-PASTE: the last line says exit 3."));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(file.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }
    // Every pointer leads to the heading of the message it names.
    let pointers = prompt.split_once("## By reference only").unwrap().1.lines().nth(1).unwrap();
    let mut followed = 0;
    for run in pointers.split(", ") {
        let first: i64 = run.trim_start_matches('#').split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap();
        let line: usize = run.split_once("(line ").unwrap().1.trim_end_matches(')').parse().unwrap();
        assert!(lines[line - 1].starts_with(&format!("## #{first} ")), "{run}: {}", lines[line - 1]);
        followed += 1;
    }
    assert!(followed > 0);
    let excerpt = prompt.split_once(" user · excerpt: ").unwrap().1.lines().next().unwrap();
    let line: usize = excerpt.rsplit_once("history file line ").unwrap().1.trim_end_matches(']').parse().unwrap();
    assert!(lines[line - 1].ends_with(" user") && lines[line].starts_with("Here is the failing log from CI"));
    // The thread's notice now says the history was summarised, with what went how.
    let notice = handoff_row(&store, &task.id);
    assert_eq!((notice["fromProvider"].as_str(), notice["toProvider"].as_str(), notice["shortened"].as_bool()), (Some("codex"), Some("claude"), Some(true)));
    assert!(notice["summary"].as_str().unwrap().starts_with("History was summarised for Claude Code (context window 32,000 tokens): "), "{notice}");
    let carried = &notice["carried"];
    assert_eq!(carried["full"].as_u64().unwrap() + carried["summarised"].as_u64().unwrap() + carried["referenced"].as_u64().unwrap(), ids.len() as u64);
    // The file goes with the thread.
    crate::handoff::archive::forget(&disk.0, std::slice::from_ref(&task.id));
    assert!(!file.parent().unwrap().exists());
}

#[test]
fn a_window_with_no_room_refuses_the_turn_and_leaves_everything_as_it_was() {
    let disk = Disk::new();
    let store = Store::open(&disk.database()).unwrap();
    let (task, _) = switched(&store);
    let before = serde_json::to_value(store.messages(&task.id, None).unwrap()).unwrap();
    let error = prepare_with(&store, &task, "Continue", &window(16_000)).unwrap_err();
    assert!(error.starts_with(CANNOT_FIT) && error.contains("Claude Code (sonnet)") && error.contains("16,000 tokens"), "{error}");
    assert!(error.ends_with("Nothing was sent and your message was not shortened."));
    // Still pending, the thread's rows untouched, and a larger model takes it afterwards.
    assert!(pending(&store, &task).unwrap());
    assert_eq!(serde_json::to_value(store.messages(&task.id, None).unwrap()).unwrap(), before);
    assert_eq!(handoff_row(&store, &task.id)["summary"], crate::handoff::SWITCHED);
    // Images attached to the request take room too: two of them fill a 32,000-token window.
    assert!(prepare_with(&store, &task, "Continue", &Options { images: 2, window: Some(32_000) }).unwrap_err().starts_with(CANNOT_FIT));
    assert!(prepare_with(&store, &task, "Continue", &window(200_000)).is_ok());
}

#[test]
fn the_catalog_window_of_the_target_model_sizes_the_handoff() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let (task, _) = switched(&store);
    let context = |prompt: &str| prompt.strip_suffix("Current request:\nContinue").unwrap().len();
    // Unknown model: 128,000 tokens assumed, 96,000 bytes.
    let unknown = prepare(&store, &task, "Continue").unwrap().0;
    assert!(unknown.contains("Budget: context window 128,000 tokens (assumed); 96,000 bytes for this handoff"));
    assert!(context(&unknown) <= 96_000);
    // The window a model last reported in any thread is used when the catalog has none.
    let other = store.create_session(&task.project_id, "claude", "sonnet", "ask").unwrap();
    store.db.execute("UPDATE sessions SET context_window=40000,context_used_tokens=10 WHERE id=?1", [&other.id]).unwrap();
    let reported = prepare(&store, &task, "Continue").unwrap().0;
    assert!(reported.contains("Budget: context window 40,000 tokens; 30,000 bytes for this handoff"));
    // The catalog's window wins.
    crate::handoff::note_models("claude", &[crate::providers::ModelInfo { id: "sonnet".into(), name: "Sonnet".into(), description: String::new(), reasoning_efforts: Vec::new(), default_reasoning_effort: None, context_window: Some(200_000) }]);
    let listed = prepare(&store, &task, "Continue").unwrap().0;
    assert!(listed.contains("Budget: context window 200,000 tokens; 150,000 bytes for this handoff"));
    assert!(context(&listed) <= 150_000 && context(&listed) > context(&reported));
}

#[test]
fn earlier_images_are_referenced_by_their_saved_copies() {
    let disk = Disk::new();
    let store = Store::open(&disk.database()).unwrap();
    let task = task(&store);
    store.message(&task.id, "user", "Match this sketch").unwrap();
    let message = store.db.last_insert_rowid();
    store.message(&task.id, "assistant", "Done").unwrap();
    store
        .db
        .execute(
            "INSERT INTO message_attachments(message_id,session_id,id,kind,name,mime,width,height,bytes,file,source) VALUES (?1,?2,'img-1','image','sketch.png','image/png',10,10,4,?3,'file')",
            params![message, task.id, format!("{}/img-1.png", task.id)],
        )
        .unwrap();
    let (prompt, _) = prepare(&store, &task, "Continue").unwrap();
    let saved = disk.0.join("attachments").join(&task.id).join("img-1.png");
    assert!(prompt.contains(&format!("(attached image “sketch.png”: contents not carried; saved copy: {})", saved.display())), "{prompt}");
    assert!(prompt.contains("1 earlier image attachments are not carried either; each is noted on its message with the path of its saved copy."));
}

#[test]
fn a_session_is_resumed_only_by_the_harness_and_account_that_wrote_it() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let task = task(&store);
    store.message(&task.id, "user", "First request").unwrap();
    store.message(&task.id, "assistant", "Codex answered first").unwrap();
    store.resume(&task.id, "codex-default").unwrap();
    // A model change on the same harness and account keeps its own session: nothing to hand over.
    store.update(&task.id, None, None, Some("gpt-other".into()), None).unwrap();
    let same = store.session(&task.id).unwrap();
    assert_eq!(same.resume_id.as_deref(), Some("codex-default"));
    assert!(!pending(&store, &same).unwrap());
    // Another harness never gets it: a fresh session and the whole conversation.
    let claude = store.configure(&task.id, "claude", "sonnet", None).unwrap();
    assert_eq!(claude.resume_id, None);
    let (prompt, through) = prepare(&store, &claude, "Second request").unwrap();
    assert!(prompt.contains("Codex answered first") && prompt.contains("This thread was written with Codex (model gpt-other)"));
    store.begin_task_turn(&task.id, "Second request").unwrap();
    store.resume(&task.id, "claude-default").unwrap();
    store.message(&task.id, "assistant", "Claude answered second").unwrap();
    store.finish_task_turn(&task.id, true, false).unwrap();
    acknowledge(&store, &task.id, through.unwrap()).unwrap();
    // Back on Codex: its own session, and only what it has not seen.
    let codex = store.configure(&task.id, "codex", "gpt-other", None).unwrap();
    assert_eq!(codex.resume_id.as_deref(), Some("codex-default"));
    assert_eq!(handoff_row(&store, &task.id)["summary"], crate::handoff::SWITCHED_BACK);
    let (prompt, through) = prepare(&store, &codex, "Third request").unwrap();
    assert!(prompt.starts_with("PandaCode handoff. This thread continued with Claude Code (model sonnet) while your own earlier session was set aside;"));
    assert_eq!(prior_entries(&prompt), [("user".to_string(), "Second request".to_string()), ("assistant".into(), "Claude answered second".into())]);
    assert!(!prompt.contains("Codex answered first"));
    store.begin_task_turn(&task.id, "Third request").unwrap();
    store.finish_task_turn(&task.id, true, false).unwrap();
    acknowledge(&store, &task.id, through.unwrap()).unwrap();
    // And back to Claude, with nothing said in between: its session, nothing to hand over
    // but the one request it missed.
    let claude = store.configure(&task.id, "claude", "sonnet", None).unwrap();
    assert_eq!(claude.resume_id.as_deref(), Some("claude-default"));
    let (prompt, _) = prepare(&store, &claude, "Fourth").unwrap();
    assert_eq!(prior_entries(&prompt), [("user".to_string(), "Third request".to_string())]);

    // Accounts of one harness: each keeps its own session, and none resumes another's.
    let switch = |from: Option<&str>, to: Option<&str>| {
        crate::connections::switch_account(&store, &task.id, from, to, true).unwrap();
        store.session(&task.id).unwrap().resume_id
    };
    assert_eq!(switch(None, Some("work")), None, "the work account never resumes the default account's session");
    assert!(pending(&store, &store.session(&task.id).unwrap()).unwrap());
    store.resume(&task.id, "claude-work").unwrap();
    assert_eq!(switch(Some("work"), Some("personal")), None);
    store.resume(&task.id, "claude-personal").unwrap();
    assert_eq!(switch(Some("personal"), Some("work")).as_deref(), Some("claude-work"));
    assert_eq!(switch(Some("work"), None).as_deref(), Some("claude-default"));
    assert_eq!(switch(None, Some("personal")).as_deref(), Some("claude-personal"));
    // A session set aside too long ago may be gone from the harness: it is not tried.
    store.db.execute("UPDATE session_resume_ids SET updated_at=updated_at-?1", [crate::handoff::resume::MAX_AGE_MS + 1]).unwrap();
    assert_eq!(switch(Some("personal"), Some("work")), None);
    // A removed account's sessions are forgotten.
    assert_eq!(switch(Some("work"), Some("personal")).as_deref(), Some("claude-personal"));
    store.resume(&task.id, "claude-personal-2").unwrap();
    crate::connections::switch_account(&store, &task.id, Some("personal"), Some("work"), true).unwrap();
    crate::handoff::resume::forget_account(&store, "personal").unwrap();
    assert_eq!(switch(Some("work"), Some("personal")), None);
    // Keys never cross harnesses either: Codex's default-account session is still Codex's.
    let rows: Vec<(String, String, String)> = store
        .db
        .prepare("SELECT provider,account,resume_id FROM session_resume_ids WHERE session_id=?1 ORDER BY provider,account")
        .unwrap()
        .query_map([&task.id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(rows.iter().all(|(provider, _, id)| id.starts_with(provider.as_str())), "{rows:?}");
}

#[test]
fn the_goal_survives_a_harness_switch_an_account_switch_and_a_new_session() {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let task = task(&store);
    store.message(&task.id, "user", "Work towards the goal").unwrap();
    store.resume(&task.id, "codex-thread").unwrap();
    let goal = crate::goal_state::GoalState { thread_id: "codex-thread".into(), objective: "Ship the offline search index".into(), status: "active".into(), token_budget: None, tokens_used: 40, time_used_seconds: 9, created_at: 1, updated_at: 2 };
    store.record_goal(&task.id, Some(&goal)).unwrap();
    // Another harness: the goal stays, paused, since only Codex runs it.
    let switched = store.configure(&task.id, "claude", "sonnet", None).unwrap();
    let kept = switched.goal_state.clone().expect("the goal was erased by the switch");
    assert_eq!((kept.objective.as_str(), kept.status.as_str(), kept.tokens_used), ("Ship the offline search index", "paused", 40));
    // The new agent is told about it.
    let (prompt, _) = prepare(&store, &switched, "Continue").unwrap();
    assert!(prompt.contains("Thread goal (paused): Ship the offline search index"));
    // Its first session id does not erase the goal, and neither does a change of account.
    store.resume(&task.id, "claude-session").unwrap();
    assert_eq!(store.session(&task.id).unwrap().goal_state.unwrap().objective, "Ship the offline search index");
    crate::connections::switch_account(&store, &task.id, None, Some("work"), true).unwrap();
    assert_eq!(store.session(&task.id).unwrap().goal_state.unwrap().status, "paused");
    // Back on Codex the goal is still there for the user to resume.
    let back = store.configure(&task.id, "codex", "gpt-test", None).unwrap();
    assert_eq!(back.goal_state.unwrap().objective, "Ship the offline search index");
}
