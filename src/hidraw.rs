//! Linux hidraw access: find Logitech nodes in sysfs and read their reports on a
//! background thread so requests can wait with a timeout using only std.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TrySendError};
use std::thread;

pub const LOGITECH: u16 = 0x046d;

pub const BUS_BLUETOOTH: u16 = 0x05;

/// HID++ report IDs a node declares in its report descriptor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reports {
    pub short: bool,
    pub long: bool,
    pub very_long: bool,
}

impl Reports {
    pub fn any(&self) -> bool {
        self.short || self.long || self.very_long
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    /// `/dev/hidrawN`
    pub devnode: PathBuf,
    pub bus: u16,
    pub vendor: u16,
    pub product: u16,
    pub name: String,
    pub reports: Reports,
    /// The node is a device paired behind a receiver (a hid-logitech-dj child).
    /// Those are reached through the receiver's own node instead.
    pub behind_receiver: bool,
}

/// All Logitech hidraw nodes, ordered by node number.
pub fn enumerate() -> Vec<NodeInfo> {
    let mut nodes = Vec::new();
    let Ok(dir) = fs::read_dir("/sys/class/hidraw") else {
        return nodes;
    };
    for entry in dir.flatten() {
        let node = entry.file_name().to_string_lossy().into_owned();
        let sys = entry.path().join("device");
        let Ok(uevent) = fs::read_to_string(sys.join("uevent")) else {
            continue;
        };
        let mut id = None;
        let mut name = String::new();
        for line in uevent.lines() {
            if let Some(v) = line.strip_prefix("HID_ID=") {
                id = parse_hid_id(v);
            } else if let Some(v) = line.strip_prefix("HID_NAME=") {
                name = v.to_owned();
            }
        }
        let Some((bus, vendor, product)) = id else {
            continue;
        };
        if vendor != LOGITECH {
            continue;
        }
        let descriptor = fs::read(sys.join("report_descriptor")).unwrap_or_default();
        // A DJ child sits directly below another HID device (the receiver interface)
        // rather than below a USB interface or Bluetooth link.
        let behind_receiver = fs::canonicalize(&sys)
            .ok()
            .and_then(|p| p.parent().map(|parent| parent.join("report_descriptor").exists()))
            .unwrap_or(false);
        nodes.push(NodeInfo {
            devnode: Path::new("/dev").join(&node),
            bus,
            vendor,
            product,
            name,
            reports: hidpp_reports(&descriptor),
            behind_receiver,
        });
    }
    nodes.sort_by_key(|n| node_number(&n.devnode));
    nodes
}

fn node_number(path: &Path) -> u32 {
    path.to_string_lossy()
        .trim_start_matches("/dev/hidraw")
        .parse()
        .unwrap_or(u32::MAX)
}

/// `0003:0000046D:0000C338` -> (bus, vendor, product)
fn parse_hid_id(s: &str) -> Option<(u16, u16, u16)> {
    let mut parts = s.split(':').map(|p| u32::from_str_radix(p, 16).ok());
    let bus = parts.next()??;
    let vendor = parts.next()??;
    let product = parts.next()??;
    Some((bus as u16, vendor as u16, product as u16))
}

/// Walk the short items of a report descriptor and note which HID++ report IDs
/// (0x10 short, 0x11 long, 0x12 very long) appear under a vendor usage page.
pub fn hidpp_reports(desc: &[u8]) -> Reports {
    let mut reports = Reports::default();
    let mut page = 0u32;
    let mut i = 0;
    while i < desc.len() {
        let prefix = desc[i];
        if prefix == 0xFE {
            // Long item: 0xFE, size, tag, data
            let size = desc.get(i + 1).copied().unwrap_or(0) as usize;
            i += 3 + size;
            continue;
        }
        let size = match prefix & 0x03 {
            3 => 4,
            n => n as usize,
        };
        let Some(data) = desc.get(i + 1..i + 1 + size) else {
            break;
        };
        let value = data.iter().rev().fold(0u32, |acc, &b| (acc << 8) | b as u32);
        match prefix & 0xFC {
            0x04 => page = value,
            0x84 if page >= 0xFF00 => match value {
                0x10 => reports.short = true,
                0x11 => reports.long = true,
                0x12 => reports.very_long = true,
                _ => {}
            },
            _ => {}
        }
        i += 1 + size;
    }
    reports
}

/// An open hidraw node. Input reports with a HID++ report ID are forwarded from a
/// reader thread; when the node goes away the channel disconnects.
pub struct Node {
    pub writer: File,
    pub reports: Receiver<Vec<u8>>,
}

pub fn open(path: &Path) -> io::Result<Node> {
    let writer = OpenOptions::new().read(true).write(true).open(path)?;
    let mut reader = writer.try_clone()?;
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(256);
    let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    thread::Builder::new()
        .name(format!("read-{label}"))
        .spawn(move || {
            let mut buf = [0u8; 64];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if !matches!(buf[0], 0x10..=0x12) {
                            continue;
                        }
                        if let Err(TrySendError::Disconnected(_)) = tx.try_send(buf[..n].to_vec()) {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })?;
    Ok(Node { writer, reports: rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn mouse_hidpp_interface() {
        // G502 LIGHTSPEED interface 2
        let d = hex("0600ff0901a101851095067508150026ff000901810009019100c00600ff0902a101851195137508150026ff000902810009029100c0");
        assert_eq!(hidpp_reports(&d), Reports { short: true, long: true, very_long: false });
    }

    #[test]
    fn keyboard_boot_interface_has_no_hidpp() {
        let d = hex("05010906a101050719e029e71500250175019508810295017508810195037501050819012903910295057501910195067508150026ff00050719002aff008100c0");
        assert!(!hidpp_reports(&d).any());
    }

    #[test]
    fn hid_id() {
        assert_eq!(parse_hid_id("0003:0000046D:0000C338"), Some((3, 0x046d, 0xc338)));
    }
}
