//! Code review by an agent, ported from ViewCode. A review is an ordinary thread whose
//! first message names a playbook; the reviewer hands its findings back through a tool,
//! and the app keeps them, how the reader rates them, and what the project's history
//! taught (`skill`). The rules live in `findings`, `skill` and `playbook` with no I/O;
//! `store` keeps the records; this file is the IPC and the agent tools.
//!
//! Posting findings to a host, Fix with agent and its undo, pull request reviews and
//! learned review styles build on these records in later changes; the rules they need
//! are already here and tested, which is why some are not called from the app yet.
#![allow(dead_code)]

pub(crate) mod findings;
pub(crate) mod playbook;
pub(crate) mod skill;
pub(crate) mod store;
#[cfg(test)]
mod tests;
pub(crate) mod tools;

use crate::{AppState, Result};
use findings::{LocalScope, Rating, ReviewMode, StartedPayload, Target};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use skill::{SkillEdit, SkillStore};
use std::path::PathBuf;
use tauri::{Emitter, Manager, State};

fn data_dir(app: &tauri::AppHandle) -> Result<PathBuf> {
    app.path().app_data_dir().map_err(|error| error.to_string())
}

/// Tells every window that a thread's review records, or a project's skill, changed.
pub(crate) fn changed(app: &tauri::AppHandle, session_id: Option<&str>, project_id: Option<&str>) {
    let _ = app.emit("review-changed", json!({"sessionId": session_id, "projectId": project_id}));
}

#[tauri::command]
pub async fn review_state(state: State<'_, AppState>, session_id: String) -> Result<Value> {
    store::state(&*state.store.lock().map_err(|error| error.to_string())?, &session_id)
}

/// Rates one finding, and counts the rating into the project's review skill.
#[tauri::command]
pub async fn review_rate_finding(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    findings_activity_id: String,
    finding_key: String,
    rating: Option<Rating>,
    reason: Option<String>,
) -> Result<Value> {
    let project_id = {
        let store = state.store.lock().map_err(|error| error.to_string())?;
        let project_id = store.session(&session_id).map_err(|_| "The review thread was not found.".to_owned())?.project_id;
        store::rate_finding(&store, &session_id, &findings_activity_id, &finding_key, rating, reason).map_err(|error| if error.contains("finding") { error } else { "Could not save the rating.".into() })?;
        // The rating is saved; a skill file that cannot be updated must not undo it.
        if let Ok(ratings) = store::project_ratings(&store, &project_id) {
            let _ = SkillStore::new(&data_dir(&app)?).apply_ratings(&project_id, &ratings);
        }
        project_id
    };
    changed(&app, Some(&session_id), Some(&project_id));
    review_state(state, session_id).await
}

#[tauri::command]
pub async fn review_skill(app: tauri::AppHandle, state: State<'_, AppState>, project_id: String) -> Result<Option<skill::Skill>> {
    state.store.lock().map_err(|error| error.to_string())?.project(&project_id).map_err(|_| "The project was not found.".to_owned())?;
    SkillStore::new(&data_dir(&app)?).read(&project_id).map_err(skill::SkillError::message)
}

#[tauri::command]
pub async fn review_edit_skill(app: tauri::AppHandle, state: State<'_, AppState>, project_id: String, edit: SkillEdit) -> Result<skill::Skill> {
    state.store.lock().map_err(|error| error.to_string())?.project(&project_id).map_err(|_| "The project was not found.".to_owned())?;
    let skill = SkillStore::new(&data_dir(&app)?).edit(&project_id, edit).map_err(skill::SkillError::message)?;
    changed(&app, None, Some(&project_id));
    Ok(skill)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BasedOnInput {
    thread_id: String,
    activity_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartLocalReview {
    project_id: String,
    /// The chat whose checkout to review; absent, the project's own folder.
    source_session_id: Option<String>,
    /// Absent: a quick review.
    mode: Option<ReviewMode>,
    scope: Option<LocalScope>,
    focus: Option<String>,
    provider: String,
    model: String,
    /// Re-review from earlier findings.
    based_on: Option<BasedOnInput>,
}

/// Everything a review of local changes needs before its thread exists.
struct LocalPlan {
    title: String,
    prompt: String,
    started: StartedPayload,
    worktree: Option<(String, Option<String>)>,
    permission_mode: String,
}

fn plan_local(state: &AppState, data_dir: &std::path::Path, input: &StartLocalReview) -> Result<LocalPlan> {
    let mode = input.mode.unwrap_or(ReviewMode::Quick);
    if let Some(scope) = &input.scope {
        scope.validate()?;
    }
    let focus = input.focus.as_deref().map(str::trim).filter(|focus| !focus.is_empty());
    if focus.is_some_and(|focus| focus.len() > 2_000) {
        return Err("The focus is too long.".into());
    }
    let store = state.store.lock().map_err(|error| error.to_string())?;
    store.project(&input.project_id).map_err(|_| "The review's project was not found.".to_owned())?;
    let source = match &input.source_session_id {
        Some(id) => Some(store.session(id).ok().filter(|task| task.project_id == input.project_id).ok_or("The thread whose changes were to be reviewed was not found in this project.")?),
        None => None,
    };
    let cwd = store.workspace_root(&input.project_id, input.source_session_id.as_deref())?;
    let branch = source.as_ref().and_then(|task| task.worktree_branch.clone());
    let mut files = playbook::PromptFiles::default();
    let mut started = StartedPayload { mode: Some(mode), focus: focus.map(str::to_owned), scope: input.scope.clone(), target: Some(Target::Local), ..Default::default() };
    if let Some(based_on) = &input.based_on {
        let (based_on, earlier) = store::based_on(&store, &input.project_id, &based_on.thread_id, &based_on.activity_id, &Target::Local)?;
        let previous_path = store::write_previous_findings(data_dir, &based_on.activity_id, &earlier)?;
        files.re_review = Some(playbook::ReReview { previous_path: previous_path.to_string_lossy().into_owned(), previous_head_sha: None, head_sha: None });
        started.incremental = Some(true);
        started.based_on = Some(based_on);
    }
    files.rated_findings_path = store::write_rated_findings(&store, data_dir, &input.project_id)?.map(|path| path.to_string_lossy().into_owned());
    // A skill that cannot be read costs the review its lessons, not its start.
    if let Ok(Some(skill)) = SkillStore::new(data_dir).read(&input.project_id) {
        files.review_skill_path = store::write_reviewer_skill(data_dir, &skill)?.map(|path| path.to_string_lossy().into_owned());
    }
    let playbook_path = playbook::ensure_playbook(data_dir, mode).map_err(|_| "Could not write the review playbook to the app's data directory.".to_owned())?;
    let prompt = playbook::local_prompt(&playbook::LocalPrompt {
        mode,
        playbook_path: &playbook_path.to_string_lossy(),
        cwd: &cwd.to_string_lossy(),
        branch: branch.as_deref(),
        scope: input.scope.as_ref(),
        focus,
        files: &files,
    });
    Ok(LocalPlan {
        title: playbook::local_title(mode, branch.as_deref(), input.scope.as_ref()),
        prompt,
        started,
        worktree: source.as_ref().and_then(|task| Some((task.worktree_path.clone()?, task.worktree_branch.clone()))),
        permission_mode: source.map_or_else(|| crate::storage::DEFAULT_PERMISSION_MODE.to_owned(), |task| task.mode),
    })
}

/// Starts an agent review of a checkout's own changes in a new thread: the source
/// thread's worktree, or the project's folder. The reviewer runs there and is told to
/// change nothing. Returns the new thread's id.
#[tauri::command]
pub async fn review_start_local(app: tauri::AppHandle, state: State<'_, AppState>, input: StartLocalReview) -> Result<Value> {
    let plan = plan_local(&state, &data_dir(&app)?, &input)?;
    let session_id = {
        let store = state.store.lock().map_err(|error| error.to_string())?;
        let session = store.create_session(&input.project_id, &input.provider, &input.model, &plan.permission_mode).map_err(|_| "Could not create the review thread.".to_owned())?;
        let (path, branch) = plan.worktree.clone().map_or((None, None), |(path, branch)| (Some(path), branch));
        store
            .db
            .execute("UPDATE sessions SET title=?2,title_origin='manual',worktree_path=?3,worktree_branch=?4 WHERE id=?1", params![session.id, plan.title, path, branch])
            .map_err(|_| "Could not create the review thread.".to_owned())?;
        store::append(&store, &session.id, findings::STARTED_KIND, &plan.started).map_err(|_| "The review thread was created, but the review could not be recorded on it.".to_owned())?;
        session.id
    };
    changed(&app, Some(&session_id), Some(&input.project_id));
    crate::launch_task(app, state, session_id.clone(), Some(plan.prompt), None).await.map_err(|error| format!("The review thread was created, but the review could not be started: {error}"))?;
    Ok(json!({"sessionId": session_id, "title": plan.title}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnSkill {
    project_id: String,
    /// Asked for by the client on its own when a project is opened, not by the user: the
    /// app then learns only when it never did and nothing is learning.
    #[serde(default)]
    auto: bool,
    provider: String,
    model: String,
}

/// Starts a thread that reads the project's history and hands over its review skill.
/// Returns the thread's id, or null when an automatic request was not due.
#[tauri::command]
pub async fn review_learn_skill(app: tauri::AppHandle, state: State<'_, AppState>, input: LearnSkill) -> Result<Value> {
    let dir = data_dir(&app)?;
    let skills = SkillStore::new(&dir);
    let (session_id, prompt) = {
        let store = state.store.lock().map_err(|error| error.to_string())?;
        let project = store.project(&input.project_id).map_err(|_| "The project was not found.".to_owned())?;
        let current = skills.read(&input.project_id).map_err(skill::SkillError::message)?;
        let is_repository = std::path::Path::new(&project.path).join(".git").exists();
        if input.auto && !skill::should_auto_learn(current.as_ref(), is_repository, !input.model.trim().is_empty(), true) {
            return Ok(Value::Null);
        }
        if current.as_ref().is_some_and(|skill| skill.status == skill::SkillStatus::Learning) {
            return Ok(json!({"sessionId": current.and_then(|skill| skill.learning_thread_id)}));
        }
        let playbook_path = playbook::ensure_skill_playbook(&dir).map_err(|_| "Could not start learning this project.".to_owned())?;
        let session = store.create_session(&input.project_id, &input.provider, &input.model, crate::storage::DEFAULT_PERMISSION_MODE).map_err(|_| "Could not start learning this project.".to_owned())?;
        store
            .db
            .execute("UPDATE sessions SET title=?2,title_origin='manual' WHERE id=?1", params![session.id, playbook::learn_skill_title(&project.name)])
            .map_err(|_| "Could not start learning this project.".to_owned())?;
        skills.begin_learning(&input.project_id, &session.id).map_err(skill::SkillError::message)?;
        (session.id, playbook::learn_skill_prompt(&playbook_path.to_string_lossy()))
    };
    changed(&app, Some(&session_id), Some(&input.project_id));
    if let Err(error) = crate::launch_task(app, state, session_id.clone(), Some(prompt), None).await {
        // Nothing is learning, so no thread may hand a skill over.
        let _ = skills.cancel_learning(&input.project_id, &session_id);
        return Err(format!("Could not start learning this project: {error}"));
    }
    Ok(json!({"sessionId": session_id}))
}
