//! Pointer panel: DPI per sensor and the report rate.

use eframe::egui::{self, RichText};

use crate::features::pointer::{Action, ReportRate, Sensor, State};
use crate::hidpp::info::Kind;
use crate::model::DeviceState;

const ONBOARD_NOTE: &str = "The mouse is using an onboard profile, so these changes last until the profile changes or the mouse restarts. Switch to host mode in Onboard profiles to keep them, or edit the profile there.";

/// DPI values offered as one-click presets, when the sensor supports them.
const PRESETS: [u16; 4] = [400, 800, 1600, 3200];

/// Editing state for one device. Rebuilt whenever the sensors' values or choices change.
#[derive(Default)]
pub struct Draft {
    /// The sensors the drafts were last built from.
    synced: Option<Vec<Sensor>>,
    /// Editable DPI per sensor, in `State::sensors` order.
    dpi: Vec<u16>,
}

impl Draft {
    /// Keeps unapplied edits while the sensors stay the same, so a report-rate change does not discard them.
    fn sync(&mut self, sensors: &[Sensor]) {
        if self.synced.as_deref() == Some(sensors) {
            return;
        }
        self.dpi = sensors.iter().map(|s| s.current).collect();
        self.synced = Some(sensors.to_vec());
    }
}

pub fn show(ui: &mut egui::Ui, dev: &DeviceState, state: &State, draft: &mut Draft, out: &mut Vec<Action>) {
    draft.sync(&state.sensors);
    // Keyboards have no onboard DPI profile to warn about.
    if dev.kind != Kind::Keyboard && dev.onboard_mode() == Some(true) {
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
        // Set when a slider drag ends or a slider key is pressed, so the value snaps onto the grid.
        let mut snap = false;
        if choices.ranges.is_empty() {
            value_list(ui, &choices.discrete, value);
        } else {
            let mut slider = egui::Slider::new(&mut *value, lo..=hi).logarithmic(true).show_value(false);
            if let Some(range) = choices.single_range() {
                // Dragging and arrow keys then move by whole steps.
                slider = slider.step_by(f64::from(range.step));
            }
            ui.scope(|ui| {
                ui.spacing_mut().slider_width = 320.0;
                snap |= commit(&ui.add(slider));
            });
        }
        let drag = ui.add(
            egui::DragValue::new(&mut *value)
                .range(lo..=hi)
                .speed((f64::from(hi - lo) / 400.0).max(1.0))
                .suffix(" DPI")
                // A typed value is applied on Enter or focus loss, not on every keystroke.
                .update_while_editing(false),
        );
        if snap || commit(&drag) {
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

/// Drop-down of the discrete values, for a sensor that offers no range.
fn value_list(ui: &mut egui::Ui, values: &[u16], value: &mut u16) {
    egui::ComboBox::from_id_salt("dpi").selected_text(format!("{value} DPI")).show_ui(ui, |ui| {
        for &dpi in values {
            ui.selectable_value(value, dpi, dpi.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::pointer::{DpiChoices, RateFeature};

    fn sensor(current: u16) -> Sensor {
        Sensor { index: 0, current, default: None, choices: DpiChoices::default() }
    }

    #[test]
    fn report_rate_change_keeps_dpi_drafts() {
        let mut draft = Draft::default();
        let mut state = State { sensors: vec![sensor(800)], report_rate: None };
        draft.sync(&state.sensors);
        draft.dpi[0] = 1600;

        state.report_rate = Some(ReportRate { feature: RateFeature::Basic, current: 1, supported: vec![1] });
        draft.sync(&state.sensors);
        assert_eq!(draft.dpi, vec![1600]);
    }

    #[test]
    fn sensor_change_resets_dpi_drafts() {
        let mut draft = Draft::default();
        let mut state = State { sensors: vec![sensor(800)], report_rate: None };
        draft.sync(&state.sensors);
        draft.dpi[0] = 1600;

        state.sensors[0].current = 1000;
        draft.sync(&state.sensors);
        assert_eq!(draft.dpi, vec![1000]);
    }
}
