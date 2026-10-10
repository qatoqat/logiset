//! Saved settings in `$XDG_CONFIG_HOME/logiset/config.json`.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::Action;
use crate::util;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Re-apply saved settings whenever a device connects.
    pub auto_apply: bool,
    pub devices: BTreeMap<String, SavedDevice>,
    /// Problem found while loading, for the UI to show.
    #[serde(skip)]
    pub warning: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config { auto_apply: true, devices: BTreeMap::new(), warning: None }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedDevice {
    pub name: String,
    /// persist key -> action. Kept as raw JSON so one entry from an older version
    /// that no longer parses does not invalidate the rest.
    pub settings: BTreeMap<String, serde_json::Value>,
}

impl Config {
    pub fn path() -> PathBuf {
        util::config_dir().join("config.json")
    }

    /// Load the config. Device records that do not parse are skipped; a file that is not
    /// JSON at all is copied to `config.json.bad` so the next save does not lose it.
    pub fn load() -> Config {
        let path = Self::path();
        let Ok(data) = fs::read(&path) else {
            return Config::default();
        };
        match serde_json::from_slice::<serde_json::Value>(&data) {
            Ok(value) => Self::from_value(value),
            Err(e) => {
                let bad = path.with_extension("json.bad");
                let warning = if fs::copy(&path, &bad).is_ok() {
                    format!("{} was not valid JSON ({e}); it was copied to {} and saved settings start empty", path.display(), bad.display())
                } else {
                    format!("{} was not valid JSON ({e}); saved settings start empty", path.display())
                };
                Config { warning: Some(warning), ..Config::default() }
            }
        }
    }

    fn from_value(value: serde_json::Value) -> Config {
        let mut config = Config::default();
        if let Some(auto) = value.get("auto_apply").and_then(|v| v.as_bool()) {
            config.auto_apply = auto;
        }
        let mut skipped = Vec::new();
        if let Some(devices) = value.get("devices").and_then(|v| v.as_object()) {
            for (key, record) in devices {
                match serde_json::from_value::<SavedDevice>(record.clone()) {
                    Ok(dev) => {
                        config.devices.insert(key.clone(), dev);
                    }
                    Err(_) => skipped.push(key.clone()),
                }
            }
        }
        if !skipped.is_empty() {
            config.warning = Some(format!("ignored unreadable saved settings for: {}", skipped.join(", ")));
        }
        config
    }

    pub fn save(&self) -> Result<(), String> {
        let data = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        util::write_atomic(&Self::path(), &data).map_err(|e| format!("saving {}: {e}", Self::path().display()))
    }

    pub fn remember(&mut self, key: &str, name: &str, action: &Action) {
        let Some(slot) = action.persist_key() else {
            return;
        };
        let Ok(value) = serde_json::to_value(action) else {
            return;
        };
        let dev = self.devices.entry(key.to_owned()).or_default();
        dev.name = name.to_owned();
        dev.settings.insert(slot, value);
    }

    pub fn forget(&mut self, key: &str, slot: &str) {
        self.forget_where(key, |s| s == slot);
    }

    /// Drop every saved setting of a device whose key starts with `prefix`.
    pub fn forget_prefix(&mut self, key: &str, prefix: &str) {
        self.forget_where(key, |s| s.starts_with(prefix));
    }

    fn forget_where(&mut self, key: &str, matches: impl Fn(&str) -> bool) {
        if let Some(dev) = self.devices.get_mut(key) {
            dev.settings.retain(|slot, _| !matches(slot));
            if dev.settings.is_empty() {
                self.devices.remove(key);
            }
        }
    }

    /// Saved actions for a device, in replay order.
    pub fn actions(&self, key: &str) -> Vec<(String, Action)> {
        let mut out: Vec<(String, Action)> = self
            .devices
            .get(key)
            .map(|d| {
                d.settings
                    .iter()
                    .filter_map(|(k, v)| serde_json::from_value(v.clone()).ok().map(|a| (k.clone(), a)))
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|(k, a)| (a.replay_rank(), k.clone()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_device_record_is_skipped_not_fatal() {
        let value = serde_json::json!({
            "auto_apply": false,
            "devices": {
                "good": { "name": "G502", "settings": { "pointer.dpi.0": {"Pointer": {"SetDpi": {"sensor": 0, "dpi": 800}}} } },
                "bad": { "name": null, "settings": 5 }
            }
        });
        let config = Config::from_value(value);
        assert!(!config.auto_apply);
        assert_eq!(config.devices.len(), 1);
        assert_eq!(config.actions("good").len(), 1);
        assert!(config.warning.as_deref().unwrap().contains("bad"));
    }

    #[test]
    fn forget_prefix_keeps_other_settings() {
        let mut config = Config::default();
        let dev = config.devices.entry("k".into()).or_default();
        for slot in ["lighting.zone.0", "lighting.zone.1", "pointer.rate"] {
            dev.settings.insert(slot.into(), serde_json::Value::Null);
        }
        config.forget_prefix("k", "lighting.zone.");
        assert_eq!(config.devices["k"].settings.keys().collect::<Vec<_>>(), ["pointer.rate"]);
    }
}
