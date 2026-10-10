//! 0x8100 Onboard Profiles: the mouse's on-board memory, its profiles, button mappings and
//! the active DPI step.
//!
//! Protocol, from Solaar (`lib/logitech_receiver/hidpp20.py`) and cross-checked with
//! libratbag (`src/hidpp20.c`):
//! - fn0 getDescription: `hidpp20.py:1662`, layout in `libratbag/src/hidpp20.h:894`.
//! - fn1 setMode / fn2 getMode (1 = onboard, 2 = host): `hidpp20.py:2114-2123`.
//! - fn3 setCurrentProfile / fn4 getCurrentProfile (sector, big-endian): `settings_templates.py:547-580`.
//! - fn5 memoryRead, fn6 addrWrite, fn7 memoryWrite, fn8 writeEnd: `hidpp20.py:1699-1728`.
//!   Reads never run past the sector end (`libratbag hidpp20.c:1875`).
//! - fn11 / fn12 current DPI index: `libratbag hidpp20.c:2132-2175`.
//! - Sector 0 is the profile directory, 4-byte entries `[sector hi, lo, enabled, 0]` ended
//!   by `FF FF`. An empty directory means the factory (ROM) directory at sector 0x0100.
//! - Every sector ends with CRC-16/CCITT-FALSE over the bytes before it, stored big-endian
//!   (`common.py:303-307`, `hidpp-generic.c:385`).
//! - Profile layout: `hidpp20.py:1532-1620`; button encoding: `hidpp20.py:1404-1530`.
//!
//! Writes keep every byte the model does not expose: a [`Profile`] keeps its sector as read
//! and patches changed fields into a copy of it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::hid_usage;
use crate::hidpp::info::DEVICE_INFO;
use crate::hidpp::{Device, Error, Result};
use crate::util;

/// HID++ 2.0 feature ID.
pub const FEATURE: u16 = 0x8100;

const FN_DESCRIPTION: u8 = 0;
const FN_SET_MODE: u8 = 1;
const FN_GET_MODE: u8 = 2;
const FN_SET_PROFILE: u8 = 3;
const FN_GET_PROFILE: u8 = 4;
const FN_MEM_READ: u8 = 5;
const FN_MEM_ADDR_WRITE: u8 = 6;
const FN_MEM_WRITE: u8 = 7;
const FN_MEM_WRITE_END: u8 = 8;
const FN_GET_DPI: u8 = 11;
const FN_SET_DPI: u8 = 12;

/// Sector of the profile directory the user edits.
const DIRECTORY: u16 = 0x0000;
/// First read-only (factory) sector; the factory directory lives here too.
const ROM_BASE: u16 = 0x0100;
/// DPI steps per profile.
pub const DPI_STEPS: usize = 5;
/// Button slots per profile (buttons and G-shift buttons, 16 each in firmware).
const BUTTON_SLOTS: usize = 16;
/// Smallest sector that holds every profile field plus the CRC.
const PROFILE_MIN_SIZE: usize = 254;
/// Name field: 24 UTF-16LE units.
const NAME_BYTES: usize = 48;
const LED_SIZE: usize = 11;

const OFF_REPORT_RATE: usize = 0;
const OFF_DEFAULT_DPI: usize = 1;
const OFF_SHIFT_DPI: usize = 2;
const OFF_DPI: usize = 3;
const OFF_COLOR: usize = 13;
const OFF_POWER_MODE: usize = 16;
const OFF_ANGLE_SNAP: usize = 17;
const OFF_PS_TIMEOUT: usize = 28;
const OFF_PO_TIMEOUT: usize = 30;
const OFF_BUTTONS: usize = 32;
const OFF_GBUTTONS: usize = 96;
const OFF_NAME: usize = 160;
const OFF_LEDS: usize = 208;

/// Report intervals the firmware accepts, as (milliseconds, label).
pub const REPORT_RATES: &[(u8, &str)] = &[
    (1, "1 ms (1000 Hz)"),
    (2, "2 ms (500 Hz)"),
    (3, "3 ms (333 Hz)"),
    (4, "4 ms (250 Hz)"),
    (5, "5 ms (200 Hz)"),
    (6, "6 ms (167 Hz)"),
    (7, "7 ms (143 Hz)"),
    (8, "8 ms (125 Hz)"),
];

/// Mouse button bitmasks used by [`ButtonAction::Mouse`].
pub const MOUSE_BUTTONS: &[(u16, &str)] = &[
    (0x0001, "Left"),
    (0x0002, "Right"),
    (0x0004, "Middle"),
    (0x0008, "Back"),
    (0x0010, "Forward"),
    (0x0020, "Button 6"),
    (0x0040, "Scroll left"),
    (0x0080, "Scroll right"),
    (0x0100, "Button 9"),
    (0x0200, "Button 10"),
    (0x0400, "Button 11"),
    (0x0800, "Button 12"),
    (0x1000, "Button 13"),
    (0x2000, "DPI"),
    (0x4000, "Button 15"),
];

/// Built-in functions used by [`ButtonAction::Function`], as (code, label).
pub const FUNCTIONS: &[(u8, &str)] = &[
    (0x01, "Tilt left"),
    (0x02, "Tilt right"),
    (0x03, "Next DPI"),
    (0x04, "Previous DPI"),
    (0x05, "Cycle DPI"),
    (0x06, "Default DPI"),
    (0x07, "DPI shift"),
    (0x08, "Next profile"),
    (0x09, "Previous profile"),
    (0x0A, "Cycle profiles"),
    (0x0B, "G-shift"),
    (0x0C, "Battery status"),
    (0x0D, "Select profile"),
    (0x0E, "Switch mode"),
    (0x0F, "Host button"),
    (0x10, "Scroll down"),
    (0x11, "Scroll up"),
];

/// Whether the mouse follows its onboard profiles or the host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Profiles drive DPI, report rate and buttons. Host changes are temporary.
    #[default]
    Onboard,
    /// The host drives the mouse; onboard profiles are ignored.
    Host,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Onboard => "Onboard profiles",
            Mode::Host => "Host control",
        }
    }

    /// Anything but 1 counts as host, as in Solaar and libratbag.
    fn from_byte(b: u8) -> Mode {
        if b == 0x01 { Mode::Onboard } else { Mode::Host }
    }

    fn byte(self) -> u8 {
        match self {
            Mode::Onboard => 0x01,
            Mode::Host => 0x02,
        }
    }
}

/// Memory layout reported by getDescription (fn0). Field names follow libratbag's
/// `hidpp20_onboard_profiles_info`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Description {
    /// Byte 0. Only 1 is supported (the model Solaar and libratbag handle).
    pub memory_model: u8,
    /// Byte 1. Profile layout version; 1..=5 are known.
    pub profile_format: u8,
    /// Byte 2.
    pub macro_format: u8,
    /// Byte 3. User profiles.
    pub profile_count: u8,
    /// Byte 4. Factory profiles.
    pub profile_count_oob: u8,
    /// Byte 5. Buttons per profile.
    pub button_count: u8,
    /// Byte 6.
    pub sector_count: u8,
    /// Bytes 7..9, big-endian.
    pub sector_size: u16,
    /// Byte 9. Bits 0..1 = 2 when G-shift buttons exist; bits 2..3 = 2 when DPI shift exists.
    pub mechanical_layout: u8,
    /// Byte 10. Connection bits: 1 corded, 2 wireless, 4 both.
    pub various_info: u8,
}

impl Description {
    pub fn parse(r: &[u8; 16]) -> Description {
        Description {
            memory_model: r[0],
            profile_format: r[1],
            macro_format: r[2],
            profile_count: r[3],
            profile_count_oob: r[4],
            button_count: r[5],
            sector_count: r[6],
            sector_size: u16::from_be_bytes([r[7], r[8]]),
            mechanical_layout: r[9],
            various_info: r[10],
        }
    }

    /// Buttons per profile, as laid out in the sector (at most 16).
    pub fn button_slots(&self) -> usize {
        usize::from(self.button_count).min(BUTTON_SLOTS)
    }

    /// G-shift buttons per profile: as many as buttons when the mouse has G-shift, else none.
    pub fn g_button_slots(&self) -> usize {
        if self.mechanical_layout & 0x03 == 0x02 { self.button_slots() } else { 0 }
    }

    pub fn has_dpi_shift(&self) -> bool {
        (self.mechanical_layout >> 2) & 0x03 == 0x02
    }
}

/// What one button slot does. Four bytes in the sector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ButtonAction {
    /// `FF FF FF FF`: nothing assigned.
    Unassigned,
    /// `80 00 FF FF`: sends nothing.
    NoAction,
    /// Mouse button bitmask, see [`MOUSE_BUTTONS`].
    Mouse(u16),
    /// Keyboard key with modifier bits (see `hid_usage::MODIFIERS`) and a keyboard-page usage.
    Key { modifiers: u8, usage: u8 },
    /// Consumer-page usage, see `hid_usage::CONSUMER`.
    Consumer(u16),
    /// Built-in function code (see [`FUNCTIONS`]) with its data byte.
    Function { code: u8, data: u8 },
    /// Run a macro (`stop: false`) or stop one, at `sector` (12 bits) and `address`.
    Macro { stop: bool, sector: u16, address: u16 },
    /// A mapping this module does not model. Kept byte for byte.
    Raw([u8; 4]),
}

impl ButtonAction {
    /// Decode the four bytes of a slot.
    pub fn from_bytes(b: [u8; 4]) -> ButtonAction {
        if b == [0xFF; 4] {
            return ButtonAction::Unassigned;
        }
        let word = u16::from_be_bytes([b[2], b[3]]);
        match b[0] >> 4 {
            behavior @ (0x0 | 0x1) => ButtonAction::Macro {
                stop: behavior == 0x1,
                sector: (u16::from(b[0] & 0x0F) << 8) | u16::from(b[1]),
                address: word,
            },
            0x8 => match b[1] {
                0x00 => ButtonAction::NoAction,
                0x01 => ButtonAction::Mouse(word),
                0x02 => ButtonAction::Key { modifiers: b[2], usage: b[3] },
                0x03 => ButtonAction::Consumer(word),
                _ => ButtonAction::Raw(b),
            },
            0x9 => ButtonAction::Function { code: b[1], data: b[3] },
            _ => ButtonAction::Raw(b),
        }
    }

    /// The four bytes to store for this action.
    pub fn encode(&self) -> Result<[u8; 4]> {
        Ok(match *self {
            ButtonAction::Unassigned => [0xFF; 4],
            ButtonAction::NoAction => [0x80, 0x00, 0xFF, 0xFF],
            ButtonAction::Mouse(mask) => {
                let [h, l] = mask.to_be_bytes();
                [0x80, 0x01, h, l]
            }
            ButtonAction::Key { modifiers, usage } => [0x80, 0x02, modifiers, usage],
            ButtonAction::Consumer(usage) => {
                let [h, l] = usage.to_be_bytes();
                [0x80, 0x03, h, l]
            }
            // Byte 2 is 0x00 as the device writes it; Solaar writes 0xFF there instead.
            ButtonAction::Function { code, data } => [0x90, code, 0x00, data],
            ButtonAction::Macro { stop, sector, address } => {
                if sector > 0x0FFF {
                    return Err(Error::Invalid(format!("macro sector 0x{sector:X} does not fit in 12 bits")));
                }
                let [sh, sl] = ((u16::from(stop) << 12) | sector).to_be_bytes();
                let [ah, al] = address.to_be_bytes();
                [sh, sl, ah, al]
            }
            ButtonAction::Raw(b) => b,
        })
    }

    /// Short text for the UI.
    pub fn label(&self) -> String {
        match self {
            ButtonAction::Unassigned => "Not assigned".into(),
            ButtonAction::NoAction => "No action".into(),
            ButtonAction::Mouse(mask) => MOUSE_BUTTONS
                .iter()
                .find(|(m, _)| m == mask)
                .map(|(_, name)| (*name).to_owned())
                .unwrap_or_else(|| format!("Mouse buttons 0x{mask:04X}")),
            ButtonAction::Key { modifiers, usage } => {
                let key = hid_usage::key_name(*usage);
                if *modifiers == 0 { key } else { format!("{}+{key}", hid_usage::modifiers_text(*modifiers)) }
            }
            ButtonAction::Consumer(usage) => hid_usage::consumer_name(*usage),
            ButtonAction::Function { code, .. } => FUNCTIONS
                .iter()
                .find(|(c, _)| c == code)
                .map(|(_, name)| (*name).to_owned())
                .unwrap_or_else(|| format!("Function 0x{code:02X}")),
            ButtonAction::Macro { stop, sector, address } => {
                let verb = if *stop { "Stop macro" } else { "Run macro" };
                format!("{verb} at {sector:03X}:{address:04X}")
            }
            ButtonAction::Raw(b) => format!("Unknown ({:02X} {:02X} {:02X} {:02X})", b[0], b[1], b[2], b[3]),
        }
    }
}

/// One LED effect slot: the effect ID and its ten parameter bytes (the 0x8070 layout).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedEffect {
    pub effect: u8,
    pub params: [u8; 10],
}

/// One onboard profile.
///
/// `raw` is the sector as read. [`Profile::to_bytes`] patches the fields below into a copy of
/// it, so bytes the model does not expose (write counter, reserved bytes, unknown mappings)
/// are written back unchanged, and the CRC is recomputed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// Report interval in milliseconds, see [`REPORT_RATES`].
    pub report_rate: u8,
    /// Index into `dpi` used at start-up (0..=4).
    pub default_dpi: u8,
    /// Index into `dpi` used while the DPI-shift button is held (0..=4).
    pub shift_dpi: u8,
    /// The five DPI steps.
    pub dpi: [u16; DPI_STEPS],
    /// Profile colour (red, green, blue).
    pub color: [u8; 3],
    pub power_mode: u8,
    pub angle_snap: u8,
    /// Timeouts in raw device units.
    pub power_save_timeout: u16,
    pub power_off_timeout: u16,
    /// One mapping per physical button (`Description::button_slots`).
    pub buttons: Vec<ButtonAction>,
    /// G-shift mappings: empty when the mouse has no G-shift.
    pub gbuttons: Vec<ButtonAction>,
    /// Profile name, at most 24 characters.
    pub name: String,
    /// Four LED effect slots.
    pub leds: [LedEffect; 4],
    raw: Vec<u8>,
}

impl Profile {
    /// Parse one profile sector. `buttons` and `gbuttons` are slot counts from the description.
    /// Fails when the sector is too small, a count does not fit, or the CRC does not match.
    pub fn parse(raw: &[u8], buttons: usize, gbuttons: usize) -> Result<Profile> {
        check_sector_size(raw.len())?;
        if buttons > BUTTON_SLOTS || gbuttons > BUTTON_SLOTS {
            return Err(invalid("more buttons than the profile layout holds"));
        }
        if !checksum_ok(raw) {
            return Err(invalid("checksum mismatch"));
        }
        let slots = |base: usize, n: usize| -> Vec<ButtonAction> {
            (0..n).map(|i| ButtonAction::from_bytes(slot(raw, base + 4 * i))).collect()
        };
        Ok(Profile {
            report_rate: raw[OFF_REPORT_RATE],
            default_dpi: raw[OFF_DEFAULT_DPI],
            shift_dpi: raw[OFF_SHIFT_DPI],
            dpi: std::array::from_fn(|i| le16(raw, OFF_DPI + 2 * i)),
            color: [raw[OFF_COLOR], raw[OFF_COLOR + 1], raw[OFF_COLOR + 2]],
            power_mode: raw[OFF_POWER_MODE],
            angle_snap: raw[OFF_ANGLE_SNAP],
            power_save_timeout: le16(raw, OFF_PS_TIMEOUT),
            power_off_timeout: le16(raw, OFF_PO_TIMEOUT),
            buttons: slots(OFF_BUTTONS, buttons),
            gbuttons: slots(OFF_GBUTTONS, gbuttons),
            name: parse_name(&raw[OFF_NAME..OFF_NAME + NAME_BYTES]),
            leds: std::array::from_fn(|i| {
                let at = OFF_LEDS + i * LED_SIZE;
                let mut params = [0u8; 10];
                params.copy_from_slice(&raw[at + 1..at + LED_SIZE]);
                LedEffect { effect: raw[at], params }
            }),
            raw: raw.to_vec(),
        })
    }

    /// The sector image to store: this profile's fields patched into `raw`, CRC recomputed.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = self.raw.clone();
        check_sector_size(out.len())?;
        if !REPORT_RATES.iter().any(|(ms, _)| *ms == self.report_rate) {
            return Err(invalid(format!("report rate {} ms is not supported", self.report_rate)));
        }
        out[OFF_REPORT_RATE] = self.report_rate;
        out[OFF_DEFAULT_DPI] = step(self.default_dpi)?;
        out[OFF_SHIFT_DPI] = step(self.shift_dpi)?;
        for (i, dpi) in self.dpi.iter().enumerate() {
            out[OFF_DPI + 2 * i..OFF_DPI + 2 * i + 2].copy_from_slice(&dpi.to_le_bytes());
        }
        out[OFF_COLOR..OFF_COLOR + 3].copy_from_slice(&self.color);
        out[OFF_POWER_MODE] = self.power_mode;
        out[OFF_ANGLE_SNAP] = self.angle_snap;
        out[OFF_PS_TIMEOUT..OFF_PS_TIMEOUT + 2].copy_from_slice(&self.power_save_timeout.to_le_bytes());
        out[OFF_PO_TIMEOUT..OFF_PO_TIMEOUT + 2].copy_from_slice(&self.power_off_timeout.to_le_bytes());
        patch_buttons(&mut out, OFF_BUTTONS, &self.buttons)?;
        patch_buttons(&mut out, OFF_GBUTTONS, &self.gbuttons)?;
        if parse_name(&out[OFF_NAME..OFF_NAME + NAME_BYTES]) != self.name {
            out[OFF_NAME..OFF_NAME + NAME_BYTES].copy_from_slice(&encode_name(&self.name)?);
        }
        for (i, led) in self.leds.iter().enumerate() {
            let at = OFF_LEDS + i * LED_SIZE;
            out[at] = led.effect;
            out[at + 1..at + LED_SIZE].copy_from_slice(&led.params);
        }
        let n = out.len();
        let crc = crc16(&out[..n - 2]);
        out[n - 2..].copy_from_slice(&crc.to_be_bytes());
        Ok(out)
    }
}

/// A profile directory entry with the profile it points to.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileEntry {
    /// Sector holding the profile. Sectors from 0x0100 are factory ROM and read-only.
    pub sector: u16,
    /// Listed as enabled in the directory.
    pub enabled: bool,
    /// The parsed profile, or why it could not be read (read error, bad checksum).
    pub profile: std::result::Result<Profile, String>,
}

/// What the device reports for onboard profiles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub description: Description,
    pub mode: Mode,
    /// Sector of the active profile.
    pub current_profile: u16,
    /// Active DPI step (0..=4), when the device reports one.
    pub dpi_index: Option<u8>,
    /// The profile directory is the factory one: no user profile is stored, and nothing can be written.
    pub rom: bool,
    /// Unit ID as eight hex digits, or "unknown". Used in backup file names.
    pub device_tag: String,
    /// Profiles in directory order.
    pub profiles: Vec<ProfileEntry>,
}

impl State {
    /// The profile at `current_profile`, when it is listed.
    pub fn active(&self) -> Option<&ProfileEntry> {
        self.profiles.iter().find(|p| p.sector == self.current_profile)
    }
}

/// A change the UI asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// Switch between onboard profiles and host control. Volatile: replayed on reconnect.
    SetMode(Mode),
    /// Make the profile at this sector the active one. Also switches to onboard mode.
    SetActiveProfile(u16),
    /// Set the active DPI step (0..=4).
    SetDpiIndex(u8),
    /// Store `profile` in a user profile sector.
    WriteProfile { sector: u16, profile: Profile },
    /// Enable or disable a user profile by rewriting the profile directory.
    SetProfileEnabled { sector: u16, enabled: bool },
    /// Write a sector image saved in a backup. The checksum must match.
    RestoreSector { sector: u16, bytes: Vec<u8> },
}

impl Action {
    pub fn persist_key(&self) -> Option<String> {
        match self {
            Action::SetMode(_) => Some("onboard.mode".into()),
            _ => None,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Action::SetMode(Mode::Onboard) => "onboard profiles".into(),
            Action::SetMode(Mode::Host) => "host control".into(),
            Action::SetActiveProfile(sector) => format!("active profile 0x{sector:04X}"),
            Action::SetDpiIndex(index) => format!("DPI index {index}"),
            Action::WriteProfile { profile, .. } => format!("save profile \"{}\"", profile.name),
            Action::SetProfileEnabled { sector, enabled } => {
                let verb = if *enabled { "enable" } else { "disable" };
                format!("{verb} profile 0x{sector:04X}")
            }
            Action::RestoreSector { sector, .. } => format!("restore sector 0x{sector:04X}"),
        }
    }
}

/// Read the slice's state. Read-only: no write functions are called.
pub fn probe(dev: &Device) -> Option<Result<State>> {
    dev.has(FEATURE).then(|| read_state(dev))
}

/// Perform `action` on the device and verify it. Then update `state` to match the device.
///
/// Sector writes first save the current contents to the backup directory, then read the
/// sector back and fail when it does not match what was written.
pub fn apply(dev: &Device, state: &mut State, action: &Action) -> Result<()> {
    match action {
        Action::SetMode(mode) => {
            set_mode(dev, *mode)?;
            state.mode = read_mode(dev)?;
        }
        Action::SetActiveProfile(sector) => {
            let idx = state
                .profiles
                .iter()
                .position(|p| p.sector == *sector)
                .ok_or_else(|| invalid(format!("sector 0x{sector:04X} is not a listed profile")))?;
            if !state.profiles[idx].enabled {
                return Err(invalid("profile is disabled"));
            }
            set_mode(dev, Mode::Onboard)?;
            dev.call(FEATURE, FN_SET_PROFILE, &sector.to_be_bytes())?;
            state.mode = read_mode(dev)?;
            state.current_profile = read_profile(dev)?;
        }
        Action::SetDpiIndex(index) => {
            let index = step(*index)?;
            dev.call(FEATURE, FN_SET_DPI, &[index])?;
            state.dpi_index = Some(read_dpi(dev)?);
        }
        Action::WriteProfile { sector, profile } => {
            let idx = user_entry(state, *sector)?;
            check_layout(profile, &state.description)?;
            let (size, buttons, gbuttons) = layout(state);
            let image = update_sector(dev, &state.device_tag, *sector, size, |_| profile.to_bytes())?;
            state.profiles[idx].profile = Profile::parse(&image, buttons, gbuttons).map_err(|e| e.to_string());
        }
        Action::SetProfileEnabled { sector, enabled } => {
            if state.rom {
                return Err(invalid("profiles are in ROM and cannot be changed"));
            }
            if !enabled && *sector == state.current_profile {
                return Err(invalid("select another profile before disabling the active one"));
            }
            let idx = user_entry(state, *sector)?;
            let (size, _, _) = layout(state);
            update_sector(dev, &state.device_tag, DIRECTORY, size, |dir| {
                if !checksum_ok(dir) {
                    return Err(invalid("profile directory checksum mismatch; not rewriting it"));
                }
                with_directory_enabled(dir, *sector, *enabled)
            })?;
            state.profiles[idx].enabled = *enabled;
        }
        Action::RestoreSector { sector, bytes } => {
            let idx = user_entry(state, *sector)?;
            let (size, buttons, gbuttons) = layout(state);
            if bytes.len() != size || !checksum_ok(bytes) {
                return Err(invalid("backup does not match this device's profile size or checksum"));
            }
            let image = update_sector(dev, &state.device_tag, *sector, size, |_| Ok(bytes.clone()))?;
            state.profiles[idx].profile = Profile::parse(&image, buttons, gbuttons).map_err(|e| e.to_string());
        }
    }
    Ok(())
}

/// True when the device runs from onboard profiles (host-side changes are temporary).
pub fn is_onboard_mode(state: &State) -> bool {
    state.mode == Mode::Onboard
}

/// One-line summary for `logiset list` (the full state includes raw memory).
pub fn summary(state: &State) -> String {
    let active = match state.active().map(|p| &p.profile) {
        Some(Ok(profile)) => format!("\"{}\"", profile.name),
        _ => format!("sector 0x{:04X}", state.current_profile),
    };
    let rom = if state.rom { " (factory ROM)" } else { "" };
    format!("{}; active profile {active}; {} profiles{rom}", state.mode.label(), state.profiles.len())
}

/// Read one whole sector of `size` bytes. Reads are 16 bytes each and never run past the end.
pub fn read_sector(dev: &Device, sector: u16, size: usize) -> Result<Vec<u8>> {
    if !(16..=usize::from(u16::MAX)).contains(&size) {
        return Err(invalid(format!("sector size {size} is out of range")));
    }
    let mut out = vec![0u8; size];
    let [sh, sl] = sector.to_be_bytes();
    for at in read_offsets(size) {
        let [oh, ol] = (at as u16).to_be_bytes();
        let reply = dev.call(FEATURE, FN_MEM_READ, &[sh, sl, oh, ol])?;
        out[at..at + 16].copy_from_slice(&reply);
    }
    Ok(out)
}

/// Write `bytes` to a sector: addressed write, then 16-byte blocks, then the end marker.
/// The end marker is sent only after every block succeeded. This does not verify the result;
/// [`update_sector`] does.
pub fn write_sector(dev: &Device, sector: u16, bytes: &[u8]) -> Result<()> {
    let len = u16::try_from(bytes.len()).map_err(|_| invalid("sector is too large to write"))?;
    let [sh, sl] = sector.to_be_bytes();
    let [lh, ll] = len.to_be_bytes();
    dev.call(FEATURE, FN_MEM_ADDR_WRITE, &[sh, sl, 0, 0, lh, ll])?;
    for block in write_blocks(bytes) {
        dev.call(FEATURE, FN_MEM_WRITE, &block)?;
    }
    dev.call(FEATURE, FN_MEM_WRITE_END, &[])?;
    Ok(())
}

/// CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF, no reflection, no final XOR).
pub fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0xFFFFu16, |crc, &b| {
        let mut crc = crc ^ (u16::from(b) << 8);
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
        crc
    })
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn slot(b: &[u8], at: usize) -> [u8; 4] {
    [b[at], b[at + 1], b[at + 2], b[at + 3]]
}

fn check_sector_size(len: usize) -> Result<()> {
    if len < PROFILE_MIN_SIZE || len > usize::from(u16::MAX) {
        return Err(invalid(format!("sector size {len} cannot hold a profile")));
    }
    Ok(())
}

fn checksum_ok(sector: &[u8]) -> bool {
    let n = sector.len();
    n >= 2 && crc16(&sector[..n - 2]) == u16::from_be_bytes([sector[n - 2], sector[n - 1]])
}

fn step(index: u8) -> Result<u8> {
    if usize::from(index) < DPI_STEPS { Ok(index) } else { Err(invalid(format!("DPI index {index} is out of range"))) }
}

/// Offsets of the 16-byte reads that cover `size` bytes. The last read is moved back to
/// `size - 16`, because the firmware refuses reads that run past the sector end.
fn read_offsets(size: usize) -> impl Iterator<Item = usize> {
    (0..size).step_by(16).map(move |o| o.min(size - 16))
}

/// The 16-byte memoryWrite payloads for `bytes`. The last one is padded with 0xFF.
fn write_blocks(bytes: &[u8]) -> Vec<[u8; 16]> {
    bytes
        .chunks(16)
        .map(|c| {
            let mut block = [0xFF; 16];
            block[..c.len()].copy_from_slice(c);
            block
        })
        .collect()
}

fn parse_name(bytes: &[u8]) -> String {
    let mut units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    while units.last().is_some_and(|&u| u == 0 || u == 0xFFFF) {
        units.pop();
    }
    String::from_utf16_lossy(&units)
}

fn encode_name(name: &str) -> Result<[u8; NAME_BYTES]> {
    let units: Vec<u16> = name.encode_utf16().collect();
    if units.len() > NAME_BYTES / 2 || units.iter().any(|&u| u == 0 || u == 0xFFFF) {
        return Err(invalid(format!("name \"{name}\" must be at most 24 characters and contain no NUL")));
    }
    let mut out = [0u8; NAME_BYTES];
    for (i, u) in units.iter().enumerate() {
        out[2 * i..2 * i + 2].copy_from_slice(&u.to_le_bytes());
    }
    Ok(out)
}

/// Write each button mapping whose decoded value changed. Unchanged slots keep their bytes.
fn patch_buttons(out: &mut [u8], base: usize, actions: &[ButtonAction]) -> Result<()> {
    if actions.len() > BUTTON_SLOTS {
        return Err(invalid("more buttons than the profile layout holds"));
    }
    for (i, action) in actions.iter().enumerate() {
        let at = base + 4 * i;
        if ButtonAction::from_bytes(slot(out, at)) != *action {
            out[at..at + 4].copy_from_slice(&action.encode()?);
        }
    }
    Ok(())
}

/// Profile directory entries in order: (sector, enabled). Stops at the `FF FF` end marker.
fn directory_entries(dir: &[u8]) -> Vec<(u16, bool)> {
    let body = &dir[..dir.len().saturating_sub(2)];
    body.as_chunks::<4>()
        .0
        .iter()
        .map(|e| (u16::from_be_bytes([e[0], e[1]]), e[2] != 0))
        .take_while(|(sector, _)| *sector != 0xFFFF)
        .collect()
}

/// An empty user directory starts with all zeros or all ones; the factory one is used instead.
fn directory_is_empty(dir: &[u8]) -> bool {
    dir[..4] == [0, 0, 0, 0] || dir[..4] == [0xFF; 4]
}

/// The directory image with `sector` marked enabled or disabled, CRC recomputed.
fn with_directory_enabled(dir: &[u8], sector: u16, enabled: bool) -> Result<Vec<u8>> {
    let index = directory_entries(dir)
        .iter()
        .position(|(s, _)| *s == sector)
        .ok_or_else(|| invalid(format!("sector 0x{sector:04X} is not in the profile directory")))?;
    let mut out = dir.to_vec();
    out[index * 4 + 2] = u8::from(enabled);
    let n = out.len();
    let crc = crc16(&out[..n - 2]);
    out[n - 2..].copy_from_slice(&crc.to_be_bytes());
    Ok(out)
}

/// Index of a user profile in `state.profiles`. Refuses the directory and factory sectors.
fn user_entry(state: &State, sector: u16) -> Result<usize> {
    if sector == DIRECTORY {
        return Err(invalid("sector 0 is the profile directory; use enable or disable instead"));
    }
    if sector >= ROM_BASE {
        return Err(invalid(format!("sector 0x{sector:04X} is factory ROM and cannot be changed")));
    }
    state
        .profiles
        .iter()
        .position(|p| p.sector == sector)
        .ok_or_else(|| invalid(format!("sector 0x{sector:04X} is not a listed profile")))
}

/// Sector size, buttons and G-shift buttons per profile, from the description.
fn layout(state: &State) -> (usize, usize, usize) {
    let d = &state.description;
    (usize::from(d.sector_size), d.button_slots(), d.g_button_slots())
}

fn check_layout(profile: &Profile, d: &Description) -> Result<()> {
    if profile.buttons.len() != d.button_slots() || profile.gbuttons.len() != d.g_button_slots() {
        return Err(invalid("profile does not match this device's button layout"));
    }
    Ok(())
}

/// Read `sector`, let `change` build the image to store from it, and write only when that
/// differs. Before writing, the current contents are saved to a backup file. After writing,
/// the sector is read back and must match. Returns the image now stored.
fn update_sector(
    dev: &Device,
    tag: &str,
    sector: u16,
    size: usize,
    change: impl FnOnce(&[u8]) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let current = read_sector(dev, sector, size)?;
    let image = change(&current)?;
    if image.len() != size {
        return Err(invalid(format!("new image for sector 0x{sector:04X} is {} bytes, expected {size}", image.len())));
    }
    if image == current {
        return Ok(image);
    }
    let backup = backup_sector(tag, sector, &current)?;
    match write_sector(dev, sector, &image).and_then(|()| read_sector(dev, sector, size)) {
        Ok(stored) if stored == image => Ok(image),
        Ok(_) => Err(invalid(format!(
            "sector 0x{sector:04X} did not read back as written; its previous contents are in {}",
            backup.display()
        ))),
        Err(e) => Err(invalid(format!(
            "sector 0x{sector:04X} could not be written and verified ({e}); its previous contents are in {}",
            backup.display()
        ))),
    }
}

/// Save `bytes` as a new file in the backup directory. An existing backup is never overwritten.
fn backup_sector(tag: &str, sector: u16, bytes: &[u8]) -> Result<PathBuf> {
    let dir = util::backup_dir();
    let stamp = util::timestamp();
    let mut path = dir.join(format!("{tag}-sector{sector:04x}-{stamp}.bin"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{tag}-sector{sector:04x}-{stamp}-{n}.bin"));
        n += 1;
    }
    util::write_atomic(&path, bytes)
        .map_err(|e| invalid(format!("could not save a backup to {}: {e}", path.display())))?;
    Ok(path)
}

fn read_mode(dev: &Device) -> Result<Mode> {
    Ok(Mode::from_byte(dev.call(FEATURE, FN_GET_MODE, &[])?[0]))
}

fn set_mode(dev: &Device, mode: Mode) -> Result<()> {
    dev.call(FEATURE, FN_SET_MODE, &[mode.byte()])?;
    Ok(())
}

fn read_profile(dev: &Device) -> Result<u16> {
    let r = dev.call(FEATURE, FN_GET_PROFILE, &[])?;
    Ok(u16::from_be_bytes([r[0], r[1]]))
}

fn read_dpi(dev: &Device) -> Result<u8> {
    Ok(dev.call(FEATURE, FN_GET_DPI, &[])?[0])
}

fn read_description(dev: &Device) -> Result<Description> {
    Ok(Description::parse(&dev.call(FEATURE, FN_DESCRIPTION, &[])?))
}

/// The unit ID from feature 0x0003 as eight hex digits, or "unknown".
fn device_tag(dev: &Device) -> String {
    match dev.call(DEVICE_INFO, 0, &[]) {
        Ok(r) => format!("{:08x}", u32::from_be_bytes([r[1], r[2], r[3], r[4]])),
        Err(_) => "unknown".into(),
    }
}

fn read_state(dev: &Device) -> Result<State> {
    let description = read_description(dev)?;
    if description.memory_model != 0x01 || !(1..=5).contains(&description.profile_format) {
        return Err(invalid(format!(
            "unsupported onboard profile layout (memory model {}, profile format {})",
            description.memory_model, description.profile_format
        )));
    }
    let size = usize::from(description.sector_size);
    check_sector_size(size)?;
    let mode = read_mode(dev)?;
    let current_profile = read_profile(dev)?;
    let dpi_index = read_dpi(dev).ok();
    let user_dir = read_sector(dev, DIRECTORY, size)?;
    let rom = directory_is_empty(&user_dir);
    let dir = if rom { read_sector(dev, ROM_BASE, size)? } else { user_dir };
    let (buttons, gbuttons) = (description.button_slots(), description.g_button_slots());
    let profiles = directory_entries(&dir)
        .into_iter()
        .map(|(sector, enabled)| {
            let profile = read_sector(dev, sector, size)
                .and_then(|raw| Profile::parse(&raw, buttons, gbuttons))
                .map_err(|e| e.to_string());
            ProfileEntry { sector, enabled, profile }
        })
        .collect();
    Ok(State { description, mode, current_profile, dpi_index, rom, device_tag: device_tag(dev), profiles })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fn0 reply on the G502 LIGHTSPEED.
    const DESCRIPTION: [u8; 16] = [0x01, 0x03, 0x01, 0x05, 0x01, 0x0b, 0x10, 0x00, 0xff, 0x0a, 0x04, 0, 0, 0, 0, 0];

    /// First 32 bytes of directory sector 0.
    const DIRECTORY_HEAD: [u8; 32] = [
        0x00, 0x01, 0x01, 0x00, // profile 1 at sector 0x0001, enabled
        0x00, 0x02, 0x00, 0x00, // profile 2 at sector 0x0002, disabled
        0x00, 0x03, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0xff, 0xff, 0x00,
        0x00, // end marker
        0xff, 0xff, 0xff, 0xff, // unused
        0xff, 0xff, 0xff, 0xff,
    ];

    /// Profile sector 1 (255 bytes, CRC 0x8A50).
    const PROFILE_HEX: &str = "01030090012003b00440060807ffffffff00ffffffffffffffffffffffffffff8001000180010002800100048001000880010010900700009004000090030000900c00009002000090010000ffffffffffffffffffffffffffffffffffffffff8001000180010002800100048001000880010010900700009004000090030000900c00009002000090010000ffffffffffffffffffffffffffffffffffffffff500052004f00460049004c0045005f004e0041004d0045005f00440045004600410055004c005400000000000000000000000000000000000000000000000000000000000000ffffffffffffffffffffffffffffffffffffffffffff008a50";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn fixture() -> Vec<u8> {
        hex(PROFILE_HEX)
    }

    /// Directory sector image with a valid CRC.
    fn directory_image() -> Vec<u8> {
        let mut dir = vec![0xFF; 255];
        dir[..32].copy_from_slice(&DIRECTORY_HEAD);
        seal(&mut dir);
        dir
    }

    fn seal(sector: &mut [u8]) {
        let n = sector.len();
        let crc = crc16(&sector[..n - 2]);
        sector[n - 2..].copy_from_slice(&crc.to_be_bytes());
    }

    fn mouse(mask: u16) -> ButtonAction {
        ButtonAction::Mouse(mask)
    }

    fn function(code: u8) -> ButtonAction {
        ButtonAction::Function { code, data: 0 }
    }

    fn expected_buttons() -> Vec<ButtonAction> {
        vec![
            mouse(0x0001),
            mouse(0x0002),
            mouse(0x0004),
            mouse(0x0008),
            mouse(0x0010),
            function(0x07),
            function(0x04),
            function(0x03),
            function(0x0C),
            function(0x02),
            function(0x01),
        ]
    }

    #[test]
    fn description_fields() {
        let d = Description::parse(&DESCRIPTION);
        assert_eq!(d.memory_model, 1);
        assert_eq!(d.profile_format, 3);
        assert_eq!(d.profile_count, 5);
        assert_eq!(d.profile_count_oob, 1);
        assert_eq!(d.sector_count, 16);
        assert_eq!(d.sector_size, 255);
        assert_eq!(d.button_slots(), 11);
        assert_eq!(d.g_button_slots(), 11);
        assert!(d.has_dpi_shift());
    }

    #[test]
    fn mode_bytes() {
        assert_eq!(Mode::from_byte(0x01), Mode::Onboard);
        assert_eq!(Mode::from_byte(0x02), Mode::Host);
        assert_eq!(Mode::from_byte(0x00), Mode::Host);
        assert_eq!(Mode::Host.byte(), 0x02);
    }

    #[test]
    fn crc_check_value() {
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }

    #[test]
    fn fixture_checksum_is_valid() {
        let raw = fixture();
        assert_eq!(raw.len(), 255);
        assert!(checksum_ok(&raw));
        assert_eq!(u16::from_be_bytes([raw[253], raw[254]]), 0x8A50);
    }

    #[test]
    fn directory_fixture_lists_five_profiles() {
        let entries = directory_entries(&directory_image());
        assert_eq!(entries, vec![(1, true), (2, false), (3, false), (4, false), (5, false)]);
        assert!(!directory_is_empty(&directory_image()));
        assert!(directory_is_empty(&[0; 255]));
        assert!(directory_is_empty(&[0xFF; 255]));
    }

    #[test]
    fn directory_enable_rewrites_one_flag_and_crc() {
        let dir = directory_image();
        let on = with_directory_enabled(&dir, 3, true).unwrap();
        assert!(checksum_ok(&on));
        assert_eq!(directory_entries(&on)[2], (3, true));
        let changed: Vec<usize> = (0..dir.len()).filter(|&i| dir[i] != on[i]).collect();
        assert!(changed.contains(&(2 * 4 + 2)));
        assert!(changed.iter().all(|&i| i == 2 * 4 + 2 || i >= 253));
        let off = with_directory_enabled(&on, 1, false).unwrap();
        assert_eq!(directory_entries(&off)[0], (1, false));
        assert!(with_directory_enabled(&dir, 9, true).is_err());
    }

    #[test]
    fn profile_parse_fixture() {
        let p = Profile::parse(&fixture(), 11, 11).unwrap();
        assert_eq!(p.report_rate, 1);
        assert_eq!(p.default_dpi, 3);
        assert_eq!(p.shift_dpi, 0);
        assert_eq!(p.dpi, [400, 800, 1200, 1600, 1800]);
        assert_eq!(p.color, [0xFF, 0xFF, 0xFF]);
        assert_eq!(p.name, "PROFILE_NAME_DEFAULT");
        assert_eq!(p.buttons, expected_buttons());
        assert_eq!(p.gbuttons, expected_buttons());
        assert_eq!(p.leds[0], LedEffect::default());
        assert_eq!(p.leds[1], LedEffect::default());
        assert_eq!(p.leds[2], LedEffect { effect: 0xFF, params: [0xFF; 10] });
        assert_eq!(p.leds[3], LedEffect { effect: 0xFF, params: [0xFF; 10] });
    }

    #[test]
    fn profile_round_trip_is_byte_identical() {
        let raw = fixture();
        let p = Profile::parse(&raw, 11, 11).unwrap();
        assert_eq!(p.to_bytes().unwrap(), raw);
    }

    #[test]
    fn profile_edit_changes_only_its_fields() {
        let raw = fixture();
        let mut edited = Profile::parse(&raw, 11, 11).unwrap();
        edited.dpi[0] = 1600;
        edited.name = "MY PROFILE".into();
        let out = edited.to_bytes().unwrap();
        assert!(checksum_ok(&out));

        let changed: Vec<usize> = (0..raw.len()).filter(|&i| raw[i] != out[i]).collect();
        assert!(
            changed.iter().all(|&i| (3..5).contains(&i) || (OFF_NAME..OFF_NAME + NAME_BYTES).contains(&i) || i >= 253)
        );

        let reparsed = Profile::parse(&out, 11, 11).unwrap();
        assert_eq!(reparsed.dpi[0], 1600);
        assert_eq!(reparsed.name, "MY PROFILE");
        assert_eq!(reparsed.buttons, edited.buttons);
        let mut expected = edited.clone();
        expected.raw = out;
        assert_eq!(reparsed, expected);
    }

    #[test]
    fn button_edit_round_trips() {
        let mut p = Profile::parse(&fixture(), 11, 11).unwrap();
        p.buttons[0] = ButtonAction::Consumer(0x00E9);
        p.buttons[5] = ButtonAction::Key { modifiers: 0x01, usage: 0x1A };
        p.gbuttons[10] = ButtonAction::Macro { stop: false, sector: 0x123, address: 0x4567 };
        let out = p.to_bytes().unwrap();
        assert!(checksum_ok(&out));
        let again = Profile::parse(&out, 11, 11).unwrap();
        assert_eq!(again.buttons, p.buttons);
        assert_eq!(again.gbuttons, p.gbuttons);
        assert_eq!(&out[32..36], &[0x80, 0x03, 0x00, 0xE9]);
        assert_eq!(&out[36..40], &[0x80, 0x01, 0x00, 0x02], "untouched slots keep their bytes");
    }

    #[test]
    fn button_codecs() {
        let cases: &[([u8; 4], ButtonAction)] = &[
            ([0x80, 0x01, 0x00, 0x08], ButtonAction::Mouse(0x0008)),
            ([0x80, 0x02, 0x01, 0x1A], ButtonAction::Key { modifiers: 0x01, usage: 0x1A }),
            ([0x80, 0x03, 0x00, 0xE9], ButtonAction::Consumer(0x00E9)),
            ([0x90, 0x07, 0x00, 0x00], ButtonAction::Function { code: 0x07, data: 0 }),
            ([0x90, 0x0C, 0x00, 0x02], ButtonAction::Function { code: 0x0C, data: 2 }),
            ([0x01, 0x23, 0x45, 0x67], ButtonAction::Macro { stop: false, sector: 0x123, address: 0x4567 }),
            ([0x11, 0x23, 0x45, 0x67], ButtonAction::Macro { stop: true, sector: 0x123, address: 0x4567 }),
            ([0x80, 0x00, 0xFF, 0xFF], ButtonAction::NoAction),
            ([0xFF, 0xFF, 0xFF, 0xFF], ButtonAction::Unassigned),
            ([0x20, 0x00, 0x00, 0x00], ButtonAction::Raw([0x20, 0x00, 0x00, 0x00])),
            ([0x80, 0x09, 0x00, 0x00], ButtonAction::Raw([0x80, 0x09, 0x00, 0x00])),
        ];
        for (bytes, action) in cases {
            assert_eq!(ButtonAction::from_bytes(*bytes), *action, "decode {bytes:02X?}");
            assert_eq!(action.encode().unwrap(), *bytes, "encode {action:?}");
        }
        assert_eq!(ButtonAction::from_bytes([0x80, 0x00, 0x00, 0x00]), ButtonAction::NoAction);
        assert_eq!(ButtonAction::Mouse(0x0001).label(), "Left");
        assert_eq!(ButtonAction::Function { code: 0x07, data: 0 }.label(), "DPI shift");
    }

    #[test]
    fn name_codec() {
        assert_eq!(parse_name(&[0xFF; NAME_BYTES]), "");
        assert_eq!(parse_name(&encode_name("ABC").unwrap()), "ABC");
        assert!(encode_name(&"x".repeat(25)).is_err());
        assert!(encode_name("a\0b").is_err());
    }

    #[test]
    fn invalid_edits_are_refused() {
        let base = Profile::parse(&fixture(), 11, 11).unwrap();
        let mut p = base.clone();
        p.default_dpi = 5;
        assert!(p.to_bytes().is_err());
        let mut p = base.clone();
        p.report_rate = 9;
        assert!(p.to_bytes().is_err());
        let mut p = base.clone();
        p.buttons[0] = ButtonAction::Macro { stop: false, sector: 0x1000, address: 0 };
        assert!(p.to_bytes().is_err());
    }

    #[test]
    fn parse_rejects_short_or_corrupt_sectors() {
        let raw = fixture();
        assert!(Profile::parse(&raw[..200], 11, 11).is_err());
        let mut bad = raw.clone();
        bad[40] ^= 0x01;
        assert!(Profile::parse(&bad, 11, 11).is_err());
        assert!(Profile::parse(&raw, 17, 11).is_err());
    }

    #[test]
    fn read_plan_covers_sector_without_overrun() {
        let offsets: Vec<usize> = read_offsets(255).collect();
        assert_eq!(offsets.len(), 16);
        assert_eq!(offsets[0], 0);
        assert_eq!(*offsets.last().unwrap(), 239);
        assert!(offsets.iter().all(|&o| o + 16 <= 255));
        let mut seen = [false; 255];
        for o in offsets {
            seen[o..o + 16].iter_mut().for_each(|s| *s = true);
        }
        assert!(seen.iter().all(|&s| s));
        assert_eq!(read_offsets(256).last(), Some(240));
    }

    #[test]
    fn write_plan_pads_last_block() {
        let bytes = fixture();
        let blocks = write_blocks(&bytes);
        assert_eq!(blocks.len(), 16);
        assert_eq!(&blocks[15][..15], &bytes[240..255]);
        assert_eq!(blocks[15][15], 0xFF);
        assert_eq!(&blocks[0][..], &bytes[..16]);
    }

    #[test]
    fn action_persistence_and_serde() {
        assert_eq!(Action::SetMode(Mode::Host).persist_key().as_deref(), Some("onboard.mode"));
        assert_eq!(Action::SetDpiIndex(2).persist_key(), None);
        let action = Action::WriteProfile { sector: 1, profile: Profile::parse(&fixture(), 11, 11).unwrap() };
        let json = serde_json::to_string(&action).unwrap();
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(back, action);
        assert_eq!(Action::SetProfileEnabled { sector: 2, enabled: false }.describe(), "disable profile 0x0002");
    }
}
