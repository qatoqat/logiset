//! Keyboard panel: game-mode keys, disabled keys, F1-F12 behaviour and the layout code.

use eframe::egui::{self, RichText};

use crate::features::keyboard::{self, Action, DisabledKeys, FnInversion, GameMode, State, DISABLE_BITS};
use crate::hid_usage;
use crate::model::DeviceState;

/// Game-mode keys offered as checkboxes: Left Super, Right Super, Menu, Caps Lock, Left Alt, Tab, Escape.
const COMMON_KEYS: &[u8] = &[0xE3, 0xE7, 0x65, 0x39, 0xE2, 0x2B, 0x29];

/// UI-only editing state for the keyboard panel.
#[derive(Default)]
pub struct Draft {
    /// `state.game_mode` as of the last sync; `None` before the first one.
    synced: Option<Option<GameMode>>,
    /// Game-mode keys as edited, not yet applied.
    keys: Vec<u8>,
}

pub fn show(ui: &mut egui::Ui, _dev: &DeviceState, state: &State, draft: &mut Draft, out: &mut Vec<Action>) {
    // Re-sync only when the game-mode part changes, so unapplied edits survive other changes.
    if draft.synced.as_ref() != Some(&state.game_mode) {
        draft.keys = state.game_mode.as_ref().and_then(|g| g.blocked.clone()).unwrap_or_default();
        draft.synced = Some(state.game_mode.clone());
    }
    if let Some(game) = &state.game_mode {
        game_mode(ui, game, &mut draft.keys, out);
    }
    if let Some(disabled) = &state.disabled_keys {
        disabled_keys(ui, disabled, out);
    }
    if let Some(fn_inv) = &state.fn_inversion {
        function_keys(ui, fn_inv, out);
    }
    if let Some(layout) = state.layout {
        ui.add_space(12.0);
        ui.label(RichText::new(format!("Keyboard layout code: {layout} (read only)")).weak());
    }
}

fn game_mode(ui: &mut egui::Ui, game: &GameMode, keys: &mut Vec<u8>, out: &mut Vec<Action>) {
    ui.heading("Game mode keys");
    ui.label(RichText::new("Keys listed here are ignored while the keyboard's Game Mode switch is on.").weak());
    if game.max == 0 {
        ui.label("This keyboard cannot block keys.");
        return;
    }
    if game.blocked.is_none() {
        ui.label(RichText::new("Not set by logiset yet.").weak());
    }

    let full = keys.len() >= game.max as usize;
    for &usage in COMMON_KEYS {
        let mut on = keys.contains(&usage);
        let enabled = on || !full;
        if ui.add_enabled(enabled, egui::Checkbox::new(&mut on, hid_usage::key_name(usage))).changed() {
            if on {
                keys.push(usage);
            } else {
                keys.retain(|k| *k != usage);
            }
        }
    }

    let extras: Vec<u8> = keys.iter().copied().filter(|k| !COMMON_KEYS.contains(k)).collect();
    let mut remove = None;
    for usage in extras {
        ui.horizontal(|ui| {
            ui.label(hid_usage::key_name(usage));
            if ui.small_button("Remove").clicked() {
                remove = Some(usage);
            }
        });
    }
    if let Some(usage) = remove {
        keys.retain(|k| *k != usage);
    }

    let mut pick: Option<u8> = None;
    if !full {
        egui::ComboBox::from_id_salt("keyboard-game-add").selected_text("Add a key").show_ui(ui, |ui| {
            for &(usage, name) in hid_usage::KEYS {
                if !keys.contains(&usage) {
                    ui.selectable_value(&mut pick, Some(usage), name);
                }
            }
        });
    }
    if let Some(usage) = pick {
        keys.push(usage);
    }

    ui.horizontal(|ui| {
        if ui.button("Apply").on_hover_text("Replace the game mode list on the keyboard").clicked() {
            out.push(Action::SetGameModeKeys(keys.clone()));
        }
        ui.label(RichText::new(format!("{} of {} keys", keys.len(), game.max)).weak());
    });
}

fn disabled_keys(ui: &mut egui::Ui, disabled: &DisabledKeys, out: &mut Vec<Action>) {
    ui.add_space(12.0);
    ui.heading("Disable keys");
    ui.label(RichText::new("Turn keys off on the keyboard. Changes apply right away.").weak());
    if disabled.supported == 0 {
        ui.label("No keys can be disabled.");
        return;
    }
    for &(bit, name) in DISABLE_BITS {
        if disabled.supported & bit == 0 {
            continue;
        }
        let mut on = disabled.disabled & bit != 0;
        let resp = ui.checkbox(&mut on, name).on_hover_text(format!("Disables the {name} key."));
        if resp.changed() {
            out.push(Action::SetDisabledKeys(keyboard::toggled(disabled.disabled, bit, on)));
        }
    }
}

fn function_keys(ui: &mut egui::Ui, fn_inv: &FnInversion, out: &mut Vec<Action>) {
    ui.add_space(12.0);
    ui.heading("Function keys");
    let mut plain = !fn_inv.inverted;
    let resp = ui
        .checkbox(&mut plain, "F1–F12 send function keys without Fn")
        .on_hover_text("When off, F1–F12 send their media or special function. Hold Fn for the plain F-key.");
    if resp.changed() {
        out.push(Action::SetFnInversion(!plain));
    }
}
