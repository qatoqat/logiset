//! Scroll wheel: SmartShift ratchet (0x2110 / 0x2111) and hi-res wheel direction (0x2121).
//!
//! Byte layouts follow Solaar (`lib/logitech_receiver/settings_templates.py` and
//! `hidpp20.py`). Hi-res resolution and HID++ diversion are read only: the kernel
//! driver manages them, and changing them would break scrolling.

use serde::{Deserialize, Serialize};

use crate::hidpp::{Device, Error, Result};

const SMART_SHIFT: u16 = 0x2110;
const SMART_SHIFT_ENHANCED: u16 = 0x2111;
const HIRES_WHEEL: u16 = 0x2121;

/// SmartShift wheel mode byte.
const FREESPIN: u8 = 1;
const RATCHET: u8 = 2;

/// Highest auto-shift threshold. The wheel free-spins above it.
pub const AUTO_SHIFT_MAX: u8 = 254;
/// Threshold that keeps the wheel ratcheted at all speeds.
pub const NEVER_SHIFT: u8 = 255;
/// Highest ratchet torque on 0x2111 devices that can tune it.
pub const TORQUE_MAX: u8 = 100;

/// Hi-res mode bits. Bit 0x01 (HID++ diversion) is never set by logiset; it is
/// written back as read.
const MODE_RESOLUTION: u8 = 0x02;
const MODE_INVERT: u8 = 0x04;

/// What the device reports for the scroll wheel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// SmartShift from 0x2111 when present, otherwise from 0x2110.
    pub smartshift: Option<SmartShift>,
    /// Hi-res wheel (0x2121).
    pub hires: Option<HiRes>,
}

/// Ratchet state of the wheel.
#[derive(Clone, Debug, PartialEq)]
pub struct SmartShift {
    /// 0x2110 or 0x2111.
    pub feature: u16,
    /// Ratchet (true) or free-spin (false).
    pub ratchet: bool,
    /// Wheel speed above which the ratchet releases. [`NEVER_SHIFT`] keeps it engaged.
    pub threshold: u8,
    /// Factory threshold (0x2110 only).
    pub default_threshold: Option<u8>,
    /// Ratchet torque. Only on 0x2111 devices that can tune it.
    pub torque: Option<u8>,
}

/// Hi-res wheel settings.
#[derive(Clone, Debug, PartialEq)]
pub struct HiRes {
    /// Scroll steps per notch, as reported by the device.
    pub multiplier: u8,
    /// The direction can be inverted.
    pub can_invert: bool,
    pub invert: bool,
    /// High-resolution scrolling is on.
    pub resolution: bool,
    /// Physical ratchet switch, when the device has one.
    pub ratchet_switch: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// `threshold` 1..=255 ([`NEVER_SHIFT`] keeps the ratchet). `torque` is only
    /// written on devices that can tune it.
    SetSmartShift {
        ratchet: bool,
        threshold: u8,
        torque: Option<u8>,
    },
    SetInvert(bool),
}

impl Action {
    pub fn persist_key(&self) -> Option<String> {
        let key = match self {
            Action::SetSmartShift { .. } => "wheel.smartshift",
            Action::SetInvert(_) => "wheel.invert",
        };
        Some(key.into())
    }

    pub fn describe(&self) -> String {
        match self {
            Action::SetSmartShift { ratchet, threshold, torque } => {
                let mut text = String::from(if *ratchet { "SmartShift: ratchet" } else { "SmartShift: free-spin" });
                if *ratchet {
                    if *threshold == NEVER_SHIFT {
                        text.push_str(", auto-shift off");
                    } else {
                        text.push_str(&format!(", auto-shift at {threshold}"));
                    }
                }
                if let Some(t) = torque {
                    text.push_str(&format!(", torque {t}"));
                }
                text
            }
            Action::SetInvert(on) => format!("Scroll direction: {}", if *on { "inverted" } else { "normal" }),
        }
    }
}

/// Read-only: reads whichever wheel features the device has. `None` when it has none.
pub fn probe(dev: &Device) -> Option<Result<State>> {
    let shift = [SMART_SHIFT_ENHANCED, SMART_SHIFT].into_iter().find(|&f| dev.has(f));
    let hires = dev.has(HIRES_WHEEL);
    if shift.is_none() && !hires {
        return None;
    }
    Some(read_state(dev, shift, hires))
}

/// Write one setting, then read back what the device now holds.
pub fn apply(dev: &Device, state: &mut State, action: &Action) -> Result<()> {
    match action {
        Action::SetSmartShift { ratchet, threshold, torque } => {
            let ss = state.smartshift.as_mut().ok_or(Error::Unsupported(SMART_SHIFT))?;
            let feature = ss.feature;
            let params = smart_shift_params(ss, *ratchet, *threshold, *torque)?;
            let function = if feature == SMART_SHIFT_ENHANCED { 2 } else { 1 };
            dev.call(feature, function, &params)?;
            *ss = read_smart_shift(dev, feature)?;
        }
        Action::SetInvert(on) => {
            let hires = state.hires.as_mut().ok_or(Error::Unsupported(HIRES_WHEEL))?;
            if !hires.can_invert {
                return Err(Error::Invalid("this device cannot invert the scroll direction".into()));
            }
            // Read the mode again so the bits logiset does not own match the device now.
            let mode = read_hires_mode(dev)?;
            dev.call(HIRES_WHEEL, 2, &[hires_mode_with_invert(mode, *on)])?;
            *hires = read_hires(dev)?;
        }
    }
    Ok(())
}

fn read_state(dev: &Device, shift: Option<u16>, hires: bool) -> Result<State> {
    Ok(State { smartshift: shift.map(|f| read_smart_shift(dev, f)).transpose()?, hires: hires.then(|| read_hires(dev)).transpose()? })
}

fn read_smart_shift(dev: &Device, feature: u16) -> Result<SmartShift> {
    if feature == SMART_SHIFT_ENHANCED {
        let tunable = tunable_torque(&dev.call(feature, 0, &[])?);
        parse_smart_shift_enhanced(&dev.call(feature, 1, &[])?, tunable)
    } else {
        parse_ratchet_control(&dev.call(feature, 0, &[])?)
    }
}

fn read_hires(dev: &Device) -> Result<HiRes> {
    let (multiplier, can_invert, has_switch) = parse_hires_caps(&dev.call(HIRES_WHEEL, 0, &[])?);
    let mode = read_hires_mode(dev)?;
    // The switch is informational only: a device that refuses the read still gets its invert control.
    let ratchet_switch = if has_switch { dev.call(HIRES_WHEEL, 3, &[]).ok().map(|r| parse_ratchet_switch(&r)) } else { None };
    Ok(HiRes { multiplier, can_invert, invert: mode & MODE_INVERT != 0, resolution: mode & MODE_RESOLUTION != 0, ratchet_switch })
}

fn read_hires_mode(dev: &Device) -> Result<u8> {
    Ok(dev.call(HIRES_WHEEL, 1, &[])?[0])
}

/// 0x2110 getRatchetControlMode reply: `[wheel_mode, auto_disengage, auto_disengage_default]`.
fn parse_ratchet_control(r: &[u8; 16]) -> Result<SmartShift> {
    Ok(SmartShift { feature: SMART_SHIFT, ratchet: parse_wheel_mode(r[0])?, threshold: r[1], default_threshold: Some(r[2]), torque: None })
}

/// 0x2111 get-status reply: `[wheel_mode, auto_disengage, torque]`. Torque is only
/// reported when `tunable`.
fn parse_smart_shift_enhanced(r: &[u8; 16], tunable: bool) -> Result<SmartShift> {
    Ok(SmartShift {
        feature: SMART_SHIFT_ENHANCED,
        ratchet: parse_wheel_mode(r[0])?,
        threshold: r[1],
        default_threshold: None,
        torque: tunable.then_some(r[2]),
    })
}

fn parse_wheel_mode(byte: u8) -> Result<bool> {
    match byte {
        FREESPIN => Ok(false),
        RATCHET => Ok(true),
        other => Err(Error::Invalid(format!("unknown SmartShift wheel mode {other}"))),
    }
}

/// 0x2111 getCapabilities reply: bit 0 of byte 0 means the torque can be set.
fn tunable_torque(caps: &[u8; 16]) -> bool {
    caps[0] & 0x01 != 0
}

/// Parameters for the SmartShift write: 0x2110 fn1 `setRatchetControlMode`, or
/// 0x2111 fn2 `setStatus`. Layout `[wheel_mode, auto_disengage, third]`, where the
/// third byte is the factory default on 0x2110 and the torque on 0x2111. Zero
/// leaves that byte unchanged, which is how the third byte is sent here.
fn smart_shift_params(ss: &SmartShift, ratchet: bool, threshold: u8, torque: Option<u8>) -> Result<[u8; 3]> {
    if threshold == 0 {
        return Err(Error::Invalid("auto-shift threshold must be between 1 and 255".into()));
    }
    let third = match torque {
        None => 0,
        Some(_) if ss.torque.is_none() => return Err(Error::Invalid("this device has no adjustable ratchet torque".into())),
        Some(t @ 1..=TORQUE_MAX) => t,
        Some(_) => return Err(Error::Invalid(format!("ratchet torque must be between 1 and {TORQUE_MAX}"))),
    };
    let mode = if ratchet { RATCHET } else { FREESPIN };
    Ok([mode, threshold, third])
}

/// 0x2121 getWheelCapability reply: `[multiplier, flags]`. Flag 0x08 = invert,
/// 0x04 = physical ratchet switch.
fn parse_hires_caps(r: &[u8; 16]) -> (u8, bool, bool) {
    (r[0], r[1] & 0x08 != 0, r[1] & 0x04 != 0)
}

/// 0x2121 getRatchetSwitchState reply: bit 0 of byte 0.
fn parse_ratchet_switch(r: &[u8; 16]) -> bool {
    r[0] & 0x01 != 0
}

/// The hi-res mode byte with only the invert bit changed. Every other bit,
/// including the diversion bit, keeps the value it was read with.
fn hires_mode_with_invert(current: u8, invert: bool) -> u8 {
    (current & !MODE_INVERT) | if invert { MODE_INVERT } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(bytes: &[u8]) -> [u8; 16] {
        let mut r = [0u8; 16];
        r[..bytes.len()].copy_from_slice(bytes);
        r
    }

    fn shift(feature: u16, torque: Option<u8>) -> SmartShift {
        SmartShift { feature, ratchet: true, threshold: 30, default_threshold: None, torque }
    }

    #[test]
    fn ratchet_control_reply() {
        let s = parse_ratchet_control(&reply(&[2, 30, 40])).unwrap();
        assert_eq!(s, SmartShift { feature: SMART_SHIFT, ratchet: true, threshold: 30, default_threshold: Some(40), torque: None });
        let s = parse_ratchet_control(&reply(&[1, 255, 0])).unwrap();
        assert!(!s.ratchet);
        assert_eq!(s.threshold, NEVER_SHIFT);
    }

    #[test]
    fn unknown_wheel_mode_is_invalid() {
        assert!(matches!(parse_ratchet_control(&reply(&[0, 30, 0])), Err(Error::Invalid(_))));
        assert!(matches!(parse_ratchet_control(&reply(&[3, 30, 0])), Err(Error::Invalid(_))));
        assert!(matches!(parse_smart_shift_enhanced(&reply(&[7, 30, 0]), true), Err(Error::Invalid(_))));
    }

    #[test]
    fn enhanced_status_reply_with_and_without_torque() {
        let s = parse_smart_shift_enhanced(&reply(&[2, 30, 50]), true).unwrap();
        assert_eq!(s.feature, SMART_SHIFT_ENHANCED);
        assert!(s.ratchet);
        assert_eq!(s.threshold, 30);
        assert_eq!(s.torque, Some(50));
        assert_eq!(s.default_threshold, None);

        let s = parse_smart_shift_enhanced(&reply(&[1, 255, 9]), false).unwrap();
        assert!(!s.ratchet);
        assert_eq!(s.torque, None);
    }

    #[test]
    fn enhanced_capabilities_torque_bit() {
        assert!(tunable_torque(&reply(&[0x01])));
        assert!(tunable_torque(&reply(&[0x03])));
        assert!(!tunable_torque(&reply(&[0x00])));
        assert!(!tunable_torque(&reply(&[0x02])));
    }

    #[test]
    fn smartshift_params_layout() {
        let plain = shift(SMART_SHIFT, None);
        assert_eq!(smart_shift_params(&plain, true, 30, None), Ok([2, 30, 0]));
        assert_eq!(smart_shift_params(&plain, false, 255, None), Ok([1, 255, 0]));
        assert!(smart_shift_params(&plain, true, 0, None).is_err());
        assert!(smart_shift_params(&plain, true, 30, Some(50)).is_err());

        let tunable = shift(SMART_SHIFT_ENHANCED, Some(40));
        assert_eq!(smart_shift_params(&tunable, true, 30, Some(50)), Ok([2, 30, 50]));
        assert_eq!(smart_shift_params(&tunable, true, 30, None), Ok([2, 30, 0]));
        assert!(smart_shift_params(&tunable, true, 30, Some(0)).is_err());
        assert!(smart_shift_params(&tunable, true, 30, Some(101)).is_err());

        let fixed = shift(SMART_SHIFT_ENHANCED, None);
        assert!(smart_shift_params(&fixed, true, 30, Some(50)).is_err());
    }

    #[test]
    fn hires_caps_flags() {
        assert_eq!(parse_hires_caps(&reply(&[8, 0x0C])), (8, true, true));
        assert_eq!(parse_hires_caps(&reply(&[1, 0x04])), (1, false, true));
        assert_eq!(parse_hires_caps(&reply(&[1, 0x08])), (1, true, false));
        assert_eq!(parse_hires_caps(&reply(&[1, 0x00])), (1, false, false));
    }

    #[test]
    fn hires_invert_bit_only() {
        assert_eq!(hires_mode_with_invert(0x00, true), 0x04);
        assert_eq!(hires_mode_with_invert(0x04, false), 0x00);
        assert_eq!(hires_mode_with_invert(0x03, true), 0x07);
        assert_eq!(hires_mode_with_invert(0x07, false), 0x03);
        // The diversion bit (0x01) stays as read, whatever the invert request.
        assert_eq!(hires_mode_with_invert(0x01, true), 0x05);
        assert_eq!(hires_mode_with_invert(0x01, false), 0x01);
        for current in 0..=255u8 {
            for invert in [false, true] {
                assert_eq!(hires_mode_with_invert(current, invert) & 0x01, current & 0x01);
                assert_eq!(hires_mode_with_invert(current, invert) & MODE_RESOLUTION, current & MODE_RESOLUTION);
            }
        }
    }

    #[test]
    fn ratchet_switch_bit() {
        assert!(parse_ratchet_switch(&reply(&[0x01])));
        assert!(!parse_ratchet_switch(&reply(&[0x00])));
        assert!(!parse_ratchet_switch(&reply(&[0xFE])));
    }

    #[test]
    fn describe_and_persist_keys() {
        let ss = Action::SetSmartShift { ratchet: true, threshold: 30, torque: None };
        assert_eq!(ss.describe(), "SmartShift: ratchet, auto-shift at 30");
        assert_eq!(ss.persist_key().as_deref(), Some("wheel.smartshift"));

        let off = Action::SetSmartShift { ratchet: true, threshold: NEVER_SHIFT, torque: None };
        assert_eq!(off.describe(), "SmartShift: ratchet, auto-shift off");

        let free = Action::SetSmartShift { ratchet: false, threshold: 30, torque: Some(50) };
        assert_eq!(free.describe(), "SmartShift: free-spin, torque 50");

        assert_eq!(Action::SetInvert(true).describe(), "Scroll direction: inverted");
        assert_eq!(Action::SetInvert(false).describe(), "Scroll direction: normal");
        assert_eq!(Action::SetInvert(true).persist_key().as_deref(), Some("wheel.invert"));
    }

    #[test]
    fn saved_actions_round_trip() {
        for action in [
            Action::SetSmartShift { ratchet: true, threshold: 30, torque: Some(50) },
            Action::SetSmartShift { ratchet: false, threshold: NEVER_SHIFT, torque: None },
            Action::SetInvert(true),
        ] {
            let value = serde_json::to_value(&action).unwrap();
            let back: Action = serde_json::from_value(value).unwrap();
            assert_eq!(back, action);
        }
    }
}
