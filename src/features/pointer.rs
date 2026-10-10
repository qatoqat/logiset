//! Pointer settings: 0x2201 Adjustable DPI, 0x8060 Report Rate and 0x8061 Extended Report Rate.

use serde::{Deserialize, Serialize};

use crate::hidpp::{Device, Error, Result};

pub const ADJUSTABLE_DPI: u16 = 0x2201;
pub const REPORT_RATE: u16 = 0x8060;
pub const EXTENDED_REPORT_RATE: u16 = 0x8061;

/// Pages of the DPI list read before giving up (each page holds seven values).
const MAX_DPI_PAGES: usize = 16;

/// Hz for each index of the extended report-rate table (0 = 8 ms ... 6 = 125 us).
const EXTENDED_HZ: [u32; 7] = [125, 250, 500, 1000, 2000, 4000, 8000];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Empty when the device has no 0x2201 feature.
    pub sensors: Vec<Sensor>,
    pub report_rate: Option<ReportRate>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sensor {
    pub index: u8,
    pub current: u16,
    /// Factory default, when the device reports one.
    pub default: Option<u16>,
    pub choices: DpiChoices,
}

/// DPI values a sensor accepts: discrete values plus arithmetic ranges.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DpiChoices {
    pub discrete: Vec<u16>,
    pub ranges: Vec<DpiRange>,
}

/// `min`, `min + step`, ... up to `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpiRange {
    pub min: u16,
    pub max: u16,
    pub step: u16,
}

impl DpiRange {
    fn contains(&self, dpi: u16) -> bool {
        (self.min..=self.max).contains(&dpi) && (dpi - self.min).is_multiple_of(self.step)
    }

    /// The grid value closest to `dpi`; a tie goes to the lower value.
    fn nearest(&self, dpi: u16) -> u16 {
        if dpi <= self.min {
            return self.min;
        }
        if dpi >= self.max {
            return self.max;
        }
        let (offset, step) = (u32::from(dpi - self.min), u32::from(self.step));
        let rem = offset % step;
        let lower = offset - rem;
        let snapped = if rem * 2 <= step { lower } else { lower + step };
        (u32::from(self.min) + snapped).min(u32::from(self.max)) as u16
    }
}

impl DpiChoices {
    pub fn contains(&self, dpi: u16) -> bool {
        self.discrete.contains(&dpi) || self.ranges.iter().any(|r| r.contains(dpi))
    }

    /// The only thing the sensor offers, when it is one range with no discrete values.
    pub fn single_range(&self) -> Option<DpiRange> {
        match (self.discrete.as_slice(), self.ranges.as_slice()) {
            ([], [range]) => Some(*range),
            _ => None,
        }
    }

    /// Move each discrete value that sits one step beyond a range into that range, so a
    /// lone `100` before `150..=25600` step 50 becomes `100..=25600` step 50.
    fn fold_into_ranges(mut self) -> Self {
        for range in &mut self.ranges {
            while let Some(i) = self.discrete.iter().position(|&v| v.checked_add(range.step) == Some(range.min)) {
                range.min = self.discrete.remove(i);
            }
            while let Some(i) = self.discrete.iter().position(|&v| range.max.checked_add(range.step) == Some(v)) {
                range.max = self.discrete.remove(i);
            }
        }
        self
    }

    pub fn min(&self) -> Option<u16> {
        self.discrete.iter().copied().chain(self.ranges.iter().map(|r| r.min)).min()
    }

    pub fn max(&self) -> Option<u16> {
        self.discrete.iter().copied().chain(self.ranges.iter().map(|r| r.max)).max()
    }

    /// The supported value closest to `dpi`; ties go to the lower value.
    pub fn nearest(&self, dpi: u16) -> Option<u16> {
        self.discrete
            .iter()
            .copied()
            .chain(self.ranges.iter().map(|r| r.nearest(dpi)))
            .min_by_key(|&v| (v.abs_diff(dpi), v))
    }
}

/// Which feature reports the rate. The raw values differ: milliseconds for `Basic`,
/// a table index for `Extended`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateFeature {
    /// 0x8060
    Basic,
    /// 0x8061
    Extended,
}

impl RateFeature {
    fn id(self) -> u16 {
        match self {
            RateFeature::Basic => REPORT_RATE,
            RateFeature::Extended => EXTENDED_REPORT_RATE,
        }
    }

    /// Function numbers: (list, get, set).
    fn functions(self) -> (u8, u8, u8) {
        match self {
            RateFeature::Basic => (0, 1, 2),
            RateFeature::Extended => (1, 2, 3),
        }
    }

    /// Rate in Hz for a raw value, when the value is known.
    pub fn hz(self, raw: u8) -> Option<u32> {
        match self {
            RateFeature::Basic => (raw != 0).then(|| 1000 / u32::from(raw)),
            RateFeature::Extended => EXTENDED_HZ.get(usize::from(raw)).copied(),
        }
    }

    /// Text for a raw value, such as "1000 Hz".
    pub fn label(self, raw: u8) -> String {
        match (self.hz(raw), self) {
            (Some(hz), _) => format!("{hz} Hz"),
            (None, RateFeature::Basic) => format!("{raw} ms"),
            (None, RateFeature::Extended) => format!("index {raw}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReportRate {
    pub feature: RateFeature,
    /// Raw value the device reports: milliseconds or table index, per `feature`.
    pub current: u8,
    /// Raw values the device supports, slowest rate first.
    pub supported: Vec<u8>,
}

impl ReportRate {
    /// The action that selects the raw value `raw`.
    pub fn set_action(&self, raw: u8) -> Action {
        match self.feature {
            RateFeature::Basic => Action::SetReportRate { ms: raw },
            RateFeature::Extended => Action::SetExtendedReportRate { index: raw },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
// The variant names are the saved-settings vocabulary; keep the shared `Set` prefix.
#[allow(clippy::enum_variant_names)]
pub enum Action {
    /// Set a sensor's DPI. The value must be one the sensor accepts.
    SetDpi { sensor: u8, dpi: u16 },
    /// Set the report rate through 0x8060 (milliseconds).
    SetReportRate { ms: u8 },
    /// Set the report rate through 0x8061 (table index).
    SetExtendedReportRate { index: u8 },
}

impl Action {
    pub fn persist_key(&self) -> Option<String> {
        match self {
            Action::SetDpi { sensor, .. } => Some(format!("pointer.dpi.{sensor}")),
            Action::SetReportRate { .. } | Action::SetExtendedReportRate { .. } => Some("pointer.rate".into()),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Action::SetDpi { sensor: 0, dpi } => format!("DPI {dpi}"),
            Action::SetDpi { sensor, dpi } => format!("DPI {dpi} (sensor {sensor})"),
            Action::SetReportRate { ms } => format!("Report rate {}", RateFeature::Basic.label(*ms)),
            Action::SetExtendedReportRate { index } => format!("Report rate {}", RateFeature::Extended.label(*index)),
        }
    }
}

/// READ-ONLY. `None` when the device has none of the three features.
pub fn probe(dev: &Device) -> Option<Result<State>> {
    let rate = if dev.has(EXTENDED_REPORT_RATE) {
        Some(RateFeature::Extended)
    } else if dev.has(REPORT_RATE) {
        Some(RateFeature::Basic)
    } else {
        None
    };
    if !dev.has(ADJUSTABLE_DPI) && rate.is_none() {
        return None;
    }
    Some(read(dev, rate))
}

fn read(dev: &Device, rate: Option<RateFeature>) -> Result<State> {
    let sensors = if dev.has(ADJUSTABLE_DPI) { read_sensors(dev)? } else { Vec::new() };
    let report_rate = match rate {
        Some(feature) => read_rate(dev, feature)?,
        None => None,
    };
    Ok(State { sensors, report_rate })
}

/// Perform `action`, then update `state` from the device. Invalid values are refused
/// before anything is sent.
pub fn apply(dev: &Device, state: &mut State, action: &Action) -> Result<()> {
    match *action {
        Action::SetDpi { sensor, dpi } => set_dpi(dev, state, sensor, dpi),
        Action::SetReportRate { ms } => set_rate(dev, state, RateFeature::Basic, ms),
        Action::SetExtendedReportRate { index } => set_rate(dev, state, RateFeature::Extended, index),
    }
}

fn read_sensors(dev: &Device) -> Result<Vec<Sensor>> {
    let count = dev.call(ADJUSTABLE_DPI, 0, &[])?[0];
    (0..count).map(|index| read_sensor(dev, index)).collect()
}

fn read_sensor(dev: &Device, index: u8) -> Result<Sensor> {
    let choices = read_dpi_list(dev, index)?;
    let (current, default) = read_dpi(dev, index)?;
    Ok(Sensor { index, current, default, choices })
}

/// getSensorDpiList: the first page holds the sensor echo and seven values. A list
/// longer than that continues on further pages, requested with a page number in byte 2.
fn read_dpi_list(dev: &Device, sensor: u8) -> Result<DpiChoices> {
    let mut pages = vec![dev.call(ADJUSTABLE_DPI, 1, &[sensor])?];
    loop {
        let (choices, ended) = parse_dpi_pages(&pages)?;
        if ended || pages.len() >= MAX_DPI_PAGES {
            return Ok(choices);
        }
        let page = u8::try_from(pages.len()).unwrap_or(u8::MAX);
        let next = match dev.call(ADJUSTABLE_DPI, 1, &[sensor, 0, page]) {
            Ok(next) => next,
            // The device rejected the continuation: keep the values read so far.
            Err(Error::Hidpp20(_)) => return Ok(choices),
            Err(e) => return Err(e),
        };
        if pages.last() == Some(&next) {
            // The device ignored the page number.
            return Ok(choices);
        }
        pages.push(next);
    }
}

/// Walk the DPI list pages. Returns the choices read and whether the terminator was seen.
/// Discrete values beside a range are folded into it.
fn parse_dpi_pages(pages: &[[u8; 16]]) -> Result<(DpiChoices, bool)> {
    let mut choices = DpiChoices::default();
    let mut last: Option<u16> = None;
    let mut pending_step: Option<u16> = None;
    for page in pages {
        for k in 0..7 {
            let v = u16::from_be_bytes([page[1 + 2 * k], page[2 + 2 * k]]);
            if let Some(step) = pending_step.take() {
                let base = last.ok_or_else(|| Error::Invalid("DPI range without a start".into()))?;
                if step == 0 || v <= base {
                    return Err(Error::Invalid(format!("bad DPI range {base}..{v} step {step}")));
                }
                let (base, step, end) = (u32::from(base), u32::from(step), u32::from(v));
                let max = base + (end - base) / step * step;
                if max >= base + step {
                    choices.ranges.push(DpiRange { min: (base + step) as u16, max: max as u16, step: step as u16 });
                    last = Some(max as u16);
                }
                continue;
            }
            match v {
                0 => return Ok((choices.fold_into_ranges(), true)),
                _ if v >> 13 == 0b111 => pending_step = Some(v & 0x1FFF),
                _ => {
                    choices.discrete.push(v);
                    last = Some(v);
                }
            }
        }
    }
    Ok((choices.fold_into_ranges(), false))
}

/// getSensorDpi reply: sensor echo, current DPI, default DPI (0 when not provided).
/// A current value of 0 falls back to the default, as Solaar does.
fn parse_dpi_reading(reply: &[u8; 16]) -> Result<(u16, Option<u16>)> {
    let current = u16::from_be_bytes([reply[1], reply[2]]);
    let default = u16::from_be_bytes([reply[3], reply[4]]);
    let current = if current == 0 { default } else { current };
    if current == 0 {
        return Err(Error::Invalid("the sensor reports no DPI".into()));
    }
    Ok((current, (default != 0).then_some(default)))
}

fn read_dpi(dev: &Device, sensor: u8) -> Result<(u16, Option<u16>)> {
    parse_dpi_reading(&dev.call(ADJUSTABLE_DPI, 2, &[sensor])?)
}

/// Parameters of setSensorDpi: sensor index, then the DPI big-endian.
fn set_dpi_params(sensor: u8, dpi: u16) -> [u8; 3] {
    let [hi, lo] = dpi.to_be_bytes();
    [sensor, hi, lo]
}

fn set_dpi(dev: &Device, state: &mut State, sensor: u8, dpi: u16) -> Result<()> {
    let s = state
        .sensors
        .iter_mut()
        .find(|s| s.index == sensor)
        .ok_or_else(|| Error::Invalid(format!("no sensor {sensor}")))?;
    if !s.choices.contains(dpi) {
        return Err(Error::Invalid(format!("{dpi} DPI is not supported by this sensor")));
    }
    dev.call(ADJUSTABLE_DPI, 3, &set_dpi_params(sensor, dpi))?;
    (s.current, s.default) = read_dpi(dev, sensor)?;
    Ok(())
}

fn read_rate(dev: &Device, feature: RateFeature) -> Result<Option<ReportRate>> {
    let (list, get, _) = feature.functions();
    let mask = dev.call(feature.id(), list, &[])?;
    let supported = parse_rate_list(feature, &mask);
    if supported.is_empty() {
        return Ok(None);
    }
    let current = dev.call(feature.id(), get, &[])?[0];
    Ok(Some(ReportRate { feature, current, supported }))
}

/// Supported raw values from a list reply, ordered by rate (slowest first).
/// Basic: bit n set means n+1 ms. Extended: u16 big-endian, bit n set means table index n.
fn parse_rate_list(feature: RateFeature, reply: &[u8; 16]) -> Vec<u8> {
    let mut supported: Vec<u8> = match feature {
        RateFeature::Basic => (0..8u8).filter(|n| (reply[0] >> n) & 1 == 1).map(|n| n + 1).collect(),
        RateFeature::Extended => {
            let mask = u16::from_be_bytes([reply[0], reply[1]]);
            (0..EXTENDED_HZ.len() as u8).filter(|n| (mask >> n) & 1 == 1).collect()
        }
    };
    supported.sort_by_key(|&raw| feature.hz(raw).unwrap_or(0));
    supported
}

fn set_rate(dev: &Device, state: &mut State, feature: RateFeature, raw: u8) -> Result<()> {
    let rate = match state.report_rate.as_mut() {
        Some(rate) if rate.feature == feature => rate,
        _ => return Err(Error::Invalid("this device does not take that report rate".into())),
    };
    if !rate.supported.contains(&raw) {
        return Err(Error::Invalid(format!("{} is not supported", feature.label(raw))));
    }
    let (_, get, set) = feature.functions();
    dev.call(feature.id(), set, &[raw])?;
    rate.current = dev.call(feature.id(), get, &[])?[0];
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0x2201 fn1 reply for sensor 0 on a G502 LIGHTSPEED: 100, then 150..=25600 in steps of 50.
    const G502_DPI_LIST: [u8; 16] = [0x00, 0x00, 0x64, 0xE0, 0x32, 0x64, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0];

    fn page(sensor: u8, values: &[u16]) -> [u8; 16] {
        let mut p = [0u8; 16];
        p[0] = sensor;
        for (k, v) in values.iter().enumerate() {
            p[1 + 2 * k..3 + 2 * k].copy_from_slice(&v.to_be_bytes());
        }
        p
    }

    fn g502_choices() -> DpiChoices {
        parse_dpi_pages(&[G502_DPI_LIST]).unwrap().0
    }

    #[test]
    fn parses_g502_dpi_list() {
        let (choices, ended) = parse_dpi_pages(&[G502_DPI_LIST]).unwrap();
        assert!(ended);
        assert!(choices.discrete.is_empty(), "100 folds into the range");
        assert_eq!(choices.ranges, vec![DpiRange { min: 100, max: 25600, step: 50 }]);
        assert_eq!(choices.single_range(), Some(DpiRange { min: 100, max: 25600, step: 50 }));
        assert_eq!(choices.min(), Some(100));
        assert_eq!(choices.max(), Some(25600));
    }

    #[test]
    fn folds_values_one_step_beyond_a_range() {
        // 800, then 1000..=2000 step 200 (800 folds in below), then 2200 (folds in above).
        let p = page(0, &[800, 0xE000 | 200, 2000, 2200]);
        let (c, ended) = parse_dpi_pages(&[p]).unwrap();
        assert!(ended);
        assert!(c.discrete.is_empty());
        assert_eq!(c.single_range(), Some(DpiRange { min: 800, max: 2200, step: 200 }));
    }

    #[test]
    fn keeps_values_further_from_a_range() {
        // 400 is two steps below the range that starts at 800, so it stays discrete.
        let p = page(0, &[400, 800, 0xE000 | 200, 2000]);
        let (c, ended) = parse_dpi_pages(&[p]).unwrap();
        assert!(ended);
        assert_eq!(c.discrete, vec![400]);
        assert_eq!(c.ranges, vec![DpiRange { min: 800, max: 2000, step: 200 }]);
        assert_eq!(c.single_range(), None);
    }

    #[test]
    fn discrete_only_list_has_no_single_range() {
        let p = page(0, &[400, 800, 1600]);
        let (c, _) = parse_dpi_pages(&[p]).unwrap();
        assert_eq!(c.discrete, vec![400, 800, 1600]);
        assert_eq!(c.single_range(), None);
    }

    #[test]
    fn dpi_membership() {
        let c = g502_choices();
        assert!(c.contains(100));
        assert!(c.contains(150));
        assert!(c.contains(1600));
        assert!(c.contains(25600));
        assert!(!c.contains(125));
        assert!(!c.contains(25650));
        assert!(!c.contains(0));
    }

    #[test]
    fn dpi_nearest() {
        let c = g502_choices();
        assert_eq!(c.nearest(125), Some(100), "tie goes to the lower value");
        assert_eq!(c.nearest(149), Some(150));
        assert_eq!(c.nearest(1620), Some(1600));
        assert_eq!(c.nearest(1625), Some(1600), "tie goes to the lower value");
        assert_eq!(c.nearest(1626), Some(1650));
        assert_eq!(c.nearest(60000), Some(25600));
        assert_eq!(DpiChoices::default().nearest(800), None);
    }

    #[test]
    fn range_end_is_grid_aligned() {
        let p = page(0, &[100, 0xE000 | 30, 1010]);
        let (c, ended) = parse_dpi_pages(&[p]).unwrap();
        assert!(ended);
        // 100 is one step below 130, so it folds in.
        assert_eq!(c.ranges, vec![DpiRange { min: 100, max: 1000, step: 30 }]);
        assert!(c.discrete.is_empty());
        assert!(!c.contains(1010));
    }

    #[test]
    fn list_continues_on_next_page() {
        let first = page(0, &[400, 800, 1600, 3200, 5000, 6400, 8000]);
        let second = page(0, &[10000]);
        let (partial, ended) = parse_dpi_pages(&[first]).unwrap();
        assert!(!ended);
        assert_eq!(partial.discrete.len(), 7);
        let (c, ended) = parse_dpi_pages(&[first, second]).unwrap();
        assert!(ended);
        assert_eq!(c.discrete, vec![400, 800, 1600, 3200, 5000, 6400, 8000, 10000]);
    }

    #[test]
    fn range_without_start_is_invalid() {
        let p = page(0, &[0xE000 | 50, 25600]);
        assert!(matches!(parse_dpi_pages(&[p]), Err(Error::Invalid(_))));
    }

    #[test]
    fn parses_g502_dpi_reading() {
        let reply = [0x00, 0x01, 0x90, 0x03, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(parse_dpi_reading(&reply).unwrap(), (400, Some(800)));
    }

    #[test]
    fn dpi_reading_without_default() {
        let reply = [0x00, 0x00, 0xC8, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(parse_dpi_reading(&reply).unwrap(), (200, None));
    }

    #[test]
    fn dpi_reading_zero_current_uses_default() {
        let reply = [0x00, 0x00, 0x00, 0x03, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(parse_dpi_reading(&reply).unwrap(), (800, Some(800)));
    }

    #[test]
    fn set_dpi_encodes_big_endian() {
        assert_eq!(set_dpi_params(0, 1600), [0, 0x06, 0x40]);
        assert_eq!(set_dpi_params(1, 25600), [1, 0x64, 0x00]);
    }

    #[test]
    fn parses_basic_report_rate_list() {
        // G502 and G610: 0x8b = bits 0, 1, 3, 7 = 1, 2, 4, 8 ms.
        let mut reply = [0u8; 16];
        reply[0] = 0x8B;
        assert_eq!(parse_rate_list(RateFeature::Basic, &reply), vec![8, 4, 2, 1]);
        assert_eq!(RateFeature::Basic.label(1), "1000 Hz");
        assert_eq!(RateFeature::Basic.label(8), "125 Hz");
    }

    #[test]
    fn parses_extended_report_rate_list() {
        // Synthetic: indexes 3 and 6 (1000 Hz and 8000 Hz) are supported.
        let mut reply = [0u8; 16];
        reply[1] = 0b0100_1000;
        assert_eq!(parse_rate_list(RateFeature::Extended, &reply), vec![3, 6]);
        assert_eq!(RateFeature::Extended.label(6), "8000 Hz");
    }

    #[test]
    fn action_text_and_persist_keys() {
        assert_eq!(Action::SetDpi { sensor: 0, dpi: 1600 }.describe(), "DPI 1600");
        assert_eq!(Action::SetDpi { sensor: 1, dpi: 1600 }.describe(), "DPI 1600 (sensor 1)");
        assert_eq!(Action::SetReportRate { ms: 1 }.describe(), "Report rate 1000 Hz");
        assert_eq!(Action::SetExtendedReportRate { index: 0 }.describe(), "Report rate 125 Hz");
        assert_eq!(Action::SetDpi { sensor: 0, dpi: 800 }.persist_key().as_deref(), Some("pointer.dpi.0"));
        assert_eq!(Action::SetReportRate { ms: 2 }.persist_key().as_deref(), Some("pointer.rate"));
        assert_eq!(Action::SetExtendedReportRate { index: 3 }.persist_key().as_deref(), Some("pointer.rate"));
    }

    #[test]
    fn report_rate_set_action() {
        let rate = ReportRate { feature: RateFeature::Basic, current: 1, supported: vec![8, 4, 2, 1] };
        assert_eq!(rate.set_action(2), Action::SetReportRate { ms: 2 });
        let rate = ReportRate { feature: RateFeature::Extended, current: 3, supported: vec![3, 6] };
        assert_eq!(rate.set_action(6), Action::SetExtendedReportRate { index: 6 });
    }
}
