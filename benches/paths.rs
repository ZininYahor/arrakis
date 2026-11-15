//! The code paths the energy pass works on, timed in-process with divan.
//!
//! `cargo bench --features demo --bench paths [-- <group>]`. The groups are
//! `cover`, `frame`, `start`, `color`, `engine` and `gamma`. The gamma group
//! only reads the displays' tables: the probe it goes through has no way to
//! write one.

use std::time::{Duration, Instant};

use arrakis::color::Look;
use arrakis::settings::Settings;
use arrakis::{icon, probe, theme, ui};

fn main() {
    // Divan repeats each path many times, so it times what a process pays
    // from its second window on. The first window's frame, with the font
    // files read, happens once: time it here, before anything else warms
    // those up.
    let cold = frame::first_frame_of_the_process();
    println!(
        "first_frame_of_the_process (one sample): {:.2} ms",
        cold.as_secs_f64() * 1e3
    );
    divan::main();
}

/// Minutes since midnight.
fn at(hour: u16, minute: u16) -> f64 {
    f64::from(hour * 60 + minute)
}

mod cover {
    use super::*;

    /// The cover is 136 points square: 1× and 2×, and 1.5× between.
    const SIZES: [usize; 3] = [136, 204, 272];

    #[divan::bench(args = SIZES)]
    fn layers_new(size: usize) -> probe::CoverLayers {
        probe::cover_layers(size)
    }

    /// The six images a window's first frame sends to the GPU, from the
    /// layers, which it takes: the four it sends once, and the rim's light
    /// and the shade at the sand for the default night look. A Temperature
    /// drag costs none of the four, since the maw only moves its rect; it
    /// composes the rim's light again at each new rise, and the shade at
    /// each that moves it (`frame::temperature_drag`).
    #[divan::bench(args = SIZES)]
    fn images(bencher: divan::Bencher, size: usize) {
        let night = Settings::default().night.kelvin;
        bencher
            .with_inputs(|| probe::cover_layers(size))
            .bench_local_values(|layers| probe::cover_images(layers, night));
    }
}

mod frame {
    use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, ViewportId, pos2};

    use super::*;

    // The Temperature and Brightness sliders' tracks, as `ui::view` lays
    // them out: the driver exports them.
    use arrakis::drive::{BRIGHTNESS_Y, TEMPERATURE_Y, TRACK};
    /// A drag sweeps the track and back in 2 s, a frame at a time at 120 Hz.
    const SWEEP_FRAMES: f32 = 240.0;

    /// The window's egui side, headless, at 2 pixels per point.
    struct Window {
        ctx: egui::Context,
        app: arrakis::app::App,
        started: Instant,
    }

    impl Window {
        /// A window that has drawn a few frames already, at `minute`.
        fn warm(minute: f64) -> Self {
            probe::set_clock(minute);
            let mut window = Self::cold(probe::app(Settings::default()));
            for _ in 0..4 {
                window.frame(Vec::new());
            }
            window
        }

        /// A window before its first frame, for `app`.
        fn cold(app: arrakis::app::App) -> Self {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            Self {
                ctx,
                app,
                started: Instant::now(),
            }
        }

        fn input(&self, events: Vec<Event>) -> RawInput {
            let mut input = RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, theme::WINDOW)),
                time: Some(self.started.elapsed().as_secs_f64()),
                events,
                ..Default::default()
            };
            let viewport = input.viewports.entry(ViewportId::ROOT).or_default();
            viewport.native_pixels_per_point = Some(2.0);
            viewport.focused = Some(true);
            input
        }

        /// One frame: egui's pass over the window and its tessellation, as
        /// eframe runs them before painting.
        fn frame(&mut self, events: Vec<Event>) -> Vec<egui::ClippedPrimitive> {
            let input = self.input(events);
            let app = &mut self.app;
            let mut output = self.ctx.run_ui(input, |ui| {
                ui::draw(app, ui);
                app.apply_actions(Some(ui.ctx()));
            });
            // No renderer here to take the textures.
            output.textures_delta.clear();
            self.ctx.tessellate(output.shapes, output.pixels_per_point)
        }

        /// Holds the pointer down on a slider's day end at `y`.
        fn press(&mut self, y: f32) {
            let at = pos2(TRACK.0, y);
            self.frame(vec![
                Event::PointerMoved(at),
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ]);
        }
    }

    /// Where a sweep of the track is after `step` frames.
    fn sweep(step: u32) -> f32 {
        let phase = (step as f32 % SWEEP_FRAMES) / SWEEP_FRAMES;
        let there = 1.0 - (2.0 * phase - 1.0).abs();
        TRACK.0 + (TRACK.1 - TRACK.0) * there
    }

    fn drag(bencher: divan::Bencher, y: f32) {
        let mut window = Window::warm(at(14, 20));
        window.press(y);
        let mut step = 0;
        bencher.bench_local(|| {
            step += 1;
            window.frame(vec![Event::PointerMoved(pos2(sweep(step), y))])
        });
    }

    /// Nothing changes: what an idle window's clock refresh costs.
    #[divan::bench]
    fn steady(bencher: divan::Bencher) {
        let mut window = Window::warm(at(14, 20));
        bencher.bench_local(|| window.frame(Vec::new()));
    }

    /// The pointer rests on the Temperature slider.
    #[divan::bench]
    fn hover(bencher: divan::Bencher) {
        let mut window = Window::warm(at(14, 20));
        window.frame(vec![Event::PointerMoved(pos2(200.0, TEMPERATURE_Y))]);
        bencher.bench_local(|| window.frame(Vec::new()));
    }

    /// The pointer moves a frame's worth along the Temperature slider.
    #[divan::bench]
    fn temperature_drag(bencher: divan::Bencher) {
        drag(bencher, TEMPERATURE_Y);
    }

    /// The pointer moves a frame's worth along the Brightness slider.
    #[divan::bench]
    fn brightness_drag(bencher: divan::Bencher) {
        drag(bencher, BRIGHTNESS_Y);
    }

    /// The engine reports the next second of the 21:00 fade.
    #[divan::bench]
    fn fade_step(bencher: divan::Bencher) {
        let mut window = Window::warm(at(21, 1));
        let night = window.app.settings.night;
        let fade = f64::from(window.app.settings.fade_minutes);
        let mut second = 60.0;
        bencher.bench_local(|| {
            second = (second + 1.0) % (fade * 60.0);
            probe::set_clock(at(21, 0) + second / 60.0);
            let level = (second / (fade * 60.0)) as f32;
            window.app.status.level = level;
            window.app.status.tint = night.at_level(level);
            window.frame(Vec::new())
        });
    }

    /// The first window's first frame in a process, from a new Context:
    /// what `first_frame_fresh` times, plus the font files read, which
    /// happens once per process.
    pub(super) fn first_frame_of_the_process() -> Duration {
        probe::set_clock(at(14, 20));
        let app = probe::app(Settings::default());
        let started = Instant::now();
        let mut window = Window::cold(app);
        divan::black_box(window.frame(Vec::new()));
        started.elapsed()
    }

    /// A new Context, the window's theme on it, and its first frame: the
    /// fonts, the cover's layers and the layout from nothing. The font
    /// files are read once per process, before this runs; see
    /// `first_frame_of_the_process`.
    #[divan::bench(sample_count = 30, sample_size = 1)]
    fn first_frame_fresh(bencher: divan::Bencher) {
        probe::set_clock(at(14, 20));
        let app = std::cell::RefCell::new(Some(probe::app(Settings::default())));
        bencher.bench_local(|| {
            let mut window = Window::cold(app.borrow_mut().take().expect("an app"));
            let frame = window.frame(Vec::new());
            app.replace(Some(window.app));
            frame
        });
    }

    /// A reopened window's first frame: its Context and theme are made, as
    /// the window opens, before the timing starts.
    #[divan::bench(sample_count = 30, sample_size = 1)]
    fn first_frame_reopened(bencher: divan::Bencher) {
        probe::set_clock(at(14, 20));
        let app = std::cell::RefCell::new(Some(probe::app(Settings::default())));
        bencher
            .with_inputs(|| Window::cold(app.borrow_mut().take().expect("an app")))
            .bench_local_values(|mut window| {
                let frame = window.frame(Vec::new());
                app.replace(Some(window.app));
                frame
            });
    }

    /// A reopened window's first frame when the cover's images were built on
    /// a thread as the window was asked for, as the app does, and that
    /// thread has finished: the frame only uploads them, with the rim's light
    /// and the shade composed for its rise, each in a small box. The build is not
    /// timed; it runs in a real window while eframe makes the window.
    #[divan::bench(sample_count = 30, sample_size = 1)]
    fn first_frame_reopened_prebuilt(bencher: divan::Bencher) {
        probe::set_clock(at(14, 20));
        let app = std::cell::RefCell::new(Some(probe::app(Settings::default())));
        bencher
            .with_inputs(|| {
                probe::prebuild_cover(2.0);
                Window::cold(app.borrow_mut().take().expect("an app"))
            })
            .bench_local_values(|mut window| {
                let frame = window.frame(Vec::new());
                app.replace(Some(window.app));
                frame
            });
    }
}

mod start {
    use super::*;

    /// The menu bar's sizes: 1×, 2× (the macOS one), and a large 2×.
    const ICON_SIZES: [usize; 3] = [18, 36, 44];

    /// The fonts and the style on a new Context, and the empty pass that
    /// takes the fonts in: egui loads them as the next pass begins. The font
    /// files are read once per process, before this runs.
    #[divan::bench]
    fn theme_install(bencher: divan::Bencher) {
        bencher
            .with_inputs(egui::Context::default)
            .bench_local_refs(|ctx| {
                theme::install(ctx);
                let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
                output.textures_delta.clear();
            });
    }

    /// Computes the table afresh, as the golden test does. The app reads the
    /// table embedded in the binary instead (`precomputed`), which costs
    /// nothing at the start.
    #[divan::bench]
    fn color_table() -> Vec<[f32; 3]> {
        probe::color_table()
    }

    /// The full-color icon, drawn at every size. `fastframe-tray` asks for it
    /// off macOS only (32 px, 64 px), where the 36 px row is not the start's.
    #[divan::bench(args = ICON_SIZES)]
    fn tray_icon(size: usize) -> Vec<u8> {
        icon::tray(size)
    }

    /// What `fastframe-tray` calls on macOS: the embedded copy at 36 px, the
    /// macOS size, and a drawn icon at the others.
    #[divan::bench(args = ICON_SIZES)]
    fn tray_template(size: usize) -> Vec<u8> {
        icon::tray_template(size)
    }
}

mod color {
    use super::*;

    #[divan::bench]
    fn look_at_level() -> arrakis::color::Tint {
        divan::black_box(Look::default()).at_level(divan::black_box(0.37))
    }

    #[divan::bench]
    fn kelvin_at_redness() -> u16 {
        arrakis::color::kelvin_at_redness(divan::black_box(0.37))
    }
}

mod engine {
    use super::*;

    /// A worker that has settled at `minute`.
    fn settled(minute: f64) -> probe::EngineWorker {
        probe::set_clock(minute);
        let mut worker = probe::engine_worker(Settings::default());
        for _ in 0..4 {
            probe::tick(&mut worker, Duration::from_secs(1));
        }
        worker
    }

    /// By day: nothing to paint, and a sleep of up to a minute.
    #[divan::bench]
    fn tick_by_day(bencher: divan::Bencher) {
        let mut worker = settled(at(14, 20));
        bencher.bench_local(|| probe::tick(&mut worker, Duration::from_secs(10)));
    }

    /// Tinted and settled, a check of the tables due at every tick: the wake
    /// that the backoff spaces 2 to 30 s apart. Each tick is 30 s after the
    /// last on the engine's clock, which no backoff outlasts; the check
    /// looks at the `NullScreen`, which reads no table.
    #[divan::bench]
    fn tick_tinted(bencher: divan::Bencher) {
        let mut worker = settled(at(23, 30));
        let mut now = Instant::now();
        bencher.bench_local(|| {
            now += Duration::from_secs(30);
            probe::tick_at(&mut worker, now, Duration::from_secs(30))
        });
    }

    /// Each step of the 21:00 fade, a second apart: most paint a new table on
    /// the `NullScreen`, and each works out from the curve when the next
    /// table is due.
    #[divan::bench]
    fn tick_in_a_fade(bencher: divan::Bencher) {
        let mut worker = settled(at(21, 1));
        let mut second = 60.0;
        bencher.bench_local(|| {
            second = (second + 1.0) % (20.0 * 60.0);
            probe::set_clock(at(21, 0) + second / 60.0);
            probe::tick(&mut worker, Duration::from_secs(1))
        });
    }
}

/// Reads only: the probe has no way to write a table.
#[cfg(target_os = "macos")]
mod gamma {
    use super::*;

    fn displays() -> Vec<u32> {
        probe::online_displays().unwrap_or_default()
    }

    #[divan::bench]
    fn online_displays() -> Result<Vec<u32>, String> {
        probe::online_displays()
    }

    #[divan::bench(args = displays())]
    fn read_table(display: u32) -> Option<probe::DisplayTable> {
        probe::read_table(display)
    }

    /// The comparison each check of the tables makes once it has read one.
    #[divan::bench(args = displays())]
    fn scaled_and_matches(bencher: divan::Bencher, display: u32) {
        let table = probe::read_table(display);
        let tint = Look::default().tint();
        bencher.bench_local(|| {
            table
                .as_ref()
                .map(|table| probe::matches(table, &probe::scaled(table, tint)))
        });
    }

    /// One display's share of a check of the tables: read, scale, compare.
    #[divan::bench(args = displays())]
    fn check_share(bencher: divan::Bencher, display: u32) {
        let tint = Look::default().tint();
        bencher.bench_local(|| {
            let shown = probe::read_table(display);
            shown.is_some_and(|shown| probe::matches(&shown, &probe::scaled(&shown, tint)))
        });
    }
}
