//! STUB — to be implemented.

use eframe::egui;

use crate::model::DeviceState;

pub fn show(ui: &mut egui::Ui, dev: &DeviceState) {
    ui.heading(&dev.name);
}
