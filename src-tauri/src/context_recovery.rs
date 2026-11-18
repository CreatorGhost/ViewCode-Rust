//! A one-time conversation handoff when a harness context must be replaced. This module keeps
//! what is pending and loads the saved thread; `handoff` decides what the new agent is told.
use crate::{
    handoff,
    storage::{Session, Store},
    Result,
};
use rusqlite::{params, OptionalExtension};
use serde_json::json;

pub(crate) const LEGACY_NOTICE: &str = "Architect mode starts a fresh harness context so its team tools are available. Your earlier conversation remains here.";
pub(crate) const RECOVERY_REQUIRED: &str = "This task's earlier conversation needs to be restored. Send a normal message in the conversation first, then retry this command.";
/// Schema 41: what a handoff covers and who wrote it (`Store::open`).
pub(crate) const SCHEMA: &str = "ALTER TABLE context_recovery ADD COLUMN after_message_id INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE context_recovery ADD COLUMN native_tokens INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE context_recovery ADD COLUMN from_provider TEXT;
    ALTER TABLE context_recovery ADD COLUMN from_model TEXT;";

pub(crate) fn pending(store: &Store, session: &Session) -> Result<bool> {
    store.db.query_row(
        "SELECT EXISTS(SELECT 1 FROM context_recovery WHERE session_id=?1) OR (?2 AND EXISTS(SELECT 1 FROM messages WHERE session_id=?1 AND role IN ('user','assistant','coordination') AND content<>''))",
        params![session.id, session.resume_id.as_deref().is_none_or(|id| id.trim().is_empty())],
        |row| row.get(0),
    ).map_err(|e| e.to_string())
}

/// Queue before clearing a resume ID, in the caller's configuration transaction. The whole
/// saved conversation is handed over; the agent that wrote it is the task's current one.
pub(crate) fn queue(store: &Store, sid: &str) -> Result<()> {
    store.session(sid)?;
    store.db.execute(
        "INSERT INTO context_recovery(session_id,through_message_id,after_message_id,native_tokens,from_provider,from_model) SELECT ?1,MAX(m.id),0,0,(SELECT provider FROM sessions WHERE id=?1),(SELECT model FROM sessions WHERE id=?1) FROM messages m WHERE m.session_id=?1 AND m.role IN ('user','assistant','coordination') AND m.content<>'' HAVING MAX(m.id) IS NOT NULL ON CONFLICT(session_id) DO UPDATE SET through_message_id=MAX(through_message_id,excluded.through_message_id),after_message_id=0,native_tokens=0",
        [sid],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

/// After `queue`, when the target resumes a session of its own that already holds everything
/// up to `after` (`handoff::resume`): only what came later is handed over, or nothing.
pub(crate) fn only_after(store: &Store, sid: &str, after: i64, native_tokens: u64) -> Result<()> {
    store
        .db
        .execute("UPDATE context_recovery SET after_message_id=?2,native_tokens=?3 WHERE session_id=?1", params![sid, after, native_tokens as i64])
        .map_err(|e| e.to_string())?;
    store.db.execute("DELETE FROM context_recovery WHERE session_id=?1 AND through_message_id<=after_message_id", [sid]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Keeps a pending handoff pending and extends it to the newest message, without widening a
/// handoff that covers only what a resumed session has not seen.
pub(crate) fn keep(store: &Store, sid: &str) -> Result<()> {
    store.session(sid)?;
    store.db.execute(
        "INSERT INTO context_recovery(session_id,through_message_id,from_provider,from_model) SELECT ?1,MAX(m.id),(SELECT provider FROM sessions WHERE id=?1),(SELECT model FROM sessions WHERE id=?1) FROM messages m WHERE m.session_id=?1 AND m.role IN ('user','assistant','coordination') AND m.content<>'' HAVING MAX(m.id) IS NOT NULL ON CONFLICT(session_id) DO UPDATE SET through_message_id=MAX(through_message_id,excluded.through_message_id)",
        [sid],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

/// What the turn start knows beyond the task and the request.
#[derive(Default)]
pub(crate) struct Options {
    /// Images attached to the current request; each reserves room in the target's window.
    pub images: usize,
    /// The target model's context window, when the caller knows better than the catalog.
    pub window: Option<u64>,
}

#[cfg(test)]
pub(crate) fn prepare(store: &Store, session: &Session, original: &str) -> Result<(String, Option<i64>)> {
    prepare_with(store, session, original, &Options::default())
}

fn display(provider: &str) -> String {
    crate::providers::PROVIDERS.iter().find(|(id, _)| *id == provider).map_or_else(|| provider.to_owned(), |(_, name)| (*name).to_owned())
}

/// A saved row as the handoff sees it, and its whole text for the history file.
pub(crate) fn item(id: i64, role: &str, content: &str) -> (handoff::Item, String) {
    use handoff::{Item, Kind};
    match role {
        "user" => (Item::text(Kind::User, id, content), content.to_owned()),
        "assistant" => (Item::text(Kind::Assistant, id, content), content.to_owned()),
        "error" => (Item::text(Kind::Error, id, content), content.to_owned()),
        "coordination" => match crate::architect::internal_turns::decode(content) {
            Some((_, prompt)) => (Item::text(Kind::Coordination, id, &prompt), prompt),
            None => (Item { role: role.into(), ..Item::text(Kind::Notice, id, content) }, content.to_owned()),
        },
        "tool" => {
            let saved: serde_json::Value = serde_json::from_str(content).unwrap_or_default();
            let structured = saved["type"] == "tool_activity";
            let text = |name: &str| saved[name].as_str().unwrap_or_default();
            let tool = if structured {
                let (input, output) = (text("input"), text("output"));
                handoff::Tool {
                    name: text("name").to_owned(),
                    status: text("status").to_owned(),
                    input: input[..handoff::floor(input, 2_000)].to_owned(),
                    output: output[handoff::ceil(output, output.len().saturating_sub(2_000))..].to_owned(),
                    write_paths: saved["writePaths"].as_array().map(|paths| paths.iter().filter_map(|path| path.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
                }
            } else {
                // Rows older than structured tool records: the name, then what it printed.
                let (name, rest) = content.split_once('\n').unwrap_or((content, ""));
                handoff::Tool { name: handoff::compact(name, 48), input: rest[..handoff::floor(rest, 2_000)].to_owned(), ..Default::default() }
            };
            let body = if structured { format!("Tool: {}\nStatus: {}\nInput:\n{}\nOutput:\n{}", tool.name, tool.status, text("input"), text("output")) } else { content.to_owned() };
            (Item { tool: Some(tool), bytes: content.len(), ..Item::text(Kind::Tool, id, "") }, body)
        }
        // An earlier switch, as a sentence instead of its saved record.
        "handoff" => {
            let saved: serde_json::Value = serde_json::from_str(content).unwrap_or_default();
            let text = match (saved["fromProvider"].as_str(), saved["toProvider"].as_str()) {
                (Some(from), Some(to)) => format!("The thread moved from {} to {} here.", display(from), display(to)),
                _ => "The thread's agent context was replaced here.".to_owned(),
            };
            (Item { role: "switch".into(), ..Item::text(Kind::Notice, id, &text) }, text)
        }
        other => (Item { role: other.into(), ..Item::text(Kind::Notice, id, content) }, content.to_owned()),
    }
}

/// Call before recording the new user message. The visible transcript keeps its rows; a
/// switch notice still waiting in it is rewritten to say what was carried and how.
///
/// Fails, with nothing started, when the target model's window has no room for even the
/// minimum handoff (`handoff::CANNOT_FIT`).
pub(crate) fn prepare_with(store: &Store, session: &Session, original: &str, options: &Options) -> Result<(String, Option<i64>)> {
    if !pending(store, session)? {
        return Ok((original.into(), None));
    }
    // A provider can assign its new ID before accepting the prompt. Include
    // intervening failed requests too, so a later “try again” keeps its referent.
    keep(store, &session.id)?;
    let row = store
        .db
        .query_row("SELECT through_message_id,after_message_id,native_tokens,from_provider,from_model FROM context_recovery WHERE session_id=?1", [&session.id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?))
        })
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((through, after, native_tokens, from_provider, from_model)) = row else {
        return Ok((original.into(), None));
    };
    let data_dir = store.path.parent().filter(|dir| dir.is_absolute());
    let mut attachments: std::collections::HashMap<i64, Vec<handoff::Attachment>> = Default::default();
    {
        let mut query = store.db.prepare("SELECT message_id,name,file FROM message_attachments WHERE session_id=?1 ORDER BY message_id,id").map_err(|e| e.to_string())?;
        let rows = query.query_map([&session.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))).map_err(|e| e.to_string())?;
        for row in rows {
            let (message, name, file) = row.map_err(|e| e.to_string())?;
            let path = file.zip(data_dir).map(|(file, dir)| dir.join("attachments").join(file));
            attachments.entry(message).or_default().push(handoff::Attachment { name, path });
        }
    }
    let mut changed: std::collections::HashMap<i64, Vec<String>> = Default::default();
    {
        let mut query = store
            .db
            .prepare("SELECT c.message_id,f.path FROM turn_changes c JOIN turn_change_files f ON f.change_id=c.id WHERE c.session_id=?1 ORDER BY c.message_id,f.path")
            .map_err(|e| e.to_string())?;
        let rows = query.query_map([&session.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
        for row in rows {
            let (message, path) = row.map_err(|e| e.to_string())?;
            let paths = changed.entry(message).or_default();
            if paths.len() < 50 {
                paths.push(path);
            }
        }
    }
    // One pass, one message in memory at a time: each goes whole into the history file, and
    // only what the builder needs of it is kept.
    let mut writer = handoff::archive::root(store).and_then(|root| handoff::archive::Writer::create(&root, &session.id, &session.title));
    let mut items = Vec::new();
    {
        let mut query = store.db.prepare("SELECT id,role,content FROM messages WHERE session_id=?1 AND id<=?2 AND content<>'' ORDER BY id").map_err(|e| e.to_string())?;
        let mut rows = query.query(params![session.id, through]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let (id, role): (i64, String) = (row.get(0).map_err(|e| e.to_string())?, row.get(1).map_err(|e| e.to_string())?);
            let content: String = row.get(2).map_err(|e| e.to_string())?;
            let (mut item, body) = item(id, &role, &content);
            let heading = match (&item.tool, item.kind) {
                (Some(tool), _) => format!("tool {}", handoff::compact(&tool.name, 48)),
                (None, handoff::Kind::Notice) => format!("notice {}", handoff::compact(&role, 24)),
                _ => item.role.clone(),
            };
            item.line = writer.as_mut().map(|writer| writer.push(id, &heading, &body));
            if id > after {
                item.attachments = attachments.remove(&id).unwrap_or_default();
                item.changed = changed.remove(&id).unwrap_or_default();
                items.push(item);
            }
        }
    }
    let archive = writer.and_then(handoff::archive::Writer::finish);
    // A side chat's first turn: the history is its parent's, not its own.
    let snapshot = crate::side_chat::is_snapshot(store, &session.id, through);
    let from = from_provider
        .filter(|provider| *provider != session.provider || from_model.as_deref() != Some(session.model.as_str()))
        .map(|provider| (display(&provider), from_model.unwrap_or_default()));
    let parties = handoff::Parties { from, to: (display(&session.provider), session.model.clone()), lead: snapshot.then(|| crate::side_chat::SNAPSHOT_NOTICE.to_owned()), delta: after > 0 };
    let window = options.window.or_else(|| handoff::window(store, &session.provider, &session.model));
    let budget = handoff::budget::budget(window, native_tokens.max(0) as u64, original.len(), options.images);
    let goal = session.goal_state.as_ref().map(|goal| (goal.objective.as_str(), goal.status.as_str()));
    let built = handoff::build(&handoff::Request { items: &items, budget: &budget, parties: &parties, archive: archive.as_deref(), goal })?;
    // The switch notice still waiting in the thread now says what was carried.
    let notice: Option<(i64, String)> = store
        .db
        .query_row("SELECT id,content FROM messages WHERE session_id=?1 AND role='handoff' AND id>?2 ORDER BY id DESC LIMIT 1", params![session.id, through], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some((id, content)) = notice {
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&content) {
            if value.is_object() {
                value["summary"] = handoff::notice(&parties, &budget, &built.stats, archive.is_some()).into();
                value["shortened"] = built.stats.shortened().into();
                value["carried"] = json!({"full": built.stats.verbatim, "summarised": built.stats.summarised, "referenced": built.stats.referenced, "excerpts": built.stats.excerpts, "bytes": built.stats.bytes, "window": budget.window});
                store.db.execute("UPDATE messages SET content=?2 WHERE id=?1", params![id, value.to_string()]).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok((format!("{}Current request:\n{original}", built.text), Some(through)))
}

/// A failed startup, cancelled turn, or failed persistence must leave recovery pending.
pub(crate) fn acknowledge(store: &Store, sid: &str, through: i64) -> Result<()> {
    store
        .db
        .execute(
            "DELETE FROM context_recovery WHERE session_id=?1 AND through_message_id=?2",
            params![sid, through],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests;
