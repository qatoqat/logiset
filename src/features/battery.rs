//! Battery reading, read-only. Prefers 0x1004 Unified Battery, then 0x1000 Battery Status,
//! then 0x1001 Battery Voltage. Decoding follows Solaar (`hidpp20.py`: `decipher_battery_*`,
//! `estimate_battery_level_percentage`).

use crate::hidpp::{Device, Error, Result};

pub const BATTERY_STATUS: u16 = 0x1000;
pub const BATTERY_VOLTAGE: u16 = 0x1001;
pub const UNIFIED_BATTERY: u16 = 0x1004;

/// Charge level named by 0x1004 (byte 1 is a bit mask: 8 full, 4 good, 2 low, 1 critical).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Empty,
    Critical,
    Low,
    Good,
    Full,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Empty => "empty",
            Level::Critical => "critical",
            Level::Low => "low",
            Level::Good => "good",
            Level::Full => "full",
        }
    }
}

/// Charging state, from the status byte of 0x1000 / 0x1004 or the flags of 0x1001.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ChargeState {
    Discharging,
    Charging,
    /// Charging below the optimal speed.
    SlowCharging,
    Full,
    /// The battery or charger reports a fault.
    Error(String),
    #[default]
    Unknown,
}

impl ChargeState {
    /// Short text for the overview, e.g. "charging" or "error: thermal fault".
    pub fn label(&self) -> String {
        match self {
            ChargeState::Discharging => "discharging".into(),
            ChargeState::Charging => "charging".into(),
            ChargeState::SlowCharging => "charging slowly".into(),
            ChargeState::Full => "full".into(),
            ChargeState::Error(e) => format!("error: {e}"),
            ChargeState::Unknown => "unknown".into(),
        }
    }
}

/// What the battery feature reported.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Feature the reading came from (0x1004, 0x1000 or 0x1001).
    pub feature: u16,
    /// Charge in percent. Estimated from the voltage when `estimated` is set; `None` without a reading.
    pub percent: Option<u8>,
    pub estimated: bool,
    pub voltage_mv: Option<u16>,
    /// Level named by the device: 0x1004 reports one, 0x1001 flags name Full and Critical.
    pub level: Option<Level>,
    pub charging: ChargeState,
    /// Derived from the charging state: `Some(true)` while charging, `Some(false)` while discharging.
    pub external_power: Option<bool>,
}

/// READ-ONLY. Reads the battery through the first of 0x1004, 0x1000, 0x1001 the device has.
/// A feature that fails is skipped, except for timeouts and disconnects, which would only repeat.
pub fn probe(dev: &Device) -> Option<Result<State>> {
    let mut failure: Option<Error> = None;
    for (id, function) in [(UNIFIED_BATTERY, 1), (BATTERY_STATUS, 0), (BATTERY_VOLTAGE, 0)] {
        if !dev.has(id) {
            continue;
        }
        match dev.call(id, function, &[]) {
            Ok(r) => {
                return Some(Ok(match id {
                    UNIFIED_BATTERY => parse_unified(&r),
                    BATTERY_STATUS => parse_status(&r),
                    _ => parse_voltage(&r),
                }));
            }
            Err(e @ (Error::Timeout | Error::Disconnected)) => return Some(Err(e)),
            Err(e) => failure = Some(e),
        }
    }
    failure.map(Err)
}

/// 0x1004 getStatus: charge %, level bit mask, charging status, (byte 3 unused, as in Solaar).
pub fn parse_unified(r: &[u8; 16]) -> State {
    let level = match r[1] {
        8 => Level::Full,
        4 => Level::Good,
        2 => Level::Low,
        1 => Level::Critical,
        _ => Level::Empty,
    };
    let charging = charging_from_status(r[2]);
    State {
        feature: UNIFIED_BATTERY,
        percent: (r[0] != 0).then_some(r[0]),
        estimated: false,
        voltage_mv: None,
        level: Some(level),
        external_power: external_power(&charging),
        charging,
    }
}

/// 0x1000 getBatteryLevelStatus: charge %, next level %, charging status.
pub fn parse_status(r: &[u8; 16]) -> State {
    let charging = charging_from_status(r[2]);
    State {
        feature: BATTERY_STATUS,
        percent: (r[0] != 0).then_some(r[0]),
        estimated: false,
        voltage_mv: None,
        level: None,
        external_power: external_power(&charging),
        charging,
    }
}

/// 0x1001 getBatteryInfo: voltage in mV (big-endian), then flags.
/// Flags: bit 7 charging, bits 0-1 charge status (bit 0 full), bit 3 fast, bit 4 slow, bit 5 critical.
/// A 0 mV reading has no estimate, so it gives no percentage; the flags name the level instead.
pub fn parse_voltage(r: &[u8; 16]) -> State {
    let voltage = u16::from_be_bytes([r[0], r[1]]);
    let flags = r[2];
    let mut charging = ChargeState::Discharging;
    if flags & 0x80 != 0 {
        charging = ChargeState::Charging;
        if flags & 0x01 != 0 {
            charging = ChargeState::Full;
        }
    }
    let mut level = (charging == ChargeState::Full).then_some(Level::Full);
    if flags & 0x08 != 0 {
        // Fast charge: nothing more to read from the flags.
    } else if flags & 0x10 != 0 {
        charging = ChargeState::SlowCharging;
    } else if flags & 0x20 != 0 {
        level = Some(Level::Critical);
    }
    // Any voltage gets a curve estimate, and 0% from the curve is a real value.
    let (percent, estimated) = if voltage == 0 { (None, false) } else { (Some(voltage_percent(voltage)), true) };
    State {
        feature: BATTERY_VOLTAGE,
        percent,
        estimated,
        voltage_mv: Some(voltage),
        level,
        external_power: external_power(&charging),
        charging,
    }
}

/// Solaar's voltage-to-percent table (mV, percent), highest voltage first.
const VOLTAGE_CURVE: [(u16, u8); 13] = [
    (4186, 100),
    (4067, 90),
    (3989, 80),
    (3922, 70),
    (3859, 60),
    (3811, 50),
    (3778, 40),
    (3751, 30),
    (3717, 20),
    (3671, 10),
    (3646, 5),
    (3579, 2),
    (3500, 0),
];

/// Linear interpolation on [`VOLTAGE_CURVE`]. Returns 0 when no segment matches.
pub fn voltage_percent(mv: u16) -> u8 {
    let (top_mv, top_pct) = VOLTAGE_CURVE[0];
    let (low_mv, low_pct) = VOLTAGE_CURVE[VOLTAGE_CURVE.len() - 1];
    if mv >= top_mv {
        return top_pct;
    }
    if mv <= low_mv {
        return low_pct;
    }
    let mv = i64::from(mv);
    for pair in VOLTAGE_CURVE.windows(2) {
        let (v_high, p_high) = (i64::from(pair[0].0), i64::from(pair[0].1));
        let (v_low, p_low) = (i64::from(pair[1].0), i64::from(pair[1].1));
        if v_low <= mv && mv <= v_high {
            // Same order of operations as Solaar: p_low + (p_high - p_low) * (mv - v_low) / (v_high - v_low)
            let percent = p_low as f64 + ((p_high - p_low) * (mv - v_low)) as f64 / (v_high - v_low) as f64;
            return round_half_even(percent) as u8;
        }
    }
    0
}

/// Python's `round()` for non-negative values: halves go to the even neighbour.
fn round_half_even(x: f64) -> i64 {
    let floor = x.floor();
    let base = floor as i64;
    let frac = x - floor;
    if frac > 0.5 || (frac == 0.5 && base % 2 != 0) { base + 1 } else { base }
}

/// Status byte shared by 0x1000 and 0x1004 (values from Solaar's `BatteryStatus`).
fn charging_from_status(status: u8) -> ChargeState {
    match status {
        0 => ChargeState::Discharging,
        // 2 is "almost full": still charging.
        1 | 2 => ChargeState::Charging,
        3 => ChargeState::Full,
        4 => ChargeState::SlowCharging,
        5 => ChargeState::Error("invalid battery".into()),
        6 => ChargeState::Error("thermal fault".into()),
        7 => ChargeState::Error("charging fault".into()),
        _ => ChargeState::Unknown,
    }
}

/// Charging implies external power; discharging implies none. Faults and unknown states say nothing.
fn external_power(charging: &ChargeState) -> Option<bool> {
    match charging {
        ChargeState::Discharging => Some(false),
        ChargeState::Charging | ChargeState::SlowCharging | ChargeState::Full => Some(true),
        ChargeState::Error(_) | ChargeState::Unknown => None,
    }
}

/// Short text for the device list, e.g. "85%", "≈85%" (estimated), "85% ⚡" while charging, "full".
pub fn summary(state: &State) -> String {
    let mut text = if state.charging == ChargeState::Full {
        "full".to_owned()
    } else if let Some(p) = state.percent {
        format!("{}{p}%", if state.estimated { "≈" } else { "" })
    } else if let Some(level) = state.level {
        level.label().to_owned()
    } else {
        String::new()
    };
    if matches!(state.charging, ChargeState::Charging | ChargeState::SlowCharging) {
        if text.is_empty() {
            text = "charging".into();
        } else {
            text.push_str(" ⚡");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(bytes: &[u8]) -> [u8; 16] {
        let mut r = [0u8; 16];
        r[..bytes.len()].copy_from_slice(bytes);
        r
    }

    #[test]
    fn g502_voltage_reply() {
        // G502 LIGHTSPEED getBatteryInfo: 0x1073 = 4211 mV, flags 0x81.
        let s = parse_voltage(&reply(&[0x10, 0x73, 0x81]));
        assert_eq!(s.feature, BATTERY_VOLTAGE);
        assert_eq!(s.voltage_mv, Some(4211));
        assert_eq!(s.percent, Some(100));
        assert!(s.estimated);
        assert_eq!(s.charging, ChargeState::Full);
        assert_eq!(s.external_power, Some(true));
        assert_eq!(summary(&s), "full");
    }

    #[test]
    fn voltage_flags_follow_solaar() {
        let charging = |flags: u8| parse_voltage(&reply(&[0x10, 0x73, flags])).charging;
        assert_eq!(charging(0x00), ChargeState::Discharging);
        assert_eq!(charging(0x80), ChargeState::Charging);
        assert_eq!(charging(0x81), ChargeState::Full);
        // Status 2 ("not charging") is still reported as recharging by Solaar.
        assert_eq!(charging(0x82), ChargeState::Charging);
        assert_eq!(charging(0x10), ChargeState::SlowCharging);
        assert_eq!(charging(0x90), ChargeState::SlowCharging);
        // Fast charge hides the slow and critical bits.
        assert_eq!(charging(0x18), ChargeState::Discharging);
    }

    #[test]
    fn voltage_percent_from_curve() {
        // 3900 mV falls between 3922 (70%) and 3859 (60%): 60 + 10 * 41 / 63 = 66.5 -> 67.
        let s = parse_voltage(&reply(&[0x0F, 0x3C, 0x00]));
        assert_eq!((s.percent, s.estimated, s.charging.clone()), (Some(67), true, ChargeState::Discharging));
        assert_eq!(s.external_power, Some(false));
        assert_eq!(s.level, None);
    }

    #[test]
    fn voltage_below_curve_is_zero_percent() {
        // 3400 mV is a real reading below the curve: 0% is the estimate, not a flag fallback.
        let s = parse_voltage(&reply(&[0x0D, 0x48, 0x00]));
        assert_eq!((s.percent, s.estimated, s.level), (Some(0), true, None));
        assert_eq!(summary(&s), "≈0%");
        // The critical flag names the level next to the real percentage.
        let s = parse_voltage(&reply(&[0x0D, 0x48, 0x20]));
        assert_eq!((s.percent, s.estimated, s.level), (Some(0), true, Some(Level::Critical)));
    }

    #[test]
    fn zero_voltage_names_level() {
        // 0 mV is no reading: no percentage, and the flags name the level.
        let s = parse_voltage(&reply(&[0x00, 0x00, 0x00]));
        assert_eq!((s.percent, s.estimated, s.level), (None, false, None));
        assert_eq!(summary(&s), "");

        let s = parse_voltage(&reply(&[0x00, 0x00, 0x20]));
        assert_eq!((s.percent, s.estimated, s.level), (None, false, Some(Level::Critical)));
        assert_eq!(summary(&s), "critical");

        let s = parse_voltage(&reply(&[0x00, 0x00, 0x81]));
        assert_eq!((s.percent, s.estimated, s.level, s.charging.clone()), (None, false, Some(Level::Full), ChargeState::Full));
        assert_eq!(summary(&s), "full");

        // Fast charge hides the critical bit, as above.
        assert_eq!(parse_voltage(&reply(&[0x00, 0x00, 0x28])).level, None);
    }

    #[test]
    fn voltage_curve_points() {
        assert_eq!(voltage_percent(4300), 100);
        assert_eq!(voltage_percent(4186), 100);
        assert_eq!(voltage_percent(4067), 90);
        // 40 + 10 * 22 / 33 = 46.67
        assert_eq!(voltage_percent(3800), 47);
        // 20 + 10 * 33 / 34 = 29.7
        assert_eq!(voltage_percent(3750), 30);
        assert_eq!(voltage_percent(3500), 0);
        assert_eq!(voltage_percent(3000), 0);
    }

    #[test]
    fn rounding_matches_python() {
        assert_eq!(round_half_even(2.5), 2);
        assert_eq!(round_half_even(3.5), 4);
        assert_eq!(round_half_even(2.4), 2);
        assert_eq!(round_half_even(2.6), 3);
    }

    #[test]
    fn unified_reply() {
        let s = parse_unified(&reply(&[85, 4, 0, 0]));
        assert_eq!((s.feature, s.percent, s.estimated), (UNIFIED_BATTERY, Some(85), false));
        assert_eq!((s.level, s.charging.clone()), (Some(Level::Good), ChargeState::Discharging));
        assert_eq!(s.external_power, Some(false));
        assert_eq!(summary(&s), "85%");

        // No percentage: the level bit mask names the charge.
        let s = parse_unified(&reply(&[0, 8, 3]));
        assert_eq!((s.percent, s.level, s.charging.clone()), (None, Some(Level::Full), ChargeState::Full));
        assert_eq!(summary(&s), "full");

        let s = parse_unified(&reply(&[0, 2, 1]));
        assert_eq!(summary(&s), "low ⚡");
    }

    #[test]
    fn status_reply() {
        let s = parse_status(&reply(&[60, 20, 4]));
        assert_eq!((s.feature, s.percent, s.charging.clone()), (BATTERY_STATUS, Some(60), ChargeState::SlowCharging));
        assert_eq!(summary(&s), "60% ⚡");

        let s = parse_status(&reply(&[0, 0, 5]));
        assert_eq!(s.percent, None);
        assert_eq!(s.charging, ChargeState::Error("invalid battery".into()));
        assert_eq!(s.external_power, None);
        assert_eq!(summary(&s), "");
    }

    #[test]
    fn summary_text() {
        let mut s = State { percent: Some(85), ..State::default() };
        assert_eq!(summary(&s), "85%");
        s.estimated = true;
        assert_eq!(summary(&s), "≈85%");
        s.charging = ChargeState::Charging;
        assert_eq!(summary(&s), "≈85% ⚡");
        s.percent = None;
        s.level = None;
        assert_eq!(summary(&s), "charging");
    }
}
