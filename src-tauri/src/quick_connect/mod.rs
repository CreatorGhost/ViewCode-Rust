//! Quick connect: reach this computer from the phone through a relay on the user's **own**
//! Cloudflare account (docs/remote-access.md, "Quick connect"). The flow is ViewCode's,
//! step for step: sign in to Cloudflare from the app, the app deploys the relay Worker into
//! that account, shows a link/QR code, and the phone pairs with no account of its own.
//!
//! The relay is this repository's `relay/` Worker in its self-hosted mode, so frames stay
//! end-to-end encrypted and the remote stack (`crate::remote`) is reused as it is. The
//! per-deployment host secret lives in `secret_store` and as a Worker secret; it never
//! reaches the webview and is never logged. Nothing here runs a tool or touches the network
//! until the user starts setup.
mod node;
mod pairing;
mod process;
mod setup;
#[cfg(all(test, unix))]
mod setup_stream_tests;
mod state;
mod tools;
mod worker;
mod wrangler;

use crate::remote::self_relay;
use crate::Result;
use serde_json::{json, Value};
use setup::{DeployOptions, Fail, Operation, Prepared, Probe, Problem, SetupState};
use state::{Config, Device, Event, Mode};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tauri::{Emitter, Manager};

const SECRET_SERVICE: &str = "pandacode-quick-connect";
const SECRET_ACCOUNT: &str = "host-secret";
/// Emitted with the full status whenever anything in it changes.
const STATUS_EVENT: &str = "quick-connect://status";
/// Emitted when a phone finishes pairing: `{ deviceId, name, scope }`.
const PAIRED_EVENT: &str = "quick-connect://paired";
const NOT_SET_UP: &str = "Set up Quick connect first.";

struct Inner {
    config: Config,
    setup: SetupState,
    cancel: Arc<AtomicBool>,
    /// What `quick_connect_continue` and `quick_connect_choose_account` repeat.
    pending: Option<DeployOptions>,
    ledger: pairing::Ledger,
    /// Which Node the last run used: "system" or "downloaded".
    tools: Option<(&'static str, String)>,
}

pub(crate) struct QuickConnect {
    dir: PathBuf,
    inner: Mutex<Inner>,
}

impl QuickConnect {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn quick(app: &tauri::AppHandle) -> tauri::State<'_, QuickConnect> {
    app.state::<QuickConnect>()
}

fn host_secret() -> Option<String> {
    crate::secret_store::get(SECRET_SERVICE, SECRET_ACCOUNT)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Loads the settings. A computer that has Quick connect on points the remote stack at its
/// own relay; must run before `remote::setup`. No network, no process.
pub(crate) fn setup(app: &tauri::AppHandle, dir: &std::path::Path) {
    let config = state::load(dir);
    self_relay::set_relay(config.active_relay().map(|relay| relay.url.clone()));
    app.manage(QuickConnect {
        dir: dir.to_owned(),
        inner: Mutex::new(Inner { config, setup: SetupState::idle(), cancel: Arc::default(), pending: None, ledger: Default::default(), tools: None }),
    });
}

/// After `remote::setup`: a computer whose relay token is gone (it expired while the app
/// was closed) registers again with the host secret. Only when Quick connect is on.
pub(crate) fn resume(app: &tauri::AppHandle) {
    let relay = quick(app).lock().config.active_relay().cloned();
    let Some(relay) = relay.filter(|_| !self_relay::attached(app)) else { return };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(secret) = host_secret() {
            let _ = self_relay::attach(&app, &relay.url, &secret).await;
            emit(&app);
        }
    });
}

/// Where a file on disk says Tailscale is installed. Looks; never runs anything.
fn tailscale_installed() -> bool {
    crate::providers::tool_path("tailscale").is_some() || ["/Applications/Tailscale.app/Contents/MacOS/Tailscale", "/usr/bin/tailscale", "/usr/sbin/tailscale", "C:\\Program Files\\Tailscale\\tailscale.exe"].iter().any(|path| std::path::Path::new(path).is_file())
}

const NEEDS_LISTENER: &str = "Not available yet: this needs the app's local network listener, which is not built.";

fn status(app: &tauri::AppHandle) -> Value {
    let (attached, connected, shared, online) = (self_relay::attached(app), self_relay::connected(app), self_relay::shared_signed_in(app), self_relay::online(app));
    let state = quick(app);
    let inner = state.lock();
    let now = chrono::Utc::now().timestamp();
    let devices: Vec<Value> = inner.config.devices.iter().map(|device| device_view(device, online.contains(&device.id))).collect();
    json!({
        "stage": inner.config.stage(),
        "mode": inner.config.mode,
        "enabled": inner.config.enabled,
        "relay": inner.config.relay,
        "attached": attached,
        "connected": connected,
        "setup": inner.setup,
        "canContinue": inner.pending.is_some(),
        "tools": inner.tools.as_ref().map(|(source, version)| json!({"source": source, "nodeVersion": version})),
        "devices": devices,
        "pairing": inner.ledger.waiting_until(now).map(|expires| json!({"expiresAt": expires * 1000})),
        "modes": {
            "quickConnect": {"available": true},
            "sameWifi": {"available": false, "reason": NEEDS_LISTENER},
            "tailscale": {"available": false, "installed": tailscale_installed(), "reason": NEEDS_LISTENER},
        },
        // The older GitHub-based shared relay: hidden unless asked for, or already in use.
        "legacyConnect": {"visible": inner.config.legacy_connect_visible || shared, "signedIn": shared},
    })
}

fn device_view(device: &Device, online: bool) -> Value {
    json!({"deviceId": device.id, "name": device.name, "platform": device.platform, "fingerprint": device.fingerprint, "scope": device.scope, "pairedAt": device.paired_at, "online": online})
}

fn emit(app: &tauri::AppHandle) {
    let _ = app.emit(STATUS_EVENT, status(app));
}

/// Applies a state-machine event, saves it and keeps the remote stack's relay in step.
fn record(app: &tauri::AppHandle, event: Event) -> Result<()> {
    let state = quick(app);
    let (config, changed) = {
        let mut inner = state.lock();
        let changed = inner.config.apply(event);
        (inner.config.clone(), changed)
    };
    if changed {
        state::save(&state.dir, &config)?;
        emit(app);
    }
    Ok(())
}

struct AppHost {
    app: tauri::AppHandle,
    cancel: Arc<AtomicBool>,
}

impl setup::Host for AppHost {
    fn publish(&self, state: SetupState) {
        quick(&self.app).lock().setup = state;
        emit(&self.app);
    }

    fn now_ms(&self) -> i64 {
        now_ms()
    }

    fn prepare(&self, progress: &dyn Fn(&str)) -> std::result::Result<Prepared, Fail> {
        let finder = tools::Finder {
            node: crate::providers::tool_path("node"),
            npx: crate::providers::tool_path("npx"),
            downloads: quick(&self.app).dir.join("quick-connect").join("node"),
            dist: node::NODE_DIST.into(),
            version: node::PINNED_NODE,
            release: node::target().and_then(node::release_for),
        };
        let tools = tools::prepare(&finder, &self.cancel, progress)?;
        let (source, version) = (if matches!(tools.launcher, wrangler::Launcher::System { .. }) { "system" } else { "downloaded" }, tools.node_version);
        quick(&self.app).lock().tools = Some((source, format!("{}.{}.{}", version.0, version.1, version.2)));
        let staged = worker::stage(&std::env::temp_dir())?;
        let session = wrangler::Session { launcher: tools.launcher, node_version: tools.node_version, cwd: staged.dir.clone(), account_id: None, cancel: self.cancel.clone() };
        Ok(Prepared { session, staged })
    }

    fn read_secret(&self) -> Option<String> {
        host_secret()
    }

    fn write_secret(&self, secret: &str) -> Result<()> {
        crate::secret_store::set(SECRET_SERVICE, SECRET_ACCOUNT, secret)
    }

    fn remove_secret(&self) {
        crate::secret_store::remove(SECRET_SERVICE, SECRET_ACCOUNT);
    }

    fn relay(&self) -> Option<state::Relay> {
        quick(&self.app).lock().config.relay.clone()
    }

    fn record(&self, event: Event) -> Result<()> {
        record(&self.app, event)
    }

    fn probe(&self, url: &str, secret: &str) -> Probe {
        match crate::cloud::http::block(self_relay::probe(url, secret)) {
            Ok((status, self_hosted)) => setup::classify_probe(status, self_hosted),
            Err(refused) => Probe::Problem(if refused { Problem::NetworkRefused } else { Problem::Transient }),
        }
    }

    fn attach(&self, url: &str, secret: &str) -> Result<()> {
        crate::cloud::http::block(self_relay::attach(&self.app, url, secret))
    }

    fn detach(&self) {
        quick(&self.app).lock().ledger.cancel_all(chrono::Utc::now().timestamp());
        self_relay::detach(&self.app);
    }

    fn pause(&self, seconds: u64) -> bool {
        let until = std::time::Instant::now() + Duration::from_secs(seconds);
        while std::time::Instant::now() < until {
            if self.cancelled() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        !self.cancelled()
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// Claims the single slot and runs `operation` off the async runtime; its end state is
/// published as the setup state, so closing the window does not stop it.
fn launch(app: &tauri::AppHandle, operation: Operation) -> Result<Value> {
    if matches!(operation, Operation::Deploy(_) | Operation::Reuse) && self_relay::shared_signed_in(app) {
        return Err("Sign out of PandaCode Connect before using Quick connect.".into());
    }
    let cancel = {
        let state = quick(app);
        let mut inner = state.lock();
        if inner.setup.is_running() {
            return Err("Quick connect setup is already running.".into());
        }
        inner.cancel = Arc::default();
        inner.pending = None;
        inner.setup = SetupState::Running { step: setup::Step::CheckingTools, step_started_at: now_ms(), message: "Getting ready…".into(), sign_in: None, problem: None };
        inner.cancel.clone()
    };
    emit(app);
    let host = AppHost { app: app.clone(), cancel };
    tauri::async_runtime::spawn_blocking(move || {
        let pending = setup::run(&host, operation);
        let state = quick(&host.app);
        let mut inner = state.lock();
        inner.pending = pending;
        // Whatever happened, the slot is free again.
        if inner.setup.is_running() {
            inner.setup = SetupState::idle();
        }
        drop(inner);
        emit(&host.app);
    });
    Ok(status(app))
}

// ---- Hooks from the remote stack ----

pub(crate) fn is_host_code(code: &str) -> bool {
    pairing::looks_like_host_code(code)
}

/// Host side of a Quick connect pairing: checks and spends the code a phone derived from
/// the link shown here. Returns the scope that pairing grants.
pub(crate) fn redeem_host_code(app: &tauri::AppHandle, code: &str, now: i64) -> Result<String> {
    quick(app).lock().ledger.redeem(code, now).map_err(|_| "That pairing code is not valid or has expired.".to_string())
}

/// A phone redeemed its pairing: remember it and tell the window.
pub(crate) fn paired(app: &tauri::AppHandle, id: &str, name: &str, platform: &str, fingerprint: &str, scope: &str) {
    let device = Device { id: id.into(), name: name.into(), platform: platform.into(), fingerprint: fingerprint.into(), scope: scope.into(), paired_at: now_ms() };
    if let Err(error) = record(app, Event::Paired(device)) {
        eprintln!("{error}");
    }
    let _ = app.emit(PAIRED_EVENT, json!({"deviceId": id, "name": name, "scope": scope}));
}

// ---- IPC ----

/// Everything the Connect phone UI shows. Reads memory only.
#[tauri::command]
pub async fn quick_connect_status(app: tauri::AppHandle) -> Value {
    status(&app)
}

/// "Connect with Cloudflare": tools, sign-in, deploy, secret, verify, connect. Also the
/// redeploys (`rotateSecret` makes a new host secret, which signs every paired phone out).
#[tauri::command]
pub async fn quick_connect_start_setup(app: tauri::AppHandle, rotate_secret: Option<bool>) -> Result<Value> {
    launch(&app, Operation::Deploy(DeployOptions { rotate_secret: rotate_secret.unwrap_or(false), account_id: None }))
}

/// Step 1 on its own: find Node.js 22+ or download the pinned release.
#[tauri::command]
pub async fn quick_connect_prepare_tools(app: tauri::AppHandle) -> Result<Value> {
    launch(&app, Operation::Tools)
}

/// Steps 1–2 on their own: the Cloudflare sign-in, through wrangler.
#[tauri::command]
pub async fn quick_connect_sign_in(app: tauri::AppHandle) -> Result<Value> {
    launch(&app, Operation::SignIn)
}

/// After `needs-subdomain`, once the account has a workers.dev address: deploy again.
#[tauri::command]
pub async fn quick_connect_continue(app: tauri::AppHandle) -> Result<Value> {
    let pending = quick(&app).lock().pending.clone().ok_or("There is nothing to continue.")?;
    launch(&app, Operation::Deploy(pending))
}

/// Answers `needs-account`: deploy on this account, which is then remembered.
#[tauri::command]
pub async fn quick_connect_choose_account(app: tauri::AppHandle, account_id: String) -> Result<Value> {
    if account_id.is_empty() || account_id.len() > 64 || !account_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("That is not a Cloudflare account.".into());
    }
    let pending = quick(&app).lock().pending.clone().ok_or("There is nothing to continue.")?;
    launch(&app, Operation::Deploy(DeployOptions { account_id: Some(account_id), ..pending }))
}

/// Stops the running operation and the tool it started, or dismisses a finished one.
#[tauri::command]
pub async fn quick_connect_cancel(app: tauri::AppHandle) -> Value {
    {
        let state = quick(&app);
        let mut inner = state.lock();
        if inner.setup.is_running() {
            inner.cancel.store(true, Ordering::SeqCst);
        } else {
            inner.setup = SetupState::idle();
            inner.pending = None;
        }
    }
    emit(&app);
    status(&app)
}

/// Turns a deployed relay off (it stays on Cloudflare; this computer disconnects) or on again.
#[tauri::command]
pub async fn quick_connect_set_enabled(app: tauri::AppHandle, enabled: bool) -> Result<Value> {
    if enabled {
        return launch(&app, Operation::Reuse);
    }
    if quick(&app).lock().setup.is_running() {
        return Err("Quick connect setup is already running.".into());
    }
    quick(&app).lock().ledger.cancel_all(chrono::Utc::now().timestamp());
    self_relay::detach(&app);
    record(&app, Event::Enabled(false))?;
    Ok(status(&app))
}

/// Deletes the relay from the Cloudflare account and forgets it here; `localOnly` forgets
/// it here without contacting Cloudflare.
#[tauri::command]
pub async fn quick_connect_remove(app: tauri::AppHandle, local_only: Option<bool>) -> Result<Value> {
    launch(&app, Operation::Remove { local_only: local_only.unwrap_or(false) })
}

/// A new one-time pairing: `{ link, expiresAt, scope }`. `link` is also the QR code's
/// content. Valid five minutes, single use. Never logged.
#[tauri::command]
pub async fn quick_connect_pairing_create(app: tauri::AppHandle, scope: Option<String>) -> Result<Value> {
    let scope = scope.unwrap_or_else(|| "full".into());
    if !matches!(scope.as_str(), "view" | "control" | "full") {
        return Err("A pairing can give view, control or full access.".into());
    }
    let relay = quick(&app).lock().config.active_relay().cloned().ok_or(NOT_SET_UP)?;
    if !self_relay::attached(&app) {
        return Err("This computer is not connected to its relay yet. Turn Quick connect on again.".into());
    }
    let secret = host_secret().ok_or(NOT_SET_UP)?;
    let (device_id, public_key, name) = self_relay::identity(&app)?;
    let offer = quick(&app).lock().ledger.issue(&scope, chrono::Utc::now().timestamp())?;
    self_relay::call(&relay.url, &secret, reqwest::Method::POST, "/v1/self/pairings", Some(json!({"codeHash": offer.relay_code_hash, "ttlSeconds": pairing::TTL_SECONDS}))).await?;
    emit(&app);
    let link = pairing::link(&pairing::Host { relay: &relay.url, device_id: &device_id, public_key: &public_key, name: &name }, &offer.token);
    Ok(json!({"link": link, "expiresAt": offer.expires_at * 1000, "scope": scope}))
}

/// Ends every pairing nobody has used yet, here and on the relay.
#[tauri::command]
pub async fn quick_connect_pairing_cancel(app: tauri::AppHandle) -> Result<Value> {
    let relay = quick(&app).lock().config.relay.clone();
    quick(&app).lock().ledger.cancel_all(chrono::Utc::now().timestamp());
    emit(&app);
    if let (Some(relay), Some(secret)) = (relay, host_secret()) {
        self_relay::call(&relay.url, &secret, reqwest::Method::DELETE, "/v1/self/pairings", None).await?;
    }
    Ok(status(&app))
}

/// The phones paired through Quick connect, with whether each is online now.
#[tauri::command]
pub async fn quick_connect_devices(app: tauri::AppHandle) -> Vec<Value> {
    let online = self_relay::online(&app);
    quick(&app).lock().config.devices.iter().map(|device| device_view(device, online.contains(&device.id))).collect()
}

/// Removes a paired phone: its relay registration (so its credential stops working at
/// once), what it was allowed to do here, and its row.
#[tauri::command]
pub async fn quick_connect_revoke_device(app: tauri::AppHandle, device_id: String) -> Result<Value> {
    if !quick(&app).lock().config.devices.iter().any(|device| device.id == device_id) {
        return Err("That phone is not paired with this computer.".into());
    }
    if !self_relay::attached(&app) {
        return Err("This computer is not connected to its relay yet. Turn Quick connect on again.".into());
    }
    match crate::remote::remote_revoke_device(app.clone(), device_id.clone()).await {
        Ok(_) => {}
        // Already gone from the relay: finish removing it here.
        Err(error) if error == "Relay: device not found" => {}
        Err(error) => return Err(error),
    }
    crate::remote::grants::forget(&app, &device_id);
    record(&app, Event::Revoked(device_id))?;
    Ok(status(&app))
}

/// Chooses how the phone reaches this computer: `quickConnect`, `sameWifi` or `tailscale`.
/// Only stores the choice; it never starts Tailscale or anything else.
#[tauri::command]
pub async fn quick_connect_set_mode(app: tauri::AppHandle, mode: String) -> Result<Value> {
    let mode = Mode::parse(&mode)?;
    let state = quick(&app);
    let config = {
        let mut inner = state.lock();
        inner.config.mode = mode;
        inner.config.clone()
    };
    state::save(&state.dir, &config)?;
    emit(&app);
    Ok(status(&app))
}

/// Whether Settings offers the older GitHub-based PandaCode Connect again. Off by default.
#[tauri::command]
pub async fn quick_connect_set_legacy_connect_visible(app: tauri::AppHandle, visible: bool) -> Result<Value> {
    let state = quick(&app);
    let config = {
        let mut inner = state.lock();
        inner.config.legacy_connect_visible = visible;
        inner.config.clone()
    };
    state::save(&state.dir, &config)?;
    emit(&app);
    Ok(status(&app))
}
