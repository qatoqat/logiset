//! Overview tab: identity, connection, firmware, battery and the HID++ feature table.

use eframe::egui::{self, RichText};

use crate::features::battery::{self, ChargeState};
use crate::hidpp::feature_name;
use crate::model::DeviceState;

pub fn show(ui: &mut egui::Ui, dev: &DeviceState) {
    if dev.onboard_mode() == Some(true) {
        ui.label("This mouse runs from its onboard profile. DPI, report rate and lighting changes made here are temporary.");
        ui.add_space(8.0);
    }

    egui::Grid::new("overview_info").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
        row(ui, "Type", dev.kind.label());

        let mut place = Vec::new();
        if !dev.path.as_os_str().is_empty() {
            place.push(dev.path.display().to_string());
        }
        place.push(format!("index 0x{:02X}", dev.index));
        row(ui, "Connection", format!("{} ({})", dev.connection_label(), place.join(", ")));

        row(ui, "Protocol", format!("HID++ {}.{}", dev.protocol.0, dev.protocol.1));
        row(ui, "Unit ID", dev.unit_id.map_or("—".to_owned(), |u| format!("0x{u:08X}")));

        let models: Vec<String> = dev.model_ids.iter().filter(|&&m| m != 0).map(|m| format!("0x{m:04X}")).collect();
        row(ui, "Model IDs", if models.is_empty() { "—".to_owned() } else { models.join(", ") });
        row(ui, "Serial", dev.serial.as_deref().unwrap_or("—"));

        if dev.firmware.is_empty() {
            row(ui, "Firmware", "—");
        }
        for fw in &dev.firmware {
            let value = if fw.name.is_empty() { fw.version.clone() } else { format!("{} {}", fw.name, fw.version) };
            row(ui, fw.kind, value);
        }

        match &dev.battery {
            Some(Ok(b)) => {
                ui.label(RichText::new("Battery").weak());
                battery_cell(ui, b);
                ui.end_row();
            }
            Some(Err(e)) => row_text(ui, "Battery", RichText::new(format!("Could not read: {e}")).color(ui.visuals().error_fg_color)),
            None => row(ui, "Battery", "Not reported by this device"),
        }
    });

    ui.add_space(12.0);
    egui::CollapsingHeader::new(format!("HID++ features ({})", dev.features.len()))
        .id_salt("overview_features")
        .default_open(false)
        .show(ui, |ui| {
            egui::Grid::new("overview_feature_table").striped(true).num_columns(5).spacing([16.0, 4.0]).show(ui, |ui| {
                for head in ["Index", "ID", "Name", "Version", "Flags"] {
                    ui.label(RichText::new(head).strong());
                }
                ui.end_row();
                for f in &dev.features {
                    ui.label(f.index.to_string());
                    ui.label(format!("0x{:04X}", f.id));
                    ui.label(feature_name(f.id));
                    ui.label(f.version.to_string());
                    ui.label(flags_text(f.flags));
                    ui.end_row();
                }
            });
        });
}

fn row(ui: &mut egui::Ui, label: &str, value: impl Into<egui::WidgetText>) {
    ui.label(RichText::new(label).weak());
    ui.label(value);
    ui.end_row();
}

fn row_text(ui: &mut egui::Ui, label: &str, value: RichText) {
    ui.label(RichText::new(label).weak());
    ui.label(value);
    ui.end_row();
}

/// Percent bar (or level name) with voltage and charging state under it.
fn battery_cell(ui: &mut egui::Ui, b: &battery::State) {
    ui.vertical(|ui| {
        match (b.percent, b.level) {
            (Some(p), _) => {
                let text = if b.estimated { format!("≈{p}%") } else { format!("{p}%") };
                ui.add(egui::ProgressBar::new(f32::from(p) / 100.0).text(text).desired_width(200.0));
            }
            (None, Some(level)) => {
                ui.label(level.label());
            }
            (None, None) => {
                ui.label("unknown");
            }
        }

        let mut details = Vec::new();
        if let Some(mv) = b.voltage_mv {
            details.push(format!("{mv} mV"));
        }
        let status = b.charging.label();
        let status = match &b.charging {
            ChargeState::Error(_) => RichText::new(status).color(ui.visuals().warn_fg_color),
            _ => RichText::new(status).small().weak(),
        };
        if !details.is_empty() {
            ui.label(RichText::new(details.join(" · ")).small().weak());
        }
        ui.label(status);
        if b.estimated {
            ui.label(RichText::new("Estimated from voltage").small().weak());
        }
    });
}

fn flags_text(flags: u8) -> String {
    let mut names = Vec::new();
    if flags & 0x80 != 0 {
        names.push("obsolete");
    }
    if flags & 0x40 != 0 {
        names.push("hidden");
    }
    if flags & 0x20 != 0 {
        names.push("internal");
    }
    names.join(", ")
}
