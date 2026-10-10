//! Color LED effects: the LED control switch and one effect picker per zone.

use eframe::egui::{self, RichText, SliderClamping};

use crate::features::lighting::{self, Action, Effect, Field, State, Zone};
use crate::hidpp::info::Kind;
use crate::model::DeviceState;
use crate::util;

/// Editing state for the zones: the effect shown in each picker, and the state they were built from.
#[derive(Default)]
pub struct Draft {
    base: Option<State>,
    effects: Vec<Effect>,
}

const FIELDS: [Field; 8] = [Field::Color, Field::Speed, Field::Period, Field::Intensity, Field::Saturation, Field::Ramp, Field::Form, Field::Direction];

const RAMP: &[(u8, &str)] = &[(0, "Default"), (1, "Yes"), (2, "No")];
const FORM: &[(u8, &str)] = &[(0, "Default"), (1, "Sine"), (2, "Square"), (3, "Triangle"), (4, "Sawtooth"), (5, "Shark fin"), (6, "Exponential")];
const DIRECTION: &[(u8, &str)] = &[(0, "Cycle"), (1, "Right"), (2, "Down"), (3, "Center out"), (4, "In"), (5, "Out"), (6, "Left"), (7, "Up"), (8, "Center in")];

pub fn show(ui: &mut egui::Ui, dev: &DeviceState, state: &State, draft: &mut Draft, out: &mut Vec<Action>) {
    if draft.base.as_ref() != Some(state) {
        draft.base = Some(state.clone());
        draft.effects = state.zones.iter().map(start_effect).collect();
    }

    control(ui, dev, state, out);
    if !state.readable {
        ui.label(RichText::new("The device does not report its current effects, so this shows the last one logiset set.").weak().small());
    }
    if state.zones.is_empty() {
        ui.label("This device has no LED zones.");
    }
    for (zone, effect) in state.zones.iter().zip(draft.effects.iter_mut()) {
        ui.add_space(8.0);
        ui.group(|ui| zone_ui(ui, zone, effect, out));
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

fn zone_ui(ui: &mut egui::Ui, zone: &Zone, effect: &mut Effect, out: &mut Vec<Action>) {
    ui.label(RichText::new(zone.title()).strong());
    let options = effect_options(zone);
    if options.is_empty() {
        ui.label("This zone has no effects.");
        return;
    }

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

    let differs = zone.current.as_ref().is_none_or(|current| !current.same_setting(effect));
    ui.horizontal(|ui| {
        if let Some(current) = &zone.current {
            ui.label(RichText::new(format!("Current: {}", current.label())).weak().small());
        }
        if ui.add_enabled(differs, egui::Button::new("Apply")).clicked() {
            out.push(Action::SetEffect { zone: zone.index, location: zone.location, effect: effect.clone() });
        }
    });
}

/// Effect ids of a zone with the names shown in its picker. Names shared by two effects get the id.
fn effect_options(zone: &Zone) -> Vec<(u16, String)> {
    zone.effects
        .iter()
        .map(|info| {
            let name = lighting::effect_name(info.id);
            let twins = zone.effects.iter().filter(|other| lighting::effect_name(other.id) == name).count();
            let label = if twins > 1 { format!("{name} (0x{:02X})", info.id) } else { name };
            (info.id, label)
        })
        .collect()
}

/// Draft for a zone: the effect it shows, else a white static color.
fn start_effect(zone: &Zone) -> Effect {
    zone.current.clone().unwrap_or_else(|| {
        let id = if zone.effects.iter().any(|e| e.id == lighting::STATIC) { lighting::STATIC } else { zone.effects.first().map_or(lighting::STATIC, |e| e.id) };
        Effect::new(id)
    })
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
            ui.add(egui::Slider::new(&mut effect.speed, 0..=255).clamping(SliderClamping::Never));
        }
        Field::Period => period(ui, effect),
        Field::Intensity => {
            ui.add(egui::Slider::new(&mut effect.intensity, 0..=100).suffix(" %").clamping(SliderClamping::Never));
        }
        Field::Saturation => {
            ui.add(egui::Slider::new(&mut effect.saturation, 0..=255).clamping(SliderClamping::Never));
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
        ui.add(egui::Slider::new(&mut effect.period, lo..=hi).suffix(" ms").clamping(SliderClamping::Never));
    } else {
        let mut seconds = f64::from(effect.period) / 1000.0;
        let response = ui.add(
            egui::Slider::new(&mut seconds, f64::from(lo) / 1000.0..=f64::from(hi) / 1000.0)
                .step_by(0.1)
                .fixed_decimals(1)
                .suffix(" s")
                .clamping(SliderClamping::Never),
        );
        if response.changed() {
            effect.period = (seconds * 1000.0).round() as u16;
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
