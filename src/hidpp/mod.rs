//! HID++ transport and the HID++ 2.0 feature-call layer.
//!
//! Packet layout (long report):
//! `[0x11, device_index, feature_index, function << 4 | sw_id, params × 16]`.
//! Errors come back as `[0x11, idx, 0xFF, feature_index, function|sw_id, code, ..]`
//! (HID++ 2.0) or `[0x10, idx, 0x8F, sub_id, address, code, 0]` (HID++ 1.0).

pub mod info;
pub mod receiver;

use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::hidraw::{self, NodeInfo, Reports};

pub const REPORT_SHORT: u8 = 0x10;
pub const REPORT_LONG: u8 = 0x11;

/// Device index of a device attached directly (USB cable, Bluetooth) or of the receiver itself.
pub const DIRECT: u8 = 0xFF;

const TIMEOUT: Duration = Duration::from_millis(2000);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Io(String),
    Timeout,
    /// The hidraw node disappeared.
    Disconnected,
    /// HID++ 1.0 error code (receiver registers, unreachable paired device).
    Hidpp10(u8),
    /// HID++ 2.0 error code returned by a feature call.
    Hidpp20(u8),
    /// The device does not implement this feature ID.
    Unsupported(u16),
    /// Malformed or unexpected data.
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Timeout => write!(f, "device did not answer"),
            Error::Disconnected => write!(f, "device disconnected"),
            Error::Hidpp10(code) => write!(f, "HID++ 1.0 error 0x{code:02x} ({})", hidpp10_error(*code)),
            Error::Hidpp20(code) => write!(f, "HID++ 2.0 error 0x{code:02x} ({})", hidpp20_error(*code)),
            Error::Unsupported(id) => write!(f, "feature 0x{id:04X} not supported"),
            Error::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub fn hidpp10_error(code: u8) -> &'static str {
    match code {
        0x01 => "invalid sub-ID",
        0x02 => "invalid address",
        0x03 => "invalid value",
        0x04 => "connection failed",
        0x05 => "too many devices",
        0x06 => "already exists",
        0x07 => "busy",
        0x08 => "unknown device",
        0x09 => "device unreachable",
        0x0A => "request unavailable",
        0x0B => "invalid parameter",
        0x0C => "wrong PIN",
        _ => "unknown",
    }
}

pub fn hidpp20_error(code: u8) -> &'static str {
    match code {
        0x01 => "unknown",
        0x02 => "invalid argument",
        0x03 => "out of range",
        0x04 => "hardware error",
        0x05 => "Logitech internal",
        0x06 => "invalid feature index",
        0x07 => "invalid function",
        0x08 => "busy",
        0x09 => "unsupported",
        _ => "unknown",
    }
}

/// One open hidraw node speaking HID++. Several [`Device`]s share a transport when
/// they are paired to the same receiver.
pub struct Transport {
    pub path: PathBuf,
    pub reports: Reports,
    writer: Mutex<File>,
    rx: Mutex<Receiver<Vec<u8>>>,
    swid: AtomicU8,
    alive: AtomicBool,
    notifications: Mutex<Vec<Vec<u8>>>,
}

impl Transport {
    pub fn open(info: &NodeInfo) -> io::Result<Arc<Transport>> {
        let node = hidraw::open(&info.devnode)?;
        Ok(Arc::new(Transport {
            path: info.devnode.clone(),
            reports: info.reports,
            writer: Mutex::new(node.writer),
            rx: Mutex::new(node.reports),
            swid: AtomicU8::new(0),
            alive: AtomicBool::new(true),
            notifications: Mutex::new(Vec::new()),
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Software IDs 0x8..=0xF, rotating so a late reply to an earlier request is not
    /// mistaken for the current one. The kernel driver uses 0x1.
    fn next_swid(&self) -> u8 {
        0x8 | (self.swid.fetch_add(1, Ordering::Relaxed) & 0x7)
    }

    /// Write `packet`, then wait for a report that `matcher` accepts. Reports that do
    /// not match are kept as possible notifications (see [`Transport::take_notifications`]).
    pub fn transact(
        &self,
        packet: &[u8],
        timeout: Duration,
        mut matcher: impl FnMut(&[u8]) -> Option<Result<Vec<u8>>>,
    ) -> Result<Vec<u8>> {
        if !self.is_alive() {
            return Err(Error::Disconnected);
        }
        let rx = self.rx.lock().unwrap();
        while let Ok(report) = rx.try_recv() {
            self.stash(report);
        }
        if let Err(e) = self.writer.lock().unwrap().write_all(packet) {
            if e.raw_os_error() == Some(19) {
                // ENODEV
                self.alive.store(false, Ordering::Relaxed);
                return Err(Error::Disconnected);
            }
            return Err(e.into());
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout);
            }
            match rx.recv_timeout(left) {
                Ok(report) => match matcher(&report) {
                    Some(result) => return result,
                    None => self.stash(report),
                },
                Err(RecvTimeoutError::Timeout) => return Err(Error::Timeout),
                Err(RecvTimeoutError::Disconnected) => {
                    self.alive.store(false, Ordering::Relaxed);
                    return Err(Error::Disconnected);
                }
            }
        }
    }

    fn stash(&self, report: Vec<u8>) {
        let mut queue = self.notifications.lock().unwrap();
        if queue.len() >= 64 {
            queue.remove(0);
        }
        queue.push(report);
    }

    /// Unsolicited reports received since the last call (connection changes,
    /// battery events, other programs' traffic).
    pub fn take_notifications(&self) -> Vec<Vec<u8>> {
        if let Ok(rx) = self.rx.try_lock() {
            loop {
                match rx.try_recv() {
                    Ok(report) => self.stash(report),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.alive.store(false, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
        std::mem::take(&mut *self.notifications.lock().unwrap())
    }

    /// Raw HID++ 2.0 call by feature index. Returns the 16 parameter bytes of the reply.
    pub fn call(&self, index: u8, feature_index: u8, function: u8, params: &[u8]) -> Result<[u8; 16]> {
        if params.len() > 16 {
            return Err(Error::Invalid(format!("{} parameter bytes do not fit a long report", params.len())));
        }
        let fs = (function << 4) | self.next_swid();
        let mut packet = [0u8; 20];
        packet[0] = REPORT_LONG;
        packet[1] = index;
        packet[2] = feature_index;
        packet[3] = fs;
        packet[4..4 + params.len()].copy_from_slice(params);
        let short_only = self.reports.short && !self.reports.long;
        let packet: &[u8] = if short_only {
            if params.len() > 3 {
                return Err(Error::Invalid("device only accepts short reports".into()));
            }
            packet[0] = REPORT_SHORT;
            &packet[..7]
        } else {
            &packet
        };
        let reply = self.transact(packet, TIMEOUT, |r| {
            if r.len() < 7 || r[1] != index {
                return None;
            }
            if r[2] == feature_index && r[3] == fs {
                return Some(Ok(r[4..].to_vec()));
            }
            if r[0] == REPORT_LONG && r[2] == 0xFF && r[3] == feature_index && r[4] == fs {
                return Some(Err(Error::Hidpp20(r[5])));
            }
            if r[0] == REPORT_SHORT && r[2] == 0x8F && r[3] == feature_index && r[4] == fs {
                return Some(Err(Error::Hidpp10(r[5])));
            }
            None
        })?;
        let mut out = [0u8; 16];
        let n = reply.len().min(16);
        out[..n].copy_from_slice(&reply[..n]);
        Ok(out)
    }

    /// HID++ 2.0 ping (Root.GetProtocolVersion). Receivers and HID++ 1.0 devices
    /// answer with `Error::Hidpp10(0x01)`.
    pub fn ping(&self, index: u8) -> Result<(u8, u8)> {
        let reply = self.call(index, 0, 1, &[0, 0, 0x5A])?;
        Ok((reply[0], reply[1]))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatureEntry {
    pub id: u16,
    pub index: u8,
    pub version: u8,
    /// bit 7 obsolete, bit 6 hidden (software), bit 5 internal (engineering)
    pub flags: u8,
}

impl FeatureEntry {
    pub fn hidden(&self) -> bool {
        self.flags & 0x60 != 0
    }
}

/// A HID++ 2.0 device reachable at `index` on `transport`.
#[derive(Clone)]
pub struct Device {
    pub transport: Arc<Transport>,
    pub index: u8,
    pub protocol: (u8, u8),
    pub features: Vec<FeatureEntry>,
}

impl fmt::Debug for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Device")
            .field("path", &self.transport.path)
            .field("index", &self.index)
            .field("protocol", &self.protocol)
            .finish()
    }
}

impl Device {
    /// Ping the device and read its feature table.
    pub fn open(transport: Arc<Transport>, index: u8) -> Result<Device> {
        let protocol = transport.ping(index)?;
        if protocol.0 < 2 {
            return Err(Error::Invalid(format!("HID++ {}.{} devices are not supported", protocol.0, protocol.1)));
        }
        let mut dev = Device { transport, index, protocol, features: vec![FeatureEntry { id: 0, index: 0, version: 0, flags: 0 }] };
        let set = dev.call_index(0, 0, &[0x00, 0x01])?;
        let set_index = set[0];
        if set_index == 0 {
            return Err(Error::Unsupported(0x0001));
        }
        let count = dev.call_index(set_index, 0, &[])?[0];
        let mut features = Vec::with_capacity(count as usize + 1);
        for i in 0..=count {
            let r = dev.call_index(set_index, 1, &[i])?;
            features.push(FeatureEntry { id: u16::from_be_bytes([r[0], r[1]]), index: i, version: r[3], flags: r[2] });
        }
        dev.features = features;
        Ok(dev)
    }

    pub fn feature(&self, id: u16) -> Option<&FeatureEntry> {
        self.features.iter().find(|f| f.id == id)
    }

    pub fn has(&self, id: u16) -> bool {
        self.feature(id).is_some()
    }

    pub fn version(&self, id: u16) -> Option<u8> {
        self.feature(id).map(|f| f.version)
    }

    /// Call `function` (0..=15) of feature `id`. Returns the 16 reply parameter bytes.
    pub fn call(&self, id: u16, function: u8, params: &[u8]) -> Result<[u8; 16]> {
        let index = self.feature(id).ok_or(Error::Unsupported(id))?.index;
        self.call_index(index, function, params)
    }

    pub fn call_index(&self, feature_index: u8, function: u8, params: &[u8]) -> Result<[u8; 16]> {
        self.transport.call(self.index, feature_index, function, params)
    }

    pub fn via_receiver(&self) -> bool {
        self.index != DIRECT
    }
}

/// Human-readable names for the feature IDs we know about.
pub fn feature_name(id: u16) -> &'static str {
    match id {
        0x0000 => "Root",
        0x0001 => "Feature set",
        0x0002 => "Feature info",
        0x0003 => "Device information",
        0x0004 => "Unit ID",
        0x0005 => "Device name & type",
        0x0007 => "Device friendly name",
        0x0008 => "Keep alive",
        0x0020 => "Configuration change",
        0x0021 => "Crypto ID",
        0x00C0 => "DFU control (legacy)",
        0x00C1 => "DFU control (unsigned)",
        0x00C2 => "DFU control (signed)",
        0x00C3 => "DFU control",
        0x00D0 => "DFU",
        0x1000 => "Battery status",
        0x1001 => "Battery voltage",
        0x1004 => "Unified battery",
        0x1010 => "Charging control",
        0x1300 => "LED control",
        0x1800 => "Generic test",
        0x1802 => "Device reset",
        0x1803 => "GPIO access",
        0x1805 => "OOB state",
        0x1806 => "Configurable device properties",
        0x1811 => "Unknown 0x1811",
        0x1814 => "Change host",
        0x1815 => "Hosts info",
        0x1821 => "Unknown 0x1821",
        0x1830 => "Power modes",
        0x1890 => "RF test",
        0x1891 => "RF test (BLE)",
        0x18A1 => "LED test",
        0x18B1 => "Unknown 0x18B1",
        0x1981 => "Keyboard backlight (v1)",
        0x1982 => "Keyboard backlight",
        0x1983 => "Keyboard backlight (v3)",
        0x1990 => "Illumination",
        0x1A00 => "Presenter control",
        0x1A01 => "Sensor 3D",
        0x1B00 => "Reprogrammable keys",
        0x1B01 => "Reprogrammable keys v2",
        0x1B02 => "Reprogrammable keys v3",
        0x1B03 => "Reprogrammable keys v4 (old)",
        0x1B04 => "Reprogrammable controls v4",
        0x1BC0 => "Report HID usages",
        0x1C00 => "Persistent remappable action",
        0x1D4B => "Wireless device status",
        0x1DF0 => "Remaining pairings",
        0x1DF3 => "Unknown 0x1DF3",
        0x1E00 => "Enable hidden features",
        0x1E02 => "Device reset (hidden)",
        0x1E22 => "SPI direct access",
        0x1EB0 => "TDE access",
        0x1863 => "Unknown 0x1863",
        0x1801 => "Manufacturing mode",
        0x2001 => "Swap left/right button",
        0x2005 => "Button swap cancel",
        0x2006 => "Pointer axis orientation",
        0x2100 => "Vertical scrolling",
        0x2110 => "SmartShift",
        0x2111 => "SmartShift (enhanced)",
        0x2120 => "High-resolution scrolling",
        0x2121 => "Hi-res wheel",
        0x2130 => "Low-resolution wheel",
        0x2150 => "Thumb wheel",
        0x2200 => "Mouse pointer",
        0x2201 => "Adjustable DPI",
        0x2202 => "Extended adjustable DPI",
        0x2205 => "Pointer motion scaling",
        0x2230 => "Sensor angle snapping",
        0x2240 => "Surface tuning",
        0x2250 => "XY stats",
        0x2251 => "Wheel stats",
        0x2400 => "Hybrid tracking",
        0x40A0 => "Fn inversion",
        0x40A2 => "Fn inversion (new)",
        0x40A3 => "Fn inversion (hot key)",
        0x4100 => "Encryption",
        0x4220 => "Lock key state",
        0x4301 => "Solar dashboard",
        0x4520 => "Keyboard layout",
        0x4521 => "Disable keys",
        0x4522 => "Game mode keys",
        0x4530 => "Dual platform",
        0x4531 => "Multi-platform",
        0x4540 => "Keyboard international layouts",
        0x4600 => "Crown",
        0x6010 => "Touchpad FW items",
        0x6011 => "Touchpad SW items",
        0x6012 => "Touchpad Win8 FW items",
        0x6020 => "Tap enable",
        0x6021 => "Tap enable (extended)",
        0x6030 => "Cursor ballistic",
        0x6040 => "Touchpad resolution",
        0x6100 => "Touchpad raw XY",
        0x6110 => "Touchmouse raw points",
        0x6120 => "Touchmouse 6120",
        0x6500 => "Gesture",
        0x6501 => "Gestures 2",
        0x8010 => "G-keys",
        0x8020 => "M-keys",
        0x8030 => "Macro record",
        0x8040 => "Brightness control",
        0x8060 => "Report rate",
        0x8061 => "Extended report rate",
        0x8070 => "Color LED effects",
        0x8071 => "RGB effects",
        0x8080 => "Per-key lighting",
        0x8081 => "Per-key lighting v2",
        0x8090 => "Mode status",
        0x8100 => "Onboard profiles",
        0x8110 => "Mouse button spy",
        0x8111 => "Latency monitoring",
        0x8120 => "Gaming attachments",
        0x8123 => "Force feedback",
        0x8300 => "Sidetone",
        0x8310 => "Equalizer",
        0x8320 => "Headset out",
        _ => "Unknown",
    }
}
