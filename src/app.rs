//! The egui shell: device list, tabs, status line. Slice panels live in `ui/`.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};

use crate::backend::{self, Command, Event};
use crate::features::battery;
use crate::model::{Action, DeviceState, Slot};
use crate::ui::{self, Drafts};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Pointer,
    Lighting,
    Profiles,
    Keyboard,
    Scrolling,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Pointer => "Pointer",
            Tab::Lighting => "Lighting",
            Tab::Profiles => "Onboard profiles",
            Tab::Keyboard => "Keyboard",
            Tab::Scrolling => "Scrolling",
        }
    }
}

struct Status {
    text: String,
    error: bool,
    at: Instant,
}

pub struct App {
    commands: Sender<Command>,
    events: Receiver<Event>,
    devices: Vec<DeviceState>,
    scanned: bool,
    selected: Option<String>,
    tab: Tab,
    drafts: HashMap<String, Drafts>,
    busy: HashSet<String>,
    status: Option<Status>,
    problems: Vec<String>,
    auto_apply: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> App {
        let ctx = cc.egui_ctx.clone();
        let (commands, events) = backend::spawn(move || ctx.request_repaint());
        App {
            commands,
            events,
            devices: Vec::new(),
            scanned: false,
            selected: None,
            tab: Tab::Overview,
            drafts: HashMap::new(),
            busy: HashSet::new(),
            status: None,
            problems: Vec::new(),
            auto_apply: true,
        }
    }

    fn send(&self, cmd: Command) {
        let _ = self.commands.send(cmd);
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Devices(list) => {
                    self.scanned = true;
                    self.devices = list;
                    let still_there = self.selected.as_ref().is_some_and(|s| self.devices.iter().any(|d| &d.id == s));
                    if !still_there {
                        self.selected = self.devices.iter().find(|d| d.online).or(self.devices.first()).map(|d| d.id.clone());
                    }
                    self.drafts.retain(|id, _| self.devices.iter().any(|d| &d.id == id));
                }
                Event::Device(state) => match self.devices.iter_mut().find(|d| d.id == state.id) {
                    Some(d) => *d = state,
                    None => self.devices.push(state),
                },
                Event::Busy(id, true) => {
                    self.busy.insert(id);
                }
                Event::Busy(id, false) => {
                    self.busy.remove(&id);
                }
                Event::Status(text) => self.status = Some(Status { text, error: false, at: Instant::now() }),
                Event::Error(text) => self.status = Some(Status { text, error: true, at: Instant::now() }),
                Event::Problems(p) => self.problems = p,
                Event::AutoApply(on) => self.auto_apply = on,
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("logiset");
            ui.separator();
            if ui.button("⟳ Rescan").on_hover_text("Detect devices again and re-read all settings").clicked() {
                self.scanned = false;
                self.send(Command::Rescan);
            }
            let mut auto = self.auto_apply;
            if ui
                .checkbox(&mut auto, "Re-apply saved settings on connect")
                .on_hover_text("Settings such as host-mode DPI are lost when a device powers off. logiset remembers what you set and applies it again.")
                .changed()
            {
                self.auto_apply = auto;
                self.send(Command::SetAutoApply(auto));
            }
            if !self.busy.is_empty() || !self.scanned {
                ui.spinner();
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| match &self.status {
            Some(s) if s.at.elapsed() < Duration::from_secs(12) => {
                let color = if s.error { ui.visuals().error_fg_color } else { ui.visuals().weak_text_color() };
                ui.label(RichText::new(&s.text).color(color));
            }
            _ => {
                ui.label(RichText::new(format!("{} device(s)", self.devices.len())).weak());
            }
        });
        if self.status.as_ref().is_some_and(|s| s.at.elapsed() < Duration::from_secs(12)) {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
    }

    fn device_list(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            if !self.scanned && self.devices.is_empty() {
                ui.label("Looking for devices…");
            } else if self.devices.is_empty() {
                ui.label("No Logitech HID++ devices found.");
            }
            let mut groups: Vec<Option<String>> = Vec::new();
            for d in &self.devices {
                if !groups.contains(&d.receiver) {
                    groups.push(d.receiver.clone());
                }
            }
            for group in groups {
                ui.add_space(4.0);
                ui.label(RichText::new(group.as_deref().unwrap_or("Connected directly")).small().weak());
                for d in self.devices.iter().filter(|d| d.receiver == group) {
                    let mut text = format!("{}  {}", d.kind.icon(), d.name);
                    if let Some(Ok(b)) = &d.battery {
                        let s = battery::summary(b);
                        if !s.is_empty() {
                            text.push_str(&format!("  ·  {s}"));
                        }
                    }
                    let mut label = RichText::new(text);
                    if !d.online {
                        label = label.weak().italics();
                    }
                    let selected = self.selected.as_deref() == Some(d.id.as_str());
                    let resp = ui.selectable_label(selected, label);
                    let resp = if d.online { resp } else { resp.on_hover_text("Offline: turn the device on or move it to wake it up") };
                    if resp.clicked() {
                        self.selected = Some(d.id.clone());
                    }
                }
            }
            if !self.problems.is_empty() {
                ui.add_space(12.0);
                ui.separator();
                ui.label(RichText::new("⚠ Some devices can't be used").color(ui.visuals().warn_fg_color));
                for p in &self.problems {
                    ui.label(RichText::new(p).small());
                }
                ui.label(RichText::new("The package installs /usr/lib/udev/rules.d/70-logiset.rules; after installing, replug the device or log in again.").small().weak());
            }
        });
    }

    fn device_view(&mut self, ui: &mut egui::Ui) {
        let Some(dev) = self.selected.as_ref().and_then(|id| self.devices.iter().find(|d| &d.id == id)).cloned() else {
            ui.centered_and_justified(|ui| {
                ui.label("Select a device on the left.");
            });
            return;
        };
        let busy = self.busy.contains(&dev.id);

        ui.horizontal(|ui| {
            ui.heading(format!("{} {}", dev.kind.icon(), dev.name));
            ui.label(RichText::new(dev.connection_label()).weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if busy {
                    ui.spinner();
                }
                if dev.online && ui.add_enabled(!busy, egui::Button::new("Refresh")).on_hover_text("Read all settings from the device again").clicked() {
                    self.send(Command::Refresh(dev.id.clone()));
                }
            });
        });

        if !dev.online {
            ui.add_space(8.0);
            ui.label("This device is paired but not connected. Turn it on, or move it, and it will appear here.");
            return;
        }

        let tabs: Vec<Tab> = [
            (Tab::Overview, true),
            (Tab::Pointer, dev.pointer.is_some()),
            (Tab::Profiles, dev.onboard.is_some()),
            (Tab::Lighting, dev.lighting.is_some()),
            (Tab::Keyboard, dev.keyboard.is_some()),
            (Tab::Scrolling, dev.wheel.is_some()),
        ]
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(t, _)| t)
        .collect();
        if !tabs.contains(&self.tab) {
            self.tab = Tab::Overview;
        }
        ui.horizontal(|ui| {
            for t in &tabs {
                ui.selectable_value(&mut self.tab, *t, t.label());
            }
        });
        ui.separator();

        let mut actions: Vec<Action> = Vec::new();
        let drafts = self.drafts.entry(dev.id.clone()).or_default();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_enabled_ui(!busy, |ui| match self.tab {
                Tab::Overview => {
                    ui::overview::show(ui, &dev);
                    saved_settings(ui, &dev, &self.commands);
                }
                Tab::Pointer => slice(ui, &dev.pointer, |ui, s| {
                    let mut out = Vec::new();
                    ui::pointer::show(ui, &dev, s, &mut drafts.pointer, &mut out);
                    actions.extend(out.into_iter().map(Action::Pointer));
                }),
                Tab::Lighting => slice(ui, &dev.lighting, |ui, s| {
                    let mut out = Vec::new();
                    ui::lighting::show(ui, &dev, s, &mut drafts.lighting, &mut out);
                    actions.extend(out.into_iter().map(Action::Lighting));
                }),
                Tab::Profiles => slice(ui, &dev.onboard, |ui, s| {
                    let mut out = Vec::new();
                    ui::onboard::show(ui, &dev, s, &mut drafts.onboard, &mut out);
                    actions.extend(out.into_iter().map(Action::Onboard));
                }),
                Tab::Keyboard => slice(ui, &dev.keyboard, |ui, s| {
                    let mut out = Vec::new();
                    ui::keyboard::show(ui, &dev, s, &mut drafts.keyboard, &mut out);
                    actions.extend(out.into_iter().map(Action::Keyboard));
                }),
                Tab::Scrolling => slice(ui, &dev.wheel, |ui, s| {
                    let mut out = Vec::new();
                    ui::wheel::show(ui, &dev, s, &mut drafts.wheel, &mut out);
                    actions.extend(out.into_iter().map(Action::Wheel));
                }),
            });
        });
        for action in actions {
            self.send(Command::Apply(dev.id.clone(), action));
        }
    }
}

fn slice<T>(ui: &mut egui::Ui, slot: &Slot<T>, body: impl FnOnce(&mut egui::Ui, &T)) {
    match slot {
        Some(Ok(state)) => body(ui, state),
        Some(Err(e)) => {
            ui.label(RichText::new(format!("Could not read this from the device: {e}")).color(ui.visuals().error_fg_color));
        }
        None => {
            ui.label("Not supported by this device.");
        }
    }
}

fn saved_settings(ui: &mut egui::Ui, dev: &DeviceState, commands: &Sender<Command>) {
    ui.add_space(12.0);
    ui.heading("Saved settings");
    if dev.saved.is_empty() {
        ui.label(RichText::new("Nothing saved yet. Settings you change that the device forgets on power-off are remembered here.").weak());
        return;
    }
    ui.label(RichText::new("Re-applied whenever this device connects.").weak());
    egui::Grid::new("saved").num_columns(2).striped(true).show(ui, |ui| {
        for (key, text) in &dev.saved {
            ui.label(text);
            if ui.small_button("Forget").clicked() {
                let _ = commands.send(Command::Forget(dev.id.clone(), key.clone()));
            }
            ui.end_row();
        }
    });
    if ui.button("Apply saved settings now").clicked() {
        let _ = commands.send(Command::ApplySaved(dev.id.clone()));
    }
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            self.status_bar(ui);
        });
        egui::Panel::left("devices").resizable(true).default_size(250.0).min_size(180.0).show(ui, |ui| {
            ui.add_space(4.0);
            self.device_list(ui);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            self.device_view(ui);
        });
    }
}
