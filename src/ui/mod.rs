//! Per-slice panels. Each `show` renders the slice and pushes requested changes to `out`.

pub mod keyboard;
pub mod lighting;
pub mod onboard;
pub mod overview;
pub mod pointer;
pub mod wheel;

/// UI-only editing state for one device.
#[derive(Default)]
pub struct Drafts {
    pub pointer: pointer::Draft,
    pub lighting: lighting::Draft,
    pub onboard: onboard::Draft,
    pub keyboard: keyboard::Draft,
    pub wheel: wheel::Draft,
}
