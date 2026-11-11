//! Chooses the Node.js that runs wrangler: the machine's own when it is version 22 or
//! newer, otherwise the pinned download (`node.rs`). Called only from a setup operation the
//! user started, so nothing here runs or downloads before that.
use super::node::{self, Release};
use super::process::{self, Run};
use super::setup::Fail;
use super::wrangler::Launcher;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub(crate) struct Finder {
    /// `node` and `npx` where an interactive shell would find them, if installed.
    pub node: Option<PathBuf>,
    pub npx: Option<PathBuf>,
    /// Where a downloaded Node lives (under the app data dir).
    pub downloads: PathBuf,
    pub dist: String,
    pub version: &'static str,
    /// The pinned release for this platform; none where Node publishes no build.
    pub release: Option<Release>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tools {
    pub launcher: Launcher,
    pub node_version: (u32, u32, u32),
}

const DOWNLOADING: &str = "Getting Cloudflare's tools ready (downloading Node.js)…";
const INSTALL_NODE: &str = "Quick connect setup needs Node.js 22 or newer (nodejs.org). Install it, then try again.";

/// What `node --version` prints, bounded to a few seconds; none when it cannot be run.
fn system_version(node: &std::path::Path, cancel: &AtomicBool) -> Option<String> {
    let run = Run { program: node.to_owned(), args: vec!["--version".into()], cwd: std::env::temp_dir(), env: Vec::new(), input: None, timeout: Duration::from_secs(20) };
    process::run(run, cancel, &mut |_| {}).ok().filter(|finished| finished.code == 0).map(|finished| finished.output)
}

pub(crate) fn prepare(finder: &Finder, cancel: &AtomicBool, progress: &dyn Fn(&str)) -> Result<Tools, Fail> {
    let printed = finder.node.as_deref().filter(|_| finder.npx.is_some()).and_then(|node| system_version(node, cancel));
    let found = printed.as_deref().and_then(node::parse_version);
    if let (Some(version), Some(npx)) = (found.filter(|version| node::usable(*version)), &finder.npx) {
        return Ok(Tools { launcher: Launcher::System { npx: npx.clone() }, node_version: version });
    }
    // Missing or too old: the pinned download stands in, so the person installs nothing.
    let too_old = printed.as_deref().filter(|_| found.is_some()).map(|printed| format!("Found Node.js {}.", printed.trim()));
    let install = |details: Option<String>| Fail { message: INSTALL_NODE.into(), details, action: Some("install-node") };
    let Some(release) = finder.release else { return Err(install(too_old)) };
    let version = node::parse_version(finder.version).ok_or_else(|| install(None))?;
    let installed = crate::cloud::http::block(node::ensure(&finder.downloads, &finder.dist, finder.version, release, || progress(DOWNLOADING)))
        .map_err(|error| install(Some(match &too_old { Some(found) => format!("{found} {error}"), None => error })))?;
    Ok(Tools { launcher: Launcher::Downloaded(installed), node_version: version })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pandacode-qc-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn finder(dir: &std::path::Path, node: Option<PathBuf>, npx: bool) -> Finder {
        // No server listens here and the checksum is the marker: a download can only fail.
        Finder { node, npx: npx.then(|| script(dir, "npx", "exit 0")), downloads: dir.join("node"), dist: "http://127.0.0.1:9/dist".into(), version: "22.99.0", release: Some(Release { target: "linux-x64", archive: "tar.gz", sha256: node::CHECKSUM_NOT_RECORDED }) }
    }

    #[test]
    fn the_machines_node_is_used_from_version_22() {
        let dir = temp();
        for (printed, version) in [("v22.0.0", (22, 0, 0)), ("v24.3.1", (24, 3, 1))] {
            let node = script(&dir, "node", &format!("echo {printed}"));
            let tools = prepare(&finder(&dir, Some(node), true), &AtomicBool::new(false), &|_| panic!("nothing is downloaded")).unwrap();
            assert_eq!(tools, Tools { launcher: Launcher::System { npx: dir.join("npx") }, node_version: version });
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_old_missing_or_broken_node_falls_back_to_the_pinned_download() {
        let dir = temp();
        let old = script(&dir, "node-old", "echo v20.19.4");
        let broken = script(&dir, "node-broken", "echo boom >&2; exit 1");
        let odd = script(&dir, "node-odd", "echo nightly");
        let cases = [(Some(old), true, Some("Found Node.js v20.19.4.")), (Some(broken), true, None), (Some(odd), true, None), (None, false, None), (Some(script(&dir, "node-new", "echo v22.1.0")), false, None)];
        for (node, npx, found) in cases {
            // Here the download itself is refused (no recorded checksum), which is what a
            // build without real checksums must do: say how to fix it, run nothing.
            let failure = prepare(&finder(&dir, node, npx), &AtomicBool::new(false), &|_| panic!("refused before downloading")).unwrap_err();
            assert_eq!((failure.message.as_str(), failure.action), (INSTALL_NODE, Some("install-node")));
            let details = failure.details.unwrap();
            assert!(details.ends_with(node::CHECKSUM_MISSING), "{details}");
            assert_eq!(details.starts_with("Found Node.js"), found.is_some(), "{details}");
            if let Some(found) = found {
                assert!(details.starts_with(found), "{details}");
            }
        }
        assert!(!dir.join("node").exists());
        // A platform Node publishes nothing for cannot download at all.
        let failure = prepare(&Finder { release: None, ..finder(&dir, None, false) }, &AtomicBool::new(false), &|_| {}).unwrap_err();
        assert_eq!((failure.action, failure.details), (Some("install-node"), None));
        let _ = std::fs::remove_dir_all(dir);
    }
}
