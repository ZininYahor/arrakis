//! The cover's counters are process-wide statics. This is the only test in
//! its binary, so what they count between two reads is its own doing.

mod common;

use arrakis::probe;
use arrakis::settings::Settings;
use arrakis::ui::cover::counts;
use common::Window;

#[test]
fn the_layers_are_built_once_for_each_size_the_cover_is_drawn_at() {
    probe::set_clock(12.0 * 60.0);
    let start = counts::read();
    let mut window = Window::new(probe::app(Settings::default()));

    // On a 2x display the cover is 272 pixels square.
    window.frame(Vec::new());
    let at_2x = counts::read();
    assert_eq!(at_2x.layer_builds - start.layer_builds, 1);
    assert_eq!(
        at_2x.uploaded_bytes - start.uploaded_bytes,
        common::cover_bytes(272)
    );

    // The window moves to a display of 1.5x, where the cover is 204 pixels
    // square: one more build, and the images for that size.
    window.scale = 1.5;
    window.frame(Vec::new());
    let at_1_5x = counts::read();
    assert_eq!(at_1_5x.layer_builds - at_2x.layer_builds, 1);
    assert_eq!(
        at_1_5x.uploaded_bytes - at_2x.uploaded_bytes,
        common::cover_bytes(204)
    );

    // Frames at the same size build nothing more.
    for _ in 0..3 {
        window.frame(Vec::new());
    }
    assert_eq!(counts::read(), at_1_5x);
}
