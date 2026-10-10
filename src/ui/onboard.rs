//! Onboard profiles panel: the mode, the profile list, an editor for one profile, and the
//! backups saved before each write.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui::{self, RichText};

use crate::features::onboard::{
    Action, ButtonAction, DPI_STEPS, FUNCTIONS, MOUSE_BUTTONS, Mode, Profile, REPORT_RATES, State,
};
use crate::hid_usage;
use crate::model::DeviceState;
use crate::util;

/// Report rates offered in the editor. Other rates stay listed when a profile already uses one.
const COMMON_RATES: &[u8] = &[1, 2, 4, 8];
/// Profile sectors from here on are factory ROM.
const ROM_SECTOR: u16 = 0x0100;
/// A profile name holds 24 UTF-16 units.
const NAME_UNITS: usize = 24;

/// UI-only editing state for the panel.
#[derive(Default)]
pub struct Draft {
    /// The state the drafts were built from. A different state rebuilds them.
    synced: Option<State>,
    /// Sector of the profile open in the editor.
    selected: Option<u16>,
    /// Editable copy of the open profile.
    edit: Option<Profile>,
    /// A change waiting for the user to confirm it.
    confirm: Option<Confirm>,
    /// Why a backup could not be restored.
    error: Option<String>,
}

/// A change that needs a second click.
#[derive(Clone, Debug, PartialEq)]
enum Confirm {
    /// Write the open profile to the mouse.
    Write,
    /// Restore the backup at `path` into `sector`.
    Restore { sector: u16, path: PathBuf },
}

impl Draft {
    /// Rebuild the drafts when `state` differs from the one they were built from.
    fn sync(&mut self, state: &State) {
        if self.synced.as_ref() == Some(state) {
            return;
        }
        self.synced = Some(state.clone());
        self.confirm = None;
        self.error = None;
        self.selected = self.selected.filter(|s| state.profiles.iter().any(|p| p.sector == *s));
        self.edit = self.selected.and_then(|s| stored_profile(state, s)).cloned();
    }

    /// Open the editor on the stored profile at `sector`.
    fn select(&mut self, state: &State, sector: u16) {
        self.selected = Some(sector);
        self.edit = stored_profile(state, sector).cloned();
        self.confirm = None;
        self.error = None;
    }
}

/// Draw the panel. Edits stay in `draft` until the user writes them to the mouse.
pub fn show(ui: &mut egui::Ui, _dev: &DeviceState, state: &State, draft: &mut Draft, out: &mut Vec<Action>) {
    draft.sync(state);
    mode_section(ui, state, out);
    ui.add_space(12.0);
    profile_list(ui, state, draft, out);
    if let Some(sector) = draft.selected {
        ui.add_space(12.0);
        match draft.edit.as_mut() {
            Some(edit) => editor(ui, state, sector, edit, &mut draft.confirm, out),
            None => {
                ui.heading("Edit profile");
                ui.label(
                    RichText::new("This profile cannot be read, so it cannot be edited.")
                        .color(ui.visuals().error_fg_color),
                );
            }
        }
    }
    ui.add_space(12.0);
    backups(ui, state, draft, out);
}

/// The stored profile at `sector`, when it reads.
fn stored_profile(state: &State, sector: u16) -> Option<&Profile> {
    state.profiles.iter().find(|p| p.sector == sector)?.profile.as_ref().ok()
}

fn mode_section(ui: &mut egui::Ui, state: &State, out: &mut Vec<Action>) {
    ui.heading("Mode");
    if ui.radio(state.mode == Mode::Onboard, "Onboard profiles (stored in the mouse)").clicked()
        && state.mode != Mode::Onboard
    {
        out.push(Action::SetMode(Mode::Onboard));
    }
    if ui.radio(state.mode == Mode::Host, "Host mode (settings from this computer)").clicked()
        && state.mode != Mode::Host
    {
        out.push(Action::SetMode(Mode::Host));
    }
    ui.label(
        RichText::new(
            "In host mode, the Pointer and Lighting tabs make lasting changes; logiset re-applies them when the mouse \
             connects. Onboard mode uses the profiles below.",
        )
        .weak(),
    );
}

fn profile_list(ui: &mut egui::Ui, state: &State, draft: &mut Draft, out: &mut Vec<Action>) {
    ui.heading("Profiles");
    ui.label(
        RichText::new(
            "Enabled profiles can be switched to. Edit changes a profile's name, DPI, report rate and buttons.",
        )
        .weak(),
    );
    if state.rom {
        ui.label(
            RichText::new("The mouse holds only its factory profiles. They can be switched to but not changed.").weak(),
        );
    }
    if state.profiles.is_empty() {
        ui.label("No profiles are listed on the mouse.");
        return;
    }
    egui::Grid::new("onboard-profiles").num_columns(4).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
        ui.strong("Profile");
        ui.strong("Enabled");
        ui.strong("Status");
        ui.label("");
        ui.end_row();
        for entry in &state.profiles {
            let sector = entry.sector;
            let active = sector == state.current_profile;
            ui.vertical(|ui| {
                match &entry.profile {
                    Ok(p) if p.name.is_empty() => {
                        ui.label("(no name)");
                    }
                    Ok(p) => {
                        ui.label(p.name.as_str());
                    }
                    Err(e) => {
                        ui.label(RichText::new(format!("Cannot read: {e}")).color(ui.visuals().error_fg_color));
                    }
                }
                ui.label(RichText::new(format!("Sector 0x{sector:04X}")).small().weak());
            });

            let can_toggle = !state.rom && !(active && entry.enabled);
            let hint = if state.rom {
                "Factory profiles cannot be changed."
            } else {
                "Switch to another profile before disabling this one."
            };
            let mut on = entry.enabled;
            if ui.add_enabled(can_toggle, egui::Checkbox::without_text(&mut on)).on_disabled_hover_text(hint).clicked()
            {
                out.push(Action::SetProfileEnabled { sector, enabled: !entry.enabled });
            }

            if active {
                ui.strong("Active");
            } else if entry.enabled {
                if ui
                    .button("Activate")
                    .on_hover_text("Use this profile on the mouse. Also switches the mouse to onboard profiles.")
                    .clicked()
                {
                    out.push(Action::SetActiveProfile(sector));
                }
            } else {
                ui.label(RichText::new("Disabled").weak());
            }

            let can_edit = !state.rom && sector < ROM_SECTOR && entry.profile.is_ok();
            let open = draft.selected == Some(sector);
            let label = if open { "Editing" } else { "Edit" };
            if ui.add_enabled(can_edit && !open, egui::Button::new(label)).clicked() {
                draft.select(state, sector);
            }
            ui.end_row();
        }
    });
}

/// The editor for the open profile. `edit` is the draft copy and `confirm` holds a pending write.
fn editor(
    ui: &mut egui::Ui,
    state: &State,
    sector: u16,
    edit: &mut Profile,
    confirm: &mut Option<Confirm>,
    out: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.heading("Edit profile");
        ui.label(RichText::new(format!("sector 0x{sector:04X}")).weak());
    });

    let rate = edit.report_rate;
    egui::Grid::new("onboard-settings").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
        ui.label("Name");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut edit.name).desired_width(200.0));
            ui.label(RichText::new(format!("{}/{NAME_UNITS}", units(&edit.name))).small().weak());
        });
        ui.end_row();

        let current = REPORT_RATES.iter().find(|(ms, _)| *ms == rate).map_or("Unknown", |&(_, label)| label);
        ui.label("Report rate");
        egui::ComboBox::from_id_salt("onboard-rate").selected_text(current).show_ui(ui, |ui| {
            for (ms, label) in REPORT_RATES.iter().copied().filter(|&(ms, _)| COMMON_RATES.contains(&ms) || ms == rate)
            {
                ui.selectable_value(&mut edit.report_rate, ms, label);
            }
        });
        ui.end_row();

        ui.label("Colour");
        ui.color_edit_button_srgb(&mut edit.color);
        ui.end_row();
    });
    fit_name(&mut edit.name);

    ui.add_space(8.0);
    dpi_editor(ui, edit, state.description.has_dpi_shift());

    ui.add_space(8.0);
    ui.strong("Buttons");
    button_grid(ui, "onboard-buttons", &mut edit.buttons);
    if !edit.gbuttons.is_empty() {
        egui::CollapsingHeader::new("G-shift buttons").id_salt("onboard-gshift").default_open(false).show(ui, |ui| {
            button_grid(ui, "onboard-gbuttons", &mut edit.gbuttons);
        });
    }

    let stored = stored_profile(state, sector);
    let changed = stored != Some(&*edit);
    let mut revert = false;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(changed, egui::Button::new("Revert")).on_hover_text("Discard the changes made here").clicked()
        {
            revert = true;
        }
        let writable = changed && edit.to_bytes().is_ok();
        if ui.add_enabled(writable, egui::Button::new("Write to mouse…")).clicked() {
            *confirm = Some(Confirm::Write);
        }
        if changed {
            ui.label(RichText::new("Unsaved changes").color(ui.visuals().warn_fg_color));
        }
    });
    if revert {
        if let Some(p) = stored {
            *edit = p.clone();
        }
        ui.ctx().request_repaint();
    }

    let check = edit.to_bytes();
    if let Err(e) = &check {
        ui.label(RichText::new(format!("This profile cannot be written: {e}")).color(ui.visuals().error_fg_color));
    }
    if matches!(confirm, Some(Confirm::Write)) {
        let n = state.profiles.iter().position(|p| p.sector == sector).map_or(0, |i| i + 1);
        ui.label(format!(
            "Write profile {n} (\"{}\") to the mouse? A backup of the current one is saved to {}.",
            edit.name,
            util::backup_dir().display()
        ));
        ui.horizontal(|ui| {
            if ui.add_enabled(check.is_ok(), egui::Button::new("Write")).clicked() {
                out.push(Action::WriteProfile { sector, profile: edit.clone() });
                *confirm = None;
            }
            if ui.button("Cancel").clicked() {
                *confirm = None;
            }
        });
    }
}

/// Five DPI steps, the default step and, when the mouse has one, the DPI-shift step.
fn dpi_editor(ui: &mut egui::Ui, edit: &mut Profile, shift: bool) {
    ui.strong("DPI");
    egui::Grid::new("onboard-dpi").num_columns(4).spacing([16.0, 6.0]).show(ui, |ui| {
        ui.label(RichText::new("Step").weak());
        ui.label(RichText::new("DPI").weak());
        ui.label(RichText::new("Default").weak()).on_hover_text("Used when the mouse starts up.");
        if shift {
            ui.label(RichText::new("Shift").weak()).on_hover_text("Used while the DPI-shift button is held.");
        }
        ui.end_row();
        for i in 0..DPI_STEPS {
            let step = i as u8;
            ui.label(format!("{}", i + 1));
            ui.add(egui::DragValue::new(&mut edit.dpi[i]).range(50..=25_600).speed(50.0).suffix(" DPI"));
            ui.radio_value(&mut edit.default_dpi, step, "");
            if shift {
                ui.radio_value(&mut edit.shift_dpi, step, "");
            }
            ui.end_row();
        }
    });
}

/// One row per button: its name, the kind of action, and the action's value.
fn button_grid(ui: &mut egui::Ui, id: &'static str, buttons: &mut [ButtonAction]) {
    egui::Grid::new(id).num_columns(3).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
        for (i, action) in buttons.iter_mut().enumerate() {
            ui.label(button_name(i));
            let before = Kind::of(action);
            let mut kind = before;
            ui.add_enabled_ui(!matches!(before, Kind::Macro | Kind::Raw), |ui| {
                egui::ComboBox::from_id_salt((id, i, "kind")).selected_text(before.label()).show_ui(ui, |ui| {
                    for choice in Kind::CHOICES {
                        ui.selectable_value(&mut kind, choice, choice.label());
                    }
                });
            });
            if kind != before
                && let Some(fresh) = kind.fresh()
            {
                *action = fresh;
            }
            ui.push_id((id, i), |ui| value_editor(ui, action));
            ui.end_row();
        }
    });
}

/// The value widgets for one button. Macro and raw mappings are shown but kept as they are.
fn value_editor(ui: &mut egui::Ui, action: &mut ButtonAction) {
    let text = action.label();
    ui.vertical(|ui| match action {
        ButtonAction::Mouse(mask) => {
            egui::ComboBox::from_id_salt("mouse").selected_text(text).show_ui(ui, |ui| {
                for &(bit, name) in MOUSE_BUTTONS {
                    ui.selectable_value(&mut *mask, bit, name);
                }
            });
        }
        ButtonAction::Key { modifiers, usage } => {
            let key = hid_usage::key_name(*usage);
            egui::ComboBox::from_id_salt("key").selected_text(key).height(260.0).show_ui(ui, |ui| {
                for &(code, name) in hid_usage::KEYS {
                    ui.selectable_value(&mut *usage, code, name);
                }
            });
            ui.horizontal_wrapped(|ui| {
                for &(bit, name) in hid_usage::MODIFIERS {
                    let mut on = *modifiers & bit != 0;
                    if ui.checkbox(&mut on, name).clicked() {
                        *modifiers ^= bit;
                    }
                }
            });
        }
        ButtonAction::Consumer(usage) => {
            egui::ComboBox::from_id_salt("media").selected_text(text).height(260.0).show_ui(ui, |ui| {
                for &(code, name) in hid_usage::CONSUMER {
                    ui.selectable_value(&mut *usage, code, name);
                }
            });
        }
        ButtonAction::Function { code, .. } => {
            egui::ComboBox::from_id_salt("function").selected_text(text).height(260.0).show_ui(ui, |ui| {
                for &(value, name) in FUNCTIONS {
                    ui.selectable_value(&mut *code, value, name);
                }
            });
        }
        ButtonAction::Macro { .. } | ButtonAction::Raw(_) => {
            ui.label(RichText::new(text).weak());
        }
        ButtonAction::Unassigned | ButtonAction::NoAction => {}
    });
}

/// The kinds of button action the editor offers.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Mouse,
    Key,
    Media,
    Function,
    Disabled,
    Unassigned,
    Macro,
    Raw,
}

impl Kind {
    /// Kinds the user can pick. Macro and raw mappings can only be kept.
    const CHOICES: [Kind; 6] = [Kind::Mouse, Kind::Key, Kind::Media, Kind::Function, Kind::Disabled, Kind::Unassigned];

    fn of(action: &ButtonAction) -> Kind {
        match action {
            ButtonAction::Mouse(_) => Kind::Mouse,
            ButtonAction::Key { .. } => Kind::Key,
            ButtonAction::Consumer(_) => Kind::Media,
            ButtonAction::Function { .. } => Kind::Function,
            ButtonAction::NoAction => Kind::Disabled,
            ButtonAction::Unassigned => Kind::Unassigned,
            ButtonAction::Macro { .. } => Kind::Macro,
            ButtonAction::Raw(_) => Kind::Raw,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Mouse => "Mouse button",
            Kind::Key => "Keyboard key",
            Kind::Media => "Media key",
            Kind::Function => "Function",
            Kind::Disabled => "Disabled",
            Kind::Unassigned => "Not assigned",
            Kind::Macro => "Macro (read only)",
            Kind::Raw => "Other (read only)",
        }
    }

    /// The action a newly picked kind starts with. `None` for kinds that cannot be picked.
    fn fresh(self) -> Option<ButtonAction> {
        Some(match self {
            Kind::Mouse => ButtonAction::Mouse(0x0001),
            Kind::Key => ButtonAction::Key { modifiers: 0, usage: 0x04 },
            Kind::Media => ButtonAction::Consumer(0x00CD),
            Kind::Function => ButtonAction::Function { code: 0x03, data: 0 },
            Kind::Disabled => ButtonAction::NoAction,
            Kind::Unassigned => ButtonAction::Unassigned,
            Kind::Macro | Kind::Raw => return None,
        })
    }
}

/// Name of the physical button at `index` (0-based).
fn button_name(index: usize) -> String {
    const NAMES: [&str; 5] = ["Left", "Right", "Middle", "Back", "Forward"];
    NAMES.get(index).map_or_else(|| format!("Button {}", index + 1), |name| (*name).to_owned())
}

/// Length of a name in UTF-16 units, the unit a profile stores it in.
fn units(name: &str) -> usize {
    name.encode_utf16().count()
}

/// Remove NUL characters and cut `name` to the units a profile name holds.
fn fit_name(name: &mut String) {
    name.retain(|c| c != '\0' && c != '\u{FFFF}');
    let mut used = 0;
    let cut = name.char_indices().find_map(|(at, c)| {
        used += c.len_utf16();
        (used > NAME_UNITS).then_some(at)
    });
    if let Some(at) = cut {
        name.truncate(at);
    }
}

/// A backup file of one profile sector.
struct Backup {
    path: PathBuf,
    name: String,
    sector: u16,
    size: u64,
    modified: SystemTime,
}

fn backups(ui: &mut egui::Ui, state: &State, draft: &mut Draft, out: &mut Vec<Action>) {
    egui::CollapsingHeader::new("Backups").id_salt("onboard-backups").default_open(false).show(ui, |ui| {
        ui.label(
            RichText::new(format!("Before each write, the old profile is saved to {}.", util::backup_dir().display()))
                .weak(),
        );
        let size = u64::from(state.description.sector_size);
        let files = list_backups(&state.device_tag);
        if files.is_empty() {
            ui.label(RichText::new("No backups for this mouse yet.").weak());
        }
        egui::Grid::new("onboard-backup-list").num_columns(4).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
            for b in &files {
                ui.label(format!("Sector 0x{:04X}", b.sector));
                ui.label(format!("{} bytes", b.size));
                ui.label(RichText::new(b.name.as_str()).small().weak());
                if ui
                    .add_enabled(!state.rom, egui::Button::new("Restore"))
                    .on_disabled_hover_text("Factory profiles cannot be changed.")
                    .clicked()
                {
                    draft.confirm = Some(Confirm::Restore { sector: b.sector, path: b.path.clone() });
                }
                ui.end_row();
            }
        });

        if let Some(Confirm::Restore { sector, path }) = draft.confirm.clone() {
            let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            ui.label(format!("Restore sector 0x{sector:04X} from {name}? The current contents are backed up first."));
            ui.horizontal(|ui| {
                if ui.button("Restore").clicked() {
                    queue_restore(draft, out, sector, &path, size);
                }
                if ui.button("Cancel").clicked() {
                    draft.confirm = None;
                }
            });
        }
        if let Some(error) = &draft.error {
            ui.label(RichText::new(error.as_str()).color(ui.visuals().error_fg_color));
        }
    });
}

/// Read the backup at `path` and queue its restore. A file of the wrong size is refused here.
fn queue_restore(draft: &mut Draft, out: &mut Vec<Action>, sector: u16, path: &Path, size: u64) {
    draft.confirm = None;
    match fs::read(path) {
        Ok(bytes) if bytes.len() as u64 == size => {
            draft.error = None;
            out.push(Action::RestoreSector { sector, bytes });
        }
        Ok(bytes) => {
            draft.error = Some(format!("{} is {} bytes, but a profile is {size} bytes.", path.display(), bytes.len()));
        }
        Err(e) => {
            draft.error = Some(format!("Could not read {}: {e}", path.display()));
        }
    }
}

/// Backups of this mouse's sectors, newest first.
fn list_backups(tag: &str) -> Vec<Backup> {
    let Ok(dir) = fs::read_dir(util::backup_dir()) else {
        return Vec::new();
    };
    let mut files: Vec<Backup> = dir
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let sector = backup_sector(&name, tag)?;
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| Backup {
                path: entry.path(),
                name,
                sector,
                size: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            })
        })
        .collect();
    files.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.name.cmp(&a.name)));
    files
}

/// The sector a backup holds, from its name `{tag}-sector{sector:04x}-{stamp}.bin`.
fn backup_sector(name: &str, tag: &str) -> Option<u16> {
    let rest = name.strip_prefix(tag)?.strip_prefix("-sector")?;
    if !name.ends_with(".bin") {
        return None;
    }
    let hex = rest.get(..4).filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))?;
    u16::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::onboard::{Description, ProfileEntry, crc16};

    /// A profile sector named "PRO" with report rate 1 ms and no mappings, with a valid CRC.
    fn sample_profile() -> Profile {
        let mut raw = vec![0xFF; 255];
        raw[0] = 1;
        raw[1] = 0;
        raw[2] = 1;
        raw[160..166].copy_from_slice(&[0x50, 0x00, 0x52, 0x00, 0x4F, 0x00]);
        let n = raw.len();
        let crc = crc16(&raw[..n - 2]);
        raw[n - 2..].copy_from_slice(&crc.to_be_bytes());
        Profile::parse(&raw, 11, 11).unwrap()
    }

    fn state_with(profile: Profile) -> State {
        State {
            description: Description { sector_size: 255, button_count: 11, ..Default::default() },
            current_profile: 1,
            profiles: vec![ProfileEntry { sector: 1, enabled: true, profile: Ok(profile) }],
            ..Default::default()
        }
    }

    #[test]
    fn draft_keeps_edits_until_the_state_changes() {
        let state = state_with(sample_profile());
        let mut draft = Draft::default();
        draft.sync(&state);
        draft.select(&state, 1);
        assert_eq!(draft.edit.as_ref(), stored_profile(&state, 1));
        draft.edit.as_mut().unwrap().report_rate = 2;
        draft.sync(&state);
        assert_eq!(draft.edit.as_ref().unwrap().report_rate, 2, "the same state keeps the edit");
        let mut next = state.clone();
        next.mode = Mode::Host;
        draft.sync(&next);
        assert_eq!(draft.edit.as_ref().unwrap().report_rate, 1, "a new state rebuilds the editor");
    }

    #[test]
    fn selection_ends_when_the_profile_is_gone() {
        let state = state_with(sample_profile());
        let mut draft = Draft::default();
        draft.sync(&state);
        draft.select(&state, 1);
        let mut gone = state.clone();
        gone.profiles.clear();
        draft.sync(&gone);
        assert_eq!(draft.selected, None);
        assert!(draft.edit.is_none());
    }

    #[test]
    fn names_fit_their_units() {
        let mut name = "a".repeat(30);
        fit_name(&mut name);
        assert_eq!(name.len(), 24);
        let mut emoji = "\u{1F600}".repeat(13);
        fit_name(&mut emoji);
        assert_eq!(units(&emoji), 24);
        let mut nul = String::from("a\0b");
        fit_name(&mut nul);
        assert_eq!(nul, "ab");
    }

    #[test]
    fn backup_names_give_their_sector() {
        assert_eq!(backup_sector("00010203-sector0004-1700000000.bin", "00010203"), Some(4));
        assert_eq!(backup_sector("00010203-sector0104-1700000000-2.bin", "00010203"), Some(0x104));
        assert_eq!(backup_sector("00010203-sector0004-1700000000.tmp", "00010203"), None);
        assert_eq!(backup_sector("0001020-sector0004-1.bin", "00010203"), None);
        assert_eq!(backup_sector("00010203-sectorZZZZ-1.bin", "00010203"), None);
    }

    #[test]
    fn button_kinds_round_trip() {
        for kind in Kind::CHOICES {
            let action = kind.fresh().unwrap();
            assert_eq!(Kind::of(&action), kind);
            assert!(action.encode().is_ok());
        }
        assert!(Kind::Macro.fresh().is_none());
        assert_eq!(Kind::of(&ButtonAction::Macro { stop: false, sector: 1, address: 2 }), Kind::Macro);
        assert_eq!(button_name(0), "Left");
        assert_eq!(button_name(4), "Forward");
        assert_eq!(button_name(10), "Button 11");
    }
}
