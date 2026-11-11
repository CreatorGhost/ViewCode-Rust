//! ViewCode's computer-use driver, built on agent-desktop. The server spawns
//! it, sends one JSON request per line on stdin and reads one reply per line
//! on stdout. See `apps/server/src/computerUse/ComputerDriverClient.ts`.

use std::io::{BufRead, StdinLock, StdoutLock, Write};

mod driver;
mod observe;
mod platform;

const NONCE_ENV: &str = "VIEWCODE_COMPUTER_DRIVER_NONCE";
const EPOCH_ENV: &str = "VIEWCODE_COMPUTER_DRIVER_EPOCH";

struct Stdio<'a> {
    input: StdinLock<'a>,
    output: StdoutLock<'a>,
}

impl Stdio<'_> {
    fn write(&mut self, line: &str) -> bool {
        writeln!(self.output, "{line}")
            .and_then(|()| self.output.flush())
            .is_ok()
    }

    fn read(&mut self) -> Option<String> {
        let mut line = String::new();
        match self.input.read_line(&mut line) {
            Ok(read) if read > 0 => Some(line),
            _ => None,
        }
    }
}

impl driver::Channel for Stdio<'_> {
    fn ask(&mut self, message: &serde_json::Value) -> Option<String> {
        if self.write(&message.to_string()) {
            self.read()
        } else {
            None
        }
    }
}

fn main() {
    // The nonce only stops casual direct use of this helper; the server is
    // the computer-use gate.
    let (Ok(nonce), Ok(epoch)) = (std::env::var(NONCE_ENV), std::env::var(EPOCH_ENV)) else {
        eprintln!("viewcode-computer-driver is started by the ViewCode server.");
        std::process::exit(2);
    };
    if nonce.is_empty() || epoch.is_empty() {
        std::process::exit(2);
    }
    let mut driver = driver::Driver::new(platform::NativeDesktop::new(), nonce, epoch);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut stdio = Stdio {
        input: stdin.lock(),
        output: stdout.lock(),
    };
    while let Some(line) = stdio.read() {
        match driver.handle_line(&line, &mut stdio) {
            Some(driver::Incoming::Reply(reply)) => {
                if !stdio.write(&reply) {
                    break;
                }
            }
            Some(driver::Incoming::Reject) => std::process::exit(2),
            None => {}
        }
    }
}
