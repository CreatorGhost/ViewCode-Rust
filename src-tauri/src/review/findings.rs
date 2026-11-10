//! What a reviewer hands over and what is done with it: the findings report, the
//! records kept on a review thread (findings, ratings, posted), and the rules that
//! read them. No I/O here. The JSON shapes and the activity kinds are ViewCode's, so
//! its chats import without a translation.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub const FINDINGS_KIND: &str = "viewcode.review.findings";
pub const STARTED_KIND: &str = "viewcode.review.started";
pub const FIX_STARTED_KIND: &str = "viewcode.review.fix-started";
pub const POSTED_KIND: &str = "viewcode.review.posted";
pub const RATING_KIND: &str = "viewcode.review.rating";

pub const MAX_FINDINGS: usize = 30;
pub const MAX_NEEDS_CHECKING: usize = 20;
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_PATH_CHARS: usize = 300;
pub const MAX_DETAIL_CHARS: usize = 1_500;
pub const MAX_SUGGESTED_FIX_CHARS: usize = 1_500;
pub const MAX_RULE_CHARS: usize = 200;
const MAX_LESSON_EVIDENCE: usize = 6;
/// Leaves room in a host's 65,536-character review for findings that have no line to sit on.
pub const POST_MAX_OVERALL_COMMENT_CHARS: usize = 10_000;
/// A host refuses a longer review.
pub const POST_MAX_BODY_CHARS: usize = 65_536;
/// How long the findings of one fix message may be in all, so a fix never fails for being too long.
const FIX_MESSAGE_MAX_FINDING_CHARS: usize = 110_000;
/// Its number, severity, confidence, labels and line breaks, beyond the finding's own text.
const FIX_MESSAGE_FINDING_OVERHEAD_CHARS: usize = 300;

/// How thorough an agent review is: one pass, several agents, or a security audit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewMode {
    Quick,
    Deep,
    Security,
}

impl ReviewMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Quick => "Quick review",
            Self::Deep => "Deep review",
            Self::Security => "Security audit",
        }
    }
    /// How a review thread's title starts.
    pub fn title_prefix(self) -> &'static str {
        match self {
            Self::Quick => "Review",
            Self::Deep => "Deep review",
            Self::Security => "Security audit",
        }
    }
}

/// Most severe first: the order findings are grouped in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Blocker,
    Important,
    Nit,
    Question,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocker => "blocker",
            Self::Important => "important",
            Self::Nit => "nit",
            Self::Question => "question",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Approve,
    ApproveWithNits,
    RequestChanges,
}

/// The verdict a review is submitted to a host with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmitVerdict {
    Comment,
    Approve,
    RequestChanges,
}

/// What the reader does with the findings: fix them (their own pull request, or a local
/// review), or post them as review comments (somebody else's pull request).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingsMode {
    Fix,
    Comment,
}

/// Where a posted finding went: on its line of the diff, or into the review's own text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Placement {
    Inline,
    Body,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rating {
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub label: String,
    pub url: String,
}

/// A repository lesson a finding came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingLesson {
    /// The lesson's id in the project's review skill, which ties ratings of the finding to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrences: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Vec<Evidence>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub severity: Severity,
    /// 0-100: 100 is proven from the code, 50 plausible but unverified.
    pub confidence: u8,
    /// Repository-relative path of the file the finding is about.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    pub title: String,
    pub detail: String,
    /// A code block the author could apply as it stands; untrimmed, indentation is code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_fix: Option<String>,
    /// The repository rule the finding breaks, quoted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_existing: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lesson: Option<FindingLesson>,
}

/// What was reviewed: the review thread's linked pull request, or its own checkout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Target {
    PullRequest { host: String, repository: String, number: u32, url: String },
    Local,
}

/// The earlier findings a re-review started from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BasedOn {
    pub activity_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
}

/// What a re-review found of the earlier findings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviousSummary {
    pub fixed: u32,
    pub still_open: u32,
}

/// The findings record of a review thread: what the reviewer submitted, and what the app
/// knew about the review when it recorded them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingsPayload {
    pub verdict: Verdict,
    /// 0-5: 5 is safe to merge as is, 0 will break something.
    pub merge_confidence: u8,
    /// 1-5: how long a careful human review takes.
    pub review_effort: u8,
    pub findings: Vec<Finding>,
    pub needs_checking: Vec<Finding>,
    pub target: Target,
    /// Absent on findings from before there was a choice: "fix".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<FindingsMode>,
    /// The head commit the reviewer was asked to review, which posted comments are pinned to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incremental: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on: Option<BasedOn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<PreviousSummary>,
    /// false: the findings verified so far, sent while the review goes on; a later set replaces them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complete: Option<bool>,
}

impl FindingsPayload {
    pub fn mode(&self) -> FindingsMode {
        self.mode.unwrap_or(FindingsMode::Fix)
    }
    /// The finding a key (`finding:3`, `check:0`) names in this set.
    pub fn at(&self, key: &str) -> Option<&Finding> {
        let (list, index) = parse_key(key)?;
        match list {
            KeyList::Finding => self.findings.get(index),
            KeyList::Check => self.needs_checking.get(index),
        }
    }
}

/// What a review of local changes reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum LocalScope {
    Uncommitted,
    Branch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
    },
    Commit { sha: String },
}

/// Recorded when a review starts, so what the start knew reaches the findings recorded later.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ReviewMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<LocalScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incremental: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on: Option<BasedOn>,
    /// What the findings are for when the reader chose; otherwise decided when they arrive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings_mode: Option<FindingsMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
}

/// Recorded on a session started to fix findings, before its first turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixStartedPayload {
    pub review_thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings_activity_id: Option<String>,
    /// The checkpoint before the fix: undoing reverts the session to it.
    pub turn_count_before: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostedFinding {
    pub key: String,
    pub placement: Placement,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostedPayload {
    pub findings_activity_id: String,
    pub verdict: SubmitVerdict,
    pub url: String,
    pub findings: Vec<PostedFinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_moved: Option<bool>,
}

/// Kept with a rating so later reviews of the project find it without reading the findings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatedFindingFacts {
    pub title: String,
    pub severity: Severity,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// The review-skill lesson the finding cited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lesson_id: Option<String>,
}

/// A reader's thumbs up or down on one finding. The latest rating of a finding wins; null takes it back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatingPayload {
    pub findings_activity_id: String,
    pub key: String,
    pub rating: Option<Rating>,
    pub project_id: String,
    pub finding: RatedFindingFacts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One record of a review thread, oldest first where a list of them is read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub id: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

impl Activity {
    pub fn findings(&self) -> Option<FindingsPayload> {
        if self.kind != FINDINGS_KIND {
            return None;
        }
        serde_json::from_value(self.payload.clone()).ok()
    }
    pub fn posted(&self) -> Option<PostedPayload> {
        if self.kind != POSTED_KIND {
            return None;
        }
        serde_json::from_value(self.payload.clone()).ok()
    }
    pub fn rating(&self) -> Option<RatingPayload> {
        if self.kind != RATING_KIND {
            return None;
        }
        serde_json::from_value(self.payload.clone()).ok()
    }
}

/// Length as ViewCode counts it (UTF-16 units), so a bound means the same in both apps.
fn chars(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Trims `text` in place and checks it is not empty and at most `max` long.
pub(crate) fn bounded(text: &mut String, max: usize, what: &str) -> Result<(), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(format!("{what} must not be empty."));
    }
    if chars(trimmed) > max {
        return Err(format!("{what} is longer than {max} characters."));
    }
    if trimmed.len() != text.len() {
        *text = trimmed.to_owned();
    }
    Ok(())
}

fn bounded_option(text: &mut Option<String>, max: usize, what: &str) -> Result<(), String> {
    match text {
        Some(text) => bounded(text, max, what),
        None => Ok(()),
    }
}

pub(crate) fn check_evidence(evidence: &mut [Evidence], what: &str) -> Result<(), String> {
    if evidence.len() > MAX_LESSON_EVIDENCE {
        return Err(format!("{what} takes at most {MAX_LESSON_EVIDENCE} pieces of evidence."));
    }
    for item in evidence {
        bounded(&mut item.label, 40, "An evidence label")?;
        bounded(&mut item.url, 500, "An evidence url")?;
    }
    Ok(())
}

impl Finding {
    /// Trims the bounded fields and refuses a finding outside the report's bounds.
    pub fn normalize(&mut self) -> Result<(), String> {
        if self.confidence > 100 {
            return Err("confidence is 0 to 100.".into());
        }
        bounded(&mut self.path, MAX_PATH_CHARS, "path")?;
        bounded(&mut self.title, MAX_TITLE_CHARS, "title")?;
        bounded(&mut self.detail, MAX_DETAIL_CHARS, "detail")?;
        bounded_option(&mut self.rule, MAX_RULE_CHARS, "rule")?;
        if let Some(fix) = &self.suggested_fix {
            if fix.is_empty() || chars(fix) > MAX_SUGGESTED_FIX_CHARS {
                return Err(format!("suggestedFix is 1 to {MAX_SUGGESTED_FIX_CHARS} characters."));
            }
        }
        if self.line == Some(0) || self.end_line == Some(0) {
            return Err("Lines start at 1.".into());
        }
        if let Some(end) = self.end_line {
            if !self.line.is_some_and(|line| end >= line) {
                return Err("endLine needs a line at or before it.".into());
            }
        }
        if let Some(lesson) = &mut self.lesson {
            bounded_option(&mut lesson.id, 64, "lesson.id")?;
            bounded(&mut lesson.text, MAX_RULE_CHARS, "lesson.text")?;
            if lesson.occurrences == Some(0) {
                return Err("lesson.occurrences starts at 1.".into());
            }
            if let Some(evidence) = &mut lesson.evidence {
                check_evidence(evidence, "A lesson")?;
            }
        }
        Ok(())
    }
}

/// A commit a review read, as the host names it.
pub fn is_commit_sha(text: &str) -> bool {
    (7..=64).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A git ref a review of local changes diffs against, as the reader names it.
pub fn is_base_ref(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 200
        && !text.starts_with('-')
        && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._/@{}^~-".contains(&byte))
}

impl LocalScope {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Branch { base: Some(base) } if !is_base_ref(base) => Err("That base is not a git ref.".into()),
            Self::Commit { sha } if !is_commit_sha(sha) => Err("That is not a commit.".into()),
            _ => Ok(()),
        }
    }
}

/// Trims and bounds a whole report, as submitted by a reviewer.
pub fn normalize_report(payload: &mut FindingsPayload) -> Result<(), String> {
    if payload.merge_confidence > 5 {
        return Err("mergeConfidence is 0 to 5.".into());
    }
    if !(1..=5).contains(&payload.review_effort) {
        return Err("reviewEffort is 1 to 5.".into());
    }
    if payload.findings.len() > MAX_FINDINGS {
        return Err(format!("A review reports at most {MAX_FINDINGS} findings."));
    }
    if payload.needs_checking.len() > MAX_NEEDS_CHECKING {
        return Err(format!("A review lists at most {MAX_NEEDS_CHECKING} findings that need checking."));
    }
    for (index, finding) in payload.findings.iter_mut().enumerate() {
        finding.normalize().map_err(|error| format!("findings[{index}]: {error}"))?;
    }
    for (index, finding) in payload.needs_checking.iter_mut().enumerate() {
        finding.normalize().map_err(|error| format!("needsChecking[{index}]: {error}"))?;
    }
    if payload.head_sha.as_deref().is_some_and(|sha| !is_commit_sha(sha)) {
        return Err("headSha is not a commit.".into());
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyList {
    Finding,
    Check,
}

/// A finding's key on its card: which list it is in, and its index there.
fn parse_key(key: &str) -> Option<(KeyList, usize)> {
    let (list, index) = key.split_once(':')?;
    let list = match list {
        "finding" => KeyList::Finding,
        "check" => KeyList::Check,
        _ => return None,
    };
    if index.is_empty() || index.len() > 3 || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((list, index.parse().ok()?))
}

pub fn is_finding_key(key: &str) -> bool {
    parse_key(key).is_some()
}

/// One account's key among the learned review styles: its host and login, as hosts compare them.
pub fn review_style_key(host: &str, login: &str) -> String {
    format!("{}/{}", host.to_lowercase(), login.to_lowercase())
}

/// The findings a thread shows: the latest set. Records are append-only, so a re-review
/// replaces the earlier set by coming after it.
pub fn latest_findings_id(activities: &[Activity]) -> Option<&str> {
    activities.iter().rev().find(|activity| activity.kind == FINDINGS_KIND).map(|activity| activity.id.as_str())
}

/// A finding's identity across the sets one review sends (findings so far, then the final
/// set): its file, its lines and its title, whitespace and case aside. Keys name a place
/// in one set only.
pub fn finding_identity(finding: &Finding) -> String {
    let title = finding.title.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let line = finding.line.map(|line| line.to_string()).unwrap_or_default();
    let end = finding.end_line.map(|line| line.to_string()).unwrap_or_default();
    format!("{}\0{line}-{end}\0{title}", finding.path)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PostedRecord {
    pub placement: Placement,
    /// The pull request the review is on.
    pub url: String,
    pub verdict: SubmitVerdict,
}

/// Which findings of one set were posted to the pull request, by key. A finding posted
/// from an earlier set of the same review counts as posted here too, matched by
/// `finding_identity`, so a later Post all cannot post it twice.
pub fn posted_findings(activities: &[Activity], findings_activity_id: &str) -> BTreeMap<String, PostedRecord> {
    let sets: HashMap<&str, FindingsPayload> =
        activities.iter().filter_map(|activity| Some((activity.id.as_str(), activity.findings()?))).collect();
    let mut key_by_identity = HashMap::new();
    if let Some(current) = sets.get(findings_activity_id) {
        for (index, finding) in current.findings.iter().enumerate() {
            key_by_identity.insert(finding_identity(finding), format!("finding:{index}"));
        }
        for (index, finding) in current.needs_checking.iter().enumerate() {
            key_by_identity.insert(finding_identity(finding), format!("check:{index}"));
        }
    }
    let mut posted = BTreeMap::new();
    for payload in activities.iter().filter_map(Activity::posted) {
        let record = |placement| PostedRecord { placement, url: payload.url.clone(), verdict: payload.verdict };
        if payload.findings_activity_id == findings_activity_id {
            for item in &payload.findings {
                posted.insert(item.key.clone(), record(item.placement));
            }
            continue;
        }
        let Some(earlier) = sets.get(payload.findings_activity_id.as_str()) else { continue };
        for item in &payload.findings {
            let here = earlier.at(&item.key).and_then(|finding| key_by_identity.get(&finding_identity(finding)));
            if let Some(here) = here {
                posted.entry(here.clone()).or_insert_with(|| record(item.placement));
            }
        }
    }
    posted
}

/// How the findings of one set are rated now, by key: the latest rating wins.
pub fn finding_ratings(activities: &[Activity], findings_activity_id: &str) -> BTreeMap<String, Rating> {
    let mut ratings = BTreeMap::new();
    for payload in activities.iter().filter_map(Activity::rating) {
        if payload.findings_activity_id != findings_activity_id {
            continue;
        }
        match payload.rating {
            Some(rating) => ratings.insert(payload.key, rating),
            None => ratings.remove(&payload.key),
        };
    }
    ratings
}

/// A rated finding as a later reviewer is shown it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RatedFinding {
    pub rating: Rating,
    pub title: String,
    pub severity: Severity,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Why it was rated down, in the user's few words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The project's most recently rated findings, newest last, at most `limit`: each finding
/// once, as last rated, and none whose rating was taken back. Read oldest first.
pub fn recent_rated_findings(activities: &[Activity], project_id: &str, limit: usize) -> Vec<RatedFinding> {
    // Re-rating moves a finding to the end, so the order is by its latest rating.
    let mut order: Vec<String> = Vec::new();
    let mut latest: HashMap<String, Option<RatedFinding>> = HashMap::new();
    for payload in activities.iter().filter_map(Activity::rating) {
        if payload.project_id != project_id {
            continue;
        }
        let id = format!("{}/{}", payload.findings_activity_id, payload.key);
        order.retain(|seen| seen != &id);
        order.push(id.clone());
        latest.insert(
            id,
            payload.rating.map(|rating| RatedFinding {
                rating,
                title: payload.finding.title,
                severity: payload.finding.severity,
                path: payload.finding.path,
                rule: payload.finding.rule,
                reason: payload.reason,
            }),
        );
    }
    let rated: Vec<RatedFinding> = order.into_iter().filter_map(|id| latest.remove(&id).flatten()).collect();
    let skip = rated.len().saturating_sub(limit);
    rated.into_iter().skip(skip).collect()
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for byte in text.bytes() {
        run = if byte == b'`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

/// A markdown code block holding `code` as it stands: the fence outgrows any backtick run
/// inside it, and the file's extension names the language when it looks like one.
pub fn fenced_code(code: &str, path: &str) -> String {
    let fence = "`".repeat((longest_backtick_run(code) + 1).max(3));
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| (1..=10).contains(&extension.len()) && extension.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .unwrap_or("");
    // Blank lines around the code would show as an empty-looking block; indentation is kept.
    let mut body = code;
    while let Some(end) = body.find('\n') {
        if body[..end].trim_matches([' ', '\t', '\r']).is_empty() {
            body = &body[end + 1..];
        } else {
            break;
        }
    }
    format!("{fence}{extension}\n{}\n{fence}", body.trim_end())
}

/// `path:line` or `path:line-endLine`, the way the report cites a location.
pub fn finding_location(finding: &Finding) -> String {
    match (finding.line, finding.end_line) {
        (None, _) => finding.path.clone(),
        (Some(line), Some(end)) if end != line => format!("{}:{line}-{end}", finding.path),
        (Some(line), _) => format!("{}:{line}", finding.path),
    }
}

/// One finding sent to be fixed; a needs-checking item says so, since nobody settled it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingToFix {
    pub finding: Finding,
    pub needs_checking: bool,
}

/// At least the characters one finding takes in a fix message.
fn fix_message_finding_chars(finding: &Finding) -> usize {
    let fix = finding.suggested_fix.as_deref();
    FIX_MESSAGE_FINDING_OVERHEAD_CHARS
        + chars(&finding.title)
        + chars(&finding.detail)
        + 2 * chars(&finding.path)
        + finding.rule.as_deref().map_or(0, chars)
        + fix.map_or(0, |fix| chars(fix) + 2 * (longest_backtick_run(fix) + 1).max(3))
}

/// Up to a whole review's findings ("Fix all"), as long as their message fits one turn.
pub fn check_findings_to_fix(items: &mut [FindingToFix]) -> Result<(), String> {
    if items.is_empty() {
        return Err("Choose at least one finding to fix.".into());
    }
    if items.len() > MAX_FINDINGS + MAX_NEEDS_CHECKING {
        return Err("Too many findings for one session. Send fewer at once.".into());
    }
    for item in items.iter_mut() {
        item.finding.normalize()?;
    }
    if items.iter().map(|item| fix_message_finding_chars(&item.finding)).sum::<usize>() > FIX_MESSAGE_MAX_FINDING_CHARS {
        return Err("These findings make too long a message for one session. Send fewer at once.".into());
    }
    Ok(())
}

/// The message that hands findings to a coding session. The findings come from another
/// agent and may be wrong, so the session is told to verify each one before fixing it and
/// to say which it skipped. Findings sent together ("Fix all") travel in one message.
pub fn build_fix_message(review_title: &str, target: &Target, findings: &[FindingToFix]) -> String {
    let source = match target {
        Target::PullRequest { number, url, .. } => format!("on PR #{number} ({url})"),
        Target::Local => "on the local changes".to_owned(),
    };
    let what = if findings.len() == 1 { "this review finding" } else { "these review findings" };
    let mut blocks = vec![format!(
        "Fix {what} from \"{review_title}\" {source}. Another agent found them: verify each against the code first; fix the ones that hold, skip the ones that don't and say why. Run the project's checks for what you touch."
    )];
    for (index, FindingToFix { finding, needs_checking }) in findings.iter().enumerate() {
        let mut facts = vec![
            format!("Severity: {}", finding.severity.as_str()),
            format!("Confidence: {}", finding.confidence),
            format!("Location: {}", finding_location(finding)),
        ];
        if finding.pre_existing == Some(true) {
            facts.push("Pre-existing: the change makes it worse or reachable".into());
        }
        if *needs_checking {
            facts.push("Needs checking: the reviewer could not settle it".into());
        }
        let mut lines = vec![format!("{}. {}", index + 1, finding.title), facts.join(" · "), String::new(), finding.detail.clone()];
        if let Some(rule) = finding.rule.as_deref().filter(|rule| !rule.is_empty()) {
            lines.extend([String::new(), format!("Rule: {rule}")]);
        }
        if let Some(fix) = finding.suggested_fix.as_deref().filter(|fix| !fix.is_empty()) {
            lines.extend([String::new(), "Suggested fix:".into(), fenced_code(fix, &finding.path)]);
        }
        blocks.push(lines.join("\n"));
    }
    blocks.join("\n\n")
}

/// The verdict a review of the findings is submitted with until the reader picks another:
/// request changes or approve where the reviewer said so, a plain comment in between.
pub fn submit_verdict_for(verdict: Verdict) -> SubmitVerdict {
    match verdict {
        Verdict::RequestChanges => SubmitVerdict::RequestChanges,
        Verdict::Approve => SubmitVerdict::Approve,
        Verdict::ApproveWithNits => SubmitVerdict::Comment,
    }
}

/// A line of a pull request's head that a host takes a comment on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Position {
    #[serde(rename_all = "camelCase")]
    Added { new_line: u32 },
    #[serde(rename_all = "camelCase")]
    Context { old_line: u32, new_line: u32, side: &'static str },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommentableFile {
    /// What the file was called before the change, where that differs.
    pub old_path: Option<String>,
    pub lines: BTreeMap<u32, Position>,
}

/// The lines of a pull request's head that a host takes a comment on, by file.
pub type CommentableLines = BTreeMap<String, CommentableFile>;

fn patch_path(header: &str) -> Option<String> {
    let path = header.split('\t').next().unwrap_or(header);
    if path == "/dev/null" {
        return None;
    }
    Some(path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")).unwrap_or(path).to_owned())
}

/// `@@ -old[,count] +new[,count] @@`: the two starts and the two counts.
fn hunk_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    let range = |text: &str| -> Option<(u32, u32)> {
        match text.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((text.parse().ok()?, 1)),
        }
    };
    let (old_line, old_left) = range(old)?;
    let (new_line, new_left) = range(new)?;
    Some((old_line, old_left, new_line, new_left))
}

/// Which lines of the head a unified diff shows, added or unchanged: hosts refuse a
/// comment on any other line, and refuse the whole review with it. Hunks are read by
/// their counts, so a removed line that starts with `--` is not mistaken for a file header.
pub fn commentable_lines(patch: &str) -> CommentableLines {
    let mut files = CommentableLines::new();
    let mut old_path: Option<String> = None;
    let mut file: Option<String> = None;
    let (mut old_line, mut new_line, mut old_left, mut new_left) = (0u32, 0u32, 0u32, 0u32);
    for line in patch.split('\n') {
        if old_left > 0 || new_left > 0 {
            let lines = file.as_ref().and_then(|path| files.get_mut(path)).map(|entry| &mut entry.lines);
            match line.as_bytes().first() {
                Some(b'+') => {
                    if let Some(lines) = lines {
                        lines.insert(new_line, Position::Added { new_line });
                    }
                    new_line += 1;
                    new_left = new_left.saturating_sub(1);
                }
                Some(b'-') => {
                    old_line += 1;
                    old_left = old_left.saturating_sub(1);
                }
                Some(b' ') | None => {
                    if let Some(lines) = lines {
                        lines.insert(new_line, Position::Context { old_line, new_line, side: "right" });
                    }
                    old_line += 1;
                    new_line += 1;
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                }
                // "\ No newline at end of file" belongs to the line before it.
                Some(_) => {}
            }
            continue;
        }
        if line.starts_with("diff ") {
            old_path = None;
            file = None;
        } else if let Some(header) = line.strip_prefix("--- ") {
            old_path = patch_path(header);
        } else if let Some(header) = line.strip_prefix("+++ ") {
            file = patch_path(header);
            if let Some(path) = &file {
                let old = old_path.clone().filter(|old| old != path);
                files.entry(path.clone()).or_insert_with(|| CommentableFile { old_path: old, lines: BTreeMap::new() });
            }
        } else if let Some((old, old_count, new, new_count)) = hunk_header(line) {
            (old_line, old_left, new_line, new_left) = (old, old_count, new, new_count);
        }
    }
    files
}

/// A finding as the review comment that carries it, exactly as posted: the reviewer's
/// words, then its fix as code. One in the review's own text also says what and where it
/// is, since no line places it.
pub fn finding_comment_body(finding: &Finding, placement: Placement) -> String {
    let mut parts = match placement {
        Placement::Inline => vec![finding.detail.clone()],
        Placement::Body => vec![format!("**{}** (`{}`)", finding.title, finding_location(finding)), finding.detail.clone()],
    };
    if let Some(fix) = finding.suggested_fix.as_deref().filter(|fix| !fix.is_empty()) {
        parts.push(fenced_code(fix, &finding.path));
    }
    parts.join("\n\n")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentDraft {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub position: Position,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewDraft {
    pub body: String,
    pub comments: Vec<CommentDraft>,
    pub placements: Vec<PostedFinding>,
}

/// One review carrying findings: each on its line where the diff shows that line, the
/// rest in the review's text under the reader's overall comment. Without the diff every
/// finding goes in the text, which a host never refuses.
pub fn build_review(overall_comment: &str, findings: &[(String, Finding)], commentable: Option<&CommentableLines>) -> ReviewDraft {
    let mut comments = Vec::new();
    let mut in_body = Vec::new();
    let mut placements = Vec::new();
    for (key, finding) in findings {
        let file = commentable.and_then(|files| files.get(&finding.path));
        let position = finding.line.and_then(|line| file?.lines.get(&line));
        let placement = match (file, position) {
            (Some(file), Some(position)) => {
                comments.push(CommentDraft {
                    path: finding.path.clone(),
                    old_path: file.old_path.clone(),
                    position: *position,
                    body: finding_comment_body(finding, Placement::Inline),
                });
                Placement::Inline
            }
            _ => {
                in_body.push(finding_comment_body(finding, Placement::Body));
                Placement::Body
            }
        };
        placements.push(PostedFinding { key: key.clone(), placement });
    }
    let overall = overall_comment.trim();
    let body = (!overall.is_empty()).then(|| overall.to_owned()).into_iter().chain(in_body).collect::<Vec<_>>().join("\n\n");
    ReviewDraft { body, comments, placements }
}

/// A review is one request, so a body the host would refuse fails every finding it carried.
pub fn check_review_draft(draft: &ReviewDraft) -> Result<(), String> {
    if chars(&draft.body) > POST_MAX_BODY_CHARS {
        return Err("These findings make a review too long to post. Post fewer at once.".into());
    }
    Ok(())
}

/// Who opened a pull request and who it is read as; None where the host could not say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Authorship {
    pub author: Option<String>,
    pub viewer: Option<String>,
    /// The reader's other accounts on the host, where its CLI holds several.
    pub viewer_logins: Vec<String>,
}

/// What a review's findings are for: fixes on the reader's own pull request, review
/// comments on somebody else's. Without both names nobody can be told apart, and fixing
/// is the choice that posts nothing.
pub fn findings_mode_for(authorship: &Authorship) -> FindingsMode {
    let (Some(author), Some(viewer)) = (&authorship.author, &authorship.viewer) else { return FindingsMode::Fix };
    let own = std::iter::once(viewer).chain(&authorship.viewer_logins).any(|login| login.eq_ignore_ascii_case(author));
    if own {
        FindingsMode::Fix
    } else {
        FindingsMode::Comment
    }
}
