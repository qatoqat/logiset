//! HID++ 1.0 receiver registers (Unifying, Lightspeed, Nano, Bolt).

use std::time::Duration;

use super::{DIRECT, Error, REPORT_SHORT, Result, Transport};
use crate::hidpp::info::Kind;

const GET_SHORT: u8 = 0x81;
const GET_LONG: u8 = 0x83;

const RECEIVER_INFO: u8 = 0xB5;
const SUB_RECEIVER_INFO: u8 = 0x03;
const SUB_PAIRING: u8 = 0x20;
const SUB_EXTENDED_PAIRING: u8 = 0x30;
const SUB_NAME: u8 = 0x40;
const SUB_BOLT_PAIRING: u8 = 0x50;
const SUB_BOLT_NAME: u8 = 0x60;

const TIMEOUT: Duration = Duration::from_millis(1000);
/// Long register reads can be slow on busy receivers (Solaar allows 1.8 s).
const TIMEOUT_LONG: Duration = Duration::from_millis(1800);

/// Read a register of the receiver (`index` = 0xFF) or of a HID++ 1.0 device.
pub fn read_register(t: &Transport, index: u8, register: u8, long: bool, params: [u8; 3]) -> Result<Vec<u8>> {
    let sub_id = if long { GET_LONG } else { GET_SHORT };
    let packet = [REPORT_SHORT, index, sub_id, register, params[0], params[1], params[2]];
    // Sub-registered reads echo their first parameter; without checking it a late reply to
    // an earlier read of the same register would be taken for this one.
    let echoed = register == RECEIVER_INFO;
    t.transact(&packet, if long { TIMEOUT_LONG } else { TIMEOUT }, |r| {
        if r.len() < 7 || r[1] != index {
            return None;
        }
        if r[2] == sub_id && r[3] == register {
            if echoed && r[4] != params[0] {
                return None;
            }
            return Some(Ok(r[4..].to_vec()));
        }
        if r[2] == 0x8F && r[3] == sub_id && r[4] == register {
            return Some(Err(Error::Hidpp10(r[5])));
        }
        None
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paired {
    pub slot: u8,
    pub wpid: u16,
    pub kind: Kind,
    pub name: Option<String>,
    /// Matches the device's unit ID from feature 0x0003.
    pub serial: Option<u32>,
}

/// Number of pairing slots; 6 when the receiver does not say.
pub fn max_devices(t: &Transport) -> u8 {
    match read_register(t, DIRECT, RECEIVER_INFO, true, [SUB_RECEIVER_INFO, 0, 0]) {
        Ok(r) if r.len() > 6 && (1..=6).contains(&r[6]) => r[6],
        _ => 6,
    }
}

/// Bolt receivers keep pairing data in their own sub-register range.
pub fn is_bolt(product: u16) -> bool {
    product == 0xC548
}

/// What the receiver remembers about pairing slot `slot` (1-based), or `None` when empty.
pub fn pairing_info(t: &Transport, slot: u8, bolt: bool) -> Option<Paired> {
    if bolt {
        return bolt_pairing_info(t, slot);
    }
    let r = read_register(t, DIRECT, RECEIVER_INFO, true, [SUB_PAIRING + slot - 1, 0, 0]).ok()?;
    if r.len() < 8 {
        return None;
    }
    let mut p = Paired { slot, wpid: u16::from_be_bytes([r[3], r[4]]), kind: Kind::from_hidpp10(r[7]), ..Default::default() };
    if let Ok(e) = read_register(t, DIRECT, RECEIVER_INFO, true, [SUB_EXTENDED_PAIRING + slot - 1, 0, 0])
        && e.len() >= 5
    {
        p.serial = Some(u32::from_be_bytes([e[1], e[2], e[3], e[4]]));
    }
    if let Ok(n) = read_register(t, DIRECT, RECEIVER_INFO, true, [SUB_NAME + slot - 1, 0, 0])
        && n.len() > 2
    {
        let len = (n[1] as usize).min(n.len() - 2);
        p.name = Some(String::from_utf8_lossy(&n[2..2 + len]).into_owned()).filter(|s| !s.is_empty());
    }
    Some(p)
}

fn bolt_pairing_info(t: &Transport, slot: u8) -> Option<Paired> {
    let r = read_register(t, DIRECT, RECEIVER_INFO, true, [SUB_BOLT_PAIRING + slot, 0, 0]).ok()?;
    if r.len() < 8 {
        return None;
    }
    let mut p = Paired {
        slot,
        wpid: u16::from_be_bytes([r[3], r[2]]),
        kind: Kind::from_hidpp10(r[1]),
        serial: Some(u32::from_be_bytes([r[4], r[5], r[6], r[7]])),
        ..Default::default()
    };
    if let Ok(n) = read_register(t, DIRECT, RECEIVER_INFO, true, [SUB_BOLT_NAME + slot, 0x01, 0])
        && n.len() > 3
    {
        let len = (n[2] as usize).min(14).min(n.len() - 3);
        p.name = Some(String::from_utf8_lossy(&n[3..3 + len]).into_owned()).filter(|s| !s.is_empty());
    }
    Some(p)
}

/// A device connection change reported by the receiver (HID++ 1.0 sub-ID 0x41).
pub fn connection_event(report: &[u8]) -> Option<(u8, bool)> {
    if report.len() >= 7 && report[0] == REPORT_SHORT && report[2] == 0x41 && (1..=6).contains(&report[1]) {
        let online = report[4] & 0x40 == 0;
        return Some((report[1], online));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_notification() {
        assert_eq!(connection_event(&[0x10, 0x01, 0x41, 0x0c, 0x02, 0x7f, 0x40]), Some((1, true)));
        assert_eq!(connection_event(&[0x10, 0x01, 0x41, 0x0c, 0x42, 0x7f, 0x40]), Some((1, false)));
        assert_eq!(connection_event(&[0x11, 0x01, 0x41, 0, 0, 0, 0]), None);
    }
}
