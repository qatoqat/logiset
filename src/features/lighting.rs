//! Color LED effects (0x8070): the effects each LED zone supports, the effect it shows,
//! and whether logiset or the device controls the LEDs.
//!
//! Wire layouts follow Solaar (`hidpp20.py` LEDEffects, LEDZoneInfo, LEDEffectSetting;
//! `settings_templates.py` LEDControl, LEDZoneSetting) and libratbag (`hidpp20.c`,
//! color LED effects). Function numbers are the high nibble of Solaar's function bytes.

use serde::{Deserialize, Serialize};

use crate::hidpp::{Device, Error, Result};

pub const FEATURE: u16 = 0x8070;
/// Effect id of a fixed color.
pub const STATIC: u16 = 0x01;

/// Parameter fields an effect can carry. Each has a fixed width in the 10 parameter bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// 3 bytes, RGB.
    Color,
    /// 1 byte.
    Speed,
    /// 2 bytes, big-endian, milliseconds.
    Period,
    /// 1 byte, percent.
    Intensity,
    /// 1 byte: 0 default, 1 yes, 2 no.
    Ramp,
    /// 1 byte, waveform.
    Form,
    /// 1 byte.
    Saturation,
    /// 1 byte, direction.
    Direction,
}

/// What the device reports about its LED zones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The device can report the effect each zone shows (fn 0, capability bit 0).
    pub readable: bool,
    /// Whether logiset controls the LEDs (fn 7). `None` when the device did not answer.
    pub sw_control: Option<bool>,
    pub zones: Vec<Zone>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub index: u8,
    /// Location code (see [`location_name`]).
    pub location: u16,
    pub effects: Vec<EffectInfo>,
    /// The effect read back from the device, or the last one logiset set when the device cannot report it.
    pub current: Option<Effect>,
}

impl Zone {
    /// Heading for the zone, e.g. "Logo"; "Zone 2" when the location is unknown.
    pub fn title(&self) -> String {
        location_name(self.location).map_or_else(|| format!("Zone {}", u16::from(self.index) + 1), str::to_owned)
    }
}

/// One effect a zone supports (fn 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectInfo {
    /// Position within the zone; sent back to the device in fn 3 and fn 14.
    pub index: u8,
    pub id: u16,
    pub capabilities: u16,
    pub default_period: u16,
}

/// An effect and its parameters. Parameters the effect does not use are ignored on the wire.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Effect {
    pub id: u16,
    pub color: [u8; 3],
    pub speed: u8,
    /// Milliseconds.
    pub period: u16,
    /// Percent, 0..=100.
    pub intensity: u8,
    pub ramp: u8,
    pub form: u8,
    pub saturation: u8,
    pub direction: u8,
}

impl Effect {
    /// Effect `id` with the starting values Solaar uses, in white.
    pub fn new(id: u16) -> Effect {
        let mut p = [0u8; 10];
        for &(field, value) in spec(id).map_or(&[][..], |s| s.defaults) {
            if let Some(&(_, at)) = fields_of(id).iter().find(|(f, _)| *f == field) {
                match field {
                    Field::Period => p[at..at + 2].copy_from_slice(&value.to_be_bytes()),
                    Field::Color => {}
                    _ => p[at] = value as u8,
                }
            }
        }
        let mut effect = decode(id, &p);
        effect.color = [255; 3];
        effect
    }

    /// Same effect with the same parameters on the wire.
    pub fn same_setting(&self, other: &Effect) -> bool {
        self.id == other.id && self.encode() == other.encode()
    }

    /// Short text such as "Breathe #ff0000 4.0 s".
    pub fn label(&self) -> String {
        let mut parts = vec![effect_name(self.id)];
        if has_field(self.id, Field::Color) {
            let [r, g, b] = self.color;
            parts.push(format!("#{r:02x}{g:02x}{b:02x}"));
        }
        if has_field(self.id, Field::Period) {
            parts.push(if self.period < 1000 { format!("{} ms", self.period) } else { format!("{:.1} s", f64::from(self.period) / 1000.0) });
        }
        parts.join(" ")
    }

    /// The 10 parameter bytes (Solaar `LEDEffectSetting.to_bytes`, without the id).
    fn encode(&self) -> [u8; 10] {
        let mut p = [0u8; 10];
        for &(field, at) in fields_of(self.id) {
            match field {
                Field::Color => p[at..at + 3].copy_from_slice(&self.color),
                Field::Period => p[at..at + 2].copy_from_slice(&self.period.to_be_bytes()),
                Field::Speed => p[at] = self.speed,
                Field::Intensity => p[at] = self.intensity,
                Field::Ramp => p[at] = self.ramp,
                Field::Form => p[at] = self.form,
                Field::Saturation => p[at] = self.saturation,
                Field::Direction => p[at] = self.direction,
            }
        }
        p
    }
}

fn decode(id: u16, p: &[u8; 10]) -> Effect {
    let mut effect = Effect { id, ..Effect::default() };
    for &(field, at) in fields_of(id) {
        match field {
            Field::Color => effect.color.copy_from_slice(&p[at..at + 3]),
            Field::Period => effect.period = u16::from_be_bytes([p[at], p[at + 1]]),
            Field::Speed => effect.speed = p[at],
            Field::Intensity => effect.intensity = p[at],
            Field::Ramp => effect.ramp = p[at],
            Field::Form => effect.form = p[at],
            Field::Saturation => effect.saturation = p[at],
            Field::Direction => effect.direction = p[at],
        }
    }
    effect
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// `true`: logiset controls the LEDs. `false`: the device does.
    SetControl(bool),
    /// Show `effect` on zone `zone`. `location` only names the zone in [`Action::describe`].
    SetEffect { zone: u8, location: u16, effect: Effect },
}

impl Action {
    pub fn persist_key(&self) -> Option<String> {
        Some(match self {
            Action::SetControl(_) => "lighting.control".into(),
            Action::SetEffect { zone, .. } => format!("lighting.zone.{zone}"),
        })
    }

    pub fn describe(&self) -> String {
        match self {
            Action::SetControl(on) => format!("LED control: {}", if *on { "logiset" } else { "device" }),
            Action::SetEffect { zone, location, effect } => {
                let place = location_name(*location).map_or_else(|| format!("Zone {}", u16::from(*zone) + 1), str::to_owned);
                format!("{place}: {}", effect.label())
            }
        }
    }
}

/// Name of a location code (Solaar `LEDZoneLocations`). `None` for unknown codes.
pub fn location_name(location: u16) -> Option<&'static str> {
    Some(match location {
        1 => "Primary",
        2 => "Logo",
        3 => "Left side",
        4 => "Right side",
        5 => "Combined",
        6 => "Primary 1",
        7 => "Primary 2",
        8 => "Primary 3",
        9 => "Primary 4",
        10 => "Primary 5",
        11 => "Primary 6",
        _ => return None,
    })
}

/// Name of an effect id; ids not in the table read "Effect 0x05".
pub fn effect_name(id: u16) -> String {
    spec(id).map_or_else(|| format!("Effect 0x{id:02X}"), |s| s.name.to_owned())
}

/// Whether effect `id` takes parameter `field`.
pub fn has_field(id: u16, field: Field) -> bool {
    fields_of(id).iter().any(|(f, _)| *f == field)
}

/// Allowed range of the period of effect `id`, in milliseconds.
pub fn period_range(id: u16) -> (u16, u16) {
    spec(id).map_or(SECONDS, |s| s.period)
}

/// Read-only. `None` when the device has no color LED effects feature.
pub fn probe(dev: &Device) -> Option<Result<State>> {
    dev.has(FEATURE).then(|| read_state(|function: u8, params: &[u8]| dev.call(FEATURE, function, params)))
}

pub fn apply(dev: &Device, state: &mut State, action: &Action) -> Result<()> {
    apply_with(|function: u8, params: &[u8]| dev.call(FEATURE, function, params), state, action)
}

/// Period ranges in milliseconds: one to twenty seconds, and the narrower range of Ripple.
const SECONDS: (u16, u16) = (1000, 20000);
const MILLISECONDS: (u16, u16) = (2, 200);

struct Spec {
    id: u16,
    name: &'static str,
    /// Parameter fields and their byte offsets within the 10 parameter bytes.
    fields: &'static [(Field, usize)],
    /// Starting values, as Solaar uses them.
    defaults: &'static [(Field, u16)],
    period: (u16, u16),
}

const fn row(id: u16, name: &'static str, fields: &'static [(Field, usize)], defaults: &'static [(Field, u16)]) -> Spec {
    Spec { id, name, fields, defaults, period: SECONDS }
}

const EFFECTS: &[Spec] = &[
    row(0x00, "Disabled", &[], &[]),
    row(0x01, "Static", &[(Field::Color, 0), (Field::Ramp, 3)], &[]),
    row(0x02, "Pulse", &[(Field::Color, 0), (Field::Speed, 3)], &[]),
    row(0x03, "Cycle", &[(Field::Period, 5), (Field::Intensity, 7)], &[(Field::Period, 5000), (Field::Intensity, 100)]),
    row(0x04, "Wave", &[(Field::Period, 6), (Field::Direction, 9)], &[(Field::Period, 5000)]),
    row(0x08, "Boot", &[], &[]),
    row(0x09, "Demo", &[], &[]),
    row(0x0A, "Breathe", &[(Field::Color, 0), (Field::Period, 3), (Field::Form, 5), (Field::Intensity, 6)], &[(Field::Period, 5000), (Field::Intensity, 100)]),
    Spec { period: MILLISECONDS, ..row(0x0B, "Ripple", &[(Field::Color, 0), (Field::Period, 4)], &[(Field::Period, 20)]) },
    row(0x0E, "Decomposition", &[(Field::Period, 6), (Field::Intensity, 8)], &[]),
    row(0x0F, "Signature1", &[(Field::Period, 5), (Field::Intensity, 7)], &[]),
    row(0x10, "Signature2", &[(Field::Period, 5), (Field::Intensity, 7)], &[]),
    row(
        0x15,
        "Cycle",
        &[(Field::Saturation, 1), (Field::Period, 6), (Field::Intensity, 8)],
        &[(Field::Saturation, 255), (Field::Period, 5000), (Field::Intensity, 100)],
    ),
    row(
        0x16,
        "Wave",
        &[(Field::Saturation, 1), (Field::Period, 6), (Field::Intensity, 8), (Field::Direction, 9)],
        &[(Field::Saturation, 255), (Field::Period, 5000), (Field::Intensity, 100)],
    ),
    Spec {
        period: MILLISECONDS,
        ..row(0x17, "Ripple", &[(Field::Color, 0), (Field::Saturation, 3), (Field::Period, 6)], &[(Field::Saturation, 255), (Field::Period, 20)])
    },
    // Solaar's host-side dim ramp; kept so the name resolves if a device lists it.
    row(0x80, "Dim", &[(Field::Intensity, 0)], &[]),
];

fn spec(id: u16) -> Option<&'static Spec> {
    EFFECTS.iter().find(|s| s.id == id)
}

fn fields_of(id: u16) -> &'static [(Field, usize)] {
    spec(id).map_or(&[][..], |s| s.fields)
}

/// Fn 0 reply: zone count and whether the current effects can be read.
fn parse_info(r: &[u8; 16]) -> (u8, bool) {
    (r[0], u16::from_be_bytes([r[3], r[4]]) & 1 != 0)
}

/// Fn 1 reply: location and number of effects of a zone.
fn parse_zone(r: &[u8; 16]) -> (u16, u8) {
    (u16::from_be_bytes([r[1], r[2]]), r[3])
}

/// Fn 2 reply.
fn parse_effect_info(r: &[u8; 16]) -> EffectInfo {
    EffectInfo {
        index: r[1],
        id: u16::from_be_bytes([r[2], r[3]]),
        capabilities: u16::from_be_bytes([r[4], r[5]]),
        default_period: u16::from_be_bytes([r[6], r[7]]),
    }
}

/// Fn 14 reply: zone, effect index, 10 parameter bytes.
fn parse_current(zone: u8, r: &[u8; 16], effects: &[EffectInfo]) -> Result<Effect> {
    if r[0] != zone {
        return Err(Error::Invalid(format!("LED zone {zone} answered as zone {}", r[0])));
    }
    let info = effects.iter().find(|e| e.index == r[1]).ok_or_else(|| Error::Invalid(format!("LED zone {zone} reports unknown effect {}", r[1])))?;
    let mut params = [0u8; 10];
    params.copy_from_slice(&r[2..12]);
    Ok(decode(info.id, &params))
}

/// Fn 3 parameters: zone, effect index within the zone, 10 parameter bytes.
fn set_params(zone: u8, index: u8, effect: &Effect) -> [u8; 12] {
    let mut out = [0u8; 12];
    out[..2].copy_from_slice(&[zone, index]);
    out[2..].copy_from_slice(&effect.encode());
    out
}

fn read_state(call: impl Fn(u8, &[u8]) -> Result<[u8; 16]>) -> Result<State> {
    let (zone_count, readable) = parse_info(&call(0, &[])?);
    let mut zones = Vec::with_capacity(zone_count as usize);
    for zone in 0..zone_count {
        let (location, count) = parse_zone(&call(1, &[zone, 0xFF, 0])?);
        let mut effects = Vec::with_capacity(count as usize);
        for i in 0..count {
            effects.push(parse_effect_info(&call(2, &[zone, i, 0])?));
        }
        let current = if readable { call(14, &[zone]).ok().and_then(|r| parse_current(zone, &r, &effects).ok()) } else { None };
        zones.push(Zone { index: zone, location, effects, current });
    }
    let sw_control = call(7, &[]).ok().map(|r| r[0] != 0);
    Ok(State { readable, sw_control, zones })
}

fn apply_with(call: impl Fn(u8, &[u8]) -> Result<[u8; 16]>, state: &mut State, action: &Action) -> Result<()> {
    match action {
        Action::SetControl(on) => {
            call(8, &[u8::from(*on)])?;
            state.sw_control = Some(*on);
        }
        Action::SetEffect { zone, effect, .. } => {
            let z = state.zones.iter_mut().find(|z| z.index == *zone).ok_or_else(|| Error::Invalid(format!("the device has no LED zone {zone}")))?;
            let index = z
                .effects
                .iter()
                .find(|e| e.id == effect.id)
                .map(|e| e.index)
                .ok_or_else(|| Error::Invalid(format!("{} is not available on {}", effect_name(effect.id), z.title())))?;
            if state.sw_control != Some(true) {
                call(8, &[1])?;
                state.sw_control = Some(true);
            }
            call(3, &set_params(*zone, index, effect))?;
            z.current = Some(effect.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// Hex bytes of a reply, padded to 16 bytes.
    fn reply(hex: &str) -> [u8; 16] {
        let clean: String = hex.split_whitespace().collect();
        let mut out = [0u8; 16];
        for (i, b) in out.iter_mut().enumerate().take(clean.len() / 2) {
            *b = u8::from_str_radix(&clean[2 * i..2 * i + 2], 16).unwrap();
        }
        out
    }

    /// (function, params, reply hex) recorded from real devices.
    type Fixture = &'static [(u8, &'static [u8], &'static str)];

    fn serve(table: Fixture, function: u8, params: &[u8]) -> Result<[u8; 16]> {
        table.iter().find(|(f, p, _)| *f == function && *p == params).map(|(_, _, hex)| reply(hex)).ok_or(Error::Hidpp20(0x07))
    }

    /// Logitech G502 LIGHTSPEED: two zones, not readable.
    const G502: Fixture = &[
        (0, &[], "02 00 03 00 04"),
        (1, &[0, 0xFF, 0], "00 00 01 04"),
        (1, &[1, 0xFF, 0], "01 00 02 04"),
        (2, &[0, 0, 0], "00 00 00 00 00 00 00 00"),
        (2, &[0, 1, 0], "00 01 00 01 00 00 00 00"),
        (2, &[0, 2, 0], "00 02 00 03 c0 05 03 e8"),
        (2, &[0, 3, 0], "00 03 00 0a c1 05 00 3c"),
        (2, &[1, 0, 0], "01 00 00 00 00 00 00 00"),
        (2, &[1, 1, 0], "01 01 00 01 00 00 00 00"),
        (2, &[1, 2, 0], "01 02 00 03 c0 05 03 e8"),
        (2, &[1, 3, 0], "01 03 00 0a c1 05 00 3c"),
        (7, &[], "00"),
    ];

    /// Logitech G610 keyboard: two zones, not readable.
    const G610: Fixture = &[
        (0, &[], "02 00 01 00 08"),
        (1, &[0, 0xFF, 0], "00 00 01 06"),
        (1, &[1, 0xFF, 0], "01 00 02 03"),
        (2, &[0, 0, 0], "0000000000000000"),
        (2, &[0, 1, 0], "0001000100050000"),
        (2, &[0, 2, 0], "0002000ac00103e0"),
        (2, &[0, 3, 0], "00030004dce1001e"),
        (2, &[0, 4, 0], "0004000500000000"),
        (2, &[0, 5, 0], "0005000b00000000"),
        (2, &[1, 0, 0], "0100000000000000"),
        (2, &[1, 1, 0], "0101000100050000"),
        (2, &[1, 2, 0], "0102000ac00103e0"),
        (7, &[], "00"),
    ];

    fn g502(function: u8, params: &[u8]) -> Result<[u8; 16]> {
        serve(G502, function, params)
    }

    fn g610(function: u8, params: &[u8]) -> Result<[u8; 16]> {
        serve(G610, function, params)
    }

    #[test]
    fn parses_info_and_zones() {
        assert_eq!(parse_info(&reply("02 00 03 00 04")), (2, false));
        assert_eq!(parse_info(&reply("02 00 01 00 08")), (2, false));
        assert_eq!(parse_info(&reply("02 00 01 00 01")), (2, true));
        assert_eq!(parse_zone(&reply("00 00 01 04")), (1, 4));
        assert_eq!(parse_zone(&reply("01 00 02 03")), (2, 3));
    }

    #[test]
    fn parses_effect_info() {
        let breathe = parse_effect_info(&reply("00 03 00 0a c1 05 00 3c"));
        assert_eq!(breathe, EffectInfo { index: 3, id: 0x0a, capabilities: 0xc105, default_period: 60 });
        let cycle = parse_effect_info(&reply("00 02 00 03 c0 05 03 e8"));
        assert_eq!(cycle, EffectInfo { index: 2, id: 0x03, capabilities: 0xc005, default_period: 1000 });
    }

    #[test]
    fn reads_g502_state() {
        let state = read_state(g502).unwrap();
        assert!(!state.readable);
        assert_eq!(state.sw_control, Some(false));
        assert_eq!(state.zones.len(), 2);
        assert_eq!(state.zones[0].title(), "Primary");
        assert_eq!(state.zones[1].title(), "Logo");
        assert_eq!(state.zones[0].effects.len(), 4);
        assert_eq!(state.zones[0].effects[2].id, 0x03);
        assert_eq!(state.zones[1].effects[3].id, 0x0a);
        assert!(state.zones.iter().all(|z| z.current.is_none()));
    }

    #[test]
    fn reads_g610_state() {
        let state = read_state(g610).unwrap();
        assert_eq!(state.sw_control, Some(false));
        assert_eq!(state.zones.len(), 2);
        assert_eq!(state.zones[0].effects.len(), 6);
        assert_eq!(state.zones[0].effects[4].id, 0x05);
        assert_eq!(state.zones[0].effects[3].default_period, 30);
        assert_eq!(state.zones[1].title(), "Logo");
    }

    #[test]
    fn current_effect_error_is_not_fatal() {
        // Capability bit says readable, but fn 14 is unsupported on this device.
        let call = |f: u8, p: &[u8]| -> Result<[u8; 16]> {
            match f {
                0 => Ok(reply("02 00 01 00 09")),
                14 => Err(Error::Hidpp20(0x07)),
                _ => g610(f, p),
            }
        };
        let state = read_state(&call).unwrap();
        assert!(state.readable);
        assert!(state.zones.iter().all(|z| z.current.is_none()));
    }

    #[test]
    fn decodes_current_effect() {
        let effects = read_state(g610).unwrap().zones[0].effects.clone();
        let current = parse_current(0, &reply("00 02 ff 00 00 0f a0 00 64 00 00 00"), &effects).unwrap();
        assert_eq!(current.id, 0x0a);
        assert_eq!(current.color, [255, 0, 0]);
        assert_eq!(current.period, 4000);
        assert_eq!(current.intensity, 100);
        assert_eq!(current.form, 0);
    }

    #[test]
    fn rejects_mismatched_current_effect() {
        let effects = read_state(g610).unwrap().zones[0].effects.clone();
        assert!(matches!(parse_current(1, &reply("00 02 ff 00 00"), &effects), Err(Error::Invalid(_))));
        assert!(matches!(parse_current(0, &reply("00 09 ff 00 00"), &effects), Err(Error::Invalid(_))));
    }

    #[test]
    fn encodes_breathe_command() {
        let effect = Effect { color: [255, 0, 0], period: 4000, ..Effect::new(0x0a) };
        assert_eq!(effect.intensity, 100);
        assert_eq!(set_params(0, 2, &effect), [0x00, 0x02, 0xff, 0, 0, 0x0f, 0xa0, 0, 0x64, 0, 0, 0]);
    }

    #[test]
    fn static_and_ripple_layouts() {
        let fixed = Effect { color: [1, 2, 3], ramp: 2, ..Effect::new(STATIC) };
        assert_eq!(fixed.encode(), [1, 2, 3, 2, 0, 0, 0, 0, 0, 0]);
        // Ripple (0x17): saturation at 3, period at 6..8.
        let ripple = Effect::new(0x17);
        assert_eq!(ripple.saturation, 255);
        assert_eq!(ripple.period, 20);
        assert_eq!(ripple.encode(), [255, 255, 255, 255, 0, 0, 0, 20, 0, 0]);
        assert_eq!(period_range(0x17), (2, 200));
        assert_eq!(period_range(0x0A), (1000, 20000));
    }

    #[test]
    fn decodes_breathe_round_trip() {
        let effect = Effect { color: [10, 20, 30], period: 1500, intensity: 40, form: 3, ..Effect::new(0x0a) };
        assert_eq!(decode(0x0a, &effect.encode()), effect);
    }

    #[test]
    fn layouts_fit_and_do_not_overlap() {
        for s in EFFECTS {
            let mut used = [false; 10];
            for &(field, at) in s.fields {
                let width = match field {
                    Field::Color => 3,
                    Field::Period => 2,
                    _ => 1,
                };
                assert!(at + width <= 10, "0x{:02X} {field:?} overflows", s.id);
                for slot in &mut used[at..at + width] {
                    assert!(!*slot, "0x{:02X} {field:?} overlaps", s.id);
                    *slot = true;
                }
            }
        }
    }

    #[test]
    fn unknown_effect_has_no_parameters() {
        assert_eq!(effect_name(0x05), "Effect 0x05");
        assert!(!has_field(0x05, Field::Color));
        assert_eq!(Effect::new(0x05).label(), "Effect 0x05");
    }

    #[test]
    fn location_names() {
        assert_eq!(location_name(0), None);
        assert_eq!(location_name(1), Some("Primary"));
        assert_eq!(location_name(2), Some("Logo"));
        assert_eq!(location_name(6), Some("Primary 1"));
        assert_eq!(location_name(11), Some("Primary 6"));
        assert_eq!(location_name(12), None);
    }

    #[test]
    fn describes_actions() {
        let breathe = Effect { color: [255, 0, 0], period: 4000, ..Effect::new(0x0a) };
        let action = Action::SetEffect { zone: 1, location: 2, effect: breathe };
        assert_eq!(action.describe(), "Logo: Breathe #ff0000 4.0 s");
        assert_eq!(Action::SetControl(true).describe(), "LED control: logiset");
        assert_eq!(Action::SetControl(false).describe(), "LED control: device");
        assert_eq!(action.persist_key().as_deref(), Some("lighting.zone.1"));
        assert_eq!(Action::SetControl(true).persist_key().as_deref(), Some("lighting.control"));
    }

    #[test]
    fn same_setting_ignores_unused_fields() {
        let a = Effect::new(0x01);
        let b = Effect { speed: 9, period: 7, ..Effect::new(0x01) };
        assert!(a.same_setting(&b));
        assert!(!a.same_setting(&Effect { color: [1, 1, 1], ..Effect::new(0x01) }));
        assert!(!a.same_setting(&Effect::new(0x02)));
    }

    #[test]
    fn action_round_trips_through_json() {
        let action = Action::SetEffect { zone: 0, location: 1, effect: Effect::new(0x03) };
        let json = serde_json::to_value(&action).unwrap();
        let back: Action = serde_json::from_value(json).unwrap();
        assert_eq!(back, action);
    }

    #[test]
    fn apply_enables_control_then_writes_effect() {
        let log = RefCell::new(Vec::new());
        let call = |f: u8, p: &[u8]| -> Result<[u8; 16]> {
            log.borrow_mut().push((f, p.to_vec()));
            Ok([0; 16])
        };
        let mut state = read_state(g610).unwrap();
        let breathe = Effect { color: [255, 0, 0], period: 4000, ..Effect::new(0x0a) };
        let action = Action::SetEffect { zone: 0, location: 1, effect: breathe.clone() };
        apply_with(&call, &mut state, &action).unwrap();
        assert_eq!(*log.borrow(), vec![(8, vec![1]), (3, vec![0, 2, 0xff, 0, 0, 0x0f, 0xa0, 0, 0x64, 0, 0, 0])]);
        assert_eq!(state.sw_control, Some(true));
        assert_eq!(state.zones[0].current, Some(breathe));
    }

    #[test]
    fn apply_skips_control_when_already_on() {
        let log = RefCell::new(Vec::new());
        let call = |f: u8, p: &[u8]| -> Result<[u8; 16]> {
            log.borrow_mut().push((f, p.to_vec()));
            Ok([0; 16])
        };
        let mut state = read_state(g610).unwrap();
        state.sw_control = Some(true);
        let action = Action::SetEffect { zone: 1, location: 2, effect: Effect::new(STATIC) };
        apply_with(&call, &mut state, &action).unwrap();
        assert_eq!(*log.borrow(), vec![(3, vec![1, 1, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0])]);
    }

    #[test]
    fn apply_rejects_unknown_zone_or_effect_without_writing() {
        let log = RefCell::new(Vec::new());
        let call = |f: u8, p: &[u8]| -> Result<[u8; 16]> {
            log.borrow_mut().push((f, p.to_vec()));
            Ok([0; 16])
        };
        let mut state = read_state(g610).unwrap();
        let missing_effect = Action::SetEffect { zone: 1, location: 2, effect: Effect::new(0x03) };
        assert!(matches!(apply_with(&call, &mut state, &missing_effect), Err(Error::Invalid(_))));
        let missing_zone = Action::SetEffect { zone: 7, location: 0, effect: Effect::new(STATIC) };
        assert!(matches!(apply_with(&call, &mut state, &missing_zone), Err(Error::Invalid(_))));
        assert!(log.borrow().is_empty());
        assert!(state.zones.iter().all(|z| z.current.is_none()));
    }

    #[test]
    fn set_control_writes_switch() {
        let log = RefCell::new(Vec::new());
        let call = |f: u8, p: &[u8]| -> Result<[u8; 16]> {
            log.borrow_mut().push((f, p.to_vec()));
            Ok([0; 16])
        };
        let mut state = read_state(g502).unwrap();
        apply_with(&call, &mut state, &Action::SetControl(true)).unwrap();
        apply_with(&call, &mut state, &Action::SetControl(false)).unwrap();
        assert_eq!(*log.borrow(), vec![(8, vec![1]), (8, vec![0])]);
        assert_eq!(state.sw_control, Some(false));
    }
}
