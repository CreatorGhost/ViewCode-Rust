//! What Quick connect remembers between launches (`quick-connect.json` in the app data
//! dir, never a secret) and the setup state machine over it:
//!
//! not started → node ready → signed in → deployed → paired
//!
//! The stage is derived from what is stored, so it cannot disagree with it: removing the
//! relay falls back to "signed in" (Cloudflare's tool is still there and signed in),
//! revoking the last device falls back to "deployed".
use serde::{Deserialize, Serialize};
use std::path::Path;

const FILE: &str = "quick-connect.json";
const LABEL: &str = "Quick connect settings";

/// How the phone reaches this computer. An explicit choice, as in ViewCode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Mode {
    /// Through the relay on the user's own Cloudflare account.
    #[default]
    QuickConnect,
    SameWifi,
    /// `tailscale serve`. Choosing it never runs `tailscale`.
    Tailscale,
}

impl Mode {
    pub(crate) fn parse(text: &str) -> crate::Result<Mode> {
        match text {
            "quickConnect" => Ok(Mode::QuickConnect),
            "sameWifi" => Ok(Mode::SameWifi),
            "tailscale" => Ok(Mode::Tailscale),
            _ => Err("Unknown connection choice.".into()),
        }
    }
}

/// The deployed Worker, for redeploys and removal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Relay {
    pub name: String,
    pub url: String,
    /// The Cloudflare account the Worker went to, when setup knew it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

/// A phone paired through a Quick connect code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Device {
    pub id: String,
    pub name: String,
    pub platform: String,
    /// Fingerprint of the key it paired with.
    pub fingerprint: String,
    pub scope: String,
    /// Unix milliseconds.
    pub paired_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Config {
    pub mode: Mode,
    pub tools_ready: bool,
    pub signed_in: bool,
    pub relay: Option<Relay>,
    /// Off keeps the relay deployed but this computer disconnected from it.
    pub enabled: bool,
    pub devices: Vec<Device>,
    /// Whether Settings still offers the older GitHub-based PandaCode Connect. Off by
    /// default; its code keeps working either way.
    pub legacy_connect_visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Stage {
    NotStarted,
    NodeReady,
    SignedIn,
    Deployed,
    Paired,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    ToolsReady,
    SignedIn,
    Deployed(Relay),
    Enabled(bool),
    Paired(Device),
    Revoked(String),
    Removed,
}

impl Config {
    pub(crate) fn stage(&self) -> Stage {
        match (&self.relay, self.signed_in, self.tools_ready) {
            (Some(_), _, _) if !self.devices.is_empty() => Stage::Paired,
            (Some(_), _, _) => Stage::Deployed,
            (None, true, _) => Stage::SignedIn,
            (None, false, true) => Stage::NodeReady,
            (None, false, false) => Stage::NotStarted,
        }
    }

    /// Applies one step. Returns false when the event does not apply in this state and
    /// nothing changed (a pairing with no relay, enabling nothing).
    pub(crate) fn apply(&mut self, event: Event) -> bool {
        match event {
            Event::ToolsReady => self.tools_ready = true,
            Event::SignedIn => {
                self.tools_ready = true;
                self.signed_in = true;
            }
            Event::Deployed(relay) => {
                // Phones paired with another deployment are not on this one.
                if self.relay.as_ref().is_some_and(|current| current.url != relay.url) {
                    self.devices.clear();
                }
                self.tools_ready = true;
                self.signed_in = true;
                self.enabled = true;
                self.relay = Some(relay);
            }
            Event::Enabled(enabled) => {
                if self.relay.is_none() {
                    return false;
                }
                self.enabled = enabled;
            }
            Event::Paired(device) => {
                if self.relay.is_none() {
                    return false;
                }
                self.devices.retain(|known| known.id != device.id);
                self.devices.push(device);
            }
            Event::Revoked(id) => {
                let before = self.devices.len();
                self.devices.retain(|device| device.id != id);
                return self.devices.len() != before;
            }
            Event::Removed => {
                self.relay = None;
                self.enabled = false;
                self.devices.clear();
            }
        }
        true
    }

    /// The relay this computer should be connected to right now.
    pub(crate) fn active_relay(&self) -> Option<&Relay> {
        self.relay.as_ref().filter(|_| self.enabled)
    }
}

/// Missing reads as the defaults; so does an unreadable or corrupt file, logged.
pub(crate) fn load(dir: &Path) -> Config {
    crate::secret_store::load(&dir.join(FILE), LABEL).unwrap_or_else(|error| {
        eprintln!("{error}");
        Config::default()
    })
}

pub(crate) fn save(dir: &Path, config: &Config) -> crate::Result<()> {
    let config = config.clone();
    crate::secret_store::update(dir, FILE, LABEL, |_: Config| config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay(url: &str) -> Relay {
        Relay { name: "pandacode-quick-connect".into(), url: url.into(), account_id: Some("abc".into()) }
    }

    fn device(id: &str) -> Device {
        Device { id: id.into(), name: "Phone".into(), platform: "ios".into(), fingerprint: "f".into(), scope: "full".into(), paired_at: 1 }
    }

    #[test]
    fn setup_moves_through_its_stages_in_order() {
        let mut config = Config::default();
        assert_eq!(config.stage(), Stage::NotStarted);
        assert!(config.apply(Event::ToolsReady));
        assert_eq!(config.stage(), Stage::NodeReady);
        assert!(config.apply(Event::SignedIn));
        assert_eq!(config.stage(), Stage::SignedIn);
        assert!(config.apply(Event::Deployed(relay("https://a.b.workers.dev"))));
        assert_eq!((config.stage(), config.enabled), (Stage::Deployed, true));
        assert!(config.apply(Event::Paired(device("one"))));
        assert_eq!(config.stage(), Stage::Paired);
        assert!(Stage::NotStarted < Stage::NodeReady && Stage::NodeReady < Stage::SignedIn && Stage::SignedIn < Stage::Deployed && Stage::Deployed < Stage::Paired);
    }

    #[test]
    fn later_steps_imply_the_earlier_ones() {
        // A machine with its own Node that was already signed in to wrangler.
        let mut config = Config::default();
        config.apply(Event::SignedIn);
        assert!(config.tools_ready);
        let mut config = Config::default();
        config.apply(Event::Deployed(relay("https://a.b.workers.dev")));
        assert!(config.tools_ready && config.signed_in && config.stage() == Stage::Deployed);
    }

    #[test]
    fn pairing_needs_a_relay_and_revoking_the_last_device_steps_back() {
        let mut config = Config::default();
        assert!(!config.apply(Event::Paired(device("one"))));
        assert!(!config.apply(Event::Enabled(true)));
        assert_eq!((config.stage(), config.devices.len(), config.enabled), (Stage::NotStarted, 0, false));

        config.apply(Event::Deployed(relay("https://a.b.workers.dev")));
        config.apply(Event::Paired(device("one")));
        config.apply(Event::Paired(device("two")));
        // Pairing the same phone again replaces its row.
        config.apply(Event::Paired(Device { name: "Renamed".into(), ..device("one") }));
        assert_eq!(config.devices.iter().map(|device| (device.id.as_str(), device.name.as_str())).collect::<Vec<_>>(), [("two", "Phone"), ("one", "Renamed")]);
        assert!(config.apply(Event::Revoked("two".into())));
        assert!(!config.apply(Event::Revoked("two".into())));
        assert_eq!(config.stage(), Stage::Paired);
        assert!(config.apply(Event::Revoked("one".into())));
        assert_eq!(config.stage(), Stage::Deployed);
    }

    #[test]
    fn turning_off_keeps_the_relay_and_removing_forgets_it() {
        let mut config = Config::default();
        config.apply(Event::Deployed(relay("https://a.b.workers.dev")));
        config.apply(Event::Paired(device("one")));
        assert!(config.apply(Event::Enabled(false)));
        assert_eq!((config.stage(), config.active_relay()), (Stage::Paired, None));
        config.apply(Event::Enabled(true));
        assert_eq!(config.active_relay().map(|relay| relay.url.as_str()), Some("https://a.b.workers.dev"));

        // A redeploy to the same address keeps the phones; another address is another relay.
        config.apply(Event::Deployed(relay("https://a.b.workers.dev")));
        assert_eq!(config.devices.len(), 1);
        config.apply(Event::Deployed(relay("https://a.other.workers.dev")));
        assert_eq!((config.devices.len(), config.stage()), (0, Stage::Deployed));

        config.apply(Event::Paired(device("one")));
        config.apply(Event::Removed);
        // Cloudflare's tool is still installed and signed in.
        assert_eq!((config.stage(), config.relay.is_none(), config.enabled, config.devices.len()), (Stage::SignedIn, true, false, 0));
    }

    #[test]
    fn settings_round_trip_and_default_to_quick_connect_with_the_old_relay_hidden() {
        let dir = std::env::temp_dir().join(format!("pandacode-qc-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let fresh = load(&dir);
        assert_eq!((fresh.mode, fresh.legacy_connect_visible, fresh.stage()), (Mode::QuickConnect, false, Stage::NotStarted));
        let mut config = fresh;
        config.apply(Event::Deployed(relay("https://a.b.workers.dev")));
        config.apply(Event::Paired(device("one")));
        config.mode = Mode::Tailscale;
        save(&dir, &config).unwrap();
        assert_eq!(load(&dir), config);
        let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(text.contains("\"mode\":\"tailscale\"") || text.contains("\"mode\": \"tailscale\""), "{text}");
        // An older file without newer fields still loads.
        std::fs::write(dir.join(FILE), r#"{"relay":{"name":"n","url":"https://n.s.workers.dev"},"enabled":true}"#).unwrap();
        assert_eq!(load(&dir).stage(), Stage::Deployed);
        for (text, mode) in [("quickConnect", Mode::QuickConnect), ("sameWifi", Mode::SameWifi), ("tailscale", Mode::Tailscale)] {
            assert_eq!(Mode::parse(text), Ok(mode));
            assert_eq!(serde_json::to_value(mode).unwrap(), text);
        }
        assert!(Mode::parse("relay").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
