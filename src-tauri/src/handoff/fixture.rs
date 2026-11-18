//! A long made-up thread for the handoff tests: 304 messages in 58 turns of a user asking for
//! steps of a search index, an agent reading, editing and running tests (2 KB replies), a few
//! failures, app notices, and two very long messages (an old 120 KB reply and a recent 60 KB
//! pasted log). About 320 KB in all.
use crate::storage::Store;

pub(crate) const STANDING: [&str; 2] = ["Always run `npm test` before you say a step is done.", "Never touch the files under vendor/."];
pub(crate) const TURNS: usize = 58;
/// The reply of turn 5: far longer than any window's long-message limit.
pub(crate) const LONG_REPLY_BYTES: usize = 120_000;
/// What an ordinary reply adds after its three sentences.
pub(crate) const REPLY_NOTES_BYTES: usize = 2_000;
/// The user's message of the second-newest turn.
pub(crate) const LONG_PASTE_BYTES: usize = 60_000;

fn tool(id: &str, name: &str, status: &str, input: &str, output: &str, paths: Option<&[&str]>) -> String {
    let mut value = serde_json::json!({"type":"tool_activity","version":1,"id":id,"name":name,"status":status,"input":input,"output":output});
    if let Some(paths) = paths {
        value["writePaths"] = serde_json::json!(paths);
    }
    value.to_string()
}

fn filler(seed: &str, bytes: usize) -> String {
    let mut out = String::with_capacity(bytes + seed.len());
    let mut line = 0;
    while out.len() < bytes {
        line += 1;
        out.push_str(&format!("{seed} line {line}: the tokenizer saw 竹 and kept going.\n"));
    }
    out.truncate(super::floor(&out, bytes));
    out
}

/// `(role, content, tool id)` in thread order.
pub(crate) fn rows() -> Vec<(&'static str, String, Option<String>)> {
    let mut rows: Vec<(&'static str, String, Option<String>)> = Vec::new();
    for turn in 1..=TURNS {
        let file = format!("src/search/part_{}.ts", turn % 7);
        let user = match turn {
            1 => format!("Build an offline search index for the notes app. {} {}", STANDING[0], STANDING[1]),
            t if t == TURNS - 1 => format!("Here is the failing log from CI, look at the end.\n{}\nTAIL-OF-PASTE: the last line says exit 3.", filler("ci", LONG_PASTE_BYTES)),
            t if t == TURNS => "Now make the ranking stable when two notes tie.".to_owned(),
            t => format!("Step {t}: handle case {t} in the indexer and keep the public API unchanged."),
        };
        rows.push(("user", user, None));
        rows.push(("tool", tool(&format!("t{turn}:read"), "Read", "completed", &format!("{{\"file_path\":\"{file}\"}}"), &format!("export function part{turn}() {{}}\n"), None), Some(format!("t{turn}:read"))));
        rows.push(("tool", tool(&format!("t{turn}:edit"), "Edit", "completed", &format!("{{\"file_path\":\"{file}\",\"old_string\":\"a\",\"new_string\":\"b\"}}"), "Edited", Some(&[file.as_str()])), Some(format!("t{turn}:edit"))));
        if turn % 9 == 0 {
            rows.push(("tool", tool(&format!("t{turn}:test"), "Bash", "failed", "{\"command\":\"npm test\"}", &format!("FAIL search.test.ts\n  case {turn} expected 3 hits, got 2\n1 failed, 41 passed"), None), Some(format!("t{turn}:test"))));
            rows.push(("error", format!("The test run for step {turn} failed: case {turn} expected 3 hits, got 2."), None));
            rows.push(("assistant", format!("The root cause was an off-by-one in the window for case {turn}. I fixed the bound and the suite passes again."), None));
        } else {
            rows.push(("tool", tool(&format!("t{turn}:test"), "Bash", "completed", "{\"command\":\"npm test\"}", "PASS search.test.ts\n42 passed", None), Some(format!("t{turn}:test"))));
        }
        if turn % 20 == 0 {
            rows.push(("status", format!("Checkpoint saved after step {turn}"), None));
        }
        let reply = match turn {
            5 => format!("I decided to store postings as sorted arrays instead of hash sets because merges stay linear.\n{}\nEND-OF-LONG-REPLY", filler("design", LONG_REPLY_BYTES)),
            t if t == TURNS => "Ties now fall back to the note's creation time. Next step: add a test for three-way ties, which is still pending.".to_owned(),
            t => format!("Step {t} is in. I chose a trie for prefixes rather than scanning, because lookups for case {t} must stay under a millisecond. Next step: wire case {} into the ranker.\n{}", t + 1, filler("notes", REPLY_NOTES_BYTES)),
        };
        rows.push(("assistant", reply, None));
    }
    rows
}

/// Saves the thread and returns its message ids in order.
pub(crate) fn save(store: &Store, session_id: &str) -> Vec<i64> {
    for (role, content, tool_id) in rows() {
        match tool_id {
            Some(tool_id) => store.tool_message(session_id, &tool_id, &content).unwrap(),
            None => store.message(session_id, role, &content).unwrap(),
        }
    }
    let mut query = store.db.prepare("SELECT id FROM messages WHERE session_id=?1 ORDER BY id").unwrap();
    let ids = query.query_map([session_id], |row| row.get(0)).unwrap().collect::<rusqlite::Result<Vec<i64>>>().unwrap();
    ids
}

/// The same thread as the builder's input, without a store or a history file.
pub(crate) fn items() -> Vec<super::Item> {
    rows()
        .into_iter()
        .enumerate()
        .map(|(index, (role, content, _))| {
            let mut item = crate::context_recovery::item(index as i64 + 1, role, &content).0;
            item.line = Some(index * 4 + 5);
            item
        })
        .collect()
}
