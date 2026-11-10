//! The tools a reviewer hands its work back with. `submit_review_findings` is offered
//! only on a thread the app started a review in; `submit_review_skill` only on the
//! thread the app asked to learn its project, since every later review trusts the file.

use super::{findings, playbook, skill::SkillStore, store};
use crate::agent_tools::spec;
use crate::providers::ToolHost;
use crate::{storage::Store, AppState, Result};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::Manager;

fn short(name: &str) -> &str {
    name.strip_prefix("pandacode_").unwrap_or(name)
}

fn finding_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["severity","confidence","path","title","detail"],"properties":{
        "severity":{"type":"string","enum":["blocker","important","nit","question"]},
        "confidence":{"type":"integer","minimum":0,"maximum":100,"description":"100 is proven from the code, 50 plausible but unverified."},
        "path":{"type":"string","maxLength":findings::MAX_PATH_CHARS,"description":"Repository-relative path."},
        "line":{"type":"integer","minimum":1},
        "endLine":{"type":"integer","minimum":1},
        "title":{"type":"string","maxLength":findings::MAX_TITLE_CHARS},
        "detail":{"type":"string","maxLength":findings::MAX_DETAIL_CHARS,"description":"The problem, why it matters and the path from a caller or input to it. Must stand on its own."},
        "suggestedFix":{"type":"string","maxLength":findings::MAX_SUGGESTED_FIX_CHARS,"description":"Code the author could apply as it stands."},
        "rule":{"type":"string","maxLength":findings::MAX_RULE_CHARS,"description":"The repository rule the finding breaks, quoted."},
        "preExisting":{"type":"boolean"},
        "lesson":{"type":"object","additionalProperties":false,"required":["text"],"properties":{
            "id":{"type":"string","maxLength":64},"text":{"type":"string","maxLength":findings::MAX_RULE_CHARS},"occurrences":{"type":"integer","minimum":1},
            "evidence":{"type":"array","maxItems":6,"items":{"type":"object","additionalProperties":false,"required":["label","url"],"properties":{"label":{"type":"string","maxLength":40},"url":{"type":"string","maxLength":500}}}}}}
    }})
}

pub(crate) fn findings_tool() -> Value {
    spec(
        short(playbook::SUBMIT_FINDINGS_TOOL),
        "Hand your review's findings to the app, which shows them to the reader beside your report. Every call carries the whole set so far and replaces the one before it. Pass complete: false while the review goes on; leave it out for the final set.",
        json!({
            "verdict":{"type":"string","enum":["approve","approve-with-nits","request-changes"]},
            "mergeConfidence":{"type":"integer","minimum":0,"maximum":5,"description":"5 is safe to merge as is, 0 will break something."},
            "reviewEffort":{"type":"integer","minimum":1,"maximum":5,"description":"How long a careful human review takes."},
            "findings":{"type":"array","maxItems":findings::MAX_FINDINGS,"items":finding_schema()},
            "needsChecking":{"type":"array","maxItems":findings::MAX_NEEDS_CHECKING,"items":finding_schema()},
            "target":{"type":"string","enum":["local"],"description":"Pass \"local\" for a review of local changes."},
            "mode":{"type":"string","enum":["fix","comment"],"description":"Only when the first message says which."},
            "previous":{"type":"object","additionalProperties":false,"required":["fixed","stillOpen"],"properties":{"fixed":{"type":"integer","minimum":0},"stillOpen":{"type":"integer","minimum":0}},"description":"A re-review: how the earlier findings fared."},
            "complete":{"type":"boolean"}
        }),
        &["verdict", "mergeConfidence", "reviewEffort", "findings", "needsChecking"],
    )
}

pub(crate) fn skill_tool() -> Value {
    spec(
        short(playbook::SUBMIT_SKILL_TOOL),
        "Hand over what you learned about how this project is reviewed. Call once, at the end.",
        json!({
            "summary":{"type":"string","maxLength":4000},
            "lessons":{"type":"array","maxItems":super::skill::MAX_LESSONS,"items":{"type":"object","additionalProperties":false,"required":["text","source"],"properties":{
                "text":{"type":"string","maxLength":super::skill::MAX_LESSON_CHARS},
                "where":{"type":"string","maxLength":super::skill::MAX_LESSON_CHARS,"description":"Path globs it applies to."},
                "source":{"type":"string","enum":["kept-breaking","reviewers-ask"]},
                "occurrences":{"type":"integer","minimum":1},
                "evidence":{"type":"array","maxItems":6,"items":{"type":"object","additionalProperties":false,"required":["label","url"],"properties":{"label":{"type":"string","maxLength":40},"url":{"type":"string","maxLength":500}}}}
            }}}
        }),
        &["summary", "lessons"],
    )
}

/// Which review tools a thread is offered: (findings, skill).
pub(crate) fn offered(store: &Store, data_dir: &Path, session_id: &str) -> (bool, bool) {
    let review = store::is_review_thread(store, session_id).unwrap_or(false);
    let learning = store
        .session(session_id)
        .ok()
        .and_then(|task| SkillStore::new(data_dir).read(&task.project_id).ok().flatten())
        .is_some_and(|skill| skill.status == super::skill::SkillStatus::Learning && skill.learning_thread_id.as_deref() == Some(session_id));
    (review, learning)
}

/// Runs one review tool for a thread. The caller has checked the tool was offered.
pub(crate) fn dispatch(store: &Store, data_dir: &Path, session_id: &str, name: &str, args: &Value) -> Result<Value> {
    if name == playbook::SUBMIT_FINDINGS_TOOL {
        // Who opened the pull request is read from the host with pull request reviews;
        // until then nobody is told apart, and fixing is the choice that posts nothing.
        let (_, reply) = store::submit_findings(store, session_id, args, |_| findings::Authorship::default())?;
        return Ok(reply);
    }
    let project_id = store.session(session_id)?.project_id;
    let learned = serde_json::from_value(args.clone()).map_err(|error| format!("The review skill could not be read: {error}"))?;
    let skill = SkillStore::new(data_dir).accept_learned(&project_id, session_id, learned, &chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)).map_err(super::skill::SkillError::message)?;
    Ok(json!({"lessons": skill.lessons.len(), "message": "Saved as this project's review skill. End your turn with two or three sentences on what you learned."}))
}

/// Adds the review tools a thread may use; otherwise returns the host unchanged.
pub(crate) fn with_review(base: Option<ToolHost>, app: tauri::AppHandle, session_id: String, stopped: Arc<AtomicBool>) -> Option<ToolHost> {
    let Ok(data_dir) = app.path().app_data_dir() else { return base };
    let (review, learning) = match app.state::<AppState>().store.lock() {
        Ok(store) => offered(&store, &data_dir, &session_id),
        Err(_) => return base,
    };
    if !review && !learning {
        return base;
    }
    let mut tools = base.as_ref().map_or_else(Vec::new, |host| host.tools.clone());
    let approval_tools = base.as_ref().map_or_else(Default::default, |host| host.approval_tools.clone());
    let mut names = Vec::new();
    if review {
        tools.push(findings_tool());
        names.push(playbook::SUBMIT_FINDINGS_TOOL);
    }
    if learning {
        tools.push(skill_tool());
        names.push(playbook::SUBMIT_SKILL_TOOL);
    }
    Some(ToolHost {
        tools,
        approval_tools,
        handler: Arc::new(move |name, args| {
            let (base, app, session_id, stopped, data_dir, names) = (base.clone(), app.clone(), session_id.clone(), stopped.clone(), data_dir.clone(), names.clone());
            Box::pin(async move {
                if stopped.load(Ordering::SeqCst) {
                    return Err("Task has stopped.".into());
                }
                if !names.contains(&name.as_str()) {
                    return match base {
                        Some(base) => (base.handler)(name, args).await,
                        None => Err("Unknown PandaCode tool.".into()),
                    };
                }
                let result = {
                    let state = app.state::<AppState>();
                    let store = state.store.lock().map_err(|error| error.to_string())?;
                    dispatch(&store, &data_dir, &session_id, &name, &args)?
                };
                super::changed(&app, Some(&session_id), None);
                Ok(result)
            })
        }),
    })
}
