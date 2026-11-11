//! `pandacode --pandacode-mcp-bridge`: an MCP stdio server that forwards every
//! JSON-RPC line to the run's loopback MCP host (`mcp_host.rs`) and prints the
//! answer. ViewCode gives every ACP agent its tools this way (`acp-mcp-bridge`):
//! stdio is the one MCP transport every ACP agent must support, while agents
//! that advertise HTTP servers often drop them. The harness starts this as a
//! child; it talks to 127.0.0.1 only and ends when its stdin closes.
use serde_json::{json, Value};
use std::{
    io::{BufRead, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};

pub(crate) const FLAG: &str = "--pandacode-mcp-bridge";
/// `127.0.0.1:<port>` of the run's MCP host.
pub(crate) const AUTHORITY: &str = "PANDACODE_MCP_BRIDGE_AUTHORITY";
/// The run's `Bearer <token>` header value.
pub(crate) const AUTHORIZATION: &str = "PANDACODE_MCP_BRIDGE_AUTHORIZATION";
const LINE_LIMIT: u64 = 16 * 1024 * 1024;
const RESPONSE_LIMIT: u64 = 64 * 1024 * 1024;

/// Runs the bridge when the process was started as one. Never returns to the app.
pub fn handle_mcp_bridge() -> bool {
    if std::env::args().nth(1).as_deref() != Some(FLAG) {
        return false;
    }
    let authority = std::env::var(AUTHORITY).unwrap_or_default();
    let authorization = std::env::var(AUTHORIZATION).unwrap_or_default();
    // Calls still in flight belong to a client that is gone; the host ends them
    // when their connections close with this process.
    let _calls = run(std::io::stdin().lock(), std::io::stdout(), &authority, &authorization);
    std::process::exit(0);
}

/// Forwards each line until `input` ends. Calls run on their own threads so a
/// tool waiting for approval never holds up the others; the handles are returned
/// for callers that want every answer written.
pub(crate) fn run(mut input: impl BufRead, output: impl Write + Send + 'static, authority: &str, authorization: &str) -> Vec<std::thread::JoinHandle<()>> {
    let output = Arc::new(Mutex::new(output));
    let mut calls = Vec::new();
    loop {
        let mut line = Vec::new();
        match (&mut input).take(LINE_LIMIT + 1).read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.len() as u64 > LINE_LIMIT {
            break;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let (output, authority, authorization) = (output.clone(), authority.to_owned(), authorization.to_owned());
        calls.retain(|call: &std::thread::JoinHandle<()>| !call.is_finished());
        calls.push(std::thread::spawn(move || {
            let Some(answer) = forward(&authority, &authorization, &line) else { return };
            if let Ok(mut output) = output.lock() {
                let _ = output.write_all(&answer).and_then(|()| output.write_all(b"\n")).and_then(|()| output.flush());
            }
        }));
    }
    calls
}

/// One request to the MCP host. `None` when there is nothing to print: a
/// notification, or an answer to one. A request the host cannot be reached for
/// gets a JSON-RPC error, so the agent's call fails instead of waiting.
fn forward(authority: &str, authorization: &str, line: &[u8]) -> Option<Vec<u8>> {
    let body = line.trim_ascii();
    let unavailable = || {
        let request: Value = serde_json::from_slice(body).ok()?;
        let id = request.get("id").filter(|id| !id.is_null())?;
        request.get("method")?;
        serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"The app's tools are no longer available for this run."}})).ok()
    };
    let Some(answer) = post(authority, authorization, body) else { return unavailable() };
    let split = answer.windows(4).position(|window| window == b"\r\n\r\n")?;
    let status = answer[..split].split(|byte| *byte == b' ').nth(1).and_then(|code| std::str::from_utf8(code).ok());
    let content = answer[split + 4..].trim_ascii();
    match status {
        Some("200") if !content.is_empty() => Some(content.to_vec()),
        Some("200" | "202") => None,
        _ => unavailable(),
    }
}

/// The host answers one request per connection and closes it. Only a loopback
/// address is ever dialed.
fn post(authority: &str, authorization: &str, body: &[u8]) -> Option<Vec<u8>> {
    let address: SocketAddr = authority.parse().ok().filter(|address: &SocketAddr| address.ip().is_loopback())?;
    if authorization.contains(['\r', '\n']) {
        return None;
    }
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5)).ok()?;
    let head = format!("POST /mcp HTTP/1.1\r\nHost: {authority}\r\nAuthorization: {authorization}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    stream.write_all(head.as_bytes()).ok()?;
    stream.write_all(body).ok()?;
    // No read deadline: a call can wait on the user's approval for minutes.
    let mut answer = Vec::new();
    stream.take(RESPONSE_LIMIT).read_to_end(&mut answer).ok()?;
    Some(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A stand-in for the MCP host: answers each connection with `reply(request)`.
    fn host(reply: fn(&str) -> String) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let served = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut request = Vec::new();
                let mut chunk = [0; 1024];
                let text = loop {
                    let count = stream.read(&mut chunk).unwrap();
                    request.extend_from_slice(&chunk[..count]);
                    let text = String::from_utf8_lossy(&request).into_owned();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length: usize = head.lines().find_map(|line| line.strip_prefix("Content-Length: ")).unwrap().parse().unwrap();
                        if body.len() >= length {
                            break text;
                        }
                    }
                };
                stream.write_all(reply(&text).as_bytes()).unwrap();
                let done = text.contains("\"stop\"");
                seen.push(text);
                if done {
                    break;
                }
            }
            seen
        });
        (authority, served)
    }

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn ok(body: &str) -> String {
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    #[test]
    fn requests_are_posted_with_the_runs_token_and_answers_come_back_one_per_line() {
        let (authority, served) = host(|request| {
            if request.contains("notifications/initialized") {
                "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()
            } else if request.contains("tools/list") {
                ok(r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}"#)
            } else {
                ok(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#)
            }
        });
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\r\n",
        );
        let sink = Sink::default();
        for call in run(input.as_bytes(), sink.clone(), &authority, "Bearer run-token") {
            call.join().unwrap();
        }
        for call in run("{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"stop\"}\n".as_bytes(), sink.clone(), &authority, "Bearer run-token") {
            call.join().unwrap();
        }
        let seen = served.join().unwrap();
        assert_eq!(seen.len(), 3);
        for request in &seen {
            assert!(request.starts_with("POST /mcp HTTP/1.1\r\n"), "{request}");
            assert!(request.contains(&format!("Host: {authority}\r\n")));
            assert!(request.contains("Authorization: Bearer run-token\r\n"));
            assert!(request.contains("Content-Type: application/json\r\n"));
        }
        // The notification prints nothing; each request prints its answer on one line.
        let printed = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        let mut lines: Vec<&str> = printed.lines().collect();
        lines.sort();
        assert_eq!(lines, [r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}"#, r#"{"jsonrpc":"2.0","id":3,"result":{}}"#]);
    }

    #[test]
    fn a_host_that_is_gone_or_refuses_fails_the_call_instead_of_leaving_it_waiting() {
        // Nothing listens on this port any more.
        let gone = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
        let request = br#"{"jsonrpc":"2.0","id":"call-1","method":"tools/call","params":{}}"#;
        let answer: Value = serde_json::from_slice(&forward(&gone, "Bearer x", request).unwrap()).unwrap();
        assert_eq!(answer["id"], "call-1");
        assert_eq!(answer["error"]["code"], -32000);
        // A notification has nobody to tell.
        assert_eq!(forward(&gone, "Bearer x", br#"{"jsonrpc":"2.0","method":"notifications/cancelled"}"#), None);
        // A stale token is refused by the host (401): the call fails the same way.
        let (authority, served) = host(|_| "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into());
        let request = br#"{"jsonrpc":"2.0","id":7,"method":"stop"}"#;
        let answer: Value = serde_json::from_slice(&forward(&authority, "Bearer old", request).unwrap()).unwrap();
        assert_eq!((answer["id"].as_u64(), answer["error"]["code"].as_i64()), (Some(7), Some(-32000)));
        served.join().unwrap();
    }

    #[test]
    fn only_loopback_is_dialed_and_headers_cannot_be_injected() {
        assert_eq!(post("93.184.216.34:80", "Bearer x", b"{}"), None);
        assert_eq!(post("example.com:80", "Bearer x", b"{}"), None);
        assert_eq!(post("127.0.0.1:1", "Bearer x\r\nX-Evil: 1", b"{}"), None);
    }
}
