//! Quick connect's side of PandaCode Connect: the same socket, frames, encryption and host
//! RPC, pointed at the relay on the user's own Cloudflare account. This computer proves
//! the deployment's host secret instead of signing in with GitHub; everything after that
//! token is the ordinary remote stack. Driven by `crate::quick_connect`.
use super::{auth, config, crypto, remote, secrets, socket};
use crate::{cloud::http, Result};
use serde_json::{json, Value};
use std::time::Duration;

const ACCOUNT_LOGIN: &str = "Quick connect";
const REFUSED: &str = "The relay did not accept this computer's secret. Redeploy with a new secret to fix it.";

/// Whether this computer is signed in to the shared, GitHub-based relay (not its own).
pub(crate) fn shared_signed_in(app: &tauri::AppHandle) -> bool {
    !config::self_relay_active() && remote(app).lock().token.is_some()
}

/// Points the remote stack at the user's own relay (or back at the build's shared one).
pub(crate) fn set_relay(url: Option<String>) {
    config::set_self_relay(url);
}

/// Whether this computer holds a token for its own relay.
pub(crate) fn attached(app: &tauri::AppHandle) -> bool {
    config::self_relay_active() && remote(app).lock().token.is_some()
}

pub(crate) fn connected(app: &tauri::AppHandle) -> bool {
    config::self_relay_active() && remote(app).lock().connected
}

/// Ids of the devices presence lists as online.
pub(crate) fn online(app: &tauri::AppHandle) -> std::collections::HashSet<String> {
    remote(app).lock().devices.iter().filter(|device| device.online).map(|device| device.id.clone()).collect()
}

/// This computer as the pairing link names it: (device id, public key, name).
pub(crate) fn identity(app: &tauri::AppHandle) -> Result<(String, String, String)> {
    let state = remote(app);
    Ok((state.device_id(), crypto::public_key(&state.secret()?), config::device_name()))
}

async fn send(url: &str, host_secret: &str, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<(u16, Value)> {
    let mut request = http::client(Duration::from_secs(20))?.request(method, format!("{}{path}", url.trim_end_matches('/'))).bearer_auth(host_secret);
    if let Some(body) = body {
        request = request.header("Content-Type", "application/json").body(body.to_string());
    }
    let response = request.send().await.map_err(|e| if e.is_timeout() { "The relay did not answer in time.".to_string() } else { "Could not reach the relay. Check your connection.".to_string() })?;
    let status = response.status().as_u16();
    let bytes = response.bytes().await.map_err(|_| "The relay's response was cut off.")?;
    Ok((status, if bytes.len() <= 64 * 1024 { serde_json::from_slice(&bytes).unwrap_or(Value::Null) } else { Value::Null }))
}

/// A host-secret call to the deployment's own API (`/v1/self/*`). Errors never carry the secret.
pub(crate) async fn call(url: &str, host_secret: &str, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value> {
    match send(url, host_secret, method, path, body).await? {
        (200..=299, value) => Ok(value),
        (401 | 403, _) => Err(REFUSED.into()),
        (409, value) if value["error"] == "device key changed" => Err(auth::KEY_CHANGED.into()),
        (status, value) => Err(value["error"].as_str().map(|error| format!("Relay: {}", error.chars().take(300).collect::<String>())).unwrap_or_else(|| format!("The relay returned HTTP {status}."))),
    }
}

/// The HTTP status of `GET /v1/self/probe` and whether the answer is a self-hosted relay's; none when nothing answered.
pub(crate) async fn probe(url: &str, host_secret: &str) -> std::result::Result<(u16, bool), bool> {
    let client = http::client(Duration::from_secs(15)).map_err(|_| false)?;
    match client.get(format!("{}/v1/self/probe", url.trim_end_matches('/'))).bearer_auth(host_secret).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let body: Value = response.json().await.unwrap_or(Value::Null);
            Ok((status, body["mode"] == "self"))
        }
        // true: the connection itself was refused or reset (a firewall), not merely slow.
        Err(error) => Err(error.is_connect() && !error.is_timeout()),
    }
}

/// Registers this computer as its relay's host with the host secret, stores the relay token
/// and starts the socket. Turns "Allow remote control" on: a paired phone is the point.
pub(crate) async fn attach(app: &tauri::AppHandle, url: &str, host_secret: &str) -> Result<()> {
    if shared_signed_in(app) {
        return Err("Sign out of PandaCode Connect before using Quick connect.".into());
    }
    let state = remote(app);
    let public_key = crypto::public_key(&state.ensure_secret()?);
    let device = |id: String| json!({"device": {"id": id, "name": config::device_name(), "platform": std::env::consts::OS, "publicKey": public_key}});
    let reply = match call(url, host_secret, reqwest::Method::POST, "/v1/self/host", Some(device(state.device_id()))).await {
        // This id is registered with another key (the key file was lost, say): become a new device once.
        Err(error) if error == auth::KEY_CHANGED => {
            let device_id = uuid::Uuid::new_v4().to_string();
            state.lock().settings.device_id = device_id.clone();
            state.save();
            call(url, host_secret, reqwest::Method::POST, "/v1/self/host", Some(device(device_id))).await?
        }
        other => other?,
    };
    let token = reply["token"].as_str().filter(|token| token.len() <= 4096).ok_or("The relay did not return a token.")?.to_owned();
    if config::parse_token(&token).is_none_or(|claims| claims.dev != state.device_id()) {
        return Err("The relay returned an unexpected sign-in token.".into());
    }
    secrets::set_token(&state.dir, &token)?;
    config::set_self_relay(Some(url.to_owned()));
    {
        let mut inner = state.lock();
        inner.token = Some(token);
        inner.settings.account = Some(config::Account { id: "self".into(), login: ACCOUNT_LOGIN.into(), avatar_url: String::new() });
        inner.settings.host_enabled = true;
    }
    state.save();
    socket::start(app);
    super::emit_status(app);
    Ok(())
}

/// Disconnects from the user's own relay and forgets its token. The shared relay's
/// sign-in, if any, is not this function's to touch.
pub(crate) fn detach(app: &tauri::AppHandle) {
    if config::self_relay_active() {
        super::sign_out_local(app, None);
        config::set_self_relay(None);
        super::emit_status(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::http::{block, mock};

    const SECRET: &str = "host-secret-0123456789abcdefghijklmnopqrstu";

    #[test]
    fn a_relay_of_ones_own_is_an_https_address_without_a_github_app() {
        let build = config::self_build("https://pandacode-quick-connect.me.workers.dev/").unwrap();
        assert_eq!((build.relay.as_str(), build.client_id.as_str()), ("https://pandacode-quick-connect.me.workers.dev", ""));
        for bad in ["http://pandacode-quick-connect.me.workers.dev", "pandacode-quick-connect.me.workers.dev", "https://", "https://a b.workers.dev"] {
            assert!(config::self_build(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn host_calls_carry_the_secret_in_the_header_and_never_in_an_error() {
        let (base, seen) = mock::serve(|request, _| match (request.method.as_str(), request.path.as_str(), request.authorization.as_deref()) {
            (_, _, auth) if auth != Some(&format!("Bearer {SECRET}")) => (401, json!({"error": "invalid host secret"}).to_string()),
            ("POST", "/v1/self/pairings", _) => (200, json!({"ok": true}).to_string()),
            ("POST", "/v1/self/host", _) => (409, json!({"error": "device key changed"}).to_string()),
            ("GET", "/v1/self/probe", _) => (200, json!({"ok": true, "mode": "self", "protocol": 1}).to_string()),
            ("DELETE", "/v1/self/pairings", _) => (409, json!({"error": format!("too many for {SECRET}")}).to_string()),
            _ => (500, "server error".to_string()),
        });
        let offer = json!({"codeHash": "a".repeat(64), "ttlSeconds": 300});
        assert_eq!(block(call(&base, SECRET, reqwest::Method::POST, "/v1/self/pairings", Some(offer.clone()))), Ok(json!({"ok": true})));
        let sent = seen.lock().unwrap().last().cloned().unwrap();
        assert_eq!((sent.body.as_str(), sent.path.contains(SECRET)), (offer.to_string().as_str(), false));
        // A trailing slash on the stored address does not double up.
        assert!(block(call(&format!("{base}/"), SECRET, reqwest::Method::POST, "/v1/self/pairings", Some(offer))).is_ok());
        assert_eq!(seen.lock().unwrap().last().unwrap().path, "/v1/self/pairings");

        assert_eq!(block(call(&base, "wrong-secret", reqwest::Method::POST, "/v1/self/pairings", None)), Err(REFUSED.into()));
        assert_eq!(block(call(&base, SECRET, reqwest::Method::POST, "/v1/self/host", Some(json!({})))), Err(auth::KEY_CHANGED.into()));
        assert_eq!(block(call(&base, SECRET, reqwest::Method::GET, "/v1/self/nope", None)), Err("The relay returned HTTP 500.".into()));
        assert!(!REFUSED.contains(SECRET));

        assert_eq!(block(probe(&base, SECRET)), Ok((200, true)));
        assert_eq!(block(probe(&base, "wrong-secret")), Ok((401, false)));
        // Nothing listening: refused, as a firewall resetting a new hostname looks.
        assert_eq!(block(probe("http://127.0.0.1:9", SECRET)), Err(true));
    }
}
