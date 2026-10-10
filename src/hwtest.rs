//! Hardware round-trip tests. Ignored by default; they change settings on a connected
//! device and put them back. Run with `cargo test hw_ -- --ignored --test-threads=1`.

use crate::features::{lighting, pointer};
use crate::hidpp::{DIRECT, Device, Transport};
use crate::hidraw;

/// The first directly attached HID++ 2.0 device that has `feature`.
fn device_with(feature: u16) -> Option<Device> {
    hidraw::enumerate()
        .into_iter()
        .filter(|n| n.reports.any() && !n.behind_receiver)
        .filter_map(|n| Transport::open(&n).ok())
        .filter_map(|t| Device::open(t, DIRECT).ok())
        .find(|d| d.has(feature))
}

#[test]
#[ignore]
fn hw_dpi_round_trip() {
    let dev = device_with(0x2201).expect("no device with adjustable DPI");
    let mut state = pointer::probe(&dev).unwrap().unwrap();
    let sensor = state.sensors[0].clone();
    let other = if sensor.choices.contains(800) && sensor.current != 800 { 800 } else { 1600 };
    pointer::apply(&dev, &mut state, &pointer::Action::SetDpi { sensor: sensor.index, dpi: other }).unwrap();
    let read = pointer::probe(&dev).unwrap().unwrap().sensors[0].current;
    pointer::apply(&dev, &mut state, &pointer::Action::SetDpi { sensor: sensor.index, dpi: sensor.current }).unwrap();
    let restored = pointer::probe(&dev).unwrap().unwrap().sensors[0].current;
    assert_eq!(read, other, "device did not take the new DPI");
    assert_eq!(restored, sensor.current, "DPI was not restored");
}

#[test]
#[ignore]
fn hw_lighting_effect_then_release() {
    let dev = device_with(0x8070).expect("no device with LED effects");
    let mut state = lighting::probe(&dev).unwrap().unwrap();
    let before = state.sw_control;
    let zone = state.zones.iter().find(|z| z.location == 2).unwrap_or(&state.zones[0]).clone();
    let effect = lighting::Effect { id: 0x01, color: [255, 0, 0], ..Default::default() };
    lighting::apply(&dev, &mut state, &lighting::Action::SetEffect { zone: zone.index, location: zone.location, effect }).unwrap();
    assert_eq!(state.sw_control, Some(true));
    std::thread::sleep(std::time::Duration::from_millis(800));
    lighting::apply(&dev, &mut state, &lighting::Action::SetControl(before.unwrap_or(false))).unwrap();
    let after = lighting::probe(&dev).unwrap().unwrap().sw_control;
    assert_eq!(after, before, "LED control was not handed back");
}
