//! Keyboard settings: game-mode key blocking (0x4522), disabled keys (0x4521),
//! F1-F12 behaviour (0x40A0 / 0x40A2 / 0x40A3) and the layout code (0x4540, read-only).

use serde::{Deserialize, Serialize};

use crate::hid_usage;
use crate::hidpp::{Device, Error, Result};

const GAME_MODE: u16 = 0x4522;
const DISABLE_KEYS: u16 = 0x4521;
const FN_STANDARD: u16 = 0x40A0;
const FN_NEW: u16 = 0x40A2;
const FN_K375S: u16 = 0x40A3;
const HOSTS_INFO: u16 = 0x1815;
const LAYOUT: u16 = 0x4540;

/// Keys per block call.
const BLOCK_BATCH: usize = 16;

/// Bits of the disable-keys mask, as in Solaar's `special_keys.DISABLE`.
pub const DISABLE_BITS: &[(u8, &str)] = &[(0x01, "Caps Lock"), (0x02, "Num Lock"), (0x04, "Scroll Lock"), (0x08, "Insert"), (0x10, "Win/Super")];
const DISABLE_MASK: u8 = 0x1F;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnVariant {
    /// 0x40A0
    Standard,
    /// 0x40A2
    New,
    /// 0x40A3, keyed by host slot
    K375s,
}

impl FnVariant {
    fn feature(self) -> u16 {
        match self {
            FnVariant::Standard => FN_STANDARD,
            FnVariant::New => FN_NEW,
            FnVariant::K375s => FN_K375S,
        }
    }

    /// The Fn feature this keyboard has, newest first when several exist.
    fn present(dev: &Device) -> Option<FnVariant> {
        [FnVariant::New, FnVariant::Standard, FnVariant::K375s].into_iter().find(|v| dev.has(v.feature()))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameMode {
    /// Most keys the keyboard can block.
    pub max: u8,
    /// Keys logiset last blocked. The device cannot report them, so `None` until applied.
    pub blocked: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisabledKeys {
    /// Bits the keyboard can disable.
    pub supported: u8,
    /// Bits currently disabled.
    pub disabled: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnInversion {
    pub variant: FnVariant,
    /// `true`: F1-F12 trigger their special function without Fn.
    pub inverted: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub game_mode: Option<GameMode>,
    pub disabled_keys: Option<DisabledKeys>,
    pub fn_inversion: Option<FnInversion>,
    /// HID keyboard country code reported by the keyboard.
    pub layout: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(clippy::enum_variant_names)] // names are the saved-setting vocabulary
pub enum Action {
    /// Keys (HID keyboard usages) ignored while the hardware game mode is on.
    SetGameModeKeys(Vec<u8>),
    /// Disable-keys mask, see [`DISABLE_BITS`].
    SetDisabledKeys(u8),
    /// `true`: F1-F12 trigger their special function without Fn.
    SetFnInversion(bool),
}

impl Action {
    pub fn persist_key(&self) -> Option<String> {
        let key = match self {
            Action::SetGameModeKeys(_) => "keyboard.gamemode",
            Action::SetDisabledKeys(_) => "keyboard.disabled",
            Action::SetFnInversion(_) => "keyboard.fn",
        };
        Some(key.into())
    }

    pub fn describe(&self) -> String {
        match self {
            Action::SetGameModeKeys(keys) if keys.is_empty() => "Game mode blocks no keys".into(),
            Action::SetGameModeKeys(keys) => {
                let names: Vec<String> = keys.iter().map(|k| hid_usage::key_name(*k)).collect();
                format!("Game mode blocks {}", names.join(", "))
            }
            Action::SetDisabledKeys(0) => "No keys disabled".into(),
            Action::SetDisabledKeys(mask) => format!("Disabled {}", disabled_names(*mask)),
            Action::SetFnInversion(true) => "Fn keys: media first".into(),
            Action::SetFnInversion(false) => "Fn keys: F1-F12 first".into(),
        }
    }
}

/// Names of the disable-keys bits set in `mask`, comma separated.
fn disabled_names(mask: u8) -> String {
    DISABLE_BITS.iter().filter(|(bit, _)| mask & bit != 0).map(|(_, name)| *name).collect::<Vec<_>>().join(", ")
}

/// `mask` with `bit` set to `on`.
pub fn toggled(mask: u8, bit: u8, on: bool) -> u8 {
    if on { mask | bit } else { mask & !bit }
}

/// READ-ONLY. Calls: 0x4522 fn0; 0x4521 fn0, fn1; 0x40A2/0x40A0/0x40A3 fn0 (0x40A3 also
/// 0x1815 fn0 for the host slot); 0x4540 fn0.
pub fn probe(dev: &Device) -> Option<Result<State>> {
    let present = [GAME_MODE, DISABLE_KEYS, FN_NEW, FN_STANDARD, FN_K375S, LAYOUT].iter().any(|&id| dev.has(id));
    if !present {
        return None;
    }
    Some(read_state(dev))
}

fn read_state(dev: &Device) -> Result<State> {
    let game_mode = if dev.has(GAME_MODE) { Some(parse_game_mode(&dev.call(GAME_MODE, 0, &[])?)) } else { None };
    let disabled_keys = if dev.has(DISABLE_KEYS) {
        let caps = dev.call(DISABLE_KEYS, 0, &[])?;
        let current = dev.call(DISABLE_KEYS, 1, &[])?;
        Some(parse_disabled(&caps, &current))
    } else {
        None
    };
    let fn_inversion = match FnVariant::present(dev) {
        Some(variant) => {
            let host = host_for(dev, variant);
            let reply = dev.call(variant.feature(), 0, &fn_read_params(variant, host))?;
            Some(FnInversion { variant, inverted: parse_fn(variant, &reply) })
        }
        None => None,
    };
    let layout = if dev.has(LAYOUT) { Some(parse_layout(&dev.call(LAYOUT, 0, &[])?)) } else { None };
    Ok(State { game_mode, disabled_keys, fn_inversion, layout })
}

/// Perform `action`, then re-read what the device reports.
pub fn apply(dev: &Device, state: &mut State, action: &Action) -> Result<()> {
    match action {
        Action::SetGameModeKeys(keys) => set_game_mode(dev, state, keys),
        Action::SetDisabledKeys(mask) => set_disabled(dev, state, *mask),
        Action::SetFnInversion(on) => set_fn(dev, state, *on),
    }
}

fn set_game_mode(dev: &Device, state: &mut State, keys: &[u8]) -> Result<()> {
    let game = state.game_mode.as_mut().ok_or(Error::Unsupported(GAME_MODE))?;
    let keys = checked_keys(keys, game.max)?;
    // Unknown until every write succeeds.
    game.blocked = None;
    dev.call(GAME_MODE, 3, &[])?;
    for chunk in keys.chunks(BLOCK_BATCH) {
        dev.call(GAME_MODE, 1, chunk)?;
    }
    game.blocked = Some(keys);
    Ok(())
}

fn set_disabled(dev: &Device, state: &mut State, mask: u8) -> Result<()> {
    let slot = state.disabled_keys.as_mut().ok_or(Error::Unsupported(DISABLE_KEYS))?;
    if mask & !slot.supported != 0 {
        return Err(Error::Invalid("this keyboard cannot disable those keys".into()));
    }
    dev.call(DISABLE_KEYS, 2, &[mask])?;
    let current = dev.call(DISABLE_KEYS, 1, &[])?;
    slot.disabled = current[0] & slot.supported;
    Ok(())
}

fn set_fn(dev: &Device, state: &mut State, on: bool) -> Result<()> {
    let slot = state.fn_inversion.as_mut().ok_or(Error::Unsupported(FN_STANDARD))?;
    let variant = slot.variant;
    let host = host_for(dev, variant);
    dev.call(variant.feature(), 1, &fn_write_params(variant, host, on))?;
    let reply = dev.call(variant.feature(), 0, &fn_read_params(variant, host))?;
    slot.inverted = parse_fn(variant, &reply);
    Ok(())
}

/// Host slot the K375s setting is stored under: the current host, or 0xFF when unknown.
fn host_for(dev: &Device, variant: FnVariant) -> u8 {
    if variant != FnVariant::K375s {
        return 0;
    }
    dev.call(HOSTS_INFO, 0, &[]).map(|r| parse_current_host(&r)).unwrap_or(0xFF)
}

/// Game-mode list to send: no duplicates, no key 0 (the padding value), at most `max` keys.
pub fn checked_keys(keys: &[u8], max: u8) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(keys.len());
    for &key in keys {
        if key == 0 {
            return Err(Error::Invalid("key 0 cannot be blocked".into()));
        }
        if !out.contains(&key) {
            out.push(key);
        }
    }
    if out.len() > max as usize {
        return Err(Error::Invalid(format!("this keyboard blocks at most {max} keys in game mode")));
    }
    Ok(out)
}

/// 0x4522 fn0 reply: `[max keys]`.
fn parse_game_mode(reply: &[u8; 16]) -> GameMode {
    GameMode { max: reply[0], blocked: None }
}

/// 0x4521 fn0 reply (capabilities) and fn1 reply (current state), both mask bytes.
fn parse_disabled(caps: &[u8; 16], current: &[u8; 16]) -> DisabledKeys {
    let supported = caps[0] & DISABLE_MASK;
    DisabledKeys { supported, disabled: current[0] & supported }
}

/// Fn-key reply. Standard and new: `[value]`. K375s: `[host echo, value]`.
/// Value 0x01 means inverted; anything else reads as not inverted, as in Solaar.
fn parse_fn(variant: FnVariant, reply: &[u8; 16]) -> bool {
    match variant {
        FnVariant::K375s => reply[1] == 0x01,
        _ => reply[0] == 0x01,
    }
}

/// Read params for the Fn setting: K375s takes the host slot, the others take nothing.
fn fn_read_params(variant: FnVariant, host: u8) -> Vec<u8> {
    match variant {
        FnVariant::K375s => vec![host],
        _ => Vec::new(),
    }
}

/// Write params for the Fn setting: `[value]`, or `[host, value]` for K375s.
fn fn_write_params(variant: FnVariant, host: u8, on: bool) -> Vec<u8> {
    match variant {
        FnVariant::K375s => vec![host, on as u8],
        _ => vec![on as u8],
    }
}

/// HostsInfo fn0 reply: `[capability flags, reserved, host count, current host]`.
fn parse_current_host(reply: &[u8; 16]) -> u8 {
    reply[3]
}

/// 0x4540 fn0 reply: `[layout code]`.
fn parse_layout(reply: &[u8; 16]) -> u8 {
    reply[0]
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
    fn game_mode_max_from_g610() {
        // Real G610 reply to 0x4522 fn0: ff 00 ...
        let game = parse_game_mode(&reply(&[0xFF]));
        assert_eq!(game, GameMode { max: 255, blocked: None });
    }

    #[test]
    fn checked_keys_dedupes_and_limits() {
        assert_eq!(checked_keys(&[0xE3, 0xE7, 0xE3], 2), Ok(vec![0xE3, 0xE7]));
        assert!(checked_keys(&[0xE3, 0xE7, 0x65], 2).is_err());
        assert!(checked_keys(&[0x00], 5).is_err());
        assert!(checked_keys(&[0x65], 0).is_err());
        assert_eq!(checked_keys(&[], 0), Ok(vec![]));
    }

    #[test]
    fn block_calls_hold_16_keys() {
        let keys: Vec<u8> = (4..=20).collect();
        let sizes: Vec<usize> = keys.chunks(BLOCK_BATCH).map(|c| c.len()).collect();
        assert_eq!(sizes, vec![16, 1]);
    }

    #[test]
    fn disabled_keys_are_masked_to_capabilities() {
        assert_eq!(parse_disabled(&reply(&[0x1F]), &reply(&[0x09])), DisabledKeys { supported: 0x1F, disabled: 0x09 });
        assert_eq!(parse_disabled(&reply(&[0x03]), &reply(&[0x1F])), DisabledKeys { supported: 0x03, disabled: 0x03 });
    }

    #[test]
    fn disabled_bit_toggle_and_names() {
        assert_eq!(toggled(0x01, 0x08, true), 0x09);
        assert_eq!(toggled(0x09, 0x01, false), 0x08);
        assert_eq!(disabled_names(0x09), "Caps Lock, Insert");
    }

    #[test]
    fn fn_standard_layout() {
        assert!(parse_fn(FnVariant::Standard, &reply(&[0x01])));
        assert!(!parse_fn(FnVariant::New, &reply(&[0x00])));
        assert_eq!(fn_read_params(FnVariant::Standard, 3), Vec::<u8>::new());
        assert_eq!(fn_write_params(FnVariant::New, 3, true), vec![1]);
        assert_eq!(fn_write_params(FnVariant::Standard, 3, false), vec![0]);
    }

    #[test]
    fn fn_k375s_layout_uses_host_prefix() {
        assert!(parse_fn(FnVariant::K375s, &reply(&[0x02, 0x01])));
        assert!(!parse_fn(FnVariant::K375s, &reply(&[0x02, 0x00])));
        assert_eq!(fn_read_params(FnVariant::K375s, 2), vec![2]);
        assert_eq!(fn_write_params(FnVariant::K375s, 2, true), vec![2, 1]);
        assert_eq!(parse_current_host(&reply(&[0x01, 0x00, 0x03, 0x01])), 1);
    }

    #[test]
    fn layout_code() {
        assert_eq!(parse_layout(&reply(&[0x21])), 0x21);
    }

    #[test]
    fn actions_describe_and_persist() {
        assert_eq!(Action::SetGameModeKeys(vec![0xE3, 0xE7]).describe(), "Game mode blocks Left Super, Right Super");
        assert_eq!(Action::SetGameModeKeys(vec![]).describe(), "Game mode blocks no keys");
        assert_eq!(Action::SetDisabledKeys(0x09).describe(), "Disabled Caps Lock, Insert");
        assert_eq!(Action::SetFnInversion(true).describe(), "Fn keys: media first");
        assert_eq!(Action::SetFnInversion(false).describe(), "Fn keys: F1-F12 first");
        assert_eq!(Action::SetDisabledKeys(1).persist_key().as_deref(), Some("keyboard.disabled"));
        assert_eq!(Action::SetFnInversion(true).persist_key().as_deref(), Some("keyboard.fn"));
        assert_eq!(Action::SetGameModeKeys(vec![]).persist_key().as_deref(), Some("keyboard.gamemode"));
    }

    #[test]
    fn actions_round_trip_through_json() {
        for action in [Action::SetGameModeKeys(vec![0x39, 0x65]), Action::SetDisabledKeys(0x10), Action::SetFnInversion(true)] {
            let json = serde_json::to_string(&action).unwrap();
            assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), action);
        }
    }
}
