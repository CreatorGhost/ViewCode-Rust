//! The review rules, checked against the cases ViewCode's own tests use
//! (`reviewFindings.test.ts`, `reviewSkill.test.ts`), then the store and the tools.

use super::findings::*;
use super::skill::{self, LearnedLesson, LearnedSkill, LessonSource, SkillEdit, SkillError, SkillStatus, SkillStore};
use super::{playbook, store, tools};
use crate::storage::Store;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// A scratch directory removed when the test ends.
struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("pandacode-review-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn memory() -> (Store, String, String) {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let project = store.add_project(&std::env::temp_dir()).unwrap();
    let session = store.create_session(&project.id, "claude", "sonnet", "full").unwrap();
    (store, project.id, session.id)
}

fn finding(value: Value) -> Finding {
    let mut base = json!({"severity": "important", "confidence": 92, "path": "src/run.ts", "title": "Shell injection", "detail": "It reaches a shell."});
    base.as_object_mut().unwrap().extend(value.as_object().unwrap().clone());
    serde_json::from_value(base).unwrap()
}

fn rating(set: &str, key: &str, rating: Value, project: &str, lesson: Option<&str>) -> Activity {
    Activity {
        id: format!("r-{set}-{key}-{rating}"),
        kind: RATING_KIND.into(),
        payload: json!({"findingsActivityId": set, "key": key, "rating": rating, "projectId": project,
            "finding": {"title": format!("{set} {key}"), "severity": "nit", "path": "src/a.ts", "lessonId": lesson}, "timelineBypass": true}),
    }
}

const PATCH: &str = "diff --git a/src/run.ts b/src/run.ts\nindex 1111111..2222222 100644\n--- a/src/run.ts\n+++ b/src/run.ts\n@@ -10,4 +10,4 @@ export function run() {\n   const a = 1;\n-  exec(branch);\n--- a removed line that looks like a header\n+  execFile('git', [branch]);\n+  log(a);\n   return a;\ndiff --git a/old/name.ts b/new/name.ts\nsimilarity index 90%\nrename from old/name.ts\nrename to new/name.ts\n--- a/old/name.ts\n+++ b/new/name.ts\n@@ -1 +1 @@\n-export const x = 1;\n+export const x = 2;\ndiff --git a/gone.ts b/gone.ts\ndeleted file mode 100644\n--- a/gone.ts\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\n";

#[test]
fn review_ratings_keep_the_latest_and_forget_one_taken_back() {
    let activities = [rating("a", "finding:0", json!("up"), "p", None), rating("a", "finding:1", json!("down"), "p", None), rating("b", "finding:0", json!("down"), "p", None), rating("a", "finding:0", json!("down"), "p", None), rating("a", "finding:1", Value::Null, "p", None)];
    let ratings = finding_ratings(&activities, "a");
    assert_eq!(ratings.into_iter().collect::<Vec<_>>(), vec![("finding:0".to_owned(), Rating::Down)]);
}

#[test]
fn review_recent_rated_findings_are_by_latest_rating_newest_last_and_bounded() {
    let activities = [
        rating("a", "finding:0", json!("up"), "p", None),
        rating("a", "finding:1", json!("down"), "p", None),
        rating("x", "finding:0", json!("up"), "other", None),
        rating("a", "finding:2", json!("up"), "p", None),
        rating("a", "finding:0", json!("down"), "p", None),
        rating("a", "finding:1", Value::Null, "p", None),
    ];
    let rated = recent_rated_findings(&activities, "p", 30);
    assert_eq!(rated.iter().map(|item| (item.title.as_str(), item.rating)).collect::<Vec<_>>(), vec![("a finding:2", Rating::Up), ("a finding:0", Rating::Down)]);
    assert_eq!(recent_rated_findings(&activities, "p", 1).len(), 1);
    assert_eq!(recent_rated_findings(&activities, "p", 1)[0].title, "a finding:0");
}

#[test]
fn review_location_and_fenced_code_match_the_report() {
    assert_eq!(finding_location(&finding(json!({"path": "src/a.ts"}))), "src/a.ts");
    assert_eq!(finding_location(&finding(json!({"path": "src/a.ts", "line": 4, "endLine": 4}))), "src/a.ts:4");
    assert_eq!(finding_location(&finding(json!({"path": "src/a.ts", "line": 4, "endLine": 9}))), "src/a.ts:4-9");
    assert_eq!(fenced_code("  const a = 1;\n", "src/a.ts"), "```ts\n  const a = 1;\n```");
    assert_eq!(fenced_code("const s = ````;", "Makefile"), "`````\nconst s = ````;\n`````");
    assert_eq!(fenced_code("\n\n  const a = 1;\n\n", "src/a.ts"), "```ts\n  const a = 1;\n```");
}

#[test]
fn review_fix_message_asks_for_verification_and_carries_the_finding() {
    let blocker = finding(json!({"severity": "blocker", "confidence": 90, "line": 12, "endLine": 14, "detail": "The branch name reaches `sh -c` unquoted.",
        "suggestedFix": "  execFile('git', ['checkout', branch]);", "rule": "\"Never build shell strings\" (AGENTS.md)", "preExisting": true}));
    let target = Target::PullRequest { host: "github.com".into(), repository: "o/r".into(), number: 7, url: "https://github.com/o/r/pull/7".into() };
    let message = build_fix_message("Review: #7 Run", &target, &[FindingToFix { finding: blocker.clone(), needs_checking: false }, FindingToFix { finding: finding(json!({})), needs_checking: true }]);
    assert!(message.starts_with("Fix these review findings from \"Review: #7 Run\" on PR #7 (https://github.com/o/r/pull/7). Another agent found them: verify each against the code first;"));
    assert!(message.contains("1. Shell injection\nSeverity: blocker · Confidence: 90 · Location: src/run.ts:12-14 · Pre-existing: the change makes it worse or reachable\n\nThe branch name reaches `sh -c` unquoted.\n\nRule: \"Never build shell strings\" (AGENTS.md)\n\nSuggested fix:\n```ts\n  execFile('git', ['checkout', branch]);\n```"));
    assert!(message.contains("Needs checking: the reviewer could not settle it"));
    let single = build_fix_message("Review: local changes", &Target::Local, &[FindingToFix { finding: blocker, needs_checking: false }]);
    assert!(single.starts_with("Fix this review finding from \"Review: local changes\" on the local changes."));
}

#[test]
fn review_fix_all_is_bounded_by_count_and_by_message_size() {
    let mut none: Vec<FindingToFix> = Vec::new();
    assert!(check_findings_to_fix(&mut none).is_err());
    let big = finding(json!({"detail": "d".repeat(MAX_DETAIL_CHARS), "suggestedFix": "f".repeat(MAX_SUGGESTED_FIX_CHARS), "title": "t".repeat(MAX_TITLE_CHARS), "path": "p".repeat(MAX_PATH_CHARS)}));
    let mut most: Vec<FindingToFix> = (0..MAX_FINDINGS + MAX_NEEDS_CHECKING).map(|_| FindingToFix { finding: big.clone(), needs_checking: false }).collect();
    assert_eq!(check_findings_to_fix(&mut most).unwrap_err(), "These findings make too long a message for one session. Send fewer at once.");
    let mut some: Vec<FindingToFix> = most.iter().take(20).cloned().collect();
    check_findings_to_fix(&mut some).unwrap();
    assert!(build_fix_message("t", &Target::Local, &some).len() < 120_000);
    most.push(most[0].clone());
    assert!(check_findings_to_fix(&mut most).unwrap_err().starts_with("Too many"));
}

#[test]
fn review_findings_are_read_only_when_well_formed_and_default_to_fixes() {
    let payload = json!({"verdict": "approve", "mergeConfidence": 5, "reviewEffort": 1, "findings": [], "needsChecking": [], "target": {"kind": "local"}});
    let good = Activity { id: "a".into(), kind: FINDINGS_KIND.into(), payload: payload.clone() };
    assert_eq!(good.findings().unwrap().mode(), FindingsMode::Fix);
    assert!(Activity { id: "a".into(), kind: "other".into(), payload }.findings().is_none());
    assert!(Activity { id: "a".into(), kind: FINDINGS_KIND.into(), payload: json!({"verdict": "maybe"}) }.findings().is_none());
    assert_eq!(submit_verdict_for(Verdict::RequestChanges), SubmitVerdict::RequestChanges);
    assert_eq!(submit_verdict_for(Verdict::Approve), SubmitVerdict::Approve);
    assert_eq!(submit_verdict_for(Verdict::ApproveWithNits), SubmitVerdict::Comment);
    assert_eq!(review_style_key("GitHub.com", "Octo"), "github.com/octo");
}

#[test]
fn review_commentable_lines_follow_the_hunks_own_counts() {
    let lines = commentable_lines(PATCH);
    assert_eq!(lines.keys().cloned().collect::<Vec<_>>(), vec!["new/name.ts", "src/run.ts"]);
    let run = &lines["src/run.ts"];
    assert_eq!(run.old_path, None);
    assert_eq!(
        run.lines.iter().map(|(line, position)| (*line, *position)).collect::<Vec<_>>(),
        vec![(10, Position::Context { old_line: 10, new_line: 10, side: "right" }), (11, Position::Added { new_line: 11 }), (12, Position::Added { new_line: 12 }), (13, Position::Context { old_line: 13, new_line: 13, side: "right" })]
    );
    assert_eq!(lines["new/name.ts"].old_path.as_deref(), Some("old/name.ts"));
    assert_eq!(lines["new/name.ts"].lines.iter().collect::<Vec<_>>(), vec![(&1, &Position::Added { new_line: 1 })]);
}

#[test]
fn review_puts_a_finding_on_its_line_when_the_diff_shows_it() {
    let on_line = finding(json!({"line": 11, "detail": "This still reaches a shell.", "suggestedFix": "execFile('git', [branch]);"}));
    let off_diff = finding(json!({"line": 40, "detail": "Same problem here."}));
    let no_line = finding(json!({"path": "README.md", "detail": "The docs still say exec."}));
    let renamed = finding(json!({"path": "new/name.ts", "line": 1, "detail": "Why 2?"}));
    let all: Vec<(String, Finding)> = [on_line.clone(), off_diff, no_line, renamed].into_iter().enumerate().map(|(index, finding)| (format!("finding:{index}"), finding)).collect();
    let commentable = commentable_lines(PATCH);
    let review = build_review("  Thanks, a few things. ", &all, Some(&commentable));
    assert_eq!(review.placements.iter().map(|item| item.placement).collect::<Vec<_>>(), vec![Placement::Inline, Placement::Body, Placement::Body, Placement::Inline]);
    assert_eq!(
        serde_json::to_value(&review.comments).unwrap(),
        json!([
            {"path": "src/run.ts", "position": {"kind": "added", "newLine": 11}, "body": "This still reaches a shell.\n\n```ts\nexecFile('git', [branch]);\n```"},
            {"path": "new/name.ts", "oldPath": "old/name.ts", "position": {"kind": "added", "newLine": 1}, "body": "Why 2?"}
        ])
    );
    assert_eq!(review.body, "Thanks, a few things.\n\n**Shell injection** (`src/run.ts:40`)\n\nSame problem here.\n\n**Shell injection** (`README.md`)\n\nThe docs still say exec.");
    check_review_draft(&review).unwrap();
    let blind = build_review("", &all[..1], None);
    assert!(blind.comments.is_empty());
    assert_eq!(blind.body, finding_comment_body(&on_line, Placement::Body));
    let long = ReviewDraft { body: "x".repeat(POST_MAX_BODY_CHARS + 1), comments: vec![], placements: vec![] };
    assert!(check_review_draft(&long).is_err());
}

#[test]
fn review_posted_findings_carry_across_the_sets_of_one_review() {
    let set = |id: &str, findings: Value| Activity { id: id.into(), kind: FINDINGS_KIND.into(), payload: json!({"verdict": "request-changes", "mergeConfidence": 2, "reviewEffort": 3, "findings": findings, "needsChecking": [], "target": {"kind": "local"}}) };
    let a = json!({"severity": "nit", "confidence": 90, "path": "a.ts", "line": 1, "title": "First  Thing", "detail": "d"});
    let b = json!({"severity": "nit", "confidence": 90, "path": "b.ts", "line": 2, "title": "second", "detail": "d"});
    let a_again = json!({"severity": "nit", "confidence": 95, "path": "a.ts", "line": 1, "title": "first thing", "detail": "reworded"});
    let posted = |id: &str, set: &str, keys: Value| Activity { id: id.into(), kind: POSTED_KIND.into(), payload: json!({"findingsActivityId": set, "verdict": "comment", "url": "https://h/pr/1", "findings": keys}) };
    let activities = [
        set("so-far", json!([a])),
        posted("p1", "so-far", json!([{"key": "finding:0", "placement": "inline"}])),
        set("final", json!([b, a_again])),
        posted("p2", "final", json!([{"key": "finding:0", "placement": "body"}])),
        posted("p3", "unknown", json!([{"key": "finding:0", "placement": "body"}])),
    ];
    let here = posted_findings(&activities, "final");
    assert_eq!(here.iter().map(|(key, record)| (key.as_str(), record.placement)).collect::<Vec<_>>(), vec![("finding:0", Placement::Body), ("finding:1", Placement::Inline)]);
    assert_eq!(latest_findings_id(&activities), Some("final"));
    assert_eq!(posted_findings(&activities, "so-far").len(), 1);
}

#[test]
fn review_findings_are_trimmed_and_refused_outside_their_bounds() {
    let mut good = finding(json!({"title": "  Padded  ", "line": 3, "endLine": 5}));
    good.normalize().unwrap();
    assert_eq!(good.title, "Padded");
    for (bad, message) in [
        (json!({"line": 5, "endLine": 3}), "endLine needs a line at or before it."),
        (json!({"endLine": 3}), "endLine needs a line at or before it."),
        (json!({"confidence": 101}), "confidence is 0 to 100."),
        (json!({"title": "   "}), "title must not be empty."),
        (json!({"detail": "d".repeat(MAX_DETAIL_CHARS + 1)}), "detail is longer than 1500 characters."),
        (json!({"suggestedFix": ""}), "suggestedFix is 1 to 1500 characters."),
        (json!({"line": 0}), "Lines start at 1."),
    ] {
        assert_eq!(finding(bad).normalize().unwrap_err(), message);
    }
    assert!(is_commit_sha("7523ce0") && !is_commit_sha("75zz") && !is_commit_sha("abc"));
    assert!(is_base_ref("origin/main") && !is_base_ref("-x") && !is_base_ref("a b") && !is_base_ref("a;rm"));
    assert!(LocalScope::Commit { sha: "nope".into() }.validate().is_err());
    assert!(LocalScope::Branch { base: Some("--upload-pack=x".into()) }.validate().is_err());
    assert!(is_finding_key("check:12") && !is_finding_key("finding:") && !is_finding_key("x:1") && !is_finding_key("finding:1234"));
}

#[test]
fn review_findings_mode_follows_who_opened_the_pull_request() {
    let who = |author: Option<&str>, viewer: Option<&str>, others: &[&str]| Authorship { author: author.map(str::to_owned), viewer: viewer.map(str::to_owned), viewer_logins: others.iter().map(|login| login.to_string()).collect() };
    assert_eq!(findings_mode_for(&who(Some("Me"), Some("me"), &[])), FindingsMode::Fix);
    assert_eq!(findings_mode_for(&who(Some("them"), Some("me"), &[])), FindingsMode::Comment);
    assert_eq!(findings_mode_for(&who(Some("work"), Some("me"), &["WORK"])), FindingsMode::Fix);
    assert_eq!(findings_mode_for(&who(None, Some("me"), &[])), FindingsMode::Fix);
    assert_eq!(findings_mode_for(&who(Some("them"), None, &[])), FindingsMode::Fix);
}

fn learned(lessons: &[(&str, LessonSource)]) -> LearnedSkill {
    LearnedSkill { summary: " Reviewers care about migrations. ".into(), lessons: lessons.iter().map(|(text, source)| LearnedLesson { text: (*text).into(), r#where: None, source: *source, occurrences: Some(2), evidence: None }).collect() }
}

#[test]
fn review_skill_lesson_ids_come_from_the_words() {
    assert_eq!(skill::lesson_id("NOT NULL columns need a backfill!", &HashSet::new()), "not-null-columns-need-a-backfill");
    let taken: HashSet<String> = ["not-null-columns-need-a-backfill".to_owned()].into();
    assert_eq!(skill::lesson_id("NOT NULL columns need a backfill", &taken), "not-null-columns-need-a-backfill-2");
    assert_eq!(skill::lesson_id("!!!", &HashSet::new()), "lesson");
    assert!(skill::lesson_id(&"word ".repeat(40), &HashSet::new()).len() <= 56);
}

#[test]
fn review_skill_learning_again_keeps_the_users_lessons_and_choices() {
    let first = skill::merge_learned(None, &learned(&[("Check migrations", LessonSource::KeptBreaking), ("Check migrations", LessonSource::ReviewersAsk), ("Regenerate the client", LessonSource::ReviewersAsk)]), "p", Some("t1"), "2026-01-01T00:00:00.000Z");
    assert_eq!(first.lessons.len(), 2, "the same words twice are one lesson");
    assert_eq!(first.summary, "Reviewers care about migrations.");
    let edited = skill::edit(first, SkillEdit::Add { text: "Mine".into(), r#where: Some("src/**".into()) }).unwrap();
    assert_eq!(edited.lessons[0].source, LessonSource::Yours);
    let edited = skill::edit(edited, SkillEdit::SetEnabled { lesson_id: "check-migrations".into(), enabled: false }).unwrap();
    let again = skill::merge_learned(Some(&edited), &learned(&[("Check migrations", LessonSource::KeptBreaking), ("Mine", LessonSource::KeptBreaking), ("New one", LessonSource::KeptBreaking)]), "p", Some("t2"), "2026-02-01T00:00:00.000Z");
    assert_eq!(again.lessons.iter().map(|lesson| (lesson.id.as_str(), lesson.enabled, lesson.source)).collect::<Vec<_>>(), vec![("mine", true, LessonSource::Yours), ("check-migrations", false, LessonSource::KeptBreaking), ("new-one", true, LessonSource::KeptBreaking)]);
    assert_eq!(again.learning_thread_id.as_deref(), Some("t2"));
    assert_eq!(skill::edit(again.clone(), SkillEdit::Remove { lesson_id: "nope".into() }).unwrap_err(), SkillError::Lesson);
    assert_eq!(skill::edit(again.clone(), SkillEdit::SetEnabled { lesson_id: "nope".into(), enabled: true }).unwrap_err(), SkillError::Lesson);
    assert_eq!(skill::edit(again.clone(), SkillEdit::Remove { lesson_id: "mine".into() }).unwrap().lessons.len(), 2);
    assert_eq!(skill::edit(again.clone(), SkillEdit::Add { text: " ".into(), r#where: None }).unwrap_err(), SkillError::Invalid);
    let mut full = again;
    while full.lessons.len() < skill::MAX_LESSONS {
        full = skill::edit(full.clone(), SkillEdit::Add { text: format!("Lesson {}", full.lessons.len()), r#where: None }).unwrap();
    }
    assert_eq!(skill::edit(full, SkillEdit::Add { text: "One more".into(), r#where: None }).unwrap_err(), SkillError::Full);
}

#[test]
fn review_skill_ratings_switch_a_lesson_off_once_and_respect_the_user_after() {
    let base = skill::merge_learned(None, &learned(&[("Check migrations", LessonSource::KeptBreaking), ("Regenerate the client", LessonSource::ReviewersAsk)]), "p", None, "now");
    let downs = |count: usize, lesson: &str| -> Vec<Activity> { (0..count).map(|index| rating("set", &format!("finding:{index}"), json!("down"), "p", Some(lesson))).collect() };
    let mut activities = downs(3, "check-migrations");
    activities.push(rating("set", "finding:9", json!("down"), "elsewhere", Some("check-migrations")));
    activities.push(rating("set", "finding:8", json!("down"), "p", None));
    let counts = skill::rating_counts(&activities, "p");
    assert_eq!(counts, HashMap::from([("check-migrations".to_owned(), skill::RatingCount { helpful: 0, unhelpful: 3 })]));
    let off = skill::apply_ratings(base.clone(), &counts);
    assert_eq!((off.lessons[0].enabled, off.lessons[0].disabled_by_ratings, off.lessons[0].unhelpful), (false, Some(true), 3));
    let back_on = skill::edit(off, SkillEdit::SetEnabled { lesson_id: "check-migrations".into(), enabled: true }).unwrap();
    assert_eq!(back_on.lessons[0].disabled_by_ratings, None);
    let more = skill::rating_counts(&downs(4, "check-migrations"), "p");
    assert!(skill::apply_ratings(back_on, &more).lessons[0].enabled, "later ratings leave the user's choice alone");
    // A lesson that was also found helpful stays on; one re-rated counts once; one taken back not at all.
    let mut mixed = downs(3, "regenerate-the-client");
    mixed.push(rating("set", "finding:5", json!("up"), "p", Some("regenerate-the-client")));
    mixed.push(rating("set", "finding:0", json!("up"), "p", Some("regenerate-the-client")));
    mixed.push(rating("set", "finding:1", Value::Null, "p", Some("regenerate-the-client")));
    let counts = skill::rating_counts(&mixed, "p");
    assert_eq!(counts["regenerate-the-client"], skill::RatingCount { helpful: 2, unhelpful: 1 });
    assert!(skill::apply_ratings(base.clone(), &counts).lessons[1].enabled);
    let reviewer = skill::for_reviewer(&skill::apply_ratings(base, &skill::rating_counts(&downs(3, "check-migrations"), "p")));
    assert_eq!(reviewer.lessons.iter().map(|lesson| lesson.id.as_str()).collect::<Vec<_>>(), vec!["regenerate-the-client"]);
    assert!(serde_json::to_value(&reviewer).unwrap()["lessons"][0].get("evidence").is_none());
}

#[test]
fn review_skill_is_only_taken_from_the_thread_asked_to_learn() {
    let dir = TempDir::new();
    let skills = SkillStore::new(dir.path());
    assert_eq!(skills.read("p1").unwrap(), None);
    assert_eq!(skills.accept_learned("p1", "t1", learned(&[("A lesson", LessonSource::KeptBreaking)]), "now").unwrap_err(), SkillError::NotLearning);
    assert_eq!(skills.begin_learning("p1", "t1").unwrap().status, SkillStatus::Learning);
    assert_eq!(skills.accept_learned("p1", "other", learned(&[("Poison", LessonSource::KeptBreaking)]), "now").unwrap_err(), SkillError::NotLearning);
    let mut yours = learned(&[("Sneaky", LessonSource::Yours)]);
    assert!(yours.normalize().is_err());
    assert_eq!(skills.accept_learned("p1", "t1", yours, "now").unwrap_err(), SkillError::Invalid);
    let saved = skills.accept_learned("p1", "t1", learned(&[("A lesson", LessonSource::KeptBreaking)]), "2026-01-01T00:00:00.000Z").unwrap();
    assert_eq!((saved.status, saved.lessons.len()), (SkillStatus::Learned, 1));
    assert_eq!(skills.accept_learned("p1", "t1", learned(&[("Again", LessonSource::KeptBreaking)]), "now").unwrap_err(), SkillError::NotLearning, "one hand-over per ask");
    assert_eq!(skills.read("p1").unwrap().unwrap(), saved);
    // Stopping a learning thread leaves the last skill in place.
    skills.begin_learning("p1", "t2").unwrap();
    assert_eq!(skills.cancel_learning("p1", "t2").unwrap().unwrap().status, SkillStatus::Learned);
    assert_eq!(skills.read("p1").unwrap().unwrap().lessons, saved.lessons);
    // The project id names a file: nothing outside the directory, and no file speaks for another project.
    assert_eq!(skills.read("../p1").unwrap_err(), SkillError::Read);
    std::fs::copy(dir.path().join("review-skills/p1.json"), dir.path().join("review-skills/p2.json")).unwrap();
    assert_eq!(skills.read("p2").unwrap_err(), SkillError::Read);
    assert_eq!(skills.edit("p3", SkillEdit::Add { text: "Mine".into(), r#where: None }).unwrap().lessons.len(), 1);
    assert!(std::fs::read_dir(dir.path().join("review-skills")).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
}

#[test]
fn review_skill_auto_learning_is_decided_by_the_app() {
    let learning = skill::Skill { status: SkillStatus::Learning, learning_thread_id: Some("t".into()), ..skill::Skill::empty("p") };
    let done = skill::Skill { learned_at: Some("then".into()), ..skill::Skill::empty("p") };
    assert!(skill::should_auto_learn(None, true, true, true));
    assert!(!skill::should_auto_learn(None, false, true, true), "needs a repository");
    assert!(!skill::should_auto_learn(None, true, false, true), "needs a model");
    assert!(!skill::should_auto_learn(None, true, true, false), "honours the setting");
    assert!(!skill::should_auto_learn(Some(&learning), true, true, true));
    assert!(!skill::should_auto_learn(Some(&done), true, true, true));
    assert!(skill::should_auto_learn(Some(&skill::Skill::empty("p")), true, true, true), "a skill of the user's own lessons was never learned");
}

#[test]
fn review_playbooks_name_the_tools_and_are_written_whole() {
    for (mode, heading) in [(ReviewMode::Quick, "## Quick review"), (ReviewMode::Deep, "## Deep review"), (ReviewMode::Security, "## Security audit")] {
        let text = playbook::playbook_markdown(mode);
        assert!(text.contains(heading) && text.contains("## Ground rules") && text.contains("## Re-review"));
        assert!(text.contains(playbook::SUBMIT_FINDINGS_TOOL) && text.contains(playbook::POST_FINDINGS_TOOL));
        assert!(!text.contains("{{") && !text.contains("${"), "no placeholder is left in {heading}");
    }
    assert!(playbook::skill_playbook_markdown().contains(playbook::SUBMIT_SKILL_TOOL) && !playbook::skill_playbook_markdown().contains("{{"));
    let dir = TempDir::new();
    let path = playbook::ensure_playbook(dir.path(), ReviewMode::Deep).unwrap();
    assert_eq!(path, dir.path().join("review-playbooks/deep-review.md"));
    std::fs::write(&path, "stale").unwrap();
    playbook::ensure_playbook(dir.path(), ReviewMode::Deep).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), playbook::playbook_markdown(ReviewMode::Deep));
    assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1, "no temporary file is left");
    assert!(playbook::ensure_skill_playbook(dir.path()).unwrap().ends_with("learn-review-skill.md"));
}

#[test]
fn review_first_messages_say_what_to_read_and_where_the_files_are() {
    let files = playbook::PromptFiles {
        re_review: Some(playbook::ReReview { previous_path: "/d/prev.json".into(), previous_head_sha: Some("aaaaaaa".into()), head_sha: Some("bbbbbbb".into()) }),
        rated_findings_path: Some("/d/rated.json".into()),
        review_skill_path: Some("/d/skill.json".into()),
    };
    let pr = playbook::PullRequestFacts { provider: "github".into(), repository: "o/r".into(), url: "https://github.com/o/r/pull/7".into(), number: 7, title: "Run git safely".into(), head_branch: "fix".into(), base_branch: "main".into() };
    let comments = playbook::CommentStyle { style: Some("Short and kind.".into()), samples_path: Some("/d/samples.json".into()) };
    let prompt = playbook::pull_request_prompt(&playbook::PullRequestPrompt { mode: ReviewMode::Deep, playbook_path: "/d/deep-review.md", pr: &pr, focus: Some(" auth "), head_sha: Some("bbbbbbb"), findings_mode: Some(FindingsMode::Comment), files: &files, comments: Some(&comments) });
    assert!(prompt.starts_with("Deep review of pull request #7: Run git safely\nhttps://github.com/o/r/pull/7\nGitHub o/r, fix into main (head ref pull/7/head).\nReview head commit bbbbbbb."));
    for part in ["Focus on: auth", "`git diff aaaaaaa bbbbbbb`", "/d/prev.json", "/d/rated.json", "/d/skill.json", "pass mode \"comment\"", "/d/samples.json", "Short and kind."] {
        assert!(prompt.contains(part), "{part}");
    }
    assert_eq!(playbook::pull_request_title(ReviewMode::Security, &pr), "Security audit: #7 Run git safely");
    assert_eq!(playbook::host_head_ref(&playbook::PullRequestFacts { provider: "gitlab".into(), ..pr.clone() }), "merge-requests/7/head");
    assert_eq!(playbook::host_head_ref(&playbook::PullRequestFacts { provider: "bitbucket".into(), ..pr }), "fix");

    let none = playbook::PromptFiles::default();
    let commit = LocalScope::Commit { sha: "7523ce0abc".into() };
    let local = playbook::local_prompt(&playbook::LocalPrompt { mode: ReviewMode::Quick, playbook_path: "/d/quick-review.md", cwd: "/repo", branch: Some("feat"), scope: Some(&commit), focus: None, files: &none });
    assert!(local.starts_with("Quick review of the local changes in /repo on branch feat.\n\nReview commit 7523ce0abc and only that commit: `git show 7523ce0abc`."));
    assert!(local.ends_with("pass target \"local\"."));
    assert_eq!(playbook::local_title(ReviewMode::Quick, Some("feat"), None), "Review: local changes on feat");
    assert_eq!(playbook::local_title(ReviewMode::Quick, None, Some(&commit)), "Review: commit 7523ce0");
    assert_eq!(playbook::local_title(ReviewMode::Deep, None, Some(&LocalScope::Branch { base: Some("origin/main".into()) })), "Deep review: branch vs origin/main");
    assert_eq!(playbook::local_title(ReviewMode::Security, None, Some(&LocalScope::Uncommitted)), "Security audit: uncommitted changes");
    let same = playbook::PromptFiles { re_review: Some(playbook::ReReview { previous_path: "/p".into(), previous_head_sha: Some("a".into()), head_sha: Some("a".into()) }), ..Default::default() };
    assert!(playbook::local_prompt(&playbook::LocalPrompt { mode: ReviewMode::Quick, playbook_path: "/q", cwd: "/r", branch: None, scope: None, focus: None, files: &same }).contains("there is nothing new to review"));
}

/// A reviewer's tool calls as a harness delivers them, replayed through the same dispatch
/// the running app uses: findings so far, then the final set of a re-review.
const RECORDED_CALLS: &str = r#"{"name":"pandacode_submit_review_findings","arguments":{"verdict":"request-changes","mergeConfidence":2,"reviewEffort":3,"complete":false,"target":"local","findings":[{"severity":"blocker","confidence":90,"path":"src/a.ts","line":4,"title":" Unchecked input ","detail":"What is wrong, why it matters, how it is reached.","suggestedFix":"the code as it applies","lesson":{"id":"check-input","text":"Inputs are validated at the edge"}}],"needsChecking":[]}}
{"name":"pandacode_submit_review_findings","arguments":{"verdict":"approve-with-nits","mergeConfidence":4,"reviewEffort":2,"target":"local","mode":"comment","headSha":"deadbeef","findings":[{"severity":"blocker","confidence":95,"path":"src/a.ts","line":4,"title":"Unchecked input","detail":"Still open."}],"needsChecking":[{"severity":"question","confidence":60,"path":"src/b.ts","title":"Is this reachable?","detail":"Could not settle it."}],"previous":{"fixed":1,"stillOpen":1}}}"#;

#[test]
fn review_recorded_tool_calls_become_the_threads_findings() {
    let (store, project, session) = memory();
    let dir = TempDir::new();
    assert_eq!(tools::offered(&store, dir.path(), &session), (false, false), "an ordinary thread is offered no review tool");
    let calls: Vec<Value> = RECORDED_CALLS.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    // Without a review started here, findings may only be about local changes.
    let mut aimed = calls[0]["arguments"].clone();
    aimed.as_object_mut().unwrap().remove("target");
    assert!(store::submit_findings(&store, &session, &aimed, |_| Authorship::default()).unwrap_err().starts_with("No pull request is linked"));

    store::append(&store, &session, STARTED_KIND, &StartedPayload { mode: Some(ReviewMode::Quick), target: Some(Target::Local), ..Default::default() }).unwrap();
    assert_eq!(tools::offered(&store, dir.path(), &session), (true, false));
    let first = tools::dispatch(&store, dir.path(), &session, calls[0]["name"].as_str().unwrap(), &calls[0]["arguments"]).unwrap();
    assert_eq!((first["target"].as_str(), first["findings"].as_u64()), (Some("local"), Some(1)));
    assert!(first["message"].as_str().unwrap().contains("findings so far"));
    let so_far = store::state(&store, &session).unwrap();
    assert_eq!(so_far["findings"]["complete"], json!(false));
    assert_eq!(so_far["findings"]["findings"][0]["title"], json!("Unchecked input"), "trimmed");
    let so_far_id = so_far["findingsActivityId"].as_str().unwrap().to_owned();

    // The reader rates a finding; the rating names the lesson it cited.
    assert_eq!(store::rate_finding(&store, &session, &so_far_id, "finding:7", Some(Rating::Up), None).unwrap_err(), "That finding is not part of this review.");
    assert!(store::rate_finding(&store, "missing", &so_far_id, "finding:0", Some(Rating::Up), None).is_err());
    let rated = store::rate_finding(&store, &session, &so_far_id, "finding:0", Some(Rating::Down), Some("  not here  ".into())).unwrap();
    assert_eq!((rated.rating().unwrap().finding.lesson_id.as_deref(), rated.rating().unwrap().reason.as_deref()), (Some("check-input"), Some("not here")));
    assert_eq!(store::state(&store, &session).unwrap()["ratings"], json!({"finding:0": "down"}));
    let rated_path = store::write_rated_findings(&store, dir.path(), &project).unwrap().unwrap();
    let rated_file: Value = serde_json::from_str(&std::fs::read_to_string(rated_path).unwrap()).unwrap();
    assert_eq!(rated_file["ratedFindings"], json!([{"rating": "down", "title": "Unchecked input", "severity": "blocker", "path": "src/a.ts", "reason": "not here"}]));

    // A re-review in a second thread starts from those findings.
    let second = store.create_session(&project, "claude", "sonnet", "full").unwrap().id;
    let (based_on, earlier) = store::based_on(&store, &project, &session, &so_far_id, &Target::Local).unwrap();
    assert!(store::based_on(&store, &project, &session, "nope", &Target::Local).is_err());
    assert!(store::based_on(&store, "other-project", &session, &so_far_id, &Target::Local).is_err());
    assert!(store::based_on(&store, &project, &session, &so_far_id, &Target::PullRequest { host: "h".into(), repository: "o/r".into(), number: 1, url: "u".into() }).is_err());
    let previous: Value = serde_json::from_str(&std::fs::read_to_string(store::write_previous_findings(dir.path(), &based_on.activity_id, &earlier).unwrap()).unwrap()).unwrap();
    assert_eq!(previous["findings"][0]["key"], json!("finding:0"));
    store::append(&store, &second, STARTED_KIND, &StartedPayload { target: Some(Target::Local), incremental: Some(true), based_on: Some(based_on.clone()), ..Default::default() }).unwrap();
    let last = tools::dispatch(&store, dir.path(), &second, calls[1]["name"].as_str().unwrap(), &calls[1]["arguments"]).unwrap();
    assert_eq!((last["findings"].as_u64(), last["needsChecking"].as_u64()), (Some(1), Some(1)));
    assert!(last["message"].as_str().unwrap().contains("end your turn"));
    let payload: FindingsPayload = serde_json::from_value(store::state(&store, &second).unwrap()["findings"].clone()).unwrap();
    assert_eq!(payload.mode, Some(FindingsMode::Fix), "local changes have nowhere to post to, whatever the reviewer passed");
    assert_eq!(payload.head_sha, None, "a reviewer cannot pin its findings to a commit of its choosing");
    assert_eq!((payload.incremental, payload.based_on, payload.complete), (Some(true), Some(based_on), Some(true)));
    assert_eq!(payload.previous, Some(PreviousSummary { fixed: 1, still_open: 1 }));

    // A malformed call is refused with the field at fault, and records nothing.
    let mut bad = calls[0]["arguments"].clone();
    bad["findings"][0]["endLine"] = json!(2);
    assert_eq!(tools::dispatch(&store, dir.path(), &session, "pandacode_submit_review_findings", &bad).unwrap_err(), "findings[0]: endLine needs a line at or before it.");
    assert_eq!(store::list(&store, &session).unwrap().iter().filter(|activity| activity.kind == FINDINGS_KIND).count(), 1);

    // Deleting the review thread keeps the project's ratings and drops the rest.
    store.db.execute("DELETE FROM sessions WHERE id=?1", [&session]).unwrap();
    assert_eq!(store::list(&store, &session).unwrap().iter().map(|activity| activity.kind.as_str()).collect::<Vec<_>>(), vec![RATING_KIND]);
    assert_eq!(store::project_ratings(&store, &project).unwrap().len(), 1);
}

#[test]
fn review_pull_request_findings_take_the_started_target_commit_and_mode() {
    let (store, _, session) = memory();
    let target = Target::PullRequest { host: "github.com".into(), repository: "o/r".into(), number: 7, url: "https://github.com/o/r/pull/7".into() };
    store::append(&store, &session, STARTED_KIND, &StartedPayload { target: Some(target.clone()), head_sha: Some("abcdef1".into()), ..Default::default() }).unwrap();
    let report = json!({"verdict": "approve", "mergeConfidence": 5, "reviewEffort": 1, "findings": [], "needsChecking": [], "target": "local"});
    let theirs = |_: &Target| Authorship { author: Some("them".into()), viewer: Some("me".into()), viewer_logins: vec![] };
    let (activity, reply) = store::submit_findings(&store, &session, &report, theirs).unwrap();
    let payload = activity.findings().unwrap();
    assert_eq!(reply["target"], json!("pull-request"), "the review's own target wins over what the reviewer claims");
    assert_eq!((payload.target, payload.head_sha.as_deref(), payload.mode), (target, Some("abcdef1"), Some(FindingsMode::Comment)));
    let mut chosen = report.clone();
    chosen["mode"] = json!("fix");
    assert_eq!(store::submit_findings(&store, &session, &chosen, theirs).unwrap().0.findings().unwrap().mode, Some(FindingsMode::Fix));
    let mut too_many = report;
    too_many["mergeConfidence"] = json!(6);
    assert_eq!(store::submit_findings(&store, &session, &too_many, theirs).unwrap_err(), "mergeConfidence is 0 to 5.");
}

#[test]
fn review_skill_tool_is_offered_to_the_learning_thread_alone() {
    let (store, project, session) = memory();
    let other = store.create_session(&project, "claude", "sonnet", "full").unwrap().id;
    let dir = TempDir::new();
    SkillStore::new(dir.path()).begin_learning(&project, &session).unwrap();
    assert_eq!(tools::offered(&store, dir.path(), &session), (false, true));
    assert_eq!(tools::offered(&store, dir.path(), &other), (false, false));
    let hand_over = json!({"summary": "Little history.", "lessons": [{"text": "Check migrations", "source": "kept-breaking", "occurrences": 3, "evidence": [{"label": "#12", "url": "https://h/12"}]}]});
    assert_eq!(tools::dispatch(&store, dir.path(), &other, "pandacode_submit_review_skill", &hand_over).unwrap_err(), "This thread was not asked to learn the project's review skill.");
    assert_eq!(tools::dispatch(&store, dir.path(), &session, "pandacode_submit_review_skill", &hand_over).unwrap()["lessons"], json!(1));
    let skill = SkillStore::new(dir.path()).read(&project).unwrap().unwrap();
    let path = store::write_reviewer_skill(dir.path(), &skill).unwrap().unwrap();
    assert!(std::fs::read_to_string(path).unwrap().contains("check-migrations"));
    assert_eq!(store::write_reviewer_skill(dir.path(), &skill::Skill::empty("empty")).unwrap(), None);
    assert_eq!(tools::findings_tool()["name"], json!("pandacode_submit_review_findings"));
    assert_eq!(tools::skill_tool()["name"], json!("pandacode_submit_review_skill"));
}
