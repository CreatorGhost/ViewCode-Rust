//! The Node.js that runs Cloudflare's `wrangler` for Quick connect. A Node 22 or newer the
//! user installed is used as it is. Without one, a pinned Node release is downloaded from
//! nodejs.org into the app data dir, checked against the SHA-256 compiled in here before a
//! byte of it is unpacked or run, and reused afterwards. Nothing here runs or downloads
//! anything until the user starts Quick connect setup.
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// wrangler 4 refuses older Node.
pub(crate) const MIN_NODE_MAJOR: u32 = 22;
/// The release downloaded when the machine has no usable Node. Bump it together with
/// every checksum in `RELEASES`.
pub(crate) const PINNED_NODE: &str = "22.23.4";
pub(crate) const NODE_DIST: &str = "https://nodejs.org/dist";
/// Node archives are 30–50 MB; anything far larger is not one.
const MAX_ARCHIVE_BYTES: u64 = 160 * 1024 * 1024;

/// Marks a checksum that still has to be copied from nodejs.org's signed
/// `SHASUMS256.txt` for `PINNED_NODE`. It can never equal a SHA-256, so a build that still
/// carries it refuses to download instead of running an unverified Node
/// (`a_placeholder_checksum_refuses_before_any_request`), and the ignored release gate
/// `every_pinned_checksum_is_filled_in` fails until all six are real.
pub(crate) const CHECKSUM_NOT_RECORDED: &str = "CHECKSUM-NOT-RECORDED-copy-it-from-SHASUMS256.txt";

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Release {
    /// nodejs.org's name for the platform and CPU, as in `node-v22.x.y-<target>.<archive>`.
    pub target: &'static str,
    pub archive: &'static str,
    pub sha256: &'static str,
}

pub(crate) const RELEASES: [Release; 6] = [
    Release { target: "darwin-arm64", archive: "tar.gz", sha256: CHECKSUM_NOT_RECORDED },
    Release { target: "darwin-x64", archive: "tar.gz", sha256: CHECKSUM_NOT_RECORDED },
    Release { target: "linux-x64", archive: "tar.gz", sha256: CHECKSUM_NOT_RECORDED },
    Release { target: "linux-arm64", archive: "tar.gz", sha256: CHECKSUM_NOT_RECORDED },
    Release { target: "win-x64", archive: "zip", sha256: CHECKSUM_NOT_RECORDED },
    Release { target: "win-arm64", archive: "zip", sha256: CHECKSUM_NOT_RECORDED },
];

/// This build's nodejs.org target, or none on a platform Node publishes nothing for.
pub(crate) fn target() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("windows", "x86_64") => "win-x64",
        ("windows", "aarch64") => "win-arm64",
        _ => return None,
    })
}

pub(crate) fn release_for(target: &str) -> Option<Release> {
    RELEASES.iter().copied().find(|release| release.target == target)
}

/// `v22.15.1` → (22, 15, 1); none when unreadable.
pub(crate) fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let text = text.trim().lines().next()?.trim();
    let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
    let mut number = || parts.next()?.parse::<u32>().ok();
    let version = (number()?, number()?, number()?);
    parts.next().is_none().then_some(version)
}

/// Whether an installed Node can run wrangler.
pub(crate) fn usable(version: (u32, u32, u32)) -> bool {
    version.0 >= MIN_NODE_MAJOR
}

/// `--use-system-ca` exists (and is allowed in NODE_OPTIONS) from Node 22.15 / 23.8. With it
/// a TLS-inspecting corporate proxy the OS trusts does not break wrangler.
pub(crate) fn supports_system_ca(version: (u32, u32, u32)) -> bool {
    version.0 > 23 || (version.0 == 23 && version.1 >= 8) || (version.0 == 22 && version.1 >= 15)
}

pub(crate) fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hasher.update(&buffer[..size]);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

const CHECKSUM_FAILED: &str = "The Node.js download did not match its checksum, so it was not used. Try again, or install Node.js 22 or newer (nodejs.org).";
pub(crate) const CHECKSUM_MISSING: &str = "This build of the app cannot download Node.js by itself. Install Node.js 22 or newer (nodejs.org), then try again.";

/// Ok only when the file's SHA-256 is exactly `expected`.
pub(crate) fn verify(path: &Path, expected: &str) -> crate::Result<()> {
    if !is_sha256(expected) {
        return Err(CHECKSUM_MISSING.into());
    }
    let actual = sha256_file(path).map_err(|_| "The Node.js download could not be read.".to_string())?;
    if actual == expected { Ok(()) } else { Err(CHECKSUM_FAILED.into()) }
}

/// A Node that is ready to run: the binary, npm's `npx-cli.js` and the folder to put first
/// on PATH (wrangler's shebang and npm's install scripts start `node` by name).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Installed {
    pub node: PathBuf,
    pub npx_cli: PathBuf,
    pub bin: PathBuf,
}

fn layout(root: &Path, windows: bool) -> Installed {
    if windows {
        Installed { node: root.join("node.exe"), npx_cli: root.join("node_modules").join("npm").join("bin").join("npx-cli.js"), bin: root.to_owned() }
    } else {
        Installed { node: root.join("bin").join("node"), npx_cli: root.join("lib").join("node_modules").join("npm").join("bin").join("npx-cli.js"), bin: root.join("bin") }
    }
}

fn folder(version: &str, target: &str) -> String {
    format!("node-v{version}-{target}")
}

const VERIFIED: &str = ".pandacode-verified";

/// The already-downloaded release under `dir`, if it was verified against this checksum.
pub(crate) fn installed(dir: &Path, version: &str, release: Release) -> Option<Installed> {
    let root = dir.join(folder(version, release.target));
    let found = layout(&root, release.target.starts_with("win-"));
    (is_sha256(release.sha256) && std::fs::read_to_string(root.join(VERIFIED)).ok()?.trim() == release.sha256 && found.node.is_file() && found.npx_cli.is_file()).then_some(found)
}

/// Unpacks a release archive into `dest` (which must not exist yet). Entries that would
/// land outside it are refused by the archive readers.
pub(crate) fn unpack(archive: &Path, kind: &str, dest: &Path) -> crate::Result<()> {
    let failed = |error: &dyn std::fmt::Display| format!("Could not unpack Node.js: {error}");
    std::fs::create_dir_all(dest).map_err(|e| failed(&e))?;
    let file = std::fs::File::open(archive).map_err(|e| failed(&e))?;
    match kind {
        "tar.gz" => {
            let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::io::BufReader::new(file)));
            tar.set_preserve_permissions(true);
            tar.set_overwrite(false);
            tar.unpack(dest).map_err(|e| failed(&e))
        }
        "zip" => zip::ZipArchive::new(file).and_then(|mut zip| zip.extract(dest)).map_err(|e| failed(&e)),
        other => Err(format!("Could not unpack Node.js: unknown archive type {other}.")),
    }
}

/// Streams `url` into `dest`, refusing anything too large. A body cut short is an error.
async fn fetch(url: &str, dest: &Path) -> crate::Result<()> {
    use std::io::Write;
    let client = crate::cloud::http::client(std::time::Duration::from_secs(600))?;
    let unreachable = "Could not download Node.js (nodejs.org). Check your connection, or install Node.js 22 or newer yourself.";
    let mut response = client.get(url).send().await.map_err(|_| unreachable.to_string())?;
    if !response.status().is_success() {
        return Err(format!("nodejs.org answered HTTP {} for the Node.js download.", response.status().as_u16()));
    }
    if response.content_length().is_some_and(|length| length > MAX_ARCHIVE_BYTES) {
        return Err("The Node.js download is larger than expected, so it was not used.".into());
    }
    let mut file = std::fs::File::create(dest).map_err(|e| format!("Could not save the Node.js download: {e}"))?;
    let mut total = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(|_| "The Node.js download was cut off. Try again.".to_string())? {
        total += chunk.len() as u64;
        if total > MAX_ARCHIVE_BYTES {
            return Err("The Node.js download is larger than expected, so it was not used.".into());
        }
        file.write_all(&chunk).map_err(|e| format!("Could not save the Node.js download: {e}"))?;
    }
    file.sync_all().map_err(|e| format!("Could not save the Node.js download: {e}"))
}

/// The pinned release under `dir`, downloading it once. Order matters: the checksum is
/// known to be real before any request, and the archive is verified before it is unpacked.
pub(crate) async fn ensure(dir: &Path, dist: &str, version: &str, release: Release, downloading: impl FnOnce()) -> crate::Result<Installed> {
    if let Some(found) = installed(dir, version, release) {
        return Ok(found);
    }
    if !is_sha256(release.sha256) {
        return Err(CHECKSUM_MISSING.into());
    }
    downloading();
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not prepare a folder for Node.js: {e}"))?;
    let name = folder(version, release.target);
    let scratch = dir.join(format!(".download-{}", uuid::Uuid::new_v4()));
    let result = async {
        std::fs::create_dir_all(&scratch).map_err(|e| format!("Could not prepare a folder for Node.js: {e}"))?;
        let archive = scratch.join(format!("{name}.{}", release.archive));
        fetch(&format!("{}/v{version}/{name}.{}", dist.trim_end_matches('/'), release.archive), &archive).await?;
        verify(&archive, release.sha256)?;
        let unpacked = scratch.join("unpacked");
        unpack(&archive, release.archive, &unpacked)?;
        let root = unpacked.join(&name);
        let found = layout(&root, release.target.starts_with("win-"));
        if !found.node.is_file() || !found.npx_cli.is_file() {
            return Err("The Node.js download does not look like a Node.js release, so it was not used.".to_string());
        }
        std::fs::write(root.join(VERIFIED), release.sha256).map_err(|e| format!("Could not finish installing Node.js: {e}"))?;
        let dest = dir.join(&name);
        // A leftover from an interrupted earlier attempt was never marked verified.
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&root, &dest).map_err(|e| format!("Could not finish installing Node.js: {e}"))
    }
    .await;
    let _ = std::fs::remove_dir_all(&scratch);
    result?;
    installed(dir, version, release).ok_or_else(|| "Could not finish installing Node.js.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pandacode-qc-node-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A tiny archive shaped like a Node release: `bin/node` and npm's `npx-cli.js`.
    fn fake_release(version: &str, target: &str) -> Vec<u8> {
        let name = folder(version, target);
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, body, mode) in [("bin/node", format!("#!/bin/sh\necho v{version}\n"), 0o755), ("lib/node_modules/npm/bin/npx-cli.js", "// npx\n".to_string(), 0o644)] {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(mode);
            header.set_cksum();
            tar.append_data(&mut header, format!("{name}/{path}"), body.as_bytes()).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    fn sha(bytes: &[u8]) -> &'static str {
        let hex: String = Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect();
        Box::leak(hex.into_boxed_str())
    }

    /// A loopback server for one download path. `declared` is the Content-Length it claims,
    /// so a body shorter than that is a connection cut off mid-download.
    fn serve(body: Vec<u8>, declared: usize, status: u16) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/dist", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = std::io::BufRead::read_line(&mut reader, &mut line);
                log.lock().unwrap().push(line.split_whitespace().nth(1).unwrap_or("").to_string());
                loop {
                    let mut header = String::new();
                    if std::io::BufRead::read_line(&mut reader, &mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                }
                let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/gzip\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(&body);
            }
        });
        (base, seen)
    }

    fn release(sha256: &'static str) -> Release {
        Release { target: "linux-x64", archive: "tar.gz", sha256 }
    }

    fn ensure_blocking(dir: &Path, dist: &str, release: Release) -> (crate::Result<Installed>, bool) {
        let mut announced = false;
        let result = crate::cloud::http::block(ensure(dir, dist, "22.99.0", release, || announced = true));
        (result, announced)
    }

    #[test]
    fn versions_are_parsed_and_gated_at_22() {
        assert_eq!(parse_version("v22.15.1\n"), Some((22, 15, 1)));
        assert_eq!(parse_version("24.0.0"), Some((24, 0, 0)));
        for bad in ["", "v22", "v22.1", "node", "v22.1.x", "v22.1.2.3", "v-1.0.0"] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
        assert!(!usable((20, 19, 4)) && !usable((21, 7, 3)) && usable((22, 0, 0)) && usable((24, 1, 0)));
        assert!(!supports_system_ca((22, 14, 0)) && supports_system_ca((22, 15, 0)) && !supports_system_ca((23, 7, 0)) && supports_system_ca((23, 8, 0)) && supports_system_ca((24, 0, 0)));
        assert!(usable(parse_version(PINNED_NODE).unwrap()) && supports_system_ca(parse_version(PINNED_NODE).unwrap()));
    }

    #[test]
    fn every_supported_platform_has_one_pinned_release() {
        for target in ["darwin-arm64", "darwin-x64", "linux-x64", "linux-arm64", "win-x64", "win-arm64"] {
            let release = release_for(target).unwrap();
            assert_eq!(release.archive, if target.starts_with("win-") { "zip" } else { "tar.gz" });
            // A real digest or the marker, never something in between that could pass for one.
            assert!(is_sha256(release.sha256) || release.sha256 == CHECKSUM_NOT_RECORDED, "{target}");
        }
        assert_eq!(RELEASES.len(), 6);
        assert!(release_for("sunos-x64").is_none());
        assert!(release_for(target().expect("CI builds only supported targets")).is_some());
        assert!(!is_sha256(CHECKSUM_NOT_RECORDED) && !is_sha256(&"A".repeat(64)) && is_sha256(&"a1".repeat(32)));
    }

    /// The release gate: run with `--ignored` before shipping Quick connect. It fails while
    /// any checksum is still the marker.
    #[test]
    #[ignore = "release gate: fill RELEASES from nodejs.org's SHASUMS256.txt for PINNED_NODE"]
    fn every_pinned_checksum_is_filled_in() {
        for release in RELEASES {
            assert!(is_sha256(release.sha256), "{} has no recorded checksum", release.target);
        }
    }

    #[test]
    fn checksum_verification_accepts_only_the_exact_digest() {
        let dir = temp();
        let file = dir.join("archive");
        std::fs::write(&file, b"node").unwrap();
        assert_eq!(sha256_file(&file).unwrap(), sha(b"node"));
        assert_eq!(verify(&file, sha(b"node")), Ok(()));
        assert_eq!(verify(&file, sha(b"nodf")), Err(CHECKSUM_FAILED.into()));
        assert_eq!(verify(&file, CHECKSUM_NOT_RECORDED), Err(CHECKSUM_MISSING.into()));
        assert_eq!(verify(&file, &sha(b"node").to_uppercase()), Err(CHECKSUM_MISSING.into()));
        assert!(verify(&dir.join("missing"), sha(b"node")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_placeholder_checksum_refuses_before_any_request() {
        let dir = temp();
        let (dist, seen) = serve(fake_release("22.99.0", "linux-x64"), 0, 200);
        let (result, announced) = ensure_blocking(&dir, &dist, release(CHECKSUM_NOT_RECORDED));
        assert_eq!(result, Err(CHECKSUM_MISSING.into()));
        assert!(!announced && seen.lock().unwrap().is_empty());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_good_download_is_verified_unpacked_and_reused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp();
        let archive = fake_release("22.99.0", "linux-x64");
        let (dist, seen) = serve(archive.clone(), archive.len(), 200);
        let (result, announced) = ensure_blocking(&dir, &dist, release(sha(&archive)));
        let found = result.unwrap();
        assert!(announced);
        assert_eq!(seen.lock().unwrap().as_slice(), ["/dist/v22.99.0/node-v22.99.0-linux-x64.tar.gz"]);
        assert_eq!(found.node, dir.join("node-v22.99.0-linux-x64/bin/node"));
        assert!(found.npx_cli.ends_with("lib/node_modules/npm/bin/npx-cli.js") && found.bin.ends_with("bin"));
        assert_eq!(std::fs::metadata(&found.node).unwrap().permissions().mode() & 0o111, 0o111);
        // Only the release folder is left: no archive, no scratch folder.
        assert_eq!(std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().file_name()).collect::<Vec<_>>(), ["node-v22.99.0-linux-x64"]);
        // The second time nothing is requested.
        let (again, announced) = ensure_blocking(&dir, &dist, release(sha(&archive)));
        assert_eq!((again.unwrap(), announced, seen.lock().unwrap().len()), (found, false, 1));
        // A folder verified against another checksum (an older pin) is not trusted.
        assert!(installed(&dir, "22.99.0", release(sha(b"other"))).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_bad_checksum_or_a_truncated_download_installs_nothing() {
        let archive = fake_release("22.99.0", "linux-x64");
        let tampered = {
            let mut bytes = archive.clone();
            let last = bytes.len() - 1;
            bytes[last] ^= 1;
            bytes
        };
        let cases: [(Vec<u8>, usize, u16, &str); 4] = [
            // The right length, the wrong bytes.
            (tampered.clone(), tampered.len(), 200, "did not match its checksum"),
            // The connection drops half way through the declared length.
            (archive[..archive.len() / 2].to_vec(), archive.len(), 200, "cut off"),
            // A complete response that is only the first half of the archive.
            (archive[..archive.len() / 2].to_vec(), archive.len() / 2, 200, "did not match its checksum"),
            (b"not found".to_vec(), 9, 404, "HTTP 404"),
        ];
        for (body, declared, status, expected) in cases {
            let dir = temp();
            let (dist, _) = serve(body, declared, status);
            let (result, _) = ensure_blocking(&dir, &dist, release(sha(&archive)));
            let error = result.unwrap_err();
            assert!(error.contains(expected), "{error}");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "{expected}");
            assert!(installed(&dir, "22.99.0", release(sha(&archive))).is_none());
            let _ = std::fs::remove_dir_all(dir);
        }
        // Nothing listening at all.
        let dir = temp();
        let (result, _) = ensure_blocking(&dir, "http://127.0.0.1:9/dist", release(sha(&archive)));
        assert!(result.unwrap_err().contains("Could not download Node.js"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_archive_that_is_not_a_node_release_is_refused_after_verification() {
        let dir = temp();
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "something-else/readme", &b"hi"[..]).unwrap();
        let archive = tar.into_inner().unwrap().finish().unwrap();
        let (dist, _) = serve(archive.clone(), archive.len(), 200);
        let (result, _) = ensure_blocking(&dir, &dist, release(sha(&archive)));
        assert!(result.unwrap_err().contains("does not look like a Node.js release"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn windows_releases_are_zip_archives_with_node_at_the_top() {
        let dir = temp();
        let path = dir.join("node.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in [("node-v22.99.0-win-x64/node.exe", "MZ"), ("node-v22.99.0-win-x64/node_modules/npm/bin/npx-cli.js", "// npx")] {
            zip.start_file(name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        unpack(&path, "zip", &dir.join("out")).unwrap();
        let found = layout(&dir.join("out").join("node-v22.99.0-win-x64"), true);
        assert!(found.node.is_file() && found.npx_cli.is_file() && found.bin == dir.join("out").join("node-v22.99.0-win-x64"));
        assert!(unpack(&path, "rar", &dir.join("other")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
