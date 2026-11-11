//! The relay Worker's sources, compiled into the app and staged into a temporary folder for
//! each wrangler run. It is the same Worker as the shared relay (`relay/`), deployed with
//! `relay/wrangler.self.json`, which turns on its self-hosted mode. wrangler bundles the
//! TypeScript at deploy time, so there is no second bundler and no `node_modules`.
use std::path::{Path, PathBuf};

const CONFIG: &str = include_str!("../../../relay/wrangler.self.json");

macro_rules! sources {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../../../relay/src/", $name)))),*]
    };
}

const SOURCES: &[(&str, &str)] = sources![
    "account-delete.ts", "account.ts", "body.ts", "bucket.ts", "config.ts", "env.ts", "frames.ts", "github.ts", "index.ts", "limits.ts", "policy.ts", "push.ts", "routes.ts", "self.ts", "token.ts", "validate.ts",
];

/// A staged copy of the Worker; the folder goes away when this is dropped.
pub(crate) struct Staged {
    pub dir: PathBuf,
    pub config: PathBuf,
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Writes the Worker under a fresh folder in `parent`.
pub(crate) fn stage(parent: &Path) -> crate::Result<Staged> {
    let failed = |error: std::io::Error| format!("Could not prepare the relay on this computer: {error}");
    let dir = parent.join(format!("pandacode-quick-connect-{}", uuid::Uuid::new_v4()));
    let staged = Staged { config: dir.join("wrangler.json"), dir };
    std::fs::create_dir_all(staged.dir.join("src")).map_err(failed)?;
    std::fs::write(&staged.config, CONFIG).map_err(failed)?;
    for (name, text) in SOURCES {
        std::fs::write(staged.dir.join("src").join(name), text).map_err(failed)?;
    }
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_relay_source_is_staged_with_the_self_hosted_config() {
        let relay = Path::new(env!("CARGO_MANIFEST_DIR")).join("../relay/src");
        let mut on_disk: Vec<String> = std::fs::read_dir(relay).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        on_disk.sort();
        assert_eq!(on_disk, SOURCES.iter().map(|(name, _)| name.to_string()).collect::<Vec<_>>(), "add new relay sources to SOURCES");

        let staged = stage(&std::env::temp_dir()).unwrap();
        let dir = staged.dir.clone();
        let config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&staged.config).unwrap()).unwrap();
        assert_eq!(config["vars"]["RELAY_MODE"], "self");
        assert_eq!(config["name"], super::super::wrangler::DEFAULT_WORKER_NAME);
        assert_eq!(config["main"], "src/index.ts");
        assert_eq!(config["durable_objects"]["bindings"][0]["class_name"], "Account");
        // No custom domain, no secrets and no account id: the deploy goes to whoever signed in.
        for absent in ["routes", "account_id", "ratelimits"] {
            assert!(config.get(absent).is_none(), "{absent}");
        }
        assert!(!CONFIG.contains("SECRET"));
        assert!(dir.join("src/index.ts").is_file() && dir.join("src/self.ts").is_file());
        drop(staged);
        assert!(!dir.exists());
    }
}
