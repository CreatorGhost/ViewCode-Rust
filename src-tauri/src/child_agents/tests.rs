use super::tools::{self, Env};
use super::*;
use crate::concurrent_task_tests::test_state;
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::Path;

/// The tool environment of a test: a clock, which agents are busy, the launch gate's answer
/// and worktrees as plain folders (or real git worktrees with `git`).
struct TestEnv {
    now: Cell<i64>,
    busy: RefCell<HashSet<String>>,
    refused: RefCell<Option<String>>,
    worktrees: RefCell<Vec<PathBuf>>,
    git: bool,
}

impl TestEnv {
    fn new() -> Self {
        Self { now: Cell::new(1_000), busy: Default::default(), refused: Default::default(), worktrees: Default::default(), git: false }
    }
    fn tick(&self) -> i64 {
        self.now.set(self.now.get() + 1);
        self.now.get()
    }
    fn set_busy(&self, id: &str, busy: bool) {
        if busy {
            self.busy.borrow_mut().insert(id.into());
        } else {
            self.busy.borrow_mut().remove(id);
        }
    }
}

impl Env for TestEnv {
    fn busy(&self, sid: &str) -> bool {
        self.busy.borrow().contains(sid)
    }
    fn gate(&self, harness: &str) -> Result<()> {
        match &*self.refused.borrow() {
            Some(refused) if refused == harness => {
                let gate = crate::launch_gate::Gate::default();
                Err(gate.check(harness, crate::providers::binary_name(harness)).unwrap_err().message("Codex"))
            }
            _ => Ok(()),
        }
    }
    fn worktree(&self, plan: &SpawnPlan) -> Result<PathBuf> {
        let path = if self.git {
            crate::workspace::create_worktree_with(&plan.checkout, &plan.branch, None, false)?.path
        } else {
            let path = std::env::temp_dir().join(format!("pandacode-child-{}", crate::storage::id()));
            std::fs::create_dir_all(&path).unwrap();
            path
        };
        self.worktrees.borrow_mut().push(path.clone());
        Ok(path)
    }
    fn now(&self) -> i64 {
        self.tick()
    }
}

fn store_in(dir: &Path) -> (Store, Session) {
    let store = Store::open(Path::new(":memory:")).unwrap();
    let project = store.add_project(dir).unwrap();
    let parent = store.create_session(&project.id, "claude", "default", "auto").unwrap();
    (store, parent)
}

fn fixture() -> (Store, Session, TestEnv) {
    let (store, parent) = store_in(&std::env::temp_dir());
    (store, parent, TestEnv::new())
}

fn spawn(store: &Store, env: &TestEnv, caller: &str, name: &str) -> String {
    let result = tools::call(store, env, caller, None, tools::SPAWN, &json!({"name": name, "prompt": format!("Do the {name} part.")})).unwrap();
    result["agentId"].as_str().unwrap().to_owned()
}

fn send_to(store: &Store, env: &TestEnv, caller: &str, run: Option<&str>, to: &str, body: &str, reply: bool) -> Result<Value> {
    tools::call(store, env, caller, run, tools::SEND, &json!({"to": to, "message": body, "reply_expected": reply}))
}

use serde_json::Value;

/// Delivers `sid`'s next held message to a run called `run`, as `pump` and the run's start do.
fn deliver(store: &Store, sid: &str, run: &str, now: i64) -> Mail {
    let next = ready(store, sid).unwrap().expect("a message was ready");
    check_claimable(store, &next.id, sid).unwrap();
    claim(store, &next.id, sid, run, now).unwrap();
    mail(store, &next.id).unwrap().unwrap()
}

fn done(answer: &str) -> Outcome {
    Outcome::Completed { answer: Some(answer.into()) }
}

#[test]
fn a_child_is_a_session_of_kind_child_with_the_general_parent_link_and_is_hidden_from_lists() {
    let (store, parent, env) = fixture();
    let id = spawn(&store, &env, &parent.id, "Frontend audit");
    let child_session = store.session(&id).unwrap();
    assert_eq!(child_session.kind, KIND);
    assert_eq!(child_session.parent_session_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!((child_session.title.as_str(), child_session.provider.as_str(), child_session.model.as_str()), ("Frontend audit", "claude", "default"));
    let row = child(&store, &id).unwrap().unwrap();
    assert_eq!((row.root_session_id.as_str(), row.depth, row.parent_session_id.as_str()), (parent.id.as_str(), 1, parent.id.as_str()));
    // Not in the sidebar, boards, tray, phones (`Store::sessions`) or search.
    let listed: Vec<String> = store.sessions().unwrap().into_iter().map(|session| session.id).collect();
    assert!(listed.contains(&parent.id) && !listed.contains(&id));
    store.message(&id, "user", "searchable child words").unwrap();
    assert!(store.search_tasks(&parent.project_id, "searchable child", 10).unwrap().is_empty());
    // Nested under its parent instead, and listed once opened as a thread.
    let snapshot = snapshot(&store, &parent.id, &|_| false).unwrap();
    assert_eq!(snapshot.children.iter().map(|agent| agent.session_id.as_str()).collect::<Vec<_>>(), [id.as_str()]);
    let opened = open_as_thread(&store, &id, 5_000).unwrap();
    assert_eq!((opened.kind.as_str(), opened.parent_session_id.as_deref()), ("thread", Some(parent.id.as_str())));
    assert!(store.sessions().unwrap().iter().any(|session| session.id == id));
    assert_eq!(snapshot_ids(&store, &parent.id), [id.clone()], "an opened child stays in its tree");
    assert_eq!(open_as_thread(&store, &parent.id, 1).unwrap_err(), "This is not a child agent.");
}

fn snapshot_ids(store: &Store, sid: &str) -> Vec<String> {
    snapshot(store, sid, &|_| false).unwrap().children.into_iter().map(|agent| agent.session_id).collect()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "Fixture").env("GIT_AUTHOR_EMAIL", "fixture@example.invalid").env("GIT_COMMITTER_NAME", "Fixture").env("GIT_COMMITTER_EMAIL", "fixture@example.invalid").output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn repository() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pandacode-child-repo-{}", crate::storage::id())).join("project");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "fixture\n").unwrap();
    git(&dir, &["add", "README.md"]);
    git(&dir, &["commit", "-q", "-m", "first"]);
    dir.canonicalize().unwrap()
}

#[test]
fn each_child_gets_its_own_worktree_from_the_parents_commit_and_deletion_removes_clean_ones() {
    let repo = repository();
    let (store, parent) = store_in(&repo);
    let mut env = TestEnv::new();
    env.git = true;
    // An uncommitted edit in the parent is not copied: the child starts from its commit.
    std::fs::write(repo.join("draft.txt"), "uncommitted").unwrap();
    let one = spawn(&store, &env, &parent.id, "Search index");
    let two = spawn(&store, &env, &parent.id, "Search index");
    let (one, two) = (store.session(&one).unwrap(), store.session(&two).unwrap());
    let (path_one, path_two) = (PathBuf::from(one.worktree_path.clone().unwrap()), PathBuf::from(two.worktree_path.clone().unwrap()));
    assert_ne!(path_one, path_two);
    assert_ne!(path_one, repo);
    assert!(one.worktree_branch.as_deref().unwrap().starts_with("pandacode/agent-search-index-"));
    assert_ne!(one.worktree_branch, two.worktree_branch);
    assert_eq!(git(&path_one, &["rev-parse", "HEAD"]), git(&repo, &["rev-parse", "HEAD"]));
    assert!(path_one.join("README.md").is_file() && !path_one.join("draft.txt").exists());
    assert_eq!(store.workspace_root(&parent.project_id, Some(&one.id)).unwrap(), path_one);

    // Deleting the parent deletes its children; their clean worktrees go, a dirty one stays.
    std::fs::write(path_two.join("work.txt"), "unsaved work").unwrap();
    let (state, _) = test_state();
    *state.store.lock().unwrap() = store;
    let ids = {
        let store = state.store.lock().unwrap();
        let mut ids = vec![parent.id.clone()];
        ids.extend(descendants(&store, &parent.id).unwrap());
        ids
    };
    let list = worktrees(&state.store.lock().unwrap(), &ids);
    assert_eq!(list.len(), 2);
    let deleted = crate::task_deletion::delete_state(&state, &parent.id).unwrap();
    assert_eq!(deleted.deleted_session_ids.len(), 3);
    let kept = remove_worktrees(&list);
    assert!(!path_one.exists(), "a clean child checkout is removed");
    assert!(git(&repo, &["branch", "--list", one.worktree_branch.as_deref().unwrap()]).is_empty(), "with its merged branch");
    assert_eq!(kept.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>(), [path_two.clone()]);
    assert!(path_two.join("work.txt").is_file(), "uncommitted work is kept");
    let store = state.store.lock().unwrap();
    for id in [&parent.id, &one.id, &two.id] {
        assert!(store.session(id).is_err());
    }
    let left: i64 = store.db.query_row("SELECT (SELECT COUNT(*) FROM child_agents)+(SELECT COUNT(*) FROM agent_mail)+(SELECT COUNT(*) FROM agent_control)", [], |row| row.get(0)).unwrap();
    assert_eq!(left, 0);
}

#[test]
fn a_spawn_holds_the_prompt_as_the_childs_first_message_in_the_envelope() {
    let (store, parent, env) = fixture();
    let id = spawn(&store, &env, &parent.id, "Docs");
    let first = ready(&store, &id).unwrap().unwrap();
    assert_eq!((first.kind.as_str(), first.from_session.as_str(), first.hop, first.reply_expected), ("spawn", parent.id.as_str(), 0, true));
    let text = envelope(&first);
    assert!(text.starts_with(&format!("<pandacode-agent-message from=\"{}\" from-id=\"{}\" message-id=\"{}\" reply-expected=\"true\">\nDo the Docs part.\n</pandacode-agent-message>", parent.title, parent.id, first.id)), "{text}");
    assert!(text.contains(&format!("pandacode_send_message tool (to=\"{}\", response_id=\"{}\")", parent.id, first.id)));
    let quiet = tools::call(&store, &env, &parent.id, None, tools::SPAWN, &json!({"name":"Quiet","prompt":"x","reply_expected":false})).unwrap();
    let quiet = ready(&store, quiet["agentId"].as_str().unwrap()).unwrap().unwrap();
    assert!(envelope(&quiet).ends_with("No reply is required."));
}

#[test]
fn a_reply_comes_from_the_run_that_took_the_message_only() {
    let (store, parent, env) = fixture();
    let id = spawn(&store, &env, &parent.id, "Worker");
    let taken = deliver(&store, &id, "run-a", env.tick());
    // Another run of the child (the user typed into it) finishes first: nothing is routed.
    let other = run_finished(&store, &id, "run-b", &done("an answer to the user"), env.tick()).unwrap();
    assert!(other.replies.is_empty());
    assert!(held(&store, &parent.id).unwrap().is_empty());
    // The run that took the spawn answers it.
    let ended = run_finished(&store, &id, "run-a", &done("The audit found 3 issues."), env.tick()).unwrap();
    assert_eq!(ended.replies.len(), 1);
    assert!(ended.wake.contains(&parent.id) && ended.wake.contains(&id));
    let reply = held(&store, &parent.id).unwrap().remove(0);
    assert_eq!((reply.kind.as_str(), reply.body.as_str(), reply.in_reply_to.as_deref(), reply.hop, reply.from_name.as_str()), ("reply", "The audit found 3 issues.", Some(taken.id.as_str()), 1, "Worker"));
    assert!(envelope(&reply).contains(&format!("in-reply-to=\"{}\"", taken.id)));
    assert!(envelope(&reply).ends_with(&format!("This is Worker's reply to your message {}.", taken.id)));
    // A second end of the same run sends nothing more.
    assert!(run_finished(&store, &id, "run-a", &done("again"), env.tick()).unwrap().replies.is_empty());
}

#[test]
fn a_failed_run_answers_with_why_and_a_stopped_one_sends_nothing() {
    let (store, parent, env) = fixture();
    let id = spawn(&store, &env, &parent.id, "Worker");
    deliver(&store, &id, "run-1", env.tick());
    run_finished(&store, &id, "run-1", &Outcome::Failed { answer: None, error: "rate limited".into() }, env.tick()).unwrap();
    assert_eq!(held(&store, &parent.id).unwrap()[0].body, "(failed without an answer: rate limited)");
    send_to(&store, &env, &parent.id, None, &id, "Try again", true).unwrap();
    deliver(&store, &id, "run-2", env.tick());
    let stopped = run_finished(&store, &id, "run-2", &Outcome::Stopped, env.tick()).unwrap();
    assert!(stopped.replies.is_empty());
    send_to(&store, &env, &parent.id, None, &id, "Third", true).unwrap();
    deliver(&store, &id, "run-3", env.tick());
    run_finished(&store, &id, "run-3", &Outcome::Completed { answer: None }, env.tick()).unwrap();
    assert_eq!(held(&store, &parent.id).unwrap().last().unwrap().body, "(finished without a written answer)");
}

#[test]
fn an_explicit_reply_with_response_id_is_not_sent_twice() {
    let (store, parent, env) = fixture();
    let id = spawn(&store, &env, &parent.id, "Worker");
    let taken = deliver(&store, &id, "run-1", env.tick());
    let sent = tools::call(&store, &env, &id, Some("run-1"), tools::SEND, &json!({"to": parent.id, "message": "Done early", "response_id": taken.id})).unwrap();
    assert_eq!(sent["delivery"], "started");
    run_finished(&store, &id, "run-1", &done("final words"), env.tick()).unwrap();
    let mail = held(&store, &parent.id).unwrap();
    assert_eq!(mail.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["Done early"]);
    assert_eq!(mail[0].in_reply_to.as_deref(), Some(taken.id.as_str()));
    let wrong = tools::call(&store, &env, &parent.id, None, tools::SEND, &json!({"to": id, "message": "x", "response_id": taken.id})).unwrap_err();
    assert_eq!(wrong, "That response id does not belong to a message from this agent.");
}

#[test]
fn a_ping_pong_is_cut_at_the_hop_limit() {
    let (store, parent, env) = fixture();
    let id = spawn(&store, &env, &parent.id, "Echo");
    // The spawn is hop 0; each side answers what started its run with a new message.
    let mut run = 0;
    let mut receiver = id.clone();
    let mut sender = parent.id.clone();
    let mut hops = vec![0];
    loop {
        run += 1;
        let run_id = format!("run-{run}");
        deliver(&store, &receiver, &run_id, env.tick());
        match send_to(&store, &env, &receiver, Some(&run_id), &sender, "ping", true) {
            Ok(_) => {
                hops.push(held(&store, &sender).unwrap().last().unwrap().hop);
                run_finished(&store, &receiver, &run_id, &Outcome::Stopped, env.tick()).unwrap();
                std::mem::swap(&mut receiver, &mut sender);
            }
            Err(error) => {
                assert_eq!(error, "This conversation reached 24 automatic hops. Summarize the state for the user.");
                break;
            }
        }
    }
    assert_eq!(hops, (0..MAX_HOPS).collect::<Vec<_>>());
    // A message nobody was started by begins a new chain.
    assert!(send_to(&store, &env, &parent.id, None, &id, "fresh start", false).is_ok());
    assert_eq!(held(&store, &id).unwrap().last().unwrap().hop, 0);
}

#[test]
fn an_automatic_reply_past_the_hop_limit_is_dropped() {
    let (store, parent, env) = fixture();
    let id = spawn(&store, &env, &parent.id, "Worker");
    let taken = deliver(&store, &id, "run-1", env.tick());
    store.db.execute("UPDATE agent_mail SET hop=?2 WHERE id=?1", params![taken.id, MAX_HOPS - 1]).unwrap();
    let ended = run_finished(&store, &id, "run-1", &done("answer"), env.tick()).unwrap();
    assert!(ended.replies.is_empty());
    assert!(ended.dropped.unwrap().contains("24 automatic hops"));
}

#[test]
fn mail_to_a_busy_or_paused_agent_is_held_and_can_be_discarded() {
    let (store, parent, env) = fixture();
    let id = spawn(&store, &env, &parent.id, "Worker");
    deliver(&store, &id, "run-1", env.tick());
    env.set_busy(&id, true);
    let sent = send_to(&store, &env, &parent.id, None, &id, "one more thing", false).unwrap();
    assert_eq!(sent["delivery"], "queued");
    env.set_busy(&id, false);
    pause(&store, &id).unwrap();
    assert_eq!(send_to(&store, &env, &parent.id, None, &id, "and another", false).unwrap()["delivery"], "queued");
    assert!(ready(&store, &id).unwrap().is_none(), "nothing starts while paused");
    let views = held_views(&store, &id).unwrap();
    assert_eq!(views.iter().map(|m| m.preview.as_str()).collect::<Vec<_>>(), ["one more thing", "and another"]);
    assert_eq!(views[0].from_name, parent.title);
    assert_eq!(snapshot(&store, &parent.id, &|_| false).unwrap().children[0].held_mail, 2);
    // Discard one: the agent stays paused with the other.
    assert_eq!(discard(&store, &id, Some(&views[0].id)).unwrap(), 1);
    assert!(control(&store, &id).unwrap().paused);
    assert_eq!(held(&store, &id).unwrap().len(), 1);
    // Discard all: unpaused, nothing to start, nothing woken.
    assert_eq!(discard(&store, &id, None).unwrap(), 1);
    assert_eq!(control(&store, &id).unwrap(), Control::default());
    assert!(ready(&store, &id).unwrap().is_none());
    assert_eq!(mail(&store, &views[0].id).unwrap().unwrap().state, "discarded");
    // A discarded message cannot start a turn any more.
    assert!(check_claimable(&store, &views[0].id, &id).is_err());
}

#[test]
fn held_mail_and_pauses_survive_a_restart_and_a_cut_off_run_resumes_still_answering() {
    let path = std::env::temp_dir().join(format!("pandacode-child-agents-{}.db", crate::storage::id()));
    let env = TestEnv::new();
    let (parent, child_id, first) = {
        let store = Store::open(&path).unwrap();
        let project = store.add_project(&std::env::temp_dir()).unwrap();
        let parent = store.create_session(&project.id, "claude", "default", "auto").unwrap();
        let child_id = spawn(&store, &env, &parent.id, "Worker");
        let first = deliver(&store, &child_id, "run-before-quit", env.tick());
        env.set_busy(&child_id, true);
        send_to(&store, &env, &parent.id, None, &child_id, "held across the restart", false).unwrap();
        (parent, child_id, first)
    };
    let store = Store::open(&path).unwrap();
    assert_eq!(held(&store, &child_id).unwrap().iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["held across the restart"]);
    // The run that had the spawn never ended: its agent is paused with that turn remembered.
    assert!(recover(&store).unwrap().is_empty());
    let control = control(&store, &child_id).unwrap();
    assert_eq!(control, Control { paused: true, interrupted: true, interrupted_mail: Some(first.id.clone()) });
    // Resume continues it first, still owing the parent its answer, then the held mail.
    assert!(resume(&store, &child_id, env.tick()).unwrap());
    let next = deliver(&store, &child_id, "run-after", env.tick());
    assert_eq!((next.kind.as_str(), next.body.as_str(), next.reply_expected, next.from_session.as_str()), ("continue", CONTINUE_PROMPT, true, parent.id.as_str()));
    run_finished(&store, &child_id, "run-after", &done("finished after the restart"), env.tick()).unwrap();
    assert_eq!(held(&store, &parent.id).unwrap()[0].in_reply_to.as_deref(), Some(first.id.as_str()));
    assert_eq!(ready(&store, &child_id).unwrap().unwrap().body, "held across the restart");
    drop(store);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

#[test]
fn pausing_one_child_pauses_its_descendants_and_leaves_siblings_and_parent_alone() {
    let (store, parent, env) = fixture();
    let one = spawn(&store, &env, &parent.id, "One");
    let two = spawn(&store, &env, &parent.id, "Two");
    let grandchild = spawn(&store, &env, &one, "Grandchild");
    let scope = pause(&store, &one).unwrap();
    assert_eq!(scope, [one.clone(), grandchild.clone()]);
    for (id, paused) in [(&one, true), (&grandchild, true), (&two, false), (&parent.id, false)] {
        assert_eq!(control(&store, id).unwrap().paused, paused, "{id}");
    }
    assert!(ready(&store, &two).unwrap().is_some(), "a sibling keeps getting its mail");
    let views = snapshot(&store, &parent.id, &|id| id == two).unwrap().children;
    let statuses: Vec<(&str, &str, i64)> = views.iter().map(|agent| (agent.name.as_str(), agent.status.as_str(), agent.depth)).collect();
    assert_eq!(statuses, [("One", "paused", 1), ("Grandchild", "paused", 2), ("Two", "running", 1)]);
    // A user stop mid-turn is remembered; Resume continues it, and a plain user turn clears it.
    deliver(&store, &two, "two-run", env.tick());
    pause(&store, &two).unwrap();
    run_finished(&store, &two, "two-run", &Outcome::Stopped, env.tick()).unwrap();
    assert!(control(&store, &two).unwrap().interrupted);
    assert!(user_prompted(&store, &two).unwrap());
    assert_eq!(control(&store, &two).unwrap(), Control::default());
    assert!(!user_prompted(&store, &two).unwrap());
    // Resuming an agent whose stopped turn was the user's own continues it as plain text.
    pause(&store, &one).unwrap();
    run_finished(&store, &one, "user-turn", &Outcome::Stopped, env.tick()).unwrap();
    resume(&store, &one, env.tick()).unwrap();
    let next = ready(&store, &one).unwrap().unwrap();
    assert_eq!(envelope(&next), CONTINUE_PROMPT);
    assert!(control(&store, &grandchild).unwrap().paused, "resuming one agent leaves the agents it started paused");
}

#[test]
fn the_stop_tool_reaches_only_the_callers_descendants() {
    let (store, parent, env) = fixture();
    let one = spawn(&store, &env, &parent.id, "One");
    let two = spawn(&store, &env, &parent.id, "Two");
    let error = tools::call(&store, &env, &one, None, tools::STOP, &json!({"agent": two})).unwrap_err();
    assert!(error.contains("only agents you started"));
    assert!(tools::call(&store, &env, &one, None, tools::STOP, &json!({"agent": parent.id})).is_err());
    let stopped = tools::call(&store, &env, &parent.id, None, tools::STOP, &json!({"agent": "Two"})).unwrap();
    assert_eq!(stopped["status"], "paused");
    assert!(control(&store, &two).unwrap().paused && !control(&store, &one).unwrap().paused);
}

#[test]
fn spawning_is_limited_in_count_and_depth() {
    let (store, parent, env) = fixture();
    let mut ids = Vec::new();
    for index in 0..MAX_CHILDREN {
        ids.push(spawn(&store, &env, &parent.id, &format!("Child {index}")));
    }
    let error = tools::call(&store, &env, &parent.id, None, tools::SPAWN, &json!({"name":"One too many","prompt":"x"})).unwrap_err();
    assert_eq!(error, "You already have 8 child agents. Wait for them, or stop one, before starting another.");
    // An archived child no longer counts.
    store.db.execute("UPDATE sessions SET archived=1 WHERE id=?1", [&ids[0]]).unwrap();
    spawn(&store, &env, &parent.id, "Replacement");
    let depth1 = ids[1].clone();
    let depth2 = spawn(&store, &env, &depth1, "Depth 2");
    let depth3 = spawn(&store, &env, &depth2, "Depth 3");
    assert_eq!(child(&store, &depth3).unwrap().unwrap().depth, MAX_DEPTH);
    assert_eq!(child(&store, &depth3).unwrap().unwrap().root_session_id, parent.id);
    let error = tools::call(&store, &env, &depth3, None, tools::SPAWN, &json!({"name":"Too deep","prompt":"x"})).unwrap_err();
    assert!(error.starts_with("Child agents can be nested 3 deep"), "{error}");
}

#[test]
fn a_child_inherits_the_parents_mode_and_can_never_exceed_it() {
    let (store, parent, env) = fixture();
    let id = spawn(&store, &env, &parent.id, "Worker");
    let grandchild = spawn(&store, &env, &id, "Grandchild");
    assert_eq!(store.session(&id).unwrap().mode, "auto");
    // Raised on the child later: each turn still runs in the parent's mode.
    store.db.execute("UPDATE sessions SET mode='full' WHERE id=?1", [&id]).unwrap();
    assert_eq!(effective_mode(&store, &store.session(&id).unwrap()), "auto");
    // Lowered on the parent: the whole tree below it is lowered.
    store.db.execute("UPDATE sessions SET mode='plan' WHERE id=?1", [&parent.id]).unwrap();
    assert_eq!(effective_mode(&store, &store.session(&id).unwrap()), "plan");
    assert_eq!(effective_mode(&store, &store.session(&grandchild).unwrap()), "plan");
    assert_eq!(view(&store, &grandchild, &|_| false).unwrap().mode, "plan");
    // A child may run narrower than its parent.
    store.db.execute("UPDATE sessions SET mode='full' WHERE id=?1", [&parent.id]).unwrap();
    store.db.execute("UPDATE sessions SET mode='ask' WHERE id=?1", [&id]).unwrap();
    assert_eq!(effective_mode(&store, &store.session(&id).unwrap()), "ask");
    assert_eq!(effective_mode(&store, &store.session(&grandchild).unwrap()), "ask");
    // A new spawn takes the parent's effective mode.
    let narrow = spawn(&store, &env, &id, "Narrow");
    assert_eq!(store.session(&narrow).unwrap().mode, "ask");
    assert_eq!((narrower("full", "plan"), narrower("review", "auto")), ("plan", "review"));
}

#[test]
fn a_turn_start_binds_the_message_to_its_run_and_caps_the_mode() {
    let (state, project) = test_state();
    let env = TestEnv::new();
    let (parent, id) = {
        let store = state.store.lock().unwrap();
        let parent = store.create_session(&project, "claude", "default", "ask").unwrap();
        let id = spawn(&store, &env, &parent.id, "Worker");
        store.db.execute("UPDATE sessions SET mode='full' WHERE id=?1", [&id]).unwrap();
        (parent, id)
    };
    let first = ready(&state.store.lock().unwrap(), &id).unwrap().unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let prepared = crate::prepare_task_run_in_project(&state, id.clone(), Some(envelope(&first)), Vec::new(), tx, Arc::new(std::sync::atomic::AtomicBool::new(false)), crate::RunAdmission { mail: Some(first.id.clone()), ..Default::default() }).unwrap();
    assert_eq!(prepared.config.mode, "ask", "the child's turn runs in its parent's mode");
    assert!(prepared.config.prompt.contains("Do the Worker part."));
    let store = state.store.lock().unwrap();
    let bound = mail(&store, &first.id).unwrap().unwrap();
    assert_eq!((bound.state.as_str(), bound.run_id.as_deref()), ("delivered", Some(prepared.run_id.as_str())));
    drop(store);
    state.agents.lock().unwrap().remove(&id);
    // The same message cannot start a second turn.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let again = crate::prepare_task_run_in_project(&state, id.clone(), Some("again".into()), Vec::new(), tx, Arc::new(std::sync::atomic::AtomicBool::new(false)), crate::RunAdmission { mail: Some(first.id.clone()), ..Default::default() });
    assert_eq!(again.err().unwrap(), "This agent message was already delivered or discarded.");
    let _ = parent;
}

#[test]
fn a_spawn_naming_an_agent_that_is_off_returns_the_gates_refusal_and_starts_nothing() {
    let (store, parent, env) = fixture();
    *env.refused.borrow_mut() = Some("codex".into());
    let before: i64 = store.db.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0)).unwrap();
    let error = tools::call(&store, &env, &parent.id, None, tools::SPAWN, &json!({"name":"Codex helper","prompt":"x","provider_id":"codex"})).unwrap_err();
    assert_eq!(error, "Codex is not enabled. Turn it on in Settings → Providers before using it.");
    assert_eq!(error, crate::launch_gate::Refused::NotEnabled.message("Codex"));
    let after: i64 = store.db.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0)).unwrap();
    assert_eq!(before, after);
    assert!(env.worktrees.borrow().is_empty(), "no worktree was made");
    let mail: i64 = store.db.query_row("SELECT COUNT(*) FROM agent_mail", [], |row| row.get(0)).unwrap();
    assert_eq!(mail, 0);
    // A policy block reads the policy's own sentence.
    let mut gate = crate::launch_gate::Gate::default();
    gate.policy.blocked.insert("codex".into());
    let blocked = plan_spawn(&store, &parent.id, None, SpawnInput { name: "x", task: None, prompt: "x", provider: Some("codex"), model: None, effort: None, reply_expected: None }, &|harness| gate.check(harness, "codex").map_err(|refused| refused.message("Codex"))).unwrap_err();
    assert_eq!(blocked, "Codex is blocked by your organization, so it was not started.");
    // An unknown harness is refused before the gate is asked.
    assert_eq!(tools::call(&store, &env, &parent.id, None, tools::SPAWN, &json!({"name":"x","prompt":"x","provider_id":"nope"})).unwrap_err(), "Unknown agent: nope.");
}

#[test]
fn side_chats_team_threads_and_remote_tasks_cannot_spawn() {
    let (store, parent, env) = fixture();
    let side = crate::side_chat::open(&store, &parent.id, "q", 10).unwrap();
    assert_eq!(tools::call(&store, &env, &side.id, None, tools::SPAWN, &json!({"name":"x","prompt":"x"})).unwrap_err(), "A side chat cannot start child agents.");
    store.db.execute("UPDATE sessions SET remote_device='device-1' WHERE id=?1", [&parent.id]).unwrap();
    assert!(tools::call(&store, &env, &parent.id, None, tools::SPAWN, &json!({"name":"x","prompt":"x"})).unwrap_err().contains("runs elsewhere"));
}

#[test]
fn agents_reach_only_their_own_tree_and_list_it_with_relations() {
    let (store, parent, env) = fixture();
    let one = spawn(&store, &env, &parent.id, "One");
    let two = spawn(&store, &env, &parent.id, "Two");
    let stranger = store.create_session(&parent.project_id, "claude", "default", "auto").unwrap();
    assert_eq!(send_to(&store, &env, &one, None, &stranger.id, "hi", false).unwrap_err(), "The agent is not in your agent tree.");
    assert_eq!(send_to(&store, &env, &one, None, &one, "hi", false).unwrap_err(), "You cannot send a message to yourself.");
    // A child messages its parent and its sibling by name.
    assert_eq!(send_to(&store, &env, &one, None, &parent.title, "status", false).unwrap()["delivery"], "started");
    assert!(send_to(&store, &env, &one, None, "Two", "hello", false).is_ok());
    spawn(&store, &env, &parent.id, "Two");
    assert_eq!(send_to(&store, &env, &one, None, "Two", "hello", false).unwrap_err(), "Several agents have that name. Use its id.");
    env.set_busy(&two, true);
    let listed = tools::call(&store, &env, &one, None, tools::LIST, &json!({})).unwrap();
    let rows: Vec<(String, String, String)> = listed["agents"].as_array().unwrap().iter().map(|agent| (agent["name"].as_str().unwrap().to_owned(), agent["relation"].as_str().unwrap().to_owned(), agent["status"].as_str().unwrap().to_owned())).collect();
    assert_eq!(rows[0], (parent.title.clone(), "parent".into(), "idle".into()));
    assert_eq!(rows[1], ("One".into(), "you".into(), "new".into()));
    assert_eq!(rows[2], ("Two".into(), "sibling".into(), "running".into()));
    assert_eq!(listed["agents"][2]["queuedMessages"], 2);
}

#[test]
fn wait_collects_finished_answers_instead_of_delivering_them_again() {
    let (store, parent, env) = fixture();
    let one = spawn(&store, &env, &parent.id, "One");
    let two = spawn(&store, &env, &parent.id, "Two");
    deliver(&store, &one, "one-run", env.tick());
    deliver(&store, &two, "two-run", env.tick());
    env.set_busy(&one, true);
    env.set_busy(&two, true);
    let waiting = tools::wait_check(&store, &env, &parent.id, &[]).unwrap();
    assert_eq!(waiting["done"], false);
    run_finished(&store, &one, "one-run", &done("one says hi"), env.tick()).unwrap();
    env.set_busy(&one, false);
    let partial = tools::wait_check(&store, &env, &parent.id, &[one.clone()]).unwrap();
    assert_eq!((partial["done"].as_bool(), partial["agents"][0]["answer"].as_str(), partial["agents"][0]["collected"].as_bool()), (Some(true), Some("one says hi"), Some(true)));
    assert!(held(&store, &parent.id).unwrap().is_empty(), "a collected answer does not also start a turn");
    assert!(tools::wait_check(&store, &env, &one, &[two.clone()]).unwrap_err().contains("only for your own child agents"));
}

#[test]
fn read_transcript_returns_the_newest_messages_of_a_tree_member() {
    let (store, parent, env) = fixture();
    let one = spawn(&store, &env, &parent.id, "One");
    for index in 0..20 {
        store.message(&one, if index % 2 == 0 { "user" } else { "assistant" }, &format!("line {index}")).unwrap();
    }
    let read = tools::call(&store, &env, &parent.id, None, tools::READ, &json!({"agent": "One", "last_messages": 3})).unwrap();
    assert_eq!(read["transcript"], "assistant: line 17\n\nuser: line 18\n\nassistant: line 19");
}

#[test]
fn a_message_that_cannot_start_bounces_a_reply_to_its_sender() {
    let (store, parent, env) = fixture();
    let id = spawn(&store, &env, &parent.id, "Worker");
    let first = ready(&store, &id).unwrap().unwrap();
    assert_eq!(bounce(&store, &first.id, "Codex is not enabled.", env.tick()).unwrap(), Some(parent.id.clone()));
    assert_eq!(mail(&store, &first.id).unwrap().unwrap().state, "failed");
    assert_eq!(held(&store, &parent.id).unwrap()[0].body, "(could not start: Codex is not enabled.)");
    assert_eq!(bounce(&store, &first.id, "again", env.tick()).unwrap(), None);
}

#[test]
fn the_schema_step_creates_the_tables_and_the_tools_are_well_formed() {
    let (store, _parent, _env) = fixture();
    assert_eq!(crate::storage::SCHEMA_VERSION, 42);
    assert_eq!(store.db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(), 42);
    for table in ["child_agents", "agent_control", "agent_mail"] {
        let exists: bool = store.db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)", [table], |row| row.get(0)).unwrap();
        assert!(exists, "{table}");
    }
    // Running the step again changes nothing.
    store.db.execute_batch(SCHEMA).unwrap();
    let names: Vec<String> = tools::catalog().iter().map(|tool| tool["name"].as_str().unwrap().to_owned()).collect();
    assert_eq!(names, ["pandacode_list_agents", "pandacode_spawn_agent", "pandacode_send_message", "pandacode_wait_agents", "pandacode_read_transcript", "pandacode_stop_agent"]);
    assert!(!tools::self_scoped(tools::SPAWN) && tools::self_scoped(tools::SEND) && tools::self_scoped(tools::STOP));
    let host = crate::providers::ToolHost { tools: tools::catalog(), approval_tools: [tools::SPAWN.to_owned()].into(), handler: Arc::new(|_, _| Box::pin(async { Ok(json!({})) })) };
    assert!(crate::providers::host_tools_accept(&host));
}
