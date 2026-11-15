//! The app's state and the one place it changes.
//!
//! `App` owns the settings, the engine handle and the menu-bar item. Views
//! read it and push [`Action`]s; [`App::apply`] acts on them after drawing.
//! It lives through any number of windows: closing the window keeps it,
//! and the engine, running from the menu bar.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fastframe_shell::{Closed, Headless, Resident};
use fastframe_tray::Tray;

use crate::color::{self, Look};
use crate::engine::{self, Command, Engine, Event, Status};
use crate::idle;
use crate::model::Action;
use crate::painter::{self, WindowPainter};
use crate::schedule::Phase;
use crate::settings::{FADE_MAX_MINUTES, Settings};
use crate::ui::cover::prebuilt;

/// Settings wait this long after the last change before they are written,
/// so a slider drag writes once.
const SAVE_DELAY: Duration = Duration::from_millis(600);
/// The window's text depends on the clock ("Turns red at 21:00"), which
/// changes once a minute: an open window redraws when the minute turns, even
/// when nothing else happens ([`until_next_minute`]).
const MINUTE: Duration = Duration::from_secs(60);
/// With the window closed, the longest a headless tick waits: a wake cuts it
/// short, and a debounced save shortens it.
const HEADLESS_WAIT: Duration = Duration::from_secs(60);

/// Flags other threads raise for the main thread: a second launch asking
/// for the window, macOS reconfiguring a display, or another notice from
/// macOS that the tables or the clock may have moved.
#[derive(Clone, Default)]
pub struct Signals {
    pub show_window: Arc<AtomicBool>,
    pub displays_changed: Arc<AtomicBool>,
    /// The Mac or its screens woke, the user switched back, an app quit, or
    /// the clock or the time zone changed (`gamma::watch_system`).
    pub system_changed: Arc<AtomicBool>,
    /// The open window is occluded or minimized, as its own pass last found
    /// it ([`App::background_frame`]); a new window starts it lowered
    /// ([`App::attach`]). The one flag that the main thread raises for the
    /// others: the engine's wake leaves a hidden window alone for a status
    /// that only moves the level ([`engine::Reach::Window`]), because eframe
    /// draws nothing for it.
    pub window_hidden: Arc<AtomicBool>,
    /// Actions the demo driver sends from its own thread.
    #[cfg(feature = "demo")]
    pub injected: Arc<std::sync::Mutex<Vec<Action>>>,
}

/// What [`App::apply`] plays on the trackpad, as plain functions:
/// [`Haptics::TRACKPAD`] when the app runs, and nothing in tests, which must
/// not call AppKit or the trackpad's private actuator.
#[derive(Clone, Copy)]
pub struct Haptics {
    /// One tick, as the Temperature slider's detent catches or is passed.
    pub align: fn(),
    /// One thump, a beat of the Temperature slider's thumper.
    pub thump: fn(),
    /// One press, as a button takes a click from the pointer.
    pub press: fn(),
}

impl Haptics {
    /// The trackpad's own feedback, from [`crate::haptics`].
    pub const TRACKPAD: Self = Self {
        align: crate::haptics::align,
        thump: crate::haptics::thump,
        press: crate::haptics::press,
    };
}

pub struct App {
    pub settings: Settings,
    settings_path: Option<PathBuf>,
    changed_at: Option<Instant>,
    writer: fastframe_file::Writer,
    engine: Engine,
    tray: Option<Tray>,
    /// What the engine reports the displays show.
    pub status: Status,
    pub red_now: bool,
    /// The last error from the displays, shown in the status bar.
    pub error: Option<String>,
    pub actions: Vec<Action>,
    signals: Signals,
    clock: fn() -> f64,
    /// What `apply` plays on the trackpad. Tests may count its calls.
    pub haptics: Haptics,
    closing: Closed,
    /// The painter of the window made for this app since the last close; none
    /// when no window has been since. A close after a glow window releases what
    /// that window leaves ([`App::release_closed`]).
    window_held: Option<WindowPainter>,
    /// What lets go of a closed glow window's last frame:
    /// `ui::appkit::release_closed_window`. A test counts its calls.
    release_closed: fn(),
    /// What lets go of a closed Metal window's last surfaces:
    /// `painter::empty_closed_surface`. A test counts its calls.
    empty_closed: fn(),
    wants_show: bool,
    /// This window's painter failed, so the close under way opens the next
    /// window at once, on glow: [`App::note_painter_failure`].
    reopen_on_glow: bool,
    /// This window has asked for its close after a failure, once for good:
    /// `reopen_on_glow` does not say so, as a close the person asked for may
    /// have cleared it since, and a window nobody sees never set it.
    closed_for_failure: bool,
    /// Whether the Metal painter has failed in this process. A unit test gives
    /// this a flag of its own.
    painter_failed: fn() -> bool,
    quitting: bool,
    /// Whether this window has hidden the Dock icon yet: winit puts it back
    /// once AppKit finishes launching an unbundled run.
    dock_hidden: bool,
    tray_shown: Option<TrayState>,
    /// What a window asked for from the menu bar starts:
    /// [`App::window_asked_for`]. Nothing in this crate's unit tests, which
    /// leave the process's cover slot to the cover's own tests.
    prebuild_cover: fn(),
}

/// What the menu shows, to update it only when it changes.
#[derive(Clone, Debug, PartialEq)]
struct TrayState {
    status: String,
    red_now: bool,
    schedule_on: bool,
}

pub mod menu {
    pub const STATUS: &str = "status";
    pub const OPEN: &str = "open";
    pub const RED_NOW: &str = "red-now";
    pub const SCHEDULE: &str = "schedule";
    pub const QUIT: &str = "quit";
}

impl App {
    pub fn new(
        settings: Settings,
        settings_path: Option<PathBuf>,
        engine: Engine,
        tray: Option<Tray>,
        signals: Signals,
        clock: fn() -> f64,
        haptics: Haptics,
    ) -> Self {
        let closing = if tray.is_some() {
            Closed::Hide
        } else {
            Closed::Quit
        };
        Self {
            settings,
            settings_path,
            changed_at: None,
            writer: fastframe_file::Writer::new(),
            engine,
            tray,
            status: Status::default(),
            red_now: false,
            error: None,
            actions: Vec::new(),
            signals,
            clock,
            haptics,
            closing,
            window_held: None,
            release_closed: crate::ui::appkit::release_closed_window,
            empty_closed: painter::empty_closed_surface,
            wants_show: false,
            reopen_on_glow: false,
            closed_for_failure: false,
            painter_failed: painter::failed,
            quitting: false,
            dock_hidden: false,
            tray_shown: None,
            prebuild_cover: Self::window_asked_for,
        }
    }

    /// A window is about to open, at the start or from the menu bar: starts
    /// building the cover's images on a thread, for the main screen's scale,
    /// so that the window's first frame only uploads them, with the rim's
    /// light composed for its rise in a small box. The start calls
    /// this unless it starts hidden; every later window is asked for in
    /// [`Resident::headless_frame`], through the app's `prebuild_cover`.
    pub fn window_asked_for() {
        // The thread needs no handle: it ends by itself.
        let _ = prebuilt::window_asked_for(main_screen_scale);
    }

    /// A window was just made for this app, with `painter`.
    pub fn attach(&mut self, painter: WindowPainter) {
        self.closing = if self.tray.is_some() {
            Closed::Hide
        } else {
            Closed::Quit
        };
        self.wants_show = false;
        self.reopen_on_glow = false;
        self.closed_for_failure = false;
        self.dock_hidden = false;
        self.window_held = Some(painter);
        // A new window starts shown, whatever the last one was when it closed.
        self.signals.window_hidden.store(false, Ordering::Relaxed);
        if let Some(tray) = &mut self.tray {
            tray.attach();
        }
    }

    /// Whether the close under way reopens the window on glow: what a drive
    /// goes by, as a close the person asked for reopens nothing though the
    /// flag is up, and a quit wins over a reopen.
    #[cfg(feature = "demo")]
    pub fn reopening_on_glow(&self) -> bool {
        self.reopen_on_glow && !self.quitting
    }

    /// Whether the app holds its menu-bar item, for a drive's report.
    #[cfg(feature = "demo")]
    pub fn has_tray(&self) -> bool {
        self.tray.is_some()
    }

    pub fn minute_of_day(&self) -> f64 {
        (self.clock)()
    }

    /// Asks the engine to read the clock again: demo scenes move it by hand.
    #[cfg(feature = "demo")]
    pub fn clock_moved(&self) {
        self.engine.send(Command::Refresh);
    }

    /// Where the schedule stands, for the status line.
    pub fn phase(&self) -> Phase {
        self.settings.schedule().phase(self.minute_of_day())
    }

    /// Runs every pass, with or without a window: engine and menu events,
    /// actions, saving.
    pub fn background_frame(&mut self, ctx: Option<&egui::Context>) {
        // Before the poll, so that no status falls between the two: one sent
        // before the flag is lowered is in the channel by then, and one sent
        // after finds the window shown and wakes it, whether or not this
        // poll takes it. Lowered after the poll, a status sent in between
        // is neither polled nor woken for.
        if let Some(ctx) = ctx {
            self.note_window_hidden(ctx);
        }
        for event in self.engine.poll() {
            match event {
                Event::Status(status) => {
                    self.status = status;
                    if !status.failed {
                        self.error = None;
                    }
                }
                Event::Error(error) => self.error = Some(error),
            }
        }
        // macOS may have reset the tables, or moved the clock under the
        // schedule. Both flags are lowered, whichever was raised.
        let displays = self.signals.displays_changed.swap(false, Ordering::Relaxed);
        let system = self.signals.system_changed.swap(false, Ordering::Relaxed);
        if displays || system {
            self.engine.send(Command::Refresh);
        }
        if self.signals.show_window.swap(false, Ordering::Relaxed) {
            self.actions.push(Action::ShowWindow);
        }
        #[cfg(feature = "demo")]
        self.actions.append(
            &mut self
                .signals
                .injected
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        self.handle_tray();
        if let Some(ctx) = ctx {
            self.note_close_request(ctx);
            self.note_painter_failure(ctx);
            if !self.dock_hidden {
                #[cfg(target_os = "macos")]
                fastframe_dock::set_dock_icon_visible(false);
                self.dock_hidden = true;
            }
        }
        self.apply_actions(ctx);
        self.save_if_due();
        self.sync_tray();
        if let Some(ctx) = ctx {
            if self.wants_show {
                self.wants_show = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            // The next pass is due when the clock's text changes, or sooner
            // for a save. A hidden window gets it too: eframe runs only this
            // pass for it, which keeps the menu-bar item's line current.
            let clock_wait = until_next_minute(self.minute_of_day());
            let save_wait = self
                .changed_at
                .map(|at| SAVE_DELAY.saturating_sub(at.elapsed()));
            repaint_after(
                ctx,
                save_wait.map_or(clock_wait, |wait| wait.min(clock_wait)),
            );
        }
    }

    /// Tells the engine's wake whether the window is hidden: occluded or
    /// minimized, as eframe reads it (`ViewportInfo::visible`). eframe runs
    /// only `logic` for such a window, with no UI and no paint, so a status
    /// that only moves the level has no frame to draw. Showing the window
    /// again raises winit's `Occluded(false)`, which egui-winit answers with
    /// a repaint: that pass lowers this flag, then polls the engine, so it
    /// finds the latest status (see [`App::background_frame`]) and draws it.
    fn note_window_hidden(&self, ctx: &egui::Context) {
        self.signals
            .window_hidden
            .store(window_out_of_sight(ctx), Ordering::Relaxed);
    }

    /// A close the window itself asked for (the system, or Cmd+W) hides to
    /// the menu bar instead of quitting, when there is a menu bar item.
    fn note_close_request(&mut self, ctx: &egui::Context) {
        if crate::ui::closes(ctx) && !self.quitting && self.tray.is_some() {
            self.closing = Closed::Hide;
        }
    }

    /// A Metal window whose painter failed closes by the path of every close,
    /// so the engine hears `Command::Preview(None)` as for any close. It is
    /// not `Closed::Reopen`, which skips `window_gone` and so would leave a
    /// preview on the displays.
    ///
    /// The window the person sees gives way to a new one on glow at once
    /// ([`Resident::window_gone`]). It runs in `logic`, so a covered or
    /// minimized window reads the failure too, and that one closes to the menu
    /// bar and reopens nothing: a new centered window would come up in front
    /// and take the keyboard while the person works elsewhere. The next window
    /// they ask for is glow's.
    ///
    /// With no menu-bar item every failed window reopens, or the app would have
    /// no window and no way to quit, and this `Hide` stands where `closing`
    /// said `Quit`. A close the person asked for in this pass, or a quit under
    /// way, stays what it was. A glow window never closes itself.
    fn note_painter_failure(&mut self, ctx: &egui::Context) {
        if self.window_held == Some(WindowPainter::Metal)
            && !self.closed_for_failure
            && (self.painter_failed)()
            && !self.quitting
            && !crate::ui::closes(ctx)
        {
            self.closed_for_failure = true;
            self.closing = Closed::Hide;
            self.reopen_on_glow = self.tray.is_none() || !window_out_of_sight(ctx);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn handle_tray(&mut self) {
        let Some(tray) = &self.tray else { return };
        for event in tray.events() {
            let action = match event {
                fastframe_tray::Event::Show | fastframe_tray::Event::Toggle => Action::ShowWindow,
                fastframe_tray::Event::Menu(menu::OPEN) => Action::ShowWindow,
                fastframe_tray::Event::Menu(menu::RED_NOW) => Action::RedNow(!self.red_now),
                fastframe_tray::Event::Menu(menu::SCHEDULE) => {
                    Action::SetSchedule(!self.settings.schedule_on)
                }
                fastframe_tray::Event::Menu(menu::QUIT) => Action::Quit,
                fastframe_tray::Event::Menu(_) => continue,
            };
            self.actions.push(action);
        }
    }

    pub fn apply_actions(&mut self, ctx: Option<&egui::Context>) {
        let mut actions = std::mem::take(&mut self.actions);
        while !actions.is_empty() {
            for action in actions.drain(..) {
                self.apply(action, ctx);
            }
            actions = std::mem::take(&mut self.actions);
        }
    }

    pub fn apply(&mut self, action: Action, ctx: Option<&egui::Context>) {
        match action {
            Action::SetNight { look, dragging } => {
                let look = look.clamped();
                if look != self.settings.night {
                    self.settings.night = look;
                    self.settings_changed(ctx);
                }
                if dragging {
                    self.engine.send(Command::Preview(Some(look)));
                }
            }
            Action::EndPreview => self.engine.send(Command::Preview(None)),
            // After the frame's UI pass, on the main thread.
            Action::Detent => (self.haptics.align)(),
            Action::Thump => (self.haptics.thump)(),
            Action::Press => (self.haptics.press)(),
            Action::SetSchedule(on) => {
                self.settings.schedule_on = on;
                self.settings_changed(ctx);
            }
            Action::SetStart(time) => {
                self.settings.start = time;
                self.settings_changed(ctx);
            }
            Action::SetEnd(time) => {
                self.settings.end = time;
                self.settings_changed(ctx);
            }
            Action::SetFade(minutes) => {
                self.settings.fade_minutes = minutes.min(FADE_MAX_MINUTES);
                self.settings_changed(ctx);
            }
            Action::RedNow(on) => {
                self.red_now = on;
                self.engine.send(Command::RedNow(on));
            }
            Action::ShowWindow => self.wants_show = true,
            Action::HideWindow => {
                // A close the person asked for stays a close, and the next
                // window they ask for is glow's.
                self.reopen_on_glow = false;
                if self.tray.is_some() {
                    // A quit under way stays one, as in `note_close_request`:
                    // the way out takes no release of the window.
                    if !self.quitting {
                        self.closing = Closed::Hide;
                    }
                    if let Some(ctx) = ctx {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                } else {
                    self.apply(Action::Quit, ctx);
                }
            }
            Action::Quit => {
                self.quitting = true;
                self.closing = Closed::Quit;
                if let Some(ctx) = ctx {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    /// The interface shows a change at once; the engine and the file catch
    /// up behind it. With a window (`ctx`), the save asks for its own pass:
    /// a change made while the window draws comes after that pass's last
    /// request (`background_frame` runs before the UI), and without this the
    /// next pass would be the minute's, up to a minute away.
    fn settings_changed(&mut self, ctx: Option<&egui::Context>) {
        self.engine.send(Command::Configure(self.settings.clone()));
        self.changed_at = Some(Instant::now());
        if let Some(ctx) = ctx {
            repaint_after(ctx, SAVE_DELAY);
        }
    }

    fn save_if_due(&mut self) {
        if self.changed_at.is_some_and(|at| at.elapsed() >= SAVE_DELAY) {
            self.save_now();
        }
    }

    fn save_now(&mut self) {
        self.changed_at = None;
        if let Some(path) = &self.settings_path {
            let path = path.clone();
            let shown = path.display().to_string();
            self.writer.write_or(
                &path,
                self.settings.to_json(),
                fastframe_file::Access::Default,
                move || {
                    log::warn!("unable to save settings to {shown}");
                },
            );
        }
    }

    /// The line the window and the menu show about now and next.
    pub fn status_line(&self) -> String {
        if self.red_now {
            return "Red Now is on".to_owned();
        }
        if !self.settings.schedule_on {
            return "Schedule is off".to_owned();
        }
        match self.phase() {
            Phase::Never => "From and To match".to_owned(),
            Phase::Day { next_start } => format!("Turns red at {next_start}"),
            Phase::Night { until } => format!("Red until {until}"),
            Phase::Morning => "Fading back to normal".to_owned(),
        }
    }

    /// What the displays show right now, in words.
    pub fn shown_line(&self) -> String {
        if self.status.failed {
            return self
                .error
                .clone()
                .unwrap_or_else(|| "Could not change the colors".to_owned());
        }
        if self.status.tint.is_neutral() {
            return "Now: normal colors".to_owned();
        }
        if self.status.level >= 1.0 {
            // The look itself, which the card's title and the sliders below
            // show.
            return "Now: as set".to_owned();
        }
        let look = self.shown_look();
        let red = if look.is_pure_red() {
            "pure red".to_owned()
        } else {
            format!("{} K", look.kelvin)
        };
        if look.brightness < color::MAX_BRIGHTNESS {
            format!("Now: {red}, {}%", look.brightness)
        } else {
            format!("Now: {red}")
        }
    }

    /// The look the displays are at, partway through a fade.
    fn shown_look(&self) -> Look {
        let night = self.settings.night;
        let level = self.status.level.clamp(0.0, 1.0);
        let kelvin = color::kelvin_at_redness(color::redness_of(night.kelvin) * level);
        let brightness = 100.0 + (f32::from(night.brightness) - 100.0) * level;
        Look {
            kelvin,
            brightness: brightness.round() as u8,
        }
    }

    pub fn displays_line(&self) -> String {
        match self.status.displays {
            1 => "1 display".to_owned(),
            count => format!("{count} displays"),
        }
    }

    /// The card's third line: what the displays show and how many there
    /// are. When the tint failed it is the failure alone, since that line
    /// would not fit the card's column with the count after it.
    pub fn detail_line(&self) -> String {
        if self.status.failed {
            return self.shown_line();
        }
        format!("{} · {}", self.shown_line(), self.displays_line())
    }

    fn sync_tray(&mut self) {
        let state = TrayState {
            status: self.status_line(),
            red_now: self.red_now,
            schedule_on: self.settings.schedule_on,
        };
        if self.tray_shown.as_ref() == Some(&state) {
            return;
        }
        let Some(tray) = &mut self.tray else { return };
        tray.set_label(menu::STATUS, state.status.clone());
        tray.set_label(
            menu::RED_NOW,
            if state.red_now {
                "Stop Red Now"
            } else {
                "Go Red Now"
            },
        );
        tray.set_label(
            menu::SCHEDULE,
            if state.schedule_on {
                "Pause Schedule"
            } else {
                "Resume Schedule"
            },
        );
        tray.set_tooltip(format!("Arrakis: {}", state.status));
        self.tray_shown = Some(state);
    }

    pub fn tray_menu() -> Vec<fastframe_tray::MenuItem> {
        use fastframe_tray::MenuItem;
        vec![
            MenuItem::action(menu::STATUS, "Arrakis").enabled(false),
            MenuItem::Separator,
            MenuItem::action(menu::OPEN, "Open Arrakis..."),
            MenuItem::action(menu::RED_NOW, "Go Red Now"),
            MenuItem::action(menu::SCHEDULE, "Pause Schedule"),
            MenuItem::Separator,
            MenuItem::action(menu::QUIT, "Quit Arrakis"),
        ]
    }
}

/// How long from `minute` (minutes since midnight, with its fraction) to the
/// next whole minute: the clock's text changes then. Never zero, and never
/// more than a minute: on the minute itself, the whole of one; on a clock
/// that is not a number, a minute, for the next pass to read it again.
fn until_next_minute(minute: f64) -> Duration {
    let left = minute.floor() + 1.0 - minute;
    if left.is_finite() && left > 0.0 {
        Duration::from_secs_f64((left * 60.0).min(MINUTE.as_secs_f64()))
    } else {
        MINUTE
    }
}

/// Asks egui for a pass `wait` from now. egui takes its predicted frame time
/// (1/60 s unless the integration says otherwise) off every delay it is asked
/// for, so as not to overshoot, and a delay shorter than that becomes an
/// immediate repaint, which draws two frames: a bare request of the driver's
/// pointer period (1/120 s) held nothing, and its first window drew thousands
/// of frames a second whatever its pointer did. The time is added back here,
/// so that the pass lands at `wait`, not a frame before it: a clock's pass a
/// frame before the minute turns would draw the old minute. The driver
/// (`src/drive.rs`) asks through this too.
pub(crate) fn repaint_after(ctx: &egui::Context, wait: Duration) {
    let predicted = ctx.input(|input| input.predicted_dt);
    ctx.request_repaint_after(wait + Duration::try_from_secs_f32(predicted).unwrap_or_default());
}

/// The pixels to the point of the display the first window opens on, before
/// there is a window to ask: the screen with the menu bar, which is where
/// eframe centers it (winit's primary monitor), through AppKit's
/// `backingScaleFactor`, as winit reads it for the window. None off the
/// main thread, where AppKit may not be asked, and off macOS.
#[cfg(target_os = "macos")]
fn main_screen_scale() -> Option<f32> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;

    let screen = NSScreen::screens(MainThreadMarker::new()?).firstObject()?;
    Some(screen.backingScaleFactor() as f32)
}

#[cfg(not(target_os = "macos"))]
fn main_screen_scale() -> Option<f32> {
    None
}

/// Runs `body` in an autorelease pool: what AppKit autoreleases in it goes
/// when it returns.
#[cfg(target_os = "macos")]
fn pooled<R>(body: impl FnOnce() -> R) -> R {
    objc2::rc::autoreleasepool(|_| body())
}

/// Only macOS has autorelease pools.
#[cfg(not(target_os = "macos"))]
fn pooled<R>(body: impl FnOnce() -> R) -> R {
    body()
}

/// Whether the window is covered or minimized, as eframe reads it
/// (`ViewportInfo::visible`). One whose state eframe has not told yet, as in
/// its first frames, counts as seen.
fn window_out_of_sight(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.viewport().visible()) == Some(false)
}

impl Resident for App {
    fn closed(&self) -> Closed {
        self.closing
    }

    fn window_gone(&mut self) {
        self.wants_show = std::mem::take(&mut self.reopen_on_glow);
        // Let go of a preview a closing window may have left held.
        self.engine.send(Command::Preview(None));
        // The cover's images wait for no window now: a build still under way
        // is dropped when it finishes.
        prebuilt::window_gone();
        // The shell comes here with the window off the screen, so nothing shows
        // the surfaces a Metal window parked. After a glow window, or with no
        // window at all, there are none and this does nothing.
        (self.empty_closed)();
        // Only a glow window leaves what the release answers. A Metal window
        // has no GL context, a failed one included, whose next window is glow's
        // and comes back here; and the shell comes here at a hidden start too,
        // with no window at all. There the release would only load OpenGL. It
        // never comes here on a quit, when the process ends with what the
        // window left.
        if self.window_held.take() == Some(WindowPainter::Glow) {
            (self.release_closed)();
        }
    }

    fn headless_frame(&mut self, _ctx: &egui::Context) -> Headless {
        // The main thread has no pool between the shell's ticks, and `idle`'s
        // holds only the wait. Without this, what the tick autoreleases stays
        // for good: the menu's new labels (`sync_tray`) and the main screen's
        // list (`main_screen_scale`).
        pooled(|| {
            #[cfg(feature = "demo")]
            headless_counts::add();
            self.background_frame(None);
            // `idle` waits the whole of it in one pump, until the deadline this
            // publishes. Every answer goes through this one call.
            idle::publish(if self.quitting {
                Headless::Quit
            } else if self.wants_show {
                // Every way to ask for the window comes here with none open: a
                // click on the menu-bar item or its Open, and a second launch.
                (self.prebuild_cover)();
                Headless::Show
            } else {
                // The engine keeps its own time on its own thread; a wake cuts
                // this short for menu events, macOS's notices and the engine's
                // events that a closed window needs (`idle::wake`,
                // `idle::wake_paced`). A change from the menu is saved on time.
                let save_wait = self
                    .changed_at
                    .map(|at| SAVE_DELAY.saturating_sub(at.elapsed()));
                Headless::WaitFor(save_wait.map_or(HEADLESS_WAIT, |wait| wait.min(HEADLESS_WAIT)))
            })
        })
    }

    fn start_hidden(&mut self) -> bool {
        match &mut self.tray {
            Some(tray) => {
                tray.create_item();
                true
            }
            None => false,
        }
    }

    fn shutdown(&mut self) {
        if self.changed_at.is_some() {
            self.save_now();
        }
        self.engine.shutdown();
        if !self.writer.flush(fastframe_file::FLUSH_TIMEOUT) {
            log::warn!("settings were still being written at exit");
        }
    }
}

/// A screen that paints nothing, for demos and tests. Demo builds count what
/// the engine asks of it, for the driver's report.
pub struct NullScreen;

impl engine::Screen for NullScreen {
    fn apply(&mut self, _tint: color::Tint) -> Result<usize, String> {
        #[cfg(feature = "demo")]
        screen_counts::add(&screen_counts::PAINTS);
        Ok(1)
    }
    fn intact(&mut self) -> bool {
        #[cfg(feature = "demo")]
        screen_counts::add(&screen_counts::CHECKS);
        true
    }
    fn restore(&mut self) {
        #[cfg(feature = "demo")]
        screen_counts::add(&screen_counts::RESTORES);
    }
    fn display_count(&mut self) -> usize {
        1
    }
}

/// How many headless ticks the process has run, with its window closed: what
/// the driver reports as `headless`.
#[cfg(feature = "demo")]
pub mod headless_counts {
    use std::sync::atomic::{AtomicU64, Ordering};

    static FRAMES: AtomicU64 = AtomicU64::new(0);

    pub fn read() -> u64 {
        FRAMES.load(Ordering::Relaxed)
    }

    pub(super) fn add() {
        FRAMES.fetch_add(1, Ordering::Relaxed);
    }
}

/// What every [`NullScreen`] in the process was asked to do so far.
#[cfg(feature = "demo")]
pub mod screen_counts {
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(super) static CHECKS: AtomicU64 = AtomicU64::new(0);
    pub(super) static PAINTS: AtomicU64 = AtomicU64::new(0);
    pub(super) static RESTORES: AtomicU64 = AtomicU64::new(0);

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
    pub struct ScreenCounts {
        /// `intact` calls: the engine's checks of a tinted screen.
        pub checks: u64,
        /// `apply` calls: tables the engine would have painted.
        pub paints: u64,
        /// `restore` calls.
        pub restores: u64,
    }

    pub fn read() -> ScreenCounts {
        ScreenCounts {
            checks: CHECKS.load(Ordering::Relaxed),
            paints: PAINTS.load(Ordering::Relaxed),
            restores: RESTORES.load(Ordering::Relaxed),
        }
    }

    pub(super) fn add(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// An app on a `NullScreen` and `clock`, whose window asked for from the
/// menu bar starts no build of the cover: the cover's tests, which run at the
/// same time, own the process's slot.
#[cfg(test)]
pub fn test_app(clock: fn() -> f64) -> App {
    let settings = Settings::default();
    let engine = Engine::spawn(Box::new(NullScreen), settings.clone(), clock, |_| {});
    App {
        prebuild_cover: || {},
        ..App::new(
            settings,
            None,
            engine,
            None,
            Signals::default(),
            clock,
            Haptics::SILENT,
        )
    }
}

/// An app on `engine`, which a test made with no thread of its own
/// ([`Engine::detached`]), and `signals`, for a pass that a test thread runs:
/// the Dock icon counts as hidden already, so that the pass does not call
/// AppKit off the main thread, a window asked for from the menu bar starts
/// no build of the cover (see [`test_app`]), and the trackpad plays nothing.
#[cfg(test)]
pub(crate) fn detached_app_on(engine: Engine, signals: Signals, clock: fn() -> f64) -> App {
    App {
        dock_hidden: true,
        prebuild_cover: || {},
        ..App::new(
            Settings::default(),
            None,
            engine,
            None,
            signals,
            clock,
            Haptics::SILENT,
        )
    }
}

/// An app with no engine thread ([`detached_app_on`]): the signals it was
/// made with, and the commands it sends the engine.
#[cfg(test)]
pub(crate) fn detached_app(
    clock: fn() -> f64,
) -> (App, Signals, std::sync::mpsc::Receiver<Command>) {
    let signals = Signals::default();
    let (engine, sent) = Engine::detached();
    (
        detached_app_on(engine, signals.clone(), clock),
        signals,
        sent,
    )
}

/// What the [`Haptics::counting`] sinks have played on this test's thread.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Played {
    pub ticks: usize,
    pub thumps: usize,
    pub presses: usize,
}

#[cfg(test)]
thread_local! {
    static PLAYED: std::cell::Cell<Played> = const {
        std::cell::Cell::new(Played { ticks: 0, thumps: 0, presses: 0 })
    };
}

#[cfg(test)]
impl Haptics {
    /// Sinks that play nothing and count nothing: the test apps'.
    pub const SILENT: Self = Self {
        align: nothing,
        thump: nothing,
        press: nothing,
    };

    /// Sinks that play nothing and count what they would play on this
    /// test's thread, from zero: see [`played`].
    pub fn counting() -> Self {
        PLAYED.set(Played::default());
        Self {
            align: || count(|played| played.ticks += 1),
            thump: || count(|played| played.thumps += 1),
            press: || count(|played| played.presses += 1),
        }
    }
}

/// A haptic that plays nothing.
#[cfg(test)]
fn nothing() {}

/// Counts one haptic in [`PLAYED`], as `add` does.
#[cfg(test)]
fn count(add: fn(&mut Played)) {
    let mut played = PLAYED.get();
    add(&mut played);
    PLAYED.set(played);
}

/// What the [`Haptics::counting`] sinks have played on this test's thread
/// since the last were made.
#[cfg(test)]
pub fn played() -> Played {
    PLAYED.get()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::schedule::ClockTime;

    fn noon() -> f64 {
        12.0 * 60.0
    }

    fn midnight() -> f64 {
        0.0
    }

    #[test]
    fn status_line_follows_the_schedule() {
        let mut app = test_app(noon);
        assert_eq!(app.status_line(), "Turns red at 21:00");
        app.apply(Action::SetStart(ClockTime::new(11, 0)), None);
        assert_eq!(app.status_line(), "Red until 07:00");
        app.apply(Action::SetEnd(ClockTime::new(11, 0)), None);
        assert_eq!(app.status_line(), "From and To match");
        app.apply(Action::SetSchedule(false), None);
        assert_eq!(app.status_line(), "Schedule is off");
        app.apply(Action::RedNow(true), None);
        assert_eq!(app.status_line(), "Red Now is on");
    }

    #[test]
    fn night_look_is_clamped() {
        let mut app = test_app(midnight);
        app.apply(
            Action::SetNight {
                look: Look {
                    kelvin: 1,
                    brightness: 0,
                },
                dragging: false,
            },
            None,
        );
        assert_eq!(app.settings.night.kelvin, color::PURE_RED_KELVIN);
        assert_eq!(app.settings.night.brightness, color::MIN_BRIGHTNESS);
    }

    #[test]
    fn closing_without_a_menu_bar_item_quits() {
        let mut app = test_app(noon);
        app.apply(Action::HideWindow, None);
        assert_eq!(app.closed(), Closed::Quit);
    }

    #[test]
    fn the_detail_line_drops_the_count_when_the_tint_failed() {
        let mut app = test_app(noon);
        app.status.displays = 3;
        assert_eq!(app.detail_line(), "Now: normal colors · 3 displays");
        assert!(app.detail_line().ends_with(&app.displays_line()));

        // The fallback, and an error text: both stand alone.
        app.status.failed = true;
        let line = app.detail_line();
        assert_eq!(line, "Could not change the colors");
        assert!(!line.contains(" · ") && !line.contains("display"), "{line}");
        app.error = Some("the table was refused".to_owned());
        assert_eq!(app.detail_line(), "the table was refused");

        app.status.failed = false;
        assert!(app.detail_line().ends_with(&app.displays_line()));
    }

    #[test]
    fn shown_line_names_the_fade() {
        let mut app = test_app(midnight);
        app.settings.night = Look {
            kelvin: color::PURE_RED_KELVIN,
            brightness: 60,
        };
        app.status.level = 1.0;
        app.status.tint = app.settings.night.tint();
        assert_eq!(app.shown_line(), "Now: as set");
        app.status.level = 0.5;
        app.status.tint = app.settings.night.at_level(0.5);
        let line = app.shown_line();
        assert!(
            line.starts_with("Now: ") && line.ends_with(" K, 80%"),
            "{line}"
        );
        app.status.tint = color::Tint::NEUTRAL;
        app.status.level = 0.0;
        assert_eq!(app.shown_line(), "Now: normal colors");
    }

    #[test]
    fn each_haptic_action_plays_its_own_feedback_once() {
        let mut app = test_app(noon);
        app.haptics = Haptics::counting();
        let counts = |ticks, thumps, presses| Played {
            ticks,
            thumps,
            presses,
        };
        app.apply(Action::Thump, None);
        assert_eq!(played(), counts(0, 1, 0));
        // The actions a frame leaves are taken one by one.
        app.actions = vec![Action::Thump; 3];
        app.apply_actions(None);
        assert_eq!(played(), counts(0, 4, 0));
        // And a tick action plays a tick, never a thump.
        app.apply(Action::Detent, None);
        app.actions = vec![Action::Detent; 2];
        app.apply_actions(None);
        assert_eq!(played(), counts(3, 4, 0));
        // A press plays a press, never a tick or a thump.
        app.apply(Action::Press, None);
        assert_eq!(played(), counts(3, 4, 1));
        app.actions = vec![Action::Press; 2];
        app.apply_actions(None);
        assert_eq!(played(), counts(3, 4, 3));
    }

    /// What a window leaves for the app to play: a detent's tick, a thump of
    /// the thumper and a press, one by one and as a frame leaves them.
    fn haptic_actions(app: &mut App) {
        for action in [Action::Detent, Action::Thump, Action::Press] {
            app.apply(action, None);
        }
        app.actions = vec![Action::Detent, Action::Thump, Action::Press];
        app.apply_actions(None);
    }

    #[test]
    fn the_test_apps_play_nothing_however_they_are_made_and_a_counting_app_plays_the_same() {
        let (detached, _signals, _sent) = detached_app(noon);
        let (engine, _sent) = Engine::detached();
        let on = detached_app_on(engine, Signals::default(), noon);
        for (name, mut app) in [
            ("test_app", test_app(noon)),
            ("detached_app", detached),
            ("detached_app_on", on),
        ] {
            // Sinks that count are made to start the thread's count from
            // zero. The test app's reach none of them.
            let _ = Haptics::counting();
            haptic_actions(&mut app);
            assert_eq!(played(), Played::default(), "{name}");
            // The same actions, with sinks that count, are played once each
            // twice over.
            app.haptics = Haptics::counting();
            haptic_actions(&mut app);
            assert_eq!(
                played(),
                Played {
                    ticks: 2,
                    thumps: 2,
                    presses: 2
                },
                "{name}"
            );
        }
        // And the silent sinks themselves play and count nothing.
        let _ = Haptics::counting();
        for sink in [
            Haptics::SILENT.align,
            Haptics::SILENT.thump,
            Haptics::SILENT.press,
        ] {
            sink();
        }
        assert_eq!(played(), Played::default());
    }

    #[test]
    fn a_quiet_headless_tick_publishes_the_wait_it_asks_for() {
        let mut app = test_app(noon);
        let ctx = egui::Context::default();

        let before = Instant::now();
        let answer = app.headless_frame(&ctx);
        let after = Instant::now();

        // A long wait, not the shell's tick: that is the point of asking.
        let Headless::WaitFor(wait) = answer else {
            panic!("a quiet tick should ask to wait, not {answer:?}");
        };
        assert!(wait > fastframe_shell::HEADLESS_TICK, "{wait:?}");
        // What `idle` pumps until is what the answer asked for.
        let ends = idle::published().expect("the wait's end is published");
        assert!(
            ends >= before + wait && ends <= after + wait,
            "{ends:?} is not {wait:?} from the tick"
        );
    }

    #[test]
    fn a_change_from_the_menu_shortens_the_wait_to_its_save() {
        let mut app = test_app(noon);
        let ctx = egui::Context::default();
        // What a click on Pause Schedule pushes.
        app.actions.push(Action::SetSchedule(false));

        let answer = app.headless_frame(&ctx);
        let after = Instant::now();
        assert!(!app.settings.schedule_on, "the click was applied");
        let Headless::WaitFor(wait) = answer else {
            panic!("a tick with a save to come should wait, not {answer:?}");
        };
        // The save is due a debounce after the change, which this tick
        // made, not a minute later.
        assert!(wait <= SAVE_DELAY, "{wait:?}");
        let ends = idle::published().expect("the wait's end is published");
        assert!(ends <= after + SAVE_DELAY, "{ends:?} is past the save");
    }

    #[test]
    fn a_tick_saves_a_change_that_is_due_and_waits_long_again() {
        let mut app = test_app(noon);
        let ctx = egui::Context::default();
        app.apply(Action::SetSchedule(false), None);
        // The debounce has run out by the next tick.
        app.changed_at = Some(Instant::now() - SAVE_DELAY);

        let before = Instant::now();
        assert_eq!(app.headless_frame(&ctx), Headless::WaitFor(HEADLESS_WAIT));
        assert_eq!(app.changed_at, None, "the tick saved");
        let ends = idle::published().expect("the wait's end is published");
        assert!(ends >= before + HEADLESS_WAIT, "{ends:?}");
    }

    #[test]
    fn quitting_ends_the_published_wait() {
        let mut app = test_app(noon);
        let ctx = egui::Context::default();
        assert!(matches!(app.headless_frame(&ctx), Headless::WaitFor(_)));
        assert!(idle::published().is_some());

        app.apply(Action::Quit, None);
        assert_eq!(app.headless_frame(&ctx), Headless::Quit);
        assert_eq!(idle::published(), None);
    }

    #[test]
    fn a_requested_window_ends_the_published_wait() {
        let mut app = test_app(noon);
        let ctx = egui::Context::default();
        assert!(matches!(app.headless_frame(&ctx), Headless::WaitFor(_)));
        assert!(idle::published().is_some());

        app.apply(Action::ShowWindow, None);
        assert_eq!(app.headless_frame(&ctx), Headless::Show);
        assert_eq!(idle::published(), None);
    }

    #[test]
    fn quitting_wins_over_a_requested_window() {
        let mut app = test_app(noon);
        let ctx = egui::Context::default();
        app.apply(Action::ShowWindow, None);
        app.apply(Action::Quit, None);
        assert_eq!(app.headless_frame(&ctx), Headless::Quit);
        assert_eq!(idle::published(), None);
    }

    /// AppKit may be asked only on the main thread. A thread of the test's
    /// own is never it, whatever thread libtest runs the test on.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_main_screens_scale_is_not_asked_of_appkit_off_the_main_thread() {
        let scale = std::thread::spawn(main_screen_scale)
            .join()
            .expect("the thread ends");
        assert_eq!(scale, None);
    }

    #[test]
    fn a_requested_window_starts_the_covers_build_once_and_a_quiet_tick_does_not() {
        use std::sync::atomic::AtomicUsize;

        // A starter that counts, in place of the one on the process's slot,
        // which the cover's tests own. `tests/cover_prebuilt_show.rs` runs
        // the real one, alone in its binary.
        static STARTED: AtomicUsize = AtomicUsize::new(0);
        let mut app = test_app(noon);
        app.prebuild_cover = || {
            STARTED.fetch_add(1, Ordering::SeqCst);
        };
        let ctx = egui::Context::default();

        assert!(matches!(app.headless_frame(&ctx), Headless::WaitFor(_)));
        assert_eq!(STARTED.load(Ordering::SeqCst), 0, "no window was asked for");

        app.apply(Action::ShowWindow, None);
        assert_eq!(app.headless_frame(&ctx), Headless::Show);
        assert_eq!(
            STARTED.load(Ordering::SeqCst),
            1,
            "the window was asked for"
        );

        // A window asked for while quitting is never shown, nor built for.
        app.apply(Action::ShowWindow, None);
        app.apply(Action::Quit, None);
        assert_eq!(app.headless_frame(&ctx), Headless::Quit);
        assert_eq!(STARTED.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_notice_from_macos_sends_the_engine_a_refresh_and_is_lowered() {
        // What the app sends, read off an engine with no thread: no step
        // depends on how fast the machine is. What the engine does with a
        // refresh, a look at once and one a second later, is the engine's
        // own test, in synthetic time.
        for system in [true, false] {
            let signals = Signals::default();
            let flag = if system {
                signals.system_changed.clone()
            } else {
                signals.displays_changed.clone()
            };
            let (engine, sent) = Engine::detached();
            let mut app = App::new(
                Settings::default(),
                None,
                engine,
                None,
                signals,
                noon,
                Haptics::SILENT,
            );
            let refreshes = || {
                sent.try_iter()
                    .filter(|command| matches!(command, Command::Refresh))
                    .count()
            };

            app.background_frame(None);
            assert_eq!(refreshes(), 0, "{system}: a refresh with no notice");
            flag.store(true, Ordering::Relaxed);
            app.background_frame(None);
            assert!(!flag.load(Ordering::Relaxed), "one frame lowers it");
            assert_eq!(refreshes(), 1, "{system}: one refresh for the notice");
            app.background_frame(None);
            assert_eq!(refreshes(), 0, "{system}: the notice answered again");
        }
    }

    #[test]
    fn one_frame_answers_both_notices_and_leaves_neither_for_the_next() {
        let mut app = test_app(noon);
        let signals = app.signals.clone();
        signals.displays_changed.store(true, Ordering::Relaxed);
        signals.system_changed.store(true, Ordering::Relaxed);
        app.background_frame(None);
        assert!(!signals.displays_changed.load(Ordering::Relaxed));
        assert!(!signals.system_changed.load(Ordering::Relaxed));
    }

    // The window's repaint policy: the clock repaints at the next minute, a
    // hidden window is noted for the engine's wake, and showing it again asks
    // for no frame of the app's own.

    /// 12:00:15, a quarter of the way through a minute.
    fn at_15s() -> f64 {
        12.0 * 60.0 + 0.25
    }

    fn secs(seconds: f64) -> Duration {
        Duration::from_secs_f64(seconds)
    }

    #[track_caller]
    fn assert_near(got: Duration, wanted: Duration, margin: Duration, what: &str) {
        assert!(
            got.abs_diff(wanted) <= margin,
            "{what}: wanted {wanted:?} within {margin:?}, got {got:?}"
        );
    }

    /// What `App::background_frame` does to egui, read where eframe reads
    /// it: the delays of the repaint requests that reach the integration's
    /// callback, pass by pass, and the commands a pass sent the window.
    struct Passes {
        ctx: egui::Context,
        delays: Arc<Mutex<Vec<Duration>>>,
    }

    struct Ran {
        delays: Vec<Duration>,
        commands: Vec<egui::ViewportCommand>,
    }

    impl Passes {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let delays = Arc::new(Mutex::new(Vec::new()));
            let seen = Arc::clone(&delays);
            ctx.set_request_repaint_callback(move |info| seen.lock().unwrap().push(info.delay));
            let passes = Self { ctx, delays };
            // A new context repaints a couple of times on its own.
            for _ in 0..3 {
                passes.drawn(egui::RawInput::default(), |_| {});
            }
            passes
        }

        fn take(&self) -> Vec<Duration> {
            std::mem::take(&mut *self.delays.lock().unwrap())
        }

        /// A pass that eframe draws, with the UI and the paint.
        fn drawn(&self, input: egui::RawInput, mut pass: impl FnMut(&egui::Context)) -> Ran {
            let mut output = self.ctx.run_ui(input, |ui| pass(ui.ctx()));
            // There is no renderer to take the textures.
            output.textures_delta.clear();
            Ran {
                delays: self.take(),
                commands: output
                    .viewport_output
                    .into_values()
                    .flat_map(|viewport| viewport.commands)
                    .collect(),
            }
        }

        /// The pass eframe runs for an occluded or minimized window
        /// (`update_logic_only`): `App::logic` and nothing else.
        fn logic_only(&self, input: &egui::RawInput, pass: impl FnOnce(&egui::Context)) -> Ran {
            let output = self.ctx.run_logic(input, pass);
            Ran {
                delays: self.take(),
                commands: output.viewport_commands.into_values().flatten().collect(),
            }
        }
    }

    /// What the platform reports of the window, as eframe hands it to a pass.
    fn window(occluded: Option<bool>, minimized: Option<bool>) -> egui::RawInput {
        let mut input = egui::RawInput::default();
        let info = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        info.occluded = occluded;
        info.minimized = minimized;
        input
    }

    fn shown() -> egui::RawInput {
        window(Some(false), Some(false))
    }

    fn hidden() -> egui::RawInput {
        window(Some(true), Some(false))
    }

    /// An app that has hidden the Dock icon already, so that a pass leaves
    /// AppKit alone.
    fn windowed(clock: fn() -> f64) -> App {
        let mut app = test_app(clock);
        app.dock_hidden = true;
        app
    }

    #[test]
    fn the_wait_to_the_next_minute_is_what_is_left_of_this_one() {
        let cases = [
            // On the minute itself, the whole of the next one, not zero.
            ("on the minute", 720.0, secs(60.0)),
            ("a quarter in", 720.25, secs(45.0)),
            ("half in", 720.5, secs(30.0)),
            (
                "a millisecond before the next",
                720.0 + 59.999 / 60.0,
                secs(0.001),
            ),
            ("a millisecond past one", 721.0 + 0.001 / 60.0, secs(59.999)),
            ("across midnight, before", 1439.5, secs(30.0)),
            ("across midnight, nearly there", 1439.999, secs(0.06)),
            ("midnight", 0.0, secs(60.0)),
            ("a clock before midnight", -0.5, secs(30.0)),
            // A clock that is not a number: look again in a minute.
            ("not a number", f64::NAN, secs(60.0)),
            ("infinity", f64::INFINITY, secs(60.0)),
            ("minus infinity", f64::NEG_INFINITY, secs(60.0)),
        ];
        for (what, minute, wanted) in cases {
            assert_near(
                until_next_minute(minute),
                wanted,
                Duration::from_micros(1),
                what,
            );
        }
        // A hair before the boundary is a hair of a wait: never negative,
        // which `Duration` would panic on.
        let hair = until_next_minute(721.0 - 1e-9);
        assert!(hair <= Duration::from_micros(1), "a hair before: {hair:?}");
    }

    #[test]
    fn the_wait_ends_on_a_whole_minute_and_never_runs_past_one() {
        for minute in [0.0, 0.1, 0.5, 359.3333, 719.99, 1439.9999] {
            let wait = until_next_minute(minute);
            assert!(!wait.is_zero(), "{minute}: no wait");
            assert!(wait <= Duration::from_secs(60), "{minute}: {wait:?}");
            // The clock then reads a whole minute.
            let then = (minute * 60.0 + wait.as_secs_f64()) / 60.0;
            assert!(
                (then - then.round()).abs() * 60.0 < 1e-6,
                "{minute} + {wait:?} reads {then}"
            );
        }
    }

    #[test]
    fn the_next_pass_is_asked_for_at_the_next_minute() {
        // (what, the clock, the wait for the next pass)
        type Case = (&'static str, fn() -> f64, Duration);
        let cases: [Case; 4] = [
            ("a quarter in", at_15s, secs(45.0)),
            ("on the minute", || 12.0 * 60.0, secs(60.0)),
            // The case that fails if `repaint_after` is a bare
            // `request_repaint_after`: egui would take a frame off the 1 ms.
            (
                "a millisecond before the next",
                || 12.0 * 60.0 + 59.999 / 60.0,
                secs(0.001),
            ),
            (
                "a millisecond past one",
                || 13.0 * 60.0 + 0.001 / 60.0,
                secs(59.999),
            ),
        ];
        for (what, clock, wanted) in cases {
            let mut app = windowed(clock);
            let passes = Passes::new();
            let ran = passes.drawn(shown(), |ctx| app.background_frame(Some(ctx)));
            let [delay] = ran.delays[..] else {
                panic!("{what}: one request, got {:?}", ran.delays);
            };
            assert_ne!(delay, Duration::ZERO, "{what}: an immediate repaint");
            assert_near(delay, wanted, Duration::from_micros(10), what);
        }
    }

    #[test]
    fn a_short_wait_reaches_egui_as_itself_not_as_zero() {
        for wait in [Duration::from_millis(5), Duration::from_secs(50)] {
            let passes = Passes::new();
            let ran = passes.drawn(shown(), |ctx| repaint_after(ctx, wait));
            let [delay] = ran.delays[..] else {
                panic!("{wait:?}: one request, got {:?}", ran.delays);
            };
            assert_near(delay, wait, Duration::from_micros(10), "repaint_after");
        }
        // Why the helper: egui takes its predicted frame time off a bare
        // request, and a delay under it becomes an immediate repaint.
        let passes = Passes::new();
        let ran = passes.drawn(shown(), |ctx| {
            ctx.request_repaint_after(Duration::from_millis(5));
        });
        assert_eq!(ran.delays, [Duration::ZERO]);
    }

    #[test]
    fn a_save_due_sooner_than_the_minute_wins_the_wait() {
        let pass = |clock: fn() -> f64, changed_at: Option<Instant>| {
            let mut app = windowed(clock);
            app.changed_at = changed_at;
            let passes = Passes::new();
            let ran = passes.drawn(shown(), |ctx| app.background_frame(Some(ctx)));
            let [delay] = ran.delays[..] else {
                panic!("one request, got {:?}", ran.delays);
            };
            (delay, app.changed_at)
        };
        let margin = Duration::from_micros(10);
        // A change made at an instant that has not come yet: `Instant::elapsed`
        // saturates at zero for it, so the pass finds the whole of the save's
        // delay left, however long the setup and the passes take. The wait
        // then does not depend on the machine's speed.
        let just_made = Instant::now() + Duration::from_secs(3600);

        // Nothing to save: the minute's.
        let (delay, changed_at) = pass(at_15s, None);
        assert_near(delay, secs(45.0), margin, "nothing to save");
        assert_eq!(changed_at, None);

        // A change just made: the save's wait, well short of the minute's.
        let (delay, changed_at) = pass(at_15s, Some(just_made));
        assert_near(delay, SAVE_DELAY, margin, "the save's wait");
        assert!(changed_at.is_some(), "the change is not saved yet");

        // A minute that turns before the save: the minute's, the sooner.
        let (delay, changed_at) = pass(|| 12.0 * 60.0 + 59.9 / 60.0, Some(just_made));
        assert_near(
            delay,
            secs(0.1),
            Duration::from_micros(100),
            "the minute turns first",
        );
        assert!(changed_at.is_some(), "the change is not saved yet");

        // A save that is due: this pass writes it, and what is left to wait
        // for is the minute.
        let due = Instant::now()
            .checked_sub(SAVE_DELAY)
            .expect("an old instant");
        let (delay, changed_at) = pass(at_15s, Some(due));
        assert_eq!(changed_at, None, "the pass saved");
        assert_near(delay, secs(45.0), margin, "a save made");
    }

    #[test]
    fn a_change_made_while_the_window_draws_asks_for_a_pass_at_its_save() {
        // `Window::logic` runs `background_frame`, which asks for the minute's
        // pass; then `Window::ui` draws and `apply_actions` applies what the
        // user did. The change comes after that last request.
        let night = Look {
            kelvin: 2000,
            brightness: 70,
        };
        let changes = [
            (
                "night",
                Action::SetNight {
                    look: night,
                    dragging: true,
                },
            ),
            ("schedule", Action::SetSchedule(false)),
            ("start", Action::SetStart(ClockTime::new(22, 0))),
            ("end", Action::SetEnd(ClockTime::new(6, 0))),
            ("fade", Action::SetFade(30)),
        ];
        for (what, change) in changes {
            let (mut app, _signals, _sent) = detached_app(at_15s);
            let passes = Passes::new();
            let ran = passes.drawn(shown(), |ctx| {
                app.background_frame(Some(ctx));
                app.actions.push(change.clone());
                app.apply_actions(Some(ctx));
            });
            assert!(app.changed_at.is_some(), "{what}: the change was made");
            let soonest = ran.delays.iter().min().copied();
            assert_near(
                soonest.unwrap_or_default(),
                SAVE_DELAY,
                Duration::from_micros(10),
                what,
            );
            // The minute's request stands: eframe keeps the earlier of them.
            assert!(
                ran.delays
                    .iter()
                    .any(|delay| delay.abs_diff(secs(45.0)) <= Duration::from_micros(10)),
                "{what}: the minute's pass, got {:?}",
                ran.delays
            );
        }
    }

    #[test]
    fn an_action_that_changes_nothing_asks_for_no_save() {
        // The settings' own look, applied again, and an action with no
        // setting in it.
        let same = Settings::default().night;
        let nothing = [
            (
                "the same look",
                Action::SetNight {
                    look: same,
                    dragging: false,
                },
            ),
            ("the end of a preview", Action::EndPreview),
            ("Red Now", Action::RedNow(true)),
        ];
        for (what, action) in nothing {
            let (mut app, _signals, _sent) = detached_app(at_15s);
            let passes = Passes::new();
            let ran = passes.drawn(shown(), |ctx| app.apply(action.clone(), Some(ctx)));
            assert_eq!(app.changed_at, None, "{what}: nothing to save");
            assert!(ran.delays.is_empty(), "{what}: got {:?}", ran.delays);
        }
    }

    #[test]
    fn the_hidden_flag_follows_what_the_pass_found() {
        // (occluded, minimized, hidden): hidden when either is true, shown
        // only when both are known to be false, and shown while the platform
        // has not said.
        let cases = [
            (None, None, false),
            (Some(false), Some(false), false),
            (Some(false), None, false),
            (None, Some(false), false),
            (Some(true), None, true),
            (None, Some(true), true),
            (Some(true), Some(true), true),
            (Some(true), Some(false), true),
            (Some(false), Some(true), true),
        ];
        for (occluded, minimized, hidden) in cases {
            for logic_only in [false, true] {
                let (mut app, signals, _sent) = detached_app(at_15s);
                // From the opposite, to see the pass write it.
                signals.window_hidden.store(!hidden, Ordering::Relaxed);
                let passes = Passes::new();
                let input = window(occluded, minimized);
                if logic_only {
                    passes.logic_only(&input, |ctx| app.background_frame(Some(ctx)));
                } else {
                    passes.drawn(input, |ctx| app.background_frame(Some(ctx)));
                }
                assert_eq!(
                    signals.window_hidden.load(Ordering::Relaxed),
                    hidden,
                    "occluded {occluded:?}, minimized {minimized:?}, logic only {logic_only}"
                );
            }
        }
    }

    #[test]
    fn a_window_shown_again_lowers_the_hidden_flag() {
        let (mut app, signals, _sent) = detached_app(at_15s);
        let flag = signals.window_hidden;
        let passes = Passes::new();
        let mut pass = |logic_only: bool, input: egui::RawInput| {
            if logic_only {
                passes.logic_only(&input, |ctx| app.background_frame(Some(ctx)));
            } else {
                passes.drawn(input, |ctx| app.background_frame(Some(ctx)));
            }
            flag.load(Ordering::Relaxed)
        };
        assert!(!pass(false, egui::RawInput::default()), "a first pass");
        assert!(pass(true, hidden()), "hidden");
        assert!(!pass(false, shown()), "shown again");
        assert!(pass(true, window(Some(false), Some(true))), "minimized");
        assert!(!pass(false, egui::RawInput::default()), "unknown again");
    }

    #[test]
    fn a_new_window_starts_shown_whatever_the_last_one_was() {
        let (mut app, signals, _sent) = detached_app(at_15s);
        let passes = Passes::new();
        passes.logic_only(&hidden(), |ctx| app.background_frame(Some(ctx)));
        assert!(
            signals.window_hidden.load(Ordering::Relaxed),
            "it was hidden"
        );
        app.attach(WindowPainter::Glow);
        assert!(
            !signals.window_hidden.load(Ordering::Relaxed),
            "the new window is not"
        );
    }

    #[test]
    fn a_hidden_pass_still_runs_the_apps_pass() {
        let (mut app, signals, sent) = detached_app(at_15s);
        let passes = Passes::new();
        // A notice from macOS, and a change whose save came due.
        signals.system_changed.store(true, Ordering::Relaxed);
        app.changed_at = Some(
            Instant::now()
                .checked_sub(SAVE_DELAY)
                .expect("an old instant"),
        );

        let ran = passes.logic_only(&hidden(), |ctx| app.background_frame(Some(ctx)));

        assert!(
            signals.window_hidden.load(Ordering::Relaxed),
            "it is hidden"
        );
        assert!(!signals.system_changed.load(Ordering::Relaxed));
        let refreshes = sent
            .try_iter()
            .filter(|command| matches!(command, Command::Refresh))
            .count();
        assert_eq!(refreshes, 1, "the notice reached the engine");
        assert_eq!(app.changed_at, None, "the save was made");
        // The menu-bar item's line and the clock go on: the minute's wait.
        let [delay] = ran.delays[..] else {
            panic!("one request, got {:?}", ran.delays);
        };
        assert_near(
            delay,
            secs(45.0),
            Duration::from_micros(10),
            "the next pass",
        );
    }

    #[test]
    fn a_hidden_pass_raises_a_minimized_window_when_it_is_asked_to_open() {
        let (mut app, signals, _sent) = detached_app(at_15s);
        let passes = Passes::new();
        signals.show_window.store(true, Ordering::Relaxed);

        let ran = passes.logic_only(&window(Some(false), Some(true)), |ctx| {
            app.background_frame(Some(ctx))
        });

        assert!(!signals.show_window.load(Ordering::Relaxed));
        let unminimized = ran
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Minimized(false)));
        let focused = ran
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Focus));
        assert!(
            unminimized && focused,
            "the window was raised: {:?}",
            ran.commands
        );
    }

    #[test]
    fn showing_the_window_again_asks_for_one_frame_not_two() {
        let (mut app, signals, _sent) = detached_app(at_15s);
        let passes = Passes::new();
        let micro = Duration::from_micros(10);

        let ran = passes.logic_only(&hidden(), |ctx| app.background_frame(Some(ctx)));
        assert!(signals.window_hidden.load(Ordering::Relaxed));
        let [delay] = ran.delays[..] else {
            panic!("hidden: one request, got {:?}", ran.delays);
        };
        assert_near(delay, secs(45.0), micro, "hidden");

        // Occluded(false): eframe draws one frame. The app asks for no
        // repaint at once on top of it, which would draw a second, and the
        // pass after it finds none owed either.
        for pass in ["the frame eframe draws", "the one after"] {
            let ran = passes.drawn(shown(), |ctx| app.background_frame(Some(ctx)));
            assert!(
                !signals.window_hidden.load(Ordering::Relaxed),
                "{pass}: still hidden"
            );
            assert!(
                !ran.delays.contains(&Duration::ZERO),
                "{pass}: an immediate repaint, got {:?}",
                ran.delays
            );
            let [delay] = ran.delays[..] else {
                panic!("{pass}: one request, got {:?}", ran.delays);
            };
            assert_near(delay, secs(45.0), micro, pass);
        }
    }

    // A Metal window whose painter has failed. The process's flag is never touched here: each app
    // reads a `fn() -> bool` of the test's own (`painter_failed`). `tests/painter_fallback.rs` runs
    // the same path through a real `Shell`, in a process of its own.

    fn flag_up() -> bool {
        true
    }

    fn flag_down() -> bool {
        false
    }

    /// What `closing` is after a window is attached: `Quit` for the app without a tray, which is
    /// all that `detached_app` makes, and `Hide` for one with it, which `attach` would say and a
    /// case sets by hand.
    const BOTH_STARTS: [Closed; 2] = [Closed::Quit, Closed::Hide];

    /// An app with a window on `painter`, reading the failure through `failed`, whose `closing`
    /// is `closing`.
    fn windowed_on(
        painter: WindowPainter,
        failed: fn() -> bool,
        closing: Closed,
    ) -> (App, std::sync::mpsc::Receiver<Command>) {
        let (mut app, _signals, sent) = detached_app(at_15s);
        app.painter_failed = failed;
        app.attach(painter);
        app.closing = closing;
        (app, sent)
    }

    /// The pass in which eframe brings a `ViewportCommand::Close` back as a close request.
    fn close_requested() -> egui::RawInput {
        let mut input = shown();
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        input
    }

    /// The window minimized, so that eframe runs `logic` alone.
    fn minimized() -> egui::RawInput {
        window(Some(false), Some(true))
    }

    fn closes_in(ran: &Ran) -> usize {
        ran.commands
            .iter()
            .filter(|command| matches!(command, egui::ViewportCommand::Close))
            .count()
    }

    /// The previews the engine was sent, in order.
    fn previews(sent: &std::sync::mpsc::Receiver<Command>) -> Vec<Option<Look>> {
        sent.try_iter()
            .filter_map(|command| match command {
                Command::Preview(look) => Some(look),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_failed_metal_window_asks_for_its_close_once_over_several_passes() {
        for closing in BOTH_STARTS {
            let (mut app, _sent) = windowed_on(WindowPainter::Metal, flag_up, closing);
            let passes = Passes::new();
            let mut asked = 0;
            // Drawn and logic-only passes alike, as eframe runs them.
            for logic_only in [true, false, false, true, true, false] {
                let ran = if logic_only {
                    passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)))
                } else {
                    passes.drawn(shown(), |ctx| app.background_frame(Some(ctx)))
                };
                asked += closes_in(&ran);
            }
            assert_eq!(asked, 1, "started as {closing:?}");
            assert_eq!(app.closed(), Closed::Hide, "started as {closing:?}");
            assert!(app.reopen_on_glow);
        }
    }

    #[test]
    fn the_failure_is_read_on_the_pass_after_it_comes_and_not_before() {
        static UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        fn up() -> bool {
            UP.load(Ordering::SeqCst)
        }
        let (mut app, _sent) = windowed_on(WindowPainter::Metal, up, Closed::Quit);
        let passes = Passes::new();
        for _ in 0..3 {
            let ran = passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)));
            assert_eq!(closes_in(&ran), 0);
        }
        assert_eq!(app.closed(), Closed::Quit);
        assert!(!app.reopen_on_glow);

        UP.store(true, Ordering::SeqCst);
        let ran = passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)));
        assert_eq!(closes_in(&ran), 1);
        assert_eq!(app.closed(), Closed::Hide);
    }

    /// A window nobody sees reads the failure and closes. `detached_app` has no tray, so this is the
    /// exception: with no menu-bar item it still reopens, or the app has no window and no way to quit.
    /// The rule itself, that with one it reopens nothing, needs a real `Tray`:
    /// `tests/painter_fallback.rs`, case 5.
    #[test]
    fn a_covered_or_minimized_window_with_no_menu_bar_item_closes_and_still_reopens() {
        for (what, input) in [("covered", hidden()), ("minimized", minimized())] {
            let (mut app, _sent) = windowed_on(WindowPainter::Metal, flag_up, Closed::Quit);
            let passes = Passes::new();
            let ran = passes.logic_only(&input, |ctx| app.background_frame(Some(ctx)));
            assert_eq!(closes_in(&ran), 1, "{what}");
            assert_eq!(app.closed(), Closed::Hide, "{what}");
            assert!(app.reopen_on_glow, "{what}");
            app.window_gone();
            assert_eq!(
                app.headless_frame(&egui::Context::default()),
                Headless::Show,
                "{what}"
            );
        }
    }

    #[test]
    fn the_close_that_comes_back_leaves_hide_also_where_the_app_said_quit() {
        for closing in BOTH_STARTS {
            let (mut app, _sent) = windowed_on(WindowPainter::Metal, flag_up, closing);
            let passes = Passes::new();
            let ran = passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)));
            assert_eq!(closes_in(&ran), 1, "started as {closing:?}");
            // eframe answers our Close with a close request on the next pass.
            let ran = passes.logic_only(&close_requested(), |ctx| app.background_frame(Some(ctx)));
            assert_eq!(closes_in(&ran), 0, "no second ask, started as {closing:?}");
            assert_eq!(app.closed(), Closed::Hide, "started as {closing:?}");
            assert!(app.reopen_on_glow, "the close is still ours");
        }
    }

    #[test]
    fn a_window_on_glow_never_closes_itself_with_the_flag_up() {
        for closing in BOTH_STARTS {
            let (mut app, _sent) = windowed_on(WindowPainter::Glow, flag_up, closing);
            let passes = Passes::new();
            for logic_only in [true, false, true] {
                let ran = if logic_only {
                    passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)))
                } else {
                    passes.drawn(shown(), |ctx| app.background_frame(Some(ctx)))
                };
                assert_eq!(closes_in(&ran), 0, "started as {closing:?}");
            }
            assert_eq!(app.closed(), closing);
            assert!(!app.reopen_on_glow);
            // A close request of the person's, on an app with no tray, is a quit as it was.
            passes.logic_only(&close_requested(), |ctx| app.background_frame(Some(ctx)));
            assert_eq!(app.closed(), closing);
        }
    }

    #[test]
    fn a_metal_window_with_the_flag_down_sends_no_close() {
        for closing in BOTH_STARTS {
            let (mut app, _sent) = windowed_on(WindowPainter::Metal, flag_down, closing);
            let passes = Passes::new();
            for logic_only in [true, false, true] {
                let ran = if logic_only {
                    passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)))
                } else {
                    passes.drawn(shown(), |ctx| app.background_frame(Some(ctx)))
                };
                assert_eq!(closes_in(&ran), 0, "started as {closing:?}");
            }
            assert_eq!(app.closed(), closing);
            assert!(!app.reopen_on_glow);
        }
    }

    #[test]
    fn a_quit_under_way_stays_a_quit_and_the_failure_adds_no_close_of_ours() {
        for closing in BOTH_STARTS {
            let (mut app, _sent) = windowed_on(WindowPainter::Metal, flag_up, closing);
            let passes = Passes::new();
            // No ctx, so that the quit itself sends no Close: any Close seen below is the failure's.
            app.apply(Action::Quit, None);
            let ran = passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)));
            assert_eq!(closes_in(&ran), 0, "started as {closing:?}");
            assert_eq!(app.closed(), Closed::Quit, "started as {closing:?}");
            assert!(!app.reopen_on_glow);
            #[cfg(feature = "demo")]
            assert!(!app.reopening_on_glow());
        }
    }

    #[test]
    fn a_close_the_person_asked_for_in_the_failing_pass_does_not_reopen() {
        for closing in BOTH_STARTS {
            let (mut app, sent) = windowed_on(WindowPainter::Metal, flag_up, closing);
            let passes = Passes::new();
            let ran = passes.logic_only(&close_requested(), |ctx| app.background_frame(Some(ctx)));
            assert_eq!(closes_in(&ran), 0, "started as {closing:?}");
            assert_eq!(app.closed(), closing, "it stays what it was");
            assert!(!app.reopen_on_glow);
            // The close goes by the shell: `window_gone`, and no window after it.
            app.window_gone();
            assert!(!app.wants_show);
            assert!(matches!(
                app.headless_frame(&egui::Context::default()),
                Headless::WaitFor(_)
            ));
            assert_eq!(previews(&sent), [None], "the preview is still let go");
        }
    }

    #[test]
    fn a_quit_after_the_failure_still_wins() {
        for closing in BOTH_STARTS {
            let (mut app, _sent) = windowed_on(WindowPainter::Metal, flag_up, closing);
            let passes = Passes::new();
            passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)));
            assert!(app.reopen_on_glow);
            app.apply(Action::Quit, None);
            assert_eq!(app.closed(), Closed::Quit, "started as {closing:?}");
            #[cfg(feature = "demo")]
            assert!(!app.reopening_on_glow(), "a quit overtakes the reopen");
            assert_eq!(
                app.headless_frame(&egui::Context::default()),
                Headless::Quit,
                "started as {closing:?}"
            );
        }
    }

    #[test]
    fn a_hide_after_the_failure_in_an_app_with_no_tray_is_a_quit_and_reopens_nothing() {
        // With a tray the hide clears the reopen too: `tests/painter_fallback.rs` reads that one on
        // a real `Tray`, which `detached_app` has none of.
        let (mut app, _sent) = windowed_on(WindowPainter::Metal, flag_up, Closed::Quit);
        let passes = Passes::new();
        passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)));
        assert!(app.reopen_on_glow);
        app.apply(Action::HideWindow, None);
        assert!(!app.reopen_on_glow);
        assert_eq!(app.closed(), Closed::Quit);
        #[cfg(feature = "demo")]
        assert!(!app.reopening_on_glow());
    }

    #[test]
    fn the_failed_windows_close_lets_go_of_a_held_preview_and_the_next_window_opens_once() {
        for closing in BOTH_STARTS {
            let (mut app, sent) = windowed_on(WindowPainter::Metal, flag_up, closing);
            let passes = Passes::new();
            // A drag of the Temperature slider holds a preview on the displays.
            let look = Look::default();
            app.apply(
                Action::SetNight {
                    look,
                    dragging: true,
                },
                None,
            );
            let ran = passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)));
            assert_eq!(closes_in(&ran), 1, "started as {closing:?}");

            // The shell sees `Hide` and calls `window_gone`, which is what lets the preview go.
            assert_eq!(app.closed(), Closed::Hide);
            app.window_gone();
            assert_eq!(
                previews(&sent),
                [Some(look), None],
                "the preview, then its end, started as {closing:?}"
            );
            assert!(app.window_held.is_none());

            // Its first headless pass opens the next window, and `attach` is that window's.
            let ctx = egui::Context::default();
            assert_eq!(app.headless_frame(&ctx), Headless::Show);
            app.attach(WindowPainter::Glow);
            app.closing = Closed::Hide;
            // That window closes as any other does, and nothing reopens it.
            let ran = passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)));
            assert_eq!(closes_in(&ran), 0);
            app.window_gone();
            assert!(!app.wants_show);
            assert!(
                matches!(app.headless_frame(&ctx), Headless::WaitFor(_)),
                "the reopen was handed over once"
            );
        }
    }

    #[test]
    fn a_new_metal_window_with_the_flag_up_starts_clean_and_asks_again() {
        let (mut app, _sent) = windowed_on(WindowPainter::Metal, flag_up, Closed::Quit);
        let passes = Passes::new();
        passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)));
        assert!(app.reopen_on_glow && app.closed_for_failure);
        // A window made after that, on Metal: `renderer()` would not give one with the flag up, but
        // `attach` takes what it is told.
        app.attach(WindowPainter::Metal);
        assert!(!app.reopen_on_glow, "attach clears the reopen");
        assert!(!app.closed_for_failure, "attach clears the one ask");
        assert!(!app.wants_show);
        let ran = passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)));
        assert_eq!(closes_in(&ran), 1, "it asks again");
    }

    #[test]
    fn a_window_gone_with_no_failure_reopens_nothing() {
        for painter in [WindowPainter::Metal, WindowPainter::Glow] {
            let (mut app, sent) = windowed_on(painter, flag_down, Closed::Hide);
            app.window_gone();
            assert!(!app.wants_show, "{painter:?}");
            assert!(app.window_held.is_none());
            assert_eq!(previews(&sent), [None], "{painter:?}");
            assert!(matches!(
                app.headless_frame(&egui::Context::default()),
                Headless::WaitFor(_)
            ));
        }
    }

    // What a close calls once its window has gone: the release after a glow window only, and the
    // emptying of all the surfaces a Metal window still holds at every close. Each app reads two
    // `fn()`s of the test's own, which count in the test's thread. A run on one thread
    // (`--test-threads=1`) runs every test on it, so `counting_closes` starts both counts at zero. The
    // real release loads OpenGL, and is not called here. The real emptying is called once, below: it
    // has nothing parked on a test's thread unless the test parks a surface itself.

    thread_local! {
        static RELEASES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        static EMPTIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    fn count_a_release() {
        RELEASES.set(RELEASES.get() + 1);
    }

    fn count_an_emptying() {
        EMPTIES.set(EMPTIES.get() + 1);
    }

    /// Makes `app` count its closes from zero.
    fn counting_closes(app: &mut App) {
        RELEASES.set(0);
        EMPTIES.set(0);
        app.release_closed = count_a_release;
        app.empty_closed = count_an_emptying;
    }

    /// The releases and the emptyings counted since `counting_closes`.
    fn counted() -> (usize, usize) {
        (RELEASES.get(), EMPTIES.get())
    }

    #[test]
    fn a_metal_windows_close_calls_no_release_and_empties_the_closed_surface() {
        for closing in BOTH_STARTS {
            let (mut app, _sent) = windowed_on(WindowPainter::Metal, flag_down, closing);
            counting_closes(&mut app);
            app.window_gone();
            assert_eq!(counted(), (0, 1), "started as {closing:?}");
        }
    }

    /// The production `empty_closed`, which every other close test replaces with a counter: a real surface
    /// parked as a close parks it, the close through the real constructor, and the cell then has none
    /// left to empty. The cell is `thread_local!`, and this test's thread has it to itself.
    #[cfg(all(target_os = "macos", feature = "demo"))]
    #[test]
    fn a_metal_windows_close_empties_a_real_parked_surface_through_the_real_constructor() {
        for closing in BOTH_STARTS {
            eframe::egui_wgpu::empty_closed_surface();
            if !eframe::egui_wgpu::park_a_surface_for_a_test() {
                eprintln!("skipped: the system gave no surface");
                return;
            }
            let (mut app, _sent) = windowed_on(WindowPainter::Metal, flag_down, closing);

            app.window_gone();

            assert!(
                !eframe::egui_wgpu::empty_closed_surface(),
                "the close left the surface parked, started as {closing:?}"
            );
        }
    }

    #[test]
    fn a_glow_windows_close_calls_the_release_once_and_a_second_close_with_nothing_held_calls_none()
    {
        for closing in BOTH_STARTS {
            let (mut app, _sent) = windowed_on(WindowPainter::Glow, flag_down, closing);
            counting_closes(&mut app);
            app.window_gone();
            assert_eq!(counted(), (1, 1), "started as {closing:?}");
            // The shell comes here again with no window made since: nothing is left to release.
            app.window_gone();
            assert_eq!(
                counted().0,
                1,
                "the release answers a glow window once, started as {closing:?}"
            );
        }
    }

    #[test]
    fn a_failed_metal_windows_close_calls_no_release_and_its_glow_successors_calls_one() {
        for closing in BOTH_STARTS {
            let (mut app, _sent) = windowed_on(WindowPainter::Metal, flag_up, closing);
            counting_closes(&mut app);
            let passes = Passes::new();
            let ran = passes.logic_only(&shown(), |ctx| app.background_frame(Some(ctx)));
            assert_eq!(closes_in(&ran), 1, "started as {closing:?}");
            assert!(app.reopen_on_glow);

            // The failed window's close: no GL context to release, and its surface emptied.
            app.window_gone();
            assert_eq!(
                counted(),
                (0, 1),
                "the failed window, started as {closing:?}"
            );

            // The next window opens at once, on glow, and its close is the one the release answers.
            let ctx = egui::Context::default();
            assert_eq!(app.headless_frame(&ctx), Headless::Show);
            app.attach(WindowPainter::Glow);
            app.closing = Closed::Hide;
            app.window_gone();
            assert_eq!(
                counted(),
                (1, 2),
                "the glow window after it, started as {closing:?}"
            );
        }
    }

    #[test]
    fn a_hidden_start_with_no_window_ever_releases_nothing_and_empties_once_a_call() {
        let (mut app, _signals, _sent) = detached_app(at_15s);
        assert!(app.window_held.is_none());
        counting_closes(&mut app);
        app.window_gone();
        assert_eq!(counted(), (0, 1));
        app.window_gone();
        assert_eq!(counted(), (0, 2), "one call each time the shell comes");
    }
}
