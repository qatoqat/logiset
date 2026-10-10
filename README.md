# logiset

A small, pure-Rust settings app for Logitech mice and keyboards on Linux, built with [egui](https://github.com/emilk/egui). It talks HID++ 2.0 directly over `/dev/hidraw*`: no daemon, no C libraries, and no G HUB.

- **Pointer:** DPI per sensor, with presets, and polling rate (125–8000 Hz)
- **Onboard profiles:** switch between onboard and host mode, pick the active profile, and edit a profile's name, report rate, DPI table, default/sniper DPI and button bindings. Buttons can be mouse buttons, keys with modifiers, media keys, or DPI/profile functions. Every write is backed up first and verified afterwards.
- **Lighting:** per-zone LED effects (static, breathe, cycle, wave, ripple …) with colour, period and intensity
- **Keyboard:** which keys Game Mode blocks, Fn-key inversion, and disabling Caps/Num/Scroll Lock, Insert or Super
- **Scrolling:** SmartShift ratchet and threshold, and wheel inversion
- **Battery:** level and charging state, shown in the device list
- **Receivers:** Unifying, Lightspeed, Bolt and Nano receivers, plus devices on a USB cable or Bluetooth. A mouse that is charging over its cable shows up once, not twice.
- **Remembers settings** the device forgets on power-off (host-mode DPI, lighting, game-mode keys …) and re-applies them when the device connects

Tested on a **G502 LIGHTSPEED** (cable and Lightspeed receiver) and a **G610** keyboard. Other HID++ 2.0 devices that expose the same features should work too. SmartShift and Fn-inversion follow [Solaar](https://github.com/pwr-Solaar/Solaar)'s byte layouts but have not been tried on real hardware yet.

## Install

### Arch Linux

```sh
git clone https://github.com/qatoqat/logiset
cd logiset/packaging/arch
makepkg -si          # or: paru -Bi .
```

The package installs `/usr/lib/udev/rules.d/70-logiset.rules`, which gives your login session access to the HID++ interfaces of Logitech devices. Pacman reloads the udev rules for you; if a device still says *no permission*, replug it.

### Other distros

```sh
cargo build --release
sudo install -Dm755 target/release/logiset /usr/local/bin/logiset
sudo install -Dm644 data/70-logiset.rules /etc/udev/rules.d/70-logiset.rules
sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=hidraw
```

## Use

```text
logiset                 open the settings window
logiset list            print detected devices and their current settings (read-only)
logiset apply           re-apply saved settings to connected devices
logiset apply --watch   keep running and re-apply whenever a device connects
```

To keep settings applied after reconnects without the window open:

```sh
systemctl --user enable --now logiset.service
```

Saved settings live in `~/.config/logiset/config.json`. Backups of onboard-profile memory, taken before every write, are stored in `~/.local/share/logiset/backups/`. You can restore them from the *Onboard profiles* tab.

### Onboard vs host mode

Gaming mice such as the G502 run from **onboard profiles** by default. In this mode the mouse stores DPI, polling rate, buttons and lighting itself, and changes made from the Pointer or Lighting tab only last until the profile changes. You have two options:

- edit the profile on the *Onboard profiles* tab and write it to the mouse, which makes the change permanent and travels with the mouse, or
- switch to **host mode**, where logiset owns the settings and re-applies them each time the mouse connects.

### keyledsd

If [keyleds](https://github.com/keyleds/keyleds) is running, it drives the keyboard's per-key lighting and will override lighting changes made here. Everything else works alongside it.

## How it works

- `src/hidraw.rs`: finds Logitech hidraw nodes in sysfs, works out from the report descriptor which ones speak HID++, and reads reports on a background thread
- `src/hidpp/`: the HID++ 1.0 register and 2.0 feature transport, device identity, and receiver pairing
- `src/features/`: one module per feature group, each with a read-only `probe` and an `apply`
- `src/backend.rs`: a worker thread that owns the devices and handles hot-plug, receiver connect/disconnect and re-applying saved settings
- `src/app.rs`, `src/ui/`: the egui interface

Protocol details were ported from [Solaar](https://github.com/pwr-Solaar/Solaar), [libratbag](https://github.com/libratbag/libratbag) and [keyleds](https://github.com/keyleds/keyleds).

## License

GPL-2.0-or-later
