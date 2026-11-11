//! One bounded run of an external tool for Quick connect (`node`, `npx wrangler`), in the
//! style of `workspace::run_process`: its own process group (a Windows job), a deadline, a
//! bounded tail of output. Unlike that runner it interleaves stdout and stderr as printed,
//! hands each chunk to a callback while the tool runs (the sign-in link appears before the
//! tool exits) and stops when the caller cancels.
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The tail of output kept; the start of a wrangler run is banners.
pub(crate) const MAX_OUTPUT: usize = 64 * 1024;

pub(crate) struct Run {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// Added to (or replacing entries of) this process's environment.
    pub env: Vec<(OsString, OsString)>,
    /// Written to stdin, which then closes. The only way a secret reaches a tool.
    pub input: Option<Vec<u8>>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Finished {
    pub code: i32,
    /// stdout and stderr interleaved, as printed (the last `MAX_OUTPUT` bytes).
    pub output: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Failure {
    /// The program could not be started at all.
    Start(String),
    TimedOut,
    Cancelled,
}

fn stop(child: &mut std::process::Child) {
    crate::workspace::stop_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

fn keep_tail(output: &mut String, text: &str) {
    output.push_str(text);
    if output.len() > MAX_OUTPUT {
        let mut cut = output.len() - MAX_OUTPUT;
        while !output.is_char_boundary(cut) {
            cut += 1;
        }
        output.drain(..cut);
    }
}

pub(crate) fn run(run: Run, cancel: &AtomicBool, on_output: &mut dyn FnMut(&str)) -> Result<Finished, Failure> {
    let mut command = Command::new(&run.program);
    command.args(&run.args).current_dir(&run.cwd).envs(run.env.iter().map(|(key, value)| (key, value)));
    crate::child_env::sanitize(&mut command);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, 0x08000000);
    command.stdin(if run.input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| Failure::Start(error.to_string()))?;
    #[cfg(windows)]
    let _job = crate::providers::windows_job::Job::attach(child.id());
    if let (Some(mut stdin), Some(input)) = (child.stdin.take(), run.input) {
        // A tool that exits without reading gives a broken pipe; its status tells the story.
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let (sender, chunks) = mpsc::channel::<Vec<u8>>();
    let pipes: [Option<Box<dyn Read + Send>>; 2] = [child.stdout.take().map(|pipe| Box::new(pipe) as _), child.stderr.take().map(|pipe| Box::new(pipe) as _)];
    for mut pipe in pipes.into_iter().flatten() {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut buffer = [0; 8192];
            while let Ok(size) = pipe.read(&mut buffer) {
                if size == 0 || sender.send(buffer[..size].to_vec()).is_err() {
                    break;
                }
            }
        });
    }
    drop(sender);

    let deadline = Instant::now() + run.timeout;
    let mut output = String::new();
    let mut take = |chunk: Vec<u8>, output: &mut String| {
        let text = String::from_utf8_lossy(&chunk);
        keep_tail(output, &text);
        on_output(&text);
    };
    let status = loop {
        match chunks.recv_timeout(Duration::from_millis(20)) {
            Ok(chunk) => take(chunk, &mut output),
            Err(mpsc::RecvTimeoutError::Timeout) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
        if cancel.load(Ordering::SeqCst) {
            stop(&mut child);
            return Err(Failure::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {}
            Ok(None) => {
                stop(&mut child);
                return Err(Failure::TimedOut);
            }
            Err(error) => {
                stop(&mut child);
                return Err(Failure::Start(error.to_string()));
            }
        }
    };
    // The tool has exited: read what it left in the pipes, but never wait on a grandchild
    // that inherited them.
    let drained = Instant::now() + Duration::from_secs(1);
    loop {
        match chunks.recv_timeout(drained.saturating_duration_since(Instant::now())) {
            Ok(chunk) => take(chunk, &mut output),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                crate::workspace::stop_process_group(child.id());
                break;
            }
        }
    }
    Ok(Finished { code: status.code().unwrap_or(1), output })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str, input: Option<&str>, timeout: Duration) -> Run {
        Run { program: "/bin/sh".into(), args: vec!["-c".into(), script.into()], cwd: std::env::temp_dir(), env: vec![("QC_TEST".into(), "yes".into())], input: input.map(|text| text.as_bytes().to_vec()), timeout }
    }

    #[test]
    fn output_is_interleaved_streamed_and_bounded() {
        let mut seen = String::new();
        let done = run(sh("echo one; echo two >&2; read line; echo \"got $line $QC_TEST\"; exit 3", Some("stdin\n"), Duration::from_secs(20)), &AtomicBool::new(false), &mut |text| seen.push_str(text)).unwrap();
        assert_eq!(done.code, 3);
        assert!(done.output.contains("one") && done.output.contains("two") && done.output.contains("got stdin yes"));
        assert_eq!(seen, done.output);
        let flood = run(sh("head -c 300000 /dev/zero | tr '\\0' 'x'; echo END", None, Duration::from_secs(20)), &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(flood.output.len() <= MAX_OUTPUT && flood.output.ends_with("END\n"));
    }

    #[test]
    fn a_run_that_never_ends_is_stopped_at_its_deadline_or_on_cancel() {
        let started = Instant::now();
        assert_eq!(run(sh("sleep 30", None, Duration::from_millis(200)), &AtomicBool::new(false), &mut |_| {}), Err(Failure::TimedOut));
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.store(true, Ordering::SeqCst);
        });
        assert_eq!(run(sh("echo waiting; sleep 30", None, Duration::from_secs(60)), &cancel, &mut |_| {}), Err(Failure::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(matches!(run(Run { program: "/nonexistent/quick-connect-tool".into(), ..sh("", None, Duration::from_secs(1)) }, &AtomicBool::new(false), &mut |_| {}), Err(Failure::Start(_))));
    }
}
