//! STUB — to be implemented. Read-only slice: no actions.

use crate::hidpp::{Device, Result};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {}

pub fn probe(_dev: &Device) -> Option<Result<State>> {
    None
}

/// Short text for the device list, e.g. "85%" or "charging".
pub fn summary(_state: &State) -> String {
    String::new()
}
