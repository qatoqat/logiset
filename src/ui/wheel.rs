//! Scroll wheel panel: SmartShift ratchet and hi-res direction.

use eframe::egui::{self, RichText};

use crate::features::wheel::{AUTO_SHIFT_MAX, Action, HiRes, NEVER_SHIFT, SmartShift, State, TORQUE_MAX};
use crate::model::DeviceState;

/// Auto-shift speed shown when the device has no factory value to start from.
const DEFAULT_SPEED: u8 = 30;

/// Editable copies of the SmartShift settings. Rebuilt whenever the device's SmartShift changes.
#[derive(Default)]
pub struct Draft {
    /// The SmartShift last copied from the device. `None` until the first copy.
    synced: Option<Option<SmartShift>>,
    ratchet: bool,
    auto_shift: bool,
    speed: u8,
    torque: u8,
}

impl Draft {
    /// Copy the device's SmartShift into the editable fields when it differs from the last copy.
    /// Other wheel state, such as the hi-res invert flag, keeps unapplied edits.
    fn follow(&mut self, smartshift: Option<&SmartShift>) {
        if self.synced.as_ref().map(Option::as_ref) == Some(smartshift) {
            return;
        }
        self.synced = Some(smartshift.cloned());
        if let Some(ss) = smartshift {
            self.ratchet = ss.ratchet;
            self.auto_shift = ss.threshold != NEVER_SHIFT;
            self.speed = if self.auto_shift {
                ss.threshold.clamp(1, AUTO_SHIFT_MAX)
            } else {
                ss.default_threshold.unwrap_or(DEFAULT_SPEED).clamp(1, AUTO_SHIFT_MAX)
            };
            self.torque = ss.torque.unwrap_or(1).clamp(1, TORQUE_MAX);
        }
    }
}

pub fn show(ui: &mut egui::Ui, _dev: &DeviceState, state: &State, draft: &mut Draft, out: &mut Vec<Action>) {
    draft.follow(state.smartshift.as_ref());
    if let Some(ss) = &state.smartshift {
        smartshift(ui, ss, draft, out);
    }
    if let Some(hires) = &state.hires {
        if state.smartshift.is_some() {
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(4.0);
        }
        hires_section(ui, hires, out);
    }
}

fn smartshift(ui: &mut egui::Ui, ss: &SmartShift, draft: &mut Draft, out: &mut Vec<Action>) {
    ui.label(RichText::new("SmartShift").strong());
    ui.horizontal(|ui| {
        ui.radio_value(&mut draft.ratchet, false, "Freespin");
        ui.radio_value(&mut draft.ratchet, true, "Ratchet");
    });
    ui.add_enabled_ui(draft.ratchet, |ui| {
        ui.checkbox(&mut draft.auto_shift, "Shift to free-spin when scrolling fast");
        let speed = egui::Slider::new(&mut draft.speed, 1..=AUTO_SHIFT_MAX).text("Speed");
        ui.add_enabled(draft.auto_shift, speed).on_hover_text("Lower values switch to free-spin at lower wheel speeds.");
    });
    if ss.torque.is_some() {
        let torque = egui::Slider::new(&mut draft.torque, 1..=TORQUE_MAX).text("Ratchet torque");
        ui.add(torque).on_hover_text("How much force it takes to turn the wheel in ratchet mode.");
    }

    let want = Action::SetSmartShift {
        ratchet: draft.ratchet,
        threshold: if draft.auto_shift { draft.speed } else { NEVER_SHIFT },
        torque: ss.torque.map(|_| draft.torque),
    };
    let now = Action::SetSmartShift { ratchet: ss.ratchet, threshold: ss.threshold, torque: ss.torque };
    ui.add_space(4.0);
    if ui.add_enabled(want != now, egui::Button::new("Apply")).clicked() {
        out.push(want);
    }
}

fn hires_section(ui: &mut egui::Ui, hires: &HiRes, out: &mut Vec<Action>) {
    ui.label(RichText::new("Hi-res wheel").strong());
    egui::Grid::new("wheel_hires").num_columns(2).show(ui, |ui| {
        ui.label("Multiplier");
        ui.label(format!("x{}", hires.multiplier));
        ui.end_row();
        ui.label("Resolution");
        ui.label(if hires.resolution { "High" } else { "Standard" });
        ui.end_row();
        if let Some(on) = hires.ratchet_switch {
            ui.label("Ratchet switch");
            ui.label(if on { "On" } else { "Off" });
            ui.end_row();
        }
    });
    ui.label(RichText::new("The system driver sets the resolution.").small().weak());
    ui.add_space(4.0);
    if hires.can_invert {
        let mut invert = hires.invert;
        if ui.checkbox(&mut invert, "Invert scroll direction").changed() {
            out.push(Action::SetInvert(invert));
        }
    } else {
        ui.label(RichText::new("This device cannot invert the scroll direction.").weak());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shift(ratchet: bool, threshold: u8) -> SmartShift {
        SmartShift { feature: 0x2111, ratchet, threshold, default_threshold: None, torque: Some(40) }
    }

    #[test]
    fn unrelated_state_keeps_unapplied_edits() {
        let mut draft = Draft::default();
        let device = shift(true, 30);
        draft.follow(Some(&device));
        assert!(draft.ratchet);
        assert_eq!(draft.speed, 30);

        draft.ratchet = false;
        draft.speed = 80;
        // Only the hi-res invert flag changed; the SmartShift part is the same.
        draft.follow(Some(&device));
        assert_eq!((draft.ratchet, draft.speed), (false, 80));
    }

    #[test]
    fn smartshift_change_resyncs_draft() {
        let mut draft = Draft::default();
        draft.follow(Some(&shift(true, 30)));
        draft.speed = 80;

        draft.follow(Some(&shift(false, NEVER_SHIFT)));
        assert!(!draft.ratchet);
        assert!(!draft.auto_shift);
        assert_eq!(draft.speed, DEFAULT_SPEED);
        assert_eq!(draft.torque, 40);
    }

    #[test]
    fn appearing_smartshift_is_copied() {
        let mut draft = Draft::default();
        draft.follow(None);
        draft.follow(Some(&shift(true, 50)));
        assert!(draft.ratchet);
        assert_eq!(draft.speed, 50);
    }
}
