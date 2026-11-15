//! A window asked for from the menu bar starts the cover's build on its own.
//! The slot and the counters are process-wide statics, so this is the only
//! test in its binary.
//!
//! Nothing here can look at the slot without taking from it, so the test asks
//! for a window and then tries a first frame, with longer waits between the
//! tries until one finds the images done. A first frame that comes too early
//! cancels the build and fails that try, never the test: only a build that
//! no wait of up to a minute in all ever finds done does.

mod common;

use std::time::{Duration, Instant};

use arrakis::model::Action;
use arrakis::probe;
use arrakis::settings::Settings;
use arrakis::ui::cover::counts;
use common::Window;
use fastframe_shell::{Headless, Resident};

#[test]
fn a_window_asked_for_from_the_menu_bar_builds_the_cover_for_the_last_size() {
    probe::set_clock(12.0 * 60.0);

    // A first window at 2x builds inline and leaves its size behind. The
    // build is for that size: the main screen's scale is unknown here, since
    // AppKit is not for this thread.
    Window::new(probe::app(Settings::default())).frame(Vec::new());
    assert_eq!(counts::prebuilt_used(), 0);

    // The menu-bar item was clicked: the app asks for a window and starts
    // the build.
    let mut app = probe::app(Settings::default());
    let ctx = egui::Context::default();
    let give_up = Instant::now() + Duration::from_secs(60);
    let mut wait = Duration::from_millis(50);
    loop {
        app.apply(Action::ShowWindow, None);
        assert_eq!(app.headless_frame(&ctx), Headless::Show);
        std::thread::sleep(wait);

        let before = counts::prebuilt_used();
        Window::new(probe::app(Settings::default())).frame(Vec::new());
        if counts::prebuilt_used() == before + 1 {
            break;
        }
        assert!(
            Instant::now() < give_up,
            "no first frame found the build done, after waits up to {wait:?}"
        );
        wait = (wait * 2).min(Duration::from_secs(5));
    }
}
