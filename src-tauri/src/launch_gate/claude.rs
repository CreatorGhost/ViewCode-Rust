//! Claude Code's own administrator files, which change how its CLI may be started.
//!
//! - `managed-mcp.json`: the organization lists the MCP servers Claude Code may load.
//!   The CLI then refuses `--strict-mcp-config` and any server a client passes with
//!   `--mcp-config`, and exits. This app respects that: it starts Claude without its own
//!   tool server and says so, and never routes around the file.
//! - `managed-settings.json` with `disableBypassPermissionsMode: "disable"`: the CLI
//!   refuses `bypassPermissions`, so Full access runs as accept-edits instead.
//!
//! Both are read from disk; the CLI is never asked.
use std::path::{Path, PathBuf};

/// Told to Claude when its organization's MCP file keeps this app's tools out.
pub(crate) const MCP_NOTICE: &str = "Your organization manages Claude Code's MCP servers, so this app's tools are unavailable in this session. Do not claim to call them.";

fn directories() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        vec![PathBuf::from("/Library/Application Support/ClaudeCode")]
    } else if cfg!(windows) {
        vec![PathBuf::from(r"C:\Program Files\ClaudeCode"), PathBuf::from(r"C:\ProgramData\ClaudeCode")]
    } else {
        vec![PathBuf::from("/etc/claude-code")]
    }
}

pub(super) fn mcp_managed_in(directories: &[PathBuf]) -> bool {
    directories.iter().any(|directory| directory.join("managed-mcp.json").is_file())
}

/// `disableBypassPermissionsMode: "disable"`, under `permissions` or at the top level as
/// some MDM profiles write it. An unreadable file counts as not set: Claude Code itself
/// then decides, and refuses the mode if its own reading differs.
pub(super) fn bypass_disabled_in(directories: &[PathBuf]) -> bool {
    directories.iter().any(|directory| read_settings(&directory.join("managed-settings.json")).is_some_and(|settings| {
        let nested = settings.get("permissions").and_then(|permissions| permissions.get("disableBypassPermissionsMode"));
        nested.or_else(|| settings.get("disableBypassPermissionsMode")).and_then(serde_json::Value::as_str) == Some("disable")
    }))
}

fn read_settings(path: &Path) -> Option<serde_json::Value> {
    if std::fs::metadata(path).ok()?.len() > 1024 * 1024 {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

#[cfg(test)]
thread_local! {
    /// A test's stand-in for the administrator folders.
    pub(crate) static DIRECTORIES: std::cell::RefCell<Option<Vec<PathBuf>>> = const { std::cell::RefCell::new(None) };
}

fn current() -> Vec<PathBuf> {
    #[cfg(test)]
    if let Some(directories) = DIRECTORIES.with(|directories| directories.borrow().clone()) {
        return directories;
    }
    directories()
}

/// The organization lists Claude Code's MCP servers: start it without `--strict-mcp-config`
/// and without this app's `--mcp-config`.
pub(crate) fn mcp_managed() -> bool {
    mcp_managed_in(&current())
}

/// The organization turned off bypass permissions: Full access runs as accept-edits.
pub(crate) fn bypass_disabled() -> bool {
    bypass_disabled_in(&current())
}

/// Claude Code's permission mode for an app mode, under the administrator's settings.
pub(crate) fn permission_mode(mode: &str) -> &'static str {
    match mode {
        "auto" => "acceptEdits",
        // Claude Code's auto mode: its safety reviewer approves routine actions.
        "review" => "auto",
        "plan" => "plan",
        "full" if bypass_disabled() => "acceptEdits",
        "full" => "bypassPermissions",
        _ => "default",
    }
}

/// Whether `--allow-dangerously-skip-permissions` goes with this app mode.
pub(crate) fn allows_bypass(mode: &str) -> bool {
    mode == "full" && !bypass_disabled()
}
