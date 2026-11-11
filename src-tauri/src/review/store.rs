//! The records of a review thread in SQLite, and the rules that need them: recording a
//! reviewer's findings, rating one, and writing the files a later review is pointed at.
//! Records are append-only; a thread shows its latest findings set.

use super::findings::{
    self, bounded, findings_mode_for, latest_findings_id, normalize_report, recent_rated_findings, Activity, Authorship, BasedOn, FindingsMode, FindingsPayload, RatedFindingFacts, Rating,
    RatingPayload, StartedPayload, Target,
};
use super::playbook::ensure_file;
use crate::storage::{now, Store};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, String>;

/// Ratings outlive their review thread: later reviews of the project still read them.
pub const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS review_activities(id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE, kind TEXT NOT NULL, payload TEXT NOT NULL, created_at INTEGER NOT NULL);
  CREATE INDEX IF NOT EXISTS review_activities_session ON review_activities(session_id);
  CREATE INDEX IF NOT EXISTS review_activities_project ON review_activities(project_id,kind);
  CREATE TRIGGER IF NOT EXISTS review_activities_session_deleted AFTER DELETE ON sessions BEGIN
    DELETE FROM review_activities WHERE session_id=OLD.id AND kind<>'viewcode.review.rating'; END;";

const RATED_FINDINGS_LIMIT: usize = 30;
const RATED_FINDINGS_DIR: &str = "review-ratings";
const PREVIOUS_FINDINGS_DIR: &str = "review-previous";
const SKILL_FILES_DIR: &str = "review-skill-lessons";

fn db<T>(result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|error| error.to_string())
}

fn project_of(store: &Store, session_id: &str) -> Result<String> {
    db(store.db.query_row("SELECT project_id FROM sessions WHERE id=?1", [session_id], |row| row.get(0)).optional())?.ok_or_else(|| "The review thread was not found.".to_owned())
}

/// Appends one record to a thread, and returns it.
pub fn append(store: &Store, session_id: &str, kind: &str, payload: &impl serde::Serialize) -> Result<Activity> {
    let project_id = project_of(store, session_id)?;
    let payload = serde_json::to_value(payload).map_err(|error| error.to_string())?;
    let id = uuid::Uuid::now_v7().to_string();
    db(store.db.execute(
        "INSERT INTO review_activities(id,session_id,project_id,kind,payload,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![id, session_id, project_id, kind, payload.to_string(), now()],
    ))?;
    Ok(Activity { id, kind: kind.to_owned(), payload })
}

fn read(store: &Store, sql: &str, key: &str) -> Result<Vec<Activity>> {
    let mut statement = db(store.db.prepare(sql))?;
    let rows = db(statement.query_map([key], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))))?;
    let mut activities = Vec::new();
    for row in rows {
        let (id, kind, payload) = db(row)?;
        // A row this build cannot read is skipped, never an error for the whole thread.
        if let Ok(payload) = serde_json::from_str(&payload) {
            activities.push(Activity { id, kind, payload });
        }
    }
    Ok(activities)
}

/// A thread's records, oldest first.
pub fn list(store: &Store, session_id: &str) -> Result<Vec<Activity>> {
    read(store, "SELECT id,kind,payload FROM review_activities WHERE session_id=?1 ORDER BY rowid", session_id)
}

/// Every rating in a project, oldest first.
pub fn project_ratings(store: &Store, project_id: &str) -> Result<Vec<Activity>> {
    read(store, "SELECT id,kind,payload FROM review_activities WHERE project_id=?1 AND kind='viewcode.review.rating' ORDER BY rowid", project_id)
}

/// What the app recorded when it started the review in this thread; empty when it did not.
pub fn started(store: &Store, session_id: &str) -> Result<StartedPayload> {
    Ok(list(store, session_id)?
        .iter()
        .rev()
        .find(|activity| activity.kind == findings::STARTED_KIND)
        .and_then(|activity| serde_json::from_value(activity.payload.clone()).ok())
        .unwrap_or_default())
}

/// Whether the app started a review in this thread: only such a thread records findings
/// that name a pull request.
pub fn is_review_thread(store: &Store, session_id: &str) -> Result<bool> {
    db(store.db.query_row("SELECT EXISTS(SELECT 1 FROM review_activities WHERE session_id=?1 AND kind=?2)", params![session_id, findings::STARTED_KIND], |row| row.get(0)))
}

/// What a reviewer passes beside its report.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitExtras {
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    mode: Option<FindingsMode>,
}

/// Records the findings a reviewer submitted on its own thread, and returns the record
/// and what to tell the reviewer. The target is the one the review was started on: a
/// reviewer cannot aim its findings at another pull request. `authorship` is asked only
/// when nobody chose what the findings are for.
pub fn submit_findings(store: &Store, session_id: &str, input: &Value, authorship: impl FnOnce(&Target) -> Authorship) -> Result<(Activity, Value)> {
    let start = started(store, session_id)?;
    let extras: SubmitExtras = serde_json::from_value(input.clone()).map_err(|error| format!("The findings could not be read: {error}"))?;
    let target = match (start.target.clone(), extras.target.as_deref()) {
        (Some(target), _) => target,
        (None, Some("local")) => Target::Local,
        (None, _) => return Err("No pull request is linked to this review. Pass target \"local\" for a review of local changes.".into()),
    };
    let mut report = input.clone();
    if let Some(object) = report.as_object_mut() {
        object.insert("target".into(), serde_json::to_value(&target).map_err(|error| error.to_string())?);
        for key in ["mode", "headSha", "incremental", "basedOn"] {
            object.remove(key);
        }
    }
    let mut payload: FindingsPayload = serde_json::from_value(report).map_err(|error| format!("The findings could not be read: {error}"))?;
    normalize_report(&mut payload)?;
    // Local changes have nowhere to post to.
    payload.mode = Some(match &target {
        Target::Local => FindingsMode::Fix,
        Target::PullRequest { .. } => extras.mode.or(start.findings_mode).unwrap_or_else(|| findings_mode_for(&authorship(&target))),
    });
    // The commit the start asked for: the reviewer was told to review it even if the pull
    // request moved on, so posting pins its comments there.
    payload.head_sha = start.head_sha.filter(|_| matches!(target, Target::PullRequest { .. }));
    // A re-review says which findings it started from, so clients can tell it apart.
    if let (Some(true), Some(based_on)) = (start.incremental, start.based_on) {
        payload.incremental = Some(true);
        payload.based_on = Some(based_on);
    }
    // A set sent while the review goes on, which a later call replaces.
    let complete = payload.complete.unwrap_or(true);
    payload.complete = Some(complete);
    let (count, checking) = (payload.findings.len(), payload.needs_checking.len());
    let activity = append(store, session_id, findings::FINDINGS_KIND, &payload)?;
    let message = if complete {
        "Recorded and shown to the reader under your report. Don't repeat the findings; end your turn."
    } else {
        "Recorded and shown to the reader as the findings so far. Keep reviewing; each later call passes the whole set so far, and the last one, after your report, leaves out complete."
    };
    let kind = if matches!(target, Target::Local) { "local" } else { "pull-request" };
    Ok((activity, json!({"target": kind, "findings": count, "needsChecking": checking, "message": message})))
}

/// Records a reader's thumbs up or down on one finding of a review thread, or takes it back.
pub fn rate_finding(store: &Store, session_id: &str, findings_activity_id: &str, key: &str, rating: Option<Rating>, reason: Option<String>) -> Result<Activity> {
    let project_id = project_of(store, session_id)?;
    let activities = list(store, session_id)?;
    let set = activities.iter().find(|activity| activity.id == findings_activity_id).and_then(Activity::findings);
    let finding = set.as_ref().and_then(|set| set.at(key)).ok_or("That finding is not part of this review.")?;
    let reason = match reason.filter(|_| rating == Some(Rating::Down)) {
        Some(mut reason) if !reason.trim().is_empty() => {
            bounded(&mut reason, findings::MAX_RULE_CHARS, "The reason")?;
            Some(reason)
        }
        _ => None,
    };
    let payload = RatingPayload {
        findings_activity_id: findings_activity_id.to_owned(),
        key: key.to_owned(),
        rating,
        project_id,
        finding: RatedFindingFacts {
            title: finding.title.clone(),
            severity: finding.severity,
            path: finding.path.clone(),
            rule: finding.rule.clone(),
            lesson_id: finding.lesson.as_ref().and_then(|lesson| lesson.id.clone()),
        },
        reason,
    };
    append(store, session_id, findings::RATING_KIND, &payload)
}

/// What a client needs to draw a review thread: its latest findings, how they are rated,
/// which were posted, and what was asked for.
pub fn state(store: &Store, session_id: &str) -> Result<Value> {
    let activities = list(store, session_id)?;
    let latest = latest_findings_id(&activities).map(str::to_owned);
    let findings = latest.as_ref().and_then(|id| activities.iter().find(|activity| &activity.id == id));
    let fix = activities.iter().rev().find(|activity| activity.kind == findings::FIX_STARTED_KIND).map(|activity| activity.payload.clone());
    Ok(json!({
        "started": activities.iter().rev().find(|activity| activity.kind == findings::STARTED_KIND).map(|activity| activity.payload.clone()),
        "findingsActivityId": latest,
        "findings": findings.map(|activity| activity.payload.clone()),
        "ratings": latest.as_deref().map(|id| findings::finding_ratings(&activities, id)).unwrap_or_default(),
        "posted": latest.as_deref().map(|id| findings::posted_findings(&activities, id)).unwrap_or_default(),
        "fixStarted": fix,
    }))
}

/// The earlier findings a re-review starts from: they must be findings of a thread in the
/// same project, about the same target.
pub fn based_on(store: &Store, project_id: &str, thread_id: &str, activity_id: &str, target: &Target) -> Result<(BasedOn, FindingsPayload)> {
    const MISSING: &str = "The earlier review to start from was not found, or reviewed something else.";
    if project_of(store, thread_id).map_err(|_| MISSING)? != project_id {
        return Err(MISSING.into());
    }
    let earlier = list(store, thread_id)?.iter().find(|activity| activity.id == activity_id).and_then(Activity::findings).ok_or(MISSING)?;
    let same = match (&earlier.target, target) {
        (Target::Local, Target::Local) => true,
        (Target::PullRequest { host: a, repository: b, number: c, .. }, Target::PullRequest { host: x, repository: y, number: z, .. }) => a.eq_ignore_ascii_case(x) && b.eq_ignore_ascii_case(y) && c == z,
        _ => false,
    };
    if !same {
        return Err(MISSING.into());
    }
    Ok((BasedOn { activity_id: activity_id.to_owned(), head_sha: earlier.head_sha.clone() }, earlier))
}

fn write_json(dir: &Path, name: &str, value: &Value) -> Result<PathBuf> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    ensure_file(dir, name, &text).map_err(|error| error.to_string())
}

/// Writes the project's recently rated findings for a reviewer to read, when any were rated.
pub fn write_rated_findings(store: &Store, data_dir: &Path, project_id: &str) -> Result<Option<PathBuf>> {
    let rated = recent_rated_findings(&project_ratings(store, project_id)?, project_id, RATED_FINDINGS_LIMIT);
    if rated.is_empty() {
        return Ok(None);
    }
    write_json(&data_dir.join(RATED_FINDINGS_DIR), &format!("{project_id}.json"), &json!({ "ratedFindings": rated })).map(Some)
}

/// Writes the findings a re-review starts from, each with its key.
pub fn write_previous_findings(data_dir: &Path, activity_id: &str, earlier: &FindingsPayload) -> Result<PathBuf> {
    let keyed = |list: &[findings::Finding], prefix: &str| -> Result<Vec<Value>> {
        list.iter()
            .enumerate()
            .map(|(index, finding)| {
                let mut value = serde_json::to_value(finding).map_err(|error| error.to_string())?;
                value["key"] = json!(format!("{prefix}:{index}"));
                Ok(value)
            })
            .collect()
    };
    let name = format!("{}.json", activity_id.chars().filter(|character| character.is_ascii_alphanumeric() || *character == '-').collect::<String>());
    write_json(&data_dir.join(PREVIOUS_FINDINGS_DIR), &name, &json!({"findings": keyed(&earlier.findings, "finding")?, "needsChecking": keyed(&earlier.needs_checking, "check")?}))
}

/// Writes the lessons of the project's review skill that are on, when it has any.
pub fn write_reviewer_skill(data_dir: &Path, skill: &super::skill::Skill) -> Result<Option<PathBuf>> {
    let reviewer = super::skill::for_reviewer(skill);
    if reviewer.lessons.is_empty() && reviewer.summary.trim().is_empty() {
        return Ok(None);
    }
    let value = serde_json::to_value(&reviewer).map_err(|error| error.to_string())?;
    write_json(&data_dir.join(SKILL_FILES_DIR), &format!("{}.json", skill.project_id), &value).map(Some)
}
