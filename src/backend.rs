//! Worker that owns every open device. The UI sends [`Command`]s and receives
//! [`Event`]s, so HID I/O (which can take seconds for onboard memory) never blocks
//! a frame. The same type drives the `list` and `apply` command-line modes.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::features::{battery, keyboard, lighting, onboard, pointer, wheel};
use crate::hidpp::info::{self, Kind};
use crate::hidpp::{self, DIRECT, Device, Error, Transport, receiver};
use crate::hidraw::{self, NodeInfo};
use crate::model::{Action, DeviceState, Slot};

pub enum Command {
    /// Forget everything and probe all devices again.
    Rescan,
    /// Re-read every setting of one device.
    Refresh(String),
    Apply(String, Action),
    /// Re-apply the saved settings of one device now.
    ApplySaved(String),
    /// Drop one saved setting (device id, persist key).
    Forget(String, String),
    SetAutoApply(bool),
}

pub enum Event {
    /// The full, ordered device list.
    Devices(Vec<DeviceState>),
    /// One device changed.
    Device(DeviceState),
    Busy(String, bool),
    Status(String),
    Error(String),
    /// Nodes that could not be used (usually permissions).
    Problems(Vec<String>),
    AutoApply(bool),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Re-apply saved settings on connect only when the user enabled it.
    Gui,
    /// Never write anything.
    ReadOnly,
    /// Always re-apply saved settings on connect.
    Apply,
}

struct Entry {
    state: DeviceState,
    dev: Option<Device>,
    transport: Arc<Transport>,
    /// Pairing serial (receiver slots) or unit ID (online devices); the same device
    /// on cable and receiver shares it.
    serial: Option<u32>,
}

pub struct Backend {
    mode: Mode,
    entries: Vec<Entry>,
    transports: HashMap<PathBuf, Arc<Transport>>,
    config: Config,
    sink: Box<dyn FnMut(Event) + Send>,
    problems: Vec<String>,
    nodes: Vec<PathBuf>,
    last_node_check: Instant,
    last_offline_poll: Instant,
    last_battery: Instant,
}

pub fn spawn(repaint: impl Fn() + Send + 'static) -> (Sender<Command>, Receiver<Event>) {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (ev_tx, ev_rx) = mpsc::channel();
    thread::Builder::new()
        .name("backend".into())
        .spawn(move || {
            let sink = move |e| {
                let _ = ev_tx.send(e);
                repaint();
            };
            Backend::new(Mode::Gui, Box::new(sink)).run(cmd_rx);
        })
        .expect("spawning backend thread");
    (cmd_tx, ev_rx)
}

fn receiver_name(node: &NodeInfo) -> String {
    match node.product {
        0xC52B | 0xC532 => "Unifying Receiver".into(),
        0xC539 | 0xC53A | 0xC53D | 0xC53F | 0xC541 | 0xC545 | 0xC547 | 0xC54D => "Lightspeed Receiver".into(),
        0xC548 => "Bolt Receiver".into(),
        0xC52F | 0xC526 | 0xC52E | 0xC534 | 0xC53E | 0xC542 => "Nano Receiver".into(),
        _ => node.name.trim_start_matches("Logitech ").to_owned(),
    }
}

fn slot<T>(r: Option<hidpp::Result<T>>) -> Slot<T> {
    r.map(|r| r.map_err(|e| e.to_string()))
}

fn run<T>(slot: &mut Slot<T>, f: impl FnOnce(&mut T) -> hidpp::Result<()>) -> Result<(), String> {
    match slot {
        Some(Ok(state)) => f(state).map_err(|e| e.to_string()),
        Some(Err(e)) => Err(format!("feature unavailable: {e}")),
        None => Err("the device does not support this".into()),
    }
}

fn blank(id: String, path: PathBuf, index: u8, receiver: Option<String>) -> DeviceState {
    DeviceState {
        id,
        config_key: String::new(),
        name: String::new(),
        kind: Kind::Other,
        online: false,
        path,
        index,
        receiver,
        wpid: None,
        protocol: (0, 0),
        unit_id: None,
        model_ids: Vec::new(),
        firmware: Vec::new(),
        serial: None,
        features: Vec::new(),
        battery: None,
        pointer: None,
        lighting: None,
        onboard: None,
        keyboard: None,
        wheel: None,
        saved: Vec::new(),
    }
}

/// Read every slice. Read-only.
pub fn probe_all(dev: &Device, state: &mut DeviceState) {
    state.battery = slot(battery::probe(dev));
    state.pointer = slot(pointer::probe(dev));
    state.lighting = slot(lighting::probe(dev));
    state.onboard = slot(onboard::probe(dev));
    state.keyboard = slot(keyboard::probe(dev));
    state.wheel = slot(wheel::probe(dev));
}

impl Backend {
    pub fn new(mode: Mode, sink: Box<dyn FnMut(Event) + Send>) -> Backend {
        let now = Instant::now();
        Backend {
            mode,
            entries: Vec::new(),
            transports: HashMap::new(),
            config: Config::load(),
            sink,
            problems: Vec::new(),
            nodes: Vec::new(),
            last_node_check: now,
            last_offline_poll: now,
            last_battery: now,
        }
    }

    pub fn devices(&self) -> Vec<DeviceState> {
        self.entries.iter().filter(|e| self.visible(e)).map(|e| e.state.clone()).collect()
    }

    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    pub fn run(mut self, commands: Receiver<Command>) {
        (self.sink)(Event::AutoApply(self.config.auto_apply));
        self.scan();
        loop {
            match commands.recv_timeout(Duration::from_millis(500)) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.tick();
        }
    }

    /// Keep watching for connects without a UI (`logiset apply --watch`).
    pub fn watch(mut self) -> ! {
        loop {
            thread::sleep(Duration::from_millis(500));
            self.tick();
        }
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Rescan => {
                self.entries.clear();
                self.transports.clear();
                self.scan();
            }
            Command::Refresh(id) => {
                if let Some(i) = self.find(&id) {
                    self.busy(&id, true);
                    let entry = &mut self.entries[i];
                    if let Some(dev) = &entry.dev {
                        probe_all(dev, &mut entry.state);
                    }
                    self.after_io(i);
                    self.busy(&id, false);
                }
            }
            Command::Apply(id, action) => {
                if let Some(i) = self.find(&id) {
                    self.busy(&id, true);
                    let result = self.apply(i, &action);
                    self.report(i, &action, result, true);
                    self.after_io(i);
                    self.busy(&id, false);
                }
            }
            Command::ApplySaved(id) => {
                if let Some(i) = self.find(&id) {
                    self.busy(&id, true);
                    self.replay(i);
                    self.after_io(i);
                    self.busy(&id, false);
                }
            }
            Command::Forget(id, key) => {
                if let Some(i) = self.find(&id) {
                    let config_key = self.entries[i].state.config_key.clone();
                    self.config.forget(&config_key, &key);
                    self.save_config();
                    self.entries[i].state.saved = self.saved_list(&config_key);
                    let state = self.entries[i].state.clone();
                    (self.sink)(Event::Device(state));
                }
            }
            Command::SetAutoApply(on) => {
                self.config.auto_apply = on;
                self.save_config();
                (self.sink)(Event::AutoApply(on));
            }
        }
    }

    fn find(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.state.id == id)
    }

    fn busy(&mut self, id: &str, on: bool) {
        (self.sink)(Event::Busy(id.to_owned(), on));
    }

    fn save_config(&mut self) {
        if let Err(e) = self.config.save() {
            (self.sink)(Event::Error(e));
        }
    }

    fn saved_list(&self, config_key: &str) -> Vec<(String, String)> {
        self.config.actions(config_key).into_iter().map(|(k, a)| (k, a.describe())).collect()
    }

    /// An offline receiver slot is hidden while the same device is online elsewhere
    /// (typically charging over its cable).
    fn visible(&self, e: &Entry) -> bool {
        e.state.online
            || e.serial.is_none()
            || !self.entries.iter().any(|o| o.state.online && o.serial == e.serial)
    }

    fn emit_devices(&mut self) {
        let devices = self.devices();
        (self.sink)(Event::Devices(devices));
    }

    /// After I/O on entry `i`: notice disconnects and publish the new state.
    fn after_io(&mut self, i: usize) {
        if !self.entries[i].transport.is_alive() {
            self.scan();
            return;
        }
        let state = self.entries[i].state.clone();
        (self.sink)(Event::Device(state));
    }

    fn apply(&mut self, i: usize, action: &Action) -> Result<(), String> {
        let entry = &mut self.entries[i];
        let Some(dev) = &entry.dev else {
            return Err("device is offline".into());
        };
        let st = &mut entry.state;
        let result = match action {
            Action::Pointer(a) => run(&mut st.pointer, |s| pointer::apply(dev, s, a)),
            Action::Lighting(a) => run(&mut st.lighting, |s| lighting::apply(dev, s, a)),
            Action::Onboard(a) => run(&mut st.onboard, |s| onboard::apply(dev, s, a)),
            Action::Keyboard(a) => run(&mut st.keyboard, |s| keyboard::apply(dev, s, a)),
            Action::Wheel(a) => run(&mut st.wheel, |s| wheel::apply(dev, s, a)),
        };
        if matches!(action, Action::Onboard(_)) {
            // Mode and profile switches change what the pointer and LEDs report.
            st.pointer = slot(pointer::probe(dev));
            st.lighting = slot(lighting::probe(dev));
        }
        result
    }

    fn report(&mut self, i: usize, action: &Action, result: Result<(), String>, remember: bool) {
        let name = self.entries[i].state.name.clone();
        match result {
            Ok(()) => {
                if remember && action.persist_key().is_some() {
                    let key = self.entries[i].state.config_key.clone();
                    self.config.remember(&key, &name, action);
                    self.save_config();
                    self.entries[i].state.saved = self.saved_list(&key);
                }
                (self.sink)(Event::Status(format!("{name}: {}", action.describe())));
            }
            Err(e) => (self.sink)(Event::Error(format!("{name}: {}: {e}", action.describe()))),
        }
    }

    fn replay(&mut self, i: usize) {
        let key = self.entries[i].state.config_key.clone();
        for (_, action) in self.config.actions(&key) {
            let result = self.apply(i, &action);
            self.report(i, &action, result, false);
        }
    }

    fn should_auto_apply(&self) -> bool {
        match self.mode {
            Mode::Gui => self.config.auto_apply,
            Mode::ReadOnly => false,
            Mode::Apply => true,
        }
    }

    /// Identify and probe a freshly reachable device.
    fn connect(&self, dev: Device, transport: Arc<Transport>, receiver: Option<String>, node: &NodeInfo, pairing: Option<&receiver::Paired>) -> Entry {
        let ident = info::identify(&dev);
        let id = format!("{}#{}", transport.path.display(), dev.index);
        let mut state = blank(id, transport.path.clone(), dev.index, receiver);
        state.name = ident
            .name
            .clone()
            .or_else(|| pairing.and_then(|p| p.name.clone()))
            .unwrap_or_else(|| node.name.trim_start_matches("Logitech ").to_owned());
        state.kind = match ident.kind {
            Kind::Other => match pairing {
                Some(p) if p.kind != Kind::Other => p.kind,
                _ if dev.has(0x2201) || dev.has(0x2202) => Kind::Mouse,
                _ if dev.has(0x4522) || dev.has(0x8080) || dev.has(0x40A3) => Kind::Keyboard,
                _ => Kind::Other,
            },
            k => k,
        };
        state.online = true;
        state.wpid = pairing.map(|p| p.wpid);
        state.protocol = dev.protocol;
        state.unit_id = ident.unit_id;
        state.model_ids = ident.model_ids.clone();
        state.firmware = ident.firmware.clone();
        state.serial = ident.serial.clone();
        state.features = dev.features.clone();
        state.config_key = ident.config_key(&format!("{:04x}-{}", node.product, state.name));
        probe_all(&dev, &mut state);
        let serial = ident.unit_id.or_else(|| pairing.and_then(|p| p.serial));
        let mut entry = Entry { state, dev: Some(dev), transport, serial };
        entry.state.saved = self.saved_list(&entry.state.config_key);
        entry
    }

    /// Re-apply saved settings to a device that just connected.
    fn on_connected(&mut self, i: usize) {
        if self.should_auto_apply() && !self.entries[i].state.saved.is_empty() {
            self.replay(i);
        }
    }

    /// Find all devices. Already-probed online devices on unchanged nodes are kept as they are.
    pub fn scan(&mut self) {
        let nodes = hidraw::enumerate();
        self.nodes = nodes.iter().map(|n| n.devnode.clone()).collect();
        self.last_node_check = Instant::now();
        let mut old = std::mem::take(&mut self.entries);
        let mut problems = Vec::new();
        let mut transports = HashMap::new();
        let mut fresh: Vec<Entry> = Vec::new();
        let mut connected = Vec::new();

        for node in nodes.iter().filter(|n| n.reports.any() && !n.behind_receiver) {
            let transport = match self.transports.get(&node.devnode) {
                Some(t) if t.is_alive() => t.clone(),
                _ => match Transport::open(node) {
                    Ok(t) => t,
                    Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                        problems.push(format!(
                            "{}: no permission to open {}. Install the logiset udev rule and replug the device.",
                            node.name,
                            node.devnode.display()
                        ));
                        continue;
                    }
                    Err(e) => {
                        problems.push(format!("{}: {}: {e}", node.name, node.devnode.display()));
                        continue;
                    }
                },
            };
            transports.insert(node.devnode.clone(), transport.clone());

            let indices: Vec<(u8, Option<String>)> = match transport.ping(DIRECT) {
                Ok((major, _)) if major >= 2 => vec![(DIRECT, None)],
                Ok((major, minor)) => {
                    problems.push(format!("{}: HID++ {major}.{minor} devices are not supported", node.name));
                    continue;
                }
                Err(Error::Hidpp10(_)) => {
                    let name = receiver_name(node);
                    (1..=receiver::max_devices(&transport)).map(|s| (s, Some(name.clone()))).collect()
                }
                Err(e) => {
                    problems.push(format!("{} ({}): {e}", node.name, node.devnode.display()));
                    continue;
                }
            };

            for (index, receiver_label) in indices {
                let id = format!("{}#{}", node.devnode.display(), index);
                let previous = old.iter().position(|e| e.state.id == id && Arc::ptr_eq(&e.transport, &transport)).map(|p| old.remove(p));
                if previous.as_ref().is_some_and(|p| p.dev.is_some()) {
                    fresh.extend(previous);
                    continue;
                }
                match Device::open(transport.clone(), index) {
                    Ok(dev) => {
                        let pairing = if index == DIRECT { None } else { receiver::pairing_info(&transport, index) };
                        let entry = self.connect(dev, transport.clone(), receiver_label, node, pairing.as_ref());
                        connected.push(fresh.len());
                        fresh.push(entry);
                    }
                    Err(e) if index == DIRECT => problems.push(format!("{} ({}): {e}", node.name, node.devnode.display())),
                    Err(Error::Hidpp10(0x08)) => {}
                    Err(_) => {
                        if let Some(prev) = previous {
                            fresh.push(prev);
                        } else if let Some(p) = receiver::pairing_info(&transport, index) {
                            let mut state = blank(id, node.devnode.clone(), index, receiver_label);
                            state.name = p.name.clone().unwrap_or_else(|| format!("Paired device {index}"));
                            state.kind = p.kind;
                            state.wpid = Some(p.wpid);
                            state.unit_id = p.serial;
                            fresh.push(Entry { state, dev: None, transport: transport.clone(), serial: p.serial });
                        }
                    }
                }
            }
        }

        self.transports = transports;
        self.entries = fresh;
        if problems != self.problems {
            self.problems = problems.clone();
            (self.sink)(Event::Problems(problems));
        }
        for i in connected {
            self.on_connected(i);
        }
        self.emit_devices();
    }

    /// Bring receiver slot `i` online if the device answers.
    fn try_reconnect(&mut self, i: usize) -> bool {
        let (transport, index, receiver_label, path) = {
            let e = &self.entries[i];
            (e.transport.clone(), e.state.index, e.state.receiver.clone(), e.state.path.clone())
        };
        let Ok(dev) = Device::open(transport.clone(), index) else {
            return false;
        };
        let node = NodeInfo {
            devnode: path,
            bus: hidraw::BUS_USB,
            vendor: hidraw::LOGITECH,
            product: 0,
            name: self.entries[i].state.name.clone(),
            reports: transport.reports,
            behind_receiver: false,
        };
        let pairing = receiver::pairing_info(&transport, index);
        let entry = self.connect(dev, transport, receiver_label, &node, pairing.as_ref());
        self.entries[i] = entry;
        self.on_connected(i);
        true
    }

    fn set_offline(&mut self, i: usize) {
        let e = &mut self.entries[i];
        e.dev = None;
        e.state.online = false;
    }

    /// Periodic housekeeping: hot-plug, receiver connection changes, battery.
    pub fn tick(&mut self) {
        let mut changed = false;

        // Receiver connection notifications.
        let mut events = Vec::new();
        for t in self.transports.values() {
            for report in t.take_notifications() {
                if let Some((slot, online)) = receiver::connection_event(&report) {
                    events.push((t.path.clone(), slot, online));
                }
            }
        }
        for (path, slot, online) in events {
            let id = format!("{}#{}", path.display(), slot);
            match self.find(&id) {
                Some(i) if online && self.entries[i].dev.is_none() => changed |= self.try_reconnect(i),
                Some(i) if !online && self.entries[i].dev.is_some() => {
                    self.set_offline(i);
                    changed = true;
                }
                None if online => {
                    self.scan();
                    return;
                }
                _ => {}
            }
        }

        // Hot-plug: node list changed or a node died.
        if self.last_node_check.elapsed() >= Duration::from_secs(2) {
            self.last_node_check = Instant::now();
            let now: Vec<PathBuf> = hidraw::enumerate().into_iter().map(|n| n.devnode).collect();
            if now != self.nodes || self.transports.values().any(|t| !t.is_alive()) {
                self.scan();
                return;
            }
        }

        // Missed notifications: poll offline receiver slots.
        if self.last_offline_poll.elapsed() >= Duration::from_secs(5) {
            self.last_offline_poll = Instant::now();
            for i in 0..self.entries.len() {
                let e = &self.entries[i];
                if e.dev.is_none() && e.state.index != DIRECT && e.transport.ping(e.state.index).is_ok() {
                    changed |= self.try_reconnect(i);
                }
            }
        }

        if self.last_battery.elapsed() >= Duration::from_secs(60) {
            self.last_battery = Instant::now();
            for i in 0..self.entries.len() {
                let e = &mut self.entries[i];
                if let (Some(dev), Some(_)) = (&e.dev, &e.state.battery) {
                    let fresh = slot(battery::probe(dev));
                    if fresh != e.state.battery {
                        e.state.battery = fresh;
                        let state = e.state.clone();
                        (self.sink)(Event::Device(state));
                    }
                }
            }
        }

        if changed {
            self.emit_devices();
        }
    }
}
