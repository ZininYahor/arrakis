//! `NullScreen` counts what the engine asks of it, in process-wide statics.
//! This is the only test in its binary, and it starts no engine, so what the
//! counters hold between two reads is its own doing.

use arrakis::app::{NullScreen, screen_counts};
use arrakis::color::Look;
use arrakis::engine::Screen;

#[test]
fn each_call_to_the_null_screen_counts_in_its_own_counter() {
    let mut screen: Box<dyn Screen> = Box::new(NullScreen);
    let start = screen_counts::read();

    let tint = Look::default().tint();
    assert_eq!(screen.apply(tint), Ok(1));
    let painted = screen_counts::read();
    assert_eq!(
        painted,
        screen_counts::ScreenCounts {
            paints: start.paints + 1,
            ..start
        }
    );

    assert!(screen.intact());
    let checked = screen_counts::read();
    assert_eq!(
        checked,
        screen_counts::ScreenCounts {
            checks: painted.checks + 1,
            ..painted
        }
    );

    screen.restore();
    let restored = screen_counts::read();
    assert_eq!(
        restored,
        screen_counts::ScreenCounts {
            restores: checked.restores + 1,
            ..checked
        }
    );

    // Asking how many displays there are is not one of the three.
    assert_eq!(screen.display_count(), 1);
    assert_eq!(screen_counts::read(), restored);
}
