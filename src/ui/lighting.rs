//! Color LED effects: the LED control switch and one effect picker per zone.

use eframe::egui::{self, RichText, SliderClamping};

use crate::features::lighting::{self, Action, Effect, Field, State, Zone};
use crate::hidpp::info::Kind;
use crate::model::DeviceState;
use crate::util;

/// Editing state for the zones: per zone, the zone state its draft was built from and the effect being edited.
#[derive(Default)]
pub struct Draft {
    zones: Vec<Editing>,
}

struct Editing {
    base: Zone,
    effect: Effect,
}

impl Draft {
    /// Keep the draft of every zone whose state is unchanged; start the others from the device state.
    fn sync(&mut self, zones: &[Zone]) {
        let mut old = std::mem::take(&mut self.zones);
        self.zones = zones
            .iter()
            .map(|zone| match old.iter().position(|e| e.base == *zone) {
                Some(at) => old.swap_remove(at),
                None => Editing { base: zone.clone(), effect: start_effect(zone) },
            })
            .collect();
    }
}

const FIELDS: [Field; 8] = [Field::Color, Field::Speed, Field::Period, Field::Intensity, Field::Saturation, Field::Ramp, Field::Form, Field::Direction];

const RAMP: &[(u8, &str)] = &[(0, "Default"), (1, "Yes"), (2, "No")];
const FORM: &[(u8, &str)] = &[(0, "Default"), (1, "Sine"), (2, "Square"), (3, "Triangle"), (4, "Sawtooth"), (5, "Shark fin"), (6, "Exponential")];
const DIRECTION: &[(u8, &str)] = &[(0, "Cycle"), (1, "Right"), (2, "Down"), (3, "Center out"), (4, "In"), (5, "Out"), (6, "Left"), (7, "Up"), (8, "Center in")];

pub fn show(ui: &mut egui::Ui, dev: &DeviceState, state: &State, draft: &mut Draft, out: &mut Vec<Action>) {
    draft.sync(&state.zones);

    control(ui, dev, state, out);
    if !state.readable {
        ui.label(RichText::new("The device does not report its current effects, so this shows the last one logiset set.").weak().small());
    }
    if state.zones.is_empty() {
        ui.label("This device has no LED zones.");
    }
    let keyboard = dev.kind == Kind::Keyboard;
    for (zone, editing) in state.zones.iter().zip(draft.zones.iter_mut()) {
        ui.add_space(8.0);
        let title = zone_title(zone, keyboard);
        ui.group(|ui| zone_ui(ui, title, zone, &mut editing.effect, out));
    }
}

fn control(ui: &mut egui::Ui, dev: &DeviceState, state: &State, out: &mut Vec<Action>) {
    let mut on = state.sw_control.unwrap_or(false);
    let (device, logiset) = ui
        .horizontal(|ui| {
            ui.label("LED control:");
            let device = ui.radio_value(&mut on, false, "Device");
            let logiset = ui.radio_value(&mut on, true, "logiset");
            (device, logiset)
        })
        .inner;
    if (device.clicked() || logiset.clicked()) && state.sw_control != Some(on) {
        out.push(Action::SetControl(on));
    }
    ui.label(
        RichText::new("The device, or its onboard profile, controls the LEDs until logiset takes control. Choosing an effect takes control too.")
            .weak()
            .small(),
    );
    if dev.kind == Kind::Keyboard && util::keyledsd_running() {
        ui.label(
            RichText::new("keyledsd is running and drives this keyboard's lighting; it may override changes made here.").color(ui.visuals().warn_fg_color),
        );
    }
}

fn zone_ui(ui: &mut egui::Ui, title: String, zone: &Zone, effect: &mut Effect, out: &mut Vec<Action>) {
    ui.label(RichText::new(title).strong());
    let options = effect_options(zone);
    if options.is_empty() {
        ui.label("This zone has no effects logiset can set.");
    } else {
        let mut id = effect.id;
        let shown = options.iter().find(|(option, _)| *option == id).map_or_else(|| lighting::effect_name(id), |(_, label)| label.clone());
        ui.horizontal(|ui| {
            ui.label("Effect");
            egui::ComboBox::from_id_salt(("lighting-effect", zone.index)).selected_text(shown).show_ui(ui, |ui| {
                for (option, label) in &options {
                    ui.selectable_value(&mut id, *option, label.as_str());
                }
            });
        });
        if id != effect.id {
            let color = effect.color;
            *effect = Effect::new(id);
            if lighting::has_field(id, Field::Color) {
                effect.color = color;
            }
        }

        let id = effect.id;
        egui::Grid::new(("lighting-params", zone.index)).num_columns(2).show(ui, |ui| {
            for field in FIELDS.into_iter().filter(|f| lighting::has_field(id, *f)) {
                ui.label(field_label(field));
                param(ui, zone.index, field, effect);
                ui.end_row();
            }
        });
    }

    let differs = zone.current.as_ref().is_none_or(|current| !current.same_setting(effect));
    ui.horizontal(|ui| {
        if let Some(current) = &zone.current {
            let text = if lighting::is_known(current.id) {
                format!("Current: {}", current.label())
            } else {
                format!("Current: {} (read-only, its parameters are unknown)", current.label())
            };
            ui.label(RichText::new(text).weak().small());
        }
        if !options.is_empty() && ui.add_enabled(differs, egui::Button::new("Apply")).clicked() {
            out.push(Action::SetEffect { zone: zone.index, location: zone.location, effect: effect.clone().clamped() });
        }
    });
}

/// Heading of a zone. On a keyboard the first zone is the key lighting.
fn zone_title(zone: &Zone, keyboard: bool) -> String {
    if keyboard && zone.location == 1 { "Keys".to_owned() } else { zone.title() }
}

/// Effects of a zone that logiset can set (ids with a known parameter layout), with the names shown in its picker.
/// Names shared by two effects get the id.
fn effect_options(zone: &Zone) -> Vec<(u16, String)> {
    let known: Vec<u16> = zone.effects.iter().map(|info| info.id).filter(|id| lighting::is_known(*id)).collect();
    known
        .iter()
        .map(|&id| {
            let name = lighting::effect_name(id);
            let twins = known.iter().filter(|&&other| lighting::effect_name(other) == name).count();
            let label = if twins > 1 { format!("{name} (0x{id:02X})") } else { name };
            (id, label)
        })
        .collect()
}

/// Draft for a zone: the effect it shows if logiset can set it, else a white static color or the zone's first effect.
fn start_effect(zone: &Zone) -> Effect {
    let options = effect_options(zone);
    let offered = |id: u16| options.iter().any(|(option, _)| *option == id);
    match &zone.current {
        Some(current) if offered(current.id) => current.clone(),
        _ => {
            let id = if offered(lighting::STATIC) { lighting::STATIC } else { options.first().map_or(lighting::STATIC, |(id, _)| *id) };
            Effect::new(id)
        }
    }
}

fn field_label(field: Field) -> &'static str {
    match field {
        Field::Color => "Color",
        Field::Speed => "Speed",
        Field::Period => "Period",
        Field::Intensity => "Intensity",
        Field::Saturation => "Saturation",
        Field::Ramp => "Ramp",
        Field::Form => "Waveform",
        Field::Direction => "Direction",
    }
}

fn param(ui: &mut egui::Ui, zone: u8, field: Field, effect: &mut Effect) {
    match field {
        Field::Color => {
            ui.color_edit_button_srgb(&mut effect.color);
        }
        Field::Speed => {
            ui.add(egui::Slider::new(&mut effect.speed, 0..=255).clamping(SliderClamping::Edits));
        }
        Field::Period => period(ui, effect),
        Field::Intensity => {
            ui.add(egui::Slider::new(&mut effect.intensity, 0..=100).suffix(" %").clamping(SliderClamping::Edits));
        }
        Field::Saturation => {
            ui.add(egui::Slider::new(&mut effect.saturation, 0..=255).clamping(SliderClamping::Edits));
        }
        Field::Ramp => choice(ui, ("lighting-ramp", zone), &mut effect.ramp, RAMP),
        Field::Form => choice(ui, ("lighting-form", zone), &mut effect.form, FORM),
        Field::Direction => choice(ui, ("lighting-direction", zone), &mut effect.direction, DIRECTION),
    }
}

/// Seconds for the usual range; milliseconds for effects with a short range (Ripple).
fn period(ui: &mut egui::Ui, effect: &mut Effect) {
    let (lo, hi) = lighting::period_range(effect.id);
    if lo < 1000 {
        ui.add(egui::Slider::new(&mut effect.period, lo..=hi).suffix(" ms").clamping(SliderClamping::Edits));
    } else {
        let mut seconds = f64::from(effect.period) / 1000.0;
        let response = ui.add(
            egui::Slider::new(&mut seconds, f64::from(lo) / 1000.0..=f64::from(hi) / 1000.0)
                .step_by(0.1)
                .fixed_decimals(1)
                .suffix(" s")
                .clamping(SliderClamping::Edits),
        );
        if response.changed() {
            effect.period = (seconds * 1000.0).round().clamp(f64::from(lo), f64::from(hi)) as u16;
        }
    }
}

fn choice(ui: &mut egui::Ui, salt: (&str, u8), value: &mut u8, choices: &[(u8, &str)]) {
    let shown = choices.iter().find(|(v, _)| *v == *value).map_or_else(|| value.to_string(), |(_, name)| (*name).to_owned());
    egui::ComboBox::from_id_salt(salt).selected_text(shown).show_ui(ui, |ui| {
        for (v, name) in choices {
            ui.selectable_value(value, *v, *name);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::lighting::EffectInfo;

    fn info(index: u8, id: u16) -> EffectInfo {
        EffectInfo { index, id, capabilities: 0, default_period: 0 }
    }

    fn zone(index: u8, location: u16, ids: &[u16], current: Option<Effect>) -> Zone {
        let effects = ids.iter().enumerate().map(|(i, &id)| info(i as u8, id)).collect();
        Zone { index, location, effects, current }
    }

    #[test]
    fn options_skip_unknown_layouts() {
        // G610 zone 0 lists 0x05, which logiset has no layout for.
        let z = zone(0, 1, &[0x00, 0x05, 0x01, 0x0a], None);
        let expected = [(0x00u16, "Disabled"), (0x01, "Static"), (0x0a, "Breathe")].map(|(id, name)| (id, name.to_owned()));
        assert_eq!(effect_options(&z), expected);
    }

    #[test]
    fn options_name_twins_by_id() {
        let z = zone(0, 1, &[0x03, 0x15], None);
        let labels: Vec<_> = effect_options(&z).into_iter().map(|(_, label)| label).collect();
        assert_eq!(labels, vec!["Cycle (0x03)", "Cycle (0x15)"]);
    }

    #[test]
    fn draft_starts_on_offered_current_effect() {
        let breathe = Effect { color: [255, 0, 0], period: 4000, ..Effect::new(0x0a) };
        let z = zone(0, 1, &[0x01, 0x0a], Some(breathe.clone()));
        assert_eq!(start_effect(&z), breathe);
    }

    #[test]
    fn draft_does_not_start_on_unknown_current_effect() {
        let z = zone(0, 1, &[0x05, 0x01], Some(Effect::new(0x05)));
        assert_eq!(start_effect(&z).id, lighting::STATIC);
    }

    #[test]
    fn keyboard_first_zone_is_keys() {
        let z = zone(0, 1, &[0x01], None);
        assert_eq!(zone_title(&z, true), "Keys");
        assert_eq!(zone_title(&z, false), "Primary");
        assert_eq!(zone_title(&zone(1, 2, &[0x01], None), true), "Logo");
    }

    #[test]
    fn draft_keeps_zones_that_did_not_change() {
        let logo = zone(1, 2, &[0x01, 0x0a], None);
        let keys = zone(0, 1, &[0x01, 0x0a], None);
        let mut draft = Draft::default();
        draft.sync(&[keys.clone(), logo.clone()]);
        draft.zones[0].effect.intensity = 40;

        // Only the Logo zone's current effect changed.
        let logo_now = zone(1, 2, &[0x01, 0x0a], Some(Effect::new(0x01)));
        draft.sync(&[keys.clone(), logo_now.clone()]);
        assert_eq!(draft.zones[0].effect.intensity, 40);
        assert_eq!(draft.zones[1].effect, start_effect(&logo_now));
    }

    #[test]
    fn draft_follows_zone_list_changes() {
        let keys = zone(0, 1, &[0x01], None);
        let mut draft = Draft::default();
        draft.sync(std::slice::from_ref(&keys));
        draft.zones[0].effect.color = [1, 2, 3];

        draft.sync(&[keys, zone(1, 2, &[0x01], None)]);
        assert_eq!(draft.zones.len(), 2);
        assert_eq!(draft.zones[0].effect.color, [1, 2, 3]);
        draft.sync(&[zone(1, 2, &[0x01], None)]);
        assert_eq!(draft.zones.len(), 1);
        assert_eq!(draft.zones[0].base.index, 1);
    }
}
