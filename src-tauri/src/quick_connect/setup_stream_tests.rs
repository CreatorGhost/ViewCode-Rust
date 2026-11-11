//! Quick connect setup driven through the real runner with a fake `npx` (a shell script
//! that plays `npx --yes wrangler@<pinned> …`), in the style of the provider
//! `*_stream_tests.rs`. Nothing here reaches Cloudflare, npm or nodejs.org: the "relay" is
//! a canned probe, and the one download comes from a loopback server.
use super::node::{self, Release};
use super::setup::{self, DeployOptions, Fail, Host, Operation, Prepared, Probe, Problem, SetupState, Step};
use super::state::{Config, Event, Relay, Stage};
use super::tools::{self, Finder};
use super::wrangler::{self, Launcher, Options, Session};
use super::{pairing, worker};
use std::collections::VecDeque;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FAKE_NPX: &str = r#"#!/bin/sh
# Plays `npx --yes wrangler@<version> <command…>`. Its behaviour is set by files in DIR.
DIR='__DIR__'
printf '%s\n' "$*" >> "$DIR/argv.log"
[ "$1" = "--yes" ] || { echo "expected --yes first" >&2; exit 64; }
[ "$2" = "wrangler@__VERSION__" ] || { echo "unexpected package: $2" >&2; exit 64; }
shift 2
printf '%s account=%s metrics=%s\n' "$1" "${CLOUDFLARE_ACCOUNT_ID:-}" "${WRANGLER_SEND_METRICS:-}" >> "$DIR/env.log"
mode() { cat "$DIR/$1" 2>/dev/null || echo "$2"; }
case "$1" in
  whoami)
    if [ "$2" = "--json" ]; then cat "$DIR/accounts.json"; exit 0; fi
    if [ -f "$DIR/logged-in" ]; then echo "You are logged in with an OAuth Token."; exit 0; fi
    echo 'You are not authenticated. Please run `wrangler login`.'; exit 1 ;;
  login)
    how=$(mode login approve)
    if [ "$2" = "--device" ] && [ "$how" = "no-device" ]; then echo "[ERROR] Unknown arguments: device, browser"; exit 1; fi
    if [ "$2" = "--device" ]; then
      printf 'To authorize Wrangler, please visit:\n\n  https://dash.cloudflare.com/oauth2/device\n\nand enter the code:\n\n  WDJB-MJHT\n\n'
    else
      echo "Opening a link in your default browser: https://dash.cloudflare.com/oauth2/auth?client_id=fake"
    fi
    case "$how" in
      hang) sleep 60 ;;
      deny) echo "Error: access_denied"; exit 1 ;;
    esac
    : > "$DIR/logged-in"; echo "Successfully logged in."; exit 0 ;;
  deploy)
    # deploy --config <path> --name <name>
    case "$(mode deploy ok)" in
      ok) echo "Uploaded $5 (1.2 sec)"; echo "  https://$5.fake-team.workers.dev"; exit 0 ;;
      fail) echo "[ERROR] A request to the Cloudflare API failed. Authentication error [code: 10000]" >&2; exit 1 ;;
      subdomain) echo "[ERROR] You need to register a workers.dev subdomain before publishing to workers.dev" >&2; exit 1 ;;
      no-url) echo "Uploaded $5"; exit 0 ;;
    esac ;;
  secret)
    # secret put HOST_SECRET --name <name> --config <path>: the value arrives on stdin only.
    value=$(cat)
    printf '%s' "$value" > "$DIR/secret-stdin"
    if [ "$(mode secret ok)" = "fail" ]; then echo "[ERROR] could not store $value"; exit 1; fi
    echo "Success! Uploaded secret $3"; exit 0 ;;
  delete)
    case "$(mode delete ok)" in
      ok) echo "Successfully deleted $3"; exit 0 ;;
      gone) echo "This Worker does not exist on your account. [code: 10007]" >&2; exit 1 ;;
      fail) echo "[ERROR] connect ETIMEDOUT" >&2; exit 1 ;;
    esac ;;
esac
echo "unexpected command: $*" >&2
exit 64
"#;

const ONE_ACCOUNT: &str = r#"{"loggedIn":true,"accounts":[{"id":"aaaa1111","name":"Personal"}]}"#;
const TWO_ACCOUNTS: &str = r#"{"loggedIn":true,"accounts":[{"id":"aaaa1111","name":"Personal"},{"id":"bbbb2222","name":"Work"}]}"#;
const URL: &str = "https://pandacode-quick-connect.fake-team.workers.dev";

struct Fake {
    dir: PathBuf,
    launcher: Launcher,
    cancel: Arc<AtomicBool>,
    states: Mutex<Vec<SetupState>>,
    config: Mutex<Config>,
    secret: Mutex<Option<String>>,
    probes: Mutex<VecDeque<Probe>>,
    probed: Mutex<Vec<(String, String)>>,
    attach: Mutex<crate::Result<()>>,
    attached: Mutex<Vec<(String, String)>>,
    detached: AtomicBool,
    pauses: Mutex<Vec<u64>>,
    /// Cancels the operation when this state is published.
    cancel_on: Mutex<Option<Step>>,
}

impl Fake {
    fn new() -> Fake {
        let dir = std::env::temp_dir().join(format!("pandacode-qc-setup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let npx = dir.join("npx");
        std::fs::write(&npx, FAKE_NPX.replace("__DIR__", dir.to_str().unwrap()).replace("__VERSION__", wrangler::WRANGLER_VERSION)).unwrap();
        std::fs::set_permissions(&npx, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("accounts.json"), ONE_ACCOUNT).unwrap();
        Fake {
            dir,
            launcher: Launcher::System { npx },
            cancel: Arc::default(),
            states: Mutex::default(),
            config: Mutex::default(),
            secret: Mutex::default(),
            probes: Mutex::default(),
            probed: Mutex::default(),
            attach: Mutex::new(Ok(())),
            attached: Mutex::default(),
            detached: AtomicBool::new(false),
            pauses: Mutex::default(),
            cancel_on: Mutex::default(),
        }
    }

    fn set(&self, file: &str, value: &str) -> &Fake {
        std::fs::write(self.dir.join(file), value).unwrap();
        self
    }

    fn logged_in(&self) -> &Fake {
        self.set("logged-in", "")
    }

    fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.dir.join(file)).unwrap_or_default()
    }

    /// The wrangler commands run, without the `--yes wrangler@x` prefix and paths.
    fn commands(&self) -> Vec<String> {
        self.read("argv.log").lines().map(|line| line.split_whitespace().skip(2).filter(|word| !word.starts_with('/')).collect::<Vec<_>>().join(" ")).collect()
    }

    fn session(&self) -> Session {
        Session { launcher: self.launcher.clone(), node_version: (22, 15, 0), cwd: self.dir.clone(), account_id: None, cancel: self.cancel.clone() }
    }

    fn steps(&self) -> Vec<Step> {
        let mut steps: Vec<Step> = Vec::new();
        for state in self.states.lock().unwrap().iter() {
            if let SetupState::Running { step, .. } = state {
                if steps.last() != Some(step) {
                    steps.push(*step);
                }
            }
        }
        steps
    }

    fn last(&self) -> SetupState {
        self.states.lock().unwrap().last().cloned().unwrap()
    }

    fn stage(&self) -> Stage {
        self.config.lock().unwrap().stage()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Host for Fake {
    fn publish(&self, state: SetupState) {
        if let (SetupState::Running { step, .. }, Some(on)) = (&state, *self.cancel_on.lock().unwrap()) {
            if *step == on {
                self.cancel.store(true, Ordering::SeqCst);
            }
        }
        self.states.lock().unwrap().push(state);
    }

    fn now_ms(&self) -> i64 {
        1_000 + self.states.lock().unwrap().len() as i64
    }

    fn prepare(&self, _progress: &dyn Fn(&str)) -> Result<Prepared, Fail> {
        let staged = worker::stage(&self.dir)?;
        Ok(Prepared { session: Session { cwd: staged.dir.clone(), ..self.session() }, staged })
    }

    fn read_secret(&self) -> Option<String> {
        self.secret.lock().unwrap().clone()
    }

    fn write_secret(&self, secret: &str) -> crate::Result<()> {
        *self.secret.lock().unwrap() = Some(secret.to_owned());
        Ok(())
    }

    fn remove_secret(&self) {
        *self.secret.lock().unwrap() = None;
    }

    fn relay(&self) -> Option<Relay> {
        self.config.lock().unwrap().relay.clone()
    }

    fn record(&self, event: Event) -> crate::Result<()> {
        self.config.lock().unwrap().apply(event);
        Ok(())
    }

    fn probe(&self, url: &str, secret: &str) -> Probe {
        self.probed.lock().unwrap().push((url.to_owned(), secret.to_owned()));
        self.probes.lock().unwrap().pop_front().unwrap_or(Probe::Ok)
    }

    fn attach(&self, url: &str, secret: &str) -> crate::Result<()> {
        self.attached.lock().unwrap().push((url.to_owned(), secret.to_owned()));
        self.attach.lock().unwrap().clone()
    }

    fn detach(&self) {
        self.detached.store(true, Ordering::SeqCst);
    }

    fn pause(&self, seconds: u64) -> bool {
        self.pauses.lock().unwrap().push(seconds);
        !self.cancelled()
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

fn deploy(fake: &Fake) -> Option<DeployOptions> {
    setup::run(fake, Operation::Deploy(DeployOptions::default()))
}

#[test]
fn first_setup_signs_in_deploys_stores_the_secret_on_stdin_and_verifies() {
    let fake = Fake::new();
    assert_eq!(deploy(&fake), None);
    assert_eq!(fake.steps(), [Step::CheckingTools, Step::SigningIn, Step::Deploying, Step::StoringSecret, Step::Verifying]);
    assert_eq!(fake.last(), SetupState::Succeeded { message: "Quick connect is set up.".into() });

    // The sign-in link and code reached the UI while wrangler was still waiting.
    let prompts: Vec<_> = fake.states.lock().unwrap().iter().filter_map(|state| if let SetupState::Running { sign_in: Some(prompt), .. } = state { Some(prompt.clone()) } else { None }).collect();
    let shown = prompts.last().unwrap();
    assert_eq!((shown.url.as_deref(), shown.code.as_deref()), (Some("https://dash.cloudflare.com/oauth2/device"), Some("WDJB-MJHT")));
    assert_eq!(shown.open_url.as_deref(), Some("https://dash.cloudflare.com/oauth2/device?user_code=WDJB-MJHT"));

    assert_eq!(
        fake.commands(),
        ["whoami", "login --device --browser=false", "whoami --json", "deploy --config --name pandacode-quick-connect", "secret put HOST_SECRET --name pandacode-quick-connect --config"]
    );
    // The secret is new, 256 bits, went to wrangler on stdin and nowhere on a command line.
    let secret = fake.secret.lock().unwrap().clone().unwrap();
    assert_eq!(secret.len(), 43);
    assert_eq!(fake.read("secret-stdin"), secret);
    assert!(!fake.read("argv.log").contains(&secret) && !fake.read("env.log").contains(&secret));
    assert!(!format!("{:?}", fake.states.lock().unwrap()).contains(&secret));

    // Deploy, secret and nothing else ran against the one account the sign-in has, with metrics off.
    let env = fake.read("env.log");
    assert!(env.contains("deploy account=aaaa1111 metrics=false") && env.contains("secret account=aaaa1111 metrics=false") && env.contains("login account= metrics=false"), "{env}");
    assert_eq!(fake.relay(), Some(Relay { name: "pandacode-quick-connect".into(), url: URL.into(), account_id: Some("aaaa1111".into()) }));
    assert_eq!(fake.stage(), Stage::Deployed);
    assert_eq!(*fake.probed.lock().unwrap(), [(URL.to_string(), secret.clone())]);
    assert_eq!(*fake.attached.lock().unwrap(), [(URL.to_string(), secret)]);
    // The staged Worker is gone once the run is over.
    assert!(std::fs::read_dir(&fake.dir).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with("pandacode-quick-connect-")));
}

#[test]
fn a_redeploy_keeps_the_secret_unless_asked_to_rotate_it() {
    let fake = Fake::new();
    fake.logged_in();
    deploy(&fake);
    let first = fake.read_secret().unwrap();
    // Already signed in: no sign-in step at all.
    assert_eq!(fake.steps(), [Step::CheckingTools, Step::Deploying, Step::StoringSecret, Step::Verifying]);
    assert!(!fake.commands().iter().any(|command| command.starts_with("login")));

    deploy(&fake);
    assert_eq!((fake.read_secret().unwrap(), fake.read("secret-stdin")), (first.clone(), first.clone()));
    // A phone is paired, then the secret is rotated.
    let phone = super::state::Device { id: "d".into(), name: "Phone".into(), platform: "ios".into(), fingerprint: "f".into(), scope: "full".into(), paired_at: 1 };
    fake.config.lock().unwrap().apply(Event::Paired(phone.clone()));
    setup::run(&fake, Operation::Deploy(DeployOptions { rotate_secret: true, account_id: None }));
    let rotated = fake.read_secret().unwrap();
    assert_ne!(rotated, first);
    assert_eq!(fake.read("secret-stdin"), rotated);
    // This computer registers again with the new secret, at the same address.
    assert_eq!(*fake.attached.lock().unwrap().last().unwrap(), (URL.to_string(), rotated));
    // Rotation is not a removal: the phone stays paired here, nothing was detached or deleted,
    // and the Worker was redeployed in place (same name, never `delete`).
    assert_eq!((fake.stage(), fake.config.lock().unwrap().devices.clone()), (Stage::Paired, vec![phone]));
    assert!(!fake.detached.load(Ordering::SeqCst));
    assert!(!fake.commands().iter().any(|command| command.starts_with("delete")));
    assert_eq!(fake.commands().iter().filter(|command| *command == "deploy --config --name pandacode-quick-connect").count(), 3);
}

#[test]
fn a_wrangler_without_the_device_flow_signs_in_through_the_browser() {
    let fake = Fake::new();
    fake.set("login", "no-device");
    assert_eq!(setup::run(&fake, Operation::SignIn), None);
    assert_eq!(fake.commands(), ["whoami", "login --device --browser=false", "login"]);
    let link = fake.states.lock().unwrap().iter().rev().find_map(|state| if let SetupState::Running { sign_in: Some(prompt), .. } = state { prompt.url.clone() } else { None });
    assert_eq!(link.as_deref(), Some("https://dash.cloudflare.com/oauth2/auth?client_id=fake"));
    assert_eq!(fake.last(), SetupState::Idle { message: Some("Signed in to Cloudflare.".into()) });
    assert_eq!(fake.stage(), Stage::SignedIn);
}

#[test]
fn the_tools_step_alone_stops_at_node_ready() {
    let fake = Fake::new();
    assert_eq!(setup::run(&fake, Operation::Tools), None);
    assert_eq!((fake.stage(), fake.commands().len()), (Stage::NodeReady, 0));
}

#[test]
fn several_accounts_ask_which_one_and_the_answer_is_used_and_remembered() {
    let fake = Fake::new();
    fake.logged_in().set("accounts.json", TWO_ACCOUNTS);
    let pending = deploy(&fake).expect("setup waits for an answer");
    let SetupState::NeedsAccount { accounts, .. } = fake.last() else { panic!("{:?}", fake.last()) };
    assert_eq!(accounts.iter().map(|account| (account.id.as_str(), account.name.as_str())).collect::<Vec<_>>(), [("aaaa1111", "Personal"), ("bbbb2222", "Work")]);
    assert!(fake.relay().is_none() && !fake.commands().iter().any(|command| command.starts_with("deploy")));

    assert_eq!(setup::run(&fake, Operation::Deploy(DeployOptions { account_id: Some("bbbb2222".into()), ..pending })), None);
    assert!(fake.read("env.log").contains("deploy account=bbbb2222"));
    assert_eq!(fake.relay().unwrap().account_id.as_deref(), Some("bbbb2222"));
    // The next redeploy does not ask again.
    assert_eq!(deploy(&fake), None);
    assert!(matches!(fake.last(), SetupState::Succeeded { .. }));
}

#[test]
fn a_failed_deploy_changes_nothing_and_shows_wranglers_reason() {
    let fake = Fake::new();
    fake.logged_in().set("deploy", "fail");
    assert_eq!(deploy(&fake), None);
    let SetupState::Failed { message, details, action } = fake.last() else { panic!("{:?}", fake.last()) };
    assert_eq!((message.as_str(), action), ("Cloudflare did not accept the relay. Try again.", None));
    assert!(details.unwrap().contains("[code: 10000]"));
    assert!(fake.relay().is_none() && fake.read_secret().is_none() && fake.attached.lock().unwrap().is_empty());
    assert_eq!(fake.stage(), Stage::SignedIn);

    // A deploy that reports no address is not called done either.
    fake.set("deploy", "no-url");
    deploy(&fake);
    assert!(matches!(fake.last(), SetupState::Failed { message, .. } if message.contains("did not report its address")));
    assert!(fake.relay().is_none());
}

#[test]
fn an_account_without_a_workers_dev_address_waits_for_the_person_then_continues() {
    let fake = Fake::new();
    fake.logged_in().set("deploy", "subdomain");
    let pending = deploy(&fake).expect("setup waits");
    assert!(matches!(fake.last(), SetupState::NeedsSubdomain { .. }));
    assert_eq!(pending.account_id.as_deref(), Some("aaaa1111"));
    fake.set("deploy", "ok");
    assert_eq!(setup::run(&fake, Operation::Deploy(pending)), None);
    assert!(matches!(fake.last(), SetupState::Succeeded { .. }));
}

#[test]
fn a_secret_that_cannot_be_stored_saves_nothing_and_is_never_shown() {
    let fake = Fake::new();
    fake.logged_in().set("secret", "fail");
    deploy(&fake);
    let SetupState::Failed { message, details, .. } = fake.last() else { panic!("{:?}", fake.last()) };
    assert!(message.starts_with("Could not store the relay's secret on Cloudflare."));
    // The fake echoed the value back, as a careless tool might: it is redacted.
    let sent = fake.read("secret-stdin");
    let details = details.unwrap();
    assert!(sent.len() == 43 && details.contains("could not store [redacted]") && !details.contains(&sent), "{details}");
    assert!(fake.read_secret().is_none() && fake.relay().is_none() && fake.attached.lock().unwrap().is_empty());
}

#[test]
fn a_refused_sign_in_fails_and_a_cancelled_one_stops_wrangler() {
    let fake = Fake::new();
    fake.set("login", "deny");
    deploy(&fake);
    assert!(matches!(fake.last(), SetupState::Failed { message, .. } if message == "Cloudflare sign-in did not finish. Try again."));
    assert_eq!(fake.stage(), Stage::NodeReady);

    // Cancel while wrangler waits for the browser: the run ends at once, as idle.
    let fake = Fake::new();
    fake.set("login", "hang");
    *fake.cancel_on.lock().unwrap() = Some(Step::SigningIn);
    let started = std::time::Instant::now();
    assert_eq!(deploy(&fake), None);
    assert_eq!(fake.last(), SetupState::idle());
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(fake.relay().is_none() && !fake.commands().iter().any(|command| command.starts_with("deploy")));
}

#[test]
fn every_wrangler_run_is_bounded() {
    let fake = Fake::new();
    fake.set("login", "hang");
    let session = fake.session();
    let started = std::time::Instant::now();
    let error = session.run(&["login", "--device", "--browser=false"], Options { input: None, timeout: Some(Duration::from_millis(300)) }, &mut |_| {}).unwrap_err();
    assert!(error.starts_with("The Cloudflare sign-in was not approved within"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(20));
    // A tool that is not there is one plain sentence, not a crash.
    let missing = Session { launcher: Launcher::System { npx: fake.dir.join("no-npx") }, ..fake.session() };
    assert_eq!(missing.run(&["whoami"], Options::default(), &mut |_| {}).unwrap_err(), "Could not start Cloudflare's tool on this computer. Try again.");
    // Exit codes and output come back as they are.
    let whoami = session.run(&["whoami"], Options::default(), &mut |_| {}).unwrap();
    assert!(whoami.code == 1 && wrangler::needs_login(&whoami.output, whoami.code));
    assert_eq!(session.run(&["publish"], Options::default(), &mut |_| {}).unwrap().code, 64);
}

#[test]
fn verification_waits_for_a_new_address_and_reports_what_is_wrong() {
    // A firewall resets the new hostname twice, then it works.
    let fake = Fake::new();
    fake.logged_in();
    fake.probes.lock().unwrap().extend([Probe::Problem(Problem::NetworkRefused), Probe::Problem(Problem::Transient)]);
    deploy(&fake);
    assert!(matches!(fake.last(), SetupState::Succeeded { .. }));
    assert_eq!(*fake.pauses.lock().unwrap(), [5, 10]);
    assert!(fake.states.lock().unwrap().iter().any(|state| matches!(state, SetupState::Running { step: Step::Verifying, problem: Some(Problem::NetworkRefused), message, .. } if message.starts_with("Waiting for your network"))));

    // The relay keeps refusing the secret: said after three answers in a row, and nothing connects.
    let fake = Fake::new();
    fake.logged_in();
    fake.probes.lock().unwrap().extend([Probe::Problem(Problem::CredentialRejected); 3]);
    deploy(&fake);
    assert!(matches!(fake.last(), SetupState::Unreachable { problem: Problem::CredentialRejected, .. }));
    assert_eq!((fake.pauses.lock().unwrap().len(), fake.attached.lock().unwrap().len()), (2, 0));
    // What was deployed stays saved, so turning it on again can retry.
    assert_eq!(fake.stage(), Stage::Deployed);

    // Never reachable: about ten minutes of attempts, then the verdict.
    let fake = Fake::new();
    fake.logged_in();
    fake.probes.lock().unwrap().extend([Probe::Problem(Problem::Transient); 20]);
    deploy(&fake);
    assert!(matches!(fake.last(), SetupState::Unreachable { problem: Problem::Transient, .. }));
    assert_eq!(fake.pauses.lock().unwrap().iter().sum::<u64>(), 5 + 10 + 20 + 40 + 9 * 60);

    // The relay answers but this computer cannot register with it.
    let fake = Fake::new();
    fake.logged_in();
    *fake.attach.lock().unwrap() = Err("The relay did not answer in time.".into());
    deploy(&fake);
    assert!(matches!(fake.last(), SetupState::Unreachable { problem: Problem::Transient, details: Some(details), .. } if details.contains("did not answer in time")));
}

#[test]
fn probe_answers_are_classified() {
    assert_eq!(setup::classify_probe(200, true), Probe::Ok);
    // Some other site answering 200 at that address is not our relay.
    assert_eq!(setup::classify_probe(200, false), Probe::Problem(Problem::Transient));
    assert_eq!(setup::classify_probe(401, false), Probe::Problem(Problem::CredentialRejected));
    assert_eq!(setup::classify_probe(403, false), Probe::Problem(Problem::CredentialRejected));
    // The Worker before its secret has been applied, and Cloudflare's own errors.
    assert_eq!(setup::classify_probe(500, false), Probe::Problem(Problem::Transient));
    assert_eq!(setup::classify_probe(404, false), Probe::Problem(Problem::Transient));
}

#[test]
fn turning_it_back_on_verifies_and_connects_without_running_wrangler() {
    let fake = Fake::new();
    assert_eq!(setup::run(&fake, Operation::Reuse), None);
    assert!(matches!(fake.last(), SetupState::Failed { message, .. } if message.starts_with("No relay is set up")));

    fake.logged_in();
    deploy(&fake);
    fake.config.lock().unwrap().apply(Event::Enabled(false));
    let before = fake.commands().len();
    setup::run(&fake, Operation::Reuse);
    assert!(matches!(fake.last(), SetupState::Succeeded { .. }));
    assert_eq!(fake.commands().len(), before);
    assert!(fake.config.lock().unwrap().enabled && fake.attached.lock().unwrap().len() == 2);
}

#[test]
fn removing_deletes_the_worker_then_forgets_it_here() {
    let ready = || {
        let fake = Fake::new();
        fake.logged_in();
        deploy(&fake);
        fake.config.lock().unwrap().apply(Event::Paired(super::state::Device { id: "d".into(), name: "Phone".into(), platform: "ios".into(), fingerprint: "f".into(), scope: "full".into(), paired_at: 1 }));
        assert_eq!(fake.stage(), Stage::Paired);
        fake
    };
    for (mode, message) in [("ok", "Quick connect is removed."), ("gone", "Quick connect is removed.")] {
        let fake = ready();
        fake.set("delete", mode);
        setup::run(&fake, Operation::Remove { local_only: false });
        assert_eq!(fake.last(), SetupState::Idle { message: Some(message.into()) });
        assert_eq!(fake.commands().last().unwrap(), "delete --name pandacode-quick-connect --config --force");
        assert!(fake.read("env.log").lines().last().unwrap().contains("delete account=aaaa1111"));
        assert!(fake.relay().is_none() && fake.read_secret().is_none() && fake.detached.load(Ordering::SeqCst));
        assert_eq!(fake.stage(), Stage::SignedIn);
    }

    // Cloudflare cannot be reached: nothing local changes, so a retry can still find the Worker.
    let fake = ready();
    fake.set("delete", "fail");
    setup::run(&fake, Operation::Remove { local_only: false });
    assert!(matches!(fake.last(), SetupState::RemoveFailed { details: Some(details), .. } if details.contains("ETIMEDOUT")));
    assert!(fake.relay().is_some() && fake.read_secret().is_some() && !fake.detached.load(Ordering::SeqCst));
    assert_eq!(fake.stage(), Stage::Paired);

    // "From this computer only" never runs wrangler.
    let before = fake.commands().len();
    setup::run(&fake, Operation::Remove { local_only: true });
    assert_eq!(fake.last(), SetupState::Idle { message: Some("Quick connect is removed from this computer. The relay is still on your Cloudflare account.".into()) });
    assert_eq!(fake.commands().len(), before);
    assert!(fake.relay().is_none() && fake.read_secret().is_none() && fake.detached.load(Ordering::SeqCst));
}

/// A Node release whose `node` is a shell script: `--version` prints the version, anything
/// else runs the script it is given (the fake npx), as `node npx-cli.js …` would.
fn fake_node_release(version: &str, target: &str, npx: &str) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    let node = format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo v{version}; exit 0; fi\nexec /bin/sh \"$@\"\n");
    for (path, body, mode) in [("bin/node", node.as_str(), 0o755), ("lib/node_modules/npm/bin/npx-cli.js", npx, 0o644)] {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        tar.append_data(&mut header, format!("node-v{version}-{target}/{path}"), body.as_bytes()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn serve_once(body: Vec<u8>) -> String {
    use std::io::{BufRead, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/dist", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok() && !line.trim().is_empty() {
                line.clear();
            }
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = stream.write_all(&body);
        }
    });
    base
}

#[test]
fn a_machine_with_an_old_node_downloads_the_pinned_one_and_runs_wrangler_with_it() {
    let fake = Fake::new();
    let script = std::fs::read_to_string(fake.dir.join("npx")).unwrap();
    let archive = fake_node_release("22.99.0", "linux-x64", &script);
    let sha: String = <sha2::Sha256 as sha2::Digest>::digest(&archive).iter().map(|byte| format!("{byte:02x}")).collect();
    let old_node = fake.dir.join("old-node");
    std::fs::write(&old_node, "#!/bin/sh\necho v18.20.4\n").unwrap();
    std::fs::set_permissions(&old_node, std::fs::Permissions::from_mode(0o755)).unwrap();
    let finder = Finder {
        node: Some(old_node),
        npx: Some(fake.dir.join("npx")),
        downloads: fake.dir.join("node"),
        dist: serve_once(archive),
        version: "22.99.0",
        release: Some(Release { target: "linux-x64", archive: "tar.gz", sha256: Box::leak(sha.into_boxed_str()) }),
    };
    let said = Mutex::new(Vec::new());
    let tools = tools::prepare(&finder, &AtomicBool::new(false), &|message| said.lock().unwrap().push(message.to_owned())).unwrap();
    assert_eq!(said.lock().unwrap().len(), 1);
    assert_eq!(tools.node_version, (22, 99, 0));
    let Launcher::Downloaded(installed) = &tools.launcher else { panic!("{:?}", tools.launcher) };
    assert!(installed.node.starts_with(fake.dir.join("node")) && node::installed(&finder.downloads, "22.99.0", finder.release.unwrap()).is_some());

    // wrangler runs under the downloaded Node, whose folder comes first on PATH.
    let session = Session { launcher: tools.launcher.clone(), node_version: tools.node_version, ..fake.session() };
    let whoami = session.run(&["whoami"], Options::default(), &mut |_| {}).unwrap();
    assert!(whoami.code == 1 && whoami.output.contains("not authenticated"), "{}", whoami.output);
    assert_eq!(fake.commands(), ["whoami"]);
    // The second time it is simply there: the server is not asked again (it would answer, but nothing downloads).
    let again = tools::prepare(&finder, &AtomicBool::new(false), &|_| panic!("already downloaded")).unwrap();
    assert_eq!(again, tools);
}

#[test]
fn the_host_secret_and_pairing_tokens_are_never_the_same_kind_of_value_twice() {
    // Two deployments never share a secret, and a secret is not a pairing token's code.
    let (a, b) = (pairing::new_host_secret(), pairing::new_host_secret());
    assert!(a != b && a.len() == 43 && pairing::relay_code(&a) != pairing::host_code(&a));
}
