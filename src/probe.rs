//! Doors into private code paths, for `benches/paths.rs`. Demo builds only,
//! and left out of the docs.
//!
//! From `crate::gamma` it opens the read path alone: listing the displays,
//! reading a table, scaling a copy of it and comparing two. Nothing here
//! writes a table, applies a tint or restores one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use egui::ColorImage;

use crate::app::{App, Haptics, NullScreen, Signals};
use crate::engine::{Engine, Worker};
use crate::settings::Settings;
use crate::ui::cover::{self, Layers};
use crate::ui::view::COVER_SCENE;

/// The probe's clock, in minutes since midnight, as `f64` bits.
static MINUTE: AtomicU64 = AtomicU64::new(0);

/// The clock every app and engine the probe makes reads.
pub fn clock() -> f64 {
    f64::from_bits(MINUTE.load(Ordering::Relaxed))
}

/// Moves the probe's clock to `minute` since midnight.
pub fn set_clock(minute: f64) {
    MINUTE.store(minute.to_bits(), Ordering::Relaxed);
}

/// An app as a window holds it, on the probe's clock: its engine paints a
/// `NullScreen`, and there is no menu-bar item and no settings file. Its
/// haptics are the window's: nothing plays off the main thread, and on it a
/// detent plays the public pattern, felt only under a finger on a Force
/// Touch trackpad (`crate::haptics`, "Who hears it").
pub fn app(settings: Settings) -> App {
    let engine = Engine::spawn(Box::new(NullScreen), settings.clone(), clock, |_| {});
    App::new(
        settings,
        None,
        engine,
        None,
        Signals::default(),
        clock,
        Haptics::TRACKPAD,
    )
}

/// The cover's layers for the window's scene.
pub struct CoverLayers(Layers);

/// Builds the cover's layers, `size` pixels square.
pub fn cover_layers(size: usize) -> CoverLayers {
    CoverLayers(Layers::new(size, COVER_SCENE))
}

/// The images the cover sends to the GPU for its layers, which it takes, at
/// a window's first frame with the night look at `kelvin`: the back, the
/// maw, the gap and the front, each but the back cropped to its own box, and
/// the rim's light and the shade at the sand, each in its box, composed for
/// the maw's rise at `kelvin`.
pub fn cover_images(layers: CoverLayers, kelvin: u16) -> [ColorImage; 6] {
    let images = layers.0.images();
    let rise = cover::rise_of(crate::color::redness_of(kelvin));
    let rim = images.rim.compose(rise);
    let shade = images.shade.compose(rise);
    [
        images.back,
        images.maw.image,
        images.gap.image,
        images.front.image,
        rim,
        shade,
    ]
}

/// The rim's light and the shade at the sand, each in its box, composed for
/// the maw's rise at each of `kelvins`, from one build of the cover's
/// `layers`: one pair for each kelvin, in their order.
pub fn cover_rim_and_shade(layers: CoverLayers, kelvins: &[u16]) -> Vec<[ColorImage; 2]> {
    let images = layers.0.images();
    kelvins
        .iter()
        .map(|&kelvin| {
            let rise = cover::rise_of(crate::color::redness_of(kelvin));
            [images.rim.compose(rise), images.shade.compose(rise)]
        })
        .collect()
}

/// What the app does when a window is asked for, with a main screen of
/// `scale` pixels to the point, and then waits for the thread to finish. The
/// build is for `scale`, whatever size the cover last needed, as the app's
/// is on the main thread. The cover's images are in the slot by the time the
/// window's first frame takes them, as they are in a real window, whose
/// creation takes longer than the build.
pub fn prebuild_cover(scale: f32) {
    if let Some(thread) = crate::ui::cover::prebuilt::window_asked_for(|| Some(scale)) {
        thread.join().expect("the cover's thread ended in a panic");
    }
}

/// Builds the black-body table the color math reads.
pub fn color_table() -> Vec<[f32; 3]> {
    crate::color::build_table()
}

/// The engine's state, driven by hand instead of by its thread.
pub struct EngineWorker(Worker);

/// An engine on a `NullScreen` and the probe's clock.
pub fn engine_worker(settings: Settings) -> EngineWorker {
    EngineWorker(Worker::new(Box::new(NullScreen), settings, clock))
}

/// One step of the engine, `elapsed` after the last; returns how long it
/// would sleep.
pub fn tick(worker: &mut EngineWorker, elapsed: Duration) -> Duration {
    worker.0.tick(elapsed, &mut |_| {})
}

/// [`tick`] at `now` on the engine's own clock, which times its checks of
/// the tables: a bench moves it by hand to make a check due.
pub fn tick_at(worker: &mut EngineWorker, now: Instant, elapsed: Duration) -> Duration {
    worker.0.tick_at(now, elapsed, &mut |_| {})
}

/// One display's color table, as read back.
#[cfg(target_os = "macos")]
pub struct DisplayTable(crate::gamma::Table);

/// The online displays' ids.
#[cfg(target_os = "macos")]
pub fn online_displays() -> Result<Vec<u32>, String> {
    crate::gamma::online_displays()
}

/// Reads `display`'s table.
#[cfg(target_os = "macos")]
pub fn read_table(display: u32) -> Option<DisplayTable> {
    crate::gamma::Table::read(display).map(DisplayTable)
}

/// A copy of `table` scaled by `tint`, as a paint would send it.
#[cfg(target_os = "macos")]
pub fn scaled(table: &DisplayTable, tint: crate::color::Tint) -> DisplayTable {
    DisplayTable(table.0.scaled(tint))
}

/// Whether two tables draw the same curves, as the engine's check asks.
#[cfg(target_os = "macos")]
pub fn matches(shown: &DisplayTable, wanted: &DisplayTable) -> bool {
    shown.0.matches(&wanted.0)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::color::{DAY_KELVIN, PURE_RED_KELVIN, color_at};

    /// This file's code, without the tests: the doors the probe opens.
    fn doors() -> &'static str {
        let source = include_str!("probe.rs");
        source.split("#[cfg(test)]").next().expect("the source")
    }

    /// The doors' code with its comments left out.
    fn code() -> String {
        doors()
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_probe_opens_exactly_these_doors() {
        let found: BTreeSet<&str> = doors()
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix("pub fn "))
            .filter_map(|rest| rest.split(['(', '<']).next())
            .collect();
        // A new door goes into this list on purpose, after a look at what it
        // reaches.
        let allowed: BTreeSet<&str> = [
            "clock",
            "set_clock",
            "app",
            "cover_layers",
            "cover_images",
            "cover_rim_and_shade",
            "prebuild_cover",
            "color_table",
            "engine_worker",
            "tick",
            "tick_at",
            "online_displays",
            "read_table",
            "scaled",
            "matches",
        ]
        .into();
        assert_eq!(found, allowed);
    }

    #[test]
    fn the_probe_never_names_a_writing_path() {
        let code = code();
        for forbidden in [
            "apply",
            "restore",
            "write",
            "Displays",
            "CGSetDisplayTransferByTable",
        ] {
            assert!(
                !code.contains(forbidden),
                "the probe's code names `{forbidden}`"
            );
        }
        // The check sees what it is for: the read path is there.
        assert!(code.contains("Table::read"));
    }

    #[test]
    fn the_clock_and_the_engine_follow_the_probes_minute() {
        // One test owns the probe's clock, a static: the others never set it.
        let night = 23.0 * 60.0 + 30.0;
        set_clock(night);
        assert_eq!(clock(), night);

        // Long enough since the last step for the level to settle at once.
        let settled = Duration::from_secs(10);
        let sleep_at = |minute: f64| {
            set_clock(minute);
            assert_eq!(clock(), minute);
            tick(&mut engine_worker(Settings::default()), settled)
        };
        let tinted = sleep_at(night);
        let fading = sleep_at(21.0 * 60.0 + 10.0);
        let day = sleep_at(14.0 * 60.0 + 20.0);

        // The tinted screen is first checked 2 s after its paint.
        assert_eq!(tinted, Duration::from_secs(2));
        // A fade steps faster than the day waits for its next change.
        assert!(fading < day, "fade {fading:?}, day {day:?}");
        assert!(tinted < day, "night {tinted:?}, day {day:?}");
    }

    #[test]
    fn the_probes_app_has_the_trackpads_haptics_and_they_play_nothing_off_the_main_thread() {
        use std::ptr::fn_addr_eq;

        use crate::model::Action;

        // A thread of its own, which is never the main one, whatever
        // `--test-threads` says: the sinks must return without calling
        // AppKit or the actuator. The app is made on that thread.
        std::thread::spawn(|| {
            let mut app = app(Settings::default());
            let want = Haptics::TRACKPAD;
            let silent = Haptics::SILENT;
            let have = app.haptics;
            assert!(fn_addr_eq(have.align, want.align));
            assert!(fn_addr_eq(have.thump, want.thump));
            assert!(fn_addr_eq(have.press, want.press));
            // It is not the test apps' silence: the probe's window plays
            // the trackpad's feedback where it is allowed to.
            assert!(!fn_addr_eq(have.align, silent.align));
            assert!(!fn_addr_eq(have.thump, silent.thump));
            assert!(!fn_addr_eq(have.press, silent.press));
            for action in [Action::Detent, Action::Thump, Action::Press] {
                app.apply(action, None);
            }
            app.actions = vec![Action::Detent, Action::Thump, Action::Press];
            app.apply_actions(None);
            assert!(app.actions.is_empty(), "the app took the actions");
            for sink in [have.align, have.thump, have.press] {
                sink();
            }
            // Each of those played through `haptics::play`, which returned at
            // its guard: none got as far as the actuator or AppKit.
            assert_eq!(crate::haptics::reached(), 0, "a play got past the guard");
        })
        .join()
        .expect("the probe's haptics returned off the main thread");
    }

    #[test]
    fn the_cover_hands_over_four_images_the_rims_light_and_the_shade() {
        let size = 68;
        let night = Settings::default().night.kelvin;
        let [back, maw, gap, front, rim, shade] = cover_images(cover_layers(size), night);
        assert_eq!(back.size, [size, size]);
        assert_eq!(back.pixels.len(), size * size);
        // The maw, the gap, the front, the rim's light and the shade are each
        // their own box, smaller than the cover in rows, and the pixels they
        // hold fill it.
        for image in [&maw, &gap, &front, &rim, &shade] {
            assert!(image.size.iter().all(|&side| 0 < side && side <= size));
            assert!(image.size[1] < size, "{:?}", image.size);
            assert_eq!(image.pixels.len(), image.size[0] * image.size[1]);
        }
        assert!(maw.size[0] < size);
        assert_ne!(back.pixels, front.pixels);
    }

    #[test]
    fn the_rim_and_the_shade_for_many_kelvins_are_those_the_cover_images_give() {
        let size = 68;
        let night = Settings::default().night.kelvin;
        let [.., rim, shade] = cover_images(cover_layers(size), night);
        let pairs = cover_rim_and_shade(cover_layers(size), &[PURE_RED_KELVIN, night, DAY_KELVIN]);
        assert_eq!(pairs.len(), 3);
        // The night's pair is the one the first frame sends, whole.
        assert_eq!(pairs[1], [rim, shade]);
        // Pure red sinks other teeth than the night does.
        assert_ne!(pairs[0][1], pairs[1][1]);
        // In the day the maw is under the sand, and both are clear.
        for image in &pairs[2] {
            assert!(
                image
                    .pixels
                    .iter()
                    .all(|p| *p == egui::Color32::TRANSPARENT)
            );
        }
    }

    #[test]
    fn the_color_table_is_the_one_the_color_math_reads() {
        let table = color_table();
        assert_eq!(table.first(), Some(&color_at(PURE_RED_KELVIN)));
        assert_eq!(table.last(), Some(&color_at(DAY_KELVIN)));
        // Pure red at the night end: green and blue at exactly 0.
        let [_, green, blue] = table[0];
        assert_eq!((green, blue), (0.0, 0.0));
    }
}
