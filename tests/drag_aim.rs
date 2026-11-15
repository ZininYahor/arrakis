//! The driver's aim at the sliders: the rows and track ends that
//! `--demo-drive drag` and the benchmarks press and sweep are the ones
//! `ui::view` lays out. They are derived from it, and these tests would fail
//! all the same were a slider moved without them: a press on empty space
//! moves nothing, and the driver would still write a report.

mod common;

use arrakis::color::{DAY_KELVIN, Look, MIN_BRIGHTNESS};
use arrakis::drive::{BRIGHTNESS_Y, TEMPERATURE_Y, TRACK};
use arrakis::probe;
use arrakis::settings::Settings;
use common::Window;
use egui::{Event, Modifiers, PointerButton, pos2};

/// A window at noon with a pointer pressed at `from` on the row `y`, and
/// moved to `to`, as a drag does. The frames before the press are the ones
/// egui hit-tests it against.
fn dragged(y: f32, from: f32, to: f32) -> (Window, Look) {
    probe::set_clock(12.0 * 60.0);
    let mut window = Window::new(probe::app(Settings::default()));
    let before = window.app.settings.night;
    for _ in 0..3 {
        window.frame(Vec::new());
    }
    window.frame(vec![
        Event::PointerMoved(pos2(from, y)),
        Event::PointerButton {
            pos: pos2(from, y),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    window.frame(vec![Event::PointerMoved(pos2(to, y))]);
    (window, before)
}

#[test]
fn a_press_at_the_temperature_rows_day_end_reaches_both_ends_of_its_track() {
    let (window, before) = dragged(TEMPERATURE_Y, TRACK.0, TRACK.1);
    let night = window.app.settings.night;
    assert!(night.is_pure_red(), "the far end is pure red: {night:?}");
    assert_eq!(
        night.brightness, before.brightness,
        "only Temperature moved"
    );

    let (window, _) = dragged(TEMPERATURE_Y, TRACK.1, TRACK.0);
    let night = window.app.settings.night;
    assert_eq!(night.kelvin, DAY_KELVIN, "the Day end: {night:?}");
    assert_eq!(
        night.brightness, before.brightness,
        "only Temperature moved"
    );
}

#[test]
fn a_press_at_the_brightness_row_moves_brightness_alone() {
    let (window, before) = dragged(BRIGHTNESS_Y, TRACK.1, TRACK.0);
    let night = window.app.settings.night;
    assert_eq!(night.brightness, MIN_BRIGHTNESS, "{night:?}");
    assert_eq!(night.kelvin, before.kelvin, "Temperature stays: {night:?}");
}

#[test]
fn a_press_beside_the_sliders_moves_nothing() {
    // The control for the two above: the same drag, a row too far from
    // either slider for the press to land on one.
    let (window, before) = dragged((TEMPERATURE_Y + BRIGHTNESS_Y) / 2.0, TRACK.1, TRACK.0);
    assert_eq!(window.app.settings.night, before);
}
