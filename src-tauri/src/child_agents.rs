//! Child agents and agent mail (docs/child-agents.md), after ViewCode's agents toolkit.
//!
//! A child agent is an ordinary session with `kind='child'` and the general
//! `parent_session_id` (schema 40), running in its own git worktree made from its parent's
//! checkout. Agents in one tree (a root thread and all its descendants) talk through mail: a
//! message is held while its receiver is busy or paused and is delivered as the receiver's
//! next turn, in ViewCode's `<…-agent-message>` envelope. The run that took a message that
//! expects a reply sends its final answer back to the sender, from that run only. A chain of
//! automatic messages stops after [`MAX_HOPS`]. The user can pause (Stop), resume or discard
//! the held mail of any one agent without touching its siblings or its parent.
//!
//! Everything here is a function of the store and a clock; `app` below wires it to runs.
use crate::{
    storage::{Session, Store},
    AppState, Result,
};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{Emitter, Manager, State};

pub(crate) mod tools;
#[cfg(test)]
mod tests;

pub(crate) const KIND: &str = "child";
/// ViewCode's `AGENT_MESSAGE_MAX_HOPS`: a chain of automatic messages (spawns, messages and
/// the replies routed back) ends at this many hops; the 24th is refused.
pub(crate) const MAX_HOPS: i64 = 24;
/// Live direct children one agent may have. ViewCode sets none; see docs/child-agents.md.
pub(crate) const MAX_CHILDREN: usize = 8;
/// How deep a tree may grow below its root thread (a root's children are depth 1).
pub(crate) const MAX_DEPTH: i64 = 3;
/// The most of a final answer routed back as a reply, and of a transcript read (its end is kept).
pub(crate) const REPLY_CHARS: usize = 24_000;
/// What a resumed agent is told when the user stopped it mid-turn (ViewCode's wording).
pub(crate) const CONTINUE_PROMPT: &str = "Continue where you left off.";
pub(crate) const TAG: &str = "pandacode-agent-message";
/// Emitted with `{ parentId, sessionId, reason }` whenever a tree's agents or mail change.
pub(crate) const CHANGED: &str = "child-agents-changed";

/// Schema 42. The parent link is `sessions.parent_session_id`; nothing here repeats it.
pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS child_agents(
        session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
        root_session_id TEXT NOT NULL,
        name TEXT NOT NULL,
        task TEXT NOT NULL DEFAULT '',
        depth INTEGER NOT NULL,
        created_at INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS child_agents_root ON child_agents(root_session_id);
    CREATE TABLE IF NOT EXISTS agent_control(
        session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
        paused INTEGER NOT NULL DEFAULT 0,
        interrupted INTEGER NOT NULL DEFAULT 0,
        interrupted_mail TEXT);
    CREATE TABLE IF NOT EXISTS agent_mail(
        id TEXT PRIMARY KEY,
        from_session TEXT NOT NULL,
        from_name TEXT NOT NULL,
        to_session TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        body TEXT NOT NULL,
        reply_expected INTEGER NOT NULL DEFAULT 0,
        in_reply_to TEXT,
        chain_id TEXT NOT NULL,
        hop INTEGER NOT NULL,
        state TEXT NOT NULL DEFAULT 'held',
        run_id TEXT,
        replied INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        delivered_at INTEGER);
    CREATE INDEX IF NOT EXISTS agent_mail_to ON agent_mail(to_session,state,created_at);
    CREATE INDEX IF NOT EXISTS agent_mail_run ON agent_mail(run_id) WHERE run_id IS NOT NULL;";

fn sql(error: rusqlite::Error) -> String {
    error.to_string()
}

/// One message between agents. `state` is `held` (waiting for its receiver), `delivered`
/// (a run took it: `run_id`), `done`, `collected` (a reply read by `pandacode_wait_agents`
/// instead of starting a turn), `failed` or `discarded`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Mail {
    pub id: String,
    pub from_session: String,
    pub from_name: String,
    pub to_session: String,
    /// `spawn`, `message`, `reply` or `continue`.
    pub kind: String,
    pub body: String,
    pub reply_expected: bool,
    pub in_reply_to: Option<String>,
    pub chain_id: String,
    pub hop: i64,
    pub state: String,
    pub run_id: Option<String>,
    pub replied: bool,
    pub created_at: i64,
}

const MAIL_COLUMNS: &str = "id,from_session,from_name,to_session,kind,body,reply_expected,in_reply_to,chain_id,hop,state,run_id,replied,created_at";

fn mail_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Mail> {
    Ok(Mail {
        id: row.get(0)?,
        from_session: row.get(1)?,
        from_name: row.get(2)?,
        to_session: row.get(3)?,
        kind: row.get(4)?,
        body: row.get(5)?,
        reply_expected: row.get(6)?,
        in_reply_to: row.get(7)?,
        chain_id: row.get(8)?,
        hop: row.get(9)?,
        state: row.get(10)?,
        run_id: row.get(11)?,
        replied: row.get(12)?,
        created_at: row.get(13)?,
    })
}

fn mails(store: &Store, filter: &str, values: impl rusqlite::Params) -> Result<Vec<Mail>> {
    let mut query = store.db.prepare(&format!("SELECT {MAIL_COLUMNS} FROM agent_mail {filter}")).map_err(sql)?;
    let rows = query.query_map(values, mail_row).map_err(sql)?.collect::<rusqlite::Result<Vec<_>>>().map_err(sql)?;
    Ok(rows)
}

pub(crate) fn mail(store: &Store, id: &str) -> Result<Option<Mail>> {
    Ok(mails(store, "WHERE id=?1", [id])?.pop())
}

/// What a tree knows about one child agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Child {
    pub session_id: String,
    pub parent_session_id: String,
    pub root_session_id: String,
    pub name: String,
    pub task: String,
    pub depth: i64,
}

pub(crate) fn child(store: &Store, sid: &str) -> Result<Option<Child>> {
    store
        .db
        .query_row(
            "SELECT c.session_id,COALESCE(s.parent_session_id,''),c.root_session_id,c.name,c.task,c.depth FROM child_agents c JOIN sessions s ON s.id=c.session_id WHERE c.session_id=?1",
            [sid],
            |row| Ok(Child { session_id: row.get(0)?, parent_session_id: row.get(1)?, root_session_id: row.get(2)?, name: row.get(3)?, task: row.get(4)?, depth: row.get(5)? }),
        )
        .optional()
        .map_err(sql)
}

/// The root thread of `sid`'s tree: itself unless it is a child agent.
pub(crate) fn root_of(store: &Store, sid: &str) -> Result<String> {
    Ok(child(store, sid)?.map_or_else(|| sid.to_owned(), |child| child.root_session_id))
}

/// Direct children of `parent`, oldest first.
pub(crate) fn children(store: &Store, parent: &str) -> Result<Vec<String>> {
    let mut query = store.db.prepare("SELECT c.session_id FROM child_agents c JOIN sessions s ON s.id=c.session_id WHERE s.parent_session_id=?1 ORDER BY c.created_at,c.rowid").map_err(sql)?;
    let ids = query.query_map([parent], |row| row.get(0)).map_err(sql)?.collect::<rusqlite::Result<Vec<String>>>().map_err(sql)?;
    Ok(ids)
}

/// Every descendant of `sid`, depth first (a child's own children follow it). Cycles, never
/// expected, are cut at the first revisit.
pub(crate) fn descendants(store: &Store, sid: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::from([sid.to_owned()]);
    fn visit(store: &Store, parent: &str, out: &mut Vec<String>, seen: &mut std::collections::HashSet<String>) -> Result<()> {
        for id in children(store, parent)? {
            if seen.insert(id.clone()) {
                out.push(id.clone());
                visit(store, &id, out, seen)?;
            }
        }
        Ok(())
    }
    visit(store, sid, &mut out, &mut seen)?;
    Ok(out)
}

/// The tree `sid` belongs to: its root first, then every descendant.
pub(crate) fn tree(store: &Store, sid: &str) -> Result<Vec<String>> {
    let root = root_of(store, sid)?;
    let mut ids = vec![root.clone()];
    ids.extend(descendants(store, &root)?);
    Ok(ids)
}

/// Whether `sid` is in a tree of more than one agent.
pub(crate) fn in_tree(store: &Store, sid: &str) -> Result<bool> {
    Ok(child(store, sid)?.is_some() || !children(store, sid)?.is_empty())
}

/// The name agents use for `sid`: a child's own name, else the thread's title.
fn display_name(store: &Store, session: &Session) -> Result<String> {
    Ok(child(store, &session.id)?.map_or_else(|| session.title.clone(), |child| child.name))
}

/// The user's controls over one agent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Control {
    pub paused: bool,
    pub interrupted: bool,
    pub interrupted_mail: Option<String>,
}

pub(crate) fn control(store: &Store, sid: &str) -> Result<Control> {
    Ok(store
        .db
        .query_row("SELECT paused,interrupted,interrupted_mail FROM agent_control WHERE session_id=?1", [sid], |row| Ok(Control { paused: row.get(0)?, interrupted: row.get(1)?, interrupted_mail: row.get(2)? }))
        .optional()
        .map_err(sql)?
        .unwrap_or_default())
}

fn set_control(store: &Store, sid: &str, control: &Control) -> Result<()> {
    store
        .db
        .execute(
            "INSERT INTO agent_control(session_id,paused,interrupted,interrupted_mail) VALUES(?1,?2,?3,?4)
             ON CONFLICT(session_id) DO UPDATE SET paused=excluded.paused,interrupted=excluded.interrupted,interrupted_mail=excluded.interrupted_mail",
            params![sid, control.paused, control.interrupted, control.interrupted_mail],
        )
        .map_err(sql)?;
    Ok(())
}

/// Messages waiting for `sid`, oldest first.
pub(crate) fn held(store: &Store, sid: &str) -> Result<Vec<Mail>> {
    mails(store, "WHERE to_session=?1 AND state='held' ORDER BY created_at,rowid", [sid])
}

fn held_count(store: &Store, sid: &str) -> Result<i64> {
    store.db.query_row("SELECT COUNT(*) FROM agent_mail WHERE to_session=?1 AND state='held'", [sid], |row| row.get(0)).map_err(sql)
}

// ---------------------------------------------------------------------------------------
// Permissions

/// Permission modes from narrowest to broadest.
fn rank(mode: &str) -> u8 {
    match mode {
        "plan" => 0,
        "ask" => 1,
        "review" => 2,
        "auto" => 3,
        "full" => 4,
        _ => 1,
    }
}

/// The narrower of two modes.
pub(crate) fn narrower<'a>(a: &'a str, b: &'a str) -> &'a str {
    if rank(b) < rank(a) {
        b
    } else {
        a
    }
}

/// The mode a turn of `session` runs in: its own, never broader than any ancestor's. A child
/// inherits its parent's mode when spawned; a mode raised on the child later, or lowered on
/// the parent, is capped here at every turn start.
pub(crate) fn effective_mode(store: &Store, session: &Session) -> String {
    let mut mode = session.mode.clone();
    let mut current = session.id.clone();
    let mut seen = std::collections::HashSet::new();
    while seen.insert(current.clone()) {
        let Ok(Some(child)) = child(store, &current) else { break };
        let Ok(parent) = store.session(&child.parent_session_id) else { break };
        mode = narrower(&mode, &parent.mode).to_owned();
        current = parent.id;
    }
    mode
}

// ---------------------------------------------------------------------------------------
// The envelope

fn attribute(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;").replace('\n', " ")
}

/// A message as its receiver's turn input: ViewCode's envelope, renamed for this app, and a
/// footer saying whether and how to reply. A continue the receiver owes itself is plain text.
pub(crate) fn envelope(mail: &Mail) -> String {
    if mail.kind == "continue" && mail.from_session == mail.to_session {
        return CONTINUE_PROMPT.into();
    }
    let mut attributes = vec![
        format!("from=\"{}\"", attribute(&mail.from_name)),
        format!("from-id=\"{}\"", attribute(&mail.from_session)),
        format!("message-id=\"{}\"", attribute(&mail.id)),
        format!("reply-expected=\"{}\"", mail.reply_expected),
    ];
    if let Some(original) = &mail.in_reply_to {
        attributes.push(format!("in-reply-to=\"{}\"", attribute(original)));
    }
    let footer = if let Some(original) = &mail.in_reply_to {
        format!("This is {}'s reply to your message {original}.", mail.from_name)
    } else if mail.reply_expected {
        format!("A reply is expected. Answer with the pandacode_send_message tool (to=\"{}\", response_id=\"{}\"). If you finish without calling it, your final answer is sent back automatically.", mail.from_session, mail.id)
    } else {
        "No reply is required.".into()
    };
    format!("<{TAG} {}>\n{}\n</{TAG}>\n\n{footer}", attributes.join(" "), mail.body.trim())
}

/// The end of `text` holding at most `limit` characters.
pub(crate) fn tail(text: &str, limit: usize) -> &str {
    let count = text.chars().count();
    if count <= limit {
        return text;
    }
    let start = text.char_indices().nth(count - limit).map_or(0, |(index, _)| index);
    &text[start..]
}

// ---------------------------------------------------------------------------------------
// Sending

/// `started`: the receiver is idle and its turn starts now. `queued`: it is busy or paused,
/// and the message waits for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Sent {
    pub message_id: String,
    pub delivery: &'static str,
}

/// The tree member `target` names for `caller`: an id, or an exact name (a child's name or a
/// thread's title). Only the caller's own tree is reachable.
pub(crate) fn resolve(store: &Store, caller: &str, target: &str) -> Result<Session> {
    let target = target.trim();
    let mut matches = Vec::new();
    for id in tree(store, caller)? {
        let Ok(session) = store.session(&id) else { continue };
        if session.archived {
            continue;
        }
        if id == target || display_name(store, &session)? == target || session.title == target {
            matches.push(session);
        }
    }
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err("The agent is not in your agent tree.".into()),
        _ => Err("Several agents have that name. Use its id.".into()),
    }
}

/// The message that started `caller`'s current run, when one did.
fn incoming(store: &Store, caller: &str, caller_run: Option<&str>) -> Result<Option<Mail>> {
    let Some(run) = caller_run else { return Ok(None) };
    Ok(mails(store, "WHERE to_session=?1 AND run_id=?2 ORDER BY created_at DESC LIMIT 1", params![caller, run])?.pop())
}

pub(crate) struct Message<'a> {
    pub to: &'a str,
    pub body: &'a str,
    pub reply_expected: bool,
    pub response_id: Option<&'a str>,
}

/// The next hop of a chain started by `incoming`, refused at the limit.
fn next_hop(incoming: Option<&Mail>) -> Result<(i64, Option<String>)> {
    let hop = incoming.map_or(0, |mail| mail.hop + 1);
    if hop >= MAX_HOPS {
        return Err(format!("This conversation reached {MAX_HOPS} automatic hops. Summarize the state for the user."));
    }
    Ok((hop, incoming.map(|mail| mail.chain_id.clone())))
}

#[allow(clippy::too_many_arguments)]
fn insert_mail(store: &Store, from: &Session, to: &str, kind: &str, body: &str, reply_expected: bool, in_reply_to: Option<&str>, chain: Option<String>, hop: i64, now: i64) -> Result<String> {
    let id = crate::storage::id();
    let from_name = display_name(store, from)?;
    store
        .db
        .execute(
            "INSERT INTO agent_mail(id,from_session,from_name,to_session,kind,body,reply_expected,in_reply_to,chain_id,hop,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![id, from.id, from_name, to, kind, body, reply_expected, in_reply_to, chain.unwrap_or_else(|| id.clone()), hop, now],
        )
        .map_err(sql)?;
    Ok(id)
}

/// `pandacode_send_message`: a message from `caller` to another agent in its tree.
/// `caller_run` is the caller's current run: a message it was started by gives the hop count.
pub(crate) fn send(store: &Store, caller: &str, caller_run: Option<&str>, message: Message<'_>, busy: &dyn Fn(&str) -> bool, now: i64) -> Result<Sent> {
    let from = store.session(caller)?;
    let target = resolve(store, caller, message.to)?;
    if target.id == caller {
        return Err("You cannot send a message to yourself.".into());
    }
    let body = message.body.trim();
    if body.is_empty() {
        return Err("Write a message first.".into());
    }
    if body.len() > crate::MESSAGE_LIMIT {
        return Err("The message must be under 16 MiB.".into());
    }
    let response = match message.response_id {
        Some(id) => {
            let found = mail(store, id)?.filter(|mail| mail.to_session == caller && mail.from_session == target.id);
            Some(found.ok_or("That response id does not belong to a message from this agent.")?)
        }
        None => None,
    };
    let incoming = match &response {
        Some(response) => Some(response.clone()),
        None => incoming(store, caller, caller_run)?,
    };
    let (hop, chain) = next_hop(incoming.as_ref())?;
    let kind = if response.is_some() { "reply" } else { "message" };
    let id = insert_mail(store, &from, &target.id, kind, body, message.reply_expected, message.response_id, chain, hop, now)?;
    if let Some(response) = response {
        store.db.execute("UPDATE agent_mail SET replied=1 WHERE id=?1", [&response.id]).map_err(sql)?;
    }
    let delivery = if busy(&target.id) || control(store, &target.id)?.paused { "queued" } else { "started" };
    Ok(Sent { message_id: id, delivery })
}

// ---------------------------------------------------------------------------------------
// Spawning

pub(crate) struct SpawnInput<'a> {
    pub name: &'a str,
    pub task: Option<&'a str>,
    pub prompt: &'a str,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub reply_expected: Option<bool>,
}

/// A checked spawn, before its worktree exists.
#[derive(Clone, Debug)]
pub(crate) struct SpawnPlan {
    pub parent: Session,
    pub root: String,
    pub depth: i64,
    pub name: String,
    pub task: String,
    pub prompt: String,
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
    pub mode: String,
    pub reply_expected: bool,
    pub hop: i64,
    pub chain: Option<String>,
    /// The parent's checkout, which the child's worktree starts from (its local commit).
    pub checkout: PathBuf,
    pub branch: String,
}

fn single_line(value: &str, key: &str, max: usize) -> Result<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() || value.chars().count() > max {
        return Err(format!("Invalid argument: {key}"));
    }
    Ok(value)
}

/// The branch a child's worktree gets: `pandacode/agent-<name>-<id>`.
fn branch_for(name: &str, id: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "child".to_owned() } else { slug.chars().take(40).collect() };
    format!("pandacode/agent-{slug}-{}", &id[..8.min(id.len())])
}

/// Checks a spawn from `caller` without changing anything. `gate` is the launch gate's
/// decision for a harness id (`launch_gate::allow`): an agent that is off or blocked is
/// refused here, with the gate's own sentence, and nothing is created or started.
pub(crate) fn plan_spawn(store: &Store, caller: &str, caller_run: Option<&str>, input: SpawnInput<'_>, gate: &dyn Fn(&str) -> Result<()>) -> Result<SpawnPlan> {
    let parent = store.session(caller)?;
    if parent.kind == crate::side_chat::KIND {
        return Err("A side chat cannot start child agents.".into());
    }
    if parent.remote_device.is_some() || parent.cloud.is_some() {
        return Err("Child agents run on this computer, and this task runs elsewhere.".into());
    }
    let team: bool = store.db.query_row("SELECT EXISTS(SELECT 1 FROM architect_workers WHERE session_id=?1 OR parent_session_id=?1)", [caller], |row| row.get(0)).map_err(sql)?;
    if team || crate::architect::load(store, caller)?.is_some() {
        return Err("A team thread delegates through its team tools, not child agents.".into());
    }
    if crate::bots::owner_of(store, caller)?.is_some() {
        return Err("A bot conversation cannot start child agents.".into());
    }
    let own = child(store, caller)?;
    let depth = own.as_ref().map_or(0, |child| child.depth) + 1;
    if depth > MAX_DEPTH {
        return Err(format!("Child agents can be nested {MAX_DEPTH} deep; this agent cannot start another level. Do the work yourself or ask your parent."));
    }
    let live = children(store, caller)?.into_iter().filter(|id| store.session(id).is_ok_and(|session| !session.archived)).count();
    if live >= MAX_CHILDREN {
        return Err(format!("You already have {MAX_CHILDREN} child agents. Wait for them, or stop one, before starting another."));
    }
    let name = single_line(input.name, "name", 80)?;
    let task = match input.task.map(str::trim).filter(|task| !task.is_empty()) {
        Some(task) => single_line(task, "task", 120)?,
        None => String::new(),
    };
    let prompt = input.prompt.trim();
    if prompt.is_empty() {
        return Err("Invalid argument: prompt".into());
    }
    if prompt.len() > crate::MESSAGE_LIMIT {
        return Err("The prompt must be under 16 MiB.".into());
    }
    let provider = input.provider.map(str::trim).filter(|p| !p.is_empty()).unwrap_or(&parent.provider).to_owned();
    if !crate::providers::PROVIDERS.iter().any(|(id, _)| *id == provider) {
        return Err(format!("Unknown agent: {provider}."));
    }
    // The launch gate decides before anything exists: an off or blocked agent never starts.
    gate(&provider)?;
    let model = match input.model.map(str::trim).filter(|m| !m.is_empty()) {
        Some(model) if model.len() > 128 || model.chars().any(|c| c.is_whitespace() || c.is_control()) => return Err("Invalid argument: model".into()),
        Some(model) => model.to_owned(),
        None if provider == parent.provider => parent.model.clone(),
        None => "default".into(),
    };
    let effort = match input.effort.map(str::trim).filter(|e| !e.is_empty()) {
        Some(effort) => {
            crate::providers::validate_reasoning_effort(&provider, Some(effort))?;
            Some(effort.to_owned())
        }
        None if provider == parent.provider => parent.reasoning_effort.clone(),
        None => None,
    };
    let incoming = incoming(store, caller, caller_run)?;
    let (hop, chain) = next_hop(incoming.as_ref())?;
    let checkout = store.workspace_root(&parent.project_id, Some(caller))?;
    let root = own.map_or_else(|| caller.to_owned(), |child| child.root_session_id);
    Ok(SpawnPlan {
        mode: effective_mode(store, &parent),
        branch: branch_for(&name, &crate::storage::id()),
        parent,
        root,
        depth,
        name,
        task,
        prompt: prompt.to_owned(),
        provider,
        model,
        effort,
        reply_expected: input.reply_expected.unwrap_or(true),
        hop,
        chain,
        checkout,
    })
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Spawned {
    pub agent_id: String,
    pub name: String,
    pub message_id: String,
}

/// Saves the child in the worktree made for it, with its first message held for it. The
/// count is checked again, since the worktree was made outside the store lock.
pub(crate) fn commit_spawn(store: &Store, plan: &SpawnPlan, worktree: &std::path::Path, now: i64) -> Result<Spawned> {
    let transaction = store.db.unchecked_transaction().map_err(sql)?;
    let live = children(store, &plan.parent.id)?.into_iter().filter(|id| store.session(id).is_ok_and(|session| !session.archived)).count();
    if live >= MAX_CHILDREN {
        return Err(format!("You already have {MAX_CHILDREN} child agents. Wait for them, or stop one, before starting another."));
    }
    let mut source = plan.parent.clone();
    source.provider = plan.provider.clone();
    source.model = plan.model.clone();
    source.mode = plan.mode.clone();
    source.reasoning_effort = plan.effort.clone();
    let session = store.create_worktree_session_on(&source, worktree, Some(&plan.branch))?;
    store
        .db
        .execute(
            "UPDATE sessions SET kind=?2,parent_session_id=?3,title=?4,title_origin='manual',task_stage='in_progress',task_description=?5,created_at=?6,updated_at=?6 WHERE id=?1",
            params![session.id, KIND, plan.parent.id, plan.name, plan.task, now],
        )
        .map_err(sql)?;
    store
        .db
        .execute("INSERT INTO child_agents(session_id,root_session_id,name,task,depth,created_at) VALUES(?1,?2,?3,?4,?5,?6)", params![session.id, plan.root, plan.name, plan.task, plan.depth, now])
        .map_err(sql)?;
    let message_id = insert_mail(store, &plan.parent, &session.id, "spawn", &plan.prompt, plan.reply_expected, None, plan.chain.clone(), plan.hop, now)?;
    transaction.commit().map_err(sql)?;
    Ok(Spawned { agent_id: session.id, name: plan.name.clone(), message_id })
}

// ---------------------------------------------------------------------------------------
// Delivery and run ends

/// The message `sid` should take as its next turn: the oldest held one, unless the agent is
/// paused or gone. The caller has checked that no run is active.
pub(crate) fn ready(store: &Store, sid: &str) -> Result<Option<Mail>> {
    let Ok(session) = store.session(sid) else { return Ok(None) };
    if session.archived || control(store, sid)?.paused {
        return Ok(None);
    }
    Ok(held(store, sid)?.into_iter().next())
}

/// Whether `mail` can still start a turn of `sid` (checked before the turn is saved).
pub(crate) fn check_claimable(store: &Store, mail_id: &str, sid: &str) -> Result<()> {
    match mail(store, mail_id)? {
        Some(mail) if mail.to_session == sid && mail.state == "held" => Ok(()),
        _ => Err("This agent message was already delivered or discarded.".into()),
    }
}

/// Binds a held message to the run that takes it. Replies are routed by this run id only.
pub(crate) fn claim(store: &Store, mail_id: &str, sid: &str, run_id: &str, now: i64) -> Result<()> {
    let changed = store
        .db
        .execute("UPDATE agent_mail SET state='delivered',run_id=?3,delivered_at=?4 WHERE id=?1 AND to_session=?2 AND state='held'", params![mail_id, sid, run_id, now])
        .map_err(sql)?;
    if changed == 0 {
        return Err("This agent message was already delivered or discarded.".into());
    }
    Ok(())
}

/// How a run ended, with its final written answer when it gave one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Completed { answer: Option<String> },
    Failed { answer: Option<String>, error: String },
    Stopped,
}

/// What a run end changed: replies sent (and to whom) and the agents whose held mail may now
/// be delivered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Ended {
    pub replies: Vec<(String, String)>,
    pub wake: Vec<String>,
    /// A reply that could not be sent (the hop limit, or a sender that is gone).
    pub dropped: Option<String>,
}

/// A run of `sid` with id `run_id` ended. A message that run took and that expects a reply is
/// answered with the run's final answer (a failure says why), from this run only; a stopped
/// run sends nothing and, when its agent is paused, is remembered for Resume.
pub(crate) fn run_finished(store: &Store, sid: &str, run_id: &str, outcome: &Outcome, now: i64) -> Result<Ended> {
    let mut ended = Ended { wake: vec![sid.to_owned()], ..Ended::default() };
    let taken = mails(store, "WHERE to_session=?1 AND run_id=?2 AND state='delivered'", params![sid, run_id])?;
    if let Outcome::Stopped = outcome {
        let mut control = control(store, sid)?;
        if control.paused {
            control.interrupted = true;
            control.interrupted_mail = taken.last().map(|mail| mail.id.clone());
            set_control(store, sid, &control)?;
        }
        for mail in &taken {
            store.db.execute("UPDATE agent_mail SET state='done' WHERE id=?1", [&mail.id]).map_err(sql)?;
        }
        return Ok(ended);
    }
    for mail in &taken {
        store.db.execute("UPDATE agent_mail SET state='done' WHERE id=?1", [&mail.id]).map_err(sql)?;
        // A reply the agent already sent itself (response_id) is not sent twice.
        let replied: bool = store.db.query_row("SELECT replied FROM agent_mail WHERE id=?1", [&mail.id], |row| row.get(0)).map_err(sql)?;
        if !mail.reply_expected || replied {
            continue;
        }
        let body = match outcome {
            Outcome::Completed { answer } => answer.as_deref().map(str::trim).filter(|a| !a.is_empty()).map_or_else(|| "(finished without a written answer)".to_owned(), str::to_owned),
            Outcome::Failed { answer, error } => answer.as_deref().map(str::trim).filter(|a| !a.is_empty()).map_or_else(|| format!("(failed without an answer: {error})"), str::to_owned),
            Outcome::Stopped => unreachable!(),
        };
        let Ok(from) = store.session(sid) else { continue };
        if store.session(&mail.from_session).is_err() {
            ended.dropped = Some("The agent that asked no longer exists.".into());
            continue;
        }
        match next_hop(Some(mail)) {
            Ok((hop, chain)) => {
                let id = insert_mail(store, &from, &mail.from_session, "reply", tail(&body, REPLY_CHARS), false, Some(&mail.id), chain, hop, now)?;
                store.db.execute("UPDATE agent_mail SET replied=1 WHERE id=?1", [&mail.id]).map_err(sql)?;
                ended.replies.push((mail.from_session.clone(), id));
                ended.wake.push(mail.from_session.clone());
            }
            Err(error) => ended.dropped = Some(error),
        }
    }
    Ok(ended)
}

/// A turn could not start for `mail` (not a race with another run): the message is marked
/// failed and, when it expects a reply, its sender hears why.
pub(crate) fn bounce(store: &Store, mail_id: &str, error: &str, now: i64) -> Result<Option<String>> {
    let Some(mail) = mail(store, mail_id)?.filter(|mail| mail.state == "held") else { return Ok(None) };
    store.db.execute("UPDATE agent_mail SET state='failed' WHERE id=?1", [mail_id]).map_err(sql)?;
    if !mail.reply_expected || store.session(&mail.from_session).is_err() {
        return Ok(None);
    }
    let Ok(receiver) = store.session(&mail.to_session) else { return Ok(None) };
    let Ok((hop, chain)) = next_hop(Some(&mail)) else { return Ok(None) };
    insert_mail(store, &receiver, &mail.from_session, "reply", &format!("(could not start: {error})"), false, Some(&mail.id), chain, hop, now)?;
    Ok(Some(mail.from_session))
}

// ---------------------------------------------------------------------------------------
// The user's controls

/// Stop on an agent in a tree pauses it (ViewCode's rule): its mail waits until the user
/// resumes it or writes to it. Returns `sid` and every descendant, all paused: stopping an
/// agent stops the agents it started, never its siblings or its parent.
pub(crate) fn pause(store: &Store, sid: &str) -> Result<Vec<String>> {
    let mut scope = vec![sid.to_owned()];
    scope.extend(descendants(store, sid)?);
    for id in &scope {
        let mut control = control(store, id)?;
        control.paused = true;
        set_control(store, id, &control)?;
    }
    Ok(scope)
}

/// Unpauses `sid`: a turn the user stopped continues ("Continue where you left off.", still
/// answering whoever it was answering), then the held mail is delivered in order. Returns
/// whether anything is now waiting to start.
pub(crate) fn resume(store: &Store, sid: &str, now: i64) -> Result<bool> {
    let control = control(store, sid)?;
    if control.interrupted {
        let session = store.session(sid)?;
        match control.interrupted_mail.as_deref().map(|id| mail(store, id)).transpose()?.flatten() {
            Some(prior) => {
                let from = store.session(&prior.from_session).unwrap_or_else(|_| session.clone());
                let id = crate::storage::id();
                // It goes first: older than anything held.
                store
                    .db
                    .execute(
                        "INSERT INTO agent_mail(id,from_session,from_name,to_session,kind,body,reply_expected,in_reply_to,chain_id,hop,replied,created_at) VALUES(?1,?2,?3,?4,'continue',?5,?6,?7,?8,?9,?10,?11)",
                        params![id, from.id, prior.from_name, sid, CONTINUE_PROMPT, prior.reply_expected, prior.in_reply_to, prior.chain_id, prior.hop, prior.replied, held(store, sid)?.first().map_or(now, |first| first.created_at - 1)],
                    )
                    .map_err(sql)?;
            }
            None => {
                insert_mail(store, &session, sid, "continue", CONTINUE_PROMPT, false, None, None, 0, held(store, sid)?.first().map_or(now, |first| first.created_at - 1))?;
            }
        }
    }
    set_control(store, sid, &Control::default())?;
    Ok(held_count(store, sid)? > 0)
}

/// Drops `sid`'s held mail (one message, or all of it) and unpauses it without waking it, as
/// ViewCode's discard does; a stopped turn is not continued. Returns how many were dropped.
pub(crate) fn discard(store: &Store, sid: &str, mail_id: Option<&str>) -> Result<usize> {
    let dropped = match mail_id {
        Some(id) => store.db.execute("UPDATE agent_mail SET state='discarded' WHERE id=?1 AND to_session=?2 AND state='held'", params![id, sid]).map_err(sql)?,
        None => store.db.execute("UPDATE agent_mail SET state='discarded' WHERE to_session=?1 AND state='held'", [sid]).map_err(sql)?,
    };
    if mail_id.is_none() || held_count(store, sid)? == 0 {
        set_control(store, sid, &Control::default())?;
    }
    Ok(dropped)
}

/// The user wrote to `sid` themselves: a paused agent is resumed by it (its held mail follows
/// that turn), and the stopped turn is not continued separately.
pub(crate) fn user_prompted(store: &Store, sid: &str) -> Result<bool> {
    let was = control(store, sid)?;
    if was.paused || was.interrupted {
        set_control(store, sid, &Control::default())?;
        return Ok(true);
    }
    Ok(false)
}

/// "Open as thread": the child becomes an ordinary listed thread. It stays in its tree.
pub(crate) fn open_as_thread(store: &Store, sid: &str, now: i64) -> Result<Session> {
    if child(store, sid)?.is_none() {
        return Err("This is not a child agent.".into());
    }
    store.db.execute("UPDATE sessions SET kind='thread',archived=0,updated_at=?2 WHERE id=?1", params![sid, now]).map_err(sql)?;
    store.session(sid)
}

/// After an app restart: a message whose run never ended was cut off by the quit. Its agent is
/// paused with that turn remembered, so Resume continues it and still answers the sender.
/// Held mail stays held. Returns the agents with mail ready to deliver.
pub(crate) fn recover(store: &Store) -> Result<Vec<String>> {
    for mail in mails(store, "WHERE state='delivered'", [])? {
        store.db.execute("UPDATE agent_mail SET state='done' WHERE id=?1", [&mail.id]).map_err(sql)?;
        set_control(store, &mail.to_session, &Control { paused: true, interrupted: true, interrupted_mail: Some(mail.id) })?;
    }
    let mut query = store.db.prepare("SELECT DISTINCT m.to_session FROM agent_mail m LEFT JOIN agent_control c ON c.session_id=m.to_session WHERE m.state='held' AND COALESCE(c.paused,0)=0 ORDER BY m.to_session").map_err(sql)?;
    let ids = query.query_map([], |row| row.get(0)).map_err(sql)?.collect::<rusqlite::Result<Vec<String>>>().map_err(sql)?;
    Ok(ids)
}

// ---------------------------------------------------------------------------------------
// Views

/// One agent as the UI and `pandacode_list_agents` see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentView {
    pub session_id: String,
    pub parent_session_id: Option<String>,
    pub name: String,
    pub task: String,
    pub depth: i64,
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
    pub mode: String,
    /// `running`, `waiting` (needs the user), `paused`, `error`, `stopped`, `new` or `idle`.
    pub status: String,
    pub paused: bool,
    pub held_mail: i64,
    pub worktree_branch: Option<String>,
    pub kind: String,
    pub created_at: i64,
}

fn status(store: &Store, session: &Session, paused: bool, busy: bool) -> String {
    let status = if paused {
        "paused"
    } else if busy {
        if matches!(session.status.as_str(), "waiting" | "waiting_approval") { "waiting" } else { "running" }
    } else {
        match session.status.as_str() {
            "error" => "error",
            "interrupted" => "stopped",
            _ if session.last_prompt_at.is_none() && held_count(store, &session.id).unwrap_or(0) > 0 => "new",
            _ => "idle",
        }
    };
    status.into()
}

pub(crate) fn view(store: &Store, sid: &str, busy: &dyn Fn(&str) -> bool) -> Result<AgentView> {
    let session = store.session(sid)?;
    let child = child(store, sid)?;
    let control = control(store, sid)?;
    Ok(AgentView {
        status: status(store, &session, control.paused, busy(sid)),
        paused: control.paused,
        held_mail: held_count(store, sid)?,
        name: child.as_ref().map_or_else(|| session.title.clone(), |child| child.name.clone()),
        task: child.as_ref().map(|child| child.task.clone()).unwrap_or_default(),
        depth: child.as_ref().map_or(0, |child| child.depth),
        parent_session_id: child.map(|child| child.parent_session_id),
        mode: effective_mode(store, &session),
        session_id: session.id,
        provider: session.provider,
        model: session.model,
        effort: session.reasoning_effort,
        worktree_branch: session.worktree_branch,
        kind: session.kind,
        created_at: session.created_at,
    })
}

/// A thread's agents for the UI: the thread itself (its pause and held mail) and its
/// descendants, depth first.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub session_id: String,
    pub paused: bool,
    pub held_mail: i64,
    pub children: Vec<AgentView>,
}

pub(crate) fn snapshot(store: &Store, sid: &str, busy: &dyn Fn(&str) -> bool) -> Result<Snapshot> {
    let control = control(store, sid)?;
    let children = descendants(store, sid)?.iter().map(|id| view(store, id, busy)).collect::<Result<Vec<_>>>()?;
    Ok(Snapshot { session_id: sid.to_owned(), paused: control.paused, held_mail: held_count(store, sid)?, children })
}

/// Held mail as the UI lists it: who it is from and how it begins.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MailView {
    pub id: String,
    pub from_session: String,
    pub from_name: String,
    pub to_session: String,
    pub kind: String,
    pub preview: String,
    pub reply_expected: bool,
    pub created_at: i64,
}

pub(crate) fn held_views(store: &Store, sid: &str) -> Result<Vec<MailView>> {
    Ok(held(store, sid)?
        .into_iter()
        .map(|mail| MailView {
            preview: mail.body.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(240).collect(),
            id: mail.id,
            from_session: mail.from_session,
            from_name: mail.from_name,
            to_session: mail.to_session,
            kind: mail.kind,
            reply_expected: mail.reply_expected,
            created_at: mail.created_at,
        })
        .collect())
}

/// The final written answer of the turn saved at `turn_message_id` (its last assistant row).
pub(crate) fn answer_after(store: &Store, sid: &str, turn_message_id: i64) -> Option<String> {
    store
        .db
        .query_row("SELECT content FROM messages WHERE session_id=?1 AND role='assistant' AND id>?2 AND content<>'' ORDER BY id DESC LIMIT 1", params![sid, turn_message_id], |row| row.get(0))
        .optional()
        .ok()
        .flatten()
}

/// The worktrees of the child agents among `ids`: `(project root, checkout, branch)`.
pub(crate) fn worktrees(store: &Store, ids: &[String]) -> Vec<(PathBuf, PathBuf, Option<String>)> {
    ids.iter()
        .filter(|id| child(store, id).ok().flatten().is_some())
        .filter_map(|id| {
            let session = store.session(id).ok()?;
            let path = PathBuf::from(session.worktree_path?);
            let root = PathBuf::from(store.project(&session.project_id).ok()?.path);
            Some((root, path, session.worktree_branch))
        })
        .collect()
}

/// ViewCode's cleanup rule for a deleted thread's worktree, keep when it has uncommitted
/// changes: each clean child checkout is removed with its branch (a branch with unmerged
/// commits is kept); one with uncommitted changes stays. Returns what was kept and why.
pub(crate) fn remove_worktrees(list: &[(PathBuf, PathBuf, Option<String>)]) -> Vec<(PathBuf, String)> {
    let mut kept = Vec::new();
    for (root, path, branch) in list {
        if let Err(error) = crate::workspace::remove_worktree(root, path, branch.as_deref(), true) {
            kept.push((path.clone(), error));
        }
    }
    kept
}

// ---------------------------------------------------------------------------------------
// The app: runs, events and IPC

/// Woken whenever one of a parent's children ends a run (`pandacode_wait_agents`).
pub(crate) fn signal(parent: &str) -> Arc<tokio::sync::Notify> {
    static SIGNALS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Notify>>>> = OnceLock::new();
    let mut map = SIGNALS.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    map.entry(parent.to_owned()).or_default().clone()
}

pub(crate) fn announce(app: &tauri::AppHandle, store: &Store, sid: &str, reason: &str) {
    let parent = child(store, sid).ok().flatten().map(|child| child.parent_session_id);
    let root = root_of(store, sid).unwrap_or_else(|_| sid.to_owned());
    let _ = app.emit(CHANGED, serde_json::json!({ "parentId": parent, "rootId": root, "sessionId": sid, "reason": reason }));
}

fn busy_in(state: &AppState) -> impl Fn(&str) -> bool + '_ {
    move |id: &str| state.agents.lock().map(|agents| agents.contains_key(id)).unwrap_or(true)
}

/// Starts `sid`'s next held message as a turn, if it is idle and not paused. Spawned: the
/// caller may hold the agents registry.
pub(crate) fn pump(app: &tauri::AppHandle, sid: &str) {
    let (app, sid) = (app.clone(), sid.to_owned());
    tauri::async_runtime::spawn(async move {
        if let Err(error) = deliver_next(&app, &sid).await {
            eprintln!("PandaCode: could not deliver an agent message: {error}");
        }
    });
}

async fn deliver_next(app: &tauri::AppHandle, sid: &str) -> Result<()> {
    let next = {
        let state = app.state::<AppState>();
        let agents = state.agents.lock().map_err(|e| e.to_string())?;
        if agents.contains_key(sid) {
            return Ok(());
        }
        let store = state.store.lock().map_err(|e| e.to_string())?;
        ready(&store, sid)?.map(|mail| (mail.id.clone(), envelope(&mail)))
    };
    let Some((mail_id, text)) = next else { return Ok(()) };
    match crate::launch_mail_turn(app.clone(), sid.to_owned(), mail_id.clone(), text).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let state = app.state::<AppState>();
            // Lost a race with another start: the message waits for that run's end.
            if state.agents.lock().map(|agents| agents.contains_key(sid)).unwrap_or(true) {
                return Ok(());
            }
            let sender = {
                let store = state.store.lock().map_err(|e| e.to_string())?;
                let sender = bounce(&store, &mail_id, &error, crate::storage::now())?;
                announce(app, &store, sid, "failed");
                sender
            };
            if let Some(sender) = sender {
                pump(app, &sender);
            }
            Err(error)
        }
    }
}

/// Called at every run end (lib.rs), with the agents registry still held: replies go out,
/// waiting `pandacode_wait_agents` calls wake, and held mail is delivered once it is free.
pub(crate) fn run_ended(app: &tauri::AppHandle, sid: &str, run_id: &str, turn_message_id: i64, successful: bool, stopped: bool, error: Option<&str>) {
    let state = app.state::<AppState>();
    let Ok(store) = state.store.lock() else { return };
    let answer = (turn_message_id > 0).then(|| answer_after(&store, sid, turn_message_id)).flatten();
    let outcome = if stopped {
        Outcome::Stopped
    } else if successful {
        Outcome::Completed { answer }
    } else {
        Outcome::Failed { answer, error: error.unwrap_or("the agent could not finish this turn").to_owned() }
    };
    let ended = match run_finished(&store, sid, run_id, &outcome, crate::storage::now()) {
        Ok(ended) => ended,
        Err(error) => {
            eprintln!("PandaCode: could not route an agent reply: {error}");
            return;
        }
    };
    if let Some(dropped) = &ended.dropped {
        eprintln!("PandaCode: an agent reply was not sent: {dropped}");
    }
    let tree_member = in_tree(&store, sid).unwrap_or(false);
    if let Ok(Some(child)) = child(&store, sid) {
        signal(&child.parent_session_id).notify_waiters();
    }
    if tree_member || !ended.replies.is_empty() {
        announce(app, &store, sid, "run-ended");
    }
    drop(store);
    for id in ended.wake {
        pump(app, &id);
    }
}

/// The user's Stop on `sid` (lib.rs `stop_session`): in a tree, the agent is paused and so
/// is every agent it started; running ones are stopped. Outside a tree nothing changes.
pub(crate) fn task_stopped(app: &tauri::AppHandle, sid: &str) {
    let state = app.state::<AppState>();
    let scope = {
        let Ok(store) = state.store.lock() else { return };
        if !in_tree(&store, sid).unwrap_or(false) {
            return;
        }
        let scope = pause(&store, sid).unwrap_or_default();
        announce(app, &store, sid, "paused");
        scope
    };
    stop_runs(&state, scope.iter().filter(|id| *id != sid));
}

fn stop_runs<'a>(state: &AppState, ids: impl Iterator<Item = &'a String>) {
    let Ok(agents) = state.agents.lock() else { return };
    for id in ids {
        if let Some(agent) = agents.get(id) {
            if let Ok(store) = state.store.lock() {
                let _ = store.request_stop(id);
            }
            let _ = agent.input.send(crate::providers::RunInput::Stop);
        }
    }
}

/// Starts delivering mail held across a restart (`recover`).
pub(crate) fn start(app: tauri::AppHandle) {
    let ready = {
        let state = app.state::<AppState>();
        let Ok(store) = state.store.lock() else { return };
        match recover(&store) {
            Ok(ready) => ready,
            Err(error) => {
                eprintln!("PandaCode: could not recover agent mail: {error}");
                return;
            }
        }
    };
    for id in ready {
        pump(&app, &id);
    }
}

/// Spawns a child for `caller` (the tool): checks, makes the worktree outside the store lock,
/// saves the child and starts its first turn.
pub(crate) async fn spawn_child(app: &tauri::AppHandle, caller: &str, input: SpawnInput<'_>) -> Result<serde_json::Value> {
    let state = app.state::<AppState>();
    let caller_run = state.agents.lock().map_err(|e| e.to_string())?.get(caller).map(|agent| agent.run_id.clone());
    let plan = {
        let store = state.store.lock().map_err(|e| e.to_string())?;
        plan_spawn(&store, caller, caller_run.as_deref(), input, &crate::launch_gate::allow)?
    };
    let (checkout, branch) = (plan.checkout.clone(), plan.branch.clone());
    let worktree = crate::blocking(move || crate::workspace::create_worktree_with(&checkout, &branch, None, false))
        .await
        .map_err(|error| format!("The child agent needs its own worktree, which could not be made: {error}"))?;
    let spawned = {
        let store = state.store.lock().map_err(|e| e.to_string())?;
        match commit_spawn(&store, &plan, &worktree.path, crate::storage::now()) {
            Ok(spawned) => {
                announce(app, &store, &spawned.agent_id, "spawned");
                spawned
            }
            Err(error) => {
                drop(store);
                let root = PathBuf::from(state.store.lock().map_err(|e| e.to_string())?.project(&plan.parent.project_id)?.path);
                let _ = crate::workspace::remove_worktree(&root, &worktree.path, Some(&worktree.branch), true);
                return Err(error);
            }
        }
    };
    let _ = app.emit("sessions-changed", ());
    pump(app, &spawned.agent_id);
    Ok(serde_json::json!({ "agentId": spawned.agent_id, "name": spawned.name, "messageId": spawned.message_id, "delivery": "started", "branch": worktree.branch }))
}

#[tauri::command]
pub(crate) async fn child_agents_list(state: State<'_, AppState>, session_id: String) -> Result<Snapshot> {
    let busy = busy_in(&state);
    let store = state.store.lock().map_err(|e| e.to_string())?;
    snapshot(&store, &session_id, &busy)
}

#[tauri::command]
pub(crate) async fn child_agents_mail(state: State<'_, AppState>, session_id: String) -> Result<Vec<MailView>> {
    held_views(&*state.store.lock().map_err(|e| e.to_string())?, &session_id)
}

/// Pauses one agent and the agents it started, stopping their runs. Siblings and the parent
/// keep going.
#[tauri::command]
pub(crate) async fn child_agent_stop(app: tauri::AppHandle, state: State<'_, AppState>, session_id: String) -> Result<()> {
    let scope = {
        let store = state.store.lock().map_err(|e| e.to_string())?;
        store.session(&session_id)?;
        let scope = pause(&store, &session_id)?;
        announce(&app, &store, &session_id, "paused");
        scope
    };
    stop_runs(&state, scope.iter());
    Ok(())
}

#[tauri::command]
pub(crate) async fn child_agent_resume(app: tauri::AppHandle, state: State<'_, AppState>, session_id: String) -> Result<()> {
    {
        let store = state.store.lock().map_err(|e| e.to_string())?;
        resume(&store, &session_id, crate::storage::now())?;
        announce(&app, &store, &session_id, "resumed");
    }
    pump(&app, &session_id);
    Ok(())
}

#[tauri::command]
pub(crate) async fn child_agent_discard(app: tauri::AppHandle, state: State<'_, AppState>, session_id: String, mail_id: Option<String>) -> Result<usize> {
    let store = state.store.lock().map_err(|e| e.to_string())?;
    let dropped = discard(&store, &session_id, mail_id.as_deref())?;
    announce(&app, &store, &session_id, "discarded");
    Ok(dropped)
}

#[tauri::command]
pub(crate) async fn child_agent_open(app: tauri::AppHandle, state: State<'_, AppState>, session_id: String) -> Result<Session> {
    let session = {
        let store = state.store.lock().map_err(|e| e.to_string())?;
        let session = open_as_thread(&store, &session_id, crate::storage::now())?;
        announce(&app, &store, &session_id, "opened");
        session
    };
    let _ = app.emit("sessions-changed", ());
    Ok(session)
}
