//! Computer use: an agent sees the apps open on this machine and, if the user allows it,
//! operates them. Ported from ViewCode. The app is the only gate: it decides every call
//! (setting, running turn, protected apps, approval, stale targets) and only then reaches
//! the native helper (`native/computer-driver`), which asks again before it prepares a
//! target and immediately before input.
//!
//! `policy` (modes, approvals, protected apps, the decisions and their refusals),
//! `targets` (the per-thread window, ref and shot tables and what goes stale), `command`
//! (recognising a plain launcher call) and `protocol` (the helper's JSON lines) are the
//! rules, with no I/O. `driver` runs the helper, `service` applies the rules to each
//! request, `settings` stores the mode, and `tools` hands agents the tools. The commands
//! here are what a settings screen and approval cards call. See `docs/computer-use.md`.
// Rules the launcher and the approval cards will use are not called yet.
#![allow(dead_code)]

pub(crate) mod command;
pub(crate) mod driver;
pub(crate) mod policy;
pub(crate) mod protocol;
pub(crate) mod service;
#[cfg(all(test, unix))]
mod service_tests;
pub(crate) mod settings;
#[cfg(test)]
mod stream_tests;
pub(crate) mod targets;
#[cfg(test)]
mod tests;
pub(crate) mod tools;

use crate::{AppState, Result};
use policy::{ApprovalAnswer, ApprovalResponse, Mode};
use serde_json::{json, Value};
use tauri::State;

fn service(app: &tauri::AppHandle) -> Result<std::sync::Arc<service::Service>> {
    tools::service(app).ok_or_else(|| "Computer use is unavailable: the app's data folder could not be found.".to_owned())
}

/// The settings, what the helper can do right now, the approvals waiting for the user
/// and the last requests handled (command, outcome and numbers only).
#[tauri::command]
pub async fn computer_use_state(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Value> {
    let settings = settings::read(&*state.store.lock().map_err(|error| error.to_string())?)?;
    let service = service(&app)?;
    // `bypass` is for a standing warning: every permission rule is off.
    Ok(json!({ "settings": settings, "bypass": settings.approvals == policy::Approvals::Bypass, "status": service.status().await, "approvals": service.pending_approvals(), "activity": service.recent_activity() }))
}

/// Changes the mode (off, observe, control), when input asks, or whether it may take the
/// screen; fields left out keep their value. `approvals: "bypass"` (every permission rule
/// off) is refused unless the same call carries `acknowledgeRisk: true`. Turning computer use off closes every
/// approval and forgets every thread's targets.
#[tauri::command]
pub async fn computer_use_set_settings(app: tauri::AppHandle, state: State<'_, AppState>, settings: settings::SettingsPatch) -> Result<settings::Settings> {
    let saved = settings::update(&*state.store.lock().map_err(|error| error.to_string())?, settings)?;
    if saved.mode == Mode::Off {
        if let Some(service) = tools::started() {
            service.release_all();
        }
    }
    tools::changed(&app, None);
    Ok(saved)
}

/// The gate's own approvals waiting for the user, oldest first.
#[tauri::command]
pub async fn computer_use_approvals() -> Result<Vec<service::PendingApproval>> {
    Ok(tools::started().map(|service| service.pending_approvals()).unwrap_or_default())
}

/// Answers one approval with one of the decisions it offered.
#[tauri::command]
pub async fn computer_use_respond(session_id: String, request_id: String, decision: ApprovalAnswer) -> Result<()> {
    let response = tools::started().map_or(ApprovalResponse::Stale, |service| service.respond_to_approval(&session_id, &request_id, decision));
    match response {
        ApprovalResponse::Handled => Ok(()),
        ApprovalResponse::Stale => Err("This approval is no longer pending.".into()),
        ApprovalResponse::NotOwned => Err("This is not a computer-use approval.".into()),
    }
}

/// A thread's run ended: cancel its approvals, drop its grant and refuse its remaining input.
pub(crate) fn turn_ended(session_id: &str, run_id: &str) {
    if let Some(service) = tools::started() {
        service.end_turn(session_id, Some(run_id));
    }
}

/// A thread was deleted: forget what the gate remembered for it and remove its screenshots.
pub(crate) fn thread_removed(data_dir: &std::path::Path, session_id: &str) {
    if let Some(service) = tools::started() {
        service.release_thread(session_id);
    }
    let _ = std::fs::remove_dir_all(policy::screenshots_dir(data_dir, session_id));
}

/// The app is quitting: stop the helper.
pub(crate) fn shutdown() {
    if let Some(service) = tools::started() {
        service.release_all();
        service.driver().close();
    }
}
