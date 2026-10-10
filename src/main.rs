mod app;
mod backend;
mod config;
mod features;
mod hid_usage;
mod hidpp;
mod hidraw;
#[cfg(test)]
mod hwtest;
mod model;
mod ui;
mod util;

use std::process::ExitCode;

use backend::{Backend, Event, Mode};

const HELP: &str = "\
logiset — configure Logitech HID++ mice and keyboards

Usage:
  logiset              open the settings window
  logiset list         print detected devices and their current settings (read-only)
  logiset apply        re-apply saved settings to connected devices
  logiset apply --watch
                       keep running and re-apply whenever a device connects
                       (honours \"Re-apply saved settings on connect\")
  logiset --version
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => gui(),
        ["list"] => list(),
        ["apply"] => apply(false),
        ["apply", "--watch"] => apply(true),
        ["-V" | "--version"] => {
            println!("logiset {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ["-h" | "--help" | "help"] => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{HELP}");
            ExitCode::from(2)
        }
    }
}

fn gui() -> ExitCode {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("logiset")
            .with_app_id("logiset")
            .with_inner_size([960.0, 680.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    match eframe::run_native("logiset", options, Box::new(|cc| Ok(Box::new(app::App::new(cc))))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("logiset: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_event(e: Event) {
    match e {
        Event::Status(s) => println!("{s}"),
        Event::Error(s) => eprintln!("error: {s}"),
        Event::Problems(p) => p.iter().for_each(|p| eprintln!("warning: {p}")),
        _ => {}
    }
}

fn list() -> ExitCode {
    use std::fmt::Write as _;
    use std::io::Write as _;

    let mut backend = Backend::new(Mode::ReadOnly, Box::new(print_event));
    backend.scan(false);
    let devices = backend.devices();
    let mut out = String::new();
    if devices.is_empty() {
        out.push_str("No Logitech HID++ devices found.\n");
    }
    for d in devices {
        let _ = writeln!(out, "{} {} [{}]", d.kind.icon(), d.name, if d.online { "online" } else { "offline" });
        let _ = writeln!(out, "  node        {} index 0x{:02X} ({})", d.path.display(), d.index, d.connection_label());
        if d.online {
            let _ = writeln!(out, "  protocol    HID++ {}.{}", d.protocol.0, d.protocol.1);
        }
        if let Some(unit) = d.unit_id {
            let _ = writeln!(out, "  unit id     {unit:08X}");
        }
        if !d.model_ids.is_empty() {
            let ids: Vec<String> = d.model_ids.iter().map(|m| format!("{m:04X}")).collect();
            let _ = writeln!(out, "  model ids   {}", ids.join(" "));
        }
        for fw in &d.firmware {
            let _ = writeln!(out, "  firmware    {} {} {}", fw.kind, fw.name, fw.version);
        }
        if !d.features.is_empty() {
            let f: Vec<String> = d.features.iter().map(|f| format!("{:04X}", f.id)).collect();
            let _ = writeln!(out, "  features    {}", f.join(" "));
        }
        let slices: [(&str, String); 6] = [
            ("battery", format!("{:?}", d.battery)),
            ("pointer", format!("{:?}", d.pointer)),
            ("lighting", format!("{:?}", d.lighting)),
            ("onboard", format!("{:?}", d.onboard.as_ref().map(|r| r.as_ref().map(features::onboard::summary)))),
            ("keyboard", format!("{:?}", d.keyboard)),
            ("wheel", format!("{:?}", d.wheel)),
        ];
        for (name, value) in slices {
            if value != "None" {
                let _ = writeln!(out, "  {name:<11} {value}");
            }
        }
        for (_, text) in &d.saved {
            let _ = writeln!(out, "  saved       {text}");
        }
        out.push('\n');
    }
    // A closed pipe (`logiset list | head`) is not an error worth a panic.
    let _ = std::io::stdout().lock().write_all(out.as_bytes());
    ExitCode::SUCCESS
}

fn apply(watch: bool) -> ExitCode {
    let mode = if watch { Mode::Watch } else { Mode::Apply };
    let mut backend = Backend::new(mode, Box::new(print_event));
    backend.scan(true);
    if watch {
        backend.watch();
    }
    ExitCode::SUCCESS
}
