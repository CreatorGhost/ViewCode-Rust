use super::budget::{budget, Budget};
use super::*;
use std::collections::BTreeSet;
use std::path::Path;

const REQUEST: &str = "Continue with the ranking work.";
const FILE: &str = "/data/handoffs/thread/history.md";

fn parties() -> Parties {
    Parties { from: Some(("Claude Code".into(), "sonnet".into())), to: ("Codex".into(), "gpt-test".into()), lead: None, delta: false }
}

fn built(items: &[Item], budget: &Budget) -> Built {
    build(&Request { items, budget, parties: &parties(), archive: Some(Path::new(FILE)), goal: None }).unwrap()
}

fn window(tokens: u64) -> Budget {
    budget(Some(tokens), 0, REQUEST.len(), 0)
}

/// What the prompt itself shows for each message, read back from its text alone.
#[derive(Default, Debug)]
struct Shown {
    verbatim: BTreeSet<i64>,
    excerpts: BTreeSet<i64>,
    condensed: BTreeSet<i64>,
    summarised: BTreeSet<i64>,
    referenced: BTreeSet<i64>,
    /// Ids in the order their blocks appear among the recent turns.
    recent: Vec<i64>,
}

fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let Some(start) = text.find(heading) else { return "" };
    let rest = &text[start + heading.len()..];
    // The next section begins at a line that starts with `## `.
    rest.find("\n## ").map_or(rest, |end| &rest[..end])
}

fn numbers(text: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('#') {
        rest = &rest[at + 1..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            out.push(digits.parse().unwrap());
        }
    }
    out
}

fn shown(text: &str, items: &[Item]) -> Shown {
    let mut shown = Shown::default();
    let recent = section(text, "## Recent conversation");
    for line in recent.lines().filter(|line| line.starts_with("[#")) {
        let id = numbers(line)[0];
        shown.recent.push(id);
        let item = items.iter().find(|item| item.id == id).unwrap();
        if line.starts_with(&format!("[#{id} tool ")) || line.starts_with(&format!("[#{id} notice ")) {
            assert!(shown.condensed.insert(id));
        } else if line.contains(" · excerpt: ") {
            assert!(line.contains("full text: history file line "), "{line}");
            assert!(recent.contains(&format!("\n[/#{id}]\n")));
            assert!(shown.excerpts.insert(id));
        } else {
            // In full means exactly the saved text, between its markers.
            let block = format!("{line}\n{}\n{}", item.head, if item.attachments.is_empty() { format!("[/#{id}]\n") } else { "(attached image ".into() });
            assert!(item.tail.is_empty() && recent.contains(&block) && recent.contains(&format!("\n[/#{id}]\n")), "message {id} is not whole");
            assert!(shown.verbatim.insert(id));
        }
    }
    shown.summarised = numbers(section(text, "## Summary of earlier work")).into_iter().skip(2).collect();
    let pointers = section(text, "## By reference only");
    for run in pointers.lines().nth(1).unwrap_or("").split(", ").filter(|run| !run.is_empty()) {
        let ends = numbers(run);
        let (first, last) = (ends[0], *ends.last().unwrap());
        for item in items.iter().filter(|item| (first..=last).contains(&item.id)) {
            assert!(shown.referenced.insert(item.id), "message {} is pointed at twice", item.id);
        }
    }
    shown
}

/// Every message is in exactly one of: in full, represented in the summary (or condensed as
/// a tool step), or pointed at in the history file. Checked against the prompt's own text and
/// against what the builder reports.
fn assert_accounted(built: &Built, items: &[Item]) -> Shown {
    let shown = shown(&built.text, items);
    assert_eq!(built.dispositions.len(), items.len());
    for (item, (id, disposition)) in items.iter().zip(&built.dispositions) {
        assert_eq!(item.id, *id);
        let ways = [shown.verbatim.contains(id), shown.summarised.contains(id) || shown.condensed.contains(id), shown.referenced.contains(id) || shown.excerpts.contains(id)];
        assert_eq!(ways.iter().filter(|way| **way).count(), 1, "message {id} ({:?}) is carried {ways:?}", item.kind);
        assert!(!(shown.summarised.contains(id) && shown.condensed.contains(id)) && !(shown.referenced.contains(id) && shown.excerpts.contains(id)), "message {id} appears twice");
        let expected = match disposition {
            Disposition::Verbatim => 0,
            Disposition::Summarised => 1,
            Disposition::Referenced { .. } => 2,
        };
        assert!(ways[expected], "message {id}: the builder says {disposition:?}, the prompt shows {ways:?}");
        assert_eq!(matches!(disposition, Disposition::Referenced { excerpt: true }), shown.excerpts.contains(id));
    }
    let counted = (shown.verbatim.len(), shown.summarised.len() + shown.condensed.len(), shown.referenced.len() + shown.excerpts.len(), shown.excerpts.len());
    assert_eq!(counted, (built.stats.verbatim, built.stats.summarised, built.stats.referenced, built.stats.excerpts));
    assert_eq!(counted.0 + counted.1 + counted.2, items.len());
    shown
}

#[test]
fn the_budget_follows_the_target_window() {
    // (window, native tokens, images) → allowance, summary, recent, references, in bytes.
    let request = "x".repeat(300); // 100 tokens
    for (tokens, native, images, expected) in [
        // Unknown: 128,000 assumed; reserve 32,000; a quarter of the window, 32,000 tokens.
        (None, 0, 0, (96_000, 28_800, 57_600, 9_600)),
        (Some(200_000), 0, 0, (150_000, 45_000, 90_000, 15_000)),
        // A million-token window stops at the 256 KiB ceiling.
        (Some(1_000_000), 0, 0, (262_144, 78_643, 157_287, 26_214)),
        (Some(32_000), 0, 0, (24_000, 7_200, 14_400, 2_400)),
        // 20,000 − 16,000 reserve − 100 request = 3,900 tokens.
        (Some(20_000), 0, 0, (11_700, 3_510, 7_020, 1_170)),
        // Nothing is left once the reserve is taken.
        (Some(16_000), 0, 0, (0, 0, 0, 0)),
        // Two images reserve 16,384 tokens: 32,000 − 16,000 − 100 − 16,384 would be negative.
        (Some(32_000), 0, 2, (0, 0, 0, 0)),
        // 64,000 − 16,000 − 100 − 16,384 = 31,516 free; the quarter, 16,000 tokens, is less.
        (Some(64_000), 0, 2, (48_000, 14_400, 28_800, 4_800)),
        // A resumed session that already holds 140,000 tokens leaves 9,900.
        (Some(200_000), 140_000, 0, (29_700, 8_910, 17_820, 2_970)),
    ] {
        let budget = budget(tokens, native, request.len(), images);
        assert_eq!((budget.allowance_bytes, budget.summary_bytes, budget.verbatim_bytes, budget.reference_bytes), expected, "{tokens:?} {native} {images}");
        assert_eq!(budget.summary_bytes + budget.verbatim_bytes + budget.reference_bytes, budget.allowance_bytes);
        assert_eq!(budget.window_known, tokens.is_some());
    }
    assert_eq!(budget(None, 0, 0, 0).window, budget::DEFAULT_WINDOW);
    assert_eq!(budget(Some(200_000), 0, 0, 0).reserve_tokens, 50_000);
    assert_eq!(budget(Some(40_000), 0, 0, 0).reserve_tokens, 16_000);
    // A long message is one over an eighth of the allowance, between 2,000 and 24,000 bytes.
    assert_eq!((window(20_000).long_message(), window(32_000).long_message(), window(200_000).long_message(), window(1_000_000).long_message()), (2_000, 3_000, 18_750, 24_000));
}

#[test]
fn every_message_is_accounted_for_exactly_once_at_every_window() {
    let items = fixture::items();
    assert!(items.len() >= 300, "{}", items.len());
    for tokens in [18_000, 20_000, 32_000, 64_000, 128_000, 200_000, 400_000, 1_000_000] {
        let budget = window(tokens);
        let built = built(&items, &budget);
        assert!(built.text.len() <= budget.allowance_bytes, "{tokens}: {} > {}", built.text.len(), budget.allowance_bytes);
        assert_eq!(built.stats.bytes, built.text.len());
        assert_accounted(&built, &items);
    }
}

#[test]
fn recent_turns_are_whole_and_in_order_and_older_ones_are_summarised() {
    let items = fixture::items();
    let small = built(&items, &window(32_000));
    let shown = assert_accounted(&small, &items);
    // Recent turns are the end of the thread, contiguous and in thread order.
    let tail: Vec<i64> = items[items.len() - shown.recent.len()..].iter().map(|item| item.id).collect();
    assert_eq!(shown.recent, tail);
    // The last three user turns are there, the newest request and its answer in full.
    let users: Vec<i64> = items.iter().filter(|item| item.kind == Kind::User).map(|item| item.id).collect();
    assert!(users[users.len() - 3..].iter().all(|id| shown.recent.contains(id)));
    assert!(shown.verbatim.contains(users.last().unwrap()));
    assert!(shown.verbatim.contains(&items.last().unwrap().id));
    // Sections come in reading order, and the first request leads the summary.
    let text = &small.text;
    let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle} is missing"));
    assert!(at("PandaCode handoff.") < at("## Standing instructions") && at("## Standing instructions") < at("## Summary of earlier work"));
    assert!(at("## Summary of earlier work") < at("## By reference only") && at("## By reference only") < at("## Recent conversation"));
    assert!(section(text, "### Goals and requests").trim_start_matches(|c| c != '\n').trim_start().starts_with("- (#1) Build an offline search index"));
    // A larger window carries more turns whole and summarises fewer.
    let large = built(&items, &window(200_000));
    assert!(large.stats.verbatim > small.stats.verbatim * 3, "{} vs {}", large.stats.verbatim, small.stats.verbatim);
    assert!(large.stats.referenced < small.stats.referenced);
    // The same thread and window always give the same prompt.
    assert_eq!(built(&items, &window(32_000)).text, small.text);
}

#[test]
fn the_summary_names_goals_decisions_files_commands_errors_and_what_is_open() {
    let items = fixture::items();
    let built = built(&items, &window(200_000));
    let summary = section(&built.text, "## Summary of earlier work");
    for heading in ["### Goals and requests", "### Decisions and reasons", "### Files created or changed", "### Commands run and outcomes", "### Errors and how they were resolved", "### Still open and next step"] {
        assert!(summary.contains(heading), "{heading}");
    }
    // Goals: the user's own requests, each naming its message.
    assert!(summary.contains("- (#1) Build an offline search index for the notes app."));
    assert!(summary.contains("Step 7: handle case 7 in the indexer and keep the public API unchanged."));
    // Decisions with their reason, taken from the replies.
    assert!(summary.contains("I chose a trie for prefixes rather than scanning, because lookups for case 7 must stay under a millisecond."));
    assert!(summary.contains("I decided to store postings as sorted arrays instead of hash sets because merges stay linear."));
    // Files with paths and how they were written.
    assert!(summary.contains("- src/search/part_3.ts — Edit ×"));
    // Commands and how they ended.
    assert!(summary.contains("`npm test` → completed; 42 passed"));
    // Errors, with what was done about them.
    assert!(summary.contains("`npm test` failed: 1 failed, 41 passed → then (message"));
    assert!(summary.contains("The test run for step 9 failed: case 9 expected 3 hits, got 2. → then (message") && summary.contains("The root cause was an off-by-one in the window for case 9."));
    // What is open: the last stated next step and the user's last message.
    let open = section(&built.text, "### Still open and next step");
    assert!(open.contains("Next step: add a test for three-way ties, which is still pending."));
    assert!(open.contains("Now make the ranking stable when two notes tie."));
    // Standing instructions are the user's exact words, at every window.
    for tokens in [18_000, 32_000, 1_000_000] {
        let text = built_text(&items, tokens);
        let standing = section(&text, "## Standing instructions from the user");
        for instruction in fixture::STANDING {
            assert!(standing.contains(&format!("“{instruction}”")), "{tokens}: {instruction}");
        }
    }
    // The opening says what this is, from whom, to whom, and where the rest can be read.
    let text = &built.text;
    assert!(text.starts_with("PandaCode handoff. This thread was written with Claude Code (model sonnet) and continues with you, Codex (model gpt-test), from this message on."));
    assert!(text.contains(&format!("Full history: {FILE} holds every message whole")));
    assert!(text.contains("built by PandaCode from the saved transcript and tool records, not by a model"));
    assert!(text.contains("Budget: context window 200,000 tokens; 150,000 bytes for this handoff (summary up to 45,000, recent turns at least 90,000, references 15,000)."));
}

fn built_text(items: &[Item], tokens: u64) -> String {
    built(items, &window(tokens)).text
}

#[test]
fn a_very_long_message_is_a_marked_excerpt_with_a_pointer() {
    let items = fixture::items();
    let paste = items.iter().find(|item| item.head.starts_with("Here is the failing log from CI")).unwrap();
    assert!(paste.bytes > fixture::LONG_PASTE_BYTES);
    for tokens in [32_000, 200_000] {
        let budget = window(tokens);
        let built = built(&items, &budget);
        let shown = assert_accounted(&built, &items);
        assert!(shown.excerpts.contains(&paste.id));
        let start = built.text.find(&format!("[#{} user · excerpt: ", paste.id)).unwrap();
        let block = &built.text[start..built.text[start..].find(&format!("[/#{}]", paste.id)).unwrap() + start];
        let size = budget.long_message() / 2;
        // Marked as an excerpt, with its sizes and where the whole text is.
        assert!(block.lines().next().unwrap().ends_with(&format!("of {} bytes; full text: history file line {}]", group(paste.bytes), paste.line.unwrap())), "{}", block.lines().next().unwrap());
        // The start and the end are there; the middle is said to be left out.
        assert!(block.contains("Here is the failing log from CI, look at the end.") && block.contains("TAIL-OF-PASTE: the last line says exit 3."));
        assert!(block.contains(" bytes left out here …]"));
        assert!(block.len() <= size + 400, "{} > {}", block.len(), size + 400);
    }
    // An old long reply is not excerpted: its decision is in the summary, the rest by pointer.
    let built = built(&items, &window(200_000));
    assert!(!built.text.contains("END-OF-LONG-REPLY"));
}

fn turn(items: &mut Vec<Item>, bytes: usize) {
    let id = items.len() as i64 + 1;
    items.push(Item::text(Kind::User, id, &format!("Request {id}. {}", "u".repeat(bytes))));
    items.push(Item::text(Kind::Assistant, id + 1, &format!("Answer {}. {}", id + 1, "a".repeat(bytes))));
}

#[test]
fn the_summary_shrinks_before_a_recent_turn_is_given_up() {
    // Forty small older turns, then three recent turns of 2,900-byte messages: 17.6 KB of
    // recent turns against a 24,000-byte allowance whose recent share is 14,400.
    let mut items = Vec::new();
    (0..40).for_each(|_| turn(&mut items, 60));
    (0..3).for_each(|_| turn(&mut items, 2_900));
    let budget = window(32_000);
    let built = built(&items, &budget);
    let shown = assert_accounted(&built, &items);
    let recent: Vec<i64> = items[80..].iter().map(|item| item.id).collect();
    assert!(recent.iter().all(|id| shown.verbatim.contains(id)), "all three recent turns stay whole");
    assert!(built.stats.verbatim_bytes > budget.verbatim_bytes, "recent turns took more than their share");
    // The summary gave way: under its cap, with older messages left to the history file.
    assert!(built.stats.summary_bytes < budget.summary_bytes && built.stats.referenced > 0, "{:?}", built.stats);
    assert!(built.stats.summarised > 0, "what still fits of the summary is kept");
    // Tighter still, with no summary left to give: the oldest of the recent turns goes to the
    // history file, the newest request stays whole.
    let mut items = Vec::new();
    (0..40).for_each(|_| turn(&mut items, 60));
    (0..3).for_each(|_| turn(&mut items, 1_900));
    let tight = Budget { allowance_bytes: 8_000, summary_bytes: 2_400, verbatim_bytes: 4_800, reference_bytes: 800, ..budget.clone() };
    let built = self::built(&items, &tight);
    let shown = assert_accounted(&built, &items);
    assert!(shown.verbatim.contains(&85) && shown.verbatim.contains(&86) && shown.referenced.contains(&81), "{:?}", built.stats);
    assert!(built.text.len() <= tight.allowance_bytes);
}

#[test]
fn a_window_too_small_for_the_minimum_is_an_error() {
    let items = fixture::items();
    let request = Request { items: &items, budget: &window(16_000), parties: &parties(), archive: Some(Path::new(FILE)), goal: None };
    let error = build(&request).unwrap_err();
    assert_eq!(
        error,
        "This conversation cannot be handed over to Codex (gpt-test): its context window (16,000 tokens) leaves no room for the earlier conversation after your message and the agent's own instructions. Choose a model with a larger context window or start a new thread. Nothing was sent and your message was not shortened."
    );
    // A request that fills the window by itself is refused too, never shortened.
    let huge = budget(Some(64_000), 0, 150_000, 0);
    assert!(build(&Request { budget: &huge, ..request }).unwrap_err().starts_with(CANNOT_FIT));
    // Room for the opening but not for the newest user message: still an error.
    let mut items = Vec::new();
    turn(&mut items, 1_900);
    let cramped = Budget { allowance_bytes: 4_096, summary_bytes: 1_228, verbatim_bytes: 2_459, reference_bytes: 409, ..window(32_000) };
    assert!(build(&Request { items: &items, budget: &cramped, parties: &parties(), archive: None, goal: None }).unwrap_err().starts_with(CANNOT_FIT));
    // An unknown window says it was assumed.
    let unknown = Budget { allowance_bytes: 0, ..budget(None, 0, 0, 0) };
    assert!(build(&Request { items: &items, budget: &unknown, parties: &parties(), archive: None, goal: None }).unwrap_err().contains("(128,000 tokens, assumed)"));
}

#[test]
fn a_short_thread_goes_whole_and_says_so() {
    let mut items = Vec::new();
    turn(&mut items, 200);
    let mut tool = Item::text(Kind::Tool, 3, "");
    tool.tool = Some(Tool { name: "Bash".into(), status: "completed".into(), input: "{\"command\":\"ls\"}".into(), output: "a\nb\n".into(), write_paths: Vec::new() });
    items.push(tool);
    items.push(Item { attachments: vec![Attachment { name: "wireframe.png".into(), path: Some("/data/attachments/thread/one.png".into()) }, Attachment { name: "board".into(), path: None }], ..Item::text(Kind::User, 4, "Match this sketch") });
    let budget = window(200_000);
    let built = build(&Request { items: &items, budget: &budget, parties: &parties(), archive: None, goal: Some(("Ship the search index", "paused")) }).unwrap();
    let shown = shown(&built.text, &items);
    assert_eq!((shown.verbatim.len(), shown.condensed.len(), built.stats.referenced), (3, 1, 0));
    assert!(!built.stats.shortened(), "a tool step shown as one line is not a shortened history");
    assert!(!built.text.contains("## Summary of earlier work") && !built.text.contains("## By reference only"));
    assert!(built.text.contains("[#3 tool Bash · completed] {\"command\":\"ls\"} → a b\n"));
    // Images: not carried, noted on their message with the saved copy's path.
    assert!(built.text.contains("(attached image “wireframe.png”: contents not carried; saved copy: /data/attachments/thread/one.png)"));
    assert!(built.text.contains("(attached image “board”: contents not carried; no saved copy)"));
    assert!(built.text.contains("2 earlier image attachments are not carried either"));
    // Without a history file the agent is told so instead of being pointed at one.
    assert!(built.text.contains("The full history file could not be written"));
    assert!(built.text.contains("Thread goal (paused): Ship the search index"));
    // The thread's notice says what happened, in either case.
    assert_eq!(
        notice(&parties(), &budget, &built.stats, false),
        format!("The whole earlier conversation went to Codex: 3 messages in full and 1 tool steps as one line each ({} bytes; context window 200,000 tokens). The history file could not be written, so the agent was told what it cannot see. Images are not transferred; saved copies are referenced by path.", group(built.stats.bytes))
    );
    let long = self::built(&fixture::items(), &window(32_000));
    let said = notice(&parties(), &window(32_000), &long.stats, true);
    assert!(said.starts_with("History was summarised for Codex (context window 32,000 tokens): ") && said.contains("Nothing was dropped.") && said.len() < 1_000, "{said}");
}

#[test]
fn a_resumed_session_is_told_only_what_it_missed() {
    let mut items = Vec::new();
    turn(&mut items, 50);
    let parties = Parties { delta: true, ..parties() };
    let built = build(&Request { items: &items, budget: &window(200_000), parties: &parties, archive: None, goal: None }).unwrap();
    assert!(built.text.starts_with("PandaCode handoff. This thread continued with Claude Code (model sonnet) while your own earlier session was set aside; you, Codex (model gpt-test), take it back from this message on. Your session already holds the conversation up to that point; below is only what happened since."));
    // A context replaced within one agent names no other author.
    let same = Parties { from: None, ..self::parties() };
    let built = build(&Request { items: &items, budget: &window(200_000), parties: &same, archive: None, goal: None }).unwrap();
    assert!(built.text.starts_with("PandaCode handoff. This thread's agent context was replaced; you, Codex (model gpt-test), continue it"));
}

#[test]
fn text_helpers_cut_on_characters() {
    assert_eq!(compact("  one\n two\tthree ", 100), "one two three");
    assert_eq!(compact("竹竹竹竹竹竹", 8), "竹…");
    let long = "竹".repeat(20_000);
    let (head, tail) = clip(&long);
    assert!(head.len() <= KEEP_HEAD && tail.len() >= KEEP_TAIL && tail.len() < KEEP_TAIL + 4);
    assert_eq!(group(1_234_567), "1,234,567");
    assert_eq!(group(999), "999");
}
