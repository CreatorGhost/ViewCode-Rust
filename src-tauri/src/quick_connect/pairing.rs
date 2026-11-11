//! Quick connect pairing: one-time tokens for a phone with no account.
//!
//! The QR code (a link) carries the relay's address and a random 256-bit token. Two codes
//! are derived from the token, so neither party that checks one learns the other:
//!
//! - the **relay code**, which the phone shows the relay to be registered as a device. The
//!   desktop gives the relay only the SHA-256 of it, so the Worker's storage never holds a
//!   usable code;
//! - the **host code**, which the phone then sends to this computer inside the end-to-end
//!   encrypted `pair.redeem` request. This ledger checks it, so a device is trusted here
//!   only if it really scanned the code, whatever the relay says.
//!
//! A token is valid for five minutes and works once; cancelling it ends it early. Tokens,
//! codes and links are never logged or printed (`Offer` has no `Debug`).
use base64::Engine;
use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use sha2::{Digest, Sha256};

pub(crate) const TTL_SECONDS: i64 = 5 * 60;
/// As many as the relay keeps waiting (`MAX_PENDING_PAIRINGS` in `relay/src/self.ts`).
pub(crate) const MAX_PENDING: usize = 8;
const RELAY_LABEL: &str = "pandacode quick connect relay v1";
const HOST_LABEL: &str = "pandacode quick connect host v1";
/// The link's scheme is the mobile app's (`mobile/app.json`).
const LINK: &str = "pandacode://quick-connect";

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn derive(label: &str, token: &str) -> String {
    b64(&Sha256::digest(format!("{label}\n{token}")))
}

pub(crate) fn relay_code(token: &str) -> String {
    derive(RELAY_LABEL, token)
}

pub(crate) fn host_code(token: &str) -> String {
    derive(HOST_LABEL, token)
}

pub(crate) fn sha256_hex(text: &str) -> String {
    Sha256::digest(text).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    b64(&bytes)
}

/// 256 random bits, URL-safe: the deployment's host secret. Never printed or logged.
pub(crate) fn new_host_secret() -> String {
    new_token()
}

fn percent(text: &str) -> String {
    text.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// What this computer tells the phone about itself, so the phone talks only to it.
pub(crate) struct Host<'a> {
    pub relay: &'a str,
    pub device_id: &'a str,
    /// Standard base64 X25519 public key, as the relay lists it.
    pub public_key: &'a str,
    pub name: &'a str,
}

/// The pairing link, which is also the QR code's content.
pub(crate) fn link(host: &Host, token: &str) -> String {
    format!("{LINK}?v=1&relay={}&token={token}&host={}&key={}&name={}", percent(host.relay), percent(host.device_id), percent(host.public_key), percent(host.name))
}

/// A fresh pairing. Deliberately not `Debug`: nothing should be able to print it.
pub(crate) struct Offer {
    pub token: String,
    /// What the relay is given: SHA-256 (hex) of the relay code.
    pub relay_code_hash: String,
    /// Unix seconds.
    pub expires_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Not a code this computer issued (or one already forgotten).
    Unknown,
    Expired,
    /// Already used, by this device or another.
    Used,
    /// Cancelled before anyone used it.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
enum State {
    Waiting,
    Used,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    /// SHA-256 (hex) of the host code; the code itself is not kept.
    host_code_hash: String,
    scope: String,
    expires_at: i64,
    state: State,
}

/// The pairings this computer has offered. In memory only: a restart ends them all, and
/// the relay's copies expire on their own.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    entries: Vec<Entry>,
}

impl Ledger {
    /// Forgets entries that expired more than an hour ago; until then a late or repeated
    /// code still gets its precise refusal.
    fn prune(&mut self, now: i64) {
        self.entries.retain(|entry| entry.expires_at + 3600 > now);
    }

    fn waiting(&self, now: i64) -> usize {
        self.entries.iter().filter(|entry| entry.state == State::Waiting && entry.expires_at > now).count()
    }

    /// A new pairing that grants `scope` when redeemed.
    pub(crate) fn issue(&mut self, scope: &str, now: i64) -> crate::Result<Offer> {
        self.prune(now);
        if self.waiting(now) >= MAX_PENDING {
            return Err("Too many pairing codes are waiting. Use or cancel one first.".into());
        }
        let token = new_token();
        let expires_at = now + TTL_SECONDS;
        self.entries.push(Entry { host_code_hash: sha256_hex(&host_code(&token)), scope: scope.to_owned(), expires_at, state: State::Waiting });
        Ok(Offer { relay_code_hash: sha256_hex(&relay_code(&token)), token, expires_at })
    }

    /// Checks a host code sent by a device and spends it. Returns the scope to grant.
    pub(crate) fn redeem(&mut self, code: &str, now: i64) -> Result<String, Refusal> {
        // Digests are compared, so nothing about a stored code leaks through timing.
        let hash = sha256_hex(code);
        let entry = self.entries.iter_mut().find(|entry| entry.host_code_hash == hash).ok_or(Refusal::Unknown)?;
        match entry.state {
            State::Used => Err(Refusal::Used),
            State::Cancelled => Err(Refusal::Cancelled),
            State::Waiting if entry.expires_at <= now => Err(Refusal::Expired),
            State::Waiting => {
                entry.state = State::Used;
                Ok(entry.scope.clone())
            }
        }
    }

    /// Ends every pairing nobody has used yet. Returns how many there were.
    pub(crate) fn cancel_all(&mut self, now: i64) -> usize {
        let mut cancelled = 0;
        for entry in self.entries.iter_mut().filter(|entry| entry.state == State::Waiting && entry.expires_at > now) {
            entry.state = State::Cancelled;
            cancelled += 1;
        }
        cancelled
    }

    /// When the newest waiting pairing expires (unix seconds), for the UI's countdown.
    pub(crate) fn waiting_until(&self, now: i64) -> Option<i64> {
        self.entries.iter().filter(|entry| entry.state == State::Waiting && entry.expires_at > now).map(|entry| entry.expires_at).max()
    }
}

/// Whether text sent to `pair.redeem` has the shape of a host code (and not a 6-digit code).
pub(crate) fn looks_like_host_code(text: &str) -> bool {
    text.len() == 43 && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivations_match_the_vector_shared_with_the_relay() {
        let vector: serde_json::Value = serde_json::from_str(include_str!("../../../relay/test/fixtures/quick-connect-pairing.json")).unwrap();
        let token = vector["token"].as_str().unwrap();
        assert_eq!(token, b64(&[7u8; 32]));
        assert_eq!(relay_code(token), vector["relayCode"].as_str().unwrap());
        assert_eq!(sha256_hex(&relay_code(token)), vector["relayCodeHash"].as_str().unwrap());
        assert_eq!(host_code(token), vector["hostCode"].as_str().unwrap());
        assert_ne!(relay_code(token), host_code(token));
        assert!(looks_like_host_code(&host_code(token)) && !looks_like_host_code("123456") && !looks_like_host_code(&"a".repeat(44)) && !looks_like_host_code(&"+".repeat(43)));
    }

    #[test]
    fn a_pairing_works_once_and_only_for_five_minutes() {
        let mut ledger = Ledger::default();
        let offer = ledger.issue("full", 1000).unwrap();
        assert_eq!(offer.expires_at, 1000 + 300);
        assert_eq!((offer.token.len(), offer.relay_code_hash.len()), (43, 64));
        assert_eq!(ledger.waiting_until(1000), Some(1300));
        let code = host_code(&offer.token);
        // The token itself, the relay's code and its hash are not the host code.
        for wrong in [offer.token.as_str(), &relay_code(&offer.token), &offer.relay_code_hash, "", "123456"] {
            assert_eq!(ledger.redeem(wrong, 1001), Err(Refusal::Unknown));
        }
        assert_eq!(ledger.redeem(&code, 1299), Ok("full".into()));
        assert_eq!(ledger.redeem(&code, 1299), Err(Refusal::Used));
        assert_eq!(ledger.waiting_until(1299), None);

        let late = ledger.issue("view", 2000).unwrap();
        assert_eq!(ledger.redeem(&host_code(&late.token), 2300), Err(Refusal::Expired));
        assert_eq!(ledger.redeem(&host_code(&late.token), 2299), Ok("view".into()));
        // Two offers never share a token.
        assert_ne!(ledger.issue("full", 3000).unwrap().token, ledger.issue("full", 3000).unwrap().token);
    }

    #[test]
    fn cancelling_ends_waiting_pairings_and_leaves_used_ones_used() {
        let mut ledger = Ledger::default();
        let used = ledger.issue("full", 0).unwrap();
        let waiting = ledger.issue("full", 0).unwrap();
        assert_eq!(ledger.redeem(&host_code(&used.token), 1), Ok("full".into()));
        assert_eq!(ledger.cancel_all(2), 1);
        assert_eq!(ledger.redeem(&host_code(&waiting.token), 3), Err(Refusal::Cancelled));
        assert_eq!(ledger.redeem(&host_code(&used.token), 3), Err(Refusal::Used));
        assert_eq!((ledger.cancel_all(4), ledger.waiting_until(4)), (0, None));
    }

    #[test]
    fn only_a_few_wait_at_once_and_old_ones_are_forgotten() {
        let mut ledger = Ledger::default();
        let offers: Vec<Offer> = (0..MAX_PENDING).map(|_| ledger.issue("full", 0).unwrap()).collect();
        assert!(ledger.issue("full", 0).is_err());
        // Expired ones no longer count against the cap.
        assert!(ledger.issue("full", 300).is_ok());
        assert_eq!(ledger.redeem(&host_code(&offers[0].token), 301), Err(Refusal::Expired));
        // An hour after expiry an entry is gone altogether.
        ledger.issue("full", 3901).unwrap();
        assert_eq!(ledger.redeem(&host_code(&offers[0].token), 3902), Err(Refusal::Unknown));
    }

    #[test]
    fn the_link_carries_the_relay_the_token_and_the_host_identity() {
        let host = Host { relay: "https://pandacode-quick-connect.me.workers.dev", device_id: "3f2b8c1e-9a4d-4e0b-8c55-0a1b2c3d4e5f", public_key: "ab+/cd==", name: "Ada's Mac" };
        assert_eq!(
            link(&host, "TOKEN_-"),
            "pandacode://quick-connect?v=1&relay=https%3A%2F%2Fpandacode-quick-connect.me.workers.dev&token=TOKEN_-&host=3f2b8c1e-9a4d-4e0b-8c55-0a1b2c3d4e5f&key=ab%2B%2Fcd%3D%3D&name=Ada%27s%20Mac"
        );
        assert_eq!(new_host_secret().len(), 43);
        assert_ne!(new_host_secret(), new_host_secret());
    }
}
