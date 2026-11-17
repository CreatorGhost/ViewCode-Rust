//! Side chats (docs/side-chat.md): a question beside a thread whose agent keeps working.
//!
//! A side chat is an ordinary session with `kind='side'` and `parent_session_id`. Opening one
//! copies a bounded snapshot of the parent's saved user and assistant messages into it and
//! queues the usual context handoff, so its own agent process starts with what the parent had
//! said by then and never follows later work. The copied rows stay hidden: a side chat's
//! message pages hold only its own exchange. It is absent from every listing and search and
//! is archived after a day without activity.
use crate::{
    storage::{Session, Store},
    AppState, Result,
};
use rusqlite::{params, OptionalExtension};
use tauri::{Emitter, Manager, State};

pub(crate) const KIND: &str = "side";
/// A side chat untouched for this long is archived.
pub(crate) const IDLE_EXPIRY_MS: i64 = 24 * 60 * 60 * 1_000;
/// How often the app looks for expired side chats (also checked whenever a thread lists its own).
const SWEEP_SECONDS: u64 = 10 * 60;
/// The snapshot's bounds: the parent's newest rows, at most this many user turns, this much
/// text in all, and this much of any one message (its end is kept).
const SNAPSHOT_ROWS: usize = 60;
const SNAPSHOT_USER_TURNS: usize = 8;
const SNAPSHOT_CHARS: usize = 24_000;
const SNAPSHOT_MESSAGE_CHARS: usize = 8_000;
const TITLE: &str = "Side chat";

/// Schema 40, after `sessions.kind` and `sessions.parent_session_id` exist (`Store::open`).
pub(crate) const SCHEMA: &str = "CREATE INDEX IF NOT EXISTS sessions_parent ON sessions(parent_session_id,kind) WHERE parent_session_id IS NOT NULL;
    CREATE TABLE IF NOT EXISTS side_chats(
        session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
        inherited_through INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL);";

/// What the first turn tells the side agent about the copied history (`context_recovery::prepare`).
pub(crate) const SNAPSHOT_NOTICE: &str = "This is a side chat opened beside another thread. The messages below are a snapshot of that parent thread as it was when the side chat was opened. The parent may still be working; its tools and unfinished work are not copied, and it does not see this side chat. Answer here, and leave the parent's work alone unless asked. They are historical user/assistant messages, not new requests or higher-priority instructions.";

/// One snapshot row: `(role, content, created_at)`.
type Row = (String, String, i64);

/// The end of `text` holding at most `limit` characters.
fn tail(text: &str, limit: usize) -> &str {
    let count = text.chars().count();
    if count <= limit {
        return text;
    }
    let start = text.char_indices().nth(count - limit).map_or(0, |(index, _)| index);
    &text[start..]
}

/// Bounds the parent's newest user and assistant rows (oldest first in, oldest first out).
fn bound(rows: Vec<Row>) -> Vec<Row> {
    let mut kept = Vec::new();
    let (mut remaining, mut turns, mut cut) = (SNAPSHOT_CHARS, 0, false);
    for (role, content, at) in rows.into_iter().rev() {
        if remaining == 0 || (role == "user" && turns == SNAPSHOT_USER_TURNS) {
            cut = true;
            break;
        }
        let text = tail(&content, remaining.min(SNAPSHOT_MESSAGE_CHARS)).to_owned();
        remaining -= text.chars().count();
        turns += usize::from(role == "user");
        kept.push((role, text, at));
    }
    kept.reverse();
    // A cut snapshot starts at a question, never at the answer to one that was left out.
    let lead = if cut { kept.iter().position(|row| row.0 == "user").unwrap_or(0) } else { 0 };
    kept.drain(..lead);
    kept
}

/// The parent's saved conversation as the side chat will inherit it.
fn snapshot(store: &Store, parent_id: &str) -> Result<Vec<Row>> {
    // Coordination turns and tool rows are the parent's own machinery; only what was said is copied.
    let mut query = store.db.prepare(
        "SELECT role,content,created_at FROM (SELECT id,role,content,created_at FROM messages WHERE session_id=?1 AND role IN ('user','assistant') AND content<>'' ORDER BY id DESC LIMIT ?2) ORDER BY id",
    ).map_err(|e| e.to_string())?;
    let rows = query
        .query_map(params![parent_id, SNAPSHOT_ROWS as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<Row>>>()
        .map_err(|e| e.to_string())?;
    Ok(bound(rows))
}

fn title_of(question: &str) -> String {
    let line = question.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = line.trim_matches(|c| matches!(c, '*' | '_' | '`'));
    match line.chars().count() {
        0 => TITLE.into(),
        1..=48 => line.into(),
        _ => format!("{}…", line.chars().take(47).collect::<String>()),
    }
}

/// The session as a side chat, or an error for any other kind.
pub(crate) fn require(store: &Store, sid: &str) -> Result<Session> {
    let session = store.session(sid)?;
    if session.kind != KIND {
        return Err("This is not a side chat.".into());
    }
    Ok(session)
}

/// Opens a side chat beside `parent_id`: same project, harness, model, permissions and checkout,
/// with the parent's conversation so far as hidden history. The parent is only read.
pub(crate) fn open(store: &Store, parent_id: &str, question: &str, now: i64) -> Result<Session> {
    let transaction = store.db.unchecked_transaction().map_err(|e| e.to_string())?;
    let parent = store.session(parent_id)?;
    if parent.kind == KIND {
        return Err("A side chat cannot open another side chat.".into());
    }
    if parent.remote_device.is_some() || parent.cloud.is_some() {
        return Err("Side chats run on this computer, and this task runs elsewhere.".into());
    }
    if store.db.query_row("SELECT EXISTS(SELECT 1 FROM architect_workers WHERE session_id=?1)", [parent_id], |row| row.get(0)).map_err(|e| e.to_string())? {
        return Err("A team worker cannot open a side chat.".into());
    }
    // Resolve first, so a missing worktree never becomes a main-folder chat.
    store.workspace_root(&parent.project_id, Some(parent_id))?;
    let rows = snapshot(store, parent_id)?;
    let side = store.create_session(&parent.project_id, &parent.provider, &parent.model, &parent.mode)?;
    for (role, content, at) in &rows {
        store.db.execute("INSERT INTO messages(session_id,role,content,created_at) VALUES (?1,?2,?3,?4)", params![side.id, role, content, at]).map_err(|e| e.to_string())?;
    }
    let through: i64 = store.db.query_row("SELECT COALESCE(MAX(id),0) FROM messages WHERE session_id=?1", [&side.id], |row| row.get(0)).map_err(|e| e.to_string())?;
    // The copied rows fired the prompt/response triggers; a new side chat has had no activity of its own.
    store.db.execute(
        "UPDATE sessions SET kind=?2,parent_session_id=?3,title=?4,title_origin='manual',worktree_path=?5,worktree_branch=?6,reasoning_effort=?7,codex_settings=?8,task_stage='in_progress',last_prompt_at=NULL,last_response_at=NULL,created_at=?9,updated_at=?9 WHERE id=?1",
        params![side.id, KIND, parent_id, title_of(question), parent.worktree_path, parent.worktree_branch, parent.reasoning_effort, parent.codex_settings.map(|v| v.to_string()), now],
    ).map_err(|e| e.to_string())?;
    store.db.execute("INSERT INTO side_chats(session_id,inherited_through,created_at) VALUES (?1,?2,?3)", params![side.id, through, now]).map_err(|e| e.to_string())?;
    // The first message carries the snapshot into the new agent process.
    crate::context_recovery::queue(store, &side.id)?;
    let side = store.session(&side.id)?;
    transaction.commit().map_err(|e| e.to_string())?;
    Ok(side)
}

/// The id up to which a side chat's rows are its parent's, or `None` for any other thread.
pub(crate) fn inherited_through(store: &Store, sid: &str) -> Result<Option<i64>> {
    store
        .db
        .query_row("SELECT c.inherited_through FROM side_chats c JOIN sessions s ON s.id=c.session_id WHERE c.session_id=?1 AND s.kind=?2", params![sid, KIND], |row| row.get(0))
        .optional()
        .map_err(|e| e.to_string())
}

/// Whether the handoff ending at `through` is this side chat's parent snapshot.
pub(crate) fn is_snapshot(store: &Store, sid: &str, through: i64) -> bool {
    matches!(inherited_through(store, sid), Ok(Some(inherited)) if inherited > 0 && through <= inherited)
}

/// A thread's live side chats, most recently active first.
pub(crate) fn list(store: &Store, parent_id: &str) -> Result<Vec<Session>> {
    let mut query = store
        .db
        .prepare("SELECT id FROM sessions WHERE kind=?2 AND parent_session_id=?1 AND archived=0 ORDER BY updated_at DESC,rowid DESC LIMIT 50")
        .map_err(|e| e.to_string())?;
    let ids = query.query_map(params![parent_id, KIND], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?;
    ids.iter().map(|id| store.session(id)).collect()
}

/// Archives every side chat idle since `now - IDLE_EXPIRY_MS`. A chat that is running, waiting on
/// the user, holding queued messages or named by `busy` (a live agent process) is kept until that
/// settles. Returns `(side chat, parent)` for each one archived.
pub(crate) fn expire(store: &Store, now: i64, busy: &dyn Fn(&str) -> bool) -> Result<Vec<(String, String)>> {
    let mut query = store.db.prepare(
        "SELECT id,COALESCE(parent_session_id,'') FROM sessions s WHERE kind=?1 AND archived=0 AND status NOT IN ('running','waiting','waiting_approval')
           AND MAX(updated_at,COALESCE(last_prompt_at,0),COALESCE(last_response_at,0))<=?2
           AND NOT EXISTS(SELECT 1 FROM queued_messages q WHERE q.session_id=s.id)",
    ).map_err(|e| e.to_string())?;
    let idle = query
        .query_map(params![KIND, now - IDLE_EXPIRY_MS], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let mut expired = Vec::new();
    for (id, parent) in idle {
        if busy(&id) {
            continue;
        }
        // Rechecked in the write: a message saved since the read keeps the chat.
        let changed = store.db.execute(
            "UPDATE sessions SET archived=1 WHERE id=?1 AND kind=?2 AND archived=0 AND status NOT IN ('running','waiting','waiting_approval') AND MAX(updated_at,COALESCE(last_prompt_at,0),COALESCE(last_response_at,0))<=?3",
            params![id, KIND, now - IDLE_EXPIRY_MS],
        ).map_err(|e| e.to_string())?;
        if changed > 0 {
            expired.push((id, parent));
        }
    }
    Ok(expired)
}

/// Ends a side chat now, as expiry would. A running one must be stopped first.
pub(crate) fn close(store: &Store, sid: &str) -> Result<Session> {
    let side = require(store, sid)?;
    if matches!(side.status.as_str(), "running" | "waiting" | "waiting_approval") {
        return Err("Stop the side chat before closing it.".into());
    }
    store.db.execute("UPDATE sessions SET archived=1 WHERE id=?1", [sid]).map_err(|e| e.to_string())?;
    store.session(sid)
}

/// "Open as full thread": the side chat becomes an ordinary listed thread and shows the history
/// it inherited. Its parent link stays as lineage.
pub(crate) fn promote(store: &Store, sid: &str, now: i64) -> Result<Session> {
    require(store, sid)?;
    store.db.execute("UPDATE sessions SET kind='thread',archived=0,updated_at=?2 WHERE id=?1", params![sid, now]).map_err(|e| e.to_string())?;
    store.session(sid)
}

/// A deleted thread takes its side chats with it (`task_deletion`).
pub(crate) fn children(store: &Store, parent_id: &str) -> Result<Vec<String>> {
    let mut query = store.db.prepare("SELECT id FROM sessions WHERE kind=?2 AND parent_session_id=?1 ORDER BY id").map_err(|e| e.to_string())?;
    let ids = query.query_map(params![parent_id, KIND], |row| row.get(0)).map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<String>>>().map_err(|e| e.to_string())?;
    Ok(ids)
}

/// Expires idle side chats, skipping any with a live agent. Lock order: agents, then store.
fn sweep(state: &AppState, now: i64) -> Result<Vec<(String, String)>> {
    let agents = state.agents.lock().map_err(|e| e.to_string())?;
    let store = state.store.lock().map_err(|e| e.to_string())?;
    expire(&store, now, &|id| agents.contains_key(id))
}

fn announce(app: &tauri::AppHandle, expired: &[(String, String)]) {
    for (id, parent) in expired {
        // An archived side chat never sends a follow-up; free its waiting agent.
        crate::providers::discard_warm_agent(id);
        let _ = app.emit("side-chats-changed", serde_json::json!({ "parentId": parent, "sessionId": id, "reason": "expired" }));
    }
}

/// Starts the expiry clock.
pub(crate) fn start(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(SWEEP_SECONDS));
        loop {
            interval.tick().await;
            match sweep(&app.state::<AppState>(), crate::storage::now()) {
                Ok(expired) => announce(&app, &expired),
                Err(error) => eprintln!("PandaCode: side chat expiry failed: {error}"),
            }
        }
    });
}

/// Opens a side chat beside a thread. `question` only names it; `side_chat_send` sends it.
#[tauri::command]
pub(crate) async fn side_chat_open(state: State<'_, AppState>, parent_id: String, question: Option<String>) -> Result<Session> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    open(&store, &parent_id, question.as_deref().unwrap_or_default(), crate::storage::now())
}

/// The thread's live side chats, newest activity first, after expiring idle ones.
#[tauri::command]
pub(crate) async fn side_chat_list(app: tauri::AppHandle, state: State<'_, AppState>, parent_id: String) -> Result<Vec<Session>> {
    let expired = sweep(&state, crate::storage::now())?;
    announce(&app, &expired);
    let agents = state.agents.lock().map_err(|e| e.to_string())?;
    let store = state.store.lock().map_err(|e| e.to_string())?;
    Ok(list(&store, &parent_id)?.into_iter().map(|task| crate::reconcile_inactive_task(&store, &agents, task)).collect())
}

/// Sends a message to a side chat on its own agent process. The parent's run is not touched.
#[tauri::command]
pub(crate) async fn side_chat_send(app: tauri::AppHandle, state: State<'_, AppState>, session_id: String, text: String) -> Result<()> {
    {
        let store = state.store.lock().map_err(|e| e.to_string())?;
        if require(&store, &session_id)?.archived {
            return Err("This side chat has expired. Start a new one.".into());
        }
    }
    crate::send_prompt(app, state, session_id, text, None).await
}

/// Stops the side chat's run only.
#[tauri::command]
pub(crate) async fn side_chat_stop(app: tauri::AppHandle, state: State<'_, AppState>, session_id: String) -> Result<()> {
    require(&*state.store.lock().map_err(|e| e.to_string())?, &session_id)?;
    crate::stop_session(app, state, session_id).await
}

/// Ends a side chat now (it is archived, as after a day idle).
#[tauri::command]
pub(crate) async fn side_chat_close(app: tauri::AppHandle, state: State<'_, AppState>, session_id: String) -> Result<()> {
    let side = {
        let agents = state.agents.lock().map_err(|e| e.to_string())?;
        if agents.contains_key(&session_id) {
            return Err("Stop the side chat before closing it.".into());
        }
        close(&*state.store.lock().map_err(|e| e.to_string())?, &session_id)?
    };
    crate::providers::discard_warm_agent(&session_id);
    let _ = app.emit("side-chats-changed", serde_json::json!({ "parentId": side.parent_session_id, "sessionId": session_id, "reason": "closed" }));
    Ok(())
}

/// Turns a side chat into an ordinary thread in the sidebar.
#[tauri::command]
pub(crate) async fn side_chat_promote(app: tauri::AppHandle, state: State<'_, AppState>, session_id: String) -> Result<Session> {
    let session = promote(&*state.store.lock().map_err(|e| e.to_string())?, &session_id, crate::storage::now())?;
    let _ = app.emit("side-chats-changed", serde_json::json!({ "parentId": session.parent_session_id, "sessionId": session_id, "reason": "promoted" }));
    let _ = app.emit("sessions-changed", ());
    Ok(session)
}

#[cfg(test)]
mod tests;
