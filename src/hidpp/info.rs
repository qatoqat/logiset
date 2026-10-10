//! Identity features: 0x0003 Device Information and 0x0005 Device Name & Type.

use super::{Device, Result};

pub const DEVICE_INFO: u16 = 0x0003;
pub const DEVICE_NAME: u16 = 0x0005;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    Keyboard,
    Mouse,
    Trackpad,
    Trackball,
    Numpad,
    Presenter,
    Remote,
    Receiver,
    Headset,
    #[default]
    Other,
}

impl Kind {
    /// 0x0005 getDeviceType value.
    pub fn from_hidpp20(t: u8) -> Kind {
        match t {
            0 => Kind::Keyboard,
            1 => Kind::Remote,
            2 => Kind::Numpad,
            3 => Kind::Mouse,
            4 => Kind::Trackpad,
            5 => Kind::Trackball,
            6 => Kind::Presenter,
            7 => Kind::Receiver,
            8 => Kind::Headset,
            _ => Kind::Other,
        }
    }

    /// Kind byte of the receiver's pairing register (0xB5 0x2n, byte 7 low nibble).
    pub fn from_hidpp10(t: u8) -> Kind {
        match t & 0x0F {
            1 => Kind::Keyboard,
            2 => Kind::Mouse,
            3 => Kind::Numpad,
            4 => Kind::Presenter,
            7 => Kind::Remote,
            8 => Kind::Trackball,
            9 => Kind::Trackpad,
            _ => Kind::Other,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Kind::Keyboard | Kind::Numpad => "⌨",
            Kind::Mouse | Kind::Trackball | Kind::Trackpad => "🖱",
            Kind::Receiver => "📡",
            Kind::Headset => "🎧",
            _ => "🔌",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Keyboard => "Keyboard",
            Kind::Mouse => "Mouse",
            Kind::Trackpad => "Trackpad",
            Kind::Trackball => "Trackball",
            Kind::Numpad => "Numpad",
            Kind::Presenter => "Presenter",
            Kind::Remote => "Remote",
            Kind::Receiver => "Receiver",
            Kind::Headset => "Headset",
            Kind::Other => "Device",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Firmware {
    pub kind: &'static str,
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: Option<String>,
    pub kind: Kind,
    pub unit_id: Option<u32>,
    /// Model IDs from 0x0003 (wireless PID, USB PID, Bluetooth PID; zero when unused).
    pub model_ids: Vec<u16>,
    pub firmware: Vec<Firmware>,
    pub serial: Option<String>,
}

impl Identity {
    /// Stable key for saved settings: same value whether the device is cabled or paired.
    pub fn config_key(&self, fallback: &str) -> String {
        match self.unit_id {
            Some(unit) => {
                let models: Vec<String> = self.model_ids.iter().map(|m| format!("{m:04x}")).collect();
                format!("{}-{unit:08x}", models.join(""))
            }
            None => fallback.to_owned(),
        }
    }
}

pub fn identify(dev: &Device) -> Identity {
    let mut id = Identity::default();
    if dev.has(DEVICE_NAME) {
        id.name = read_name(dev).ok().filter(|n| !n.is_empty());
        if let Ok(r) = dev.call(DEVICE_NAME, 2, &[]) {
            id.kind = Kind::from_hidpp20(r[0]);
        }
    }
    if dev.has(DEVICE_INFO) {
        if let Ok(r) = dev.call(DEVICE_INFO, 0, &[]) {
            let entities = r[0];
            id.unit_id = Some(u32::from_be_bytes([r[1], r[2], r[3], r[4]]));
            id.model_ids = (0..3).map(|i| u16::from_be_bytes([r[7 + i * 2], r[8 + i * 2]])).collect();
            let capabilities = r[14];
            for e in 0..entities {
                if let Ok(fw) = dev.call(DEVICE_INFO, 1, &[e]) {
                    id.firmware.push(parse_firmware(&fw));
                }
            }
            if capabilities & 0x01 != 0 && dev.version(DEVICE_INFO).unwrap_or(0) >= 4 {
                if let Ok(s) = dev.call(DEVICE_INFO, 2, &[]) {
                    let serial: String = s[..12].iter().take_while(|&&b| b != 0).map(|&b| b as char).collect();
                    if !serial.is_empty() {
                        id.serial = Some(serial);
                    }
                }
            }
        }
    }
    id
}

fn read_name(dev: &Device) -> Result<String> {
    let len = dev.call(DEVICE_NAME, 0, &[])?[0] as usize;
    let mut name = Vec::with_capacity(len);
    while name.len() < len {
        let chunk = dev.call(DEVICE_NAME, 1, &[name.len() as u8])?;
        let take = (len - name.len()).min(16);
        let part: Vec<u8> = chunk[..take].iter().copied().take_while(|&b| b != 0).collect();
        let short = part.len() < take;
        name.extend(part);
        if short {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&name).trim().to_owned())
}

pub fn parse_firmware(r: &[u8; 16]) -> Firmware {
    let kind = match r[0] & 0x0F {
        0 => "Firmware",
        1 => "Bootloader",
        2 => "Hardware",
        3 => "Touchpad",
        4 => "Optical sensor",
        5 => "Softdevice",
        6 => "RF companion",
        7 => "Factory app",
        8 => "RGB effects",
        9 => "Motor drive",
        _ => "Other",
    };
    if r[0] & 0x0F == 2 {
        return Firmware { kind, name: String::new(), version: r[1].to_string() };
    }
    let name: String = r[1..4].iter().filter(|b| b.is_ascii_graphic()).map(|&b| b as char).collect();
    let build = u16::from_be_bytes([r[6], r[7]]);
    let mut version = format!("{:02X}.{:02X}", r[4], r[5]);
    if build != 0 {
        version.push_str(&format!(".B{build:04X}"));
    }
    Firmware { kind, name, version }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_strings() {
        // G502 LIGHTSPEED main firmware as reported by the device
        let fw = [0x00, 0x4d, 0x50, 0x4d, 0x17, 0x00, 0x00, 0x08, 0x01, 0xc0, 0x8d, 0x21, 0xf1, 0xfa, 0x5f, 0x00];
        let f = parse_firmware(&fw);
        assert_eq!((f.kind, f.name.as_str(), f.version.as_str()), ("Firmware", "MPM", "17.00.B0008"));
    }
}
