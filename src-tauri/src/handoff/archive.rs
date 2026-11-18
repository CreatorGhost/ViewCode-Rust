//! The thread's history file: every saved message whole, so a handoff can point at what it
//! summarised or left out. `<app data>/handoffs/<thread>/history.md`, private to the user,
//! rewritten at each handoff and removed with the thread.
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) const DIR: &str = "handoffs";
const FILE: &str = "history.md";

fn session_name(session_id: &str) -> Option<&str> {
    ((1..=64).contains(&session_id.len()) && session_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then_some(session_id)
}

/// The folder next to the database; `None` for a database that is not a file.
pub(crate) fn root(store: &crate::storage::Store) -> Option<PathBuf> {
    store.path.parent().filter(|dir| dir.is_absolute()).map(|dir| dir.join(DIR))
}

#[cfg(unix)]
fn private(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}
#[cfg(not(unix))]
fn private(_: &Path, _: u32) {}

/// Neither the root nor the thread's folder may be a link: the file is written where it says.
fn folder(root: &Path, session_id: &str) -> Option<PathBuf> {
    let dir = root.join(session_name(session_id)?);
    std::fs::create_dir_all(&dir).ok()?;
    for step in [root, dir.as_path()] {
        let meta = std::fs::symlink_metadata(step).ok()?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return None;
        }
        private(step, 0o700);
    }
    Some(dir)
}

pub(crate) struct Writer {
    file: std::io::BufWriter<std::fs::File>,
    temp: PathBuf,
    path: PathBuf,
    line: usize,
    failed: bool,
}

impl Writer {
    /// `None` when the file cannot be created; the handoff then says so instead of pointing.
    pub(crate) fn create(root: &Path, session_id: &str, title: &str) -> Option<Self> {
        let dir = folder(root, session_id)?;
        let (path, temp) = (dir.join(FILE), dir.join(format!("{FILE}.tmp")));
        let _ = std::fs::remove_file(&temp);
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp).ok()?;
        private(&temp, 0o600);
        let mut writer = Self { file: std::io::BufWriter::new(file), temp, path, line: 1, failed: false };
        writer.put(&format!(
            "# Thread history: {}\n\nEvery saved message of this PandaCode thread, oldest first, whole. Each starts at a heading `## #<id> <role>`.\n",
            title.replace(['\n', '\r'], " ")
        ));
        Some(writer)
    }

    fn put(&mut self, text: &str) {
        self.failed |= self.file.write_all(text.as_bytes()).is_err();
        self.line += text.bytes().filter(|byte| *byte == b'\n').count();
    }

    /// Appends one message and returns the line its heading is on.
    pub(crate) fn push(&mut self, id: i64, role: &str, body: &str) -> usize {
        self.put("\n");
        let line = self.line;
        self.put(&format!("## #{id} {role}\n"));
        self.put(body);
        if !body.ends_with('\n') {
            self.put("\n");
        }
        line
    }

    /// The finished file, moved into place whole.
    pub(crate) fn finish(mut self) -> Option<PathBuf> {
        if self.failed || self.file.flush().is_err() || std::fs::rename(&self.temp, &self.path).is_err() {
            let _ = std::fs::remove_file(&self.temp);
            return None;
        }
        Some(self.path)
    }
}

/// Removes the history files of deleted threads, best effort.
pub(crate) fn forget(app_data: &Path, session_ids: &[String]) {
    for id in session_ids.iter().filter_map(|id| session_name(id)) {
        let dir = app_data.join(DIR).join(id);
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.file_type().is_symlink() => drop(std::fs::remove_file(&dir)),
            Ok(_) => {
                if let Err(error) = std::fs::remove_dir_all(&dir) {
                    eprintln!("PandaCode: could not remove a deleted thread's history file: {error}");
                }
            }
            Err(_) => {}
        }
    }
}
