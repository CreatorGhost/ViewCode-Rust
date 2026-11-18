//! Native session ids, kept per harness and account (docs/handoff.md#native-history).
//!
//! A thread has one live `resume_id`: the session of the harness and account it runs on now.
//! When it moves to another harness or account, that id is set aside here under the pair it
//! belongs to, with the last message that session saw. Coming back to the same pair resumes
//! that session and hands over only what happened since; no other pair is ever given it.
use crate::storage::Store;
use rusqlite::{params, OptionalExtension};

/// Schema 41.
pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS session_resume_ids(
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    account TEXT NOT NULL,
    resume_id TEXT NOT NULL,
    through_message_id INTEGER NOT NULL,
    used_tokens INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(session_id,provider,account));";

/// A harness may clear sessions it has not seen for a while; an older id is not tried.
pub(crate) const MAX_AGE_MS: i64 = 14 * 24 * 60 * 60 * 1000;

/// The account a thread uses on a harness: its connection, or `""` for the harness's own setup.
pub(crate) fn account(session_id: &str, provider: &str) -> String {
    crate::connections::for_task(session_id, provider).map_or_else(String::new, |connection| connection.id)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Saved {
    pub resume_id: String,
    /// The newest conversation message that session saw.
    pub through: i64,
    pub used_tokens: u64,
}

/// Sets the thread's live session aside under the pair it belongs to.
pub(crate) fn stash(store: &Store, session_id: &str, provider: &str, account: &str, now: i64) -> Result<(), String> {
    store
        .db
        .execute(
            "INSERT INTO session_resume_ids(session_id,provider,account,resume_id,through_message_id,used_tokens,updated_at)
             SELECT s.id,?2,?3,s.resume_id,COALESCE((SELECT MAX(id) FROM messages WHERE session_id=s.id AND role IN ('user','assistant','coordination') AND content<>''),0),COALESCE(s.context_used_tokens,0),?4
             FROM sessions s WHERE s.id=?1 AND s.resume_id IS NOT NULL AND trim(s.resume_id)<>''
             ON CONFLICT(session_id,provider,account) DO UPDATE SET resume_id=excluded.resume_id,through_message_id=excluded.through_message_id,used_tokens=excluded.used_tokens,updated_at=excluded.updated_at",
            params![session_id, provider, account, now],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Takes back the session this pair left, if it is recent enough to still exist.
pub(crate) fn take(store: &Store, session_id: &str, provider: &str, account: &str, now: i64) -> Result<Option<Saved>, String> {
    let saved = store
        .db
        .query_row(
            "SELECT resume_id,through_message_id,used_tokens,updated_at FROM session_resume_ids WHERE session_id=?1 AND provider=?2 AND account=?3",
            params![session_id, provider, account],
            |row| Ok((Saved { resume_id: row.get(0)?, through: row.get(1)?, used_tokens: row.get::<_, i64>(2)?.max(0) as u64 }, row.get::<_, i64>(3)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    store.db.execute("DELETE FROM session_resume_ids WHERE session_id=?1 AND provider=?2 AND account=?3", params![session_id, provider, account]).map_err(|e| e.to_string())?;
    Ok(saved.filter(|(_, updated)| now.saturating_sub(*updated) <= MAX_AGE_MS).map(|(saved, _)| saved))
}

/// A removed account's sessions cannot be resumed by anyone.
pub(crate) fn forget_account(store: &Store, account: &str) -> Result<(), String> {
    store.db.execute("DELETE FROM session_resume_ids WHERE account=?1 AND account<>''", [account]).map_err(|e| e.to_string())?;
    Ok(())
}
