//! The driver's app has the window's haptics, the trackpad's, as the app's
//! does. They play only on the main thread. A drive runs its frames there,
//! inside `eframe::run_native`, so a `--demo-drive drag` plays the detents'
//! ticks within its timed frames. This test makes the driver's app on a
//! thread of its own, where `haptics::play` returns before it reaches AppKit
//! or the actuator (the probe's test counts that), and checks that its sinks
//! return there. This is the only test in its binary: the driver's clock is
//! a static, and its engine paints `NullScreen`.

use std::ptr::fn_addr_eq;

use arrakis::app::{Haptics, Signals};
use arrakis::drive::{self, Scenario};
use arrakis::model::Action;
use arrakis::settings::Settings;

#[test]
fn the_drivers_app_has_the_trackpads_haptics_and_they_play_nothing_off_the_main_thread() {
    // A thread of its own, which is never the main one, whatever
    // `--test-threads` says. The app is made on that thread.
    std::thread::spawn(|| {
        let mut app = drive::app(
            Scenario::ClosedNight,
            Settings::default(),
            None,
            Signals::default(),
            |_| {},
        );
        let want = Haptics::TRACKPAD;
        assert!(fn_addr_eq(app.haptics.align, want.align));
        assert!(fn_addr_eq(app.haptics.thump, want.thump));
        assert!(fn_addr_eq(app.haptics.press, want.press));
        // The actions a drive's pointer pushes as it crosses a detent, rides
        // the thumper and presses a button.
        for action in [Action::Detent, Action::Thump, Action::Press] {
            app.apply(action, None);
        }
        app.actions = vec![Action::Detent, Action::Thump, Action::Press];
        app.apply_actions(None);
        assert!(app.actions.is_empty(), "the app took the actions");
        for sink in [app.haptics.align, app.haptics.thump, app.haptics.press] {
            sink();
        }
    })
    .join()
    .expect("the driver's haptics returned off the main thread");
}
