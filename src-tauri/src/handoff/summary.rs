//! The structured summary of older turns, built from the saved transcript and tool records.
//! No model is asked: the same thread always gives the same summary, and it is always there.
//!
//! Every older message gets at least one line that names it (`#id`), so a message is either
//! represented here or, when its lines had to go, pointed at in the history file. Lines carry
//! a priority; the builder keeps the highest ones that fit.
use super::{compact, floor, Item, Kind};
use std::collections::BTreeMap;

pub(super) struct Line {
    pub section: u8,
    pub priority: u8,
    /// Thread order within the section.
    pub order: usize,
    /// Indexes of the older messages this line stands for.
    pub ids: Vec<usize>,
    pub text: String,
}

const SECTIONS: [&str; 8] = [
    "Goals and requests (the user's words, shortened)",
    "Decisions and reasons (from the agent's replies)",
    "Files created or changed",
    "Commands run and outcomes",
    "Errors and how they were resolved",
    "Other tool activity",
    "App notices",
    "Still open and next step",
];
const GOALS: u8 = 0;
const DECISIONS: u8 = 1;
const FILES: u8 = 2;
const COMMANDS: u8 = 3;
const ERRORS: u8 = 4;
const TOOLS: u8 = 5;
const NOTICES: u8 = 6;
const OPEN: u8 = 7;

const LINE: usize = 280;
/// Ids listed on one aggregated line; a longer group is split so each line stays droppable.
const GROUP: usize = 40;

/// A sentence that says a rule holds from now on.
const STANDING: [&str; 22] = [
    "always ", "never ", "from now on", "going forward", "every time", "at all times", "for the rest of", "until i say", "unless i say", "as a rule", "standing instruction", "remember to ", "remember that ",
    "remember: ", "do not ever", "don't ever", "must always", "must never", "must not ", "under no circumstances", "rule: ", "make sure you ",
];
/// A sentence that says what was chosen, and usually why.
const DECISION: [&str; 22] = [
    "decid", "chose", "choose", "instead", "because", "so that", "rather than", "going with", "went with", "switch", "the fix", "root cause", "fixed", "the plan", "approach", "trade-off", "tradeoff", "will use", "i'll use",
    "we'll use", "opted", "reason",
];
/// A sentence about work that is not finished.
const PENDING: [&str; 12] = ["next step", "next,", "next i", "todo", "to do:", "remaining", "still need", "follow up", "follow-up", "left to", "not yet", "pending"];

/// Prose sentences of a message: lines outside code fences, split at sentence ends.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced || trimmed.is_empty() {
            continue;
        }
        let (bytes, mut start) = (trimmed.as_bytes(), 0);
        for at in 0..bytes.len() {
            if matches!(bytes[at], b'.' | b'!' | b'?') && bytes.get(at + 1) == Some(&b' ') {
                out.push(trimmed[start..=at].trim());
                start = at + 1;
            }
        }
        if !trimmed[start..].trim().is_empty() {
            out.push(trimmed[start..].trim());
        }
    }
    out
}

fn has(sentence: &str, cues: &[&str]) -> bool {
    let lower = sentence.to_lowercase();
    cues.iter().any(|cue| lower.contains(cue))
}

/// The user's standing instructions, word for word. Always carried, never shortened: an
/// instruction that does not fit makes the handoff fail rather than go missing.
pub(super) fn standing(items: &[Item]) -> String {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = String::new();
    for item in items.iter().filter(|item| item.kind == Kind::User) {
        for sentence in sentences(&item.head) {
            if sentence.len() <= 500 && has(sentence, &STANDING) && seen.insert(sentence.to_lowercase()) {
                out.push_str(&format!("- (message {}) “{sentence}”\n", item.id));
            }
        }
    }
    if out.is_empty() {
        return out;
    }
    format!("## Standing instructions from the user (their exact words, from the whole thread)\n{out}\n")
}

fn field<'a>(value: &'a serde_json::Value, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|name| value[*name].as_str()).filter(|text| !text.is_empty())
}

/// The files a tool step wrote, when the harness said so or the step is a known edit.
fn written(tool: &super::Tool) -> Vec<String> {
    if !tool.write_paths.is_empty() {
        return tool.write_paths.clone();
    }
    let lower = tool.name.to_lowercase();
    if !["write", "edit", "patch", "create"].iter().any(|word| lower.contains(word)) {
        return Vec::new();
    }
    serde_json::from_str::<serde_json::Value>(&tool.input).ok().and_then(|input| field(&input, &["file_path", "notebook_path", "path", "filePath"]).map(str::to_owned)).into_iter().collect()
}

fn is_command(tool: &super::Tool) -> bool {
    let lower = tool.name.to_lowercase();
    ["bash", "command", "shell", "exec", "terminal"].iter().any(|word| lower.contains(word))
}

fn command_text(tool: &super::Tool) -> String {
    serde_json::from_str::<serde_json::Value>(&tool.input).ok().and_then(|input| field(&input, &["command", "cmd"]).map(str::to_owned)).unwrap_or_else(|| tool.input.clone())
}

fn failed(tool: &super::Tool) -> bool {
    let status = tool.status.to_lowercase();
    ["fail", "error", "denied", "reject"].iter().any(|word| status.contains(word))
}

fn unfinished(tool: &super::Tool) -> bool {
    let status = tool.status.to_lowercase();
    ["running", "pending", "interrupt"].iter().any(|word| status.contains(word)) || tool.output.contains("[The turn ended before this tool reported a result.]")
}

fn last_line(tool: &super::Tool) -> &str {
    tool.output.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("")
}

fn outcome(tool: &super::Tool) -> String {
    let last = last_line(tool);
    let status = if tool.status.is_empty() { "done" } else { &tool.status };
    if last.is_empty() {
        compact(status, 24)
    } else {
        format!("{}; {}", compact(status, 24), compact(last, 160))
    }
}

fn names(older: &[Item], ids: &[usize]) -> String {
    ids.iter().map(|index| format!("#{}", older[*index].id)).collect::<Vec<_>>().join(", ")
}

/// The next reply after `index`, as "what happened then".
fn then(older: &[Item], all: &[Item], index: usize) -> String {
    let after = older[index].id;
    all.iter()
        .find(|item| item.id > after && item.kind == Kind::Assistant)
        .and_then(|item| sentences(&item.head).first().map(|sentence| format!(" → then (message {}): {}", item.id, compact(sentence, 160))))
        .unwrap_or_else(|| " → no reply followed".to_owned())
}

/// One or more lines for every older message, plus what is still open across the thread.
pub(super) fn lines(older: &[Item], all: &[Item], goal: Option<&str>) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut files: BTreeMap<String, (usize, Vec<usize>, BTreeMap<String, usize>)> = BTreeMap::new();
    let mut tools: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut notices: Vec<usize> = Vec::new();
    let first_user = older.iter().position(|item| item.kind == Kind::User);
    for (index, item) in older.iter().enumerate() {
        let mark = format!("(#{})", item.id);
        match item.kind {
            Kind::User | Kind::Coordination => {
                let images = if item.attachments.is_empty() { String::new() } else { format!(" [{} image(s) attached]", item.attachments.len()) };
                let who = if item.kind == Kind::Coordination { " team coordination:" } else { "" };
                lines.push(Line { section: GOALS, priority: if Some(index) == first_user { 100 } else { 80 }, order: index, ids: vec![index], text: format!("- {mark}{who} {}{images}", compact(&item.head, LINE)) });
                for path in &item.changed {
                    let entry = files.entry(path.clone()).or_insert_with(|| (index, Vec::new(), BTreeMap::new()));
                    entry.1.push(index);
                    *entry.2.entry("changed in the turn".into()).or_default() += 1;
                }
            }
            Kind::Assistant => {
                let all_sentences = sentences(&item.head);
                let mut chosen: Vec<&str> = all_sentences.iter().copied().filter(|sentence| has(sentence, &DECISION)).take(2).collect();
                if chosen.is_empty() {
                    chosen.extend(all_sentences.first());
                }
                let text = if chosen.is_empty() { compact(&item.head, LINE) } else { compact(&chosen.join(" "), LINE) };
                lines.push(Line { section: DECISIONS, priority: 60, order: index, ids: vec![index], text: format!("- {mark} {text}") });
            }
            Kind::Error => {
                lines.push(Line { section: ERRORS, priority: 75, order: index, ids: vec![index], text: format!("- {mark} {}{}", compact(&item.head, 200), then(older, all, index)) });
            }
            Kind::Tool => {
                let tool = item.tool.clone().unwrap_or_default();
                let paths = written(&tool);
                if failed(&tool) {
                    let what = if is_command(&tool) { format!("`{}`", compact(&command_text(&tool), 120)) } else { format!("{} {}", compact(&tool.name, 40), compact(&tool.input, 100)) };
                    lines.push(Line { section: ERRORS, priority: 75, order: index, ids: vec![index], text: format!("- {mark} {what} failed: {}{}", compact(if last_line(&tool).is_empty() { &tool.status } else { last_line(&tool) }, 160), then(older, all, index)) });
                } else if !paths.is_empty() {
                    for path in paths.into_iter().take(20) {
                        let entry = files.entry(compact(&path, 300)).or_insert_with(|| (index, Vec::new(), BTreeMap::new()));
                        entry.1.push(index);
                        *entry.2.entry(compact(&tool.name, 40)).or_default() += 1;
                    }
                } else if is_command(&tool) {
                    lines.push(Line { section: COMMANDS, priority: if unfinished(&tool) { 70 } else { 50 }, order: index, ids: vec![index], text: format!("- {mark} `{}` → {}", compact(&command_text(&tool), 160), outcome(&tool)) });
                } else {
                    tools.entry(compact(&tool.name, 40)).or_default().push(index);
                }
            }
            Kind::Notice => notices.push(index),
        }
    }
    for (path, (order, ids, by)) in files {
        let how = by.iter().map(|(name, count)| if *count > 1 { format!("{name} ×{count}") } else { name.clone() }).collect::<Vec<_>>().join(", ");
        for chunk in ids.chunks(GROUP) {
            lines.push(Line { section: FILES, priority: 70, order, ids: chunk.to_vec(), text: format!("- {path} — {how} ({})", names(older, chunk)) });
        }
    }
    for (name, ids) in tools {
        for chunk in ids.chunks(GROUP) {
            lines.push(Line { section: TOOLS, priority: 20, order: chunk[0], ids: chunk.to_vec(), text: format!("- {name} ×{}: {}", chunk.len(), names(older, chunk)) });
        }
    }
    for chunk in notices.chunks(GROUP) {
        lines.push(Line { section: NOTICES, priority: 10, order: chunk[0], ids: chunk.to_vec(), text: format!("- {} status lines, change summaries or switch notices: {}", chunk.len(), names(older, chunk)) });
    }
    // What is still open, from the whole thread. These name messages without `#`, since the
    // messages themselves are accounted for elsewhere.
    let mut open = Vec::new();
    if let Some(goal) = goal {
        open.push(format!("- {goal}"));
    }
    for item in all.iter().filter(|item| item.tool.as_ref().is_some_and(unfinished)) {
        let tool = item.tool.as_ref().expect("filtered");
        open.push(format!("- A {} step never reported a result (message {}): {}", compact(&tool.name, 40), item.id, compact(&tool.input, 120)));
    }
    if let Some(error) = all.iter().rev().find(|item| item.kind == Kind::Error).filter(|error| !all.iter().any(|item| item.id > error.id && item.kind == Kind::Assistant)) {
        open.push(format!("- The last error had no reply after it (message {}): {}", error.id, compact(&error.head, 200)));
    }
    let mut pending = Vec::new();
    for item in all.iter().rev().filter(|item| item.kind == Kind::Assistant).take(3) {
        let text = if item.tail.is_empty() { &item.head } else { &item.tail };
        for sentence in sentences(text).into_iter().filter(|sentence| has(sentence, &PENDING)).take(2) {
            pending.push(format!("- Said in message {}: {}", item.id, compact(sentence, 200)));
        }
    }
    pending.reverse();
    open.extend(pending);
    if let Some(last) = all.iter().rev().find(|item| item.kind == Kind::User) {
        open.push(format!("- The user's last message before this handoff was message {}: {}", last.id, compact(&last.head, 200)));
    }
    for (order, text) in open.into_iter().take(12).enumerate() {
        lines.push(Line { section: OPEN, priority: 90, order, ids: Vec::new(), text });
    }
    for line in &mut lines {
        // A line is one row of the summary, however long a path or an id list is.
        if line.ids.len() <= 1 && line.text.len() > 1_200 {
            line.text.truncate(floor(&line.text, 1_197));
            line.text.push('…');
        }
    }
    lines
}

/// `kept` in section and thread order.
pub(super) fn render(kept: &[&Line], range: (i64, i64)) -> String {
    // Only what is open says nothing about older turns on its own.
    if kept.iter().all(|line| line.ids.is_empty()) {
        return String::new();
    }
    let mut out = format!("## Summary of earlier work (#{}–#{}; each line names the messages it stands for)\n", range.0, range.1);
    let mut section = u8::MAX;
    for line in kept {
        if line.section != section {
            section = line.section;
            out.push_str(&format!("### {}\n", SECTIONS[section as usize]));
        }
        out.push_str(&line.text);
        out.push('\n');
    }
    out.push('\n');
    out
}
