//! The driver's app never touches the displays: its engine paints
//! `NullScreen`. This is the only test in its binary, so the screen counters
//! it reads are its own.

use std::time::{Duration, Instant};

use arrakis::app::{NullScreen, Signals, screen_counts};
use arrakis::color::Look;
use arrakis::drive::{self, Scenario};
use arrakis::settings::Settings;

#[test]
fn the_drivers_app_paints_the_null_screen() {
    assert_eq!(
        std::any::type_name_of_val(&drive::screen()),
        std::any::type_name::<NullScreen>()
    );
    let before = screen_counts::read();
    // The real runs' look, which the eye cannot tell from none: were the
    // engine ever handed a real display here, the owner would not see it.
    let settings = Settings {
        night: Look {
            kelvin: 6490,
            brightness: 100,
        },
        ..Settings::default()
    };
    // 23:30: the night, tinted at rest, first checked 2 s after its paint.
    let mut app = drive::app(
        Scenario::ClosedNight,
        settings,
        None,
        Signals::default(),
        |_| {},
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while (app.status.level < 0.999 || screen_counts::read().checks == before.checks)
        && Instant::now() < deadline
    {
        app.background_frame(None);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(app.status.level >= 0.999, "the night: {:?}", app.status);
    let tinted = screen_counts::read();
    assert!(tinted.paints > before.paints, "painted: {tinted:?}");
    assert!(tinted.checks > before.checks, "checked: {tinted:?}");
    drop(app);
    let after = screen_counts::read();
    assert!(after.restores > tinted.restores, "restored: {after:?}");
}
