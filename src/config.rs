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
}

impl Default for Config {
    fn default() -> Self {
        Config { auto_apply: true, devices: BTreeMap::new() }
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

    pub fn load() -> Config {
        match fs::read(Self::path()) {
            Ok(data) => serde_json::from_slice(&data).unwrap_or_else(|e| {
                eprintln!("logiset: ignoring unreadable {}: {e}", Self::path().display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
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
        if let Some(dev) = self.devices.get_mut(key) {
            dev.settings.remove(slot);
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
