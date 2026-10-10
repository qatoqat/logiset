//! Pointer panel: DPI per sensor and the report rate.

use eframe::egui::{self, RichText};

use crate::features::pointer::{Action, ReportRate, Sensor, State};
use crate::model::DeviceState;

const ONBOARD_NOTE: &str = "The mouse is using an onboard profile, so these changes last until the profile changes or the mouse restarts. Switch to host mode in Onboard profiles to keep them, or edit the profile there.";

/// DPI values offered as one-click presets, when the sensor supports them.
const PRESETS: [u16; 4] = [400, 800, 1600, 3200];

/// Editing state for one device. Rebuilt from the state whenever the state changes.
#[derive(Default)]
pub struct Draft {
    /// The state the drafts were last built from.
    synced: Option<State>,
    /// Editable DPI per sensor, in `State::sensors` order.
    dpi: Vec<u16>,
}

impl Draft {
    fn sync(&mut self, state: &State) {
        if self.synced.as_ref() == Some(state) {
            return;
        }
        self.dpi = state.sensors.iter().map(|s| s.current).collect();
        self.synced = Some(state.clone());
    }
}

pub fn show(ui: &mut egui::Ui, dev: &DeviceState, state: &State, draft: &mut Draft, out: &mut Vec<Action>) {
    draft.sync(state);
    if dev.onboard_mode() == Some(true) {
        ui.label(RichText::new(ONBOARD_NOTE).color(ui.visuals().warn_fg_color));
        ui.add_space(8.0);
    }
    let labelled = state.sensors.len() > 1;
    for (sensor, value) in state.sensors.iter().zip(draft.dpi.iter_mut()) {
        ui.push_id(sensor.index, |ui| dpi_editor(ui, sensor, value, labelled, out));
        ui.add_space(8.0);
    }
    if let Some(rate) = &state.report_rate {
        rate_editor(ui, rate, out);
    }
}

fn dpi_editor(ui: &mut egui::Ui, sensor: &Sensor, value: &mut u16, labelled: bool, out: &mut Vec<Action>) {
    let choices = &sensor.choices;
    let (Some(lo), Some(hi)) = (choices.min(), choices.max()) else {
        ui.label("The device reports no DPI values for this sensor.");
        return;
    };
    if labelled {
        ui.strong(format!("Sensor {}", sensor.index));
    }
    ui.horizontal(|ui| {
        let slider = ui.add(egui::Slider::new(&mut *value, lo..=hi).logarithmic(true).show_value(false));
        let drag = ui.add(
            egui::DragValue::new(&mut *value)
                .range(lo..=hi)
                .speed((f64::from(hi - lo) / 400.0).max(1.0))
                .suffix(" DPI"),
        );
        if commit(&slider) || commit(&drag) {
            *value = choices.nearest(*value).unwrap_or(*value);
        }
        let target = choices.nearest(*value).unwrap_or(*value);
        if ui.add_enabled(target != sensor.current, egui::Button::new("Apply")).clicked() {
            out.push(Action::SetDpi { sensor: sensor.index, dpi: target });
        }
    });
    ui.horizontal_wrapped(|ui| {
        for preset in PRESETS.into_iter().filter(|&p| choices.contains(p)) {
            if ui.button(preset.to_string()).clicked() {
                *value = preset;
            }
        }
        if let Some(default) = sensor.default
            && ui.button(format!("Default ({default})")).clicked()
        {
            *value = choices.nearest(default).unwrap_or(default);
        }
    });
}

fn rate_editor(ui: &mut egui::Ui, rate: &ReportRate, out: &mut Vec<Action>) {
    ui.add_space(8.0);
    ui.strong("Report rate");
    ui.horizontal_wrapped(|ui| {
        for &raw in &rate.supported {
            let selected = raw == rate.current;
            if ui.add(egui::Button::selectable(selected, rate.feature.label(raw))).clicked() && !selected {
                out.push(rate.set_action(raw));
            }
        }
    });
}

/// The user finished changing a value: a click, a typed value, or the end of a drag.
/// Values mid-drag stay raw, so a relative drag is not pulled back onto the grid.
fn commit(response: &egui::Response) -> bool {
    response.drag_stopped() || (response.changed() && !response.dragged())
}
