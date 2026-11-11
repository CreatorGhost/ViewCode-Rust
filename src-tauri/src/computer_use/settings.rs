//! The computer-use settings: one JSON row, like the scheduler's. Off until the user
//! turns it on. A value that cannot be read never loosens anything: the mode reads as
//! off, approvals as follow thread permissions and the screen setting as ask.

use super::policy::{Approvals, Mode, Screen};
use crate::storage::Store;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS computer_use_settings(id INTEGER PRIMARY KEY CHECK(id=1), content TEXT NOT NULL CHECK(json_valid(content)));";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Settings {
    pub(crate) mode: Mode,
    pub(crate) approvals: Approvals,
    pub(crate) screen: Screen,
}

impl Default for Settings {
    fn default() -> Self {
        Self { mode: Mode::Off, approvals: Approvals::Thread, screen: Screen::Allow }
    }
}

impl Settings {
    /// What the gate assumes when the settings cannot be read at all.
    pub(crate) const UNREADABLE: Settings = Settings { mode: Mode::Off, approvals: Approvals::Thread, screen: Screen::UNREADABLE };

    /// Reads stored settings field by field: a missing field is its default, a field
    /// that is present but not a known value is the strictest one.
    fn from_stored(content: &str) -> Self {
        let Ok(Value::Object(stored)) = serde_json::from_str::<Value>(content) else { return Self::UNREADABLE };
        fn field<T: serde::de::DeserializeOwned>(stored: &serde_json::Map<String, Value>, name: &str, missing: T, unreadable: T) -> T {
            match stored.get(name) {
                None => missing,
                Some(value) => serde_json::from_value(value.clone()).unwrap_or(unreadable),
            }
        }
        let default = Self::default();
        Self {
            mode: field(&stored, "mode", default.mode, Self::UNREADABLE.mode),
            approvals: field(&stored, "approvals", default.approvals, Self::UNREADABLE.approvals),
            screen: field(&stored, "screen", default.screen, Self::UNREADABLE.screen),
        }
    }
}

/// A change to the settings; fields left out keep their value.
#[derive(Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettingsPatch {
    #[serde(default)]
    pub(crate) mode: Option<Mode>,
    #[serde(default)]
    pub(crate) approvals: Option<Approvals>,
    #[serde(default)]
    pub(crate) screen: Option<Screen>,
    /// Must be true in the call that turns on [`Approvals::Bypass`].
    #[serde(default, rename = "acknowledgeRisk")]
    pub(crate) acknowledge_risk: bool,
}

/// Why bypass was not turned on.
pub(crate) const BYPASS_NEEDS_ACKNOWLEDGEMENT: &str = "Bypass all permissions removes every computer-use safeguard: nothing asks, and agents can operate password managers, system security dialogs and this app's own window. To turn it on, send acknowledgeRisk: true with the change.";

pub(crate) fn read(store: &Store) -> crate::Result<Settings> {
    let content: Option<String> = store.db.query_row("SELECT content FROM computer_use_settings WHERE id=1", [], |row| row.get(0)).optional().map_err(|error| error.to_string())?;
    Ok(content.map_or_else(Settings::default, |content| Settings::from_stored(&content)))
}

pub(crate) fn update(store: &Store, patch: SettingsPatch) -> crate::Result<Settings> {
    // Turning every safeguard off is never a side effect: each call that asks for it says so.
    if patch.approvals == Some(Approvals::Bypass) && !patch.acknowledge_risk {
        return Err(BYPASS_NEEDS_ACKNOWLEDGEMENT.into());
    }
    let current = read(store)?;
    let next = Settings { mode: patch.mode.unwrap_or(current.mode), approvals: patch.approvals.unwrap_or(current.approvals), screen: patch.screen.unwrap_or(current.screen) };
    let content = serde_json::to_string(&next).map_err(|error| error.to_string())?;
    store.db.execute("INSERT INTO computer_use_settings(id,content) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET content=excluded.content", [content]).map_err(|error| error.to_string())?;
    read(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Store, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("pandacode-computer-use-{}.sqlite", uuid::Uuid::new_v4().simple()));
        (Store::open(&path).unwrap(), path)
    }

    fn remove(path: &std::path::Path) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn computer_use_is_off_until_it_is_turned_on() {
        let (store, path) = store();
        assert_eq!(read(&store).unwrap(), Settings { mode: Mode::Off, approvals: Approvals::Thread, screen: Screen::Allow });
        assert_eq!(update(&store, SettingsPatch { mode: Some(Mode::Observe), ..Default::default() }).unwrap().mode, Mode::Observe);
        let changed = update(&store, SettingsPatch { approvals: Some(Approvals::Risky), screen: Some(Screen::Ask), ..Default::default() }).unwrap();
        assert_eq!(changed, Settings { mode: Mode::Observe, approvals: Approvals::Risky, screen: Screen::Ask });
        assert_eq!(update(&store, SettingsPatch { mode: Some(Mode::Control), ..Default::default() }).unwrap(), Settings { mode: Mode::Control, ..changed });
        let stored: String = store.db.query_row("SELECT content FROM computer_use_settings WHERE id=1", [], |row| row.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), serde_json::json!({"mode":"control","approvals":"risky","screen":"ask"}));
        drop(store);
        remove(&path);
    }

    #[test]
    fn a_value_that_cannot_be_read_never_loosens_anything() {
        assert_eq!(Settings::from_stored("{}"), Settings::default());
        assert_eq!(Settings::from_stored(r#"{"mode":"control"}"#), Settings { mode: Mode::Control, ..Settings::default() });
        assert_eq!(Settings::from_stored(r#"{"mode":"everything","approvals":"sometimes","screen":7}"#), Settings::UNREADABLE);
        assert_eq!(Settings::from_stored(r#"{"mode":"control","approvals":"never","screen":"maybe"}"#), Settings { mode: Mode::Control, approvals: Approvals::Never, screen: Screen::Ask });
        assert_eq!(Settings::from_stored("[]"), Settings::UNREADABLE);
        assert_eq!(Settings::from_stored("not json"), Settings::UNREADABLE);
        assert_eq!(Settings::UNREADABLE, Settings { mode: Mode::Off, approvals: Approvals::Thread, screen: Screen::Ask });
        assert!(serde_json::from_value::<SettingsPatch>(serde_json::json!({"mode":"control","extra":1})).is_err());
        assert!(serde_json::from_value::<SettingsPatch>(serde_json::json!({"mode":"on"})).is_err());
        assert_eq!(serde_json::from_value::<SettingsPatch>(serde_json::json!({"screen":"ask"})).unwrap(), SettingsPatch { screen: Some(Screen::Ask), ..Default::default() });
    }

    #[test]
    fn bypass_is_only_turned_on_by_a_call_that_acknowledges_the_risk() {
        let (store, path) = store();
        let bypass = SettingsPatch { approvals: Some(Approvals::Bypass), ..Default::default() };
        assert_eq!(update(&store, bypass).unwrap_err(), BYPASS_NEEDS_ACKNOWLEDGEMENT);
        assert!(BYPASS_NEEDS_ACKNOWLEDGEMENT.contains("acknowledgeRisk: true") && BYPASS_NEEDS_ACKNOWLEDGEMENT.contains("password managers"));
        // A refused change changes nothing, not even the other fields it carried.
        assert_eq!(update(&store, SettingsPatch { mode: Some(Mode::Control), ..bypass }).unwrap_err(), BYPASS_NEEDS_ACKNOWLEDGEMENT);
        assert_eq!(read(&store).unwrap(), Settings::default());
        // The acknowledgement alone turns nothing on.
        assert_eq!(update(&store, SettingsPatch { acknowledge_risk: true, mode: Some(Mode::Control), ..Default::default() }).unwrap().approvals, Approvals::Thread);
        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({"approvals":"bypass","acknowledgeRisk":true})).unwrap();
        assert_eq!(patch, SettingsPatch { acknowledge_risk: true, ..bypass });
        let on = update(&store, patch).unwrap();
        assert_eq!(on, Settings { mode: Mode::Control, approvals: Approvals::Bypass, screen: Screen::Allow });
        assert_eq!(serde_json::to_value(on).unwrap()["approvals"], "bypass");
        // It stays on across other changes, asking for it again needs the acknowledgement
        // again, and switching away needs nothing.
        assert_eq!(update(&store, SettingsPatch { screen: Some(Screen::Ask), ..Default::default() }).unwrap().approvals, Approvals::Bypass);
        assert_eq!(update(&store, bypass).unwrap_err(), BYPASS_NEEDS_ACKNOWLEDGEMENT);
        assert_eq!(read(&store).unwrap().approvals, Approvals::Bypass);
        assert_eq!(update(&store, SettingsPatch { approvals: Some(Approvals::Risky), ..Default::default() }).unwrap().approvals, Approvals::Risky);
        assert!(serde_json::from_value::<SettingsPatch>(serde_json::json!({"approvals":"bypass","acknowledgeRisk":"yes"})).is_err());
        drop(store);
        remove(&path);
    }

    #[test]
    fn an_unreadable_setting_is_never_bypass() {
        assert_eq!(Settings::from_stored(r#"{"mode":"control","approvals":"bypass"}"#).approvals, Approvals::Bypass, "only the exact stored word");
        for stored in [r#"{"mode":"control","approvals":"Bypass"}"#, r#"{"mode":"control","approvals":"bypass "}"#, r#"{"mode":"control","approvals":true}"#, r#"{"mode":"control","approvals":["bypass"]}"#, r#"{"mode":"control","approvals":null}"#, r#"{"mode":"control"}"#, "[\"bypass\"]", "bypass"] {
            assert_eq!(Settings::from_stored(stored).approvals, Approvals::Thread, "{stored}");
        }
        assert_eq!((Settings::default().approvals, Settings::UNREADABLE.approvals), (Approvals::Thread, Approvals::Thread));
    }

    #[test]
    fn the_settings_table_arrives_with_schema_39() {
        let (store, path) = store();
        store.db.execute_batch("DROP TABLE computer_use_settings; PRAGMA user_version=38;").unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(), crate::storage::SCHEMA_VERSION);
        assert_eq!(read(&store).unwrap(), Settings::default());
        store.db.execute_batch("DROP TABLE computer_use_settings;").unwrap();
        assert!(read(&store).is_err(), "a database without the table is unreadable, which the gate treats as off");
        drop(store);
        remove(&path);
    }
}
