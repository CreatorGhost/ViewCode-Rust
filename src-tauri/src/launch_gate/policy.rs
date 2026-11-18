//! The organization's policy and the signs of a managed computer. Everything here reads
//! files in this process: no `defaults`, `plutil` or `profiles` is run, because a spawn at
//! startup is exactly what endpoint security on a managed Mac reacts to.
//!
//! ```json
//! {
//!   "strict": true,
//!   "providers": { "codex": { "allowed": false } },
//!   "allowedProviders": ["claude"],
//!   "keepAwake": { "allowed": false }
//! }
//! ```
//!
//! Every platform reads a JSON file an administrator installs in a system folder; macOS
//! also reads the MDM managed preferences for the app's bundle id (the same keys, as an XML
//! or binary plist). Where several exist the stricter wins. They are read at launch and
//! again when a file changes.
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// `identifier` in `tauri.conf.json` (`tests.rs` checks they match).
pub(super) const BUNDLE_ID: &str = "ai.pandacode.desktop";
const MAX_POLICY: u64 = 256 * 1024;

/// What the organization decided. The default is no policy: nothing blocked, nothing pinned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Policy {
    /// The file the policy came from.
    pub(crate) source: Option<String>,
    /// `strict`: nothing is turned on for the user. Absent means "on a managed computer".
    pub(crate) strict: Option<bool>,
    /// `providers.<id>.allowed: false`. Matched against a harness id and against the
    /// executable name it runs.
    pub(crate) blocked: BTreeSet<String>,
    /// `allowedProviders`: when present, only these can be turned on.
    pub(crate) allowed: Option<BTreeSet<String>>,
    /// `keepAwake.allowed`: false stops this app from holding the computer awake.
    pub(crate) keep_awake: bool,
    /// Why a policy file that exists could not be applied. While set, nothing starts.
    pub(crate) problem: Option<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self { source: None, strict: None, blocked: BTreeSet::new(), allowed: None, keep_awake: true, problem: None }
    }
}

/// Policy keys other products use for the same harness, so one profile can serve both.
fn harness_id(key: &str) -> String {
    match key {
        "claudeAgent" | "claude-code" => "claude".into(),
        "commandCode" | "command-code" => "commandcode".into(),
        other => other.to_owned(),
    }
}

impl Policy {
    /// What a decoded policy says. A key it does not understand changes nothing; a value of
    /// the wrong shape is an error, because guessing what an administrator meant could
    /// allow what they forbade.
    pub(super) fn from_value(value: &Value) -> Result<Self, String> {
        let root = value.as_object().ok_or("the policy is not a dictionary")?;
        let mut policy = Self::default();
        if let Some(strict) = root.get("strict") {
            policy.strict = Some(strict.as_bool().ok_or("`strict` must be true or false")?);
        }
        if let Some(providers) = root.get("providers") {
            for (id, rule) in providers.as_object().ok_or("`providers` must be a dictionary")? {
                let allowed = rule.as_object().ok_or("each `providers` entry must be a dictionary")?.get("allowed");
                match allowed {
                    Some(Value::Bool(false)) => {
                        policy.blocked.insert(harness_id(id));
                    }
                    Some(Value::Bool(true)) | None => {}
                    Some(_) => return Err(format!("`providers.{id}.allowed` must be true or false")),
                }
            }
        }
        if let Some(allowed) = root.get("allowedProviders") {
            let list = allowed.as_array().ok_or("`allowedProviders` must be a list")?;
            let ids = list.iter().map(|id| id.as_str().map(harness_id).ok_or("`allowedProviders` must list provider ids")).collect::<Result<BTreeSet<_>, _>>()?;
            policy.allowed = Some(ids);
        }
        if let Some(keep_awake) = root.get("keepAwake") {
            match keep_awake.as_object().ok_or("`keepAwake` must be a dictionary")?.get("allowed") {
                Some(Value::Bool(allowed)) => policy.keep_awake = *allowed,
                None => {}
                Some(_) => return Err("`keepAwake.allowed` must be true or false".into()),
            }
        }
        Ok(policy)
    }

    /// Reads the policy at `path`: a plist when the name ends in `.plist`, else JSON. A
    /// missing file is no policy. A file that is there but unreadable is a policy with a
    /// `problem`, which the gate treats as "start nothing".
    pub(super) fn read(path: &Path) -> Option<Self> {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        let source = Some(path.to_string_lossy().into_owned());
        let parsed = (|| {
            if metadata.len() > MAX_POLICY {
                return Err("it is too large".to_owned());
            }
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            let value = if path.extension().is_some_and(|extension| extension == "plist") { plist_value(&bytes)? } else { serde_json::from_slice(&bytes).map_err(|e| e.to_string())? };
            Self::from_value(&value)
        })();
        Some(match parsed {
            Ok(policy) => Self { source, ..policy },
            Err(problem) => Self { source, problem: Some(format!("The organization policy at {} could not be applied: {problem}.", path.display())), ..Self::default() },
        })
    }

    /// Every policy file among `paths`, combined so that the stricter one wins: a harness
    /// any source blocks is blocked, only what every allow list names is allowed, strict
    /// if any says so, and one unreadable file is a problem for the whole policy.
    pub(super) fn merged(paths: &[PathBuf]) -> Self {
        let mut found = paths.iter().filter_map(|path| Self::read(path));
        let Some(mut policy) = found.next() else { return Self::default() };
        for next in found {
            policy.source = Some([policy.source.unwrap_or_default(), next.source.unwrap_or_default()].join(", "));
            policy.strict = match (policy.strict, next.strict) {
                (Some(a), Some(b)) => Some(a || b),
                (a, b) => a.or(b),
            };
            policy.blocked.extend(next.blocked);
            policy.allowed = match (policy.allowed, next.allowed) {
                (Some(a), Some(b)) => Some(a.intersection(&b).cloned().collect()),
                (a, b) => a.or(b),
            };
            policy.keep_awake &= next.keep_awake;
            policy.problem = policy.problem.or(next.problem);
        }
        policy
    }
}

/// How often the policy files are looked at again while the app runs.
pub(super) const INTERVAL_MS: u64 = 30_000;

/// Notices a policy file that appeared, changed or went away. It compares sizes and
/// modification times, no more often than [`INTERVAL_MS`]; the clock is passed in, so the
/// tests move time themselves.
pub(super) struct Watch {
    paths: Vec<PathBuf>,
    seen: Vec<Option<(u64, Option<std::time::SystemTime>)>>,
    checked: u64,
}

impl Watch {
    pub(super) fn new(paths: Vec<PathBuf>, now: u64) -> Self {
        let seen = Self::signature(&paths);
        Self { paths, seen, checked: now }
    }

    fn signature(paths: &[PathBuf]) -> Vec<Option<(u64, Option<std::time::SystemTime>)>> {
        paths.iter().map(|path| std::fs::symlink_metadata(path).ok().map(|metadata| (metadata.len(), metadata.modified().ok()))).collect()
    }

    /// The policy as the files say now.
    pub(super) fn policy(&self) -> Policy {
        Policy::merged(&self.paths)
    }

    /// The policy again, if the interval has passed and a file changed since the last look.
    pub(super) fn poll(&mut self, now: u64) -> Option<Policy> {
        if now.saturating_sub(self.checked) < INTERVAL_MS {
            return None;
        }
        self.checked = now;
        let seen = Self::signature(&self.paths);
        if seen == self.seen {
            return None;
        }
        self.seen = seen;
        Some(self.policy())
    }
}

/// Where an administrator installs the policy: a JSON file in a system folder only an
/// administrator can write, and on macOS also the MDM managed preferences for this app
/// (device-wide, then the signed-in user's; both written only by the MDM client).
pub(super) fn paths() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        let root = Path::new("/Library/Managed Preferences");
        let file = format!("{BUNDLE_ID}.plist");
        let mut paths = vec![PathBuf::from("/Library/Application Support/ViewCode/policy.json"), root.join(&file)];
        if let Some(user) = std::env::var_os("USER").filter(|user| !user.is_empty() && Path::new(user).components().count() == 1 && Path::new(user).file_name().is_some()) {
            paths.push(root.join(user).join(&file));
        }
        paths
    } else if cfg!(windows) {
        let root = std::env::var_os("ProgramData").map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
        vec![root.join("ViewCode").join("policy.json")]
    } else {
        vec![PathBuf::from("/etc/viewcode/policy.json")]
    }
}

/// A plist (XML or binary) as plain JSON values. Dates, data and UIDs carry no policy.
fn plist_value(bytes: &[u8]) -> Result<Value, String> {
    fn convert(value: plist::Value, depth: usize) -> Result<Value, String> {
        if depth > 32 {
            return Err("the plist nests too deeply".into());
        }
        Ok(match value {
            plist::Value::Boolean(value) => Value::Bool(value),
            plist::Value::String(value) => Value::String(value),
            plist::Value::Integer(value) => value.as_signed().map(Value::from).or_else(|| value.as_unsigned().map(Value::from)).unwrap_or(Value::Null),
            plist::Value::Real(value) => serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number),
            plist::Value::Array(values) => Value::Array(values.into_iter().map(|value| convert(value, depth + 1)).collect::<Result<_, _>>()?),
            plist::Value::Dictionary(dictionary) => Value::Object(dictionary.into_iter().map(|(key, value)| Ok((key, convert(value, depth + 1)?))).collect::<Result<_, String>>()?),
            _ => Value::Null,
        })
    }
    convert(plist::Value::from_reader(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?, 0)
}

/// Signs under `root` (the filesystem root) that an organization manages this Mac: any
/// MDM managed preference, or Claude Code's administrator files. Reads directory entries
/// and file metadata only.
pub(super) fn managed_evidence_in(root: &Path) -> Vec<String> {
    let mut evidence = Vec::new();
    let preferences = root.join("Library/Managed Preferences");
    let has_entries = std::fs::read_dir(&preferences).is_ok_and(|entries| entries.flatten().any(|entry| !entry.file_name().to_string_lossy().starts_with('.')));
    if has_entries {
        evidence.push("Managed preferences are installed (/Library/Managed Preferences).".into());
    }
    for file in ["managed-settings.json", "managed-mcp.json"] {
        if root.join("Library/Application Support/ClaudeCode").join(file).is_file() {
            evidence.push(format!("Claude Code is managed by an administrator ({file})."));
        }
    }
    evidence
}

/// Evidence that this computer is managed. Only macOS is examined; elsewhere the answer is
/// "no evidence", and the enabled set and any policy file still apply.
pub(super) fn managed_evidence() -> Vec<String> {
    if cfg!(target_os = "macos") {
        managed_evidence_in(Path::new("/"))
    } else {
        Vec::new()
    }
}
