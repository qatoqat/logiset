//! STUB — to be implemented.

use eframe::egui;

use crate::features::onboard::{Action, State};
use crate::model::DeviceState;

#[derive(Default)]
pub struct Draft {}

pub fn show(ui: &mut egui::Ui, dev: &DeviceState, state: &State, draft: &mut Draft, out: &mut Vec<Action>) {
    let _ = (dev, state, draft, out);
    ui.label("Not implemented yet.");
}
