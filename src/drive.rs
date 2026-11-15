//! `--demo-drive`: the app's real lifecycle, timed. The shell, the menu-bar
//! item and its idle pump, the engine thread and the window all run as they
//! do for real, with three differences: the engine paints [`NullScreen`], so
//! the displays are never touched; there is no settings file and no instance
//! slot; and the clock starts at the scenario's moment and runs at real
//! speed. The driver moves the pointer through the window's
//! `raw_input_hook`, quits by itself and writes a JSON report.
//!
//! Demo builds only.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use fastframe_shell::Waker;

use crate::app::{self, App, Haptics, NullScreen, Signals, repaint_after};
use crate::engine::{Engine, Reach};
use crate::model::Action;
use crate::painter::{self, WindowPainter};
use crate::settings::Settings;
use crate::ui::cover::counts::{self as cover_counts, CoverCounts};
use crate::{entrypoint, idle, theme, ui};

/// What the driver does. The moments are for the default schedule: night
/// from 21:00 to 07:00, with 20-minute fades.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scenario {
    /// In the menu bar by day, at 14:20.
    ClosedDay,
    /// In the menu bar, tinted at rest, at 23:30.
    ClosedNight,
    /// In the menu bar through the evening fade, from 21:00.
    ClosedFade,
    /// The window open by day, with no input.
    OpenIdle,
    /// The window open, tinted at rest, at 23:30, with no input: the engine
    /// checks the displays behind it, which it does on no other open drive.
    OpenNight,
    /// The window open through the evening fade, with no input.
    OpenFade,
    /// The window open, the Temperature slider dragged end to end.
    Drag,
    /// The window open, the Temperature slider dragged out and back and let
    /// go, 2 s of rest, and again: a paused display link has to come back.
    RestDrag,
    /// The window opened, left idle 1 s, closed, and left closed 2 s; one
    /// cycle per 3 s of the run.
    OpenClose,
    /// `open-close` with a frame asked for on every frame, so that each close
    /// comes inside a burst, before the link's pause, as a click after a moved
    /// pointer does.
    OpenCloseBurst,
    /// The window's first frame, then quit.
    Start,
}

impl Scenario {
    /// Where the clock starts, in minutes since midnight.
    pub fn moment(self) -> f64 {
        match self {
            Self::ClosedNight | Self::OpenNight => 23.0 * 60.0 + 30.0,
            Self::ClosedFade | Self::OpenFade => 21.0 * 60.0,
            _ => 14.0 * 60.0 + 20.0,
        }
    }

    /// Whether the app starts in the menu bar, its window closed.
    pub fn starts_hidden(self) -> bool {
        matches!(
            self,
            Self::ClosedDay
                | Self::ClosedNight
                | Self::ClosedFade
                | Self::OpenClose
                | Self::OpenCloseBurst
        )
    }

    /// How many times `open-close` opens the window in a run of `seconds`.
    pub fn cycles(seconds: u64) -> usize {
        ((seconds as f64 / CYCLE.as_secs_f64()).round() as usize).max(1)
    }

    /// The `elapsed_s` of a healthy run of `seconds`, by how the scenario ends (`direct`):
    ///
    /// - **Timed** (the closed and open scenarios, `drag` and `rest-drag`): the driver sleeps
    ///   until `seconds` after the start and then quits, so `seconds` to `seconds` plus
    ///   [`TIMED_SLACK_S`].
    /// - **Cycled** (`open-close` and `open-close-burst`): [`Scenario::cycles`] cycles, then a
    ///   quit. [`CYCLE`] a cycle at least, and [`CYCLE`] plus [`CYCLE_OVERRUN_S`] a cycle at most,
    ///   plus [`CYCLED_SLACK_S`]. A cycle that breaks off, when a window does not open or close,
    ///   is no matter of time: `direct` gives up only after [`PATIENCE`], so the run can end
    ///   inside this bound, and the report is spoiled by the cycle ([`BrokeOff`]) whatever its
    ///   time.
    /// - **The start** quits by itself on its second frame, and `seconds` is only its deadline.
    ///   Its time is what it measures, so a slow start is a finding and not a spoiled run: it is
    ///   spoiled only when the deadline quit it, at `seconds` or more.
    pub fn bound(self, seconds: u64) -> Bound {
        match self {
            Self::OpenClose | Self::OpenCloseBurst => {
                let cycles = Self::cycles(seconds) as f64;
                let cycle = CYCLE.as_secs_f64();
                Bound::Between {
                    from: cycles * cycle,
                    to: cycles * (cycle + CYCLE_OVERRUN_S) + CYCLED_SLACK_S,
                }
            }
            Self::Start => Bound::Under(seconds as f64),
            Self::ClosedDay
            | Self::ClosedNight
            | Self::ClosedFade
            | Self::OpenIdle
            | Self::OpenNight
            | Self::OpenFade
            | Self::Drag
            | Self::RestDrag => Bound::Between {
                from: seconds as f64,
                to: seconds as f64 + TIMED_SLACK_S,
            },
        }
    }
}

/// One open-close cycle: open, 1 s idle, close, 2 s closed.
const CYCLE: Duration = Duration::from_secs(3);
const IDLE_OPEN: Duration = Duration::from_secs(1);
const IDLE_CLOSED: Duration = Duration::from_secs(2);
/// How far past `seconds` a timed drive's `elapsed_s` may be: its start and its quit. Drives on
/// record read 0.06 to 0.09 s past it. The scripts' own checks hold the same figure, as `SLACK` in
/// `scripts/measure/energy-drag.py`, `energy-system.py` and `tear-scan.py`, and a test holds the
/// copies to this one.
const TIMED_SLACK_S: f64 = 2.0;
/// What one cycle of `open-close` may take beyond [`CYCLE`]: twice the 0.11 to 0.125 s that drives
/// read. `OVERRUN` in `scripts/measure/close-scan.py`.
const CYCLE_OVERRUN_S: f64 = 0.25;
/// What a cycled run takes beyond its cycles: the start and the end. `SLACK` in
/// `scripts/measure/close-scan.py`.
const CYCLED_SLACK_S: f64 = 4.0;
/// How long the driver waits for a window to open or close before giving up
/// on the cycle.
const PATIENCE: Duration = Duration::from_secs(10);
/// How long the app has to quit once asked before the driver ends it.
const GRACE: Duration = Duration::from_secs(10);
/// Under `ARRAKIS_METAL_FAIL=rest`, how far on a pass that has not come from
/// rest ([`painter::AT_REST`]) asks for the frame that will.
const FRAME_FROM_REST: Duration = Duration::from_secs(2);

/// The Temperature slider's row, and its track from the Day end (redness 0)
/// to the far end, in points: `ui::view`'s own, so a change to the layout
/// moves the drag with it. The Brightness slider's row is for the
/// benchmarks.
pub const TEMPERATURE_Y: f32 = ui::view::TEMPERATURE_Y;
pub const BRIGHTNESS_Y: f32 = ui::view::BRIGHTNESS_Y;
pub const TRACK: (f32, f32) = ui::view::TRACK;
/// A sweep along the track and back.
pub const SWEEP: Duration = Duration::from_secs(2);
/// How many times a second the drag's pointer moves: the refresh rate of the
/// owner's display, whose frames each carry a pointer event. [`Drag`] makes
/// a move only once a period (`1 / POINTER_RATE`) has passed since the last
/// one was due, and the window asks for its next frame at that time
/// (`repaint_after`). The frames still outnumber the moves: egui follows a
/// frame that carries a pointer event with two more at once, which bring no
/// event, so a drag draws about three frames per move where nothing holds
/// the window to the display's refresh, and about one where the buffer swap
/// does (359 frames a second unheld, 116 to 118 held). The first window
/// moved the pointer on every frame and asked for the next one a period
/// ahead, which egui took for an immediate repaint: thousands of frames a
/// second, held by no display.
pub const POINTER_RATE: f64 = 120.0;
/// Frames the window draws before the press: egui hit-tests a press against
/// the frame before, so on the first frame it would land on nothing.
pub const FRAMES_BEFORE_PRESS: u64 = 2;

/// The time between two moves of the drag's pointer.
fn pointer_period() -> Duration {
    Duration::from_secs_f64(1.0 / POINTER_RATE)
}

/// The drag's hand on the Temperature slider.
#[derive(Debug, Default)]
pub struct Drag {
    pressed: Option<Instant>,
    /// When the next move is due: a period after the last one was due, so
    /// the pointer keeps its rate however late a frame comes in.
    due: Option<Instant>,
}

impl Drag {
    /// What the pointer does in the frame being drawn now, once the window
    /// has drawn `drawn` frames: nothing before [`FRAMES_BEFORE_PRESS`], then
    /// a press on the slider's Day end, then a move along the [`sweep`] each
    /// period, the button held. A frame between two moves brings no event.
    pub fn events(&mut self, drawn: u64) -> Vec<egui::Event> {
        self.events_at(drawn, Instant::now())
    }

    /// How long until the next move is due at `now`: what the window waits
    /// before it draws its next frame. A period before the press.
    pub fn next_in(&self, now: Instant) -> Duration {
        self.due
            .map_or(pointer_period(), |due| due.saturating_duration_since(now))
    }

    /// [`Self::events`] with the time of the frame passed in.
    fn events_at(&mut self, drawn: u64, now: Instant) -> Vec<egui::Event> {
        if drawn < FRAMES_BEFORE_PRESS {
            return Vec::new();
        }
        let due = match self.due {
            Some(due) if now < due => return Vec::new(),
            Some(due) => due,
            None => now,
        };
        // The next move falls a period after this one was due. A frame a
        // whole period late (a stall) starts the schedule again from itself:
        // missed moves are not made up.
        let next = due + pointer_period();
        self.due = Some(if next > now {
            next
        } else {
            now + pointer_period()
        });

        let first = self.pressed.is_none();
        let pressed = *self.pressed.get_or_insert(now);
        let pointer = egui::pos2(sweep(now.saturating_duration_since(pressed)), TEMPERATURE_Y);
        let mut events = vec![egui::Event::PointerMoved(pointer)];
        if first {
            events.push(egui::Event::PointerButton {
                pos: pointer,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            });
        }
        events
    }
}

/// Where the drag's pointer is `since` the press: a triangle wave along the
/// track, from its Day end to the far end and back each [`SWEEP`].
pub fn sweep(since: Duration) -> f32 {
    let phase = (since.as_secs_f64() / SWEEP.as_secs_f64()).fract() as f32;
    let out_and_back = 1.0 - (2.0 * phase - 1.0).abs();
    TRACK.0 + (TRACK.1 - TRACK.0) * out_and_back
}

/// One stretch of `rest-drag`'s drag: one sweep, so each ends at the Day end,
/// where the next begins.
pub const STRETCH: Duration = SWEEP;
/// The rest after each stretch, long enough for the Metal painter's link to
/// pause. The let-go ends the preview, and the engine's quick fade draws for
/// 1.2 s more (`engine::QUICK_FADE`), which keeps the burst alive, with the
/// frame that saves the settings inside it. Then 32 refreshes with no frame,
/// about 270 ms at 120 Hz, pause the link, which rests for about 500 ms.
pub const REST: Duration = Duration::from_secs(2);

/// How much of a rest is left `since` the first press: none in a stretch.
fn rest_left(since: Duration) -> Option<Duration> {
    let cycle = (STRETCH + REST).as_nanos();
    let into = since.as_nanos() % cycle;
    (into >= STRETCH.as_nanos()).then(|| Duration::from_nanos((cycle - into) as u64))
}

/// `rest-drag`'s hand: a [`Drag`] with its pacing for each [`STRETCH`], let go
/// for each [`REST`].
#[derive(Debug, Default)]
pub struct RestDrag {
    /// This stretch's drag, new for each, so each presses again.
    drag: Drag,
    /// The first press.
    began: Option<Instant>,
}

impl RestDrag {
    /// What the pointer does in the frame being drawn now: [`Drag::events`] in
    /// a stretch, then a move to the Day end and a release in the rest's first
    /// frame, and nothing for the rest of it.
    pub fn events(&mut self, drawn: u64) -> Vec<egui::Event> {
        self.events_at(drawn, Instant::now())
    }

    /// How long until the next move is due at `now`, or the rest ends.
    pub fn next_in(&self, now: Instant) -> Duration {
        self.rest_left(now)
            .unwrap_or_else(|| self.drag.next_in(now))
    }

    fn rest_left(&self, now: Instant) -> Option<Duration> {
        rest_left(now.saturating_duration_since(self.began?))
    }

    /// [`Self::events`] with the time of the frame passed in.
    fn events_at(&mut self, drawn: u64, now: Instant) -> Vec<egui::Event> {
        if self.rest_left(now).is_none() {
            let events = self.drag.events_at(drawn, now);
            if self.began.is_none() && !events.is_empty() {
                self.began = Some(now);
            }
            return events;
        }
        if self.drag.pressed.is_none() {
            return Vec::new();
        }
        self.drag = Drag::default();
        let pointer = egui::pos2(TRACK.0, TEMPERATURE_Y);
        vec![
            egui::Event::PointerMoved(pointer),
            egui::Event::PointerButton {
                pos: pointer,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }
}

/// When the driver's clock started and the minute it started at. One per
/// process, set by [`app`].
static CLOCK: OnceLock<(Instant, f64)> = OnceLock::new();

/// The driver's clock, in minutes since midnight: the scenario's moment
/// when the driver's app was made, plus the real time since.
///
/// # Panics
///
/// Before [`app`] has started it. The clock once started at midnight then,
/// and kept that moment whatever scenario came next.
pub fn clock() -> f64 {
    let (start, minute) = *CLOCK
        .get()
        .expect("drive::clock() called before drive::app() started the clock");
    (minute + start.elapsed().as_secs_f64() / 60.0).rem_euclid(24.0 * 60.0)
}

/// Starts the clock at `minute`.
///
/// # Panics
///
/// When it already runs from another minute: it cannot restart, and a
/// second app would count from the first one's moment. The same minute again
/// keeps the clock running from its first start.
fn start_clock(minute: f64) {
    let (_, started_at) = CLOCK.get_or_init(|| (Instant::now(), minute));
    assert_eq!(
        *started_at, minute,
        "drive::app() called again with another moment: the clock runs from minute \
         {started_at}, not {minute}"
    );
}

/// The screen the driver's engine paints: never the displays.
pub fn screen() -> NullScreen {
    NullScreen
}

/// The driver's app: its engine on [`screen`] and the driver's clock, which
/// starts at `scenario`'s moment. No settings file.
///
/// # Panics
///
/// When a driver app was made before for a scenario with another moment:
/// see [`start_clock`].
pub fn app(
    scenario: Scenario,
    settings: Settings,
    tray: Option<fastframe_tray::Tray>,
    signals: Signals,
    wake: impl Fn(Reach) + Send + 'static,
) -> App {
    start_clock(scenario.moment());
    let engine = Engine::spawn(Box::new(screen()), settings.clone(), clock, wake);
    // The window's haptics. The driver watches no press, so a detent its
    // pointer crosses plays the public pattern, felt only under a finger on
    // a Force Touch trackpad (`crate::haptics`, "Who hears it").
    App::new(
        settings,
        None,
        engine,
        tray,
        signals,
        clock,
        Haptics::TRACKPAD,
    )
}

/// One window, from the request that opened it to its close.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Open {
    /// From the request (the start of `main` for a window open from the
    /// start) to the start of the window's second frame, when the first
    /// has been drawn and shown.
    pub to_first_frame_ms: Option<f64>,
    /// The first frame's CPU: its layout, tessellation and paint.
    pub first_frame_cpu_ms: Option<f64>,
    pub frames: u64,
    /// From the first frame to the close.
    pub open_ms: Option<f64>,
    /// The process's memory footprint 2 s after this window closed.
    pub footprint_after_close: Option<u64>,
    /// The process's threads at that moment: a close leaves no hold's thread,
    /// so this reads as it does for a glow window.
    pub threads_after_close: Option<u32>,
    /// The painter this window had: `metal` or `glow`, what eframe gave the
    /// app and not what was asked for.
    pub renderer: &'static str,
    /// Where a Metal window's shader came from: `file`, `shader.metallib`, or `source`, compiled at
    /// run time once the file failed in this process. Null on glow.
    pub shader: Option<&'static str>,
    /// Whether the app held its menu-bar item at this window's first frame.
    pub tray: bool,
}

/// A Metal window whose painter failed, and which a glow window replaced. It
/// counts in no cycle: [`Report::opens`] holds the window that replaced it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Fallback {
    pub why: String,
    /// The frames it began before it closed.
    pub frames: u64,
    /// The index in [`Report::opens`] of the window that replaced it.
    pub at_open: usize,
    /// The engine's checks of the displays so far, as it closed: what
    /// [`Report::screen`] counts above this came after the fallback.
    pub screen_checks: u64,
}

/// The spread of the frames' CPU, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Spread {
    pub samples: usize,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
}

impl Spread {
    /// Nearest-rank percentiles of `samples`; none without samples.
    pub fn of(samples: &[f64]) -> Option<Self> {
        let mut sorted: Vec<f64> = samples.iter().copied().filter(|s| s.is_finite()).collect();
        sorted.sort_by(f64::total_cmp);
        let at =
            |q: f64| sorted[((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1];
        (!sorted.is_empty()).then(|| Self {
            samples: sorted.len(),
            p50: at(0.50),
            p90: at(0.90),
            p99: at(0.99),
            max: at(1.0),
        })
    }
}

/// The `elapsed_s` in which a drive is in its time. The scenarios end in three ways, and each gets
/// its bound ([`Scenario::bound`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bound {
    /// `from` to `to` seconds, both held: a timed scenario, or a cycled one.
    Between { from: f64, to: f64 },
    /// Under `to` seconds: the start, which has no bound of time, as its time is what it
    /// measures. `to` is its deadline.
    Under(f64),
}

impl Bound {
    /// Whether a drive that took `elapsed_s` is in its time. A time that is no number is not.
    pub fn holds(self, elapsed_s: f64) -> bool {
        match self {
            Self::Between { from, to } => from <= elapsed_s && elapsed_s <= to,
            Self::Under(to) => elapsed_s < to,
        }
    }

    /// The bound as the summary writes it, in seconds: `<from>..<to>`, or `..<to>`.
    fn text(self) -> String {
        match self {
            Self::Between { from, to } => format!("{from:.3}..{to:.3}"),
            Self::Under(to) => format!("..{to:.3}"),
        }
    }
}

/// What the window did not do in the cycle of `open-close` that broke off, in the [`PATIENCE`]
/// that `direct` gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stuck {
    /// The window did not open.
    Open,
    /// The window did not close.
    Close,
}

/// A cycled run in which `direct` gave up on a cycle, as a window did not open or did not close.
/// The run holds fewer windows than it was asked for, so its figures do not compare, whatever its
/// time: the [`PATIENCE`] that `direct` waited before it gave up can leave the run inside its
/// bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrokeOff {
    /// The cycles that ran whole before the one that broke off, which is the next one.
    pub done: usize,
    /// The cycles the run was to make ([`Scenario::cycles`]).
    pub planned: usize,
    /// What the window did not do in the cycle that broke off.
    pub stuck: Stuck,
}

/// Why a drive's per-frame figures do not compare with another's: the machine slowed or stopped
/// it, so its `elapsed_s` is outside its bound (on 2026-10-24 the display slept and a 20 s drive
/// took 17 to 43 minutes), or a cycle of it broke off, so it holds fewer windows than asked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spoiled {
    /// The drive's `elapsed_s` is outside its `bound`.
    Time { elapsed_s: f64, bound: Bound },
    /// A cycle broke off, whatever the time.
    Cycles(BrokeOff),
}

impl Spoiled {
    /// For the time alone: none for a drive of `scenario` and `seconds` that took `elapsed_s` and
    /// is in its time. A cycle that broke off is [`Spoiled::of_run`]'s to see.
    pub fn of(scenario: Scenario, seconds: u64, elapsed_s: f64) -> Option<Self> {
        let bound = scenario.bound(seconds);
        (!bound.holds(elapsed_s)).then_some(Self::Time { elapsed_s, bound })
    }

    /// For a whole run: the cycle that broke off, if one did, whatever the time; else the time's
    /// verdict ([`Spoiled::of`]).
    pub fn of_run(
        broke_off: Option<BrokeOff>,
        scenario: Scenario,
        seconds: u64,
        elapsed_s: f64,
    ) -> Option<Self> {
        broke_off
            .map(Self::Cycles)
            .or_else(|| Self::of(scenario, seconds, elapsed_s))
    }

    /// The summary's value, with no space in it: `elapsed_s:<x>,bound_s:<from>..<to>` for the
    /// time, and `cycles:<done>/<planned>` for a cycle that broke off.
    fn pair(self) -> String {
        match self {
            Self::Time { elapsed_s, bound } => {
                format!("elapsed_s:{elapsed_s:.3},bound_s:{}", bound.text())
            }
            Self::Cycles(BrokeOff { done, planned, .. }) => format!("cycles:{done}/{planned}"),
        }
    }
}

/// The reason, as the JSON report holds it.
impl std::fmt::Display for Spoiled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Time {
                elapsed_s,
                bound: Bound::Between { from, to },
            } => write!(
                f,
                "elapsed_s {elapsed_s:.3} s is outside {from:.3} to {to:.3} s, the time this drive \
                 takes: it was slowed or stopped, so its per-frame figures do not compare"
            ),
            Self::Time {
                elapsed_s,
                bound: Bound::Under(to),
            } => write!(
                f,
                "elapsed_s {elapsed_s:.3} s reached the {to:.3} s deadline, which quit the start: \
                 its second frame never came, so its per-frame figures do not compare"
            ),
            Self::Cycles(BrokeOff {
                done,
                planned,
                stuck,
            }) => write!(
                f,
                "cycle {} broke off, as the window did not {}: {done} of {planned} cycles were \
                 done, so this drive holds fewer windows than asked and its figures do not compare",
                done + 1,
                match stuck {
                    Stuck::Open => "open",
                    Stuck::Close => "close",
                }
            ),
        }
    }
}

impl serde::Serialize for Spoiled {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// What a run measured.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Report {
    pub scenario: Scenario,
    pub seconds: u64,
    /// Whether the app quit by itself; false when the driver had to end it.
    pub finished: bool,
    /// How long the run took, rounded to the millisecond: the one value that `spoiled`, its
    /// reason and the summary read.
    pub elapsed_s: f64,
    /// Why this drive's figures do not compare, when a cycle of it broke off or `elapsed_s` is
    /// outside the bound of its scenario and `seconds`: none for a drive that held, and the
    /// reason as a sentence in the JSON. A broken cycle comes before the time. The driver's exit
    /// is 0 either way (the owner's pick, 2026-11-12): the scripts read this field and leave the
    /// run out.
    pub spoiled: Option<Spoiled>,
    pub pixels_per_point: Option<f32>,
    pub frames: u64,
    /// The time the window was open, summed over its opens.
    pub window_open_s: f64,
    /// Frames over the time the window was open.
    pub frames_per_second: Option<f64>,
    /// Of `frames`, those that began with the window covered or minimized:
    /// eframe skips their paint, so they cost almost nothing and never reach
    /// the Metal painter's hold. A run with any is spoiled. A closed window's
    /// ticks are `headless_frames`, which this does not count.
    pub unseen_frames: u64,
    /// The CPU of each frame but the last of each window, which eframe
    /// reports on the next frame.
    pub frame_cpu_ms: Option<Spread>,
    /// Of `frame_cpu_ms`, the CPU of the frames whose input held a
    /// `PointerMoved` that the driver put there (`drag` and `rest-drag`): the
    /// slower frames of a drag, which the repaints that bring no event dilute
    /// in `frame_cpu_ms`. None where the driver moves nothing.
    ///
    /// The press frame, and `rest-drag`'s release frames, hold a `PointerMoved`
    /// beside the button's event, so they count as move frames: the max, and
    /// sometimes the p99, can read one of them and not a drag's move.
    ///
    /// A run with `unseen_frames` above 0 is spoiled for this figure too, as
    /// for the others. A frame that eframe gave only its logic is not painted
    /// and reports no CPU: `frame_cpu_ms` counts the last painted frame's CPU
    /// again, this figure does not (`Record::count_frame`), but the moves of
    /// a run of such frames reach it as the one painted frame after them.
    pub move_frame_cpu_ms: Option<Spread>,
    pub cover: CoverCounts,
    pub cover_builds_per_frame: Option<f64>,
    pub cover_bytes_per_frame: Option<f64>,
    /// What the engine asked of `NullScreen`: its checks, its paints and its
    /// restores.
    pub screen: app::screen_counts::ScreenCounts,
    /// The ticks the app ran with its window closed, each the end of one
    /// headless wait.
    pub headless_frames: u64,
    /// From `main` to the first window's first frame, drawn and shown; none
    /// when the app starts in the menu bar, or when that window's painter
    /// failed: `opens[0]` is then glow's, timed from the failure.
    pub main_to_first_frame_ms: Option<f64>,
    pub opens: Vec<Open>,
    /// The windows that closed because their painter failed.
    pub fallbacks: Vec<Fallback>,
    pub footprint_after_first_close: Option<u64>,
    pub footprint_after_last_close: Option<u64>,
    pub footprint_at_end: Option<u64>,
    /// The Metal painter's hold, over every window of the run.
    pub hold: HoldCounts,
}

/// What the Metal painter's hold counted: all zeros and no make time where no
/// window ran on it, as on glow or on another platform.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct HoldCounts {
    /// The refreshes its links told of.
    pub ticks: u64,
    /// Bursts that started a link, and frames that started it again after a
    /// timeout.
    pub starts: u32,
    /// Frames that waited for a refresh.
    pub waits: u32,
    /// Waits that reached the limit, 50 ms, with no refresh.
    pub timeouts: u32,
    /// The longest a link took to make, with its thread started, on the main
    /// thread.
    pub link_make_ms: Option<f64>,
    /// Pauses whose block found a spare surface on the main thread, and
    /// dropped it: what proves the block arrives.
    pub spare_drops: u32,
}

#[cfg(target_os = "macos")]
impl From<eframe::egui_wgpu::hold::Stats> for HoldCounts {
    fn from(stats: eframe::egui_wgpu::hold::Stats) -> Self {
        Self {
            ticks: stats.ticks,
            starts: stats.starts,
            waits: stats.waits,
            timeouts: stats.timeouts,
            link_make_ms: stats.longest_make.map(|make| make.as_secs_f64() * 1e3),
            spare_drops: stats.spare_drops,
        }
    }
}

impl HoldCounts {
    #[cfg(target_os = "macos")]
    fn read() -> Self {
        eframe::egui_wgpu::hold::stats().into()
    }

    #[cfg(not(target_os = "macos"))]
    fn read() -> Self {
        Self::default()
    }
}

impl Report {
    /// One line of `key=value` pairs, for `scripts/measure.sh`.
    pub fn summary(&self) -> String {
        let number = |value: Option<f64>| value.map_or("-".to_owned(), |v| format!("{v:.3}"));
        let cpu = self.frame_cpu_ms;
        let move_cpu = self.move_frame_cpu_ms;
        [
            format!("scenario={}", self.scenario.name()),
            format!("finished={}", self.finished),
            format!(
                "spoiled={}",
                self.spoiled.map_or("-".to_owned(), Spoiled::pair)
            ),
            format!("frames={}", self.frames),
            format!("fps={}", number(self.frames_per_second)),
            format!("cpu_p50_ms={}", number(cpu.map(|c| c.p50))),
            format!("cpu_p90_ms={}", number(cpu.map(|c| c.p90))),
            format!("cpu_p99_ms={}", number(cpu.map(|c| c.p99))),
            format!("cpu_max_ms={}", number(cpu.map(|c| c.max))),
            format!("move_cpu_p50_ms={}", number(move_cpu.map(|c| c.p50))),
            format!("move_cpu_p90_ms={}", number(move_cpu.map(|c| c.p90))),
            format!("move_cpu_p99_ms={}", number(move_cpu.map(|c| c.p99))),
            format!("move_cpu_max_ms={}", number(move_cpu.map(|c| c.max))),
            format!("cover_builds={}", self.cover.layer_builds),
            format!(
                "cover_bytes_per_frame={}",
                number(self.cover_bytes_per_frame)
            ),
            format!("checks={}", self.screen.checks),
            format!("paints={}", self.screen.paints),
            format!("headless={}", self.headless_frames),
            format!("unseen={}", self.unseen_frames),
            format!(
                "main_to_first_frame_ms={}",
                number(self.main_to_first_frame_ms)
            ),
            format!(
                "first_open_ms={}",
                number(self.opens.first().and_then(|open| open.to_first_frame_ms))
            ),
            format!(
                "footprint_first_close={}",
                self.footprint_after_first_close
                    .map_or("-".to_owned(), |f| f.to_string())
            ),
            format!(
                "footprint_last_close={}",
                self.footprint_after_last_close
                    .map_or("-".to_owned(), |f| f.to_string())
            ),
        ]
        .join(" ")
    }
}

impl Scenario {
    fn name(self) -> String {
        clap::ValueEnum::to_possible_value(&self)
            .map_or_else(|| format!("{self:?}"), |value| value.get_name().to_owned())
    }
}

/// What the window and the driver's thread note as the run goes.
#[derive(Default)]
struct Record {
    frames: u64,
    /// The frames that began with the window covered or minimized.
    unseen_frames: u64,
    cpu_ms: Vec<f64>,
    /// Of `cpu_ms`, the CPU of the frames that carried a move.
    move_cpu_ms: Vec<f64>,
    /// What the frame before left for this one to count.
    left: Left,
    pixels_per_point: Option<f32>,
    opens: Vec<Open>,
    /// When the next window was asked for.
    requested: Option<Instant>,
    /// When the open window drew its first frame.
    opened: Option<Instant>,
    closes: usize,
    window_open: Duration,
    finished: bool,
    fallbacks: Vec<Fallback>,
    /// Set while the close under way reopens the window on glow: the failure
    /// behind it.
    reopening: Option<Reopening>,
    /// The cycle that `direct` gave up on, if one.
    broke_off: Option<BrokeOff>,
}

/// The failure that a window's close is answering.
struct Reopening {
    why: String,
    at: Instant,
}

/// What a frame's logic knows as the frame begins, for [`Record::count_frame`]. The fields are
/// named so that the two flags cannot swap by position.
#[derive(Clone, Copy, Debug, Default)]
struct Begun {
    /// The CPU eframe reports as this frame begins: the one of the last frame it painted, which
    /// is the frame before only if that one was painted. None on a window's first frame.
    cpu_ms: Option<f64>,
    /// Whether this frame began with the window covered or minimized.
    unseen: bool,
    /// Whether the driver put a `PointerMoved` in this frame's own input.
    moved: bool,
}

/// What a frame leaves for the next one to count.
#[derive(Debug, Default)]
struct Left {
    /// The frame was painted and carried a move: the CPU eframe reports as the next frame begins
    /// is a move's.
    move_cpu_due: bool,
    /// The frame was unseen and carried a move that its input saved for the next painted frame:
    /// eframe keeps the input of a frame it gives only its logic.
    move_waiting: bool,
}

impl Record {
    /// Counts a frame as it begins.
    ///
    /// The rule of [`Report::move_frame_cpu_ms`], from eframe 0.36.2. eframe reports a frame's CPU
    /// once it has painted the frame (`report_frame_time`, `glow_integration.rs:867` and
    /// `wgpu_integration.rs:873`), so the CPU of a frame that carried a move arrives in the frame
    /// after it. A frame it gives only its logic, as `unseen` (`update_logic_only`, with no pass
    /// and no report), leaves the value of the last painted frame in place: the next frame reads
    /// that value again. So the reading that a frame begins with is new if and only if the frame
    /// before was painted, whether or not this frame is, and then it is that frame's CPU. A CPU
    /// goes into the move spread when:
    ///
    /// - the frame before was painted, and
    /// - it carried a move: the driver put a `PointerMoved` in its own input, or an unseen frame
    ///   before it did and the input waited for it (`update_logic_only` saves the input,
    ///   `epi_integration.rs:338`, and `prepare_raw_input` hands it to the next frame, `:357`,
    ///   before the hook adds that frame's own events), and
    /// - the reading is there.
    ///
    /// So a covered drag adds no sample while the window stays covered, and the move frame just
    /// before the cover and the first painted one after it each add theirs.
    ///
    /// `cpu_ms` keeps every reading, a stale one too, as it always did.
    ///
    /// The flags cross a window's end: the first frame of the next window reads no CPU, and the
    /// scenarios that move (`drag` and `rest-drag`) keep one window.
    fn count_frame(&mut self, begun: Begun) {
        self.frames += 1;
        self.unseen_frames += u64::from(begun.unseen);
        self.cpu_ms.extend(begun.cpu_ms);
        if self.left.move_cpu_due {
            self.move_cpu_ms.extend(begun.cpu_ms);
        }
        let carried = begun.moved || self.left.move_waiting;
        self.left = Left {
            move_cpu_due: carried && !begun.unseen,
            move_waiting: carried && begun.unseen,
        };
    }
}

/// Whether eframe skips the paint of a frame with this viewport: the window
/// is covered by another or minimized. A window whose state eframe has not
/// told yet, as in the first frames, counts as seen.
fn is_unseen(viewport: &egui::ViewportInfo) -> bool {
    viewport.visible() == Some(false)
}

/// Whether the events that the driver puts in a frame's input hold a
/// `PointerMoved`: the frames that [`Report::move_frame_cpu_ms`] separates. The
/// window's own events are not the driver's and do not count, and the
/// scenarios that move nothing put in none.
fn holds_a_move(events: &[egui::Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, egui::Event::PointerMoved(_)))
}

/// Whether the frame that `ctx` is in began with the window unseen. A covered
/// window gets only its logic from eframe, with no pass, and egui tells that
/// logic the viewport all the same.
fn window_unseen(ctx: &egui::Context) -> bool {
    ctx.input(|input| is_unseen(input.viewport()))
}

struct Shared {
    record: Mutex<Record>,
    changed: Condvar,
    main: Instant,
    scenario: Scenario,
    seconds: u64,
    /// Whether the report has been written. Held while it is, so a second
    /// caller of [`finish`] waits for the first to be done and then writes
    /// nothing.
    reported: Mutex<bool>,
}

impl Shared {
    fn new(main: Instant, scenario: Scenario, seconds: u64) -> Self {
        Self {
            record: Mutex::new(Record::default()),
            changed: Condvar::new(),
            main,
            scenario,
            seconds,
            reported: Mutex::new(false),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Record> {
        self.record.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits until `done` holds, at most [`PATIENCE`]; returns whether it
    /// does.
    fn wait_for(&self, done: impl Fn(&Record) -> bool) -> bool {
        let record = self.lock();
        let (record, _) = self
            .changed
            .wait_timeout_while(record, PATIENCE, |record| !done(record))
            .unwrap_or_else(PoisonError::into_inner);
        done(&record)
    }

    /// A window's `run_native` returned. A reopen on glow, which `direct` did
    /// not ask for, must move neither of what it counts its cycles by: the
    /// failed window goes out of `opens`, `closes` stays, and the next window
    /// reads its `to_first_frame_ms` from the failure.
    fn window_closed(&self) {
        let mut record = self.lock();
        if let Some(opened) = record.opened.take() {
            let open = opened.elapsed();
            record.window_open += open;
            if let Some(last) = record.opens.last_mut() {
                last.open_ms = Some(open.as_secs_f64() * 1e3);
            }
        }
        if let Some(Reopening { why, at }) = record.reopening.take() {
            let frames = record.opens.pop().map_or(0, |open| open.frames);
            let at_open = record.opens.len();
            record.fallbacks.push(Fallback {
                why,
                frames,
                at_open,
                screen_checks: app::screen_counts::read().checks,
            });
            record.requested = Some(at);
        } else {
            record.closes += 1;
        }
        drop(record);
        self.changed.notify_all();
    }

    /// `direct` gave up on the cycle after `done` whole ones: the window did not do `stuck` in
    /// [`PATIENCE`]. The report is spoiled by it ([`Spoiled::of_run`]).
    fn break_off(&self, done: usize, stuck: Stuck) {
        self.lock().broke_off = Some(BrokeOff {
            done,
            planned: Scenario::cycles(self.seconds),
            stuck,
        });
    }

    fn report(&self, finished: bool) -> Report {
        self.report_after(finished, self.main.elapsed())
    }

    /// The report of a run that has taken `elapsed`.
    fn report_after(&self, finished: bool, elapsed: Duration) -> Report {
        let mut record = self.lock();
        record.finished |= finished;
        // A window still open at the end counts up to now.
        let window_open =
            record.window_open + record.opened.map_or(Duration::ZERO, |at| at.elapsed());
        let cover = cover_counts::read();
        let per_frame =
            |count: u64| (record.frames > 0).then(|| count as f64 / record.frames as f64);
        let closes: Vec<u64> = record
            .opens
            .iter()
            .filter_map(|open| open.footprint_after_close)
            .collect();
        // Rounded to the millisecond once, here: the JSON, the summary, the bound and the reason all
        // read this value, which the reason and the pair write with three decimals. Taken whole, a
        // drive at 32.0004 s would be spoiled with the words "32.000 s is outside 30.000 to 32.000 s".
        let elapsed_s = (elapsed.as_secs_f64() * 1e3).round() / 1e3;
        Report {
            scenario: self.scenario,
            seconds: self.seconds,
            finished: record.finished,
            elapsed_s,
            spoiled: Spoiled::of_run(record.broke_off, self.scenario, self.seconds, elapsed_s),
            pixels_per_point: record.pixels_per_point,
            frames: record.frames,
            window_open_s: window_open.as_secs_f64(),
            frames_per_second: (!window_open.is_zero())
                .then(|| record.frames as f64 / window_open.as_secs_f64()),
            unseen_frames: record.unseen_frames,
            frame_cpu_ms: Spread::of(&record.cpu_ms),
            move_frame_cpu_ms: Spread::of(&record.move_cpu_ms),
            cover,
            cover_builds_per_frame: per_frame(cover.layer_builds),
            cover_bytes_per_frame: per_frame(cover.uploaded_bytes),
            screen: app::screen_counts::read(),
            headless_frames: app::headless_counts::read(),
            main_to_first_frame_ms: if self.scenario.starts_hidden()
                || record
                    .fallbacks
                    .iter()
                    .any(|fallback| fallback.at_open == 0)
            {
                None
            } else {
                record.opens.first().and_then(|open| open.to_first_frame_ms)
            },
            opens: record.opens.clone(),
            fallbacks: record.fallbacks.clone(),
            footprint_after_first_close: closes.first().copied(),
            footprint_after_last_close: closes.last().copied(),
            footprint_at_end: footprint(),
            hold: HoldCounts::read(),
        }
    }
}

/// Runs `scenario` for `seconds`, `main` being when the process started,
/// and writes the report to `report` when one is named. The summary goes to
/// stdout.
pub fn run(
    main: Instant,
    scenario: Scenario,
    seconds: u64,
    report: Option<PathBuf>,
) -> eframe::Result {
    let _ = fastframe_log::Logging::new("arrakis", env!("CARGO_PKG_VERSION"))
        .filter("warn,arrakis=info")
        .init();
    let waker = Waker::default();
    let signals = Signals::default();
    // The lifecycle that the app has too, wired once in `entrypoint::wire`,
    // around the driver's app.
    let shell = entrypoint::wire(
        &waker,
        signals.clone(),
        scenario.starts_hidden(),
        |tray, signals, wake| self::app(scenario, Settings::default(), tray, signals, wake),
    );

    let shared = Arc::new(Shared::new(main, scenario, seconds));
    {
        let (shared, signals, waker, report) = (
            shared.clone(),
            signals.clone(),
            waker.clone(),
            report.clone(),
        );
        std::thread::Builder::new()
            .name("arrakis-drive".to_owned())
            .spawn(move || direct(&shared, &signals, &waker, report))
            .expect("unable to start the driver's thread");
    }

    let outcome = shell.run(|lease| {
        let window_shared = shared.clone();
        let injected = signals.injected.clone();
        let opened = eframe::run_native(
            "Arrakis",
            entrypoint::native_options(),
            Box::new(move |cc| {
                theme::install(&cc.egui_ctx);
                let mut app = lease.take(&cc.egui_ctx);
                let painter = WindowPainter::of(cc);
                app.attach(painter);
                Ok(Box::new(Driven {
                    window: ui::Window::new(app),
                    shared: window_shared,
                    injected,
                    frame: 0,
                    requested: None,
                    drag: Drag::default(),
                    rest_drag: RestDrag::default(),
                    moved: false,
                    painter,
                    reopening: false,
                    rest_frames: painter == WindowPainter::Metal && painter::fails_at_rest(),
                    last_pass: None,
                }))
            }),
        );
        shared.window_closed();
        opened
    });
    finish(&shared, true, report.as_ref());
    outcome
}

/// Writes the report and prints its summary, unless another caller has: the
/// main thread when the app quit, and the driver's thread when it did not
/// quit in time, may both come here. The first is the only writer, and the
/// second waits until it is done, so the file is whole when `finish` returns.
/// Returns whether this call wrote.
fn finish(shared: &Shared, finished: bool, path: Option<&PathBuf>) -> bool {
    let mut reported = shared
        .reported
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if *reported {
        return false;
    }
    *reported = true;
    let report = shared.report(finished);
    println!("{}", report.summary());
    if let Some(path) = path {
        let written = serde_json::to_string_pretty(&report)
            .map_err(|error| error.to_string())
            .and_then(|json| std::fs::write(path, json + "\n").map_err(|error| error.to_string()));
        if let Err(error) = written {
            eprintln!("demo-drive: {}: {error}", path.display());
        }
    }
    true
}

/// The driver's thread: opens and closes the window for `open-close`, asks
/// the app to quit at the end, and ends it if it does not.
fn direct(shared: &Shared, signals: &Signals, waker: &Waker, report: Option<PathBuf>) {
    let send = |action: Action| {
        signals
            .injected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(action);
        idle::wake(waker);
    };
    let seconds = Duration::from_secs(shared.seconds);
    if matches!(
        shared.scenario,
        Scenario::OpenClose | Scenario::OpenCloseBurst
    ) {
        for cycle in 0..Scenario::cycles(shared.seconds) {
            // As a click on the menu-bar item or a second launch asks.
            shared.lock().requested = Some(Instant::now());
            signals.show_window.store(true, Ordering::Relaxed);
            idle::wake(waker);
            let shown = shared.wait_for(|record| {
                record
                    .opens
                    .get(cycle)
                    .is_some_and(|open| open.to_first_frame_ms.is_some())
            });
            if !shown {
                log::warn!("demo-drive: the window did not open in cycle {cycle}");
                shared.break_off(cycle, Stuck::Open);
                break;
            }
            std::thread::sleep(IDLE_OPEN);
            send(Action::HideWindow);
            if !shared.wait_for(|record| record.closes > cycle) {
                log::warn!("demo-drive: the window did not close in cycle {cycle}");
                shared.break_off(cycle, Stuck::Close);
                break;
            }
            std::thread::sleep(IDLE_CLOSED);
            if let Some(open) = shared.lock().opens.get_mut(cycle) {
                open.footprint_after_close = footprint();
                open.threads_after_close = threads();
            }
        }
    } else {
        // The start quits by itself after its first frame; this is only its
        // deadline.
        std::thread::sleep(seconds.saturating_sub(shared.main.elapsed()));
    }
    send(Action::Quit);
    std::thread::sleep(GRACE);
    if shared.lock().finished {
        // The main thread is writing the report and returning.
        return;
    }
    log::error!("demo-drive: the app did not quit; ending it");
    // The main thread may have got there since the check above; then it has
    // written the report, and the process ends as it returns.
    if finish(shared, false, report.as_ref()) {
        std::process::exit(1);
    }
}

/// The window, with the driver's hands on it.
struct Driven {
    window: ui::Window,
    shared: Arc<Shared>,
    injected: Arc<Mutex<Vec<Action>>>,
    /// Frames this window has begun.
    frame: u64,
    /// When this window was asked for.
    requested: Option<Instant>,
    drag: Drag,
    rest_drag: RestDrag,
    /// Whether the driver put a `PointerMoved` in the input of the frame
    /// being begun: `raw_input_hook` notes it, and `note_frame`, in the same
    /// frame, hands it to the record.
    moved: bool,
    /// The painter this window has.
    painter: WindowPainter,
    /// What the record last heard of `App::reopening_on_glow`.
    reopening: bool,
    /// Whether this window fails a frame from rest, so that it must leave
    /// itself at rest: `ARRAKIS_METAL_FAIL=rest`, on Metal.
    rest_frames: bool,
    /// When the last pass came, for those frames.
    last_pass: Option<Instant>,
}

impl Driven {
    /// Notes a frame as it begins: its number, the CPU eframe reports, whether
    /// the driver moved the pointer in this frame, whether the window is seen
    /// (the record joins them, see [`Record::count_frame`]), and the first
    /// frame's arrival. A Metal window whose
    /// flag is up has shown nothing, and writes no arrival, so that `direct`
    /// never takes it for a window shown.
    fn note_frame(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        self.frame += 1;
        let cpu = frame
            .info()
            .cpu_usage
            .map(|seconds| f64::from(seconds) * 1e3);
        let unseen = window_unseen(ctx);
        let second = self.frame == 2;
        let shown = second && !(self.painter == WindowPainter::Metal && painter::failed());
        let mut record = self.shared.lock();
        record.count_frame(Begun {
            cpu_ms: cpu,
            unseen,
            moved: self.moved,
        });
        if self.frame == 1 {
            // A window open from the start was asked for by `main`.
            self.requested = Some(record.requested.take().unwrap_or(self.shared.main));
            record.opened = Some(Instant::now());
            record.pixels_per_point = Some(ctx.pixels_per_point());
            record.opens.push(Open {
                renderer: self.painter.name(),
                // The window's painter started before its first frame.
                shader: painter::shader_from(self.painter),
                tray: self.window.app().has_tray(),
                ..Open::default()
            });
        }
        if let Some(open) = record.opens.last_mut() {
            open.frames += 1;
            if second {
                open.first_frame_cpu_ms = cpu;
            }
            if shown {
                open.to_first_frame_ms = self.requested.map(|at| at.elapsed().as_secs_f64() * 1e3);
            }
        }
        drop(record);
        if second {
            self.shared.changed.notify_all();
        }
    }

    /// Tells the record whether the close under way reopens on glow, the
    /// pass it changes: `Shared::window_closed` goes by this and not by the
    /// flag, as a close the person asked for reopens nothing.
    fn note_reopening(&mut self) {
        let reopening = self.window.app().reopening_on_glow();
        if reopening != self.reopening {
            self.reopening = reopening;
            let failure = reopening.then(painter::failure).flatten();
            self.shared.lock().reopening = failure.map(|(why, at)| Reopening { why, at });
        }
    }

    /// Asks for one frame [`FRAME_FROM_REST`] on in each pass that comes less
    /// than [`painter::AT_REST`] after the one before, and in no other: the frame
    /// from rest, which the painter fails, asks for nothing, so only the redraw of
    /// its failure brings the frame that reads it. With a delay `egui` gives
    /// one pass, where `request_repaint` gives two, and one ask on the first
    /// pass would not do, as a pass with a repaint outstanding loses it.
    fn ask_for_the_frame_from_rest(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let soon = self
            .last_pass
            .replace(now)
            .is_none_or(|last| now.duration_since(last) < painter::AT_REST);
        if soon {
            ctx.request_repaint_after(FRAME_FROM_REST);
        }
    }
}

impl eframe::App for Driven {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.window.raw_input_hook(ctx, raw_input);
        // Every run sees a focused window, whatever app has the keyboard.
        for viewport in raw_input.viewports.values_mut() {
            viewport.focused = Some(true);
        }
        let events = match self.shared.scenario {
            Scenario::Drag => self.drag.events(self.frame),
            Scenario::RestDrag => self.rest_drag.events(self.frame),
            _ => Vec::new(),
        };
        self.moved = holds_a_move(&events);
        raw_input.events.extend(events);
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.note_frame(ctx, frame);
        if self.shared.scenario == Scenario::Drag {
            // The next frame comes when the pointer's next move is due.
            repaint_after(ctx, self.drag.next_in(Instant::now()));
        } else if self.shared.scenario == Scenario::RestDrag {
            // Or when the rest ends.
            repaint_after(ctx, self.rest_drag.next_in(Instant::now()));
        } else if self.frame == 1 || self.shared.scenario == Scenario::OpenCloseBurst {
            // The second frame tells the first one's CPU.
            ctx.request_repaint();
        }
        if self.frame == 2 && self.shared.scenario == Scenario::Start {
            self.injected
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Action::Quit);
        }
        self.window.logic(ctx, frame);
        self.note_reopening();
        if self.rest_frames {
            self.ask_for_the_frame_from_rest(ctx);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.window.ui(ui, frame);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.window.clear_color(visuals)
    }

    fn persist_egui_memory(&self) -> bool {
        self.window.persist_egui_memory()
    }
}

/// The process's memory footprint, as Activity Monitor's Memory column and
/// `footprint` count it.
#[cfg(target_os = "macos")]
pub fn footprint() -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: `info` is a writable `rusage_info_v4`, the struct that the
    // RUSAGE_INFO_V4 flavor fills, and it outlives the call.
    let status = unsafe {
        libc::proc_pid_rusage(
            std::process::id() as libc::c_int,
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast(),
        )
    };
    // SAFETY: a zero status means the call filled `info`; it was zeroed
    // before, so every field is initialized either way.
    (status == 0).then(|| unsafe { info.assume_init() }.ri_phys_footprint)
}

#[cfg(not(target_os = "macos"))]
pub fn footprint() -> Option<u64> {
    None
}

/// The process's threads now.
#[cfg(target_os = "macos")]
pub fn threads() -> Option<u32> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: `info` is a writable `proc_taskinfo` of `size` bytes, the struct
    // that the PROC_PIDTASKINFO flavor fills, and it outlives the call.
    let filled = unsafe {
        libc::proc_pidinfo(
            std::process::id() as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    // SAFETY: it was zeroed before, so every field is initialized either way.
    (filled == size).then(|| unsafe { info.assume_init() }.pti_threadnum as u32)
}

#[cfg(not(target_os = "macos"))]
pub fn threads() -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use clap::ValueEnum;

    use super::*;
    use crate::schedule::Phase;

    /// The flag's spelling of `scenario`.
    fn flag(scenario: Scenario) -> String {
        scenario
            .to_possible_value()
            .expect("every scenario has a name")
            .get_name()
            .to_owned()
    }

    // Scenario

    #[test]
    fn each_scenarios_moment_suits_what_it_measures() {
        let schedule = Settings::default().schedule();
        // A run lasts a minute or so; its moment must hold for that long.
        for scenario in Scenario::value_variants() {
            let (now, later) = (scenario.moment(), scenario.moment() + 1.0);
            let name = flag(*scenario);
            match scenario {
                Scenario::ClosedNight | Scenario::OpenNight => {
                    for minute in [now, later] {
                        assert_eq!(schedule.level(minute), 1.0, "{name}");
                        assert!(matches!(schedule.phase(minute), Phase::Night { .. }));
                    }
                }
                Scenario::ClosedFade | Scenario::OpenFade => {
                    // The evening fade has just begun.
                    assert_eq!(schedule.level(now), 0.0, "{name}");
                    let level = schedule.level(later);
                    assert!(0.0 < level && level < 1.0, "{name}: {level}");
                    assert!(matches!(schedule.phase(later), Phase::Night { .. }));
                }
                Scenario::ClosedDay
                | Scenario::OpenIdle
                | Scenario::Drag
                | Scenario::RestDrag
                | Scenario::OpenClose
                | Scenario::OpenCloseBurst
                | Scenario::Start => {
                    for minute in [now, later] {
                        assert_eq!(schedule.level(minute), 0.0, "{name}");
                        assert!(matches!(schedule.phase(minute), Phase::Day { .. }));
                    }
                }
            }
        }
    }

    #[test]
    fn only_the_closed_scenarios_and_open_close_start_in_the_menu_bar() {
        for scenario in Scenario::value_variants() {
            let hidden = matches!(
                scenario,
                Scenario::ClosedDay
                    | Scenario::ClosedNight
                    | Scenario::ClosedFade
                    | Scenario::OpenClose
                    | Scenario::OpenCloseBurst
            );
            assert_eq!(scenario.starts_hidden(), hidden, "{}", flag(*scenario));
        }
    }

    #[test]
    fn open_close_runs_one_cycle_per_three_seconds_and_at_least_one() {
        assert_eq!(Scenario::cycles(30), 10);
        assert_eq!(Scenario::cycles(3), 1);
        assert_eq!(Scenario::cycles(6), 2);
        // Rounded to the nearest cycle.
        assert_eq!(Scenario::cycles(4), 1);
        assert_eq!(Scenario::cycles(5), 2);
        assert_eq!(Scenario::cycles(20), 7);
        // A run too short for one still makes one.
        assert_eq!(Scenario::cycles(1), 1);
        assert_eq!(Scenario::cycles(0), 1);
    }

    #[test]
    fn the_scenarios_serialize_as_the_flag_spells_them() {
        for scenario in Scenario::value_variants() {
            assert_eq!(
                serde_json::to_value(scenario).unwrap(),
                serde_json::Value::String(flag(*scenario))
            );
        }
        assert_eq!(
            serde_json::to_value(Scenario::OpenClose).unwrap(),
            "open-close"
        );
    }

    // Time

    /// How a scenario ends, as the bound of its time reads it (`Scenario::bound`). No arm is a
    /// wildcard: a scenario added to the driver has to be put in a kind here, and the tests below
    /// then hold it to that kind's bound.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Ending {
        /// The driver sleeps until `seconds` after the start, then quits.
        Timed,
        /// `Scenario::cycles(seconds)` cycles, then a quit.
        Cycled,
        /// Quits by itself on its second frame; `seconds` is only its deadline.
        Deadline,
    }

    fn ending(scenario: Scenario) -> Ending {
        match scenario {
            Scenario::ClosedDay
            | Scenario::ClosedNight
            | Scenario::ClosedFade
            | Scenario::OpenIdle
            | Scenario::OpenNight
            | Scenario::OpenFade
            | Scenario::Drag
            | Scenario::RestDrag => Ending::Timed,
            Scenario::OpenClose | Scenario::OpenCloseBurst => Ending::Cycled,
            Scenario::Start => Ending::Deadline,
        }
    }

    /// The scenarios that end in `wanted`.
    fn ending_in(wanted: Ending) -> impl Iterator<Item = Scenario> {
        Scenario::value_variants()
            .iter()
            .copied()
            .filter(move |scenario| ending(*scenario) == wanted)
    }

    /// Whether a drive of `scenario` and `seconds` that took `elapsed_s` is in its time.
    fn in_time(scenario: Scenario, seconds: u64, elapsed_s: f64) -> bool {
        Spoiled::of(scenario, seconds, elapsed_s).is_none()
    }

    /// The report of a drive of `scenario` and `seconds` that took `elapsed_s`, with the `spoiled`
    /// that `Shared::report` gives it.
    fn taking(scenario: Scenario, seconds: u64, elapsed_s: f64) -> Report {
        Report {
            scenario,
            seconds,
            elapsed_s,
            spoiled: Spoiled::of(scenario, seconds, elapsed_s),
            ..unmeasured()
        }
    }

    #[test]
    fn a_timed_drive_is_in_its_time_from_its_seconds_to_two_seconds_past_them() {
        for scenario in ending_in(Ending::Timed) {
            let name = flag(scenario);
            // A 30 s drive: both ends of 30 to 32 s are held, and a little outside either is not.
            assert!(in_time(scenario, 30, 30.0), "{name} at 30.0");
            assert!(in_time(scenario, 30, 32.0), "{name} at 32.0");
            assert!(!in_time(scenario, 30, 29.9), "{name} at 29.9");
            assert!(!in_time(scenario, 30, 32.01), "{name} at 32.01");
            // Drives on record read 0.06 to 0.09 s past their seconds.
            assert!(in_time(scenario, 30, 30.09), "{name} at 30.09");
            assert!(in_time(scenario, 20, 20.08), "{name} at 20.08");
            // The display slept on 2026-10-24, and a 20 s drive took 17 minutes.
            assert_eq!(
                Spoiled::of(scenario, 20, 1020.0),
                Some(Spoiled::Time {
                    elapsed_s: 1020.0,
                    bound: Bound::Between {
                        from: 20.0,
                        to: 22.0
                    },
                }),
                "{name}"
            );
        }
    }

    #[test]
    fn a_cycled_drive_is_in_its_time_from_three_seconds_a_cycle_to_3_25_a_cycle_plus_four() {
        for scenario in ending_in(Ending::Cycled) {
            let name = flag(scenario);
            // 19 s is 6 cycles: 18 to 23.5 s.
            assert_eq!(Scenario::cycles(19), 6);
            assert_eq!(
                scenario.bound(19),
                Bound::Between {
                    from: 18.0,
                    to: 23.5
                },
                "{name}"
            );
            assert!(in_time(scenario, 19, 18.0), "{name} at 18.0");
            assert!(in_time(scenario, 19, 23.5), "{name} at 23.5");
            assert!(!in_time(scenario, 19, 17.9), "{name} at 17.9");
            assert!(!in_time(scenario, 19, 23.6), "{name} at 23.6");
            // A run too short for one cycle still makes one: 3 to 7.25 s.
            assert_eq!(Scenario::cycles(0), 1);
            assert_eq!(
                scenario.bound(0),
                Bound::Between {
                    from: 3.0,
                    to: 7.25
                },
                "{name}"
            );
            // A 30 s run on record read 30.86 and 30.87 s: 10 cycles, which take 30 to 36.5 s.
            assert!(in_time(scenario, 30, 30.86), "{name} at 30.86");
            assert!(in_time(scenario, 30, 30.87), "{name} at 30.87");
            assert!(!in_time(scenario, 30, 36.6), "{name} at 36.6");
        }
    }

    #[test]
    fn the_start_is_spoiled_only_when_its_deadline_quit_it() {
        // Its deadline is `seconds`, 30 unless `--demo-seconds` says. A slow start is a finding, not
        // a spoiled run: it quits by itself, on its second frame.
        assert_eq!(Scenario::Start.bound(30), Bound::Under(30.0));
        for elapsed_s in [0.2, 29.9] {
            assert!(in_time(Scenario::Start, 30, elapsed_s), "{elapsed_s}");
        }
        for elapsed_s in [30.0, 1000.0] {
            assert_eq!(
                Spoiled::of(Scenario::Start, 30, elapsed_s),
                Some(Spoiled::Time {
                    elapsed_s,
                    bound: Bound::Under(30.0)
                }),
                "{elapsed_s}"
            );
        }
        assert_eq!(Scenario::Start.bound(5), Bound::Under(5.0));
        assert!(in_time(Scenario::Start, 5, 4.9));
        assert!(!in_time(Scenario::Start, 5, 5.0));
    }

    #[test]
    fn every_scenario_has_the_bound_of_the_way_it_ends() {
        for scenario in Scenario::value_variants() {
            let name = flag(*scenario);
            for seconds in [5, 19, 30] {
                let bound = scenario.bound(seconds);
                let shape = format!("{name} for {seconds} s: {bound:?}");
                match ending(*scenario) {
                    Ending::Deadline => assert_eq!(bound, Bound::Under(seconds as f64), "{shape}"),
                    Ending::Cycled => {
                        let from = Scenario::cycles(seconds) as f64 * CYCLE.as_secs_f64();
                        assert!(
                            matches!(bound, Bound::Between { from: low, to } if low == from && to > from),
                            "{shape}"
                        );
                    }
                    Ending::Timed => assert!(
                        matches!(bound, Bound::Between { from, to }
                            if from == seconds as f64 && to > from),
                        "{shape}"
                    ),
                }
            }
        }
    }

    #[test]
    fn the_reason_names_the_time_the_bound_and_why_it_matters() {
        let slowed = Spoiled::of(Scenario::Drag, 20, 1020.0).expect("1,020 s of 20 is out");
        let reason = slowed.to_string();
        for says in [
            "elapsed_s 1020.000",
            "20.000 to 22.000",
            "per-frame figures do not compare",
        ] {
            assert!(reason.contains(says), "{says:?} missing from {reason}");
        }
        let quit = Spoiled::of(Scenario::Start, 30, 30.0).expect("the deadline quit it");
        let reason = quit.to_string();
        for says in [
            "elapsed_s 30.000",
            "30.000 s deadline",
            "second frame never came",
            "per-frame figures do not compare",
        ] {
            assert!(reason.contains(says), "{says:?} missing from {reason}");
        }

        // The JSON report holds the reason as a sentence, not as an object; a drive in its time
        // holds `null`.
        let json = serde_json::to_value(taking(Scenario::Drag, 20, 1020.0)).unwrap();
        assert_eq!(json["spoiled"], slowed.to_string());
        assert!(json["spoiled"].is_string());
        assert!(
            serde_json::to_value(taking(Scenario::Drag, 20, 20.06)).unwrap()["spoiled"].is_null()
        );
    }

    #[test]
    fn the_summary_writes_a_spoiled_drives_bound_in_one_pair_with_no_space() {
        let in_its_time = taking(Scenario::Drag, 20, 20.06).summary();
        assert_eq!(value(&in_its_time, "spoiled"), "-");
        let pairs = in_its_time.split(' ').count();
        for (report, pair) in [
            (
                taking(Scenario::Drag, 20, 1020.0),
                "elapsed_s:1020.000,bound_s:20.000..22.000",
            ),
            // 30.1 s of a 30 s `start`: the deadline quit it.
            (
                taking(Scenario::Start, 30, 30.1),
                "elapsed_s:30.100,bound_s:..30.000",
            ),
            // The eighth cycle of ten broke off, at a time inside the bound.
            (
                broken_at(Scenario::OpenClose, 30, 7, Stuck::Close, 31.7),
                "cycles:7/10",
            ),
        ] {
            let line = report.summary();
            assert_eq!(value(&line, "spoiled"), pair, "{line}");
            // No space in the pair: the line keeps its pairs, and `spoiled` stands third, after
            // `scenario` and `finished`.
            assert_eq!(line.split(' ').count(), pairs, "{line}");
            let mut keys = line
                .split(' ')
                .map(|item| item.split_once('=').map(|(key, _)| key));
            assert_eq!(keys.next(), Some(Some("scenario")), "{line}");
            assert_eq!(keys.next(), Some(Some("finished")), "{line}");
            assert_eq!(keys.next(), Some(Some("spoiled")), "{line}");
        }
    }

    /// The report of a run of `scenario` and `seconds` in which `direct` gave up on the cycle
    /// after `done` whole ones, because the window did not do `stuck`, and that took `elapsed_s`.
    fn broken_at(
        scenario: Scenario,
        seconds: u64,
        done: usize,
        stuck: Stuck,
        elapsed_s: f64,
    ) -> Report {
        let shared = Shared::new(Instant::now(), scenario, seconds);
        shared.break_off(done, stuck);
        shared.report_after(false, Duration::from_secs_f64(elapsed_s))
    }

    #[test]
    fn a_cycle_that_broke_off_spoils_the_drive_at_a_time_inside_its_bound() {
        // `direct` waits PATIENCE, 10 s, before it gives up on a cycle, so a 30 s `open-close`
        // (10 cycles, 30 to 36.5 s) that breaks off in its eighth or ninth cycle ends at 31.7 to
        // 35.9 s: in its time, and short of the windows it was asked for.
        for scenario in ending_in(Ending::Cycled) {
            for (done, stuck, elapsed_s) in [
                (7, Stuck::Open, 31.7),
                (7, Stuck::Close, 31.7),
                (8, Stuck::Close, 35.9),
            ] {
                let name = flag(scenario);
                assert!(
                    in_time(scenario, 30, elapsed_s),
                    "{name} at {elapsed_s} is in its time"
                );
                let report = broken_at(scenario, 30, done, stuck, elapsed_s);
                assert_eq!(
                    report.spoiled,
                    Some(Spoiled::Cycles(BrokeOff {
                        done,
                        planned: 10,
                        stuck
                    })),
                    "{name} after {done} cycles at {elapsed_s}"
                );
            }
        }
    }

    #[test]
    fn a_cycle_that_broke_off_is_the_reason_whatever_the_time() {
        // Out of its bound too, below it or past it: the cycle's reason comes before the time's.
        for elapsed_s in [10.0, 29.9, 36.6, 1000.0] {
            assert!(!in_time(Scenario::OpenClose, 30, elapsed_s), "{elapsed_s}");
            let report = broken_at(Scenario::OpenClose, 30, 0, Stuck::Open, elapsed_s);
            assert_eq!(
                report.spoiled,
                Some(Spoiled::Cycles(BrokeOff {
                    done: 0,
                    planned: 10,
                    stuck: Stuck::Open
                })),
                "{elapsed_s}"
            );
        }
        // The time alone is still `Spoiled::of`'s, and a run that broke nothing has only that.
        let whole = Shared::new(Instant::now(), Scenario::OpenClose, 30);
        let report = whole.report_after(true, Duration::from_secs(1000));
        assert_eq!(report.spoiled, Spoiled::of(Scenario::OpenClose, 30, 1000.0));
        assert!(matches!(report.spoiled, Some(Spoiled::Time { .. })));
        let report = whole.report_after(true, Duration::from_secs_f64(31.7));
        assert_eq!(report.spoiled, None);
    }

    #[test]
    fn the_reason_of_a_broken_cycle_names_the_cycle_what_failed_and_the_counts() {
        for (done, stuck, did_not) in [
            (7, Stuck::Open, "did not open"),
            (7, Stuck::Close, "did not close"),
            (0, Stuck::Open, "did not open"),
        ] {
            let broke = BrokeOff {
                done,
                planned: 10,
                stuck,
            };
            let reason = Spoiled::Cycles(broke).to_string();
            let cycle = format!("cycle {} broke off", done + 1);
            let counts = format!("{done} of 10 cycles");
            for says in [
                cycle.as_str(),
                did_not,
                counts.as_str(),
                "fewer windows than asked",
                "figures do not compare",
            ] {
                assert!(reason.contains(says), "{says:?} missing from {reason}");
            }
            // The JSON report holds it as a sentence, like the time's.
            let json = serde_json::to_value(broken_at(Scenario::OpenClose, 30, done, stuck, 31.7))
                .unwrap();
            assert_eq!(json["spoiled"], reason);
        }
        // The time's reason no longer says that a broken cycle is cut short.
        let reason = Spoiled::of(Scenario::Drag, 20, 1020.0).unwrap().to_string();
        assert!(reason.contains("slowed or stopped"), "{reason}");
        assert!(!reason.contains("cut short"), "{reason}");
    }

    #[test]
    fn elapsed_s_is_one_value_rounded_to_the_millisecond_that_the_bound_and_the_reason_read() {
        // 32.0004 s of a 30 s drive rounds to 32.000: in its bound, spoiled for nothing.
        let shared = Shared::new(Instant::now(), Scenario::Drag, 30);
        let report = shared.report_after(true, Duration::from_micros(32_000_400));
        assert_eq!(report.elapsed_s, 32.0);
        assert_eq!(report.spoiled, None);
        // 32.0006 s rounds to 32.001: spoiled, and the JSON, the summary and the reason say the
        // same time, which is outside the bound they write at the same three decimals.
        let report = shared.report_after(true, Duration::from_micros(32_000_600));
        assert_eq!(report.elapsed_s, 32.001);
        let reason = report.spoiled.expect("32.001 s is past 32.000").to_string();
        assert!(
            reason.contains("elapsed_s 32.001 s is outside 30.000 to 32.000 s"),
            "{reason}"
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["elapsed_s"], 32.001);
        assert_eq!(json["spoiled"], reason);
        assert_eq!(
            value(&report.summary(), "spoiled"),
            "elapsed_s:32.001,bound_s:30.000..32.000"
        );
    }

    #[test]
    fn a_time_that_is_no_number_is_in_no_bound() {
        for scenario in Scenario::value_variants() {
            for seconds in [5, 30] {
                assert!(
                    !scenario.bound(seconds).holds(f64::NAN),
                    "{} for {seconds} s",
                    flag(*scenario)
                );
                assert!(Spoiled::of(*scenario, seconds, f64::NAN).is_some());
            }
        }
    }

    #[test]
    fn a_report_takes_its_spoiled_from_the_scenario_seconds_and_time_it_ran() {
        // Made right away, a 5 s timed drive has run for about nothing: it is under its 5 s.
        for finished in [true, false] {
            let report = Shared::new(Instant::now(), Scenario::ClosedDay, 5).report(finished);
            assert!(report.elapsed_s < 5.0, "{}", report.elapsed_s);
            assert!(report.spoiled.is_some());
            assert_eq!(
                report.spoiled,
                Spoiled::of(Scenario::ClosedDay, 5, report.elapsed_s)
            );
        }
        // A start that quit by itself, in about nothing, is in its time.
        for finished in [true, false] {
            let report = Shared::new(Instant::now(), Scenario::Start, 30).report(finished);
            assert!(report.elapsed_s < 30.0, "{}", report.elapsed_s);
            assert_eq!(report.spoiled, None);
        }
    }

    #[test]
    fn the_fixtures_hold_the_spoiled_that_the_driver_would_give_them() {
        // No fixture pins a report that the driver cannot produce.
        for report in [measured(), dragged(), unmeasured()] {
            assert_eq!(
                report.spoiled,
                Spoiled::of(report.scenario, report.seconds, report.elapsed_s),
                "{} for {} s at {} s",
                flag(report.scenario),
                report.seconds,
                report.elapsed_s
            );
        }
    }

    /// The number that `source` gives `name` on a line of its own: `NAME = <number>`, from column
    /// 0, with a comment after it or none. An error when no line is so, or two are, or the value
    /// is no number.
    fn constant_in(source: &str, name: &str) -> Result<f64, String> {
        let head = format!("{name} = ");
        let lines: Vec<&str> = source
            .lines()
            .filter(|line| line.starts_with(&head))
            .collect();
        let [line] = lines.as_slice() else {
            return Err(format!("{} lines begin with {head:?}", lines.len()));
        };
        let text = line[head.len()..].split('#').next().unwrap_or("").trim();
        text.parse()
            .map_err(|error| format!("{line:?} is no number: {error}"))
    }

    /// `name`, a number, in the Python script `file` of `scripts/measure/`.
    fn python_constant(file: &str, name: &str) -> f64 {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts/measure")
            .join(file);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        constant_in(&source, name).unwrap_or_else(|error| panic!("{file}, {name}: {error}"))
    }

    #[test]
    fn the_scripts_bounds_are_the_drivers() {
        // The three scripts that start a timed drive want SECONDS to SECONDS + SLACK.
        for file in ["energy-drag.py", "energy-system.py", "tear-scan.py"] {
            assert_eq!(
                python_constant(file, "SLACK"),
                TIMED_SLACK_S,
                "{file}'s SLACK"
            );
        }

        // `close-scan.py` wants `closes` cycles to take CYCLE each at least and CYCLE + OVERRUN
        // each plus SLACK at most.
        let cycle = python_constant("close-scan.py", "CYCLE");
        let overrun = python_constant("close-scan.py", "OVERRUN");
        let slack = python_constant("close-scan.py", "SLACK");
        assert_eq!(cycle, CYCLE.as_secs_f64(), "close-scan.py's CYCLE");
        assert_eq!(overrun, CYCLE_OVERRUN_S, "close-scan.py's OVERRUN");
        assert_eq!(slack, CYCLED_SLACK_S, "close-scan.py's SLACK");
        let closes = Scenario::cycles(19) as f64;
        let script = Bound::Between {
            from: closes * cycle,
            to: closes * (cycle + overrun) + slack,
        };
        for scenario in ending_in(Ending::Cycled) {
            assert_eq!(script, scenario.bound(19), "{}", flag(scenario));
        }
    }

    #[test]
    fn a_constant_that_is_not_there_once_as_a_number_is_an_error_and_not_a_zero() {
        let script = "SECONDS = 30  # the drive's length\nSLACK = 2.0  # how far\n    SLACK = 9\n# SLACK = 5\n";
        // The indented line and the comment are no line of the constant's own.
        assert_eq!(constant_in(script, "SLACK"), Ok(2.0));
        assert_eq!(constant_in(script, "SECONDS"), Ok(30.0));
        // None.
        assert!(constant_in(script, "OVERRUN").is_err());
        // Another name that begins with it.
        assert!(constant_in("SLACK_S = 2.0\n", "SLACK").is_err());
        // Twice.
        assert!(constant_in("SLACK = 2.0\nSLACK = 3.0\n", "SLACK").is_err());
        // Not a number.
        assert!(
            constant_in(
                "SECONDS = int(os.environ.get(\"X\", \"30\"))  # 30\n",
                "SECONDS"
            )
            .is_err()
        );
        // And a script without it stops the test that reads it.
        let missing =
            std::panic::catch_unwind(|| python_constant("energy-drag.py", "NO_SUCH_NAME"));
        assert!(missing.is_err());
    }

    // The drag with rests

    /// The presses (true) and releases (false) in `events`.
    fn buttons(events: &[egui::Event]) -> Vec<bool> {
        events
            .iter()
            .filter_map(|event| match event {
                egui::Event::PointerButton { pressed, .. } => Some(*pressed),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_rest_follows_each_stretch_and_ends_as_the_next_begins() {
        let millis = Duration::from_millis;
        let cycle = STRETCH + REST;
        for (cycles, offset) in [(0, Duration::ZERO), (1, cycle), (7, cycle * 7)] {
            let label = format!("cycle {cycles}");
            assert_eq!(rest_left(offset), None, "{label}: a stretch begins");
            assert_eq!(rest_left(offset + STRETCH - millis(1)), None, "{label}");
            assert_eq!(rest_left(offset + STRETCH), Some(REST), "{label}");
            assert_eq!(
                rest_left(offset + STRETCH + millis(250)),
                Some(REST - millis(250)),
                "{label}"
            );
            assert_eq!(
                rest_left(offset + cycle - millis(1)),
                Some(millis(1)),
                "{label}: never zero, so a frame there still waits"
            );
        }
    }

    /// What the scenario is for: a rest in which the link pauses. It comes after the quick fade that
    /// follows a let-go (1.2 s, `engine::QUICK_FADE`) and 32 quiet refreshes at 120 Hz (about 270 ms).
    /// At 1 s, a drive read `starts` 1: the fade's frames kept the burst alive through every rest.
    #[test]
    fn a_rest_is_long_enough_for_the_link_to_pause() {
        assert!(REST > Duration::from_millis(1_200 + 270), "{REST:?}");
    }

    #[test]
    fn each_stretch_presses_at_the_day_end_and_the_rest_lets_go_once() {
        let (mut hand, begun) = (RestDrag::default(), Instant::now());
        let period = pointer_period();
        for drawn in 0..FRAMES_BEFORE_PRESS {
            assert!(hand.events_at(drawn, begun).is_empty(), "frame {drawn}");
        }
        for stretch in 0..3 {
            let start = begun + (STRETCH + REST) * stretch;
            let press = hand.events_at(FRAMES_BEFORE_PRESS, start);
            assert_eq!(buttons(&press), [true], "stretch {stretch} presses");
            let egui::Event::PointerMoved(pointer) = press[0] else {
                panic!("a move first, got {press:?}");
            };
            assert_eq!(pointer, egui::pos2(TRACK.0, TEMPERATURE_Y));

            // The pacing is `Drag`'s: the next period brings one move and no button.
            let half = period / 2;
            assert_eq!(hand.next_in(start + half), period - half);
            assert!(hand.events_at(99, start + half).is_empty());
            let moved = hand.events_at(99, start + period);
            assert!(
                matches!(moved[..], [egui::Event::PointerMoved(_)]),
                "{moved:?}"
            );

            // The rest's first frame lets go, at the Day end, where the sweep brought the pointer.
            let rest = start + STRETCH;
            let release = hand.events_at(99, rest);
            assert_eq!(buttons(&release), [false], "stretch {stretch} lets go");
            let egui::Event::PointerButton { pos, .. } = release[1] else {
                panic!("a move, then the release: {release:?}");
            };
            assert_eq!(pos, egui::pos2(sweep(STRETCH), TEMPERATURE_Y));

            // No more frames with anything in them, and each asks for the rest's end.
            let quarter = REST / 4;
            for later in [rest, rest + quarter, rest + REST - quarter] {
                assert!(hand.events_at(99, later).is_empty());
                assert_eq!(hand.next_in(later), REST - (later - rest));
            }
        }
    }

    #[test]
    fn a_frame_late_into_a_stretch_still_starts_it_with_a_press() {
        // A late frame must not move a pointer whose button nobody pressed.
        let (mut hand, begun) = (RestDrag::default(), Instant::now());
        hand.events_at(FRAMES_BEFORE_PRESS, begun);
        hand.events_at(99, begun + STRETCH);
        let late = begun + STRETCH + REST + Duration::from_millis(500);
        let events = hand.events_at(99, late);
        assert_eq!(buttons(&events), [true], "{events:?}");
    }

    #[test]
    fn a_stretch_with_no_frame_in_the_rest_still_lets_go_at_the_next_rest() {
        // A stall that jumps the whole rest: the button stays down until a rest has a frame.
        let (mut hand, begun) = (RestDrag::default(), Instant::now());
        hand.events_at(FRAMES_BEFORE_PRESS, begun);
        let cycle = STRETCH + REST;
        let moved = hand.events_at(99, begun + cycle);
        assert_eq!(buttons(&moved), Vec::<bool>::new(), "{moved:?}");
        let release = hand.events_at(99, begun + cycle + STRETCH);
        assert_eq!(buttons(&release), [false], "{release:?}");
    }

    // The drag

    #[test]
    fn the_sweep_runs_the_track_out_and_back_each_period() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(
            near(sweep(Duration::ZERO), TRACK.0),
            "starts at the Day end"
        );
        assert!(near(sweep(SWEEP / 2), TRACK.1), "half way is the far end");
        assert!(near(sweep(SWEEP), TRACK.0), "a whole period is back");
        assert!(near(sweep(SWEEP * 3 / 2), TRACK.1), "and again");

        let steps = 100;
        let mut before = sweep(Duration::ZERO);
        for step in 1..=2 * steps {
            let now = sweep(SWEEP * step / steps);
            assert!(TRACK.0 - 1e-3 <= now && now <= TRACK.1 + 1e-3, "{now}");
            let rising = (step - 1) % steps < steps / 2;
            assert!(
                if rising { now >= before } else { now <= before },
                "step {step}: {before} then {now}"
            );
            before = now;
        }
    }

    /// The moves in `events`.
    fn moves(events: &[egui::Event]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, egui::Event::PointerMoved(_)))
            .count()
    }

    #[test]
    fn the_drag_waits_presses_once_and_moves_once_a_period() {
        let (mut drag, begun) = (Drag::default(), Instant::now());
        let period = pointer_period();
        for drawn in 0..FRAMES_BEFORE_PRESS {
            assert!(drag.events_at(drawn, begun).is_empty(), "frame {drawn}");
        }

        let press = drag.events_at(FRAMES_BEFORE_PRESS, begun);
        let [
            egui::Event::PointerMoved(moved),
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                ..
            },
        ] = &press[..]
        else {
            panic!("a move and a press, got {press:?}");
        };
        assert_eq!(moved, pos, "the press is where the pointer is");
        assert_eq!(*button, egui::PointerButton::Primary);
        assert!(*pressed);
        assert_eq!(moved.y, TEMPERATURE_Y);
        assert_eq!(moved.x, TRACK.0, "at the Day end");

        // The press was the first move. A frame before the next is due adds
        // nothing, however many come; the one that reaches it moves once.
        let mut drawn = FRAMES_BEFORE_PRESS + 1;
        for part in [
            Duration::ZERO,
            period / 3,
            period / 2,
            period - Duration::from_nanos(1),
        ] {
            let events = drag.events_at(drawn, begun + part);
            assert!(events.is_empty(), "{part:?} into the period: {events:?}");
            drawn += 1;
        }
        for k in 1..20 {
            let events = drag.events_at(drawn, begun + period * k);
            let [egui::Event::PointerMoved(at)] = &events[..] else {
                panic!("only a move, never a second press or a release: {events:?}");
            };
            assert_eq!(at.y, TEMPERATURE_Y);
            assert!(TRACK.0 <= at.x && at.x <= TRACK.1);
            assert_eq!(*at, egui::pos2(sweep(period * k), TEMPERATURE_Y));
            drawn += 1;
            let again = drag.events_at(drawn, begun + period * k + period / 2);
            assert!(again.is_empty(), "twice in period {k}: {again:?}");
            drawn += 1;
        }
    }

    #[test]
    fn a_drag_of_n_seconds_makes_n_times_the_pointer_rate_in_moves() {
        let period = pointer_period();
        let millisecond = Duration::from_millis(1);
        // The frames come on a grid finer than the pointer's period (as when
        // something else asks for a frame), on the period itself, and a
        // little finer than it.
        for step in [millisecond, period / 3, period, period - millisecond / 2] {
            for seconds in [1_u32, 2, 5] {
                let (mut drag, begun) = (Drag::default(), Instant::now());
                let span = Duration::from_secs(u64::from(seconds));
                let (mut made, mut drawn, mut at) = (0, FRAMES_BEFORE_PRESS, Duration::ZERO);
                while at < span {
                    let events = drag.events_at(drawn, begun + at);
                    assert!(moves(&events) <= 1, "a move twice in a frame: {events:?}");
                    made += moves(&events);
                    (drawn, at) = (drawn + 1, at + step);
                }
                let expected = (POINTER_RATE * f64::from(seconds)) as i64;
                assert!(
                    (made as i64 - expected).abs() <= 1,
                    "{made} moves in {seconds} s, frames every {step:?}: expected {expected}"
                );
            }
        }
    }

    #[test]
    fn a_window_that_waits_for_the_next_move_draws_at_the_pointer_rate() {
        // Each frame ends by asking for the next one `next_in` from now, and
        // the wake-up comes a little late, as a timer's does. The lateness
        // must not pile up into a slower pointer.
        let (mut drag, begun) = (Drag::default(), Instant::now());
        let late = Duration::from_micros(300);
        let mut now = begun;
        for drawn in 0..FRAMES_BEFORE_PRESS {
            assert!(drag.events_at(drawn, now).is_empty());
            assert_eq!(drag.next_in(now), pointer_period(), "frame {drawn}");
            now += pointer_period() + late;
        }
        let pressed = now;
        let (mut drawn, mut frames) = (FRAMES_BEFORE_PRESS, 0);
        for seconds in [1_u32, 2, 5] {
            while now - pressed < Duration::from_secs(u64::from(seconds)) {
                let events = drag.events_at(drawn, now);
                // Each frame the timer brings is a move: none is wasted.
                assert_eq!(moves(&events), 1, "frame {drawn}: {events:?}");
                (drawn, frames) = (drawn + 1, frames + 1);
                now += drag.next_in(now) + late;
            }
            // The frames so far, over the seconds so far.
            let expected = (POINTER_RATE * f64::from(seconds)) as i64;
            assert!(
                (frames - expected).abs() <= 1,
                "{frames} frames in {seconds} s: expected {expected}"
            );
        }
    }

    #[test]
    fn a_stalled_drag_does_not_make_up_the_moves_it_missed() {
        let (mut drag, begun) = (Drag::default(), Instant::now());
        let period = pointer_period();
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        let first = drag.events_at(FRAMES_BEFORE_PRESS, begun);
        assert_eq!(moves(&first), 1);

        // The next frame comes half a sweep late: one move, at the far end.
        let stalled = begun + SWEEP / 2;
        let events = drag.events_at(FRAMES_BEFORE_PRESS + 1, stalled);
        let [egui::Event::PointerMoved(at)] = &events[..] else {
            panic!("one move after a stall, got {events:?}");
        };
        assert!(near(at.x, TRACK.1), "the pointer follows the time: {at:?}");
        // The moves missed are not owed: the schedule starts again from here.
        let soon = drag.events_at(FRAMES_BEFORE_PRESS + 2, stalled + period / 2);
        assert!(soon.is_empty(), "a burst after the stall: {soon:?}");
        let next = drag.events_at(FRAMES_BEFORE_PRESS + 3, stalled + period);
        assert_eq!(moves(&next), 1);
    }

    #[test]
    fn egui_waits_for_the_frame_the_window_asks_for_not_a_frame_sooner() {
        use std::sync::{Arc, Mutex};

        let ctx = egui::Context::default();
        let delays = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&delays);
        ctx.set_request_repaint_callback(move |info| seen.lock().unwrap().push(info.delay));
        // One pass, with `request` called inside it. There is no renderer to
        // take the textures.
        let pass = |request: &dyn Fn(&egui::Context)| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| request(ui.ctx()));
            output.textures_delta.clear();
        };
        // A new context repaints a couple of times on its own.
        for _ in 0..3 {
            pass(&|_| {});
        }
        delays.lock().unwrap().clear();

        // A bare `request_repaint_after` of a pointer period reaches the
        // integration as zero (egui takes its predicted frame time off the
        // delay); `repaint_after` gets it through.
        let period = pointer_period();
        pass(&|ctx| repaint_after(ctx, period));
        let delays = delays.lock().unwrap();
        let [delay] = delays[..] else {
            panic!("one request, got {delays:?}");
        };
        let off = delay.abs_diff(period);
        assert!(
            off < Duration::from_micros(10),
            "asked {period:?}, got {delay:?}"
        );
    }

    #[test]
    fn the_next_frame_is_asked_for_when_the_next_move_is_due() {
        let (mut drag, begun) = (Drag::default(), Instant::now());
        let period = pointer_period();
        // Before the press there is no move to wait for: a period.
        assert_eq!(drag.next_in(begun), period);
        drag.events_at(FRAMES_BEFORE_PRESS, begun);
        assert_eq!(drag.next_in(begun), period);
        assert_eq!(drag.next_in(begun + period / 2), period - period / 2);
        assert_eq!(drag.next_in(begun + period), Duration::ZERO);
        // Never negative.
        assert_eq!(drag.next_in(begun + period * 3), Duration::ZERO);
    }

    // Spread

    #[test]
    fn a_spread_needs_samples() {
        assert_eq!(Spread::of(&[]), None);
        assert_eq!(
            Spread::of(&[f64::NAN, f64::INFINITY, f64::NEG_INFINITY]),
            None
        );
    }

    #[test]
    fn one_sample_is_every_percentile() {
        let one = Spread::of(&[2.5]).expect("a spread");
        assert_eq!(
            one,
            Spread {
                samples: 1,
                p50: 2.5,
                p90: 2.5,
                p99: 2.5,
                max: 2.5,
            }
        );
    }

    #[test]
    fn percentiles_are_nearest_rank_whatever_the_order() {
        let mut samples: Vec<f64> = (1..=100).map(f64::from).collect();
        samples.reverse();
        let spread = Spread::of(&samples).expect("a spread");
        assert_eq!(
            spread,
            Spread {
                samples: 100,
                p50: 50.0,
                p90: 90.0,
                p99: 99.0,
                max: 100.0,
            }
        );
    }

    #[test]
    fn a_spread_leaves_out_what_is_not_a_number() {
        let spread =
            Spread::of(&[f64::NAN, 1.0, f64::INFINITY, 3.0, f64::NEG_INFINITY]).expect("a spread");
        assert_eq!(spread.samples, 2);
        assert_eq!(spread.max, 3.0);
    }

    // The report

    fn measured() -> Report {
        let open = |ms: f64, footprint: u64| Open {
            to_first_frame_ms: Some(ms),
            first_frame_cpu_ms: Some(ms - 4.0),
            frames: 12,
            open_ms: Some(1000.0),
            footprint_after_close: Some(footprint),
            threads_after_close: Some(9),
            renderer: "metal",
            shader: Some("file"),
            tray: true,
        };
        Report {
            scenario: Scenario::OpenClose,
            seconds: 30,
            finished: true,
            elapsed_s: 31.5,
            // 30 s of `open-close` is 10 cycles, which take 30 to 36.5 s.
            spoiled: None,
            pixels_per_point: Some(2.0),
            frames: 120,
            window_open_s: 10.0,
            frames_per_second: Some(12.0),
            unseen_frames: 4,
            frame_cpu_ms: Spread::of(&[1.0, 2.0, 3.0]),
            // `open-close` moves nothing.
            move_frame_cpu_ms: None,
            cover: CoverCounts {
                layer_builds: 10,
                rim_composes: 0,
                shade_composes: 0,
                uploaded_bytes: 2_959_360,
            },
            cover_builds_per_frame: Some(0.0833),
            cover_bytes_per_frame: Some(24_661.0),
            screen: app::screen_counts::ScreenCounts {
                checks: 4,
                paints: 1,
                restores: 1,
            },
            headless_frames: 23,
            main_to_first_frame_ms: Some(95.5),
            opens: vec![open(41.0, 73_000_000), open(40.0, 99_700_000)],
            fallbacks: vec![Fallback {
                why: "ARRAKIS_METAL_FAIL=frame".to_owned(),
                frames: 2,
                at_open: 1,
                screen_checks: 3,
            }],
            footprint_after_first_close: Some(73_000_000),
            footprint_after_last_close: Some(99_700_000),
            footprint_at_end: Some(100_000_000),
            hold: HoldCounts {
                ticks: 3_600,
                starts: 1,
                waits: 3_500,
                timeouts: 0,
                link_make_ms: Some(0.25),
                spare_drops: 2,
            },
        }
    }

    /// `measured()` as a `drag` leaves it: the same figures, and a move spread, which only the
    /// scenarios that move the pointer have.
    fn dragged() -> Report {
        Report {
            scenario: Scenario::Drag,
            move_frame_cpu_ms: Spread::of(&[2.0, 3.0]),
            ..measured()
        }
    }

    /// A run that measured nothing: no window ever drew a frame.
    fn unmeasured() -> Report {
        Report {
            scenario: Scenario::ClosedDay,
            seconds: 5,
            finished: false,
            elapsed_s: 5.0,
            // 5 s of a timed scenario takes 5 to 7 s.
            spoiled: None,
            pixels_per_point: None,
            frames: 0,
            window_open_s: 0.0,
            frames_per_second: None,
            unseen_frames: 0,
            frame_cpu_ms: None,
            move_frame_cpu_ms: None,
            cover: CoverCounts::default(),
            cover_builds_per_frame: None,
            cover_bytes_per_frame: None,
            screen: app::screen_counts::ScreenCounts::default(),
            headless_frames: 0,
            main_to_first_frame_ms: None,
            opens: Vec::new(),
            fallbacks: Vec::new(),
            footprint_after_first_close: None,
            footprint_after_last_close: None,
            footprint_at_end: None,
            hold: HoldCounts::default(),
        }
    }

    fn keys(value: &serde_json::Value) -> BTreeSet<&str> {
        value
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn names<const N: usize>(names: [&'static str; N]) -> BTreeSet<&'static str> {
        names.into()
    }

    #[test]
    fn the_report_has_the_field_names_the_docs_and_scripts_read() {
        let json = serde_json::to_value(measured()).unwrap();
        assert_eq!(
            keys(&json),
            names([
                "scenario",
                "seconds",
                "finished",
                "elapsed_s",
                "spoiled",
                "pixels_per_point",
                "frames",
                "window_open_s",
                "frames_per_second",
                "unseen_frames",
                "frame_cpu_ms",
                "move_frame_cpu_ms",
                "cover",
                "cover_builds_per_frame",
                "cover_bytes_per_frame",
                "screen",
                "headless_frames",
                "main_to_first_frame_ms",
                "opens",
                "fallbacks",
                "footprint_after_first_close",
                "footprint_after_last_close",
                "footprint_at_end",
                "hold",
            ])
        );
        assert_eq!(
            keys(&json["fallbacks"][0]),
            names(["why", "frames", "at_open", "screen_checks"])
        );
        assert_eq!(
            keys(&json["hold"]),
            names([
                "ticks",
                "starts",
                "waits",
                "timeouts",
                "link_make_ms",
                "spare_drops",
            ])
        );
        assert_eq!(
            keys(&json["frame_cpu_ms"]),
            names(["samples", "p50", "p90", "p99", "max"])
        );
        // The scenarios that move nothing have the key, and null.
        assert!(json["move_frame_cpu_ms"].is_null());
        // A drive in its time has the key, and null.
        assert!(json["spoiled"].is_null());
        assert_eq!(
            keys(&json["cover"]),
            names([
                "layer_builds",
                "rim_composes",
                "shade_composes",
                "uploaded_bytes"
            ])
        );
        assert_eq!(
            keys(&json["screen"]),
            names(["checks", "paints", "restores"])
        );
        let opens = json["opens"].as_array().expect("a list");
        assert_eq!(opens.len(), 2);
        for open in opens {
            assert_eq!(
                keys(open),
                names([
                    "to_first_frame_ms",
                    "first_frame_cpu_ms",
                    "frames",
                    "open_ms",
                    "footprint_after_close",
                    "threads_after_close",
                    "renderer",
                    "shader",
                    "tray",
                ])
            );
        }
        assert_eq!(opens[0]["renderer"], "metal");
        assert_eq!(opens[0]["shader"], "file");
        assert_eq!(json["scenario"], "open-close");
        assert_eq!(json["finished"], true);
        assert_eq!(json["cover"]["uploaded_bytes"], 2_959_360);
        assert_eq!(json["unseen_frames"], 4);
        assert_eq!(opens[1]["footprint_after_close"], 99_700_000);
    }

    #[test]
    fn a_report_that_measured_nothing_keeps_its_fields_as_null() {
        let measured = serde_json::to_value(measured()).unwrap();
        let nothing = serde_json::to_value(unmeasured()).unwrap();
        assert_eq!(keys(&nothing), keys(&measured));
        for key in [
            "pixels_per_point",
            "frames_per_second",
            "frame_cpu_ms",
            "move_frame_cpu_ms",
            "cover_builds_per_frame",
            "cover_bytes_per_frame",
            "main_to_first_frame_ms",
            "footprint_after_first_close",
            "footprint_after_last_close",
            "footprint_at_end",
        ] {
            assert!(nothing[key].is_null(), "{key}");
        }
        assert_eq!(nothing["opens"], serde_json::json!([]));
        // A count, not a measure: a run that drew nothing saw nothing covered.
        assert_eq!(nothing["unseen_frames"], 0);
    }

    #[test]
    fn a_drags_report_has_the_move_spread_under_its_key_beside_the_frames_spread() {
        let json = serde_json::to_value(dragged()).unwrap();
        assert_eq!(json["scenario"], "drag");
        assert_eq!(
            keys(&json["move_frame_cpu_ms"]),
            names(["samples", "p50", "p90", "p99", "max"])
        );
        assert_eq!(json["move_frame_cpu_ms"]["samples"], 2);
        assert_eq!(json["move_frame_cpu_ms"]["p50"], 2.0);
        assert_eq!(json["move_frame_cpu_ms"]["max"], 3.0);
        assert_eq!(json["frame_cpu_ms"]["samples"], 3);
    }

    // Frames the window did not show

    fn viewport(minimized: Option<bool>, occluded: Option<bool>) -> egui::ViewportInfo {
        egui::ViewportInfo {
            minimized,
            occluded,
            ..Default::default()
        }
    }

    /// A frame as `Driven::note_frame` hands it to the record: it began with the CPU eframe
    /// reported, and the driver put a `PointerMoved` in its input or did not.
    fn seen(cpu_ms: Option<f64>, moved: bool) -> Begun {
        Begun {
            cpu_ms,
            unseen: false,
            moved,
        }
    }

    /// The same for a frame that began with the window covered or minimized.
    fn covered(cpu_ms: Option<f64>, moved: bool) -> Begun {
        Begun {
            unseen: true,
            ..seen(cpu_ms, moved)
        }
    }

    #[test]
    fn a_covered_or_minimized_window_is_unseen_and_the_first_frames_are_seen() {
        // Another window over it.
        assert!(is_unseen(&viewport(Some(false), Some(true))));
        assert!(is_unseen(&viewport(None, Some(true))));
        // Minimized.
        assert!(is_unseen(&viewport(Some(true), Some(false))));
        assert!(is_unseen(&viewport(Some(true), None)));
        // Neither.
        assert!(!is_unseen(&viewport(Some(false), Some(false))));
        // Not told yet, as in the first frames: seen.
        assert!(!is_unseen(&viewport(None, None)));
        assert!(!is_unseen(&viewport(Some(false), None)));
        assert!(!is_unseen(&viewport(None, Some(false))));
        assert!(!is_unseen(&egui::ViewportInfo::default()));
    }

    /// What `note_frame` reads, in the frames eframe runs: a window it shows gets a pass, and one it
    /// does not show gets only its logic, from `run_logic`, with no pass.
    #[test]
    fn the_context_tells_a_frame_whether_the_window_is_seen_with_a_pass_and_without() {
        let ctx = egui::Context::default();
        let raw = |info: egui::ViewportInfo| {
            let mut raw = egui::RawInput::default();
            raw.viewports.insert(raw.viewport_id, info);
            raw
        };
        for (info, unseen) in [
            (viewport(Some(false), Some(false)), false),
            (viewport(None, None), false),
            (viewport(Some(false), Some(true)), true),
            (viewport(Some(true), Some(false)), true),
        ] {
            let mut by_pass = None;
            let mut output = ctx.run_ui(raw(info.clone()), |ui| {
                by_pass = Some(window_unseen(ui.ctx()));
            });
            output.textures_delta.clear();
            let mut by_logic = None;
            let _ = ctx.run_logic(&raw(info.clone()), |ctx| {
                by_logic = Some(window_unseen(ctx));
            });
            assert_eq!(by_pass, Some(unseen), "{info:?} with a pass");
            assert_eq!(by_logic, Some(unseen), "{info:?} with logic only");
        }
    }

    #[test]
    fn a_frame_counts_as_unseen_only_when_it_was() {
        let mut record = Record::default();
        record.count_frame(seen(None, false));
        record.count_frame(covered(Some(0.5), false));
        record.count_frame(seen(Some(0.25), false));
        record.count_frame(seen(Some(1.0), false));
        assert_eq!(record.frames, 4);
        assert_eq!(record.unseen_frames, 1);
        assert_eq!(record.cpu_ms, [0.5, 0.25, 1.0]);
    }

    #[test]
    fn the_report_carries_the_unseen_frames_and_a_run_with_no_window_has_none() {
        let shared = Shared::new(Instant::now(), Scenario::Drag, 5);
        {
            let mut record = shared.lock();
            record.count_frame(seen(Some(0.5), false));
            record.count_frame(covered(Some(0.5), false));
            record.count_frame(covered(Some(0.5), false));
        }
        let report = shared.report(false);
        assert_eq!((report.frames, report.unseen_frames), (3, 2));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["unseen_frames"], 2);
        assert!(report.summary().split(' ').any(|pair| pair == "unseen=2"));

        let closed = Shared::new(Instant::now(), Scenario::ClosedDay, 5).report(false);
        assert_eq!((closed.frames, closed.unseen_frames), (0, 0));
    }

    // The frames that carry a move

    fn count_frames(record: &mut Record, frames: &[Begun]) {
        for &begun in frames {
            record.count_frame(begun);
        }
    }

    #[test]
    fn the_move_frames_hold_the_cpu_reported_on_the_frame_after_each_move() {
        let shared = Shared::new(Instant::now(), Scenario::Drag, 5);
        count_frames(
            &mut shared.lock(),
            &[
                // eframe has reported nothing on the first frame.
                seen(None, false),
                // This frame carries a move, and the next one reads its CPU, 2.0.
                seen(Some(1.0), true),
                seen(Some(2.0), false),
                // The same again, with 4.0.
                seen(Some(3.0), true),
                seen(Some(4.0), false),
                seen(Some(5.0), false),
            ],
        );
        {
            let record = shared.lock();
            assert_eq!(record.move_cpu_ms, [2.0, 4.0]);
            assert_eq!(record.cpu_ms, [1.0, 2.0, 3.0, 4.0, 5.0]);
        }

        let report = shared.report(false);
        assert_eq!(report.frames, 6);
        assert_eq!(report.move_frame_cpu_ms, Spread::of(&[2.0, 4.0]));
        assert_eq!(report.frame_cpu_ms, Spread::of(&[1.0, 2.0, 3.0, 4.0, 5.0]));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["move_frame_cpu_ms"]["samples"], 2);
        assert_eq!(json["move_frame_cpu_ms"]["max"], 4.0);
        assert_eq!(json["frame_cpu_ms"]["samples"], 5);
        assert_eq!(json["frame_cpu_ms"]["max"], 5.0);
    }

    /// The trap, in values: frame k carries the move, and its CPU is the one that
    /// frame k + 1 reports, not the one frame k + 1 spends.
    #[test]
    fn the_cpu_of_a_move_is_what_the_next_frame_reports_and_not_that_frames_own() {
        let cpu_of = |frame: u64| frame as f64 * 10.0;
        let moved = [2, 5];
        let mut record = Record::default();
        for frame in 1..=7_u64 {
            // The driver's events for the frame, as `raw_input_hook` makes them, and the CPU that
            // eframe reports as the frame begins: the frame before's.
            let events = if moved.contains(&frame) {
                vec![egui::Event::PointerMoved(egui::pos2(100.0, 20.0))]
            } else {
                Vec::new()
            };
            record.count_frame(seen(
                (frame > 1).then(|| cpu_of(frame - 1)),
                holds_a_move(&events),
            ));
        }
        // Frames 3 and 6 report what frames 2 and 5 spent.
        assert_eq!(record.move_cpu_ms, [20.0, 50.0]);
        assert_eq!(record.cpu_ms, [10.0, 20.0, 30.0, 40.0, 50.0, 60.0]);
        assert_eq!(record.frames, 7);
    }

    /// A move whose CPU eframe does not report leaves a hole in the move frames, not a wrong sample:
    /// each frame's move is used once, by the frame after it.
    #[test]
    fn a_move_whose_cpu_is_not_reported_adds_no_sample_and_borrows_no_other_frames() {
        let mut record = Record::default();
        count_frames(
            &mut record,
            &[
                // A move, and eframe reports nothing for it on the next frame.
                seen(Some(1.0), true),
                seen(None, false),
                // The frame before this one carried none.
                seen(Some(2.0), true),
                seen(Some(3.0), true),
                // Two moves in a row, the CPU of the first not reported.
                seen(None, true),
                seen(Some(4.0), false),
                seen(Some(5.0), false),
            ],
        );
        assert_eq!(record.frames, 7);
        assert_eq!(record.move_cpu_ms, [3.0, 4.0]);
        assert_eq!(record.cpu_ms, [1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn only_a_pointer_move_in_the_drivers_events_makes_a_frame_carry_one() {
        let at = egui::pos2(10.0, 20.0);
        let moved = egui::Event::PointerMoved(at);
        let button = egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        };
        let key = egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };
        let text = egui::Event::Text("a".to_owned());
        for (events, carries) in [
            (vec![], false),
            (vec![button.clone()], false),
            (vec![key.clone()], false),
            (vec![text.clone()], false),
            (vec![button.clone(), key.clone(), text.clone()], false),
            (vec![moved.clone()], true),
            // The press frame holds the pointer's move and the press.
            (vec![moved.clone(), button], true),
            (vec![key, moved, text], true),
        ] {
            assert_eq!(holds_a_move(&events), carries, "{events:?}");
        }
    }

    #[test]
    fn the_drags_hand_makes_the_frames_it_moves_in_carry_a_move() {
        let (mut drag, begun) = (Drag::default(), Instant::now());
        let period = pointer_period();
        for drawn in 0..FRAMES_BEFORE_PRESS {
            let events = drag.events_at(drawn, begun);
            assert!(!holds_a_move(&events), "frame {drawn}, before the press");
        }
        // (frame, when it is drawn, whether it carries a move): the press brings the first move, a
        // frame between two moves brings nothing, and the one that reaches the next due time moves.
        let frames = [
            (FRAMES_BEFORE_PRESS, Duration::ZERO, true),
            (3, period / 2, false),
            (4, period - Duration::from_nanos(1), false),
            (5, period, true),
            (6, period + period / 3, false),
            (7, period * 2, true),
            (8, period * 2 + period / 2, false),
            (9, period * 3, true),
        ];
        for (drawn, at, carries) in frames {
            let events = drag.events_at(drawn, begun + at);
            assert_eq!(
                holds_a_move(&events),
                carries,
                "frame {drawn} at {at:?}: {events:?}"
            );
        }
    }

    #[test]
    fn the_rest_drags_stretches_and_releases_carry_a_move_and_its_rests_none() {
        let (mut hand, begun) = (RestDrag::default(), Instant::now());
        let period = pointer_period();
        for drawn in 0..FRAMES_BEFORE_PRESS {
            let events = hand.events_at(drawn, begun);
            assert!(!holds_a_move(&events), "frame {drawn}, before the press");
        }
        let rest = STRETCH;
        let quarter = REST / 4;
        let frames = [
            // A stretch: the press, then a move each period.
            ("the press", FRAMES_BEFORE_PRESS, Duration::ZERO, true),
            ("between two moves", 3, period / 2, false),
            ("a move", 4, period, true),
            // The rest's first frame moves to the Day end and lets go.
            ("the release", 99, rest, true),
            ("the rest", 99, rest + quarter, false),
            ("the rest, late", 99, rest + REST - quarter, false),
            // The next stretch presses again.
            ("the next press", 99, rest + REST, true),
            ("between two moves", 99, rest + REST + period / 2, false),
            ("a move", 99, rest + REST + period, true),
            ("the next release", 99, rest + REST + STRETCH, true),
            ("the rest", 99, rest + REST + STRETCH + quarter, false),
        ];
        for (what, drawn, at, carries) in frames {
            let events = hand.events_at(drawn, begun + at);
            assert_eq!(
                holds_a_move(&events),
                carries,
                "{what}, at {at:?}: {events:?}"
            );
        }
    }

    #[test]
    fn a_run_with_no_move_has_none_while_every_frames_cpu_stays() {
        // `Driven` puts nothing in the input of a scenario that moves nothing.
        let shared = Shared::new(Instant::now(), Scenario::OpenIdle, 5);
        for frame in 1..=6_u32 {
            shared
                .lock()
                .count_frame(seen(Some(f64::from(frame)), holds_a_move(&[])));
        }
        let report = shared.report(false);
        assert_eq!(report.move_frame_cpu_ms, None);
        assert_eq!(
            report.frame_cpu_ms,
            Spread::of(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])
        );
        let json = serde_json::to_value(&report).unwrap();
        // A key that is missing reads as null too: the report has to carry the key.
        assert!(keys(&json).contains("move_frame_cpu_ms"));
        assert!(json["move_frame_cpu_ms"].is_null());
        assert_eq!(json["frame_cpu_ms"]["samples"], 6);

        // A scenario that moves, whose frames the move did not reach, is no different.
        let drag = Shared::new(Instant::now(), Scenario::Drag, 5);
        count_frames(
            &mut drag.lock(),
            &[seen(None, false), seen(Some(0.5), false)],
        );
        assert_eq!(drag.report(false).move_frame_cpu_ms, None);
    }

    /// A frame eframe gives only its logic reports no CPU and leaves the last painted frame's in
    /// place, so the frame after it reads that value again (`Record::count_frame`). The frames, in
    /// the readings eframe gives: 1.0 is frame 1's CPU, 2.0 frame 2's, 6.0 frame 6's and 7.0
    /// frame 7's.
    #[test]
    fn a_covered_window_adds_no_stale_cpu_to_the_move_frames() {
        let mut record = Record::default();
        count_frames(
            &mut record,
            &[
                seen(None, false),
                // Painted, and carries a move.
                seen(Some(1.0), true),
                // The window is covered from here, with a move in this frame, which is not painted.
                // The reading is new, and frame 2's: it counts.
                covered(Some(2.0), true),
                // The readings that follow are frame 2's again: they count in `cpu_ms`, as they
                // did, and not in the move frames, whose frame before is not painted.
                covered(Some(2.0), true),
                covered(Some(2.0), false),
                // Uncovered. The frame has no move of its own, but its input holds the two that
                // frames 3 and 4 put in the input that eframe kept, so it carries a move. Its
                // reading is still frame 2's, since frame 5 was not painted.
                seen(Some(2.0), false),
                // Frame 6's CPU, new, and frame 6 carried a move.
                seen(Some(6.0), false),
                // Frame 7 carried none.
                seen(Some(7.0), false),
            ],
        );
        assert_eq!(record.frames, 8);
        assert_eq!(record.unseen_frames, 3);
        assert_eq!(record.move_cpu_ms, [2.0, 6.0]);
        assert_eq!(record.cpu_ms, [1.0, 2.0, 2.0, 2.0, 2.0, 6.0, 7.0]);
    }

    /// The input of a frame that was painted is gone: nothing waits for the next painted frame.
    #[test]
    fn a_move_that_was_painted_is_not_carried_again_by_the_frame_after_a_cover() {
        let mut record = Record::default();
        count_frames(
            &mut record,
            &[
                seen(None, false),
                seen(Some(1.0), true),
                // Covered with no move of its own: frame 2 was painted, and its CPU is new.
                covered(Some(2.0), false),
                covered(Some(2.0), false),
                // Uncovered: nothing was kept for it.
                seen(Some(2.0), false),
                seen(Some(5.0), false),
            ],
        );
        assert_eq!(record.move_cpu_ms, [2.0]);
        assert_eq!(record.cpu_ms, [1.0, 2.0, 2.0, 2.0, 5.0]);
    }

    /// A drag under a cover moves in every frame, and nothing is painted: the CPU the frames read
    /// is frame 1's again and again, and no move frame has a CPU of its own.
    #[test]
    fn a_drag_under_a_cover_has_no_move_frames_while_every_frames_cpu_stays() {
        let shared = Shared::new(Instant::now(), Scenario::Drag, 5);
        count_frames(
            &mut shared.lock(),
            &[
                seen(None, false),
                // Frame 1 was painted, and carried no move: the reading is its own.
                covered(Some(1.0), true),
                covered(Some(1.0), true),
                covered(Some(1.0), true),
                covered(Some(1.0), true),
            ],
        );
        let report = shared.report(false);
        assert_eq!((report.frames, report.unseen_frames), (5, 4));
        assert_eq!(report.move_frame_cpu_ms, None);
        assert_eq!(report.frame_cpu_ms, Spread::of(&[1.0, 1.0, 1.0, 1.0]));
    }

    // A fallback, read on `Shared` with no window: `record.opens`, `opened` and `reopening` are set by
    // hand, as `Driven::note_frame` and `Driven::note_reopening` set them (those need an eframe frame,
    // and `scripts/painter-fallback.sh` reads them on a real window).

    fn open_on(renderer: &'static str, frames: u64) -> Open {
        Open {
            to_first_frame_ms: Some(30.0),
            frames,
            renderer,
            tray: true,
            ..Open::default()
        }
    }

    /// `Shared` as `direct` has it for `open-close`.
    fn open_close() -> Shared {
        Shared::new(Instant::now(), Scenario::OpenClose, 9)
    }

    /// A moment `ago` before now.
    fn before(ago: Duration) -> Instant {
        Instant::now().checked_sub(ago).expect("an old instant")
    }

    fn failing(why: &str, at: Instant) -> Option<Reopening> {
        Some(Reopening {
            why: why.to_owned(),
            at,
        })
    }

    #[test]
    fn a_fallback_hands_cycle_zero_to_the_glow_window_and_moves_no_close() {
        let shared = open_close();
        let failed_at = Instant::now();
        let open_for = Duration::from_millis(60);
        {
            let mut record = shared.lock();
            record.requested = Some(before(Duration::from_secs(5)));
            record.opens.push(open_on("metal", 2));
            record.opened = Some(before(open_for));
            record.reopening = failing("ARRAKIS_METAL_FAIL=frame", failed_at);
        }
        shared.window_closed();
        {
            let record = shared.lock();
            assert_eq!(record.closes, 0, "a fallback is no close of `direct`'s");
            assert!(
                record.opens.is_empty(),
                "the failed window is out of opens, so cycle 0 waits for glow's"
            );
            let [fallback] = &record.fallbacks[..] else {
                panic!("one fallback, got {:?}", record.fallbacks);
            };
            assert_eq!(fallback.why, "ARRAKIS_METAL_FAIL=frame");
            assert_eq!(fallback.frames, 2, "the failed window's frames");
            assert_eq!(
                fallback.at_open, 0,
                "the index of the window that replaces it"
            );
            assert_eq!(
                record.requested,
                Some(failed_at),
                "the glow window reads its first frame from the failure"
            );
            assert!(
                record.window_open >= open_for,
                "the failed window's time counts: {:?}",
                record.window_open
            );
            assert!(record.opened.is_none());
            assert!(record.reopening.is_none(), "the failure is taken, once");
        }

        // The glow window's first frame, then its close, as `direct` asked for it.
        {
            let mut record = shared.lock();
            record.opens.push(open_on("glow", 1));
            record.opened = Some(Instant::now());
            assert_eq!(record.opens[0].renderer, "glow", "opens[cycle] is glow's");
        }
        shared.window_closed();
        let record = shared.lock();
        assert_eq!(record.closes, 1, "glow's close is cycle 0's");
        assert_eq!(record.opens.len(), 1);
        assert_eq!(record.fallbacks.len(), 1);
        assert!(record.opens[0].open_ms.is_some());
    }

    #[test]
    fn a_close_that_reopens_nothing_counts_in_closes_and_keeps_its_entry() {
        let shared = open_close();
        let open_for = Duration::from_millis(30);
        {
            let mut record = shared.lock();
            record.opens.push(open_on("glow", 12));
            record.opened = Some(before(open_for));
        }
        shared.window_closed();
        let record = shared.lock();
        assert_eq!(record.closes, 1);
        assert!(record.fallbacks.is_empty());
        let [open] = &record.opens[..] else {
            panic!("the window stays in opens, got {:?}", record.opens);
        };
        assert_eq!((open.renderer, open.frames), ("glow", 12));
        assert!(
            open.open_ms.is_some_and(|ms| ms >= 30.0),
            "{:?}",
            open.open_ms
        );
        assert!(record.window_open >= open_for);
    }

    #[test]
    fn a_second_windows_fallback_leaves_the_first_cycle_where_it_was() {
        let shared = open_close();
        // Cycle 0: a normal window, closed by `direct`.
        {
            let mut record = shared.lock();
            record.opens.push(open_on("metal", 40));
            record.opened = Some(Instant::now());
        }
        shared.window_closed();
        // Cycle 1: a window that fails on its third frame.
        let failed_at = Instant::now();
        {
            let mut record = shared.lock();
            record.opens.push(open_on("metal", 3));
            record.opened = Some(Instant::now());
            record.reopening = failing("a frame did not complete in time", failed_at);
        }
        shared.window_closed();
        {
            let record = shared.lock();
            assert_eq!(record.closes, 1, "only cycle 0 closed");
            let [first] = &record.opens[..] else {
                panic!("the failed window is out of opens, got {:?}", record.opens);
            };
            assert_eq!((first.renderer, first.frames), ("metal", 40));
            let [fallback] = &record.fallbacks[..] else {
                panic!("one fallback, got {:?}", record.fallbacks);
            };
            assert_eq!((fallback.frames, fallback.at_open), (3, 1));
            assert_eq!(record.requested, Some(failed_at));
        }
        shared.lock().opens.push(open_on("glow", 1));
        assert_eq!(
            shared.lock().opens[1].renderer,
            "glow",
            "opens[cycle] is glow's"
        );
    }

    #[test]
    fn a_window_that_never_drew_a_frame_still_falls_back_with_no_frames() {
        // A failure in the painter's start comes before the window's first frame is noted: there is
        // no entry in `opens` to move.
        let shared = open_close();
        shared.lock().reopening = failing("no device", Instant::now());
        shared.window_closed();
        let record = shared.lock();
        let [fallback] = &record.fallbacks[..] else {
            panic!("one fallback, got {:?}", record.fallbacks);
        };
        assert_eq!((fallback.frames, fallback.at_open), (0, 0));
        assert_eq!(record.closes, 0);
    }

    /// `opens[0]` after a first-window fallback is glow's, timed from the failure and not from `main`.
    #[test]
    fn a_first_window_that_failed_leaves_no_time_from_main_and_a_later_one_keeps_it() {
        let open_idle = || Shared::new(Instant::now(), Scenario::OpenIdle, 9);
        let fails_now = |shared: &Shared, frames: u64| {
            {
                let mut record = shared.lock();
                record.opens.push(open_on("metal", frames));
                record.opened = Some(Instant::now());
                record.reopening = failing("ARRAKIS_METAL_FAIL=frame", Instant::now());
            }
            shared.window_closed();
            shared.lock().opens.push(open_on("glow", 9));
        };

        let first = open_idle();
        fails_now(&first, 1);
        assert_eq!(first.lock().fallbacks[0].at_open, 0);
        let report = first.report(false);
        assert_eq!(report.opens[0].to_first_frame_ms, Some(30.0));
        assert_eq!(report.main_to_first_frame_ms, None);
        // The key the keys test pins stays, and says nothing.
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["main_to_first_frame_ms"], serde_json::Value::Null);
        assert!(
            json.as_object()
                .unwrap()
                .contains_key("main_to_first_frame_ms")
        );

        let later = open_idle();
        later.lock().opens.push(open_on("metal", 12));
        fails_now(&later, 2);
        assert_eq!(later.lock().fallbacks[0].at_open, 1);
        assert_eq!(later.report(false).main_to_first_frame_ms, Some(30.0));
    }

    /// What the script subtracts to read the engine after a fallback. The counts are the process's,
    /// and another test's `NullScreen` may check at any moment, so these are bounds.
    #[test]
    fn a_fallback_holds_the_checks_so_far_and_the_report_counts_those_after_it() {
        use crate::engine::Screen;

        let shared = open_close();
        NullScreen.intact();
        let before = app::screen_counts::read().checks;
        {
            let mut record = shared.lock();
            record.opens.push(open_on("metal", 1));
            record.opened = Some(Instant::now());
            record.reopening = failing("ARRAKIS_METAL_FAIL=frame", Instant::now());
        }
        shared.window_closed();
        let taken = shared.lock().fallbacks[0].screen_checks;
        assert!(taken >= before, "{taken} of the {before} checks so far");
        NullScreen.intact();
        let report = shared.report(false);
        assert!(report.screen.checks > taken, "{:?}", report.screen);
    }

    #[test]
    fn the_report_carries_the_fallbacks() {
        let shared = open_close();
        {
            let mut record = shared.lock();
            record.opens.push(open_on("metal", 5));
            record.opened = Some(Instant::now());
            record.reopening = failing("ARRAKIS_METAL_FAIL=surface", Instant::now());
        }
        shared.window_closed();
        shared.lock().opens.push(open_on("glow", 9));
        let report = shared.report(false);
        let [fallback] = &report.fallbacks[..] else {
            panic!("one fallback, got {:?}", report.fallbacks);
        };
        assert_eq!(fallback.why, "ARRAKIS_METAL_FAIL=surface");
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["fallbacks"][0]["why"], "ARRAKIS_METAL_FAIL=surface");
        assert_eq!(json["fallbacks"][0]["frames"], 5);
        assert_eq!(json["fallbacks"][0]["at_open"], 0);
        assert_eq!(json["opens"][0]["renderer"], "glow");
        // Present and null: glow's shader is its own.
        assert!(keys(&json["opens"][0]).contains("shader"));
        assert!(json["opens"][0]["shader"].is_null());
        assert_eq!(json["opens"][0]["tray"], true);

        let calm = open_close().report(false);
        assert!(calm.fallbacks.is_empty());
        assert_eq!(
            serde_json::to_value(&calm).unwrap()["fallbacks"],
            serde_json::json!([])
        );
    }

    #[test]
    fn the_holds_make_time_is_null_with_no_make_and_a_number_with_one() {
        let made = serde_json::to_value(measured()).unwrap();
        assert_eq!(made["hold"]["link_make_ms"], 0.25);
        assert_eq!(made["hold"]["ticks"], 3_600);
        assert_eq!(made["hold"]["starts"], 1);
        assert_eq!(made["hold"]["waits"], 3_500);
        assert_eq!(made["hold"]["timeouts"], 0);
        // No window ran on the Metal painter: zeros for the counts, and no make to time.
        let none = serde_json::to_value(unmeasured()).unwrap();
        // Present and null, as indexing a missing key would also read as null.
        assert_eq!(
            keys(&none["hold"]),
            names([
                "ticks",
                "starts",
                "waits",
                "timeouts",
                "link_make_ms",
                "spare_drops",
            ])
        );
        assert!(none["hold"]["link_make_ms"].is_null());
        for key in ["ticks", "starts", "waits", "timeouts", "spare_drops"] {
            assert_eq!(none["hold"][key], 0, "{key}");
        }
    }

    /// The report's `spare_drops` and `threads_after_close`, with the values `measured` gives them: the scripts
    /// read `spare_drops` as the proof that the pause's block arrives, and `threads_after_close` against
    /// a glow close's.
    #[test]
    fn the_spare_drops_and_the_threads_after_a_close_reach_the_json_with_their_figures() {
        let json = serde_json::to_value(measured()).unwrap();
        assert_eq!(json["hold"]["spare_drops"], 2);
        let opens = json["opens"].as_array().expect("a list");
        assert_eq!(opens.len(), 2);
        for open in opens {
            assert_eq!(open["threads_after_close"], 9);
        }

        // A close whose threads could not be counted: the key is there, and null.
        let mut report = measured();
        report.opens[0].threads_after_close = None;
        let json = serde_json::to_value(report).unwrap();
        assert!(keys(&json["opens"][0]).contains("threads_after_close"));
        assert!(json["opens"][0]["threads_after_close"].is_null());
        assert_eq!(
            json["opens"][1]["threads_after_close"], 9,
            "the other stays"
        );
    }

    /// Other tests start and end threads beside this one, so a count read before and one read during
    /// cannot be compared. What can be: this test's own threads, all alive and parked at the count
    /// (a barrier says so), and the one that runs it. A count that read the wrong field, or a constant,
    /// is below that.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_thread_count_includes_the_threads_this_test_holds_parked() {
        use std::sync::Barrier;

        const HELD: u32 = 16;
        let alone = threads().expect("a count of this process's threads");
        assert!(alone >= 1);
        let parked = Barrier::new(HELD as usize + 1);
        let released = Barrier::new(HELD as usize + 1);
        let counted = std::thread::scope(|scope| {
            for _ in 0..HELD {
                scope.spawn(|| {
                    parked.wait();
                    released.wait();
                });
            }
            parked.wait();
            let counted = threads();
            released.wait();
            counted
        })
        .expect("a count with the threads parked");
        assert!(
            counted > HELD,
            "{counted} threads, with {HELD} of this test's parked"
        );
    }

    /// Each counter in its own field, so a swap in `HoldCounts::read` shows: the check that a drag
    /// has no timeouts reads `timeouts` from here.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_holds_stats_reach_the_report_field_by_field() {
        let counts = HoldCounts::from(eframe::egui_wgpu::hold::Stats {
            ticks: 11,
            starts: 22,
            waits: 33,
            timeouts: 44,
            longest_make: Some(Duration::from_micros(1_250)),
            spare_drops: 55,
        });
        assert_eq!(
            counts,
            HoldCounts {
                ticks: 11,
                starts: 22,
                waits: 33,
                timeouts: 44,
                link_make_ms: Some(1.25),
                spare_drops: 55,
            }
        );
        let none = HoldCounts::from(eframe::egui_wgpu::hold::Stats::default());
        assert_eq!(none.link_make_ms, None);
    }

    /// No test in this binary drives a painter, so the process's clock has counted nothing.
    #[test]
    fn a_process_that_ran_no_window_on_the_painter_reads_zeros_and_no_make_time() {
        assert_eq!(HoldCounts::read(), HoldCounts::default());
        assert_eq!(HoldCounts::read().link_make_ms, None);
    }

    /// The value of `key` in a summary `line` of `key=value` pairs.
    fn value<'a>(line: &'a str, key: &str) -> &'a str {
        line.split(' ')
            .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
            .unwrap_or_else(|| panic!("no {key} in {line}"))
    }

    #[test]
    fn the_summary_is_one_line_of_pairs_for_the_script() {
        // A drag has every pair, the move frames' too.
        let line = dragged().summary();
        assert!(!line.contains('\n'));
        // A pair is `key=value`, and a value has no space in it.
        let pairs: Vec<(&str, &str)> = line
            .split(' ')
            .map(|pair| pair.split_once('=').unwrap_or_else(|| panic!("{pair:?}")))
            .collect();
        let keys: BTreeSet<&str> = pairs.iter().map(|(key, _)| *key).collect();
        assert_eq!(keys.len(), pairs.len(), "a key twice in {line}");
        assert_eq!(value(&line, "scenario"), "drag");
        assert_eq!(value(&line, "finished"), "true");
        for (key, value) in &pairs {
            assert!(!value.is_empty(), "{key} is empty");
            if !matches!(*key, "scenario" | "finished" | "spoiled") {
                assert!(value.parse::<f64>().is_ok(), "{key}={value} is no number");
            }
        }
        // `spoiled` stands third, after `scenario` and `finished`.
        assert_eq!(line.split(' ').nth(2), Some("spoiled=-"));
        // 31.5 s of a 30 s `open-close`, which takes 30 to 36.5 s, is in its time.
        let in_time = measured().summary();
        assert_eq!(value(&in_time, "spoiled"), "-");
        // The same report at 40 s is past its 36.5: the pair has no space in it, so the line
        // keeps its pairs.
        let late = Report {
            elapsed_s: 40.0,
            spoiled: Spoiled::of(Scenario::OpenClose, 30, 40.0),
            ..measured()
        }
        .summary();
        assert_eq!(
            value(&late, "spoiled"),
            "elapsed_s:40.000,bound_s:30.000..36.500"
        );
        assert_eq!(late.split(' ').count(), in_time.split(' ').count());
        // The eighth cycle of ten broke off at that 31.5 s, inside the bound: the third form.
        let broke = Report {
            spoiled: Some(Spoiled::Cycles(BrokeOff {
                done: 7,
                planned: 10,
                stuck: Stuck::Open,
            })),
            ..measured()
        }
        .summary();
        assert_eq!(value(&broke, "spoiled"), "cycles:7/10");
        assert_eq!(broke.split(' ').count(), in_time.split(' ').count());
        assert_eq!(value(&line, "frames"), "120");
        assert_eq!(value(&line, "cover_builds"), "10");
        assert_eq!(value(&line, "headless"), "23");
        assert_eq!(value(&line, "unseen"), "4");
        // The nearest-rank percentiles of the move frames' [2.0, 3.0].
        assert_eq!(value(&line, "move_cpu_p50_ms"), "2.000");
        assert_eq!(value(&line, "move_cpu_p90_ms"), "3.000");
        assert_eq!(value(&line, "move_cpu_p99_ms"), "3.000");
        assert_eq!(value(&line, "move_cpu_max_ms"), "3.000");
        assert_eq!(value(&line, "footprint_first_close"), "73000000");
        assert_eq!(value(&line, "footprint_last_close"), "99700000");
    }

    #[test]
    fn the_summary_writes_a_dash_for_what_was_not_measured() {
        let line = unmeasured().summary();
        for key in [
            "fps",
            "cpu_p50_ms",
            "cpu_p90_ms",
            "cpu_p99_ms",
            "cpu_max_ms",
            "move_cpu_p50_ms",
            "move_cpu_p90_ms",
            "move_cpu_p99_ms",
            "move_cpu_max_ms",
            "cover_bytes_per_frame",
            "main_to_first_frame_ms",
            "first_open_ms",
            "footprint_first_close",
            "footprint_last_close",
        ] {
            assert_eq!(value(&line, key), "-", "{key} in {line}");
        }
        assert_eq!(value(&line, "scenario"), "closed-day");
        assert_eq!(value(&line, "finished"), "false");
        assert_eq!(value(&line, "frames"), "0");
    }

    /// The two spreads have their own pairs: `dragged()`'s two spreads share their p50 and p90, so
    /// this report tells them apart.
    #[test]
    fn the_summary_reads_the_move_pairs_from_the_move_spread_and_the_cpu_pairs_from_the_frames() {
        let moved = Report {
            move_frame_cpu_ms: Spread::of(&[5.0, 7.0, 11.0]),
            ..dragged()
        }
        .summary();
        for (key, shown) in [
            ("move_cpu_p50_ms", "7.000"),
            ("move_cpu_p90_ms", "11.000"),
            ("move_cpu_p99_ms", "11.000"),
            ("move_cpu_max_ms", "11.000"),
            ("cpu_p50_ms", "2.000"),
            ("cpu_p90_ms", "3.000"),
            ("cpu_p99_ms", "3.000"),
            ("cpu_max_ms", "3.000"),
        ] {
            assert_eq!(value(&moved, key), shown, "{key} in {moved}");
        }

        // Frames measured, none of them with a move (as in `open-close`): the move pairs are not
        // measured, the cpu pairs are.
        let none = measured().summary();
        for key in [
            "move_cpu_p50_ms",
            "move_cpu_p90_ms",
            "move_cpu_p99_ms",
            "move_cpu_max_ms",
        ] {
            assert_eq!(value(&none, key), "-", "{key} in {none}");
        }
        assert_eq!(value(&none, "cpu_p50_ms"), "2.000");
        assert_eq!(value(&none, "cpu_max_ms"), "3.000");
    }

    #[test]
    fn the_summary_names_each_scenario_by_its_flag() {
        for scenario in Scenario::value_variants() {
            let report = Report {
                scenario: *scenario,
                ..unmeasured()
            };
            let first = report.summary();
            let first = first.split(' ').next().expect("a first pair");
            assert_eq!(first, format!("scenario={}", flag(*scenario)));
        }
    }

    /// A report file's path in the temp folder, removed when it goes out of
    /// scope.
    struct ReportFile(PathBuf);

    impl ReportFile {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("arrakis-drive-{}-{name}.json", std::process::id()));
            let _ = std::fs::remove_file(&path);
            Self(path)
        }
    }

    impl Drop for ReportFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn the_first_caller_of_finish_is_the_only_writer() {
        let file = ReportFile::new("finish-once");
        let shared = Shared::new(Instant::now(), Scenario::ClosedDay, 5);
        assert!(
            finish(&shared, true, Some(&file.0)),
            "the first call writes"
        );
        let written = std::fs::read_to_string(&file.0).expect("the report");
        let json: serde_json::Value = serde_json::from_str(&written).expect("whole JSON");
        assert_eq!(json["finished"], true);

        // The file is put back to something else; a second call, which asks
        // for another outcome, must leave it alone.
        std::fs::write(&file.0, "kept").expect("a file");
        assert!(
            !finish(&shared, false, Some(&file.0)),
            "the second does not"
        );
        assert_eq!(std::fs::read_to_string(&file.0).expect("the file"), "kept");
    }

    #[test]
    fn callers_of_finish_that_come_together_write_once() {
        // The main thread when the app quits and the driver's thread when it
        // did not, in the same moment: checking `finished` and then calling
        // `finish` let both write.
        for round in 0..25 {
            let file = ReportFile::new(&format!("finish-race-{round}"));
            let shared = Arc::new(Shared::new(Instant::now(), Scenario::ClosedDay, 5));
            let start = Arc::new(std::sync::Barrier::new(4));
            let callers: Vec<_> = [true, false, true, false]
                .into_iter()
                .map(|finished| {
                    let (shared, start, path) = (shared.clone(), start.clone(), file.0.clone());
                    std::thread::spawn(move || {
                        start.wait();
                        finish(&shared, finished, Some(&path))
                    })
                })
                .collect();
            let wrote = callers
                .into_iter()
                .map(|caller| caller.join().expect("a caller"))
                .filter(|&wrote| wrote)
                .count();
            assert_eq!(wrote, 1, "round {round}");
            // The report is whole once every caller is back: a caller that
            // did not write waited for the one that did.
            let written = std::fs::read_to_string(&file.0).expect("the report");
            serde_json::from_str::<serde_json::Value>(&written).expect("whole JSON");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_footprint_is_the_processs_own_and_more_than_a_megabyte() {
        let footprint = footprint().expect("proc_pid_rusage answers for this process");
        assert!(footprint > 1 << 20, "{footprint} bytes");
    }
}
