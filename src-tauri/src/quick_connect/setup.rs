//! Quick connect setup, step for step as in ViewCode: get Cloudflare's tool ready, sign in
//! to the person's own Cloudflare account (wrangler's device flow, whose link and code the
//! UI shows), ask which account when the sign-in has several, deploy the relay Worker,
//! store the host secret on it and verify the address answers before calling it done. Also
//! removes it again.
//!
//! The steps depend only on [`Host`], so the tests drive them through the real runner with
//! a fake `npx` and a fake probe. Secrets: the host secret reaches wrangler only on stdin;
//! every piece of wrangler output is redacted before it is kept or shown; none is logged.
use super::state::{Event, Relay};
use super::wrangler::{self, Account, Options, Session, SignInPrompt};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Step {
    CheckingTools,
    SigningIn,
    Deploying,
    StoringSecret,
    Verifying,
    Removing,
}

impl Step {
    fn message(self) -> &'static str {
        match self {
            Step::CheckingTools => "Getting ready…",
            Step::SigningIn => "Signing in to Cloudflare…",
            Step::Deploying => "Setting up your relay…",
            Step::StoringSecret => "Securing it…",
            Step::Verifying => "Checking it works…",
            Step::Removing => "Removing Quick connect…",
        }
    }
}

/// Why a fresh relay address did not answer as expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Problem {
    /// This network reset the connection or broke TLS. A new workers.dev hostname is often
    /// reset by a corporate firewall for a while.
    NetworkRefused,
    /// The relay answered 401/403 to this computer's secret.
    CredentialRejected,
    /// DNS, a timeout or a 5xx; the address may simply not be live yet.
    Transient,
}

impl Problem {
    fn waiting(self) -> &'static str {
        match self {
            Problem::NetworkRefused => "Waiting for your network to allow the new address. This can take a few minutes. You can close this; the app keeps trying.",
            Problem::Transient => "Waiting for the new address to come online. You can close this; the app keeps trying.",
            Problem::CredentialRejected => "The relay has not accepted this computer's secret yet. Checking again…",
        }
    }

    fn unreachable(self) -> &'static str {
        match self {
            Problem::NetworkRefused => "Your relay is set up, but this network still blocks its new address. Try again later from Settings.",
            Problem::Transient => "Your relay is set up, but its address is not answering yet. Try again later from Settings.",
            Problem::CredentialRejected => "The relay does not accept this computer's secret. Redeploy with a new secret to fix it.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Probe {
    Ok,
    Problem(Problem),
}

/// Classifies the relay's answer to `GET /v1/self/probe` with the host secret.
pub(crate) fn classify_probe(status: u16, self_hosted: bool) -> Probe {
    match status {
        200 if self_hosted => Probe::Ok,
        401 | 403 => Probe::Problem(Problem::CredentialRejected),
        // 500 is the Worker before its secret has been applied; anything else is not ours yet.
        _ => Probe::Problem(Problem::Transient),
    }
}

/// What the UI shows. `status` is the discriminant; the wording is ready to display.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub(crate) enum SetupState {
    Idle {
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Running {
        step: Step,
        /// Unix milliseconds; a new message for the same step keeps it.
        step_started_at: i64,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        sign_in: Option<SignInPrompt>,
        #[serde(skip_serializing_if = "Option::is_none")]
        problem: Option<Problem>,
    },
    /// The sign-in reaches several Cloudflare accounts: `quick_connect_choose_account`.
    NeedsAccount { accounts: Vec<Account>, message: String },
    /// The account has no workers.dev address yet: `quick_connect_continue` once it has.
    NeedsSubdomain { message: String },
    Succeeded { message: String },
    Unreachable {
        problem: Problem,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<String>,
    },
    Failed {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<String>,
        /// `install-node` when installing Node.js would fix it.
        #[serde(skip_serializing_if = "Option::is_none")]
        action: Option<&'static str>,
    },
    RemoveFailed {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<String>,
    },
}

impl SetupState {
    pub(crate) fn idle() -> Self {
        SetupState::Idle { message: None }
    }

    pub(crate) fn is_running(&self) -> bool {
        matches!(self, SetupState::Running { .. })
    }
}

/// A step that cannot continue: one plain sentence, and what would fix it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fail {
    pub message: String,
    pub details: Option<String>,
    pub action: Option<&'static str>,
}

impl From<String> for Fail {
    fn from(message: String) -> Self {
        Fail { message, details: None, action: None }
    }
}

impl From<&str> for Fail {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct DeployOptions {
    /// Rotating is only ever explicit: an unchanged secret keeps a working relay working.
    pub rotate_secret: bool,
    /// Chosen by the person after `needs-account`; wins over the remembered one.
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Operation {
    /// Step 1 only: find or download Node.js.
    Tools,
    /// Steps 1–2: tools, then the Cloudflare sign-in.
    SignIn,
    /// The whole setup ("Connect with Cloudflare", and redeploys).
    Deploy(DeployOptions),
    /// Turn an already deployed relay back on: verify it and connect.
    Reuse,
    Remove { local_only: bool },
}

/// Ready-to-run tools and the staged Worker, which lives as long as this does.
pub(crate) struct Prepared {
    pub session: Session,
    pub staged: super::worker::Staged,
}

/// Everything setup needs from the rest of the app.
pub(crate) trait Host {
    fn publish(&self, state: SetupState);
    fn now_ms(&self) -> i64;
    /// Finds Node 22+ (downloading the pinned release when the machine has none, while
    /// `progress` says so) and stages the Worker.
    fn prepare(&self, progress: &dyn Fn(&str)) -> Result<Prepared, Fail>;
    fn read_secret(&self) -> Option<String>;
    fn write_secret(&self, secret: &str) -> crate::Result<()>;
    fn remove_secret(&self);
    fn relay(&self) -> Option<Relay>;
    /// Records a step of the setup state machine (`state.rs`) and saves it.
    fn record(&self, event: Event) -> crate::Result<()>;
    /// One request to the relay with the host secret; never fails.
    fn probe(&self, url: &str, secret: &str) -> Probe;
    /// Registers this computer with its relay and connects.
    fn attach(&self, url: &str, secret: &str) -> crate::Result<()>;
    /// Disconnects from the relay and forgets the sign-in to it.
    fn detach(&self);
    /// Waits; false when the operation was cancelled meanwhile.
    fn pause(&self, seconds: u64) -> bool;
    fn cancelled(&self) -> bool;
}

/// Seconds between verification attempts: quick at first, then once a minute, about ten
/// minutes in all. A fresh workers.dev hostname can be reset by a corporate firewall for a
/// while, so one reset is never the verdict.
const VERIFY_DELAYS: [u64; 13] = [5, 10, 20, 40, 60, 60, 60, 60, 60, 60, 60, 60, 60];
/// A just-stored Worker secret can take a moment to apply; 401s in a row before believing it.
const REJECTIONS_BEFORE_REPORTING: u32 = 3;
const DETAILS_MAX: usize = 16_000;

struct Flow<'a, H: Host> {
    host: &'a H,
    /// Secrets to redact from anything kept from this run's output.
    secrets: Vec<String>,
    details: String,
    step: Option<(Step, i64)>,
}

impl<H: Host> Flow<'_, H> {
    fn set_step(&mut self, step: Step, message: Option<&str>, sign_in: Option<SignInPrompt>, problem: Option<Problem>) {
        let started = match self.step {
            Some((current, started)) if current == step => started,
            _ => self.host.now_ms(),
        };
        self.step = Some((step, started));
        self.host.publish(SetupState::Running { step, step_started_at: started, message: message.unwrap_or(step.message()).to_owned(), sign_in, problem });
    }

    fn keep(&mut self, output: &str) {
        let kept = wrangler::redact(output, &self.secrets);
        let kept = kept.trim();
        if kept.is_empty() {
            return;
        }
        if !self.details.is_empty() {
            self.details.push_str("\n\n");
        }
        self.details.push_str(kept);
        if self.details.len() > DETAILS_MAX {
            let mut cut = self.details.len() - DETAILS_MAX;
            while !self.details.is_char_boundary(cut) {
                cut += 1;
            }
            self.details.drain(..cut);
        }
    }

    fn details(&self) -> Option<String> {
        (!self.details.is_empty()).then(|| self.details.clone())
    }

    fn fail(&self, message: &str) -> Fail {
        Fail { message: message.to_owned(), details: self.details(), action: None }
    }

    fn prepare(&mut self) -> Result<Prepared, Fail> {
        self.set_step(Step::CheckingTools, None, None, None);
        let started = self.step.map_or(0, |(_, started)| started);
        let host = self.host;
        let prepared = host.prepare(&|message| host.publish(SetupState::Running { step: Step::CheckingTools, step_started_at: started, message: message.to_owned(), sign_in: None, problem: None }))?;
        host.record(Event::ToolsReady)?;
        Ok(prepared)
    }

    /// Signs in with the device flow unless wrangler already has a login.
    fn sign_in(&mut self, session: &Session) -> Result<(), Fail> {
        // The first wrangler run also downloads it, so this is still "getting ready".
        let whoami = session.run(&["whoami"], Options::default(), &mut |_| {})?;
        if wrangler::needs_login(&wrangler::redact(&whoami.output, &self.secrets), whoami.code) {
            self.set_step(Step::SigningIn, None, Some(SignInPrompt::default()), None);
            // `--browser=false`: the app shows the link and code and opens the page itself.
            let mut login = self.login(session, &["login", "--device", "--browser=false"])?;
            if login.code != 0 && wrangler::device_login_unsupported(&login.output) {
                // A wrangler without the device flow: its ordinary browser sign-in.
                login = self.login(session, &["login"])?;
            }
            self.keep(&login.output);
            if login.code != 0 {
                return Err(self.fail("Cloudflare sign-in did not finish. Try again."));
            }
        }
        self.host.record(Event::SignedIn)?;
        Ok(())
    }

    fn login(&mut self, session: &Session, args: &[&str]) -> Result<super::process::Finished, Fail> {
        let mut printed = String::new();
        let mut shown = SignInPrompt::default();
        let (host, started, secrets) = (self.host, self.step.map_or(0, |(_, started)| started), self.secrets.clone());
        let finished = session.run(args, Options::default(), &mut |text| {
            printed.push_str(text);
            let prompt = wrangler::parse_sign_in_prompt(&wrangler::redact(&printed, &secrets));
            if prompt != shown {
                shown = prompt.clone();
                host.publish(SetupState::Running { step: Step::SigningIn, step_started_at: started, message: Step::SigningIn.message().to_owned(), sign_in: Some(prompt), problem: None });
            }
        })?;
        Ok(finished)
    }

    /// The Cloudflare account to deploy to: `preferred` (just chosen, or remembered) while
    /// the sign-in still reaches it, else the only one. With several and none of them
    /// known, the person chooses: wrangler cannot ask when it runs non-interactively.
    fn resolve_account(&mut self, session: &Session, preferred: Option<String>) -> Result<Result<Option<String>, Vec<Account>>, Fail> {
        let whoami = session.run(&["whoami", "--json"], Options::default(), &mut |_| {})?;
        let accounts = if whoami.code == 0 { wrangler::parse_whoami_accounts(&whoami.output) } else { None };
        // Unreadable: deploy as before and let wrangler decide (or explain).
        let Some(accounts) = accounts.filter(|accounts| !accounts.is_empty()) else { return Ok(Ok(preferred)) };
        if let Some(preferred) = preferred.filter(|id| accounts.iter().any(|account| &account.id == id)) {
            return Ok(Ok(Some(preferred)));
        }
        if accounts.len() == 1 {
            return Ok(Ok(Some(accounts[0].id.clone())));
        }
        Ok(Err(accounts))
    }

    fn verify(&mut self, url: &str, secret: &str) -> Result<(), Fail> {
        self.set_step(Step::Verifying, None, None, None);
        let mut rejections = 0;
        for attempt in 0.. {
            if self.host.cancelled() {
                return Err(wrangler::CANCELLED.into());
            }
            let problem = match self.host.probe(url, secret) {
                Probe::Ok => {
                    return match self.host.attach(url, secret) {
                        Ok(()) => {
                            self.host.publish(SetupState::Succeeded { message: "Quick connect is set up.".into() });
                            Ok(())
                        }
                        Err(error) => {
                            self.keep(&error);
                            self.host.publish(SetupState::Unreachable { problem: Problem::Transient, message: "Your relay is set up, but this computer could not connect to it yet. Try again from Settings.".into(), details: self.details() });
                            Ok(())
                        }
                    };
                }
                Probe::Problem(problem) => problem,
            };
            rejections = if problem == Problem::CredentialRejected { rejections + 1 } else { 0 };
            let delay = VERIFY_DELAYS.get(attempt).copied();
            let (Some(delay), true) = (delay, rejections < REJECTIONS_BEFORE_REPORTING) else {
                // What was saved stays: turning Quick connect on again retries.
                self.host.publish(SetupState::Unreachable { problem, message: problem.unreachable().into(), details: self.details() });
                return Ok(());
            };
            self.set_step(Step::Verifying, Some(problem.waiting()), None, Some(problem));
            if !self.host.pause(delay) {
                return Err(wrangler::CANCELLED.into());
            }
        }
        Ok(())
    }

    fn deploy(&mut self, options: DeployOptions) -> Result<Option<DeployOptions>, Fail> {
        let prepared = self.prepare()?;
        let remembered = self.host.relay();
        let name = remembered.as_ref().map(|relay| relay.name.clone()).filter(|name| wrangler::is_valid_worker_name(name)).unwrap_or_else(|| wrangler::DEFAULT_WORKER_NAME.to_owned());
        let stored = self.host.read_secret();
        self.secrets = stored.clone().into_iter().collect();

        self.sign_in(&prepared.session)?;
        let preferred = options.account_id.clone().or_else(|| remembered.as_ref().and_then(|relay| relay.account_id.clone()));
        let account_id = match self.resolve_account(&prepared.session, preferred)? {
            Ok(account_id) => account_id,
            Err(accounts) => {
                self.host.publish(SetupState::NeedsAccount { accounts, message: "Your Cloudflare sign-in has several accounts. Choose one for the relay.".into() });
                return Ok(Some(options));
            }
        };
        let session = prepared.session.for_account(account_id.clone());
        let config = prepared.staged.config.to_string_lossy().into_owned();

        self.set_step(Step::Deploying, None, None, None);
        let deployed = session.run(&["deploy", "--config", &config, "--name", &name], Options::default(), &mut |_| {})?;
        self.keep(&deployed.output);
        let url = wrangler::parse_worker_url(&deployed.output, &name);
        // Checked before the exit code: without a subdomain wrangler may exit non-zero.
        if url.is_none() && wrangler::missing_workers_dev_subdomain(&deployed.output) {
            self.host.publish(SetupState::NeedsSubdomain { message: "Your Cloudflare account needs a free workers.dev address first. Open Cloudflare, choose one under Workers & Pages, then come back.".into() });
            return Ok(Some(DeployOptions { account_id, ..options }));
        }
        if deployed.code != 0 {
            return Err(self.fail("Cloudflare did not accept the relay. Try again."));
        }
        let Some(url) = url else { return Err(self.fail("The relay was deployed, but Cloudflare did not report its address. Try again.")) };

        self.set_step(Step::StoringSecret, None, None, None);
        let secret = stored.filter(|_| !options.rotate_secret).unwrap_or_else(super::pairing::new_host_secret);
        self.secrets.push(secret.clone());
        let put = session.run(&["secret", "put", wrangler::HOST_SECRET_NAME, "--name", &name, "--config", &config], Options { input: Some(&format!("{secret}\n")), timeout: None }, &mut |_| {})?;
        self.keep(&put.output);
        if put.code != 0 {
            return Err(self.fail("Could not store the relay's secret on Cloudflare. Nothing changed on this computer. Try again."));
        }
        // Secret first: the address and the stored secret must change together.
        self.host.write_secret(&secret)?;
        self.host.record(Event::Deployed(Relay { name, url: url.clone(), account_id }))?;
        drop(prepared);

        self.verify(&url, &secret)?;
        Ok(None)
    }

    fn reuse(&mut self) -> Result<(), Fail> {
        let (Some(relay), Some(secret)) = (self.host.relay(), self.host.read_secret()) else {
            return Err("No relay is set up on this computer yet. Set up Quick connect instead.".into());
        };
        self.secrets = vec![secret.clone()];
        self.host.record(Event::Enabled(true))?;
        self.verify(&relay.url, &secret)
    }

    fn remove(&mut self, local_only: bool) -> Result<(), Fail> {
        let remembered = self.host.relay();
        if let (Some(relay), false) = (&remembered, local_only) {
            if let Err(failure) = self.delete(relay) {
                if failure.message == wrangler::CANCELLED {
                    return Err(failure);
                }
                self.keep(&failure.message);
                // Nothing local changed, so a retry can still find the Worker.
                self.host.publish(SetupState::RemoveFailed { message: "Could not delete the relay from Cloudflare. Nothing changed on this computer. Try again, or remove it from this computer only.".into(), details: self.details() });
                return Ok(());
            }
        }
        self.set_step(Step::Removing, None, None, None);
        self.host.detach();
        self.host.remove_secret();
        self.host.record(Event::Removed)?;
        let message = match (remembered.is_some(), local_only) {
            (false, false) => "Quick connect is removed from this computer. If you set up its relay elsewhere, delete it in the Cloudflare dashboard.",
            (_, true) => "Quick connect is removed from this computer. The relay is still on your Cloudflare account.",
            (true, false) => "Quick connect is removed.",
        };
        self.host.publish(SetupState::Idle { message: Some(message.into()) });
        Ok(())
    }

    fn delete(&mut self, relay: &Relay) -> Result<(), Fail> {
        let prepared = self.prepare()?;
        self.secrets = self.host.read_secret().into_iter().collect();
        self.sign_in(&prepared.session)?;
        // A relay set up before the account was remembered relies on wrangler's own choice.
        let session = prepared.session.for_account(relay.account_id.clone());
        self.set_step(Step::Removing, None, None, None);
        let config = prepared.staged.config.to_string_lossy().into_owned();
        let result = session.run(&["delete", "--name", &relay.name, "--config", &config, "--force"], Options::default(), &mut |_| {})?;
        self.keep(&result.output);
        if result.code == 0 || wrangler::already_deleted(&result.output) { Ok(()) } else { Err("Cloudflare did not delete the relay.".into()) }
    }
}

/// Runs one operation to its end state, which is published through `host`. Returns the
/// options to repeat when setup stopped for an answer (`needs-account`, `needs-subdomain`).
pub(crate) fn run<H: Host>(host: &H, operation: Operation) -> Option<DeployOptions> {
    let mut flow = Flow { host, secrets: Vec::new(), details: String::new(), step: None };
    let result = match operation {
        Operation::Tools => flow.prepare().map(|_| {
            host.publish(SetupState::Idle { message: Some("Cloudflare's tools are ready.".into()) });
            None
        }),
        Operation::SignIn => flow.prepare().and_then(|prepared| flow.sign_in(&prepared.session)).map(|()| {
            host.publish(SetupState::Idle { message: Some("Signed in to Cloudflare.".into()) });
            None
        }),
        Operation::Deploy(options) => flow.deploy(options),
        Operation::Reuse => flow.reuse().map(|()| None),
        Operation::Remove { local_only } => flow.remove(local_only).map(|()| None),
    };
    match result {
        Ok(pending) => pending,
        // Cancel is not a failure: back to idle, with nothing to explain.
        Err(failure) if failure.message == wrangler::CANCELLED || host.cancelled() => {
            host.publish(SetupState::idle());
            None
        }
        Err(failure) => {
            let details = failure.details.or_else(|| flow.details()).map(|details| wrangler::redact(&details, &flow.secrets));
            host.publish(SetupState::Failed { message: failure.message, details, action: failure.action });
            None
        }
    }
}
