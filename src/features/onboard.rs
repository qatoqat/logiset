//! STUB — to be implemented.

use serde::{Deserialize, Serialize};

use crate::hidpp::{Device, Result};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {}

impl Action {
    pub fn persist_key(&self) -> Option<String> {
        match *self {}
    }

    pub fn describe(&self) -> String {
        match *self {}
    }
}

pub fn probe(_dev: &Device) -> Option<Result<State>> {
    None
}

pub fn apply(_dev: &Device, _state: &mut State, action: &Action) -> Result<()> {
    match *action {}
}

/// True when the device runs from onboard profiles (host-side changes are temporary).
pub fn is_onboard_mode(_state: &State) -> bool {
    false
}

/// One-line summary for `logiset list` (the full state includes raw memory).
pub fn summary(_state: &State) -> String {
    String::new()
}
