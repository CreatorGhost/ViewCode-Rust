//! Recognises a provider shell command that does nothing but run the computer-use
//! launcher, so an adapter can answer the provider's own permission prompt for it without
//! asking the user a second time.
//!
//! The command word must be exactly this session's launcher path, bare or wholly quoted.
//! A bare name resolves through the shell's PATH, which login shells (Codex runs
//! `bash -lc`) rebuild from the user's profile: a workspace entry there (direnv
//! `PATH_add bin`, `./node_modules/.bin`) would let an agent that can write files plant
//! its own program under that name and have it approved. A bare name still runs; the
//! provider just asks as usual.
//!
//! This is not the gate for computer use: every call the launcher makes is still decided
//! by `policy`. What this check must guarantee is narrower: that approving the shell
//! command runs that launcher and nothing else. So the lexer accepts only plain words and
//! quotes and rejects anything a shell would expand, redirect, chain or substitute.

use serde_json::Value;
use std::path::Path;

/// What a session spawned with computer use was handed. A session without the grant has
/// none, and nothing below approves anything for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionGrant {
    /// Absolute path of this session's launcher; the only command word that is approved.
    pub(crate) cli_path: String,
    /// Where this session's screenshots are written, when it has such a folder.
    pub(crate) screenshots_dir: Option<String>,
}

/// A provider's command as it reports it: shell source, or argv executed as is.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Command<'a> {
    Script(&'a str),
    Argv(&'a [String]),
}

/// Unquoted characters with shell meaning (operators, expansion, globbing, comments, history).
const UNQUOTED_SPECIAL: &str = ";&|<>()$`\\*?[]{}~#!^";
/// Characters that keep their meaning inside double quotes.
const DOUBLE_QUOTED_SPECIAL: &str = "$`\\!";
/// Shells whose `-c` / `-lc` script is plain shell syntax.
const WRAPPING_SHELLS: [&str; 3] = ["bash", "sh", "zsh"];
const WRAPPING_SHELL_DIRS: [&str; 3] = ["", "/bin/", "/usr/bin/"];
/// Raw-input fields providers use for the file a read tool opens.
const READ_PATH_FIELDS: [&str; 5] = ["path", "filePath", "file_path", "target_file", "absolute_path"];

/// Control characters (tab excepted), NUL, newlines and Unicode line breaks.
fn has_control_character(value: &str) -> bool {
    value.chars().any(|c| matches!(c, '\u{0}'..='\u{8}' | '\u{a}'..='\u{1f}' | '\u{7f}' | '\u{85}' | '\u{2028}' | '\u{2029}'))
}

/// Splits a script into words when it is only words, single-quoted strings and
/// double-quoted strings without expansions. `None` for anything else.
fn plain_words(script: &str) -> Option<Vec<String>> {
    if has_control_character(script) {
        return None;
    }
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut rest = script;
    while let Some(c) = rest.chars().next() {
        let after = &rest[c.len_utf8()..];
        if c == ' ' || c == '\t' {
            words.extend(word.take());
            rest = after;
        } else if c == '\'' || c == '"' {
            let end = after.find(c)?;
            let quoted = &after[..end];
            if c == '"' && quoted.chars().any(|inner| DOUBLE_QUOTED_SPECIAL.contains(inner)) {
                return None;
            }
            word.get_or_insert_with(String::new).push_str(quoted);
            rest = &after[end + 1..];
        } else if UNQUOTED_SPECIAL.contains(c) {
            return None;
        } else {
            word.get_or_insert_with(String::new).push(c);
            rest = after;
        }
    }
    words.extend(word);
    Some(words)
}

/// A shell script that is exactly one plain invocation of the launcher at `cli_path`.
fn is_plain_script(script: &str, cli_path: &str) -> bool {
    // The command word is the exact path, bare or quoted as a whole: no assignment,
    // prefix or partial quoting.
    let start = script.trim_start_matches([' ', '\t']);
    if start.is_empty() {
        return false;
    }
    let written = [cli_path.to_owned(), format!("'{cli_path}'"), format!("\"{cli_path}\"")].iter().any(|form| start.strip_prefix(form.as_str()).is_some_and(|next| next.is_empty() || next.starts_with([' ', '\t'])));
    written && plain_words(script).is_some_and(|words| words.first().is_some_and(|first| first == cli_path))
}

fn is_wrapping_shell(program: &str) -> bool {
    WRAPPING_SHELL_DIRS.iter().any(|dir| program.strip_prefix(dir).is_some_and(|name| WRAPPING_SHELLS.contains(&name)))
}

/// `bash -c <script>` / `bash -lc <script>` (also sh, zsh) as argv.
fn wrapped_script(argv: &[String]) -> Option<&str> {
    let [program, flag, script] = argv else { return None };
    (is_wrapping_shell(program) && (flag == "-c" || flag == "-lc")).then_some(script.as_str())
}

/// Whether a provider's shell command is strictly one plain invocation of the launcher
/// at `cli_path` (absolute). A script is shell source; argv is executed as is unless it
/// wraps a script in `bash|sh|zsh -c|-lc`. Rejects any other command word (the bare name
/// included), chaining, redirection, substitution, globbing, env assignments,
/// `sudo`/`env` prefixes and anything else a shell would interpret.
pub(crate) fn is_plain_computer_use_command(command: Command<'_>, cli_path: &str) -> bool {
    if !cli_path.starts_with('/') || has_control_character(cli_path) {
        return false;
    }
    match command {
        Command::Script(script) => {
            if is_plain_script(script, cli_path) {
                return true;
            }
            // Shell-joined argv of a wrapping shell, as some providers report it.
            plain_words(script).is_some_and(|words| wrapped_script(&words).is_some_and(|inner| is_plain_script(inner, cli_path)))
        }
        Command::Argv(argv) => match argv.first() {
            None => false,
            Some(first) if first == cli_path => argv.iter().all(|arg| !has_control_character(arg)),
            Some(_) => wrapped_script(argv).is_some_and(|script| is_plain_script(script, cli_path)),
        },
    }
}

/// Whether an adapter may answer a provider's permission request for this shell command
/// itself. `command` is the raw value the provider will run (a string, or an array of
/// strings), never a display title. Only a session spawned with computer use qualifies,
/// and only for its own launcher path. The gate re-reads the setting on every call, so
/// turning computer use off later makes the approved command a refused no-op.
pub(crate) fn auto_approves_computer_use_command(session: Option<&SessionGrant>, command: &Value) -> bool {
    let Some(grant) = session else { return false };
    match command {
        Value::String(script) => is_plain_computer_use_command(Command::Script(script), &grant.cli_path),
        Value::Array(parts) => {
            let argv: Option<Vec<String>> = parts.iter().map(|part| part.as_str().map(str::to_owned)).collect();
            argv.is_some_and(|argv| is_plain_computer_use_command(Command::Argv(&argv), &grant.cli_path))
        }
        _ => false,
    }
}

/// Whether a provider's request to read `file_path` only opens one of this session's own
/// computer-use screenshots, so the adapter may allow it without asking: the launcher
/// already returned the path, and the agent has to open the image to act on coordinates.
/// Both sides are resolved through symlinks, so a link planted in the folder cannot widen
/// it; a path that does not exist is refused.
pub(crate) fn is_computer_use_screenshot_read(session: Option<&SessionGrant>, file_path: &Value) -> bool {
    let Some(directory) = session.and_then(|grant| grant.screenshots_dir.as_deref()) else { return false };
    let Some(file_path) = file_path.as_str().filter(|path| !path.is_empty()) else { return false };
    if !Path::new(file_path).is_absolute() || has_control_character(file_path) {
        return false;
    }
    let (Ok(root), Ok(target)) = (std::fs::canonicalize(directory), std::fs::canonicalize(file_path)) else { return false };
    target.strip_prefix(&root).is_ok_and(|relative| !relative.as_os_str().is_empty()) && target.is_file()
}

/// How one adapter's permission requests are read.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AcpPermissionRules<'a> {
    /// Raw-input fields that carry the command the agent will execute.
    pub(crate) command_fields: &'a [&'a str],
    /// Antigravity omits `kind` on shell calls.
    pub(crate) allow_missing_kind: bool,
}

impl Default for AcpPermissionRules<'_> {
    fn default() -> Self {
        Self { command_fields: &["command"], allow_missing_kind: false }
    }
}

fn allow_once_option(request: &Value) -> Option<&str> {
    request.get("options")?.as_array()?.iter().find_map(|option| {
        let id = option.get("optionId")?.as_str()?;
        (option.get("kind").and_then(Value::as_str) == Some("allow_once") && !id.trim().is_empty()).then_some(id)
    })
}

/// True when a read request opens only this session's own computer-use screenshots: every
/// location and every path field present must be one.
fn reads_only_screenshots(session: Option<&SessionGrant>, tool_call: &Value) -> bool {
    let null = Value::Null;
    let locations = tool_call.get("locations").and_then(Value::as_array).into_iter().flatten().map(|location| location.get("path").unwrap_or(&null));
    let input = tool_call.get("rawInput").filter(|input| input.is_object());
    let fields = READ_PATH_FIELDS.iter().filter_map(|field| input?.get(*field));
    let paths: Vec<&Value> = locations.chain(fields).collect();
    !paths.is_empty() && paths.iter().all(|path| is_computer_use_screenshot_read(session, path))
}

/// The allow-once option to answer an ACP `session/request_permission` with when it only
/// runs the plain launcher in a computer-use session, or only reads that session's own
/// screenshots; `None` otherwise. The gate decides each launcher call, so asking here too
/// only doubled every prompt, and a screenshot the launcher returned has to be opened to
/// act on it. Reads the raw input the agent will execute, never the display title. Every
/// listed command field present must pass, so two fields cannot disagree about what runs.
pub(crate) fn acp_computer_use_allow_once_option<'a>(session: Option<&SessionGrant>, request: &'a Value, rules: AcpPermissionRules<'_>) -> Option<&'a str> {
    let tool_call = request.get("toolCall")?;
    let kind = tool_call.get("kind").filter(|kind| !kind.is_null());
    if kind.and_then(Value::as_str) == Some("read") {
        return reads_only_screenshots(session, tool_call).then(|| allow_once_option(request)).flatten();
    }
    let executes = kind.and_then(Value::as_str) == Some("execute") || (rules.allow_missing_kind && kind.is_none());
    let input = tool_call.get("rawInput").filter(|input| input.is_object())?;
    if !executes {
        return None;
    }
    let commands: Vec<&Value> = rules.command_fields.iter().filter_map(|field| input.get(*field)).collect();
    if commands.is_empty() || !commands.iter().all(|command| auto_approves_computer_use_command(session, command)) {
        return None;
    }
    allow_once_option(request)
}
