//! Startup: one running copy, the log, the menu-bar item, the engine, and
//! the shell that opens and closes the window around them. [`wire`] wires
//! the lifecycle for the app and for the driver (`src/drive.rs`) alike.

#[cfg(feature = "demo")]
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Parser;
use fastframe_shell::{Shell, Waker};
use fastframe_tray::Tray;

use crate::app::{App, Haptics, Signals};
use crate::engine::{self, Engine};
use crate::painter::WindowPainter;
#[cfg(all(feature = "demo", target_os = "macos"))]
use crate::painter::{DEMO_RENDERER, DemoRenderer};
use crate::settings::{self, Settings};
use crate::{gamma, icon, idle, theme, ui};

const APP_ID: &str = "dev.arrakis.Arrakis";
/// What the app was called first. Its settings are carried over once.
const OLD_NAME: &str = "Sunset Parabellum";
const SHOW_REQUEST: &str = "show";

/// How each wake source wakes the app. [`wire`] and the instance slot take
/// every wake from here, so the driver runs the wait the app runs.
struct Wiring {
    /// A second launch, asking for the window.
    instance: fn(&Waker),
    /// A click on the menu-bar item or its menu.
    tray: fn(&Waker),
    /// The engine's event that a closed window needs
    /// ([`engine::Reach::App`]).
    engine: fn(&Waker),
    /// The engine's status that only moves the level, within
    /// [`engine::LEVEL_PACE`] of its last wake ([`engine::Reach::Window`]);
    /// [`engine_wake`] keeps it from a hidden window.
    level: fn(&Waker),
    /// macOS reconfigured a display.
    displays: fn(&Waker),
    /// macOS woke, switched back to this user, saw an app quit, or changed
    /// the clock or the time zone ([`gamma::watch_system`]).
    system: fn(&Waker),
}

/// All at once ([`idle::wake`]) but the engine. A quick fade reports every
/// 33 ms: paced ([`idle::wake_paced`]), it ends a few headless waits a
/// second, not thirty. Its statuses that only move the level reach an open
/// window alone ([`idle::wake_window`]), but one in [`engine::LEVEL_PACE`].
const WIRING: Wiring = Wiring {
    instance: idle::wake,
    tray: idle::wake,
    engine: idle::wake_paced,
    level: idle::wake_window,
    displays: idle::wake,
    system: idle::wake,
};

/// The engine's wake, which [`wire`] hands the app it makes.
pub(crate) type EngineWake = Box<dyn Fn(engine::Reach) + Send>;

/// The engine's wake as [`wire`] builds it: a status that [`engine::Reach::App`]
/// carries wakes through `app`, whatever the window does; one that only moves
/// the level ([`engine::Reach::Window`]) wakes through `level`, unless `hidden`
/// says the window is occluded or minimized (`Signals::window_hidden`).
/// eframe draws no frame for such a window, and the app's next pass brings the
/// status in anyway, so the wake would only run a pass for nothing.
fn engine_wake(
    waker: Waker,
    hidden: Arc<AtomicBool>,
    app: fn(&Waker),
    level: fn(&Waker),
) -> EngineWake {
    Box::new(move |reach| match reach {
        engine::Reach::App => app(&waker),
        engine::Reach::Window => {
            if !hidden.load(Ordering::Relaxed) {
                level(&waker);
            }
        }
    })
}

#[derive(Parser, Debug)]
#[command(name = "arrakis", version, about = "Night Shift, but it means it.")]
#[cfg_attr(
    feature = "demo",
    command(group(
        clap::ArgGroup::new("demo_window")
            .args(["demo_shot", "demo_drive"])
            .multiple(true)
    ))
)]
struct Cli {
    /// Start in the menu bar without opening the window.
    #[arg(long)]
    hidden: bool,
    /// Log at debug level.
    #[arg(long)]
    verbose: bool,
    /// Save a picture of the scene to this PNG and quit. Leaves the real
    /// displays and settings alone.
    #[cfg(feature = "demo")]
    #[arg(long, value_name = "PNG")]
    demo_shot: Option<PathBuf>,
    // An option that serves one mode (this one, `demo_seconds`, `demo_report`,
    // `tolerance`, `within` and `out`) both `requires` it and excludes the
    // others. `requires` alone is
    // not enough: clap leaves out the requirements of an option beside a flag
    // that excludes the mode, so `--demo-icon i.png --demo-scene day` parsed,
    // and the scene was dropped without a word.
    /// The situation to picture.
    #[cfg(feature = "demo")]
    #[arg(
        long,
        value_enum,
        default_value = "night",
        requires = "demo_shot",
        conflicts_with_all = ["demo_icon", "demo_drive", "demo_diff"]
    )]
    demo_scene: crate::demo::Scene,
    /// Write the app's icon, the 1024 px master, to this PNG and quit. Opens
    /// no window and leaves the real displays and settings alone.
    #[cfg(feature = "demo")]
    #[arg(long, value_name = "PNG", conflicts_with_all = ["demo_shot", "demo_drive", "demo_diff"])]
    demo_icon: Option<PathBuf>,
    /// Run the real lifecycle on a stand-in for the displays, drive it,
    /// time it and quit. No settings file, no instance slot.
    #[cfg(feature = "demo")]
    #[arg(
        long,
        value_enum,
        value_name = "SCENARIO",
        conflicts_with = "demo_shot"
    )]
    demo_drive: Option<crate::drive::Scenario>,
    /// How long the drive runs.
    #[cfg(feature = "demo")]
    #[arg(
        long,
        value_name = "N",
        default_value_t = 30,
        requires = "demo_drive",
        conflicts_with_all = ["demo_shot", "demo_icon", "demo_diff"]
    )]
    demo_seconds: u64,
    /// Where the drive writes its JSON report.
    #[cfg(feature = "demo")]
    #[arg(
        long,
        value_name = "JSON",
        requires = "demo_drive",
        conflicts_with_all = ["demo_shot", "demo_icon", "demo_diff"]
    )]
    demo_report: Option<PathBuf>,
    /// The painter of a shot's or a drive's window: `scripts/painter-check.sh`
    /// compares the two.
    #[cfg(all(feature = "demo", target_os = "macos"))]
    #[arg(
        long,
        value_enum,
        value_name = "PAINTER",
        requires = "demo_window",
        conflicts_with_all = ["demo_icon", "demo_diff"]
    )]
    demo_renderer: Option<DemoRenderer>,
    /// Compare two PNGs pixel by pixel, every frame of a film, and quit:
    /// non-zero past the tolerance.
    #[cfg(feature = "demo")]
    #[arg(long, num_args = 2, value_names = ["A", "B"], conflicts_with_all = ["demo_shot", "demo_drive"])]
    demo_diff: Option<Vec<PathBuf>>,
    /// The largest difference a channel may show, out of 255.
    #[cfg(feature = "demo")]
    #[arg(
        long,
        value_name = "N",
        default_value_t = 0,
        requires = "demo_diff",
        conflicts_with_all = ["demo_shot", "demo_icon", "demo_drive"]
    )]
    tolerance: u8,
    /// Allow differences only inside this rectangle of pixels.
    #[cfg(feature = "demo")]
    #[arg(
        long,
        value_name = "X,Y,W,H",
        requires = "demo_diff",
        conflicts_with_all = ["demo_shot", "demo_icon", "demo_drive"]
    )]
    within: Option<crate::diff::Area>,
    /// Write the amplified difference to this PNG.
    #[cfg(feature = "demo")]
    #[arg(
        long,
        value_name = "PNG",
        requires = "demo_diff",
        conflicts_with_all = ["demo_shot", "demo_icon", "demo_drive"]
    )]
    out: Option<PathBuf>,
}

pub fn run() -> eframe::Result {
    #[cfg(feature = "demo")]
    let started = std::time::Instant::now();
    let cli = Cli::parse();
    #[cfg(all(feature = "demo", target_os = "macos"))]
    if let Some(renderer) = cli.demo_renderer {
        let _ = DEMO_RENDERER.set(renderer);
    }
    #[cfg(feature = "demo")]
    if let Some(path) = cli.demo_shot.clone() {
        return run_demo(path, cli.demo_scene);
    }
    #[cfg(feature = "demo")]
    if let Some(path) = cli.demo_icon.as_deref() {
        std::process::exit(crate::app_icon::run(path));
    }
    #[cfg(feature = "demo")]
    if let Some(scenario) = cli.demo_drive {
        return crate::drive::run(started, scenario, cli.demo_seconds, cli.demo_report);
    }
    #[cfg(feature = "demo")]
    if let Some([a, b]) = cli.demo_diff.as_deref() {
        let options = crate::diff::Options {
            tolerance: cli.tolerance,
            within: cli.within,
            out: cli.out,
        };
        std::process::exit(crate::diff::run(a, b, &options));
    }

    let dirs = fastframe_dirs::Dirs::platform("dev", "Arrakis", "Arrakis");
    if let Some(dirs) = &dirs
        && let Err(error) =
            std::fs::create_dir_all(&dirs.state).and(std::fs::create_dir_all(&dirs.config))
    {
        eprintln!("unable to create the app's folders: {error}");
    }
    let mut logging =
        fastframe_log::Logging::new("arrakis", env!("CARGO_PKG_VERSION")).filter(if cli.verbose {
            "info,arrakis=debug"
        } else {
            "warn,arrakis=info"
        });
    if let Some(dirs) = &dirs {
        logging = logging
            .file(dirs.state.join("arrakis.log"))
            .panic_log(dirs.state.join("panic.log"));
    }
    if let Err(error) = logging.init() {
        eprintln!("not logging: {error}");
    }

    // Two copies would fight over the same gamma tables, and when either
    // quit macOS would reset them under the other. A second launch only
    // asks the first to show its window.
    let waker = Waker::default();
    let signals = Signals::default();
    let _instance = {
        let show = signals.show_window.clone();
        let wake = waker.clone();
        match fastframe_instance::Slot::new(APP_ID).claim(SHOW_REQUEST, move |request| {
            (request == SHOW_REQUEST).then(|| {
                show.store(true, Ordering::Relaxed);
                (WIRING.instance)(&wake);
                "shown".to_owned()
            })
        }) {
            fastframe_instance::Claim::First(guard) => guard,
            other => {
                log::info!("Arrakis is already running ({other:?})");
                return Ok(());
            }
        }
    };

    let settings_path = dirs.as_ref().map(|dirs| dirs.config.join("settings.json"));
    if let Some(path) = &settings_path
        && let Some(old) = fastframe_dirs::Dirs::platform("dev", OLD_NAME, OLD_NAME)
    {
        settings::carry_over(&old.config.join("settings.json"), path);
    }
    let settings = settings_path
        .as_deref()
        .map(Settings::load)
        .unwrap_or_default();

    wire(&waker, signals, cli.hidden, |tray, signals, wake| {
        let engine = Engine::spawn(
            Box::new(gamma::Displays::new()),
            settings.clone(),
            engine::local_minute_of_day,
            wake,
        );
        App::new(
            settings,
            settings_path,
            engine,
            tray,
            signals,
            engine::local_minute_of_day,
            Haptics::TRACKPAD,
        )
    })
    .run(|lease| {
        eframe::run_native(
            "Arrakis",
            native_options(),
            Box::new(move |cc| {
                theme::install(&cc.egui_ctx);
                // On the main thread, with AppKit running, before the
                // window takes its first press. Once for the app's life.
                crate::haptics::watch_pointer();
                let mut app = lease.take(&cc.egui_ctx);
                app.attach(WindowPainter::of(cc));
                Ok(Box::new(ui::Window::new(app)))
            }),
        )
    })
}

/// The lifecycle around the window, wired once for the app and the driver,
/// in this order: the AutoFill key, the Dock icon hidden, the menu-bar item,
/// the watch on macOS's notices, the display watch, the app, and the shell
/// that runs them. `app` makes the app from the menu-bar item, `signals` and
/// the engine's wake. The two differ in the engine's screen and clock, and in
/// the settings: the app loads the settings file and saves to its path, the
/// driver starts from `Settings::default()` and saves nowhere. Every wake
/// wakes `waker` as [`WIRING`] says. The shell starts in the menu bar when
/// `hidden` asks and there is a menu-bar item to open the window from;
/// otherwise the window opens at once, and its cover's images start building
/// first.
pub(crate) fn wire(
    waker: &Waker,
    signals: Signals,
    hidden: bool,
    app: impl FnOnce(Option<Tray>, Signals, EngineWake) -> App,
) -> Shell<App> {
    // First of all, before AppKit starts: no AutoFill helper with the window.
    ui::appkit::skip_autofill_helper();

    // Out of the Dock before AppKit finishes launching; each window hides
    // it again, since an unbundled run puts it back.
    #[cfg(target_os = "macos")]
    fastframe_dock::set_dock_icon_visible(false);

    let tray = {
        let wake = waker.clone();
        Tray::spawn(tray_config(), move || (WIRING.tray)(&wake))
    };
    if tray.is_none() {
        log::warn!("no menu-bar item; closing the window will quit");
    }
    let hidden = hidden && tray.is_some();

    // Both watches before the app is built: each hears only the changes
    // after it is registered, so one while the engine starts would be lost.
    {
        let woke = signals.system_changed.clone();
        let wake = waker.clone();
        gamma::watch_system(move || {
            woke.store(true, Ordering::Relaxed);
            (WIRING.system)(&wake);
        });
    }
    let changed = signals.displays_changed.clone();
    {
        let wake = waker.clone();
        gamma::watch_reconfiguration(move || {
            changed.store(true, Ordering::Relaxed);
            (WIRING.displays)(&wake);
        });
    }
    let app = {
        let wake = engine_wake(
            waker.clone(),
            signals.window_hidden.clone(),
            WIRING.engine,
            WIRING.level,
        );
        app(tray, signals, wake)
    };

    if !hidden {
        // The window opens next: build its cover's images meanwhile.
        App::window_asked_for();
    }

    // With the window closed, one AppKit pump per headless wait. Every wake
    // above ends it at once but the engine's: a status a closed window needs
    // ends it a pace in, and one that only moves the level reaches an open
    // window alone.
    Shell::new(app, waker).idle(idle::idle).start_hidden(hidden)
}

/// The menu-bar item and its menu.
fn tray_config() -> fastframe_tray::Config {
    fastframe_tray::Config {
        id: "arrakis",
        title: "Arrakis".to_owned(),
        icon: icon::tray,
        template_icon: Some(icon::tray_template),
        themed_icon: false,
        menu: App::tray_menu(),
    }
}

/// The window, its content running up under the system's traffic lights.
pub(crate) fn native_options() -> eframe::NativeOptions {
    let mut options = sized_options(theme::WINDOW.into());
    options.viewport = std::mem::take(&mut options.viewport)
        .with_decorations(true)
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);
    options
}

/// A plate's window, `size` points, with no title bar.
#[cfg(feature = "demo")]
fn native_options_for_plate(size: egui::Vec2) -> eframe::NativeOptions {
    let mut options = sized_options(size.into());
    options.viewport = std::mem::take(&mut options.viewport).with_decorations(false);
    options
}

fn sized_options(size: [f32; 2]) -> eframe::NativeOptions {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Arrakis")
        .with_app_id(APP_ID)
        .with_resizable(false)
        .with_maximize_button(false)
        .with_inner_size(size)
        .with_min_inner_size(size)
        .with_max_inner_size(size);
    // On macOS, the system's icon, not egui's. With egui's, eframe hands
    // AppKit a new 512 px image for the app's icon at every window, and
    // CoreGraphics keeps about 4.6 MB drawn from each, for up to 16 windows.
    // An empty icon sets none. Arrakis's own
    // icon goes into an app bundle's `.icns`, never here. Elsewhere, egui's
    // icon stays.
    #[cfg(target_os = "macos")]
    let viewport = viewport.with_icon(egui::IconData::default());
    eframe::NativeOptions {
        viewport,
        centered: true,
        renderer: crate::painter::renderer(),
        ..Default::default()
    }
}

/// The patch in `Cargo.toml` puts the Metal painter behind eframe's wgpu seam.
/// Cargo drops it with only a warning when eframe asks for another version, and
/// this names a constant the real `egui-wgpu` lacks, in the crate eframe links,
/// so the build stops and wgpu never comes in.
#[cfg(target_os = "macos")]
const _: bool = eframe::egui_wgpu::METAL_PAINTER;

/// What a shot says on stderr, for `scripts/painter-check.sh`, which would
/// otherwise pass by comparing one painter with itself. `gl` is what eframe
/// gave the app, never the flag's value. The Metal painter has no GL, and its
/// `render_state` is `None`, so `wgpu_render_state` would tell nothing here.
/// A shot whose Metal window fails opens again on glow and says so too: it
/// prints a line for each window, so the check wants one line and no more. A
/// plate's has no `App` to open a window on glow, and ends with an error
/// instead ([`plate_outcome`]).
#[cfg(feature = "demo")]
fn painter_line(gl: bool) -> &'static str {
    if gl {
        "painter: glow"
    } else {
        "painter: metal"
    }
}

/// What a plate's run comes to, once its window has closed: with the painter
/// failed there is no picture, and `main` says so with a nonzero exit.
#[cfg(feature = "demo")]
fn plate_outcome(failure: Option<(String, std::time::Instant)>) -> eframe::Result {
    match failure {
        Some((why, _)) => Err(eframe::Error::AppCreation(
            format!("the Metal painter failed, so this plate has no picture: {why}").into(),
        )),
        None => Ok(()),
    }
}

/// A window with a fixed scene and a stand-in for the displays, saved to a
/// PNG. No menu-bar item, no instance slot, no settings file.
#[cfg(feature = "demo")]
fn run_demo(path: PathBuf, scene: crate::demo::Scene) -> eframe::Result {
    use crate::demo::{Plate, Shot};

    let _ = fastframe_log::Logging::new("arrakis", env!("CARGO_PKG_VERSION"))
        .filter("warn,arrakis=info")
        .init();
    if let Some(plate) = scene.plate() {
        eframe::run_native(
            "Arrakis",
            native_options_for_plate(plate.size),
            Box::new(move |cc| {
                eprintln!("{}", painter_line(cc.gl.is_some()));
                theme::install(&cc.egui_ctx);
                Ok(Box::new(Plate::new(Shot::new(path, scene), plate)))
            }),
        )?;
        return plate_outcome(crate::painter::failure());
    }
    let (settings, red_now) = scene.set_up();
    let waker = Waker::default();
    let engine = {
        let wake = waker.clone();
        Engine::spawn(
            Box::new(crate::app::NullScreen),
            settings.clone(),
            crate::demo::clock,
            // A shot's window is open: every status draws it.
            move |_| idle::wake_paced(&wake),
        )
    };
    let mut app = App::new(
        settings,
        None,
        engine,
        None,
        Signals::default(),
        crate::demo::clock,
        // With no finger on a trackpad, nothing is felt.
        Haptics::TRACKPAD,
    );
    if red_now {
        app.apply(crate::model::Action::RedNow(true), None);
    }
    App::window_asked_for();
    Shell::new(app, &waker).run(|lease| {
        let path = path.clone();
        eframe::run_native(
            "Arrakis",
            native_options(),
            Box::new(move |cc| {
                eprintln!("{}", painter_line(cc.gl.is_some()));
                theme::install(&cc.egui_ctx);
                let mut app = lease.take(&cc.egui_ctx);
                app.attach(WindowPainter::of(cc));
                Ok(Box::new(
                    ui::Window::new(app).with_shot(Some(Shot::new(path, scene))),
                ))
            }),
        )
    })
}

#[cfg(all(test, feature = "demo"))]
mod tests {
    use clap::ValueEnum;
    use clap::error::ErrorKind;

    use super::*;
    use crate::diff::Area;
    use crate::drive::Scenario;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("arrakis").chain(args.iter().copied()))
    }

    /// Why the command line is refused.
    fn refused(args: &[&str]) -> ErrorKind {
        parse(args)
            .expect_err("the command line should be refused")
            .kind()
    }

    #[test]
    fn demo_diff_takes_exactly_two_paths() {
        let cli = parse(&["--demo-diff", "a.png", "b.png"]).expect("two paths");
        assert_eq!(
            cli.demo_diff,
            Some(vec![PathBuf::from("a.png"), PathBuf::from("b.png")])
        );
        assert!(parse(&["--demo-diff"]).is_err());
        assert!(parse(&["--demo-diff", "a.png"]).is_err());
        assert!(parse(&["--demo-diff", "a.png", "b.png", "c.png"]).is_err());
    }

    #[test]
    fn the_diffs_options_need_demo_diff() {
        for option in [
            ["--tolerance", "3"],
            ["--within", "1,2,3,4"],
            ["--out", "d.png"],
        ] {
            assert_eq!(refused(&option), ErrorKind::MissingRequiredArgument);
        }
        let cli = parse(&[
            "--demo-diff",
            "a.png",
            "b.png",
            "--tolerance",
            "3",
            "--within",
            "1,2,3,4",
            "--out",
            "d.png",
        ])
        .expect("the diff and its options");
        assert_eq!(cli.tolerance, 3);
        assert_eq!(
            cli.within,
            Some(Area {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
            })
        );
        assert_eq!(cli.out, Some(PathBuf::from("d.png")));
    }

    #[test]
    fn a_within_that_is_not_an_area_is_refused() {
        for within in ["1,2,3", "1,2,3,4,5", "a,b,c,d", ""] {
            let args = ["--demo-diff", "a.png", "b.png", "--within", within];
            assert_eq!(refused(&args), ErrorKind::ValueValidation, "{within:?}");
        }
    }

    #[test]
    fn demo_diff_excludes_the_other_demo_modes() {
        let diff = ["--demo-diff", "a.png", "b.png"];
        for other in [["--demo-shot", "s.png"], ["--demo-drive", "start"]] {
            let args: Vec<&str> = diff.iter().chain(&other).copied().collect();
            assert_eq!(refused(&args), ErrorKind::ArgumentConflict, "{other:?}");
        }
        assert_eq!(
            refused(&["--demo-drive", "start", "--demo-shot", "s.png"]),
            ErrorKind::ArgumentConflict
        );
    }

    #[test]
    fn demo_icon_takes_exactly_one_path() {
        let cli = parse(&["--demo-icon", "a.png"]).expect("one path");
        assert_eq!(cli.demo_icon, Some(PathBuf::from("a.png")));
        assert!(parse(&[]).expect("no flags").demo_icon.is_none());
        assert!(parse(&["--demo-icon"]).is_err());
        assert!(parse(&["--demo-icon", "a.png", "b.png"]).is_err());
    }

    #[test]
    fn demo_icon_excludes_the_other_demo_modes_in_either_order() {
        let icon = ["--demo-icon", "i.png"];
        for other in [
            vec!["--demo-shot", "s.png"],
            vec!["--demo-drive", "start"],
            vec!["--demo-diff", "a.png", "b.png"],
        ] {
            let icon_first: Vec<&str> = icon.iter().chain(&other).copied().collect();
            let icon_last: Vec<&str> = other.iter().chain(&icon).copied().collect();
            for args in [icon_first, icon_last] {
                assert_eq!(refused(&args), ErrorKind::ArgumentConflict, "{args:?}");
            }
        }
    }

    #[test]
    fn demo_scene_needs_demo_shot() {
        assert_eq!(
            refused(&["--demo-scene", "day"]),
            ErrorKind::MissingRequiredArgument
        );
        assert!(parse(&["--demo-shot", "s.png", "--demo-scene", "day"]).is_ok());
        // Its default does not count as given: the other modes parse.
        for mode in [
            vec!["--demo-icon", "i.png"],
            vec!["--demo-drive", "start"],
            vec!["--demo-diff", "a.png", "b.png"],
        ] {
            let cli = parse(&mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
            assert_eq!(cli.demo_scene, crate::demo::Scene::Night, "{mode:?}");
        }
    }

    #[test]
    fn the_banner_and_the_bench_are_scenes_by_name() {
        use crate::demo::Scene;
        for (name, scene) in [("banner", Scene::Banner), ("bench", Scene::Bench)] {
            let cli = parse(&["--demo-shot", "b.png", "--demo-scene", name])
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(cli.demo_scene, scene, "{name}");
        }
    }

    #[test]
    fn the_scale_the_banner_and_the_bench_are_the_plates_and_each_opens_its_own_size() {
        use crate::demo::Scene;
        let plates: Vec<Scene> = Scene::value_variants()
            .iter()
            .copied()
            .filter(|scene| scene.plate().is_some())
            .collect();
        assert_eq!(plates.len(), 3, "{plates:?}");
        for scene in [Scene::Scale, Scene::Banner, Scene::Bench] {
            assert!(plates.contains(&scene), "{scene:?}");
        }
        // A plate's window is as big as its picture and has no title bar.
        for scene in plates {
            let plate = scene.plate().expect("a plate");
            let viewport = native_options_for_plate(plate.size).viewport;
            assert_eq!(viewport.inner_size, Some(plate.size), "{scene:?}");
            assert_eq!(viewport.decorations, Some(false), "{scene:?}");
        }
    }

    /// Beside a mode it does not serve, an option that `requires` its own
    /// mode is not refused as missing: clap leaves out the requirements of
    /// what a present flag excludes, so `--demo-icon i.png --demo-scene day`
    /// parsed, and the scene was dropped without a word. Each such option
    /// excludes the modes it does not serve, so that it is refused.
    #[test]
    fn an_option_is_refused_beside_every_mode_it_does_not_serve() {
        let modes: [(&str, Vec<&str>); 4] = [
            ("--demo-shot", vec!["--demo-shot", "s.png"]),
            ("--demo-icon", vec!["--demo-icon", "i.png"]),
            ("--demo-drive", vec!["--demo-drive", "start"]),
            ("--demo-diff", vec!["--demo-diff", "a.png", "b.png"]),
        ];
        let options: [(&str, &str, Vec<&str>); 6] = [
            ("--demo-shot", "--demo-scene", vec!["--demo-scene", "day"]),
            (
                "--demo-drive",
                "--demo-seconds",
                vec!["--demo-seconds", "5"],
            ),
            (
                "--demo-drive",
                "--demo-report",
                vec!["--demo-report", "r.json"],
            ),
            ("--demo-diff", "--tolerance", vec!["--tolerance", "3"]),
            ("--demo-diff", "--within", vec!["--within", "1,2,3,4"]),
            ("--demo-diff", "--out", vec!["--out", "d.png"]),
        ];
        for (serves, option, given) in &options {
            for (mode, args) in &modes {
                let mut together: Vec<&str> = args.clone();
                together.extend(given);
                let mut flipped: Vec<&str> = given.clone();
                flipped.extend(args);
                for line in [together, flipped] {
                    if mode == serves {
                        parse(&line).unwrap_or_else(|error| panic!("{line:?}: {error}"));
                    } else {
                        assert_eq!(
                            refused(&line),
                            ErrorKind::ArgumentConflict,
                            "{option} beside {mode}: {line:?}"
                        );
                    }
                }
            }
        }
    }

    /// `--demo-renderer` serves both modes that open a window, and is refused beside the others
    /// as the options above are. Without a mode it is missing one.
    #[cfg(target_os = "macos")]
    #[test]
    fn demo_renderer_serves_a_shot_and_a_drive_and_no_other_mode() {
        let modes: [(&str, Vec<&str>, bool); 4] = [
            ("--demo-shot", vec!["--demo-shot", "s.png"], true),
            ("--demo-drive", vec!["--demo-drive", "start"], true),
            ("--demo-icon", vec!["--demo-icon", "i.png"], false),
            ("--demo-diff", vec!["--demo-diff", "a.png", "b.png"], false),
        ];
        for (name, painter) in [("glow", DemoRenderer::Glow), ("metal", DemoRenderer::Metal)] {
            for (mode, args, serves) in &modes {
                let given = ["--demo-renderer", name];
                let mut together: Vec<&str> = args.clone();
                together.extend(given);
                let mut flipped: Vec<&str> = given.to_vec();
                flipped.extend(args);
                for line in [together, flipped] {
                    if *serves {
                        let cli = parse(&line).unwrap_or_else(|error| panic!("{line:?}: {error}"));
                        assert_eq!(cli.demo_renderer, Some(painter), "{line:?}");
                    } else {
                        assert_eq!(
                            refused(&line),
                            ErrorKind::ArgumentConflict,
                            "--demo-renderer {name} beside {mode}: {line:?}"
                        );
                    }
                }
            }
            // On its own there is no window to serve.
            assert_eq!(
                refused(&["--demo-renderer", name]),
                ErrorKind::MissingRequiredArgument
            );
        }
        // The default is no choice at all.
        let cli = parse(&["--demo-shot", "s.png"]).expect("a shot");
        assert_eq!(cli.demo_renderer, None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn demo_renderer_takes_the_two_painters_by_name_and_no_other() {
        for value in ["wgpu", "gl", "Glow", "METAL", "", "metal "] {
            let line = ["--demo-shot", "s.png", "--demo-renderer", value];
            assert_eq!(refused(&line), ErrorKind::InvalidValue, "{value:?}");
        }
        assert_eq!(
            refused(&["--demo-shot", "s.png", "--demo-renderer"]),
            ErrorKind::InvalidValue
        );
    }

    /// The lines `scripts/painter-check.sh` looks for, whole.
    #[test]
    fn a_shot_says_which_painter_drew_it_from_what_eframe_gave_it() {
        assert_eq!(painter_line(true), "painter: glow");
        assert_eq!(painter_line(false), "painter: metal");
    }

    #[test]
    fn the_drives_options_need_demo_drive() {
        assert_eq!(
            refused(&["--demo-seconds", "5"]),
            ErrorKind::MissingRequiredArgument
        );
        assert_eq!(
            refused(&["--demo-report", "r.json"]),
            ErrorKind::MissingRequiredArgument
        );
        let cli = parse(&[
            "--demo-drive",
            "open-idle",
            "--demo-seconds",
            "5",
            "--demo-report",
            "r.json",
        ])
        .expect("the drive and its options");
        assert_eq!(cli.demo_drive, Some(Scenario::OpenIdle));
        assert_eq!(cli.demo_seconds, 5);
        assert_eq!(cli.demo_report, Some(PathBuf::from("r.json")));
    }

    #[test]
    fn every_scenario_name_parses_and_no_other_does() {
        let names = [
            "closed-day",
            "closed-night",
            "closed-fade",
            "open-idle",
            "open-night",
            "open-fade",
            "drag",
            "rest-drag",
            "open-close",
            "open-close-burst",
            "start",
        ];
        let parsed: Vec<Scenario> = names
            .iter()
            .map(|name| {
                parse(&["--demo-drive", name])
                    .unwrap_or_else(|error| panic!("{name}: {error}"))
                    .demo_drive
                    .expect("a scenario")
            })
            .collect();
        // The names are the scenarios, each once.
        assert_eq!(parsed.len(), Scenario::value_variants().len());
        for scenario in Scenario::value_variants() {
            assert_eq!(parsed.iter().filter(|p| *p == scenario).count(), 1);
        }
        assert_eq!(
            refused(&["--demo-drive", "closed-evening"]),
            ErrorKind::InvalidValue
        );
        assert_eq!(refused(&["--demo-drive", "Start"]), ErrorKind::InvalidValue);
    }

    #[test]
    fn a_plate_on_a_failed_painter_ends_with_an_error_that_says_why() {
        assert!(plate_outcome(None).is_ok());
        let failure = Some(("the reason".to_owned(), std::time::Instant::now()));
        let error = plate_outcome(failure).expect_err("no picture came");
        let said = error.to_string();
        assert!(said.contains("this plate has no picture"), "{said}");
        assert!(said.contains("the reason"), "{said}");
    }

    #[test]
    fn the_plates_window_carries_the_empty_icon_too() {
        // The plate's window opens through `sized_options` as the app's does:
        // the empty icon on macOS, none (so egui's) elsewhere.
        assert_eq!(
            native_options_for_plate(crate::demo::PLATE)
                .viewport
                .icon
                .as_deref(),
            cfg!(target_os = "macos")
                .then(egui::IconData::default)
                .as_ref(),
            "on macOS a plate window without the empty icon gets egui's"
        );
    }
}

/// The lifecycle that the app and the driver share. Not behind `demo`: it
/// is the real app's.
#[cfg(test)]
mod wiring {
    /// The app's and the driver's window both open with these options.
    #[test]
    fn the_window_carries_the_empty_icon_on_macos() {
        // On macOS, `None` leaves eframe to load egui's own icon and hand
        // AppKit a new 512 px image at every window: the footprint then grows
        // about 4.6 MB with each open. Elsewhere the window keeps egui's icon.
        assert_eq!(
            super::native_options().viewport.icon.as_deref(),
            cfg!(target_os = "macos")
                .then(egui::IconData::default)
                .as_ref(),
            "on macOS the window has no icon of its own, so eframe would set egui's at every open"
        );
    }

    /// `fastframe-tray` draws a macOS template icon alone and never the color
    /// one beside it. So the template at 36 pixels is embedded in the binary
    /// (`precomputed`) and the color icon is drawn, off macOS only, when
    /// asked for. A config without the template would draw the color icon at
    /// 36 pixels on macOS, which nothing embeds.
    #[test]
    fn the_menu_bar_item_draws_the_embedded_template_at_the_macos_size() {
        use super::*;
        use crate::precomputed;

        let config = tray_config();
        let template = config.template_icon.expect("a template icon");
        assert_eq!(template(precomputed::TRAY_SIZE), precomputed::TRAY_TEMPLATE);
        // Off macOS the item shows the color icon, which is not the template.
        assert_ne!((config.icon)(32), template(32));
    }

    // macOS only: elsewhere the two wakes compile to one body, which may share one address.
    #[cfg(target_os = "macos")]
    #[test]
    fn every_source_wakes_the_app_at_once_but_the_engine() {
        use std::ptr::fn_addr_eq;

        use super::*;

        let now: fn(&Waker) = idle::wake;
        let paced: fn(&Waker) = idle::wake_paced;
        let window: fn(&Waker) = idle::wake_window;
        assert!(!fn_addr_eq(now, paced), "the two wakes are told apart");
        assert!(!fn_addr_eq(window, now), "the window's wake is not at once");
        assert!(!fn_addr_eq(window, paced), "the window's wake is not paced");
        // A second launch, a menu-bar click, a display change and macOS's
        // notices end a headless wait at once; the engine's status ends it
        // a pace in, and its level-only one does not end it at all.
        for (source, wake, wanted) in [
            ("instance slot", WIRING.instance, now),
            ("menu-bar item", WIRING.tray, now),
            ("engine", WIRING.engine, paced),
            ("engine's level", WIRING.level, window),
            ("display watch", WIRING.displays, now),
            ("macOS's notices", WIRING.system, now),
        ] {
            assert!(fn_addr_eq(wake, wanted), "the {source}'s wake");
        }
    }

    /// The one top-level `fn` in `source` whose line starts with
    /// `signature`, up to its closing brace, without comment lines. `\r\n`
    /// ends a line as `\n` does.
    fn body(source: &str, signature: &str) -> String {
        // The comments go first, so that one naming the signature cannot
        // match.
        let code = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let code = format!("\n{code}\n");
        let line = format!("\n{signature}");
        let mut starts = code.match_indices(&line);
        let (start, _) = starts.next().unwrap_or_else(|| panic!("no `{signature}`"));
        assert!(starts.next().is_none(), "two `{signature}`");
        let rest = &code[start + 1..];
        let end = rest.find("\n}\n").expect("the function's end");
        rest[..end].to_owned()
    }

    #[test]
    fn the_scan_reads_the_function_not_a_comment_that_names_it() {
        let source = concat!(
            "/// Calls `pub fn run(` first.\r\n",
            "fn other() {\r\n",
            "}\r\n",
            "impl Thing {\r\n",
            "    pub fn run(&self) {}\r\n",
            "}\r\n",
            "pub fn run() {\r\n",
            "    // `idle::` in a comment\r\n",
            "    real();\r\n",
            "}\r\n",
        );
        assert_eq!(body(source, "pub fn run("), "pub fn run() {\n    real();");
    }

    #[test]
    fn the_app_and_the_driver_wire_their_lifecycle_only_through_wire() {
        // A path that wired a piece itself could drift from the other: the
        // driver would then time a wait the app does not run, and nothing
        // would fail.
        let entrypoint = include_str!("entrypoint.rs");
        let drive = include_str!("drive.rs");
        // The closure that `entrypoint::run` hands `wire` is in its body.
        for (path, source) in [("entrypoint::run", entrypoint), ("drive::run", drive)] {
            let run = body(source, "pub fn run(");
            assert!(run.contains("wire("), "{path} does not call `wire`");
            for piece in [
                "idle::",
                "Tray::spawn",
                "watch_reconfiguration",
                "watch_system",
                "set_dock_icon_visible",
                "skip_autofill_helper",
                "Shell::new",
                ".idle(",
                ".start_hidden(",
            ] {
                assert!(
                    !run.contains(piece),
                    "{path} calls `{piece}` itself, not through `wire`"
                );
            }
        }
        // The driver's closure makes its app in `drive::app`, which takes
        // the engine's wake from `wire` too.
        assert!(
            !body(drive, "pub fn app(").contains("idle::"),
            "drive::app wakes the app itself, not through `wire`"
        );
        let wire = body(entrypoint, "pub(crate) fn wire(");
        assert!(
            !wire.contains("idle::wake"),
            "`wire` takes every wake from `WIRING`"
        );
        // Each watch before the app is built: it hears only the changes
        // after it is registered.
        let built = wire.find(" app(").expect("`wire` builds the app");
        for watch in ["watch_system(", "watch_reconfiguration("] {
            let at = wire
                .find(watch)
                .unwrap_or_else(|| panic!("`wire` does not call `{watch}`"));
            assert!(
                at < built,
                "`wire` builds the app before `{watch}`, and misses a change meanwhile"
            );
        }
        // And each source takes its wake from the table.
        for source in ["tray", "engine", "level", "displays", "system"] {
            let field = format!("WIRING.{source}");
            assert!(wire.contains(&field), "`wire` does not use `{field}`");
        }
    }

    #[test]
    fn every_start_turns_the_autofill_helper_off_before_appkit_or_a_window_exists() {
        // Nothing goes above the call, so that nothing can read the key first.
        let entrypoint = include_str!("entrypoint.rs");
        let wire = body(entrypoint, "pub(crate) fn wire(");
        let (_, statements) = wire
            .split_once("-> Shell<App> {\n")
            .expect("`wire`'s signature");
        assert!(
            statements
                .trim_start()
                .starts_with("ui::appkit::skip_autofill_helper();"),
            "turning the AutoFill helper off is not the first statement of `wire`"
        );
        // And the app and the driver each start through `wire`, whose shell
        // opens every window of theirs.
        for (path, source, call) in [
            ("entrypoint::run", entrypoint, "\n    wire("),
            ("drive::run", include_str!("drive.rs"), " entrypoint::wire("),
        ] {
            assert!(
                body(source, "pub fn run(").contains(call),
                "{path} does not start through `wire`"
            );
        }
    }

    // The engine's wake, tested with stubs that count their calls: the real
    // wakes raise AppKit flags that `idle.rs`'s tests read, and both sets of
    // tests run in one process.

    /// For the body of a test: the counters of the two wakes that
    /// `engine_wake` is given, and the stubs that count their calls. It
    /// defines `APP`, `LEVEL`, `wake_app`, `wake_level` and `counts()`, which
    /// returns the two counts. A test has its own, as tests run at the same
    /// time. Needs `use super::*;` before it.
    macro_rules! counting_wakes {
        () => {
            static APP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            static LEVEL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            fn wake_app(_: &Waker) {
                APP.fetch_add(1, Ordering::SeqCst);
            }
            fn wake_level(_: &Waker) {
                LEVEL.fetch_add(1, Ordering::SeqCst);
            }
            #[allow(dead_code)]
            fn counts() -> (usize, usize) {
                (APP.load(Ordering::SeqCst), LEVEL.load(Ordering::SeqCst))
            }
        };
    }

    #[test]
    fn an_app_status_wakes_the_app_whether_the_window_is_hidden_or_not() {
        use super::*;

        counting_wakes!();

        for hidden in [false, true] {
            let flag = Arc::new(AtomicBool::new(hidden));
            let wake = engine_wake(Waker::default(), flag, wake_app, wake_level);
            let (before, _) = counts();
            wake(engine::Reach::App);
            assert_eq!(counts().0, before + 1, "hidden: {hidden}");
        }
        assert_eq!(counts().1, 0, "never through `level`");
    }

    #[test]
    fn a_level_status_wakes_a_shown_window_once_per_call() {
        use super::*;

        counting_wakes!();

        let flag = Arc::new(AtomicBool::new(false));
        let wake = engine_wake(Waker::default(), flag, wake_app, wake_level);
        for calls in 1..=3 {
            wake(engine::Reach::Window);
            assert_eq!(counts().1, calls);
        }
        assert_eq!(counts().0, 0, "never through `app`");
    }

    #[test]
    fn a_level_status_is_dropped_for_a_hidden_window_and_an_app_status_is_not() {
        use super::*;

        counting_wakes!();

        let flag = Arc::new(AtomicBool::new(true));
        let wake = engine_wake(Waker::default(), flag.clone(), wake_app, wake_level);
        for _ in 0..3 {
            wake(engine::Reach::Window);
        }
        assert_eq!(counts(), (0, 0), "a hidden window hears no level status");
        wake(engine::Reach::App);
        assert_eq!(counts(), (1, 0), "the app still hears its own");
        flag.store(false, Ordering::Relaxed);
        wake(engine::Reach::Window);
        assert_eq!(counts(), (1, 1), "a shown window hears it again");
    }

    #[test]
    fn the_wake_reads_the_flag_at_each_call_not_when_it_is_built() {
        use super::*;

        counting_wakes!();

        let flag = Arc::new(AtomicBool::new(false));
        let wake = engine_wake(Waker::default(), flag.clone(), wake_app, wake_level);
        flag.store(true, Ordering::Relaxed);
        wake(engine::Reach::Window);
        assert_eq!(counts().1, 0, "raised after it was built");
        flag.store(false, Ordering::Relaxed);
        wake(engine::Reach::Window);
        assert_eq!(counts().1, 1, "lowered again");
    }

    #[test]
    fn a_window_hidden_in_a_pass_keeps_its_level_status_until_it_is_drawn_again() {
        use super::*;

        counting_wakes!();

        // The flag is the app's own, as `wire` hands it to the engine's wake.
        // The app leaves the Dock alone: its drawn pass runs on a test
        // thread, where AppKit may not be called.
        let (mut app, signals, _sent) = crate::app::detached_app(|| 12.0 * 60.0);
        let wake = engine_wake(
            Waker::default(),
            signals.window_hidden.clone(),
            wake_app,
            wake_level,
        );
        let ctx = egui::Context::default();

        occluded_pass(&ctx, &mut app);
        wake(engine::Reach::Window);
        assert_eq!(counts().1, 0, "dropped while occluded");

        drawn_pass(&ctx, &mut app);
        wake(engine::Reach::Window);
        assert_eq!(counts().1, 1, "heard once it is drawn");
    }

    #[test]
    fn a_level_status_sent_as_the_window_is_shown_again_is_not_lost() {
        use super::*;

        counting_wakes!();

        let signals = Signals::default();
        let wake = engine_wake(
            Waker::default(),
            signals.window_hidden.clone(),
            wake_app,
            wake_level,
        );
        // The engine's thread, placed in the one gap that matters: it sends a
        // status the moment a pass has polled, once the test arms it. The
        // pass has taken its events by then, so only the wake can bring the
        // status in.
        let sent = engine::Status {
            level: 0.5,
            ..engine::Status::default()
        };
        let armed = Arc::new(AtomicBool::new(false));
        let (engine, _commands) = Engine::detached_with({
            let armed = armed.clone();
            move |events| {
                if armed.swap(false, Ordering::SeqCst) {
                    events.send(engine::Event::Status(sent)).unwrap();
                    wake(engine::Reach::Window);
                }
            }
        });
        let mut app = crate::app::detached_app_on(engine, signals.clone(), || 12.0 * 60.0);
        let ctx = egui::Context::default();

        occluded_pass(&ctx, &mut app);
        assert!(signals.window_hidden.load(Ordering::Relaxed), "hidden");

        // The window is shown again, and the status arrives right after the
        // pass for it polled. The pass has lowered the flag by then, so the
        // wake is not dropped, and the pass it asks for polls the status in.
        armed.store(true, Ordering::SeqCst);
        drawn_pass(&ctx, &mut app);
        assert_eq!(counts().1, 1, "the window was woken for the status");
        assert_ne!(app.status, sent, "this pass polled before it was sent");
        drawn_pass(&ctx, &mut app);
        assert_eq!(app.status, sent, "the next pass brought it in");
        assert_eq!(counts().1, 1, "no other wake");
    }

    /// eframe's pass for an occluded window: `logic` alone.
    fn occluded_pass(ctx: &egui::Context, app: &mut super::App) {
        let mut input = egui::RawInput::default();
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .occluded = Some(true);
        let _ = ctx.run_logic(&input, |ctx| app.background_frame(Some(ctx)));
    }

    /// `Occluded(false)`: eframe draws one frame, UI and paint.
    fn drawn_pass(ctx: &egui::Context, app: &mut super::App) {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.background_frame(Some(ui.ctx()));
        });
        output.textures_delta.clear();
    }

    #[test]
    fn wire_builds_the_engines_wake_from_the_apps_hidden_flag_before_the_app() {
        let wire = body(include_str!("entrypoint.rs"), "pub(crate) fn wire(");
        // The old closure, inline in `wire`, would bypass the flag.
        assert!(
            !wire.contains("Reach::"),
            "`wire` decides on the engine's reach itself, not in `engine_wake`"
        );
        let wake = wire
            .find("engine_wake(")
            .expect("`wire` builds the engine's wake with `engine_wake`");
        let built = wire.find(" app(").expect("`wire` builds the app");
        assert!(wake < built, "`wire` builds the app before its wake");
        // The two wakes are the same type: swapped, they would still compile.
        let call = &wire[wake..built];
        let at = |part: &str| {
            call.find(part)
                .unwrap_or_else(|| panic!("`engine_wake` is not given `{part}`"))
        };
        let (hidden, app, level) = (
            at("signals.window_hidden"),
            at("WIRING.engine"),
            at("WIRING.level"),
        );
        assert!(hidden < app && app < level, "the arguments: {call}");
    }

    #[test]
    fn wire_starts_the_covers_build_only_for_a_window_it_opens_at_once() {
        // The start's window is asked for in `wire`, once the app is built,
        // unless the shell starts in the menu bar. The app and the driver
        // leave it to `wire`, or a build would start twice, or for no window.
        let entrypoint = include_str!("entrypoint.rs");
        let wire = body(entrypoint, "pub(crate) fn wire(");
        let built = wire.find(" app(").expect("`wire` builds the app");
        let shown = wire
            .find("\n    if !hidden {\n")
            .expect("`wire` opens the window at once unless hidden");
        let block = &wire[shown..];
        let block = &block[..block.find("\n    }\n").expect("the block's end")];
        assert!(
            block.contains("App::window_asked_for();"),
            "`wire` does not start the cover's build for the window it opens"
        );
        assert_eq!(
            wire.matches("window_asked_for").count(),
            1,
            "`wire` starts the cover's build outside `if !hidden`"
        );
        assert!(
            built < shown,
            "`wire` starts the cover's build before the app is built"
        );
        for (path, source) in [
            ("entrypoint::run", entrypoint),
            ("drive::run", include_str!("drive.rs")),
        ] {
            assert!(
                !body(source, "pub fn run(").contains("window_asked_for"),
                "{path} starts the cover's build itself, not through `wire`"
            );
        }
    }
}
