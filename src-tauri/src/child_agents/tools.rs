//! The agent tools for child agents (ViewCode's `viewcode_*` agents toolkit, with this app's
//! `pandacode_` prefix): spawn a child, message any agent in the tree (a child or the parent),
//! list the tree, wait for children and collect their answers, read a transcript, and stop a
//! child. Registered on ordinary threads and child agents; not on side chats, team threads,
//! bot conversations or tasks that run on another computer or a cloud machine.
use super::*;
use crate::agent_tools::{spec, validate_spec};
use crate::providers::ToolHost;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const SPAWN: &str = "pandacode_spawn_agent";
pub(crate) const SEND: &str = "pandacode_send_message";
pub(crate) const LIST: &str = "pandacode_list_agents";
pub(crate) const WAIT: &str = "pandacode_wait_agents";
pub(crate) const READ: &str = "pandacode_read_transcript";
pub(crate) const STOP: &str = "pandacode_stop_agent";
/// `pandacode_wait_agents`: default and longest wait. Harnesses give up on a host tool after
/// about a minute, so a wait always answers sooner.
pub(crate) const WAIT_DEFAULT: u64 = 30;
pub(crate) const WAIT_MAX: u64 = 40;
/// `pandacode_read_transcript`: default and most messages.
const READ_DEFAULT: usize = 12;
const READ_MAX: usize = 100;

pub(crate) fn catalog() -> Vec<Value> {
    let agent = json!({"type":"string","maxLength":200,"description":"The agent's id from pandacode_list_agents, or its exact name."});
    vec![
        spec("list_agents", "List the PandaCode agents in your agent tree (your root thread and all of its descendants) with their relation to you, harness, model, reasoning effort and status. paused: the user stopped this agent; messages to it wait until the user resumes it. Use the ids with pandacode_send_message, pandacode_wait_agents, pandacode_read_transcript and pandacode_stop_agent.", json!({}), &[]),
        spec("spawn_agent", "Start a child agent: a separate, full PandaCode agent with its own conversation and model that the user can open, follow and stop. It works in its own git worktree made from your checkout's current commit (uncommitted edits are not copied). Give it a short name and a self-contained prompt; it does not see your conversation. By default its final answer is sent back to you as a message when it finishes, which starts a new turn for you if you are idle: end your turn instead of polling, or call pandacode_wait_agents when you need the answer in this turn. It runs in your permission mode, never a broader one. Prefer this over in-session sub-agents when the user asks for agents they can see, or for work in a separate checkout.", json!({
            "name":{"type":"string","maxLength":80,"description":"Short display name, e.g. \"Frontend audit\"."},
            "task":{"type":"string","maxLength":120,"description":"What the agent is doing, in 3 to 8 words. Shown next to its name."},
            "prompt":{"type":"string","maxLength":crate::MESSAGE_LIMIT,"description":"Complete instructions. The child does not see your conversation."},
            "provider_id":{"type":"string","maxLength":40,"description":"Harness id, e.g. claude or codex. Defaults to your own. An agent the user has not turned on, or that their organization blocks, is refused."},
            "model":{"type":"string","maxLength":128,"description":"Model id. Defaults to your model on your harness, else the harness's default."},
            "effort":{"type":"string","maxLength":32,"description":"Reasoning effort, e.g. high. Defaults to yours on your harness."},
            "reply_expected":{"type":"boolean","description":"Send the child's final answer back to you. Default true."}
        }), &["name","prompt"]),
        spec("send_message", "Send a message to another agent in your tree: one of your children, your parent, or another agent there. An idle agent starts working on it immediately; a busy one gets it after its current turn, and a paused one when the user resumes it. Set reply_expected to have its answer routed back to you. To answer a message that expects a reply, pass its response_id. A chain of automatic messages stops after 24 hops.", json!({
            "to":agent,
            "message":{"type":"string","maxLength":crate::MESSAGE_LIMIT},
            "reply_expected":{"type":"boolean"},
            "response_id":{"type":"string","maxLength":64,"description":"The message-id of a message you are answering. Marks this send as the reply."}
        }), &["to","message"]),
        spec("wait_agents", &format!("Wait (at most timeout_seconds, default {WAIT_DEFAULT}, max {WAIT_MAX}) until the listed child agents (default: all your children) are no longer running, then return each one's status and answer. An answer still waiting to be delivered to you is collected here instead of starting a new turn for you. Prefer ending your turn: finished children report back on their own. Call again while done is false."), json!({
            "agents":{"type":"array","items":{"type":"string","maxLength":200},"maxItems":20},
            "timeout_seconds":{"type":"integer","description":"1 to 40, default 30"}
        }), &[]),
        spec("read_transcript", "Read the recent conversation of another agent in your tree.", json!({
            "agent":agent,
            "last_messages":{"type":"integer","description":"How many recent messages. Default 12, at most 100."}
        }), &["agent"]),
        spec("stop_agent", "Stop one of your child agents (and the agents it started): its running turn is interrupted and it is paused, so messages to it wait until the user resumes it. Its siblings and you keep running.", json!({"agent":agent}), &["agent"]),
    ]
}

fn names() -> Vec<String> {
    catalog().iter().filter_map(|tool| tool["name"].as_str().map(str::to_owned)).collect()
}

/// Tools that act only inside the caller's own tree and start nothing new, so harnesses need
/// not ask before them. Spawning starts a new agent and asks unless the task is in Full access,
/// like every other PandaCode host tool that starts work.
pub(crate) fn self_scoped(name: &str) -> bool {
    name != SPAWN && names().iter().any(|known| known == name)
}

/// What a tool call needs from the app, so the same code runs in tests without one.
pub(crate) trait Env {
    fn busy(&self, sid: &str) -> bool;
    fn gate(&self, harness: &str) -> Result<()>;
    /// Makes the child's worktree on `plan.branch` from `plan.checkout`; returns its path.
    fn worktree(&self, plan: &SpawnPlan) -> Result<PathBuf>;
    fn now(&self) -> i64;
}

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args[key].as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn spawn_input(args: &Value) -> SpawnInput<'_> {
    SpawnInput {
        name: args["name"].as_str().unwrap_or_default(),
        task: text(args, "task"),
        prompt: args["prompt"].as_str().unwrap_or_default(),
        provider: text(args, "provider_id"),
        model: text(args, "model"),
        effort: text(args, "effort"),
        reply_expected: args["reply_expected"].as_bool(),
    }
}

fn relation(caller: &str, own_parent: Option<&str>, other: &AgentView) -> &'static str {
    if other.session_id == caller {
        "you"
    } else if Some(other.session_id.as_str()) == own_parent {
        "parent"
    } else if other.parent_session_id.as_deref() == Some(caller) {
        "child"
    } else if other.parent_session_id.is_some() && other.parent_session_id.as_deref() == own_parent {
        "sibling"
    } else {
        "other"
    }
}

fn list(store: &Store, env: &dyn Env, caller: &str) -> Result<Value> {
    let own_parent = child(store, caller)?.map(|child| child.parent_session_id);
    let busy = |id: &str| env.busy(id);
    let agents: Vec<Value> = tree(store, caller)?
        .iter()
        .filter_map(|id| view(store, id, &busy).ok())
        .map(|agent| {
            // ViewCode's statuses: a needs-you turn is still running.
            let status = if agent.status == "waiting" { "running" } else { agent.status.as_str() };
            json!({"id":agent.session_id,"name":agent.name,"task":agent.task,"parentId":agent.parent_session_id,"relation":relation(caller,own_parent.as_deref(), &agent),"provider":agent.provider,"model":agent.model,"effort":agent.effort,"status":status,"queuedMessages":agent.held_mail})
        })
        .collect();
    Ok(json!({"agents": agents}))
}

/// Status and answer of each child `wait_agents` names; `done` once none is running.
/// Answers held for the caller are collected (they will not start a turn as well).
pub(crate) fn wait_check(store: &Store, env: &dyn Env, caller: &str, wanted: &[String]) -> Result<Value> {
    let ids: Vec<String> = if wanted.is_empty() {
        children(store, caller)?
    } else {
        wanted.iter().map(|target| resolve(store, caller, target).map(|session| session.id)).collect::<Result<Vec<_>>>()?
    };
    for id in &ids {
        if child(store, id)?.is_none_or(|child| child.parent_session_id != caller) {
            return Err("pandacode_wait_agents waits only for your own child agents.".into());
        }
    }
    let busy = |id: &str| env.busy(id);
    let mut done = true;
    let mut agents = Vec::new();
    for id in &ids {
        let agent = view(store, id, &busy)?;
        let running = matches!(agent.status.as_str(), "running" | "waiting" | "new");
        done &= !running;
        let reply = mails(store, "WHERE to_session=?1 AND from_session=?2 AND state='held' AND kind='reply' ORDER BY created_at DESC,rowid DESC LIMIT 1", params![caller, id])?.pop();
        let (answer, collected) = match reply {
            Some(reply) if !running => {
                store.db.execute("UPDATE agent_mail SET state='collected' WHERE id=?1", [&reply.id]).map_err(sql)?;
                (Some(reply.body), true)
            }
            _ if !running => (store.db.query_row("SELECT content FROM messages WHERE session_id=?1 AND role='assistant' AND content<>'' ORDER BY id DESC LIMIT 1", [id], |row| row.get::<_, String>(0)).optional().map_err(sql)?.map(|text| tail(&text, REPLY_CHARS).to_owned()), false),
            _ => (None, false),
        };
        agents.push(json!({"id":agent.session_id,"name":agent.name,"status":agent.status,"answer":answer,"collected":collected}));
    }
    Ok(json!({"done": done, "agents": agents}))
}

fn read(store: &Store, caller: &str, args: &Value) -> Result<Value> {
    let target = resolve(store, caller, args["agent"].as_str().unwrap_or_default())?;
    let count = args["last_messages"].as_u64().map_or(READ_DEFAULT, |n| (n as usize).clamp(1, READ_MAX));
    let mut query = store.db.prepare("SELECT role,content FROM (SELECT id,role,content FROM messages WHERE session_id=?1 AND role IN ('user','assistant') AND content<>'' ORDER BY id DESC LIMIT ?2) ORDER BY id").map_err(sql)?;
    let rows = query.query_map(params![target.id, count as i64], |row| Ok(format!("{}: {}", row.get::<_, String>(0)?, row.get::<_, String>(1)?))).map_err(sql)?.collect::<rusqlite::Result<Vec<_>>>().map_err(sql)?;
    let transcript = rows.join("\n\n");
    Ok(json!({"agentId": target.id, "transcript": tail(&transcript, REPLY_CHARS)}))
}

/// The tool calls that need nothing but the store: everything except the wait loop and
/// stopping a running process. `spawn_agent` here makes its worktree through `env`.
pub(crate) fn call(store: &Store, env: &dyn Env, caller: &str, caller_run: Option<&str>, name: &str, args: &Value) -> Result<Value> {
    validate_spec(&catalog(), name, args)?;
    match name {
        LIST => list(store, env, caller),
        SPAWN => {
            let plan = plan_spawn(store, caller, caller_run, spawn_input(args), &|harness| env.gate(harness))?;
            let worktree = env.worktree(&plan)?;
            let spawned = commit_spawn(store, &plan, &worktree, env.now())?;
            Ok(json!({"agentId":spawned.agent_id,"name":spawned.name,"messageId":spawned.message_id,"delivery":"started","branch":plan.branch}))
        }
        SEND => {
            let sent = send(store, caller, caller_run, Message { to: args["to"].as_str().unwrap_or_default(), body: args["message"].as_str().unwrap_or_default(), reply_expected: args["reply_expected"].as_bool().unwrap_or(false), response_id: text(args, "response_id") }, &|id| env.busy(id), env.now())?;
            Ok(json!({"messageId": sent.message_id, "delivery": sent.delivery}))
        }
        READ => read(store, caller, args),
        WAIT => wait_check(store, env, caller, &wanted(args)),
        STOP => {
            let target = resolve(store, caller, args["agent"].as_str().unwrap_or_default())?;
            if !descendants(store, caller)?.contains(&target.id) {
                return Err("pandacode_stop_agent stops only agents you started (your children and theirs).".into());
            }
            let scope = pause(store, &target.id)?;
            Ok(json!({"agentId": target.id, "status": "paused", "stopped": scope}))
        }
        _ => Err("Unknown PandaCode tool.".into()),
    }
}

fn wanted(args: &Value) -> Vec<String> {
    args["agents"].as_array().into_iter().flatten().filter_map(|id| id.as_str().map(str::to_owned)).collect()
}

/// Whether `session_id` gets these tools.
fn eligible(store: &Store, session_id: &str) -> bool {
    let Ok(session) = store.session(session_id) else { return false };
    if session.kind == crate::side_chat::KIND || session.remote_device.is_some() || session.cloud.is_some() {
        return false;
    }
    let team = store.db.query_row("SELECT EXISTS(SELECT 1 FROM architect_workers WHERE session_id=?1 OR parent_session_id=?1)", [session_id], |row| row.get::<_, bool>(0)).unwrap_or(true);
    !team && !crate::architect::load(store, session_id).ok().flatten().is_some() && !crate::bots::owner_of(store, session_id).ok().flatten().is_some()
}

struct AppEnv<'a>(&'a AppState);
impl Env for AppEnv<'_> {
    fn busy(&self, sid: &str) -> bool {
        self.0.agents.lock().map(|agents| agents.contains_key(sid)).unwrap_or(true)
    }
    fn gate(&self, harness: &str) -> Result<()> {
        crate::launch_gate::allow(harness)
    }
    fn worktree(&self, _: &SpawnPlan) -> Result<PathBuf> {
        Err("Spawning runs through spawn_child.".into())
    }
    fn now(&self) -> i64 {
        crate::storage::now()
    }
}

/// Adds the child agent tools for eligible threads; otherwise returns the host unchanged.
pub(crate) fn with_children(base: Option<ToolHost>, app: tauri::AppHandle, session_id: String, stopped: Arc<AtomicBool>) -> Option<ToolHost> {
    let allowed = app.state::<AppState>().store.lock().ok().is_some_and(|store| eligible(&store, &session_id));
    if !allowed {
        return base;
    }
    let mut tools = base.as_ref().map_or_else(Vec::new, |host| host.tools.clone());
    let mut approval_tools = base.as_ref().map_or_else(Default::default, |host| host.approval_tools.clone());
    approval_tools.insert(SPAWN.to_owned());
    tools.extend(catalog());
    Some(ToolHost {
        tools,
        approval_tools,
        handler: Arc::new(move |name, args| {
            let (base, app, session_id, stopped) = (base.clone(), app.clone(), session_id.clone(), stopped.clone());
            Box::pin(async move {
                if stopped.load(Ordering::SeqCst) {
                    return Err("Task has stopped.".into());
                }
                if !names().contains(&name) {
                    return match base {
                        Some(base) => (base.handler)(name, args).await,
                        None => Err("Unknown PandaCode tool.".into()),
                    };
                }
                validate_spec(&catalog(), &name, &args)?;
                dispatch(&app, &session_id, &name, &args).await
            })
        }),
    })
}

async fn dispatch(app: &tauri::AppHandle, caller: &str, name: &str, args: &Value) -> Result<Value> {
    let state = app.state::<AppState>();
    match name {
        SPAWN => spawn_child(app, caller, spawn_input(args)).await,
        WAIT => wait(app, caller, args).await,
        _ => {
            let caller_run = state.agents.lock().map_err(|e| e.to_string())?.get(caller).map(|agent| agent.run_id.clone());
            let (result, target) = {
                let store = state.store.lock().map_err(|e| e.to_string())?;
                let result = call(&store, &AppEnv(&state), caller, caller_run.as_deref(), name, args)?;
                let target = match name {
                    SEND => resolve(&store, caller, args["to"].as_str().unwrap_or_default()).ok().map(|session| session.id),
                    STOP => result["agentId"].as_str().map(str::to_owned),
                    _ => None,
                };
                if let Some(target) = &target {
                    announce(app, &store, target, if name == STOP { "paused" } else { "mail" });
                }
                (result, target)
            };
            match (name, target) {
                (SEND, Some(target)) => pump(app, &target),
                (STOP, Some(_)) => {
                    let scope: Vec<String> = result["stopped"].as_array().into_iter().flatten().filter_map(|id| id.as_str().map(str::to_owned)).collect();
                    stop_runs(&state, scope.iter());
                }
                _ => {}
            }
            Ok(result)
        }
    }
}

async fn wait(app: &tauri::AppHandle, caller: &str, args: &Value) -> Result<Value> {
    let timeout = args["timeout_seconds"].as_u64().unwrap_or(WAIT_DEFAULT).clamp(1, WAIT_MAX);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout);
    let wanted = wanted(args);
    let signal = signal(caller);
    loop {
        let notified = signal.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let result = {
            let state = app.state::<AppState>();
            let store = state.store.lock().map_err(|e| e.to_string())?;
            wait_check(&store, &AppEnv(&state), caller, &wanted)?
        };
        if result["done"] == true {
            return Ok(result);
        }
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep_until(deadline) => return Ok(result),
        }
    }
}
