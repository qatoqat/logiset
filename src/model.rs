//! Snapshot of a device as the UI sees it, and the actions the UI can request.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::features::{battery, keyboard, lighting, onboard, pointer, wheel};
use crate::hidpp::FeatureEntry;
use crate::hidpp::info::{Firmware, Kind};

/// `None`: the device lacks the feature. `Some(Err)`: reading it failed.
pub type Slot<T> = Option<Result<T, String>>;

#[derive(Clone, Debug)]
pub struct DeviceState {
    /// Routing key for commands: `/dev/hidrawN#index`.
    pub id: String,
    /// Key under which settings are saved; the same over cable and receiver.
    pub config_key: String,
    pub name: String,
    pub kind: Kind,
    pub online: bool,
    pub path: PathBuf,
    /// 0xFF when attached directly, otherwise the receiver pairing slot.
    pub index: u8,
    /// Name of the receiver this device is paired to, when reached through one.
    pub receiver: Option<String>,
    pub wpid: Option<u16>,
    pub protocol: (u8, u8),
    pub unit_id: Option<u32>,
    pub model_ids: Vec<u16>,
    pub firmware: Vec<Firmware>,
    pub serial: Option<String>,
    pub features: Vec<FeatureEntry>,

    pub battery: Slot<battery::State>,
    pub pointer: Slot<pointer::State>,
    pub lighting: Slot<lighting::State>,
    pub onboard: Slot<onboard::State>,
    pub keyboard: Slot<keyboard::State>,
    pub wheel: Slot<wheel::State>,

    /// Settings logiset re-applies when the device connects: (key, description).
    pub saved: Vec<(String, String)>,
}

impl DeviceState {
    /// `Some(true)` when the mouse runs from its onboard profiles, so host-side
    /// DPI / report-rate / lighting changes are temporary.
    pub fn onboard_mode(&self) -> Option<bool> {
        match &self.onboard {
            Some(Ok(state)) => Some(onboard::is_onboard_mode(state)),
            _ => None,
        }
    }

    pub fn connection_label(&self) -> String {
        match &self.receiver {
            Some(r) => format!("{r}, slot {}", self.index),
            None if self.path.as_os_str().is_empty() => "unknown".into(),
            None => "USB / Bluetooth".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    Pointer(pointer::Action),
    Lighting(lighting::Action),
    Onboard(onboard::Action),
    Keyboard(keyboard::Action),
    Wheel(wheel::Action),
}

impl Action {
    /// Key under which the action is remembered and re-applied; `None` for actions
    /// whose effect the device already stores permanently.
    pub fn persist_key(&self) -> Option<String> {
        match self {
            Action::Pointer(a) => a.persist_key(),
            Action::Lighting(a) => a.persist_key(),
            Action::Onboard(a) => a.persist_key(),
            Action::Keyboard(a) => a.persist_key(),
            Action::Wheel(a) => a.persist_key(),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Action::Pointer(a) => a.describe(),
            Action::Lighting(a) => a.describe(),
            Action::Onboard(a) => a.describe(),
            Action::Keyboard(a) => a.describe(),
            Action::Wheel(a) => a.describe(),
        }
    }

    /// Replay order: the onboard/host mode switch must land before the settings it gates.
    pub fn replay_rank(&self) -> u8 {
        match self {
            Action::Onboard(_) => 0,
            _ => 1,
        }
    }
}
