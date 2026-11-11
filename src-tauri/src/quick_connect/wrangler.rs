//! Cloudflare's `wrangler`, run as `npx --yes wrangler@<exact version> …` through the
//! bounded runner in `process.rs`, plus everything that reads its output. wrangler is the
//! only thing that ever holds the Cloudflare sign-in; the app never sees a Cloudflare token.
//! The host secret reaches wrangler on stdin only, and every piece of its output is
//! redacted before it is kept or shown. Nothing here is logged.
use super::node;
use super::process::{self, Failure, Finished, Run};
use regex::Regex;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Pinned exactly: `npx` never picks a newer wrangler on its own. Bump deliberately, and
/// run the ignored live smoke test in `setup.rs` when you do.
pub(crate) const WRANGLER_VERSION: &str = "4.50.0";
pub(crate) const DEFAULT_WORKER_NAME: &str = "pandacode-quick-connect";
/// The Worker secret that holds the host secret (`relay/src/env.ts`).
pub(crate) const HOST_SECRET_NAME: &str = "HOST_SECRET";

/// Every run ends. The first one also downloads wrangler, hence the generous bound.
pub(crate) const RUN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Sign-in waits for the person to approve in the browser.
pub(crate) const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub(crate) fn package() -> String {
    format!("wrangler@{WRANGLER_VERSION}")
}

/// How each run starts.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Launcher {
    /// The machine's own Node 22+: `npx` as found on the widened PATH.
    System { npx: PathBuf },
    /// The downloaded Node (`node.rs`): its `node` runs npm's `npx-cli.js`, with its folder
    /// first on PATH for everything below npx that starts `node` by name.
    Downloaded(node::Installed),
}

#[derive(Clone)]
pub(crate) struct Session {
    pub launcher: Launcher,
    pub node_version: (u32, u32, u32),
    /// The staged Worker: every run's working directory.
    pub cwd: PathBuf,
    /// The Cloudflare account every run acts on, once it is known.
    pub account_id: Option<String>,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub(crate) struct Options<'a> {
    pub input: Option<&'a str>,
    pub timeout: Option<Duration>,
}

/// The command line of one wrangler run.
pub(crate) fn command(launcher: &Launcher, args: &[&str]) -> (PathBuf, Vec<OsString>) {
    let npx_args = ["--yes".into(), package().into()].into_iter().chain(args.iter().map(OsString::from));
    match launcher {
        Launcher::System { npx } => (npx.clone(), npx_args.collect()),
        Launcher::Downloaded(installed) => (installed.node.clone(), std::iter::once(installed.npx_cli.clone().into_os_string()).chain(npx_args).collect()),
    }
}

/// The environment added to every run. `path` and `node_options` are this process's values.
pub(crate) fn environment(launcher: &Launcher, node_version: (u32, u32, u32), account_id: Option<&str>, path: Option<OsString>, node_options: Option<String>) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = [("NO_COLOR", "1"), ("FORCE_COLOR", "0"), ("WRANGLER_SEND_METRICS", "false"), ("npm_config_update_notifier", "false"), ("npm_config_fund", "false")]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
    // A Node before 22.15 rejects the flag in NODE_OPTIONS and would not start at all.
    let inherited = node_options.map(|options| options.trim().to_owned()).filter(|options| !options.is_empty());
    let options: Vec<String> = node::supports_system_ca(node_version).then(|| "--use-system-ca".to_owned()).into_iter().chain(inherited).collect();
    if !options.is_empty() {
        env.push(("NODE_OPTIONS".into(), options.join(" ").into()));
    }
    if let Some(account) = account_id {
        env.push(("CLOUDFLARE_ACCOUNT_ID".into(), account.into()));
    }
    if let Launcher::Downloaded(installed) = launcher {
        let dirs = std::iter::once(installed.bin.clone()).chain(path.iter().flat_map(std::env::split_paths));
        if let Ok(joined) = std::env::join_paths(dirs) {
            env.push(("PATH".into(), joined));
        }
    }
    env
}

impl Session {
    /// Runs `wrangler <args>`. Ok carries the exit code, whatever it is; Err is a run that
    /// could not start, timed out or was cancelled, as one sentence for the person.
    pub(crate) fn run(&self, args: &[&str], options: Options, on_output: &mut dyn FnMut(&str)) -> crate::Result<Finished> {
        let (program, args_os) = command(&self.launcher, args);
        let signing_in = args.first() == Some(&"login");
        let timeout = options.timeout.unwrap_or(if signing_in { SIGN_IN_TIMEOUT } else { RUN_TIMEOUT });
        let run = Run {
            program,
            args: args_os,
            cwd: self.cwd.clone(),
            env: environment(&self.launcher, self.node_version, self.account_id.as_deref(), std::env::var_os("PATH"), std::env::var("NODE_OPTIONS").ok()),
            input: options.input.map(|text| text.as_bytes().to_vec()),
            timeout,
        };
        process::run(run, &self.cancel, on_output).map_err(|failure| match failure {
            Failure::Cancelled => CANCELLED.to_owned(),
            Failure::TimedOut if signing_in => format!("The Cloudflare sign-in was not approved within {} minutes. Try again.", timeout.as_secs().div_ceil(60)),
            Failure::TimedOut => format!("Cloudflare's tool did not finish within {} minutes. Try again.", timeout.as_secs().div_ceil(60)),
            Failure::Start(_) => "Could not start Cloudflare's tool on this computer. Try again.".to_owned(),
        })
    }

    pub(crate) fn for_account(&self, account_id: Option<String>) -> Session {
        Session { account_id: account_id.or_else(|| self.account_id.clone()), ..self.clone() }
    }
}

pub(crate) const CANCELLED: &str = "Quick connect setup was cancelled.";

// ---- Reading wrangler's output ----

fn pattern(cell: &'static OnceLock<Regex>, source: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(source).expect("a constant pattern"))
}

pub(crate) fn strip_ansi(text: &str) -> String {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    pattern(&ANSI, "\u{1b}\\[[0-9;?]*[A-Za-z]").replace_all(text, "").into_owned()
}

/// Removes every secret from text before it is kept or shown.
pub(crate) fn redact(text: &str, secrets: &[String]) -> String {
    secrets.iter().filter(|secret| secret.len() >= 8).fold(strip_ansi(text), |text, secret| text.replace(secret.as_str(), "[redacted]"))
}

/// Cloudflare Worker names: lowercase letters, digits and dashes.
pub(crate) fn is_valid_worker_name(name: &str) -> bool {
    static NAME: OnceLock<Regex> = OnceLock::new();
    pattern(&NAME, "^[a-z0-9](?:[a-z0-9-]{0,52}[a-z0-9])?$").is_match(name)
}

/// The `https://<name>.<subdomain>.workers.dev` address wrangler prints after a deploy.
pub(crate) fn parse_worker_url(output: &str, name: &str) -> Option<String> {
    let found = Regex::new(&format!("(?i)https://{}\\.[a-z0-9-]+\\.workers\\.dev", regex::escape(name))).ok()?.find(output)?;
    Some(found.as_str().to_ascii_lowercase())
}

/// Whether `wrangler whoami` says nobody is signed in. Anything else, including output we
/// do not recognise, counts as signed in, so a changed message cannot loop the login.
pub(crate) fn needs_login(output: &str, code: i32) -> bool {
    static SIGNED_OUT: OnceLock<Regex> = OnceLock::new();
    code != 0 || pattern(&SIGNED_OUT, "(?i)not authenticated|not logged in|you are not").is_match(output)
}

/// wrangler declines to register a workers.dev subdomain when it runs non-interactively.
pub(crate) fn missing_workers_dev_subdomain(output: &str) -> bool {
    static SUBDOMAIN: OnceLock<Regex> = OnceLock::new();
    pattern(&SUBDOMAIN, "(?i)workers\\.dev subdomain").is_match(output)
}

/// `wrangler delete` of a Worker that is already gone: nothing left to delete.
pub(crate) fn already_deleted(output: &str) -> bool {
    static GONE: OnceLock<Regex> = OnceLock::new();
    pattern(&GONE, "(?i)\\[code: 10007\\]|script_not_found|does not exist|could not be found").is_match(output)
}

/// An older wrangler without the device flow.
pub(crate) fn device_login_unsupported(output: &str) -> bool {
    static UNKNOWN: OnceLock<Regex> = OnceLock::new();
    pattern(&UNKNOWN, "(?i)unknown arguments?:.*\\bdevice\\b").is_match(output)
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignInPrompt {
    /// The page to open, e.g. `https://dash.cloudflare.com/oauth2/device`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The code to check there (device flow only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The link that pre-fills the code, built the way wrangler builds it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_url: Option<String>,
}

/// Reads the link and code from `wrangler login` output. Only https links on
/// cloudflare.com count, matching wrangler's own check; either field is none until it has
/// been printed (or if the wording changed).
pub(crate) fn parse_sign_in_prompt(output: &str) -> SignInPrompt {
    static URL: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    static VISIT: OnceLock<Regex> = OnceLock::new();
    let text = strip_ansi(output);
    let after = pattern(&VISIT, "(?i)please visit:?|opening a link in your default browser:?|visit this link to authenticate:?").find(&text).map_or(text.as_str(), |found| &text[found.start()..]);
    let url = pattern(&URL, "(?i)https://[a-z0-9.-]*cloudflare\\.com/[^\\s\"'<>]*").find(after).map(|found| found.as_str().to_owned()).filter(|url| cloudflare_host(url));
    let code = pattern(&CODE, "(?i)enter the code:?\\s*\\n?\\s*([A-Za-z0-9]{3,}(?:-[A-Za-z0-9]{3,})*)\\b").captures(&text).map(|found| found[1].to_owned());
    let open_url = url.as_ref().map(|url| match &code {
        Some(code) => format!("{url}{}user_code={code}", if url.contains('?') { '&' } else { '?' }),
        None => url.clone(),
    });
    SignInPrompt { url, code, open_url }
}

fn cloudflare_host(url: &str) -> bool {
    let host = url.trim_start_matches("https://").split(['/', '?', '#']).next().unwrap_or("").to_ascii_lowercase();
    host == "cloudflare.com" || host.ends_with(".cloudflare.com")
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Account {
    pub id: String,
    pub name: String,
}

/// The accounts in `wrangler whoami --json` (id and name only); none when unreadable.
pub(crate) fn parse_whoami_accounts(output: &str) -> Option<Vec<Account>> {
    let (start, end) = (output.find('{')?, output.rfind('}')?);
    let value: serde_json::Value = serde_json::from_str(output.get(start..=end)?).ok()?;
    let accounts = value["accounts"].as_array()?;
    Some(
        accounts
            .iter()
            .filter_map(|account| {
                let id = account["id"].as_str()?.trim();
                let name = account["name"].as_str().unwrap_or("").trim();
                (!id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric())).then(|| Account { id: id.to_owned(), name: if name.is_empty() { id.to_owned() } else { name.chars().take(120).collect() } })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn downloaded() -> Launcher {
        Launcher::Downloaded(node::Installed { node: "/data/node/bin/node".into(), npx_cli: "/data/node/lib/node_modules/npm/bin/npx-cli.js".into(), bin: "/data/node/bin".into() })
    }

    #[test]
    fn wrangler_is_pinned_to_one_exact_version() {
        let parts: Vec<&str> = WRANGLER_VERSION.split('.').collect();
        assert!(parts.len() == 3 && parts[0] == "4" && parts.iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())), "{WRANGLER_VERSION}");
        assert_eq!(package(), format!("wrangler@{WRANGLER_VERSION}"));
        let (program, args) = command(&Launcher::System { npx: "/usr/bin/npx".into() }, &["whoami", "--json"]);
        assert_eq!((program, args), (PathBuf::from("/usr/bin/npx"), vec![OsString::from("--yes"), package().into(), "whoami".into(), "--json".into()]));
        let (program, args) = command(&downloaded(), &["deploy"]);
        assert_eq!(program, PathBuf::from("/data/node/bin/node"));
        assert_eq!(args, vec![OsString::from("/data/node/lib/node_modules/npm/bin/npx-cli.js"), "--yes".into(), package().into(), "deploy".into()]);
    }

    #[test]
    fn the_environment_turns_off_metrics_and_trusts_the_system_certificates_when_node_can() {
        let get = |env: &[(OsString, OsString)], key: &str| env.iter().find(|(name, _)| name == key).map(|(_, value)| value.to_string_lossy().into_owned());
        let system = Launcher::System { npx: "npx".into() };
        let env = environment(&system, (22, 15, 0), Some("abc123"), Some("/usr/bin".into()), Some(" --max-old-space-size=512 ".into()));
        assert_eq!(get(&env, "WRANGLER_SEND_METRICS").as_deref(), Some("false"));
        assert_eq!(get(&env, "NODE_OPTIONS").as_deref(), Some("--use-system-ca --max-old-space-size=512"));
        assert_eq!(get(&env, "CLOUDFLARE_ACCOUNT_ID").as_deref(), Some("abc123"));
        // The machine's own Node keeps the machine's PATH.
        assert_eq!(get(&env, "PATH"), None);
        let old = environment(&system, (22, 14, 0), None, None, None);
        assert_eq!((get(&old, "NODE_OPTIONS"), get(&old, "CLOUDFLARE_ACCOUNT_ID")), (None, None));
        let own = environment(&downloaded(), (22, 23, 4), None, Some(std::env::join_paths(["/usr/bin", "/bin"]).unwrap()), None);
        let path = get(&own, "PATH").unwrap();
        assert_eq!(std::env::split_paths(&path).collect::<Vec<_>>(), [PathBuf::from("/data/node/bin"), "/usr/bin".into(), "/bin".into()]);
    }

    #[test]
    fn worker_names_and_addresses() {
        for name in ["pandacode-quick-connect", "a", "a1", "relay-2"] {
            assert!(is_valid_worker_name(name), "{name}");
        }
        for name in ["", "-a", "a-", "Relay", "a_b", "a b", &"a".repeat(64)] {
            assert!(!is_valid_worker_name(name), "{name}");
        }
        let output = "Uploaded pandacode-quick-connect (2.1 sec)\nDeployed pandacode-quick-connect triggers\n  https://PandaCode-quick-connect.my-team.workers.dev\nCurrent Version ID: 1\n";
        assert_eq!(parse_worker_url(output, DEFAULT_WORKER_NAME).as_deref(), Some("https://pandacode-quick-connect.my-team.workers.dev"));
        assert_eq!(parse_worker_url(output, "other"), None);
        assert_eq!(parse_worker_url("https://pandacode-quick-connect.evil.example/workers.dev", DEFAULT_WORKER_NAME), None);
        assert_eq!(parse_worker_url("https://a.b.workers.dev", "a.b"), None);
    }

    #[test]
    fn whoami_and_failure_wording() {
        assert!(needs_login("You are not authenticated. Please run `wrangler login`.", 0));
        assert!(needs_login("anything", 1));
        assert!(!needs_login("You are logged in with an OAuth Token, associated with the email a@b.c.", 0));
        assert!(!needs_login("some new wording", 0));
        assert!(missing_workers_dev_subdomain("You need to register a workers.dev subdomain before publishing to workers.dev"));
        assert!(!missing_workers_dev_subdomain("Deployed"));
        assert!(already_deleted("This Worker does not exist on your account. [code: 10007]"));
        assert!(!already_deleted("Authentication error [code: 10000]"));
        assert!(device_login_unsupported("✘ [ERROR] Unknown arguments: device, browser"));
        assert!(!device_login_unsupported("To authorize Wrangler, please visit:"));
    }

    #[test]
    fn sign_in_prompts_of_both_flows_are_read() {
        let device = "\u{1b}[1mTo authorize Wrangler, please visit:\u{1b}[0m\n\n  https://dash.cloudflare.com/oauth2/device\n\nand enter the code:\n\n  WDJB-MJHT\n\n";
        assert_eq!(
            parse_sign_in_prompt(device),
            SignInPrompt { url: Some("https://dash.cloudflare.com/oauth2/device".into()), code: Some("WDJB-MJHT".into()), open_url: Some("https://dash.cloudflare.com/oauth2/device?user_code=WDJB-MJHT".into()) }
        );
        let browser = "Attempting to login via OAuth...\nOpening a link in your default browser: https://dash.cloudflare.com/oauth2/auth?response_type=code&client_id=x\n";
        let prompt = parse_sign_in_prompt(browser);
        assert_eq!(prompt.url.as_deref(), Some("https://dash.cloudflare.com/oauth2/auth?response_type=code&client_id=x"));
        assert_eq!((prompt.code.as_deref(), prompt.open_url), (None, prompt.url.clone()));
        // Nothing printed yet, and links that are not Cloudflare's.
        assert_eq!(parse_sign_in_prompt("Attempting to login via OAuth...\n"), SignInPrompt::default());
        assert_eq!(parse_sign_in_prompt("please visit: https://evil.example/cloudflare.com/x").url, None);
        assert_eq!(parse_sign_in_prompt("please visit: https://dash.cloudflare.com.evil.example/x").url, None);
    }

    #[test]
    fn accounts_are_read_from_whoami_json_and_secrets_are_redacted() {
        let output = "warning: something\n{\"loggedIn\":true,\"email\":\"a@b.c\",\"accounts\":[{\"id\":\"0123456789abcdef0123456789abcdef\",\"name\":\"Personal\"},{\"id\":\"ffff\",\"name\":\"  \"},{\"id\":\"../x\",\"name\":\"bad\"}]}\n";
        assert_eq!(parse_whoami_accounts(output), Some(vec![Account { id: "0123456789abcdef0123456789abcdef".into(), name: "Personal".into() }, Account { id: "ffff".into(), name: "ffff".into() }]));
        assert_eq!(parse_whoami_accounts("not json"), None);
        assert_eq!(parse_whoami_accounts("{\"loggedIn\":false}"), None);
        let secret = "s3cr3t-host-value-0123456789".to_string();
        assert_eq!(redact(&format!("\u{1b}[31mfailed with {secret}\u{1b}[0m and {secret}"), &[secret.clone(), "short".into()]), "failed with [redacted] and [redacted]");
        assert_eq!(redact("short stays", &["short".into()]), "short stays");
    }
}
