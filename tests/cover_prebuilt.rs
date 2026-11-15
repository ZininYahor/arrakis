//! The cover's images built on a thread, as the process's slot hands them to
//! a window's first frame. The slot and the cover's counters are process-wide
//! statics. This is the only test in its binary, so what they hold between
//! two steps is its own doing.

mod common;

use arrakis::probe;
use arrakis::settings::Settings;
use arrakis::ui::cover::counts;
use common::Window;
use fastframe_shell::Resident;

/// A window on a 2x display, before its first frame.
fn window() -> Window {
    Window::new(probe::app(Settings::default()))
}

/// What the cover has cost since `start`: the builds, the bytes sent to the
/// GPU, and the builds a first frame found done already.
fn since(start: (counts::CoverCounts, u64)) -> (u64, u64, u64) {
    let (counts, used) = start;
    let now = counts::read();
    (
        now.layer_builds - counts.layer_builds,
        now.uploaded_bytes - counts.uploaded_bytes,
        counts::prebuilt_used() - used,
    )
}

fn now() -> (counts::CoverCounts, u64) {
    (counts::read(), counts::prebuilt_used())
}

#[test]
fn a_first_frame_takes_the_images_the_thread_built_and_a_window_that_is_gone_leaves_none() {
    probe::set_clock(12.0 * 60.0);
    let bytes = common::cover_bytes(272);
    let night = Settings::default().night.kelvin;
    let wanted = probe::cover_images(probe::cover_layers(272), night);

    // Nothing asked for: the first frame builds inline, and sends the six
    // images of a cover 272 pixels square, the rim's light and the shade for
    // the window's night look among them.
    let start = now();
    let mut inline = window();
    let inline_textures = inline.frame_textures(Vec::new());
    assert_eq!(since(start), (1, bytes, 0));
    assert!(
        inline_textures == wanted,
        "the inline images are the cover's"
    );

    // The thread built them first: the first frame takes them, and sends the
    // same bytes, which are the same pixels.
    probe::prebuild_cover(2.0);
    let start = now();
    let mut prebuilt = window();
    let textures = prebuilt.frame_textures(Vec::new());
    assert_eq!(since(start), (1, bytes, 1));
    assert!(
        textures == inline_textures,
        "the prebuilt images are the inline ones"
    );

    // Frames at the same size build and take nothing more.
    for _ in 0..3 {
        assert!(prebuilt.frame_textures(Vec::new()).is_empty());
    }
    assert_eq!(since(start), (1, bytes, 1));

    // The window is gone, and the next one is asked for by no one: its first
    // frame builds inline.
    prebuilt.app.window_gone();
    let start = now();
    let textures = window().frame_textures(Vec::new());
    assert_eq!(since(start), (1, bytes, 0));
    assert!(textures == wanted, "the inline images are the cover's");

    // A build left in the slot, and then the window gone: the next first
    // frame does not use it.
    probe::prebuild_cover(2.0);
    prebuilt.app.window_gone();
    let start = now();
    let textures = window().frame_textures(Vec::new());
    assert_eq!(since(start), (1, bytes, 0));
    assert!(textures == wanted, "the inline images are the cover's");

    // A build for another size is no use to a window: the one on a display
    // of 1.5x builds its own 204 pixels. The slot is empty after it.
    probe::prebuild_cover(2.0);
    let start = now();
    let mut small = window();
    small.scale = 1.5;
    small.frame_textures(Vec::new());
    assert_eq!(since(start), (1, common::cover_bytes(204), 0));
    let start = now();
    window().frame_textures(Vec::new());
    assert_eq!(since(start), (1, bytes, 0), "the miss emptied the slot");
}
