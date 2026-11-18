//! Diagrams in replies: the agent guidance for ```mermaid blocks (drawn by the webview, see
//! docs/diagrams.md) and the bundled `stack-diagram` skill for architecture diagrams with the
//! products' real logos, ported from ViewCode (`apps/server/src/diagrams/`).
//!
//! The skill's files are embedded in the binary and written to `<app data>/tools/skills/stack-diagram`
//! before a turn that may use them, because the agent's own shell has to read them. A changed copy
//! is assembled beside the old one and swapped in by rename, so a turn starting concurrently never
//! reads half a skill. The skill is offered by name: switching `stack-diagram` off in
//! Settings → Skills leaves it out of the instructions.

use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

/// The skill's name, as agents address it and as the Skills switch keys it.
pub(crate) const SKILL_NAME: &str = "stack-diagram";
const VERSION_FILE: &str = ".pandacode-version";

macro_rules! skill_files {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_bytes!(concat!("../skills/stack-diagram/", $path)) as &[u8])),*]
    };
}

/// Every file of the bundled skill (`src-tauri/skills/stack-diagram`), by relative path.
pub(crate) const SKILL_FILES: &[(&str, &[u8])] = skill_files![
    "SKILL.md",
    "LICENSE",
    "references/geometry.md",
    "references/tsx.md",
    "scripts/build.py",
    "scripts/fetch_icons.py",
    "assets/template.html",
    "assets/generic/browser.svg",
    "assets/generic/device.svg",
    "assets/generic/internet.svg",
    "assets/generic/server.svg",
    "assets/generic/user.svg",
    "assets/fonts/Geist.woff2",
    "assets/fonts/JetBrainsMono.woff2",
    "assets/fonts/OFL-Geist.txt",
    "assets/fonts/OFL-JetBrainsMono.txt",
    "assets/fonts/README.md",
];

/// A digest of the bundled files: a copy on disk with another version is replaced whole.
pub(crate) static SKILL_VERSION: LazyLock<String> = LazyLock::new(|| {
    let mut hash = Sha256::new();
    for (path, bytes) in SKILL_FILES {
        hash.update(path.as_bytes());
        hash.update([0]);
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
});

/// Mermaid renders in the webview and needs no tools, so every conversation gets this. Adapted
/// from ViewCode's `VIEWCODE_DIAGRAMS_GUIDANCE`.
pub(crate) const MERMAID_GUIDANCE: &str = "\n\nDiagrams: a ```mermaid block in your reply renders inline in PandaCode as a diagram the reader can open larger, zoom, pan, copy and save. Use one unasked when a flowchart, sequence, state, class, ER, gantt, pie, xychart, quadrant, git graph, mindmap or timeline diagram explains better than prose. The reader already sees it, so don't point to mermaid.live or another viewer, say where to view it, or restate it in prose, and don't build HTML pages, servers or screenshots to show a Mermaid diagram. Don't set colours (style, classDef, init themes); PandaCode themes diagrams.";

/// The pointer to the bundled skill, for a conversation whose agent can read `dir`.
pub(crate) fn skill_guidance(dir: &Path) -> String {
    format!(
        " For an architecture, system or tech-stack diagram of real products (clouds, frameworks, databases, services), read {} and follow it instead: it draws one with the products' official logos, and you embed its picture in your reply. Use Mermaid for those only when the user asks for Mermaid or the skill can't run (no Python 3 or no network).",
        dir.join("SKILL.md").display()
    )
}

/// Where the skill is written: `<app data>/tools/skills/stack-diagram`.
pub(crate) fn skill_dir(app_data: &Path) -> PathBuf {
    app_data.join("tools").join("skills").join(SKILL_NAME)
}

static ENSURE: Mutex<()> = Mutex::new(());

/// Leaves the bundled skill in `dir` at the bundled version and returns `dir`.
pub(crate) fn ensure_skill(dir: &Path) -> Result<PathBuf, String> {
    let _guard = ENSURE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let version = SKILL_VERSION.as_str();
    if fs::read_to_string(dir.join(VERSION_FILE)).is_ok_and(|current| current == version) {
        return Ok(dir.to_path_buf());
    }
    let parent = dir.parent().ok_or("The diagram skill has no parent folder.")?;
    fs::create_dir_all(parent).map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    let name = dir.file_name().and_then(|name| name.to_str()).unwrap_or(SKILL_NAME);
    let staging = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), uuid::Uuid::new_v4().simple()));
    let written = (|| -> std::io::Result<()> {
        for (path, bytes) in SKILL_FILES {
            let target = staging.join(path);
            if let Some(folder) = target.parent() {
                fs::create_dir_all(folder)?;
            }
            fs::write(&target, bytes)?;
        }
        fs::write(staging.join(VERSION_FILE), version)
    })();
    if let Err(error) = written {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("Could not write the diagram skill: {error}"));
    }
    // Only a folder this module wrote is replaced: a symlink or file in its place is left alone.
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_dir() => {
            fs::remove_dir_all(dir).map_err(|error| format!("Could not replace {}: {error}", dir.display()))?;
        }
        Ok(_) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(format!("{} is not a folder PandaCode wrote.", dir.display()));
        }
        Err(_) => {}
    }
    fs::rename(&staging, dir).map_err(|error| {
        let _ = fs::remove_dir_all(&staging);
        format!("Could not install the diagram skill: {error}")
    })?;
    Ok(dir.to_path_buf())
}

/// The skill's part of a turn's instructions (after `MERMAID_GUIDANCE`, which every conversation
/// gets): empty when the skill is switched off for this task (`disabled` is the task's
/// switched-off skill names) or its files could not be written.
pub(crate) fn skill_instructions(skill: Option<&Path>, disabled: &std::collections::BTreeSet<String>) -> String {
    match skill {
        Some(dir) if !disabled.contains(SKILL_NAME) => skill_guidance(dir),
        _ => String::new(),
    }
}

/// The skill folder for a local turn, written if needed; None when there is no app data folder
/// yet or it could not be written (the turn then gets Mermaid only).
pub(crate) fn prepare_skill() -> Option<PathBuf> {
    let app_data = crate::secret_store::dir()?;
    match ensure_skill(&skill_dir(app_data)) {
        Ok(dir) => Some(dir),
        Err(error) => {
            eprintln!("PandaCode: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pandacode-diagrams-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bundles_every_file_of_the_skill_folder() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("skills").join(SKILL_NAME);
        let mut on_disk = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(folder) = stack.pop() {
            for entry in fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    on_disk.push(path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        on_disk.sort();
        let mut bundled: Vec<String> = SKILL_FILES.iter().map(|(path, _)| (*path).to_owned()).collect();
        bundled.sort();
        assert_eq!(bundled, on_disk, "SKILL_FILES must list exactly the files in src-tauri/skills/stack-diagram");
    }

    #[test]
    fn the_skill_never_launches_a_browser_or_turns_off_a_sandbox() {
        assert!(!SKILL_FILES.iter().any(|(path, _)| path.contains("render_png")));
        for (path, bytes) in SKILL_FILES {
            let Ok(text) = std::str::from_utf8(bytes) else { continue };
            for banned in ["--no-sandbox", "LD_LIBRARY_PATH", "playwright install", "chromium.launch", "viewcode-browser"] {
                assert!(!text.contains(banned), "{path} contains {banned}");
            }
        }
        let skill = std::str::from_utf8(SKILL_FILES[0].1).unwrap();
        assert!(skill.starts_with("---\nname: stack-diagram\n"));
        assert!(skill.contains("pandacode_capture_browser") && skill.contains("pandacode_open_browser"));
        assert!(skill.contains("Never launch a headless browser"));
        assert!(!skill.contains("ViewCode chat"));
    }

    #[test]
    fn writes_the_skill_once_and_replaces_a_stale_copy_whole() {
        let base = temp();
        let dir = skill_dir(&base);
        assert_eq!(ensure_skill(&dir).unwrap(), dir);
        for (path, bytes) in SKILL_FILES {
            assert_eq!(&fs::read(dir.join(path)).unwrap(), bytes, "{path}");
        }
        assert_eq!(fs::read_to_string(dir.join(VERSION_FILE)).unwrap(), *SKILL_VERSION);

        // Current: left alone, so a file the agent left there stays.
        fs::write(dir.join("kept.txt"), "x").unwrap();
        ensure_skill(&dir).unwrap();
        assert!(dir.join("kept.txt").exists());

        // Stale: replaced whole, and no staging folder is left behind.
        fs::write(dir.join(VERSION_FILE), "old").unwrap();
        ensure_skill(&dir).unwrap();
        assert!(!dir.join("kept.txt").exists());
        assert_eq!(fs::read_to_string(dir.join(VERSION_FILE)).unwrap(), *SKILL_VERSION);
        let leftovers: Vec<_> = fs::read_dir(dir.parent().unwrap()).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from(SKILL_NAME)]);
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn never_replaces_something_it_did_not_write() {
        let base = temp();
        let dir = skill_dir(&base);
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        let elsewhere = base.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
        assert!(ensure_skill(&dir).is_err());
        assert!(fs::read_dir(&elsewhere).unwrap().next().is_none(), "the link's target is untouched");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn instructions_offer_the_skill_unless_it_is_switched_off() {
        let dir = Path::new("/data/tools/skills/stack-diagram");
        let on = skill_instructions(Some(dir), &BTreeSet::new());
        assert!(on.contains(&dir.join("SKILL.md").display().to_string()));
        assert!(on.contains("official logos"));
        assert_eq!(skill_instructions(Some(dir), &BTreeSet::from(["stack-diagram".to_owned()])), "");
        assert_eq!(skill_instructions(None, &BTreeSet::new()), "");
        assert!(MERMAID_GUIDANCE.contains("```mermaid block in your reply renders inline in PandaCode"));
        assert!(!MERMAID_GUIDANCE.contains("ViewCode"));
    }
}
