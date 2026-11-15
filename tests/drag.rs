//! The driver's hand lands on the Temperature slider. The cover's counters
//! are process-wide statics, and this is the only test in its binary.

mod common;

use std::time::{Duration, Instant};

use arrakis::drive::{Drag, FRAMES_BEFORE_PRESS, POINTER_RATE, SWEEP};
use arrakis::probe;
use arrakis::settings::Settings;
use arrakis::ui::cover::counts;
use common::Window;

#[test]
fn the_drag_sweeps_the_temperature_slider_end_to_end() {
    probe::set_clock(12.0 * 60.0);
    let start = counts::read();
    let mut window = Window::new(probe::app(Settings::default()));
    let default = window.app.settings.night.kelvin;
    let mut drag = Drag::default();

    // The window's very first frames, as the driver draws them, for a little
    // longer than one sweep of the track. The driver waits for the frames
    // that egui hit-tests a press against: the first drive run pressed on the
    // window's first frame, and the press landed on nothing.
    //
    // The drag moves once a pointer period (8.3 ms), and `events` reads the
    // real time, so the frames come three periods apart: each is due, and a
    // sleep that runs long cannot make one miss its move. That a frame
    // between two moves brings nothing is pinned in `src/drive.rs`, on a
    // clock the test sets.
    let gap = Duration::from_secs_f64(3.0 / POINTER_RATE);
    let begun = Instant::now();
    let mut kelvins = Vec::new();
    let mut drawn = 0;
    while begun.elapsed() < SWEEP + SWEEP / 4 {
        window.frame(drag.events(drawn));
        kelvins.push(window.app.settings.night.kelvin);
        drawn += 1;
        std::thread::sleep(gap);
    }

    // Nothing moves until the press, which comes after the frames it waits
    // for; then the slider leaves its default.
    let waiting = FRAMES_BEFORE_PRESS as usize;
    assert!(
        kelvins[..=waiting].iter().all(|&kelvin| kelvin == default),
        "moved before the press: {kelvins:?}"
    );
    assert!(
        kelvins.iter().any(|&kelvin| kelvin != default),
        "the press landed on nothing: {kelvins:?}"
    );
    // Over the sweep it reaches both ends of its range: from the Day end,
    // where the track begins, down to the pure red at the other.
    let (coolest, warmest) = (kelvins.iter().min(), kelvins.iter().max());
    assert!(coolest < Some(&1000), "never near pure red: {kelvins:?}");
    assert!(warmest > Some(&5000), "never near the day: {kelvins:?}");

    // The cover followed with its maw's rect, the rim's light and the shade at
    // the sand: its layers
    // were built and its images sent once, however far the slider went.
    let end = counts::read();
    assert_eq!(end.layer_builds - start.layer_builds, 1, "{end:?}");
}
