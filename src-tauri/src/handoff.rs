//! What a new agent is told about a thread it did not write (docs/handoff.md).
//!
//! A thread whose native context is replaced (another harness, another account, a fork, a team
//! reset) hands its saved conversation to the next agent in its first prompt. Nothing is dropped
//! silently: every saved message is carried in full, represented in a structured summary, or
//! pointed at in the thread's history file, which holds every message whole. The summary is
//! built here from the stored transcript and tool records, with no model call.
//!
//! The size comes from the target model's context window (`budget`). Recent turns stay in full;
//! older work becomes the summary, which shrinks before any recent turn is given up; a single
//! very long message becomes a marked excerpt plus a pointer. When even the minimum cannot fit,
//! `build` fails and the caller starts nothing.
pub(crate) mod archive;
pub(crate) mod budget;
pub(crate) mod resume;
mod summary;

use budget::Budget;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Text kept in memory per message: the builder never needs more, the history file has it all.
pub(crate) const KEEP_HEAD: usize = 32 * 1024;
pub(crate) const KEEP_TAIL: usize = 8 * 1024;
/// The last user turns that stay in full while the summary still has anything to give up.
pub(crate) const RECENT_TURNS: usize = 3;
/// One condensed tool step: what it did and its result.
const TOOL_INPUT: usize = 200;
const TOOL_RESULT: usize = 200;
/// The thread's notice at a switch, until the first message replaces it with what was carried.
pub(crate) const SWITCHED: &str = "Switched agent. The next message hands this conversation to the new agent within its model's context window: recent turns in full, older work as a structured summary, and everything else by reference to this thread's history file, so nothing is dropped. Files and project stay in place. The earlier agent's private reasoning and native tool state are not transferred; images are referenced by their saved copies.";
pub(crate) const SWITCHED_BACK: &str = "Switched back. This agent resumes its own earlier session for this thread, so the next message hands over only what happened since. Files and project stay in place.";
/// How the error starts, so callers and tests can tell it from any other refusal.
pub(crate) const CANNOT_FIT: &str = "This conversation cannot be handed over";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    User,
    Assistant,
    /// A team turn PandaCode wrote on the user's side.
    Coordination,
    Tool,
    Error,
    /// Status lines, change summaries, switch notices: the app's own rows.
    Notice,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Tool {
    pub name: String,
    pub status: String,
    pub input: String,
    /// The end of the output, where a command's result is.
    pub output: String,
    pub write_paths: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Attachment {
    pub name: String,
    /// The saved copy, when the image was a file of this thread.
    pub path: Option<PathBuf>,
}

/// One saved message, as much of it as the builder needs.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub id: i64,
    pub kind: Kind,
    /// The stored role, for notices.
    pub role: String,
    /// The first `KEEP_HEAD` bytes.
    pub head: String,
    /// The last `KEEP_TAIL` bytes when the message is longer than `head`.
    pub tail: String,
    /// The whole message's length.
    pub bytes: usize,
    pub tool: Option<Tool>,
    pub attachments: Vec<Attachment>,
    /// Files the turn this user message started changed (`turn_changes`).
    pub changed: Vec<String>,
    /// Its heading's line in the history file.
    pub line: Option<usize>,
}

impl Item {
    pub(crate) fn text(kind: Kind, id: i64, text: &str) -> Self {
        let (head, tail) = clip(text);
        Self { id, kind, role: label(kind).into(), head, tail, bytes: text.len(), tool: None, attachments: Vec::new(), changed: Vec::new(), line: None }
    }
    fn whole(&self) -> bool {
        self.tail.is_empty() && self.head.len() == self.bytes
    }
}

/// Splits a message into what is kept in memory.
pub(crate) fn clip(text: &str) -> (String, String) {
    if text.len() <= KEEP_HEAD {
        return (text.to_owned(), String::new());
    }
    (text[..floor(text, KEEP_HEAD)].to_owned(), text[ceil(text, text.len() - KEEP_TAIL)..].to_owned())
}

pub(crate) fn floor(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

pub(crate) fn ceil(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// One line of at most `max` bytes: whitespace collapsed, cut on a character with an ellipsis.
pub(crate) fn compact(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(max.min(text.len()));
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.len() > max {
            break;
        }
    }
    if out.len() > max {
        out.truncate(floor(&out, max.saturating_sub(3)));
        out.push('…');
    }
    out
}

fn label(kind: Kind) -> &'static str {
    match kind {
        Kind::User => "user",
        Kind::Assistant => "assistant",
        Kind::Coordination => "user (team coordination)",
        Kind::Tool => "tool",
        Kind::Error => "error",
        Kind::Notice => "notice",
    }
}

/// How one message reached the new agent. Every message has exactly one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// Its full text is in the prompt.
    Verbatim,
    /// A line of the summary, or a condensed tool step, stands for it.
    Summarised,
    /// Only a pointer into the history file (with a marked excerpt for a long recent message).
    Referenced { excerpt: bool },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Stats {
    pub messages: usize,
    pub verbatim: usize,
    pub summarised: usize,
    pub referenced: usize,
    pub excerpts: usize,
    /// Tool steps and notices shown as one line each among the recent turns.
    pub tool_lines: usize,
    pub images: usize,
    pub summary_bytes: usize,
    pub verbatim_bytes: usize,
    pub reference_bytes: usize,
    /// Everything before `Current request:`.
    pub bytes: usize,
}

impl Stats {
    /// Whether any of the conversation itself was summarised, excerpted or left to the
    /// history file, so the thread says so. A tool step shown as one line does not count.
    pub(crate) fn shortened(&self) -> bool {
        self.summarised - self.tool_lines + self.referenced > 0
    }
}

/// Who hands over to whom.
#[derive(Clone, Debug, Default)]
pub(crate) struct Parties {
    /// Display name and model of the agent that wrote the history, when one switch explains it.
    pub from: Option<(String, String)>,
    pub to: (String, String),
    /// Replaces the opening sentence (a side chat's parent snapshot).
    pub lead: Option<String>,
    /// The target resumed its own earlier session; only what it has not seen is handed over.
    pub delta: bool,
}

pub(crate) struct Request<'a> {
    pub items: &'a [Item],
    pub budget: &'a Budget,
    pub parties: &'a Parties,
    /// The history file, when it could be written.
    pub archive: Option<&'a std::path::Path>,
    /// The thread's goal, carried as a line.
    pub goal: Option<(&'a str, &'a str)>,
}

#[derive(Debug)]
pub(crate) struct Built {
    /// The context that goes before `Current request:`.
    pub text: String,
    /// One per message, in order: the accounting the tests check against the text.
    #[cfg_attr(not(test), allow(dead_code))]
    pub dispositions: Vec<(i64, Disposition)>,
    pub stats: Stats,
}

pub(crate) fn cannot_fit(parties: &Parties, budget: &Budget) -> String {
    format!(
        "{CANNOT_FIT} to {} ({}): its context window ({} tokens{}) leaves no room for the earlier conversation after your message and the agent's own instructions. Choose a model with a larger context window or start a new thread. Nothing was sent and your message was not shortened.",
        parties.to.0,
        if parties.to.1.is_empty() { "default model" } else { &parties.to.1 },
        group(budget.window as usize),
        if budget.window_known { "" } else { ", assumed" },
    )
}

/// `12,345`.
pub(crate) fn group(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A message as it appears among the recent turns, and what that makes of it.
fn render(item: &Item, long: usize, archive: bool) -> (String, Disposition) {
    let pointer = |item: &Item| match (archive, item.line) {
        (true, Some(line)) => format!("full text: history file line {line}"),
        _ => "full text is not available to you".to_owned(),
    };
    match item.kind {
        Kind::Tool => {
            let tool = item.tool.clone().unwrap_or_default();
            let result = if tool.output.trim().is_empty() { "no output".to_owned() } else { compact(last_lines(&tool.output), TOOL_RESULT) };
            let status = if tool.status.is_empty() { String::new() } else { format!(" · {}", compact(&tool.status, 24)) };
            (format!("[#{} tool {}{status}] {} → {result}\n", item.id, compact(&tool.name, 48), compact(&tool.input, TOOL_INPUT)), Disposition::Summarised)
        }
        Kind::Notice => (format!("[#{} notice {}] {}\n", item.id, compact(&item.role, 24), compact(&item.head, 160)), Disposition::Summarised),
        _ => {
            let mut attached = String::new();
            for attachment in &item.attachments {
                match &attachment.path {
                    Some(path) => attached.push_str(&format!("(attached image “{}”: contents not carried; saved copy: {})\n", compact(&attachment.name, 80), path.display())),
                    None => attached.push_str(&format!("(attached image “{}”: contents not carried; no saved copy)\n", compact(&attachment.name, 80))),
                }
            }
            let name = label(item.kind);
            if item.whole() && item.bytes <= long {
                return (format!("[#{} {name}]\n{}\n{attached}[/#{}]\n", item.id, item.head, item.id), Disposition::Verbatim);
            }
            // A marked excerpt: two thirds from the start, one third from the end.
            let size = long / 2;
            let head = &item.head[..floor(&item.head, size * 2 / 3)];
            let end = if item.tail.is_empty() { &item.head } else { &item.tail };
            let tail = &end[ceil(end, end.len().saturating_sub(size / 3))..];
            let omitted = item.bytes.saturating_sub(head.len() + tail.len());
            (
                format!(
                    "[#{} {name} · excerpt: the first {} and last {} of {} bytes; {}]\n{head}\n[… {} bytes left out here …]\n{tail}\n{attached}[/#{}]\n",
                    item.id,
                    group(head.len()),
                    group(tail.len()),
                    group(item.bytes),
                    pointer(item),
                    group(omitted),
                    item.id
                ),
                Disposition::Referenced { excerpt: true },
            )
        }
    }
}

/// The last few non-empty lines, where a command says how it went.
fn last_lines(output: &str) -> &str {
    let trimmed = output.trim_end();
    let mut start = trimmed.len();
    for _ in 0..3 {
        match trimmed[..start].rfind('\n') {
            Some(at) => start = at,
            None => return trimmed,
        }
    }
    &trimmed[start..]
}

/// Runs of consecutive referenced messages, each with where its text starts in the history file.
fn runs(items: &[Item], referenced: &[bool]) -> String {
    let mut out = String::new();
    let mut index = 0;
    while index < items.len() {
        if !referenced[index] {
            index += 1;
            continue;
        }
        let start = index;
        while index + 1 < items.len() && referenced[index + 1] {
            index += 1;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(&format!("#{}", items[start].id));
        if index > start {
            out.push_str(&format!("–#{}", items[index].id));
        }
        if let Some(line) = items[start].line {
            out.push_str(&format!(" (line {line})"));
        }
        index += 1;
    }
    out
}

fn references(items: &[Item], referenced: &[bool], archive: bool) -> String {
    let count = referenced.iter().filter(|r| **r).count();
    if count == 0 {
        return String::new();
    }
    let how = if archive {
        "in the history file only, in full; open it when you need them"
    } else {
        "not available to you: the history file could not be written, so ask the user about them"
    };
    format!("## By reference only ({count} messages, {how})\n{}\n\n", runs(items, referenced))
}

struct Fitted {
    summary: String,
    references: String,
    /// Per older message: no summary line stands for it.
    referenced: Vec<bool>,
}

/// The most summary lines, by priority, that fit `limit` together with the pointers to what
/// they leave out. With no line at all the pointers are one run, which always fits.
fn fit(older: &[Item], lines: &[summary::Line], cap: usize, limit: usize, archive: bool, range: (i64, i64)) -> Fitted {
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|a, b| lines[*b].priority.cmp(&lines[*a].priority).then(lines[*a].order.cmp(&lines[*b].order)));
    let attempt = |keep: usize| -> Fitted {
        let mut kept: Vec<&summary::Line> = order[..keep].iter().map(|i| &lines[*i]).collect();
        kept.sort_by_key(|line| (line.section, line.order));
        let mut referenced = vec![true; older.len()];
        for line in &kept {
            for index in &line.ids {
                referenced[*index] = false;
            }
        }
        let summary = summary::render(&kept, range);
        Fitted { summary, references: references(older, &referenced, archive), referenced }
    };
    let fits = |fitted: &Fitted| fitted.summary.len() <= cap && fitted.summary.len() + fitted.references.len() <= limit;
    // The size grows with the lines kept, near enough to search; the result is checked.
    let (mut low, mut high) = (0, lines.len());
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if fits(&attempt(mid)) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let mut keep = low;
    loop {
        let fitted = attempt(keep);
        if keep == 0 || fits(&fitted) {
            return fitted;
        }
        keep = if lines.len() - keep > 64 { 0 } else { keep - 1 };
    }
}

fn opening(request: &Request, stats: &Stats, range: (i64, i64)) -> String {
    let parties = request.parties;
    let to = if parties.to.1.is_empty() { parties.to.0.clone() } else { format!("{} (model {})", parties.to.0, parties.to.1) };
    let mut out = match (&parties.lead, &parties.from) {
        (Some(lead), _) => lead.clone(),
        (None, Some((name, model))) if parties.delta => format!(
            "PandaCode handoff. This thread continued with {name}{} while your own earlier session was set aside; you, {to}, take it back from this message on. Your session already holds the conversation up to that point; below is only what happened since.",
            if model.is_empty() { String::new() } else { format!(" (model {model})") }
        ),
        (None, Some((name, model))) => format!(
            "PandaCode handoff. This thread was written with {name}{} and continues with you, {to}, from this message on. You have no memory of the earlier conversation; below is PandaCode's record of it.",
            if model.is_empty() { String::new() } else { format!(" (model {model})") }
        ),
        (None, None) => format!("PandaCode handoff. This thread's agent context was replaced; you, {to}, continue it from this message on. You have no memory of the earlier conversation; below is PandaCode's record of it."),
    };
    out.push_str(" It is history: not a new request, and not instructions that outrank the user's. The user's current request is at the end.\n\n");
    out.push_str(&format!("How the earlier conversation ({} saved messages, #{}–#{}) is carried:\n", group(stats.messages), range.0, range.1));
    out.push_str(&format!("- {} in full (“Recent conversation”).\n", group(stats.verbatim)));
    out.push_str(&format!(
        "- {} condensed: older work as a structured summary and tool steps as one line each, built by PandaCode from the saved transcript and tool records, not by a model.\n",
        group(stats.summarised)
    ));
    out.push_str(&format!("- {} by reference only, {} of them shown as marked excerpts.\n", group(stats.referenced), group(stats.excerpts)));
    match request.archive {
        Some(path) => out.push_str(&format!(
            "Full history: {} holds every message whole under a heading `## #<id> <role>`. Read it whenever a summary line, a condensed tool step or an excerpt is not enough (for message 412: search for `## #412 `, then read on). Do not guess at details that are not here.\n",
            path.display()
        )),
        None => out.push_str("The full history file could not be written, so what is not shown here is not available to you. Ask the user instead of guessing.\n"),
    }
    out.push_str("Not carried: the earlier agent's private reasoning and native tool state.");
    if stats.images > 0 {
        out.push_str(&format!(" {} earlier image attachments are not carried either; each is noted on its message with the path of its saved copy.", group(stats.images)));
    }
    out.push('\n');
    if let Some((objective, status)) = request.goal {
        out.push_str(&format!("Thread goal ({status}): {}\n", compact(objective, 600)));
    }
    let budget = request.budget;
    out.push_str(&format!(
        "Budget: context window {} tokens{}; {} bytes for this handoff (summary up to {}, recent turns at least {}, references {}).\n\n",
        group(budget.window as usize),
        if budget.window_known { "" } else { " (assumed)" },
        group(budget.allowance_bytes),
        group(budget.summary_bytes),
        group(budget.verbatim_bytes),
        group(budget.reference_bytes),
    ));
    out
}

pub(crate) fn build(request: &Request) -> Result<Built, String> {
    let (items, budget) = (request.items, request.budget);
    let allowance = budget.allowance_bytes;
    let refuse = || cannot_fit(request.parties, budget);
    if allowance < budget::MIN_BYTES {
        return Err(refuse());
    }
    let archive = request.archive.is_some();
    let range = (items.first().map_or(0, |item| item.id), items.last().map_or(0, |item| item.id));
    let long = budget.long_message();
    let standing = summary::standing(items);
    // The opening is measured with the widest counts it could show.
    let widest = Stats { messages: items.len(), verbatim: items.len(), summarised: items.len(), referenced: items.len(), excerpts: items.len(), images: items.len(), ..Stats::default() };
    let fixed = opening(request, &widest, range).len() + standing.len() + budget::REFERENCE_MIN;
    if fixed > allowance {
        return Err(refuse());
    }
    let room = allowance - fixed;
    let rendered: Vec<(String, Disposition)> = items.iter().map(|item| render(item, long, archive)).collect();
    let cost = |index: usize| rendered[index].0.len();
    let everything: usize = (0..items.len()).map(cost).sum();
    let users: Vec<usize> = (0..items.len()).filter(|i| matches!(items[*i].kind, Kind::User | Kind::Coordination)).collect();
    // Recent turns first: back to the start of the third-newest user turn, then as far as the
    // recent share reaches. The newest user message has to make it, or nothing is sent.
    let mut start = items.len();
    let mut used = 0;
    if everything <= room {
        start = 0;
        used = everything;
    } else {
        let core = users.iter().rev().nth(RECENT_TURNS - 1).copied().unwrap_or(0);
        while start > 0 {
            let next = cost(start - 1);
            if used + next > room || (start <= core && used + next > budget.verbatim_bytes) {
                break;
            }
            used += next;
            start -= 1;
        }
        if users.last().is_some_and(|last| start > *last) || (users.is_empty() && start == items.len() && !items.is_empty()) {
            return Err(refuse());
        }
    }
    let goal_line = request.goal.map(|(objective, status)| format!("The thread's goal is {status}: {}", compact(objective, 300)));
    let summarise = |start: usize, used: usize| {
        let older = &items[..start];
        let lines = summary::lines(older, items, goal_line.as_deref());
        // The pointers' own minimum was set aside before the room was measured.
        fit(older, &lines, budget.summary_bytes, room - used + budget::REFERENCE_MIN, archive, (range.0, older.last().map_or(range.0, |item| item.id)))
    };
    let mut fitted = summarise(start, used);
    // What the summary did not need goes back to recent turns, once.
    let mut left = (room - used + budget::REFERENCE_MIN).saturating_sub(fitted.summary.len() + fitted.references.len());
    let mut extended = false;
    while start > 0 && cost(start - 1) <= left {
        left -= cost(start - 1);
        used += cost(start - 1);
        start -= 1;
        extended = true;
    }
    if extended {
        fitted = summarise(start, used);
    }
    let mut dispositions = Vec::with_capacity(items.len());
    let mut stats = Stats { messages: items.len(), images: items.iter().map(|item| item.attachments.len()).sum(), ..Stats::default() };
    for (index, item) in items.iter().enumerate() {
        let disposition = if index >= start {
            rendered[index].1
        } else if fitted.referenced[index] {
            Disposition::Referenced { excerpt: false }
        } else {
            Disposition::Summarised
        };
        match disposition {
            Disposition::Verbatim => stats.verbatim += 1,
            Disposition::Summarised => {
                stats.summarised += 1;
                stats.tool_lines += usize::from(index >= start);
            }
            Disposition::Referenced { excerpt } => {
                stats.referenced += 1;
                stats.excerpts += usize::from(excerpt);
            }
        }
        dispositions.push((item.id, disposition));
    }
    let mut recent = String::new();
    if start < items.len() {
        recent.push_str(&format!("## Recent conversation (#{}–#{}, in order; tool steps condensed to one line)\n", items[start].id, range.1));
        (start..items.len()).for_each(|index| recent.push_str(&rendered[index].0));
        recent.push('\n');
    }
    stats.summary_bytes = fitted.summary.len();
    stats.reference_bytes = fitted.references.len();
    stats.verbatim_bytes = used;
    let mut text = opening(request, &stats, range);
    text.push_str(&standing);
    text.push_str(&fitted.summary);
    text.push_str(&fitted.references);
    text.push_str(&recent);
    stats.bytes = text.len();
    if text.len() > allowance {
        return Err(refuse());
    }
    Ok(Built { text, dispositions, stats })
}

/// Context windows the model catalog reported, by harness and model. Filled whenever a catalog
/// is listed; nothing is run to learn one.
fn windows() -> &'static Mutex<HashMap<(String, String), u64>> {
    static WINDOWS: OnceLock<Mutex<HashMap<(String, String), u64>>> = OnceLock::new();
    WINDOWS.get_or_init(Default::default)
}

pub(crate) fn note_window(provider: &str, model: &str, window: u64) {
    if let Ok(mut known) = windows().lock() {
        known.insert((provider.to_owned(), model.to_owned()), window);
    }
}

pub(crate) fn note_models(provider: &str, models: &[crate::providers::ModelInfo]) {
    for model in models {
        if let Some(window) = model.context_window {
            note_window(provider, &model.id, window);
        }
    }
}

/// The target model's window: the catalog's, else the one this model last reported in any
/// thread, else unknown (`budget::DEFAULT_WINDOW` applies).
pub(crate) fn window(store: &crate::storage::Store, provider: &str, model: &str) -> Option<u64> {
    let model = if model.is_empty() { "default" } else { model };
    if let Some(window) = windows().lock().ok().and_then(|known| known.get(&(provider.to_owned(), model.to_owned())).copied()) {
        return Some(window);
    }
    store
        .db
        .query_row(
            "SELECT context_window FROM sessions WHERE provider=?1 AND (model=?2 OR (?2='default' AND model='')) AND context_window>0 ORDER BY updated_at DESC LIMIT 1",
            rusqlite::params![provider, model],
            |row| row.get::<_, i64>(0),
        )
        .ok()
        .map(|window| window as u64)
}

/// The sentence the thread shows for a delivered handoff (the `handoff` row's summary).
pub(crate) fn notice(parties: &Parties, budget: &Budget, stats: &Stats, archive: bool) -> String {
    let window = format!("{} tokens{}", group(budget.window as usize), if budget.window_known { "" } else { ", assumed" });
    let file = if archive { " The agent can read every message in full in this thread's history file." } else { " The history file could not be written, so the agent was told what it cannot see." };
    let images = if stats.images > 0 { " Images are not transferred; saved copies are referenced by path." } else { "" };
    if !stats.shortened() {
        return format!(
            "The whole earlier conversation went to {}: {} messages in full and {} tool steps as one line each ({} bytes; context window {window}).{file}{images}",
            parties.to.0,
            group(stats.verbatim),
            group(stats.tool_lines),
            group(stats.bytes)
        );
    }
    format!(
        "History was summarised for {} (context window {window}): {} recent messages in full, {} condensed into a structured summary and tool lines ({} characters), {} by reference ({} as marked excerpts). Nothing was dropped.{file}{images}",
        parties.to.0,
        group(stats.verbatim),
        group(stats.summarised),
        group(stats.summary_bytes),
        group(stats.referenced),
        group(stats.excerpts),
    )
}

/// The messages a prompt carries among its recent turns, read back from its text:
/// `(id, role, text)`, an excerpt with its marker as the role.
#[cfg(test)]
pub(crate) fn carried(prompt: &str) -> Vec<(i64, String, String)> {
    let mut out = Vec::new();
    let Some(start) = prompt.find("## Recent conversation") else { return out };
    let mut rest = &prompt[start..];
    while let Some(at) = rest.find("\n[#") {
        rest = &rest[at + 1..];
        let Some((header, body)) = rest.split_once('\n') else { break };
        let Some((id, role)) = header.strip_prefix("[#").and_then(|inner| inner.strip_suffix(']')).and_then(|inner| inner.split_once(' ')) else { continue };
        let Ok(id) = id.parse::<i64>() else { continue };
        if let Some(end) = body.find(&format!("\n[/#{id}]\n")) {
            out.push((id, role.to_owned(), body[..end].to_owned()));
            rest = &body[end..];
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod fixture;
#[cfg(test)]
mod tests;
