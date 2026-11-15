//! The tint engine: one thread that owns the displays' color.
//!
//! It works out from the schedule how red the screen should be, moves the
//! displays there smoothly, and keeps checking that the tint is still in
//! place, because macOS puts the normal colors back after sleep, display
//! changes and screen locks. The window and the menu-bar item only send it
//! [`Command`]s; it reports back with [`Status`].
//!
//! There is no network or file work here, so a plain thread waiting on a
//! channel with a timeout does the job; an async runtime would add threads
//! and nothing else.
//!
//! It wakes as little as it can. macOS's notifications of a wake, a user
//! switch, an app quitting or a display change send [`Command::Refresh`],
//! which checks the displays at once and again [`RECHECK`] later. The check
//! it runs on its own is only a safety net for resets that no notification
//! announces, and it backs off ([`Checks`]). It tells its wake which
//! statuses a closed window needs ([`Pacer`]). A scheduled fade wakes it
//! only when the displays' tables would change
//! ([`Worker::next_table_step`]), and on macOS the thread runs at utility
//! QoS, whose timers macOS may run late and group with other wakeups.

use std::sync::{Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use jiff::tz::TimeZone;

use crate::color::{Look, TABLE_STEP, Tint};
use crate::settings::Settings;

/// The displays the engine paints. The real one is in `crate::gamma`;
/// tests use a fake.
pub trait Screen: Send {
    /// Puts `tint` on every display. Returns how many displays have it.
    fn apply(&mut self, tint: Tint) -> Result<usize, String>;
    /// Whether every display still shows what `apply` last put there.
    /// Answering `false` when unsure only costs a repaint of the table.
    fn intact(&mut self) -> bool;
    /// Gives every display its normal colors back.
    fn restore(&mut self);
    /// How many displays are connected.
    fn display_count(&mut self) -> usize;
}

#[derive(Clone, Debug)]
pub enum Command {
    Configure(Settings),
    /// Forces the full night look now, whatever the schedule says.
    RedNow(bool),
    /// Shows a look at once while a slider is held; `None` lets go.
    Preview(Option<Look>),
    /// Read the clock again, and check the displays now and [`RECHECK`]
    /// later, repainting a reset table: macOS woke, switched back to this
    /// user, saw an app quit, changed the clock or reconfigured a display.
    Refresh,
    /// Restore normal colors and stop.
    Shutdown,
}

/// What the displays show now, for the status line and the menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Status {
    /// How far into the night look the screen is, 0 to 1.
    pub level: f32,
    pub tint: Tint,
    pub displays: usize,
    /// The last error from the displays, if painting failed.
    pub failed: bool,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            level: 0.0,
            tint: Tint::NEUTRAL,
            displays: 0,
            failed: false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    Status(Status),
    Error(String),
}

/// Whom an event is for: what the engine tells its wake with each one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// The app, its window open or not: an open window draws it at once, and
    /// a closed app's headless wait ends (paced, `idle::wake_paced`).
    App,
    /// An open window only, which draws it at once. A closed app sleeps on
    /// and reads it at its next tick: the event waits in the channel.
    Window,
}

/// How soon after a paint or a refresh a tinted screen is checked for a
/// reset.
const CHECK_FIRST: Duration = Duration::from_secs(2);
/// The longest wait between two checks of a screen that keeps reading
/// intact. A reset that no notification announces then shows normal colors
/// for this long at worst: the safe direction, never a tint stuck on.
const CHECK_LONGEST: Duration = Duration::from_secs(30);
/// A refresh checks at once and again this long after, because WindowServer
/// may reset the table just after the notification.
const RECHECK: Duration = Duration::from_secs(1);
/// While no window is open, the shortest time between two wakes of the app
/// for statuses that only move the level ([`Pacer`]).
pub const LEVEL_PACE: Duration = Duration::from_secs(5);
/// Frame time for quick transitions, such as Red now.
const ANIMATION_FRAME: Duration = Duration::from_millis(33);
/// How long a quick transition takes from day to full red.
const QUICK_FADE: Duration = Duration::from_millis(1200);
/// While a scheduled fade runs, the shortest wait for its next step. A step
/// is painted once the tint has moved a table step ([`TABLE_STEP`]) from
/// the one shown. Where the curve is fast, the tint moves several table
/// steps a second: painting each would paint more often than a step a
/// second did, so the engine waits at least this long and paints them as
/// one.
const FADE_STEP: Duration = Duration::from_secs(1);
/// How closely [`Worker::next_table_step`] finds when a fade's tint next
/// moves a table step, or when the fade ends.
const STEP_PRECISION: Duration = Duration::from_millis(1);
/// The longest sleep. The thread's clock stops while the Mac sleeps; macOS's
/// wake notification, and its notice of a new clock or time zone, send a
/// [`Command::Refresh`], which reads the clock again. So this only bounds
/// the error of a clock that drifts from the thread's.
const MAX_SLEEP: Duration = Duration::from_secs(60);

/// When a tinted screen is next checked for a reset: the safety net under
/// macOS's notifications ([`Command::Refresh`]).
///
/// The first check runs [`CHECK_FIRST`] after a paint or a refresh. Each one
/// that reads the tables intact doubles the wait before the next, up to
/// [`CHECK_LONGEST`]: 2, 4, 8, 16, 30, 30 s. A paint or a refresh starts it
/// over, and a check that finds a foreign table repaints, which starts it
/// over too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Checks {
    /// When the backoff's next check is due.
    due: Instant,
    /// The wait before `due`, which the check at `due` doubles if it reads
    /// the tables intact.
    interval: Duration,
    /// A refresh's second check, [`RECHECK`] after its first.
    again: Option<Instant>,
}

impl Checks {
    fn new(now: Instant) -> Self {
        Self {
            due: now + CHECK_FIRST,
            interval: CHECK_FIRST,
            again: None,
        }
    }

    /// The displays took a tint at `now`: the next check is [`CHECK_FIRST`]
    /// away. A refresh's second check still to come stays; one already
    /// past is dropped, the paint being newer.
    fn painted(&mut self, now: Instant) {
        self.interval = CHECK_FIRST;
        self.due = now + CHECK_FIRST;
        self.again = self.again.filter(|again| *again > now);
    }

    /// A refresh at `now`, whose first check the caller runs at once: the
    /// second follows [`RECHECK`] later, and the backoff starts over.
    fn refreshed(&mut self, now: Instant) {
        self.interval = CHECK_FIRST;
        self.due = now + CHECK_FIRST;
        self.again = Some(now + RECHECK);
    }

    /// Whether a check is due at `now`: the backoff's, or a refresh's
    /// second.
    fn is_due(&self, now: Instant) -> bool {
        now >= self.due || self.again.is_some_and(|again| now >= again)
    }

    /// A check at `now` read the tables intact. Only the backoff's own
    /// check, the one at `due`, doubles the wait. A refresh's checks are
    /// plain looks: they move neither `due` nor the interval.
    fn intact(&mut self, now: Instant) {
        if self.again.is_some_and(|again| now >= again) {
            self.again = None;
        }
        if now >= self.due {
            self.interval = (self.interval * 2).min(CHECK_LONGEST);
            self.due = now + self.interval;
        }
    }

    /// How long from `now` until the next check.
    fn wait(&self, now: Instant) -> Duration {
        let next = self.again.map_or(self.due, |again| again.min(self.due));
        next.saturating_duration_since(now)
    }
}

/// Decides whom each of the engine's events reaches ([`Reach`]).
///
/// With the window closed nothing reads the engine's status: the menu bar
/// shows the clock and the settings (`App::status_line`), and the window
/// reads every queued event when it opens. So only what a closed app might
/// act on wakes it at once:
/// - the tint coming or going, which the schedule's turn and Red Now make
///   (Red Now has no field of its own: switched on by day its first step
///   tints the screen, and switched off its last step clears it);
/// - painting failing or recovering, and an error.
///
/// A status that only moves the level wakes the app when [`LEVEL_PACE`] has
/// passed since the last wake this sent, and otherwise reaches an open
/// window alone. A fade then ends one headless wait in 5 s, not one a
/// second.
#[derive(Debug, Default)]
pub(crate) struct Pacer {
    /// The status sent last, to tell what the next one changes.
    last_status: Option<Status>,
    /// When the app was last woken ([`Reach::App`]).
    last_wake: Option<Instant>,
}

impl Pacer {
    /// Whom `event`, sent at `now`, reaches.
    pub(crate) fn reach(&mut self, event: &Event, now: Instant) -> Reach {
        let matters = match event {
            Event::Error(_) => true,
            Event::Status(status) => {
                let before = self.last_status.replace(*status);
                before.is_none_or(|before| {
                    before.tint.is_neutral() != status.tint.is_neutral()
                        || before.failed != status.failed
                })
            }
        };
        let paced = self
            .last_wake
            .is_none_or(|last| now.saturating_duration_since(last) >= LEVEL_PACE);
        if matters || paced {
            self.last_wake = Some(now);
            Reach::App
        } else {
            Reach::Window
        }
    }
}

/// The interface's handle to the engine thread.
pub struct Engine {
    commands: mpsc::Sender<Command>,
    events: mpsc::Receiver<Event>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// What a test runs the moment a poll has taken its events
    /// ([`Engine::detached_with`]).
    #[cfg(test)]
    after_poll: Option<AfterPoll>,
}

/// A test's stand-in for the engine thread: a closure that runs after each
/// poll, with the channel the engine's events come through.
#[cfg(test)]
struct AfterPoll {
    events: Events,
    run: Box<dyn Fn(&Events) + Send>,
}

/// The channel that the engine's events come through.
#[cfg(test)]
type Events = mpsc::Sender<Event>;

impl Engine {
    /// Starts the engine on `screen`. `wake` is called after each event is
    /// sent, with whom it reaches ([`Pacer`]).
    pub fn spawn(
        screen: Box<dyn Screen>,
        settings: Settings,
        clock: fn() -> f64,
        wake: impl Fn(Reach) + Send + 'static,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("sunset-engine".into())
            .spawn(move || {
                let mut worker = Worker::new(screen, settings, clock);
                let mut pacer = Pacer::default();
                worker.run(&command_rx, |event| {
                    let reach = pacer.reach(&event, Instant::now());
                    let _ = event_tx.send(event);
                    wake(reach);
                });
            })
            .map_err(|error| log::error!("unable to start the tint engine: {error}"))
            .ok();
        Self {
            commands: command_tx,
            events: event_rx,
            thread,
            #[cfg(test)]
            after_poll: None,
        }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn poll(&self) -> Vec<Event> {
        let events = self.events.try_iter().collect();
        #[cfg(test)]
        if let Some(after) = &self.after_poll {
            (after.run)(&after.events);
        }
        events
    }

    /// Restores normal colors and waits for the thread to finish.
    pub fn shutdown(&mut self) {
        self.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Moves the calling thread, the engine's, to user-initiated QoS
/// (`user_initiated`), and to utility QoS otherwise.
///
/// At utility, macOS gives the thread's timers more leeway, groups its
/// wakeups with others, and runs it on the efficiency cores first. A slow
/// fade's step that lands 100 ms late is invisible. But a quick fade's
/// 33 ms frames came 34 to 108 ms apart there, and 42 to 152 ms apart on a
/// busy Mac. So what the user starts, watches or waits for
/// runs at user-initiated QoS, whose timers keep time: a quick fade's
/// frames, a preview's paints while the user drags, and the restore at the
/// end, which the app waits for in `join`.
#[cfg(target_os = "macos")]
fn set_qos(user_initiated: bool) {
    use libc::qos_class_t::{QOS_CLASS_USER_INITIATED, QOS_CLASS_UTILITY};
    let (class, name) = if user_initiated {
        (QOS_CLASS_USER_INITIATED, "user-initiated")
    } else {
        (QOS_CLASS_UTILITY, "utility")
    };
    // SAFETY: the call takes two plain values, a class from libc's own enum
    // and a relative priority of 0, within the -15..=0 it accepts. It
    // changes only the calling thread's scheduling class, and reads or
    // writes no memory of ours. A thread std spawned is a plain pthread,
    // which may set its own class, up or down.
    let error = unsafe { libc::pthread_set_qos_class_self_np(class, 0) };
    if error != 0 {
        // The thread keeps the class it had, which costs only energy, or a
        // quick fade's smoothness.
        log::warn!("unable to run the tint engine at {name} QoS: error {error}");
    }
}

#[cfg(not(target_os = "macos"))]
fn set_qos(_user_initiated: bool) {}

#[cfg(test)]
impl Engine {
    /// An engine with no thread, and the commands it is sent, for a test to
    /// read: what the app asks of the engine, with no timing in it.
    pub(crate) fn detached() -> (Self, mpsc::Receiver<Command>) {
        let (commands, sent) = mpsc::channel();
        let (_, events) = mpsc::channel();
        let engine = Self {
            commands,
            events,
            thread: None,
            after_poll: None,
        };
        (engine, sent)
    }

    /// [`detached`](Self::detached), with `after_poll` run at the end of each
    /// [`poll`](Self::poll), given the channel the events come through. It
    /// stands for the engine thread acting the moment after a pass took its
    /// events: it sends a status there and calls the engine's wake, which is
    /// the one step of the race that a test cannot otherwise place.
    pub(crate) fn detached_with(
        after_poll: impl Fn(&Events) + Send + 'static,
    ) -> (Self, mpsc::Receiver<Command>) {
        let (commands, sent) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        let engine = Self {
            commands,
            events,
            thread: None,
            after_poll: Some(AfterPoll {
                events: events_tx,
                run: Box::new(after_poll),
            }),
        };
        (engine, sent)
    }
}

/// Local time as fractional minutes since midnight, to the nanosecond.
///
/// The time zone is read once and kept ([`ZONE`]): the engine reads the
/// clock up to 30 times a second in a quick fade, and reading the zone reads
/// files.
pub fn local_minute_of_day() -> f64 {
    minute_of_day(jiff::Timestamp::now(), &ZONE.get(Instant::now()))
}

/// macOS moved the system's time zone (`gamma::watch_system`): the next read
/// of the clock reads the zone again.
pub fn time_zone_changed() {
    ZONE.forget();
}

/// Whether the clock keeps a zone it read: for `tests/observers.rs`, which
/// checks that macOS's notice of a new zone drops it.
#[doc(hidden)]
pub fn zone_is_kept() -> bool {
    ZONE.is_kept()
}

/// `now`'s time of day in `zone`, in fractional minutes since midnight, down
/// to the nanosecond: the window waits for the turn of the minute by this
/// clock (`app::until_next_minute`), and a clock cut to whole seconds would
/// land that pass up to a second late.
fn minute_of_day(now: jiff::Timestamp, zone: &TimeZone) -> f64 {
    let time = zone.to_datetime(now);
    let seconds = f64::from(time.second()) + f64::from(time.subsec_nanosecond()) / 1e9;
    f64::from(time.hour()) * 60.0 + f64::from(time.minute()) + seconds / 60.0
}

/// How long a zone is kept with no notice from macOS: as long as jiff keeps
/// its own, in case a change ever comes without one.
const ZONE_KEPT: Duration = Duration::from_secs(5 * 60);

/// The local time zone that the clock reads.
static ZONE: Zone = Zone::new(system_zone);

/// A time zone read once and kept, until [`Zone::forget`] (macOS's notice of
/// a new zone) or for [`ZONE_KEPT`].
struct Zone {
    /// The zone, and when it was read.
    kept: Mutex<Option<(TimeZone, Instant)>>,
    read: fn() -> TimeZone,
}

impl Zone {
    const fn new(read: fn() -> TimeZone) -> Self {
        Self {
            kept: Mutex::new(None),
            read,
        }
    }

    /// The zone kept, or, if none is or it is [`ZONE_KEPT`] old at `now`, a
    /// new read of it.
    fn get(&self, now: Instant) -> TimeZone {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((zone, read_at)) = &*kept
            && now.saturating_duration_since(*read_at) < ZONE_KEPT
        {
            return zone.clone();
        }
        let zone = (self.read)();
        *kept = Some((zone.clone(), now));
        zone
    }

    fn forget(&self) {
        *self.kept.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn is_kept(&self) -> bool {
        self.kept
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }
}

/// The system's time zone, read now.
///
/// jiff's own `TimeZone::system()` keeps the zone it read for five minutes,
/// with no way to drop it, so a read just after macOS's notice could still
/// get the old zone. This reads `/etc/localtime` as jiff does: the IANA name
/// its link points to, looked up in jiff's time zone database. jiff's read
/// covers the rest: a `TZ` variable, or a link it cannot name.
fn system_zone() -> TimeZone {
    linked_zone().unwrap_or_else(TimeZone::system)
}

/// The zone that `/etc/localtime` links to, unless `TZ` overrides it.
#[cfg(unix)]
fn linked_zone() -> Option<TimeZone> {
    if std::env::var_os("TZ").is_some() {
        return None;
    }
    let target = std::fs::read_link("/etc/localtime").ok()?;
    let (_, name) = target.to_str()?.rsplit_once("zoneinfo/")?;
    jiff::tz::db().get(name).ok()
}

#[cfg(not(unix))]
fn linked_zone() -> Option<TimeZone> {
    None
}

/// What a tick aims the displays at ([`Worker::aim`]): a preview, or the
/// level the switches and the schedule ask for. Commands that change it
/// start a quick fade ([`Worker::fade_time`]).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Aim {
    /// The clock's minute of the day, when the schedule decides the level.
    minute: Option<f64>,
    preview: Option<Look>,
    level: f32,
}

pub(crate) struct Worker {
    screen: Box<dyn Screen>,
    settings: Settings,
    red_now: bool,
    preview: Option<Look>,
    clock: fn() -> f64,
    /// How far into the night look the engine has moved: what a quick fade
    /// steps from. A scheduled fade's step that would not change the tables
    /// moves it and not [`shown_level`](Self::shown_level).
    level: f32,
    /// Whether the last tick was a step of a quick fade, so that this one
    /// lands it, painted whatever it changes.
    animating: bool,
    /// What the last tick aimed at; none before the first.
    aimed: Option<Aim>,
    /// The tint on the displays now.
    shown: Tint,
    /// The level of [`shown`](Self::shown), which a status carries.
    shown_level: f32,
    /// When the displays took a new tint last, from which a scheduled
    /// fade's next step waits at least [`FADE_STEP`].
    shown_since: Option<Instant>,
    /// When the tinted screen is next checked for a reset.
    checks: Checks,
    /// A [`Command::Refresh`] that the next tick answers.
    refresh: bool,
    last_status: Option<Status>,
    failed: bool,
}

impl Worker {
    pub(crate) fn new(screen: Box<dyn Screen>, settings: Settings, clock: fn() -> f64) -> Self {
        Self {
            screen,
            settings,
            red_now: false,
            preview: None,
            clock,
            level: 0.0,
            animating: false,
            aimed: None,
            shown: Tint::NEUTRAL,
            shown_level: 0.0,
            shown_since: None,
            checks: Checks::new(Instant::now()),
            refresh: false,
            last_status: None,
            failed: false,
        }
    }

    fn run(&mut self, commands: &mpsc::Receiver<Command>, mut emit: impl FnMut(Event)) {
        set_qos(false);
        let mut user_initiated = false;
        let mut last_tick = Instant::now();
        let mut sleep = Duration::ZERO;
        'run: loop {
            match commands.recv_timeout(sleep) {
                Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(command) => self.handle(command),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            // Take every queued command before painting, so a slider drag
            // paints only its newest position.
            loop {
                match commands.try_recv() {
                    Ok(Command::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => break 'run,
                    Ok(command) => self.handle(command),
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            let now = Instant::now();
            let elapsed = self.fade_time(now.saturating_duration_since(last_tick));
            sleep = self.tick(elapsed, &mut emit);
            last_tick = now;
            // A quick fade's frames and a preview's paints wait at
            // user-initiated QoS, and every other wait at utility
            // ([`set_qos`]). The first frame or paint, which the thread was
            // already awake for, runs at the class it had.
            let watched = self.animating || self.preview.is_some();
            if watched != user_initiated {
                user_initiated = watched;
                set_qos(user_initiated);
            }
        }
        // The app's main thread waits in `join` for the restore, which then
        // runs at user-initiated QoS too, not behind other processes' work.
        if !user_initiated {
            set_qos(true);
        }
        self.screen.restore();
    }

    /// What a tick aims the displays at: a preview, and the level that the
    /// switches and the schedule ask for. The schedule decides while Red Now
    /// is off and the schedule on, at the minute `clock` reads, which it is
    /// asked for then only.
    fn aim(&self, clock: impl FnOnce() -> f64) -> Aim {
        let (minute, level) = if self.red_now {
            (None, 1.0)
        } else if self.settings.schedule_on {
            let minute = clock();
            (Some(minute), self.settings.schedule().level(minute))
        } else {
            (None, 0.0)
        };
        Aim {
            minute,
            preview: self.preview,
            level,
        }
    }

    /// How long the quick fade has run at the next tick, `since` the last
    /// one: what [`tick`](Self::tick) takes as `elapsed`.
    ///
    /// - While a quick fade runs, its frames are the ticks, so that is
    ///   `since`.
    /// - Commands that changed the aim since the last tick (Red Now on or
    ///   off, a preview let go, settings that move the level), and the first
    ///   tick, start a quick fade at this tick. Its first step is one
    ///   [`ANIMATION_FRAME`], however long the engine slept before.
    /// - Otherwise the level lands at once: [`QUICK_FADE`], the whole fade.
    ///   Only the clock moved the level then: the schedule's own fade, its
    ///   edges, a new time zone, or a sleep of the Mac. `since` cannot tell
    ///   a sleep, as an `Instant` does not count the time the Mac slept, so
    ///   a wake into the morning would fade back through the red that macOS
    ///   has already reset.
    ///
    /// The aim is compared at the minute the last tick read: the clock is
    /// read again only if that tick read none and the schedule decides now,
    /// an aim that differs at any minute.
    fn fade_time(&self, since: Duration) -> Duration {
        if self.animating {
            return since;
        }
        match self.aimed {
            Some(aimed) if self.aim(|| aimed.minute.unwrap_or_else(self.clock)) == aimed => {
                QUICK_FADE
            }
            _ => ANIMATION_FRAME,
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Configure(settings) => self.settings = settings,
            Command::RedNow(on) => self.red_now = on,
            Command::Preview(look) => self.preview = look,
            // The next tick reads the clock, which every tick does, and
            // checks the displays at its own `now`.
            Command::Refresh => self.refresh = true,
            Command::Shutdown => {}
        }
    }

    /// Moves the displays one step toward the target and returns how long
    /// to sleep before the next step. The level moves at most `elapsed` over
    /// [`QUICK_FADE`]: the engine passes how long the quick fade has run
    /// since its last step ([`fade_time`](Self::fade_time)).
    pub(crate) fn tick(&mut self, elapsed: Duration, emit: &mut impl FnMut(Event)) -> Duration {
        self.tick_at(Instant::now(), elapsed, emit)
    }

    /// [`tick`](Self::tick) at `now`, `elapsed` after the last step: the
    /// checks' times count from `now`, so tests can move it by hand.
    pub(crate) fn tick_at(
        &mut self,
        now: Instant,
        elapsed: Duration,
        emit: &mut impl FnMut(Event),
    ) -> Duration {
        // The clock is read once a tick, so that its step and the wait after
        // it agree on the time.
        let aim = self.aim(self.clock);
        self.aimed = Some(aim);
        let Aim {
            minute,
            level: target_level,
            ..
        } = aim;
        // Quick transitions move the level, not the color, so Red now
        // fades through the oranges of the black-body curve instead of
        // cutting across through pink.
        let fresh = match self.preview {
            Some(look) => {
                self.level = 1.0;
                look.tint()
            }
            None => {
                let max_step = elapsed.as_secs_f32() / QUICK_FADE.as_secs_f32();
                self.level = step_toward(self.level, target_level, max_step);
                self.settings.night.at_level(self.level)
            }
        };
        let animating = self.preview.is_none() && self.level != target_level;
        let landing = std::mem::replace(&mut self.animating, animating);
        // The schedule's fade, when it decides the tint: its minutes until a
        // change are 0 while it runs.
        let fading = self.preview.is_none()
            && minute
                .is_some_and(|minute| self.settings.schedule().minutes_until_change(minute) <= 0.0);
        // A step of the schedule's fade that moves no channel a table step
        // from the tint shown would paint the same table again: the displays
        // keep that tint, and a status its level. The fade's end, which is
        // no longer `fading`, paints whatever it changes, as do a quick
        // fade's steps, its last one too, and a preview.
        let held = fading
            && !animating
            && !landing
            && fresh != self.shown
            && fresh.distance(self.shown) < TABLE_STEP;
        let next = if held { self.shown } else { fresh };
        if !held {
            self.shown_level = self.level;
        }
        let changed = next != self.shown;
        let tinted = !next.is_neutral();
        // A refresh while the screen is untinted has nothing to check: its
        // tick has read the clock, which is all it asks then.
        let refresh = std::mem::take(&mut self.refresh) && tinted;
        if refresh {
            self.checks.refreshed(now);
        }
        let check = tinted && (refresh || self.checks.is_due(now));
        let mut displays = None;
        if changed || check {
            // A new tint is painted without a look; a check that finds a
            // foreign table paints ours back.
            if changed || !self.screen.intact() {
                displays = Some(self.paint(next, emit));
                if tinted {
                    self.checks.painted(now);
                }
            } else {
                self.checks.intact(now);
            }
            if changed {
                self.shown_since = Some(now);
            }
            self.shown = next;
        }
        let displays = displays
            .or(self.last_status.map(|status| status.displays))
            .unwrap_or_else(|| self.screen.display_count());
        let status = Status {
            level: self.shown_level,
            tint: self.shown,
            displays,
            failed: self.failed,
        };
        if self.last_status != Some(status) {
            self.last_status = Some(status);
            emit(Event::Status(status));
        }

        if animating {
            return ANIMATION_FRAME;
        }
        let schedule_wait = match minute {
            Some(minute) if fading => {
                let since = self
                    .shown_since
                    .map_or(FADE_STEP, |since| now.saturating_duration_since(since));
                self.next_table_step(minute, FADE_STEP.saturating_sub(since))
            }
            Some(minute) if self.preview.is_none() => {
                let minutes = self.settings.schedule().minutes_until_change(minute);
                Duration::from_secs_f64(minutes * 60.0).min(MAX_SLEEP)
            }
            _ => MAX_SLEEP,
        };
        if self.shown.is_neutral() {
            schedule_wait
        } else {
            schedule_wait.min(self.checks.wait(now))
        }
    }

    /// How long after `minute`, in the schedule's fade, the next tick has
    /// something to paint: the fade's end, or, `floor` from now on, its tint
    /// a table step ([`TABLE_STEP`]) from the one shown. At most
    /// [`MAX_SLEEP`], and late by at most [`STEP_PRECISION`].
    ///
    /// It walks the curve [`FADE_STEP`] at a time, then halves the step it
    /// stopped in. Walking first finds such a moment even where the level
    /// turns back, as it does in a window no longer than its fade: no later
    /// than a tick every [`FADE_STEP`] would have painted it.
    fn next_table_step(&self, minute: f64, floor: Duration) -> Duration {
        let schedule = self.settings.schedule();
        let look = self.settings.night;
        let due = |after: Duration| {
            let at = minute + after.as_secs_f64() / 60.0;
            schedule.minutes_until_change(at) > 0.0
                || (after >= floor
                    && look.at_level(schedule.level(at)).distance(self.shown) >= TABLE_STEP)
        };
        let mut before = Duration::ZERO;
        let mut after = FADE_STEP;
        while !due(after) {
            if after >= MAX_SLEEP {
                return MAX_SLEEP;
            }
            before = after;
            after += FADE_STEP;
        }
        while after - before > STEP_PRECISION {
            let middle = (before + after) / 2;
            if due(middle) {
                after = middle;
            } else {
                before = middle;
            }
        }
        after
    }

    /// Puts `tint` on the displays and returns how many show it.
    fn paint(&mut self, tint: Tint, emit: &mut impl FnMut(Event)) -> usize {
        if tint.is_neutral() {
            self.screen.restore();
            self.failed = false;
            return self.screen.display_count();
        }
        match self.screen.apply(tint) {
            Ok(count) => {
                self.failed = false;
                count
            }
            Err(error) => {
                if !self.failed {
                    log::warn!("unable to tint the displays: {error}");
                    emit(Event::Error(error));
                }
                self.failed = true;
                0
            }
        }
    }
}

/// Moves `from` toward `to` by at most `max_step`.
fn step_toward(from: f32, to: f32, max_step: f32) -> f32 {
    let delta = to - from;
    if delta.abs() <= max_step.max(0.0) {
        to
    } else {
        from + max_step * delta.signum()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::color::PURE_RED_KELVIN;
    use crate::schedule::{ClockTime, Schedule};

    #[derive(Default)]
    struct Log {
        /// Every `apply`, the refused ones too.
        applied: Vec<Tint>,
        restores: usize,
        /// How many times the engine asked whether the tables were intact.
        checks: usize,
        /// Another app took the tables: `intact` answers `false` until the
        /// next successful `apply`.
        reset: bool,
        /// `apply` refuses the table, which then shows nothing of ours.
        fail: bool,
    }

    struct FakeScreen(Arc<Mutex<Log>>);

    impl Screen for FakeScreen {
        fn apply(&mut self, tint: Tint) -> Result<usize, String> {
            let mut log = self.0.lock().unwrap();
            log.applied.push(tint);
            if log.fail {
                // A refused table leaves the displays as they were, which is
                // not what the engine painted: the next check finds it so.
                log.reset = true;
                return Err("macOS refused the color table".to_owned());
            }
            log.reset = false;
            Ok(2)
        }
        fn intact(&mut self) -> bool {
            let mut log = self.0.lock().unwrap();
            log.checks += 1;
            !log.reset
        }
        fn restore(&mut self) {
            self.0.lock().unwrap().restores += 1;
        }
        fn display_count(&mut self) -> usize {
            2
        }
    }

    /// The settings the workers run on: 21:00 to 07:00, a 20 minute fade and
    /// the full red.
    fn night_settings() -> Settings {
        Settings {
            start: ClockTime::new(21, 0),
            end: ClockTime::new(7, 0),
            night: Look {
                kelvin: PURE_RED_KELVIN,
                brightness: 100,
            },
            ..Settings::default()
        }
    }

    fn worker(clock: fn() -> f64) -> (Worker, Arc<Mutex<Log>>) {
        let log = Arc::new(Mutex::new(Log::default()));
        (
            Worker::new(Box::new(FakeScreen(log.clone())), night_settings(), clock),
            log,
        )
    }

    fn midnight() -> f64 {
        0.0
    }

    fn noon() -> f64 {
        12.0 * 60.0
    }

    fn settle(worker: &mut Worker) {
        for _ in 0..100 {
            worker.tick(Duration::from_millis(100), &mut |_| {});
        }
    }

    #[test]
    fn night_fades_to_the_full_look_and_stays() {
        let (mut worker, log) = worker(midnight);
        let first = worker.tick(Duration::from_millis(100), &mut |_| {});
        assert_eq!(first, ANIMATION_FRAME, "a quick fade runs at startup");
        settle(&mut worker);
        let last = *log.lock().unwrap().applied.last().unwrap();
        assert_eq!(
            last,
            Tint {
                red: 1.0,
                green: 0.0,
                blue: 0.0
            }
        );
        assert!(worker.shown_level > 0.99);
    }

    #[test]
    fn red_now_fades_through_orange_not_pink() {
        let (mut worker, log) = worker(noon);
        worker.handle(Command::RedNow(true));
        settle(&mut worker);
        // Along the black-body curve green always stays above blue; a
        // straight line from white to red would keep them equal (pink).
        for tint in &log.lock().unwrap().applied {
            assert!(tint.green >= tint.blue, "{tint:?}");
            if tint.green < 0.9 && tint.green > 0.1 {
                assert!(tint.green - tint.blue > 0.05, "{tint:?}");
            }
        }
    }

    #[test]
    fn day_leaves_the_screen_alone() {
        let (mut worker, log) = worker(noon);
        settle(&mut worker);
        assert!(log.lock().unwrap().applied.is_empty());
    }

    #[test]
    fn red_now_overrides_the_day_and_lets_go() {
        let (mut worker, log) = worker(noon);
        worker.handle(Command::RedNow(true));
        settle(&mut worker);
        assert_eq!(
            worker.shown,
            Tint {
                red: 1.0,
                green: 0.0,
                blue: 0.0
            }
        );
        worker.handle(Command::RedNow(false));
        settle(&mut worker);
        assert!(worker.shown.is_neutral());
        assert!(
            log.lock().unwrap().restores >= 1,
            "neutral restores ColorSync"
        );
    }

    #[test]
    fn preview_paints_at_once() {
        let (mut worker, _log) = worker(noon);
        let look = held();
        worker.handle(Command::Preview(Some(look)));
        worker.tick(Duration::from_millis(1), &mut |_| {});
        assert_eq!(worker.shown, look.tint());
    }

    #[test]
    fn a_reset_display_is_painted_again() {
        let (mut worker, log) = worker(midnight);
        settle(&mut worker);
        let painted = log.lock().unwrap().applied.len();
        log.lock().unwrap().reset = true;
        worker.handle(Command::Refresh);
        worker.tick(Duration::from_millis(100), &mut |_| {});
        assert_eq!(log.lock().unwrap().applied.len(), painted + 1);
        assert!(!log.lock().unwrap().reset);
    }

    #[test]
    fn steady_day_sleeps_long() {
        let (mut worker, _log) = worker(noon);
        let sleep = worker.tick(Duration::from_millis(100), &mut |_| {});
        assert_eq!(sleep, MAX_SLEEP);
    }

    #[test]
    fn engine_thread_restores_on_shutdown() {
        let log = Arc::new(Mutex::new(Log::default()));
        let mut engine = Engine::spawn(
            Box::new(FakeScreen(log.clone())),
            Settings::default(),
            noon,
            |_| {},
        );
        engine.send(Command::RedNow(true));
        std::thread::sleep(Duration::from_millis(200));
        engine.shutdown();
        assert!(log.lock().unwrap().restores >= 1);
        assert!(!log.lock().unwrap().applied.is_empty());
    }

    // Synthetic time. `Worker::new` seeds its checks from the real clock, so
    // these tests take `t0` after it and move time by hand with `tick_at`.

    /// Long enough for a quick fade to finish in one step.
    const SETTLES: Duration = Duration::from_secs(10);

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn millis(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// One tick of `worker` at `now`, its events dropped.
    fn at(worker: &mut Worker, now: Instant) -> Duration {
        worker.tick_at(now, SETTLES, &mut |_| {})
    }

    thread_local! {
        /// The clock of the tests that move it by hand: `worker(dial)`. A
        /// test's own thread ticks its worker, so its dial is its own.
        static DIAL: Cell<f64> = const { Cell::new(0.0) };
    }

    fn dial() -> f64 {
        DIAL.get()
    }

    fn paints(log: &Arc<Mutex<Log>>) -> usize {
        log.lock().unwrap().applied.len()
    }

    fn looks(log: &Arc<Mutex<Log>>) -> usize {
        log.lock().unwrap().checks
    }

    fn restores(log: &Arc<Mutex<Log>>) -> usize {
        log.lock().unwrap().restores
    }

    /// Ticks the checks at 2, 6, 14 and 30 s after the paint at `t0`, which
    /// all read intact: the wait is then 30 s, and the next check due at
    /// `t0` + 60 s.
    fn backed_off(worker: &mut Worker, t0: Instant) {
        for n in [2, 6, 14, 30] {
            at(worker, t0 + secs(n));
        }
        assert_eq!(worker.checks.interval, CHECK_LONGEST);
        assert_eq!(worker.checks.due, t0 + secs(60));
    }

    #[test]
    fn the_check_backs_off_to_30_s_while_the_tables_are_intact() {
        let (mut worker, log) = worker(midnight);
        let t0 = Instant::now();
        assert_eq!(at(&mut worker, t0), secs(2), "a paint is checked 2 s on");
        assert_eq!(paints(&log), 1);
        assert_eq!(looks(&log), 0, "a paint is not checked as well");
        // The wait doubles at each check that reads intact, and stops at 30 s.
        let checks = [(2, 4), (6, 8), (14, 16), (30, 30), (60, 30), (90, 30)];
        for (count, (when, wait)) in checks.into_iter().enumerate() {
            assert_eq!(
                at(&mut worker, t0 + secs(when)),
                secs(wait),
                "the check at {when} s"
            );
            assert_eq!(looks(&log), count + 1, "one check at {when} s");
        }
        assert_eq!(paints(&log), 1, "intact tables are not painted again");
    }

    #[test]
    fn no_check_runs_before_it_is_due() {
        let (mut worker, log) = worker(midnight);
        let t0 = Instant::now();
        at(&mut worker, t0);
        let early = t0 + millis(1900);
        assert_eq!(
            at(&mut worker, early),
            millis(100),
            "sleeps up to the check"
        );
        assert_eq!(looks(&log), 0);
        // A hair before, and then exactly on time.
        let hair = t0 + secs(2) - Duration::from_nanos(1);
        assert_eq!(at(&mut worker, hair), Duration::from_nanos(1));
        assert_eq!(looks(&log), 0);
        assert_eq!(at(&mut worker, t0 + secs(2)), secs(4));
        assert_eq!(looks(&log), 1);
    }

    #[test]
    fn a_paint_starts_the_backoff_over() {
        let look = held();
        let another_night = Settings {
            night: look,
            ..night_settings()
        };
        for command in [
            Command::Preview(Some(look)),
            Command::Configure(another_night),
        ] {
            let (mut worker, log) = worker(midnight);
            let t0 = Instant::now();
            at(&mut worker, t0);
            backed_off(&mut worker, t0);
            let (painted, looked) = (paints(&log), looks(&log));

            let paint = t0 + secs(40);
            worker.handle(command.clone());
            assert_eq!(at(&mut worker, paint), secs(2), "{command:?}");
            assert_eq!(paints(&log), painted + 1, "{command:?}");
            assert_eq!(looks(&log), looked, "a new tint is painted unseen");
            assert_eq!(worker.checks.interval, CHECK_FIRST, "{command:?}");
            // The next check is 2 s after the paint, not at the old 60 s.
            assert_eq!(at(&mut worker, paint + secs(1)), secs(1));
            assert_eq!(looks(&log), looked);
            assert_eq!(at(&mut worker, paint + secs(2)), secs(4));
            assert_eq!(looks(&log), looked + 1);
        }
    }

    #[test]
    fn a_refresh_checks_at_once_and_again_a_second_later() {
        let (mut worker, log) = worker(midnight);
        let t0 = Instant::now();
        at(&mut worker, t0);
        backed_off(&mut worker, t0);
        let (painted, looked) = (paints(&log), looks(&log));

        let refresh = t0 + secs(31);
        worker.handle(Command::Refresh);
        assert_eq!(at(&mut worker, refresh), secs(1), "checks now, and in 1 s");
        assert_eq!(looks(&log), looked + 1);
        assert_eq!(worker.checks.interval, CHECK_FIRST, "a refresh starts over");

        // The recheck is a plain look: it doubles nothing and moves nothing.
        assert_eq!(at(&mut worker, refresh + secs(1)), secs(1));
        assert_eq!(looks(&log), looked + 2);
        assert_eq!(worker.checks.interval, CHECK_FIRST);
        assert_eq!(worker.checks.due, refresh + secs(2));
        assert_eq!(worker.checks.again, None);

        // The backoff's own check is the one at 2 s, and it doubles.
        assert_eq!(at(&mut worker, refresh + secs(2)), secs(4));
        assert_eq!(looks(&log), looked + 3);
        assert_eq!(paints(&log), painted, "intact tables are not painted again");
    }

    #[test]
    fn a_check_that_finds_a_foreign_table_paints_again() {
        let (mut worker, log) = worker(midnight);
        let t0 = Instant::now();
        at(&mut worker, t0);
        log.lock().unwrap().reset = true;
        // Not looked at before it is due.
        assert_eq!(at(&mut worker, t0 + secs(1)), secs(1));
        assert_eq!(paints(&log), 1);

        assert_eq!(at(&mut worker, t0 + secs(2)), secs(2), "painted: 2 s again");
        assert_eq!(paints(&log), 2);
        assert!(!log.lock().unwrap().reset, "the table is ours again");
        assert_eq!(worker.checks.interval, CHECK_FIRST);
        // The next check reads intact, and the wait doubles from 2 s.
        assert_eq!(at(&mut worker, t0 + secs(4)), secs(4));
        assert_eq!(paints(&log), 2);
    }

    #[test]
    fn a_refresh_that_finds_a_foreign_table_paints_at_once_and_looks_again() {
        let (mut worker, log) = worker(midnight);
        let t0 = Instant::now();
        at(&mut worker, t0);
        backed_off(&mut worker, t0);
        let (painted, looked) = (paints(&log), looks(&log));

        let refresh = t0 + secs(31);
        log.lock().unwrap().reset = true;
        worker.handle(Command::Refresh);
        assert_eq!(at(&mut worker, refresh), secs(1));
        assert_eq!(paints(&log), painted + 1, "painted on the notice");
        assert_eq!(looks(&log), looked + 1);
        assert_eq!(worker.checks.again, Some(refresh + secs(1)));

        // WindowServer resets the table again just after the notice: the
        // second look catches it, and the paint starts the wait over.
        log.lock().unwrap().reset = true;
        assert_eq!(at(&mut worker, refresh + secs(1)), secs(2));
        assert_eq!(paints(&log), painted + 2, "painted at the recheck");
        assert_eq!(looks(&log), looked + 2);
        assert_eq!(worker.checks.again, None);
        assert_eq!(worker.checks.due, refresh + secs(3));
    }

    #[test]
    fn a_refresh_by_day_checks_nothing_and_leaves_no_recheck() {
        let (mut worker, log) = worker(noon);
        let t0 = Instant::now();
        at(&mut worker, t0);
        worker.handle(Command::Refresh);
        // It reads the clock, which every tick does, and no more.
        assert_eq!(at(&mut worker, t0 + secs(1)), MAX_SLEEP);
        assert_eq!(looks(&log), 0);
        assert_eq!(paints(&log), 0);
        assert_eq!(worker.checks.again, None);

        // A night paint soon after has no recheck of that refresh to come:
        // the one it would have left falls due at 2 s.
        worker.handle(Command::RedNow(true));
        let night = t0 + millis(1200);
        assert_eq!(at(&mut worker, night), secs(2));
        assert_eq!(paints(&log), 1);
        assert_eq!(at(&mut worker, t0 + secs(2)), millis(1200));
        assert_eq!(looks(&log), 0, "no check at 2 s: the paint's is at 3.2 s");
    }

    #[test]
    fn a_paint_drops_a_recheck_that_is_already_past() {
        let t0 = Instant::now();
        // `Checks` alone: kept while ahead, gone at or past its time.
        let mut checks = Checks::new(t0);
        checks.refreshed(t0);
        checks.painted(t0 + millis(999));
        assert_eq!(checks.again, Some(t0 + secs(1)));
        checks.painted(t0 + secs(1));
        assert_eq!(checks.again, None, "a recheck due now is past");
        checks.refreshed(t0);
        checks.painted(t0 + secs(5));
        assert_eq!(checks.again, None);

        // A tick that comes late, with a new tint to paint.
        let (mut worker, log) = worker(midnight);
        at(&mut worker, t0);
        worker.handle(Command::Refresh);
        assert_eq!(at(&mut worker, t0 + secs(10)), secs(1));
        assert_eq!(worker.checks.again, Some(t0 + secs(11)));
        let look = held();
        worker.handle(Command::Preview(Some(look)));
        let late = t0 + secs(15);
        let looked = looks(&log);
        assert_eq!(at(&mut worker, late), secs(2));
        assert_eq!(
            worker.checks.again, None,
            "the paint is newer than the recheck"
        );
        // The recheck's time came and went: no check runs for it now.
        assert_eq!(at(&mut worker, late + secs(1)), secs(1));
        assert_eq!(looks(&log), looked);
    }

    #[test]
    fn a_fast_scheduled_fade_paints_every_second_and_never_checks() {
        let evening = 21.0 * 60.0 + 5.0;
        DIAL.set(evening);
        let (mut worker, log) = worker(dial);
        let t0 = Instant::now();
        // The fade of 21:00 to 21:20 to pure red is a quarter in, where its
        // tint moves more than a table step a second: it steps every second.
        assert_eq!(at(&mut worker, t0), FADE_STEP);
        for step in 1..=10_u64 {
            DIAL.set(evening + step as f64 / 60.0);
            assert_eq!(
                worker.tick_at(t0 + secs(step), secs(1), &mut |_| {}),
                FADE_STEP
            );
            assert_eq!(paints(&log), step as usize + 1, "step {step} paints");
        }
        assert_eq!(looks(&log), 0, "a paint a second leaves nothing to check");
    }

    #[test]
    fn a_failed_paint_is_tried_again_at_the_next_check() {
        let (mut worker, log) = worker(midnight);
        log.lock().unwrap().fail = true;
        let mut events = Vec::new();
        let mut emit = |event: Event| events.push(event);
        let t0 = Instant::now();

        assert_eq!(worker.tick_at(t0, SETTLES, &mut emit), secs(2));
        assert_eq!(paints(&log), 1);
        assert_eq!(looks(&log), 0);
        // The check finds the displays not showing what was painted.
        assert_eq!(worker.tick_at(t0 + secs(2), SETTLES, &mut emit), secs(2));
        assert_eq!(paints(&log), 2, "tried again");
        assert_eq!(looks(&log), 1);

        log.lock().unwrap().fail = false;
        assert_eq!(worker.tick_at(t0 + secs(4), SETTLES, &mut emit), secs(2));
        assert_eq!(paints(&log), 3);
        assert!(!log.lock().unwrap().reset);
        assert_eq!(worker.tick_at(t0 + secs(6), SETTLES, &mut emit), secs(4));
        assert_eq!(paints(&log), 3, "painted: it backs off");

        // One error, however often it fails, and the recovery after it.
        assert!(
            matches!(
                events.as_slice(),
                [
                    Event::Error(_),
                    Event::Status(Status {
                        failed: true,
                        displays: 0,
                        ..
                    }),
                    Event::Status(Status {
                        failed: false,
                        displays: 2,
                        ..
                    }),
                ]
            ),
            "{events:?}"
        );
    }

    #[test]
    fn the_checks_arithmetic_at_its_edges() {
        let t0 = Instant::now();
        let mut checks = Checks::new(t0);
        assert_eq!(checks.due, t0 + CHECK_FIRST);
        assert_eq!(checks.interval, CHECK_FIRST);
        assert_eq!(checks.again, None);
        assert_eq!(
            (CHECK_FIRST, CHECK_LONGEST, RECHECK),
            (secs(2), secs(30), secs(1))
        );

        // Due on the dot, and the wait counts down to zero and stays there.
        assert!(!checks.is_due(t0 + secs(2) - Duration::from_nanos(1)));
        assert!(checks.is_due(t0 + secs(2)));
        assert!(checks.is_due(t0 + secs(9)));
        assert_eq!(checks.wait(t0), secs(2));
        assert_eq!(checks.wait(t0 + secs(2)), Duration::ZERO);
        assert_eq!(checks.wait(t0 + secs(9)), Duration::ZERO);

        // Each check on time doubles the wait, up to the longest.
        let mut now = t0 + secs(2);
        let mut waits = Vec::new();
        for _ in 0..6 {
            checks.intact(now);
            waits.push(checks.interval);
            assert_eq!(checks.due, now + checks.interval);
            now = checks.due;
        }
        assert_eq!(waits, [4, 8, 16, 30, 30, 30].map(secs));
        // A check that comes late counts its wait from when it ran.
        let late = checks.due + secs(10);
        checks.intact(late);
        assert_eq!(checks.due, late + CHECK_LONGEST);

        // A paint starts over from its own time.
        checks.painted(t0 + secs(100));
        assert_eq!(checks.due, t0 + secs(102));
        assert_eq!(checks.interval, CHECK_FIRST);
    }

    #[test]
    fn a_refresh_makes_two_looks_and_the_second_is_a_plain_one() {
        let t0 = Instant::now();
        let mut checks = Checks::new(t0);
        checks.intact(t0 + secs(2));
        checks.intact(t0 + secs(6));
        assert_eq!(checks.interval, secs(8));

        let now = t0 + secs(10);
        checks.refreshed(now);
        assert_eq!(checks.interval, CHECK_FIRST, "the backoff starts over");
        assert_eq!(checks.due, now + secs(2));
        assert_eq!(checks.again, Some(now + secs(1)));
        // The earlier of the two is the next, and each is due on the dot.
        assert_eq!(checks.wait(now), secs(1));
        assert!(!checks.is_due(now + millis(999)));
        assert!(checks.is_due(now + secs(1)));
        assert_eq!(checks.wait(now + secs(1)), Duration::ZERO);

        // The first look is before the recheck: it clears nothing.
        checks.intact(now);
        assert_eq!(checks.again, Some(now + secs(1)));
        checks.intact(now + millis(999));
        assert_eq!(checks.again, Some(now + secs(1)));
        // The recheck clears itself and moves neither the wait nor `due`.
        checks.intact(now + secs(1));
        assert_eq!(checks.again, None);
        assert_eq!(checks.interval, CHECK_FIRST);
        assert_eq!(checks.due, now + secs(2));
        // The backoff's own check then doubles.
        checks.intact(now + secs(2));
        assert_eq!(checks.interval, secs(4));

        // With the recheck after `due`, which the arithmetic allows, the
        // earlier is still the next.
        let checks = Checks {
            due: t0 + secs(2),
            interval: CHECK_FIRST,
            again: Some(t0 + secs(5)),
        };
        assert_eq!(checks.wait(t0), secs(2));
        assert!(checks.is_due(t0 + secs(2)));
        assert!(!checks.is_due(t0 + secs(2) - Duration::from_nanos(1)));
    }

    #[test]
    fn by_day_the_engine_sleeps_up_to_a_minute() {
        assert_eq!(MAX_SLEEP, secs(60));
        let evening = 21.0 * 60.0;
        for (clock, wait) in [
            (noon(), secs(60)),
            // The schedule changes in 90 s, 30 s and under a second.
            (evening - 1.5, secs(60)),
            (evening - 0.5, secs(30)),
            (evening - 0.5 / 60.0, millis(500)),
        ] {
            DIAL.set(clock);
            let (mut worker, log) = worker(dial);
            assert_eq!(at(&mut worker, Instant::now()), wait, "at minute {clock}");
            assert_eq!(paints(&log), 0, "by day");
        }
    }

    #[test]
    fn without_the_schedule_only_the_check_bounds_a_tinted_sleep() {
        let look = held();
        for (what, command) in [
            ("Red Now", Command::RedNow(true)),
            ("a held preview", Command::Preview(Some(look))),
        ] {
            let (mut worker, _log) = worker(noon);
            let off = Settings {
                schedule_on: false,
                ..night_settings()
            };
            worker.handle(Command::Configure(off));
            let t0 = Instant::now();
            assert_eq!(at(&mut worker, t0), MAX_SLEEP, "nothing to do by day");

            worker.handle(command);
            let paint = t0 + secs(1);
            assert_eq!(at(&mut worker, paint), secs(2), "{what}: the check");
            backed_off(&mut worker, paint);
            let sleep = at(&mut worker, paint + secs(60));
            assert_eq!(sleep, CHECK_LONGEST, "{what}: not a minute");
        }
        // The same by day with the schedule on: Red Now is the night.
        let (mut worker, _log) = worker(noon);
        let t0 = Instant::now();
        worker.handle(Command::RedNow(true));
        assert_eq!(at(&mut worker, t0), secs(2));
    }

    fn status(level: f32, tint: Tint, displays: usize, failed: bool) -> Event {
        Event::Status(Status {
            level,
            tint,
            displays,
            failed,
        })
    }

    /// A tint that is not neutral, redder with `level`.
    fn reddish(level: f32) -> Tint {
        Tint {
            red: 1.0,
            green: 0.5 - level / 4.0,
            blue: 0.1,
        }
    }

    /// A tinted status at `level`.
    fn red(level: f32) -> Event {
        status(level, reddish(level), 2, false)
    }

    fn plain() -> Event {
        status(0.0, Tint::NEUTRAL, 2, false)
    }

    fn error() -> Event {
        Event::Error("macOS refused the color table".to_owned())
    }

    #[test]
    fn the_first_event_reaches_the_app_whatever_it_is() {
        let t0 = Instant::now();
        for first in [plain(), red(0.3), error()] {
            assert_eq!(Pacer::default().reach(&first, t0), Reach::App, "{first:?}");
        }
    }

    #[test]
    fn a_level_only_status_reaches_a_closed_app_once_in_five_seconds() {
        assert_eq!(LEVEL_PACE, secs(5));
        let t0 = Instant::now();
        let mut pacer = Pacer::default();
        assert_eq!(pacer.reach(&red(0.1), t0), Reach::App);
        for after in [millis(1), secs(1), millis(4999)] {
            assert_eq!(
                pacer.reach(&red(0.2), t0 + after),
                Reach::Window,
                "{after:?}"
            );
        }
        // Exactly a pace is enough, and the count starts again from there.
        assert_eq!(pacer.reach(&red(0.3), t0 + secs(5)), Reach::App);
        assert_eq!(pacer.reach(&red(0.4), t0 + secs(9)), Reach::Window);
        assert_eq!(pacer.reach(&red(0.5), t0 + millis(9999)), Reach::Window);
        assert_eq!(pacer.reach(&red(0.6), t0 + secs(10)), Reach::App);
        // Later than a pace is enough too.
        assert_eq!(pacer.reach(&red(0.7), t0 + secs(60)), Reach::App);
    }

    #[test]
    fn what_a_closed_app_acts_on_reaches_it_at_once() {
        let t0 = Instant::now();
        let soon = t0 + millis(100);
        let refused = |level| status(level, reddish(level), 0, true);
        let cases = [
            ("the tint coming", plain(), red(0.1)),
            ("the tint going", red(0.1), plain()),
            ("painting failing", red(1.0), refused(1.0)),
            ("painting recovering", refused(1.0), red(1.0)),
        ];
        for (what, before, after) in cases {
            let mut pacer = Pacer::default();
            assert_eq!(pacer.reach(&before, t0), Reach::App);
            assert_eq!(pacer.reach(&after, soon), Reach::App, "{what}");
        }
        // Each error, however close to the last wake.
        let mut pacer = Pacer::default();
        pacer.reach(&red(0.5), t0);
        assert_eq!(pacer.reach(&error(), soon), Reach::App);
        assert_eq!(pacer.reach(&error(), soon + millis(1)), Reach::App);
    }

    #[test]
    fn a_change_of_displays_alone_is_level_only() {
        // The display watch already wakes the app for a display change.
        let t0 = Instant::now();
        let mut pacer = Pacer::default();
        let tint = reddish(0.5);
        assert_eq!(pacer.reach(&status(0.5, tint, 2, false), t0), Reach::App);
        let more = status(0.5, tint, 3, false);
        assert_eq!(pacer.reach(&more, t0 + millis(100)), Reach::Window);
    }

    #[test]
    fn a_wake_that_waits_for_the_window_does_not_move_the_pace() {
        let t0 = Instant::now();
        let mut pacer = Pacer::default();
        assert_eq!(pacer.reach(&red(0.1), t0), Reach::App);
        assert_eq!(pacer.reach(&red(0.2), t0 + secs(3)), Reach::Window);
        assert_eq!(pacer.reach(&red(0.3), t0 + secs(4)), Reach::Window);
        assert_eq!(pacer.reach(&red(0.4), t0 + secs(5)), Reach::App);
        // An error that wakes the app does move it.
        assert_eq!(pacer.reach(&error(), t0 + secs(6)), Reach::App);
        assert_eq!(pacer.reach(&red(0.5), t0 + secs(10)), Reach::Window);
        assert_eq!(pacer.reach(&red(0.6), t0 + secs(11)), Reach::App);
    }

    #[test]
    fn a_twenty_second_fade_wakes_the_app_every_five_seconds() {
        let t0 = Instant::now();
        let mut pacer = Pacer::default();
        // A status a second, the level up a little each time.
        let woken: Vec<u64> = (0..20)
            .filter(|&n| pacer.reach(&red(0.04 * (n + 1) as f32), t0 + secs(n)) == Reach::App)
            .collect();
        assert_eq!(woken, [0, 5, 10, 15]);
    }

    /// Polls `ready` until it holds, for 5 s at most: a bound for a loaded
    /// machine, never a wait that a test expects to spend.
    fn until(what: &str, mut ready: impl FnMut() -> bool) {
        let started = Instant::now();
        while !ready() {
            assert!(started.elapsed() < secs(5), "timed out waiting for {what}");
            std::thread::sleep(millis(2));
        }
    }

    /// Waits for a status of `engine` that `wanted` likes.
    fn until_status(engine: &Engine, what: &str, mut wanted: impl FnMut(&Status) -> bool) {
        until(what, || {
            engine
                .poll()
                .iter()
                .any(|event| matches!(event, Event::Status(status) if wanted(status)))
        });
    }

    /// One tick of `worker` at `now`, a frame after the last, with each event
    /// sent through `pacer` as the engine thread sends it: each status, and
    /// whom it reaches.
    fn paced_tick(
        worker: &mut Worker,
        pacer: &mut Pacer,
        now: Instant,
        steps: &mut Vec<(Status, Reach)>,
    ) {
        worker.tick_at(now, ANIMATION_FRAME, &mut |event: Event| {
            let reach = pacer.reach(&event, now);
            match event {
                Event::Status(status) => steps.push((status, reach)),
                Event::Error(error) => panic!("an error: {error}"),
            }
        });
    }

    /// Ticks `worker` a frame at a time from `now` until `done` holds for the
    /// last status, and returns the time it got there.
    fn paced_until(
        worker: &mut Worker,
        pacer: &mut Pacer,
        mut now: Instant,
        steps: &mut Vec<(Status, Reach)>,
        done: fn(&Status) -> bool,
    ) -> Instant {
        for _ in 0..100 {
            if steps.last().is_some_and(|(status, _)| done(status)) {
                return now;
            }
            now += ANIMATION_FRAME;
            paced_tick(worker, pacer, now, steps);
        }
        panic!("100 frames of a quick fade, and still not there");
    }

    #[test]
    fn red_now_wakes_the_app_for_its_first_and_last_step_only() {
        // The quick fade a frame at a time, by hand, through the pacer the
        // engine thread runs: no step depends on how fast the machine is.
        let (mut worker, _log) = worker(noon);
        let mut pacer = Pacer::default();
        let mut steps = Vec::new();
        let t0 = Instant::now();
        paced_tick(&mut worker, &mut pacer, t0, &mut steps);
        // By day, Red Now on until half red, then off until normal colors.
        worker.handle(Command::RedNow(true));
        let half = paced_until(&mut worker, &mut pacer, t0, &mut steps, |status| {
            status.level >= 0.5
        });
        worker.handle(Command::RedNow(false));
        let end = paced_until(&mut worker, &mut pacer, half, &mut steps, |status| {
            status.tint.is_neutral()
        });
        assert!(end - t0 < LEVEL_PACE, "the fades took {:?}", end - t0);

        assert!(steps[0].0.tint.is_neutral());
        assert_eq!(steps[0].1, Reach::App, "the first status");
        let up = steps
            .iter()
            .position(|(status, _)| !status.tint.is_neutral())
            .expect("a tinted step");
        assert_eq!(steps[up].1, Reach::App, "neutral to tinted");
        let down = steps.len() - 1;
        assert!(down > up && steps[down].0.tint.is_neutral());
        assert_eq!(steps[down].1, Reach::App, "tinted to neutral");

        // Between them each step only moves the level, and the two fades are
        // over well inside the pace: each reaches the window alone.
        let between = &steps[up + 1..down];
        assert!(between.len() >= 30, "{} steps between", between.len());
        assert!(
            between.iter().all(|(_, reach)| *reach == Reach::Window),
            "{between:?}"
        );
    }

    #[test]
    fn the_engine_thread_wakes_once_for_each_event_with_whom_it_reaches() {
        // The thread runs on its own clock, so this checks only what no
        // timing changes: a wake for each event, and the ones that reach the
        // app whenever they come.
        let log = Arc::new(Mutex::new(Log::default()));
        let reaches = Arc::new(Mutex::new(Vec::new()));
        let recorded = reaches.clone();
        let mut engine = Engine::spawn(
            Box::new(FakeScreen(log)),
            Settings::default(),
            noon,
            move |reach| recorded.lock().unwrap().push(reach),
        );
        let tinted =
            |event: &Event| matches!(event, Event::Status(status) if !status.tint.is_neutral());
        let mut events = Vec::new();
        engine.send(Command::RedNow(true));
        until("a tinted status", || {
            events.extend(engine.poll());
            events.iter().any(tinted)
        });
        engine.shutdown();
        events.extend(engine.poll());

        let reaches = reaches.lock().unwrap();
        assert_eq!(reaches.len(), events.len(), "a wake for each event");
        assert_eq!(reaches[0], Reach::App, "the first event");
        let up = events.iter().position(tinted).expect("a tinted status");
        assert_eq!(reaches[up], Reach::App, "neutral to tinted");
    }

    fn spawned(clock: fn() -> f64) -> (Engine, Arc<Mutex<Log>>) {
        let log = Arc::new(Mutex::new(Log::default()));
        let engine = Engine::spawn(
            Box::new(FakeScreen(log.clone())),
            Settings::default(),
            clock,
            |_| {},
        );
        (engine, log)
    }

    /// An engine on `settings`, for the tests whose fade is not the default.
    fn spawned_on(settings: Settings, clock: fn() -> f64) -> (Engine, Arc<Mutex<Log>>) {
        let log = Arc::new(Mutex::new(Log::default()));
        let engine = Engine::spawn(Box::new(FakeScreen(log.clone())), settings, clock, |_| {});
        (engine, log)
    }

    /// Shuts `engine` down: soon, whatever it sleeps on, and giving the
    /// displays back.
    fn shuts_down_and_restores(mut engine: Engine, log: &Arc<Mutex<Log>>) {
        let before = restores(log);
        let started = Instant::now();
        engine.shutdown();
        let took = started.elapsed();
        assert!(took < secs(1), "the engine took {took:?} to stop");
        assert!(restores(log) > before, "the displays were not given back");
    }

    fn mid_fade() -> f64 {
        21.0 * 60.0 + 5.0
    }

    fn held() -> Look {
        Look {
            kelvin: 2000,
            brightness: 60,
        }
    }

    #[test]
    fn shutdown_restores_a_neutral_screen_asleep_for_a_minute() {
        let (engine, log) = spawned(noon);
        until_status(&engine, "the first status", |_| true);
        shuts_down_and_restores(engine, &log);
    }

    #[test]
    fn shutdown_restores_a_screen_whose_check_is_30_s_away() {
        let (mut worker, log) = worker(midnight);
        let t0 = Instant::now();
        at(&mut worker, t0);
        backed_off(&mut worker, t0);
        let (commands, inbox) = mpsc::channel();
        let thread = std::thread::spawn(move || worker.run(&inbox, |_| {}));
        // It ticks once, and then sleeps until its check, a minute off.
        std::thread::sleep(millis(100));

        let before = restores(&log);
        let started = Instant::now();
        commands.send(Command::Shutdown).unwrap();
        thread.join().expect("the engine thread");
        assert!(started.elapsed() < secs(1));
        assert!(restores(&log) > before);
    }

    #[test]
    fn shutdown_restores_in_the_middle_of_a_quick_fade() {
        let (engine, log) = spawned(noon);
        engine.send(Command::RedNow(true));
        until_status(&engine, "a step of the fade", |status| {
            status.level > 0.0 && status.level < 1.0
        });
        shuts_down_and_restores(engine, &log);
    }

    #[test]
    fn shutdown_restores_in_the_middle_of_a_scheduled_fade() {
        let (engine, log) = spawned(mid_fade);
        until_status(&engine, "the fade's level", |status| {
            (status.level - 0.25).abs() < 1e-3
        });
        shuts_down_and_restores(engine, &log);
    }

    #[test]
    fn shutdown_restores_a_held_preview() {
        let (engine, log) = spawned(noon);
        engine.send(Command::Preview(Some(held())));
        until_status(&engine, "the preview", |status| !status.tint.is_neutral());
        shuts_down_and_restores(engine, &log);
    }

    #[test]
    fn shutdown_restores_just_after_a_refresh_with_its_recheck_pending() {
        let (engine, log) = spawned(noon);
        engine.send(Command::Preview(Some(held())));
        until_status(&engine, "the preview", |status| !status.tint.is_neutral());
        engine.send(Command::Refresh);
        until("the refresh's check", || looks(&log) >= 1);
        shuts_down_and_restores(engine, &log);
    }

    #[test]
    fn shutdown_restores_a_screen_that_refuses_the_tint() {
        let (engine, log) = spawned(noon);
        log.lock().unwrap().fail = true;
        engine.send(Command::Preview(Some(held())));
        until("the refusal", || {
            engine
                .poll()
                .iter()
                .any(|event| matches!(event, Event::Error(_)))
        });
        shuts_down_and_restores(engine, &log);
    }

    #[test]
    fn a_dropped_command_channel_restores_an_engine_asleep() {
        let (mut worker, log) = worker(midnight);
        let t0 = Instant::now();
        at(&mut worker, t0);
        backed_off(&mut worker, t0);
        let (commands, inbox) = mpsc::channel();
        let thread = std::thread::spawn(move || worker.run(&inbox, |_| {}));
        std::thread::sleep(millis(100));

        let before = restores(&log);
        let started = Instant::now();
        drop(commands);
        thread.join().expect("the engine thread");
        assert!(started.elapsed() < secs(1));
        assert_eq!(restores(&log), before + 1);
    }

    #[test]
    fn a_dropped_command_channel_restores_once_after_a_last_command() {
        let (mut worker, log) = worker(midnight);
        at(&mut worker, Instant::now());
        let (commands, inbox) = mpsc::channel();
        commands.send(Command::Refresh).unwrap();
        drop(commands);
        worker.run(&inbox, |_| {});
        assert_eq!(restores(&log), 1, "restored once, not twice");
    }

    // The clock's time zone, on a reader of the test's own: the process's
    // real zone is never changed.

    /// The test's system zone, in hours east of UTC.
    static EAST: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(0);
    /// How many times the test's zone was read.
    static READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Reads the test's system zone: [`EAST`] of UTC.
    fn test_zone() -> TimeZone {
        use std::sync::atomic::Ordering;
        READS.fetch_add(1, Ordering::SeqCst);
        TimeZone::fixed(jiff::tz::offset(EAST.load(Ordering::SeqCst)))
    }

    #[test]
    fn the_clock_keeps_its_zone_until_macos_drops_it_or_five_minutes_pass() {
        use std::sync::atomic::Ordering;
        let reads = || READS.load(Ordering::SeqCst);
        let move_east = |hours| EAST.store(hours, Ordering::SeqCst);
        let zone = Zone::new(test_zone);
        let noon_utc: jiff::Timestamp = "2026-10-23T12:00:00Z".parse().unwrap();
        let minute = |now| minute_of_day(noon_utc, &zone.get(now));
        let t0 = Instant::now();

        assert!(!zone.is_kept());
        assert_eq!(minute(t0), 12.0 * 60.0);
        assert!(zone.is_kept());
        // The system moves three hours east, with no notice yet: the zone
        // kept is read from, and the system's is not read again.
        move_east(3);
        assert_eq!(minute(t0 + secs(1)), 12.0 * 60.0);
        assert_eq!(reads(), 1, "a kept zone read again");

        // macOS's notice drops it: the next read has the new zone.
        zone.forget();
        assert!(!zone.is_kept());
        assert_eq!(minute(t0 + secs(2)), 15.0 * 60.0);
        assert_eq!(reads(), 2);

        // A change with no notice shows after five minutes, as jiff's own.
        move_east(5);
        let read = t0 + secs(2);
        assert_eq!(minute(read + ZONE_KEPT - millis(1)), 15.0 * 60.0);
        assert_eq!(reads(), 2);
        assert_eq!(minute(read + ZONE_KEPT), 17.0 * 60.0);
        assert_eq!(reads(), 3);
    }

    #[test]
    fn the_time_of_day_is_read_in_the_zone() {
        let at: jiff::Timestamp = "2026-10-23T22:45:30Z".parse().unwrap();
        let utc = minute_of_day(at, &TimeZone::UTC);
        assert_eq!(utc, 22.0 * 60.0 + 45.5);
        // Two hours east it is past midnight.
        let east = minute_of_day(at, &TimeZone::fixed(jiff::tz::offset(2)));
        assert_eq!(east, 45.5);
    }

    #[test]
    fn the_time_of_day_keeps_the_fraction_of_the_second() {
        // The window waits for the turn of the minute by this clock: cut to
        // whole seconds, it would read 12:00:59 until 12:01:00 and land the
        // pass up to a second late.
        let near = |at: &str| {
            let at: jiff::Timestamp = at.parse().unwrap();
            minute_of_day(at, &TimeZone::UTC)
        };
        let close = |got: f64, wanted: f64, what: &str| {
            assert!(
                (got - wanted).abs() < 1e-9,
                "{what}: wanted {wanted}, got {got}"
            );
        };
        close(
            near("2026-10-23T22:45:30.25Z"),
            22.0 * 60.0 + 45.0 + 30.25 / 60.0,
            "a quarter of a second past 22:45:30",
        );
        close(
            near("2026-10-23T12:00:59.999Z"),
            12.0 * 60.0 + 59.999 / 60.0,
            "a millisecond before the minute",
        );
        // A nanosecond before the minute is within a microsecond of it; cut
        // to whole seconds, it would be a full second short.
        let next = 12.0 * 60.0 + 1.0;
        let hair = near("2026-10-23T12:00:59.999999999Z");
        assert!(hair < next, "a nanosecond before: {hair}");
        assert!(next - hair < 1e-6 / 60.0, "a nanosecond before: {hair}");
        // On the second itself, as before.
        close(
            near("2026-10-23T12:01:00Z"),
            12.0 * 60.0 + 1.0,
            "on the minute",
        );
    }

    #[test]
    fn the_zone_read_from_the_link_is_the_one_jiff_reads() {
        // On a Mac, `/etc/localtime` links into a zoneinfo folder.
        if cfg!(target_os = "macos") && std::env::var_os("TZ").is_none() {
            assert!(linked_zone().is_some(), "/etc/localtime names no zone");
        }
        let now = jiff::Timestamp::now();
        assert_eq!(
            system_zone().to_offset(now),
            TimeZone::system().to_offset(now)
        );
    }

    // Fade pacing, on synthetic time. A fade is driven the way
    // the engine thread drives it: each tick at the time the last one asked
    // for, with the clock moved to match.

    /// One tick of a [`Drive`].
    #[derive(Clone, Copy, Debug)]
    struct Beat {
        /// When the tick ran, from the drive's start.
        at: Duration,
        /// The wait it returned.
        wait: Duration,
        /// The tint it put on the displays, if it did.
        painted: Option<Tint>,
        /// Whether it gave the displays back.
        restored: bool,
        /// Whether it asked the displays if their tables were intact.
        looked: bool,
        /// The status it sent, if it sent one.
        status: Option<Status>,
    }

    /// A worker ticked on a clock made of the ticks' own times: the dial and
    /// the `Instant` move together, as the walk through the curve predicts
    /// from the clock it read.
    struct Drive {
        worker: Worker,
        log: Arc<Mutex<Log>>,
        look: Look,
        schedule: Schedule,
        t0: Instant,
        /// The minute of the day at `t0`.
        start: f64,
        /// When the next tick runs, from `t0`.
        at: Duration,
        /// What the next tick passes as the time since the last one.
        elapsed: Duration,
        /// The tint on the displays, as the screen was last told.
        shown: Tint,
    }

    impl Drive {
        /// A worker on `settings` that starts at `start`, a minute of the
        /// day. Its first tick, which has `SETTLES` behind it, takes the
        /// schedule's level at once.
        fn new(settings: Settings, start: f64) -> Self {
            DIAL.set(start);
            let log = Arc::new(Mutex::new(Log::default()));
            let (look, schedule) = (settings.night, settings.schedule());
            let worker = Worker::new(Box::new(FakeScreen(log.clone())), settings, dial);
            Self {
                worker,
                log,
                look,
                schedule,
                t0: Instant::now(),
                start,
                at: Duration::ZERO,
                elapsed: SETTLES,
                shown: Tint::NEUTRAL,
            }
        }

        /// The minute of the day `at` after the start, which may pass
        /// midnight.
        fn minute(&self, at: Duration) -> f64 {
            (self.start + at.as_secs_f64() / 60.0).rem_euclid(24.0 * 60.0)
        }

        /// The tint the schedule asks for `at` after the start.
        fn wanted(&self, at: Duration) -> Tint {
            self.look.at_level(self.schedule.level(self.minute(at)))
        }

        /// How far the tint on the displays is from the one wanted `at`
        /// after the start.
        fn lag(&self, at: Duration) -> f32 {
            self.wanted(at).distance(self.shown)
        }

        /// One tick at `at` after the start, `elapsed` after the last.
        fn tick_at(&mut self, at: Duration, elapsed: Duration) -> Beat {
            DIAL.set(self.minute(at));
            let (applied, restored, looked) = {
                let log = self.log.lock().unwrap();
                (log.applied.len(), log.restores, log.checks)
            };
            let mut status = None;
            let wait = self
                .worker
                .tick_at(self.t0 + at, elapsed, &mut |event| match event {
                    Event::Status(sent) => status = Some(sent),
                    Event::Error(error) => panic!("an error: {error}"),
                });
            let log = self.log.lock().unwrap();
            let painted = log.applied.get(applied).copied();
            if let Some(tint) = painted {
                self.shown = tint;
            }
            let restored = log.restores > restored;
            if restored {
                self.shown = Tint::NEUTRAL;
            }
            Beat {
                at,
                wait,
                painted,
                restored,
                looked: log.checks > looked,
                status,
            }
        }

        /// The next tick, when the last one asked for it.
        fn tick(&mut self) -> Beat {
            let beat = self.tick_at(self.at, self.elapsed);
            self.at += beat.wait;
            self.elapsed = beat.wait;
            beat
        }

        /// Ticks until the next one is later than `length` after the start.
        fn run(&mut self, length: Duration) -> Vec<Beat> {
            let mut beats = Vec::new();
            while self.at <= length {
                beats.push(self.tick());
            }
            beats
        }
    }

    /// The default look's night from 21:00, its fade `fade` minutes long.
    fn fade_of(fade: u16) -> Settings {
        Settings {
            fade_minutes: fade,
            ..Settings::default()
        }
    }

    const EVENING: f64 = 21.0 * 60.0;

    fn painting(beats: &[Beat]) -> Vec<Beat> {
        beats
            .iter()
            .copied()
            .filter(|beat| beat.painted.is_some())
            .collect()
    }

    fn minutes(n: u64) -> Duration {
        secs(n * 60)
    }

    /// The tick that ended a fade: the last that painted or restored.
    fn ending(beats: &[Beat]) -> Beat {
        *beats
            .iter()
            .rev()
            .find(|beat| beat.painted.is_some() || beat.restored)
            .expect("a tick that painted or restored")
    }

    /// A fade that ends `end` after the start ended then: not before, and
    /// late by no more than the precision of the wait.
    fn assert_on_time(beat: &Beat, end: Duration) {
        assert!(
            beat.at >= end && beat.at - end <= STEP_PRECISION,
            "the fade ends {end:?} in, and its last tick ran {:?} in",
            beat.at
        );
    }

    /// Along the black-body curve green always stays above blue; a straight
    /// line from white to red would keep them equal (pink).
    fn assert_orange_not_pink(tints: &[Tint]) {
        for tint in tints {
            assert!(tint.green >= tint.blue, "{tint:?}");
            if tint.green < 0.9 && tint.green > 0.1 {
                assert!(tint.green - tint.blue > 0.05, "{tint:?}");
            }
        }
    }

    /// A paint that comes more than `FADE_STEP` after the last waited for
    /// the curve, so the tint was less than a table step from the last one
    /// until the moment before it came: no step is slept through. The fade's
    /// own end waits for nothing, and is left out. Returns how many paints
    /// waited.
    fn assert_no_step_is_slept_through(drive: &Drive, beats: &[Beat], end: Duration) -> usize {
        let mut waited = 0;
        for pair in painting(beats).windows(2) {
            let (from, to) = (pair[0].at, pair[1].at);
            if to - from <= FADE_STEP || to + STEP_PRECISION >= end {
                continue;
            }
            waited += 1;
            let shown = pair[0].painted.expect("a paint");
            let early = to - 2 * STEP_PRECISION;
            let mut at = from;
            while at < early {
                let lag = drive.wanted(at).distance(shown);
                assert!(lag < TABLE_STEP, "a step came {at:?} in, painted at {to:?}");
                at += millis(250);
            }
            let lag = drive.wanted(early).distance(shown);
            assert!(lag < TABLE_STEP, "a step came before {early:?}, not {to:?}");
        }
        waited
    }

    #[test]
    fn a_twenty_minute_fade_paints_once_for_each_new_table_not_each_second() {
        let mut drive = Drive::new(fade_of(20), EVENING);
        let end = minutes(20);
        let beats = drive.run(end + secs(5));
        let paints = painting(&beats).len();
        // A step a second made 1,201 paints, of which 1,037 were new tables.
        assert!(paints <= 1040, "{paints} paints");
        assert!(beats.len() < 1100, "{} ticks", beats.len());
        let waited = assert_no_step_is_slept_through(&drive, &beats, end);
        assert!(waited > 0, "the default look has slow parts");
    }

    #[test]
    fn each_paint_of_a_fade_moves_a_channel_a_table_step() {
        for (what, settings) in [
            ("default look", fade_of(20)),
            ("pure red", night_settings()),
        ] {
            let mut drive = Drive::new(settings, EVENING);
            drive.run(minutes(20) + secs(5));
            let tints = drive.log.lock().unwrap().applied.clone();
            // The last paint lands the fade's end and may be closer.
            let (last, before_it) = tints.split_last().expect("paints");
            assert_eq!(*last, drive.look.tint(), "{what}: the end");
            let mut shown = Tint::NEUTRAL;
            for tint in before_it {
                let step = tint.distance(shown);
                assert!(step >= TABLE_STEP, "{what}: {tint:?} after {shown:?}");
                shown = *tint;
            }
        }
    }

    #[test]
    fn a_paced_fade_still_passes_through_orange_not_pink() {
        for (what, settings) in [
            ("default look", fade_of(20)),
            ("two hours", fade_of(120)),
            ("pure red", night_settings()),
        ] {
            let mut drive = Drive::new(settings, EVENING);
            drive.run(minutes(120) + secs(5));
            let tints = drive.log.lock().unwrap().applied.clone();
            assert!(tints.len() > 100, "{what}: {} paints", tints.len());
            assert_orange_not_pink(&tints);
        }
    }

    #[test]
    fn a_fade_in_lands_its_full_look_on_time_and_then_only_checks() {
        let mut drive = Drive::new(fade_of(20), EVENING);
        let end = minutes(20);
        let beats = drive.run(end + secs(5));
        let last = ending(&beats);
        assert_eq!(last.painted, Some(drive.look.tint()), "the full look");
        assert_on_time(&last, end);
        let status = last.status.expect("a status for the last paint");
        assert_eq!((status.level, status.tint), (1.0, drive.look.tint()));
        assert_eq!(last.wait, CHECK_FIRST, "the paint's check, not a step");
        // After it nothing is painted, and no sleep is longer than the
        // schedule's.
        let after: Vec<_> = beats.iter().filter(|beat| beat.at > last.at).collect();
        assert!(!after.is_empty() && after.iter().all(|beat| beat.looked));
        assert!(
            after
                .iter()
                .all(|beat| beat.painted.is_none() && !beat.restored)
        );
        assert!(after.iter().all(|beat| beat.wait <= MAX_SLEEP));
    }

    #[test]
    fn a_fade_out_restores_on_time_and_then_sleeps_a_minute() {
        // From 07:00 the fade runs back to normal colors at 07:20.
        let mut drive = Drive::new(fade_of(20), 7.0 * 60.0);
        let end = minutes(20);
        let beats = drive.run(end + secs(5));
        let last = ending(&beats);
        assert!(last.restored && last.painted.is_none(), "{last:?}");
        assert_on_time(&last, end);
        let status = last.status.expect("a status for the restore");
        assert_eq!(status.level, 0.0);
        assert!(status.tint.is_neutral());
        assert_eq!(last.wait, MAX_SLEEP, "a neutral screen by day");
        assert_eq!(restores(&drive.log), 1, "one restore, at the end");
        // Every paint on the way down is a table step from the one before.
        let tints = drive.log.lock().unwrap().applied.clone();
        let mut shown = drive.look.tint();
        for tint in &tints[1..] {
            assert!(
                tint.distance(shown) >= TABLE_STEP,
                "{tint:?} after {shown:?}"
            );
            shown = *tint;
        }
        assert!(tints.len() <= 1040, "{} paints", tints.len());
    }

    #[test]
    fn a_fade_starts_unpainted_until_a_tint_is_a_table_step_from_neutral() {
        // 6400 K moves the tint about eleven table steps in 20 minutes: the
        // first comes after about 110 s, long after the check that
        // `Worker::new` set up for 2 s on.
        let settings = Settings {
            night: Look {
                kelvin: 6400,
                brightness: 100,
            },
            ..fade_of(20)
        };
        let mut drive = Drive::new(settings, EVENING);
        let beats = drive.run(secs(300));
        let first = beats
            .iter()
            .position(|beat| beat.painted.is_some())
            .expect("a first paint");
        assert!(beats[first].at > 10 * CHECK_FIRST, "{:?}", beats[first].at);
        let paint = beats[first].painted.unwrap();
        assert!(paint.distance(Tint::NEUTRAL) >= TABLE_STEP, "{paint:?}");
        for beat in &beats[..first] {
            assert!(!beat.restored, "nothing to give back: {beat:?}");
            assert!(!beat.looked, "the screen shown is neutral: {beat:?}");
            assert!(beat.wait <= MAX_SLEEP);
        }
        assert_eq!(looks(&drive.log), beats.iter().filter(|b| b.looked).count());
        // Held ticks send no status: the first tick's neutral one stands.
        assert!(beats[0].status.is_some_and(|status| status.level == 0.0));
        assert!(beats[1..first].iter().all(|beat| beat.status.is_none()));
        // The paint starts the checks over.
        assert_eq!(beats[first].wait, CHECK_FIRST);
    }

    #[test]
    fn a_two_hour_fade_paints_far_less_than_a_step_a_second_and_backs_off_between() {
        let mut drive = Drive::new(fade_of(120), EVENING);
        let end = minutes(120);
        let beats = drive.run(end + secs(5));
        let paints = painting(&beats);
        // A step a second made 7,201 paints.
        assert!(paints.len() <= 1260, "{} paints", paints.len());
        assert!(beats.len() < 2700, "{} ticks", beats.len());
        assert!(assert_no_step_is_slept_through(&drive, &beats, end) > 100);
        assert_on_time(&ending(&beats), end);
        // Between two paints more than 2 s apart the checks come 2 s after
        // the first, and then 4 s after that check, as the backoff says.
        let looked_at = |at: Duration| beats.iter().any(|beat| beat.at == at && beat.looked);
        let mut backed_off = 0;
        for pair in paints.windows(2) {
            let (from, to) = (pair[0].at, pair[1].at);
            if to - from > CHECK_FIRST {
                assert!(looked_at(from + CHECK_FIRST), "no check 2 s after {from:?}");
            }
            if to - from > 3 * CHECK_FIRST {
                assert!(
                    looked_at(from + 3 * CHECK_FIRST),
                    "no check 6 s after {from:?}"
                );
                backed_off += 1;
            }
        }
        assert!(backed_off > 0, "no two paints were 6 s apart");
    }

    #[test]
    fn fades_across_midnight_pace_like_the_others_and_end_on_time() {
        let end = minutes(20);
        // The default night from 23:50: the fade passes midnight.
        let across = Settings {
            start: ClockTime::new(23, 50),
            ..fade_of(20)
        };
        let mut drive = Drive::new(across, 23.0 * 60.0 + 50.0);
        let beats = drive.run(end + secs(5));
        let mut evening_drive = Drive::new(fade_of(20), EVENING);
        let evening_beats = evening_drive.run(end + secs(5));
        let (paints, evening_paints) = (painting(&beats).len(), painting(&evening_beats).len());
        assert!(paints <= 1040, "{paints} paints");
        assert!(
            paints.abs_diff(evening_paints) <= 2,
            "{paints}, {evening_paints}"
        );
        assert_on_time(&ending(&beats), end);
        assert_eq!(ending(&beats).painted, Some(drive.look.tint()));
        assert!(assert_no_step_is_slept_through(&drive, &beats, end) > 0);
        assert_orange_not_pink(&drive.log.lock().unwrap().applied);

        // A window that ends at 23:50 fades out across midnight, and one
        // that ends at 00:10 fades out after it.
        for (what, window_end, start) in [
            (
                "across midnight",
                ClockTime::new(23, 50),
                23.0 * 60.0 + 50.0,
            ),
            ("after midnight", ClockTime::new(0, 10), 10.0),
        ] {
            let settings = Settings {
                start: ClockTime::new(17, 0),
                end: window_end,
                ..fade_of(20)
            };
            let mut drive = Drive::new(settings, start);
            let beats = drive.run(end + secs(5));
            let last = ending(&beats);
            assert!(last.restored, "{what}: {last:?}");
            assert_on_time(&last, end);
            let paints = painting(&beats).len();
            assert!(paints <= 1040, "{what}: {paints} paints");
            assert!(assert_no_step_is_slept_through(&drive, &beats, end) > 0);
        }
    }

    #[test]
    fn a_window_no_longer_than_its_fade_turns_back_without_a_stuck_step() {
        // 21:00 to 21:20 with a fade of 20 minutes: the level rises to 1 at
        // 21:20, turns back at once, and is down at 21:40.
        let settings = Settings {
            end: ClockTime::new(21, 20),
            ..fade_of(20)
        };
        let mut drive = Drive::new(settings, EVENING);
        let end = minutes(40);
        let beats = drive.run(end + secs(5));
        let last = ending(&beats);
        assert!(last.restored, "{last:?}");
        assert_on_time(&last, end);
        assert!(assert_no_step_is_slept_through(&drive, &beats, end) > 0);
        // The tint on the displays is never a table step behind the curve
        // at a moment the engine looks, the turn included.
        let mut shown = Tint::NEUTRAL;
        for beat in &beats {
            if let Some(tint) = beat.painted {
                shown = tint;
            }
            if beat.restored {
                shown = Tint::NEUTRAL;
            }
            let lag = drive.wanted(beat.at).distance(shown);
            assert!(lag < TABLE_STEP, "{:?} behind at {:?}", lag, beat.at);
        }
        // It rose to within a table step of the full look, which a step
        // held at the top may leave unpainted, and came back down.
        let tints = drive.log.lock().unwrap().applied.clone();
        let full = drive.look.tint().green;
        let top = tints.iter().map(|tint| tint.green).fold(1.0_f32, f32::min);
        assert!(top >= full && top - full < TABLE_STEP, "{top} for {full}");
        assert!(tints.last().unwrap().green > top, "{:?}", tints.last());
    }

    #[test]
    fn a_fast_fade_never_paints_twice_inside_a_second_but_for_its_end() {
        // Pure red moves its tint several table steps a second, all through
        // the fade: it paints every second, to the end. The default look
        // has slow parts, and is only held to the second.
        for (what, settings, start, fast) in [
            ("pure red in", night_settings(), EVENING, true),
            ("default look in", fade_of(20), EVENING, false),
            ("pure red out", night_settings(), 7.0 * 60.0, true),
        ] {
            // 0.4 s into the fade, so that its end falls inside a second
            // that a paint every second leaves.
            let start = start + 0.4 / 60.0;
            let mut drive = Drive::new(settings, start);
            let end = minutes(20) - millis(400);
            let beats = drive.run(end + secs(5));
            let last = ending(&beats);
            assert_on_time(&last, end);
            let paints: Vec<_> = painting(&beats)
                .into_iter()
                .filter(|beat| beat.at < last.at)
                .collect();
            for pair in paints.windows(2) {
                let gap = pair[1].at - pair[0].at;
                assert!(gap >= FADE_STEP, "{what}: {gap:?} between paints");
            }
            // The end does not wait for the second: it comes inside it.
            let before = paints.last().expect("paints before the end").at;
            assert!(last.at > before);
            if fast {
                assert!(
                    last.at - before < FADE_STEP,
                    "{what}: {:?}",
                    last.at - before
                );
            }
        }
    }

    #[test]
    fn a_slow_part_waits_for_its_table_step_and_paints_when_it_comes() {
        let Slow {
            mut drive,
            paint,
            step,
        } = slow();
        let gap = step - paint.at;
        assert!(gap > FADE_STEP, "{gap:?}");
        // Ticked as the engine ticks it, the next paint comes at the step.
        while drive.at < step {
            let beat = drive.tick();
            assert!(beat.painted.is_none(), "painted early: {beat:?}");
        }
        let beat = drive.tick();
        assert_eq!(beat.at, step);
        let tint = beat.painted.expect("the step is painted");
        assert!(tint.distance(paint.painted.unwrap()) >= TABLE_STEP);
    }

    #[test]
    fn a_wake_a_little_early_waits_the_rest_and_not_a_second() {
        // A wall clock slewed against the monotonic one can wake the engine
        // just before the table step it slept for.
        let Slow {
            mut drive, step, ..
        } = slow();
        while drive.at < step {
            drive.tick();
        }
        assert_eq!(drive.at, step);
        let last = drive.at - drive.elapsed;
        let mut early = step;
        while drive.lag(early) >= TABLE_STEP {
            early -= Duration::from_micros(100);
        }
        assert!(step - early <= 2 * STEP_PRECISION, "{:?}", step - early);

        let held = drive.tick_at(early, early - last);
        assert!(held.painted.is_none() && held.status.is_none() && !held.restored);
        assert!(!held.looked, "no check was due");
        // The floor counts from the last paint, which is long ago: the rest
        // of the step, not a second.
        assert!(held.wait > Duration::ZERO && held.wait <= 2 * STEP_PRECISION);
        assert_ne!(held.wait, FADE_STEP);
        let painted = drive.tick_at(early + held.wait, held.wait);
        assert!(painted.painted.is_some(), "{painted:?}");
    }

    #[test]
    fn the_wait_for_a_table_step_stops_at_its_edges() {
        let minute = EVENING + 5.0;
        // Nothing moves for a minute: a look of the day itself stays
        // neutral, and the fade is a quarter of an hour from its end.
        let day = Settings {
            night: Look::DAY,
            ..fade_of(20)
        };
        let drive = Drive::new(day, EVENING);
        assert_eq!(drive.worker.next_table_step(minute, FADE_STEP), MAX_SLEEP);
        assert_eq!(
            drive.worker.next_table_step(minute, Duration::ZERO),
            MAX_SLEEP
        );

        // The fade's end comes before the floor: 0.4 s away with a 1 s
        // floor, the wait is 0.4 s, late by no more than the precision.
        let drive = Drive::new(night_settings(), EVENING);
        let schedule = drive.schedule;
        let fading = |minute: f64| schedule.minutes_until_change(minute) == 0.0;
        let near_the_end = EVENING + 20.0 - 0.4 / 60.0;
        let wait = drive.worker.next_table_step(near_the_end, FADE_STEP);
        let minute_after = |wait: Duration| near_the_end + wait.as_secs_f64() / 60.0;
        assert!(!fading(minute_after(wait)), "{wait:?} is early");
        assert!(
            fading(minute_after(wait - 2 * STEP_PRECISION)),
            "{wait:?} is late"
        );
        assert!(wait > millis(399) && wait <= millis(401), "{wait:?}");

        // The tint has moved a table step already: the floor itself, late
        // by no more than the precision.
        for floor in [millis(600), millis(1), Duration::ZERO] {
            let wait = drive.worker.next_table_step(minute, floor);
            assert!(wait >= floor, "{wait:?} is before the floor {floor:?}");
            assert!(
                wait <= floor + STEP_PRECISION,
                "{wait:?}, the floor {floor:?}"
            );
        }
    }

    /// A drive of the default look's two-hour fade, stopped after the paint
    /// that its longest wait for a table step follows: a slow part, where
    /// the next step is many seconds away.
    struct Slow {
        drive: Drive,
        /// The paint it stopped at.
        paint: Beat,
        /// When the next table step comes, from the start.
        step: Duration,
    }

    fn slow() -> Slow {
        let mut reference = Drive::new(fade_of(120), EVENING);
        let paints = painting(&reference.run(minutes(120) + secs(5)));
        let (paint, step) = paints
            .windows(2)
            .map(|pair| (pair[0], pair[1].at))
            .max_by_key(|(paint, step)| *step - paint.at)
            .expect("paints");
        assert!(step - paint.at >= secs(8), "{:?}", step - paint.at);
        let mut drive = Drive::new(fade_of(120), EVENING);
        while drive.at <= paint.at {
            drive.tick();
        }
        Slow { drive, paint, step }
    }

    #[test]
    fn a_refresh_mid_sleep_looks_at_once_and_waits_for_the_soonest_of_three() {
        // Half a second after a paint, with the next step far off: the
        // recheck, a second on, is the soonest.
        let Slow {
            mut drive, paint, ..
        } = slow();
        let looked = looks(&drive.log);
        drive.worker.handle(Command::Refresh);
        let beat = drive.tick_at(paint.at + millis(500), millis(500));
        assert!(beat.looked && looks(&drive.log) == looked + 1);
        assert!(
            beat.painted.is_none(),
            "the tint has not moved a table step"
        );
        assert_eq!(beat.wait, RECHECK);
        let again = drive.tick_at(paint.at + millis(1500), RECHECK);
        assert!(again.looked && again.painted.is_none());

        // Three tenths of a second before the step: the step is the soonest.
        let Slow {
            mut drive,
            paint,
            step,
        } = slow();
        drive.worker.handle(Command::Refresh);
        let near = step - millis(300);
        let beat = drive.tick_at(near, near - paint.at);
        assert!(beat.looked && beat.painted.is_none());
        assert!(
            beat.wait >= millis(299) && beat.wait <= millis(301),
            "{:?}",
            beat.wait
        );
        let painted = drive.tick_at(near + beat.wait, beat.wait);
        assert!(painted.painted.is_some(), "the step comes");

        // The step has come already, and the refresh paints it: no look,
        // the paint says what a look would, and the recheck still follows.
        let Slow {
            mut drive,
            paint,
            step,
        } = slow();
        let looked = looks(&drive.log);
        drive.worker.handle(Command::Refresh);
        let late = step + millis(500);
        let beat = drive.tick_at(late, late - paint.at);
        assert!(beat.painted.is_some() && !beat.looked);
        assert_eq!(looks(&drive.log), looked);
        assert_eq!(beat.wait, RECHECK);
        let again = drive.tick_at(late + RECHECK, RECHECK);
        assert!(again.looked, "the recheck a second after the refresh");
    }

    #[test]
    fn a_preview_mid_sleep_paints_at_once_and_letting_go_fades_back_to_the_schedule() {
        let Slow {
            mut drive, paint, ..
        } = slow();
        let look = held();
        drive.worker.handle(Command::Preview(Some(look)));
        let mut at = paint.at + millis(500);
        let beat = drive.tick_at(at, millis(500));
        assert_eq!(beat.painted, Some(look.tint()));
        let status = beat.status.expect("a status");
        assert_eq!((status.level, status.tint), (1.0, look.tint()));

        // Let go: frames of a quick fade back down, and the fade's own tint
        // at the end, painted, with its level.
        drive.worker.handle(Command::Preview(None));
        let mut frames = 0;
        let end = loop {
            at += ANIMATION_FRAME;
            let beat = drive.tick_at(at, ANIMATION_FRAME);
            if beat.wait != ANIMATION_FRAME {
                break beat;
            }
            assert!(beat.painted.is_some(), "a frame of the fade back");
            frames += 1;
            assert!(frames < 200, "the fade back does not end");
        };
        let level = drive.schedule.level(drive.minute(at));
        assert_eq!(end.painted, Some(drive.look.at_level(level)));
        assert_eq!(end.status.map(|status| status.level), Some(level));
        assert!(end.wait > ANIMATION_FRAME);
    }

    #[test]
    fn red_now_mid_fade_fades_up_then_back_to_the_schedules_level() {
        let Slow {
            mut drive, paint, ..
        } = slow();
        drive.worker.handle(Command::RedNow(true));
        let mut at = paint.at + millis(500);
        let mut up = Vec::new();
        let end = loop {
            at += ANIMATION_FRAME;
            let beat = drive.tick_at(at, ANIMATION_FRAME);
            if beat.wait != ANIMATION_FRAME {
                break beat;
            }
            up.push(beat);
            assert!(up.len() < 200, "Red Now does not land");
        };
        assert!(up.iter().all(|beat| beat.painted.is_some()));
        assert_eq!(end.painted, Some(drive.look.tint()), "the full look");
        assert_eq!(end.status.map(|status| status.level), Some(1.0));
        // Then the displays are checked, as for any tint, with no step to
        // wait for.
        assert_eq!(end.wait, CHECK_FIRST);

        // Off again: back down to wherever the fade is, painted there.
        drive.worker.handle(Command::RedNow(false));
        let mut frames = 0;
        let end = loop {
            at += ANIMATION_FRAME;
            let beat = drive.tick_at(at, ANIMATION_FRAME);
            if beat.wait != ANIMATION_FRAME {
                break beat;
            }
            frames += 1;
            assert!(frames < 200, "the fade back does not end");
        };
        let level = drive.schedule.level(drive.minute(at));
        assert_eq!(end.painted, Some(drive.look.at_level(level)));
        assert_eq!(end.status.map(|status| status.level), Some(level));
        assert!(frames > 0, "a quick fade, not a jump");
    }

    #[test]
    fn a_new_look_mid_sleep_is_painted_at_once_and_the_same_one_is_held() {
        let Slow {
            mut drive, paint, ..
        } = slow();
        // The same settings change nothing a table step wide.
        drive.worker.handle(Command::Configure(fade_of(120)));
        let beat = drive.tick_at(paint.at + millis(500), millis(500));
        assert!(beat.painted.is_none() && beat.status.is_none() && !beat.restored);

        let other = Settings {
            night: Look {
                kelvin: 2000,
                brightness: 100,
            },
            ..fade_of(120)
        };
        drive.worker.handle(Command::Configure(other.clone()));
        let beat = drive.tick_at(paint.at + secs(1), millis(500));
        let status = beat.status.expect("a status for the new look");
        assert_eq!(beat.painted, Some(other.night.at_level(status.level)));
        let distance = beat.painted.unwrap().distance(paint.painted.unwrap());
        assert!(distance >= TABLE_STEP, "{distance}");
    }

    #[test]
    fn a_quick_fades_landing_in_a_scheduled_fade_is_painted_however_small() {
        // A worker that starts at 21:05, a quarter into the fade, fades up
        // to the schedule's 0.25 at the quick fade's pace, and the last
        // step may be smaller than a table step.
        let target = night_settings().schedule().level(mid_fade());
        assert_eq!(target, 0.25);
        let mut sub_step = 0;
        for first in [0.1, 0.2999, 0.29999, 0.3] {
            DIAL.set(mid_fade());
            let (mut worker, log) = worker(dial);
            let t0 = Instant::now();
            let mut statuses = Vec::new();
            let mut tick = |worker: &mut Worker, at: Duration, elapsed: Duration| {
                worker.tick_at(t0 + at, elapsed, &mut |event| {
                    if let Event::Status(status) = event {
                        statuses.push(status);
                    }
                })
            };
            let mut at = Duration::ZERO;
            let mut wait = tick(&mut worker, at, Duration::from_secs_f64(first));
            let mut ticks = 1;
            while wait == ANIMATION_FRAME {
                at += ANIMATION_FRAME;
                wait = tick(&mut worker, at, ANIMATION_FRAME);
                ticks += 1;
                assert!(ticks < 20, "{first}: the fade up does not end");
            }
            assert_eq!(paints(&log), ticks, "{first}: every tick of it paints");
            let tints = log.lock().unwrap().applied.clone();
            let last_step = tints
                .windows(2)
                .last()
                .map(|pair| pair[1].distance(pair[0]));
            if last_step.is_some_and(|step| step < TABLE_STEP) {
                sub_step += 1;
            }
            let night = night_settings().night;
            let last = tints.last().copied();
            assert_eq!(last, Some(night.at_level(target)), "{first}: the level");
            assert_eq!(statuses.last().map(|status| status.level), Some(target));
        }
        assert!(
            sub_step >= 2,
            "{sub_step} landings smaller than a table step"
        );
    }

    #[test]
    fn a_landing_smaller_than_a_table_step_is_painted_when_a_preview_ends() {
        // The same in a slow part, where the quick fade back from a preview
        // ends a hair above the level the schedule asks for.
        let Slow {
            mut drive, paint, ..
        } = slow();
        drive.worker.handle(Command::Preview(Some(held())));
        let mut at = paint.at + millis(500);
        drive.tick_at(at, millis(500));
        drive.worker.handle(Command::Preview(None));
        let level = drive.schedule.level(drive.minute(at + ANIMATION_FRAME));
        let step = (1.0 - f64::from(level) - 1e-5) * QUICK_FADE.as_secs_f64();
        at += ANIMATION_FRAME;
        let first = drive.tick_at(at, Duration::from_secs_f64(step));
        assert_eq!(first.wait, ANIMATION_FRAME);
        let first_tint = first.painted.expect("a frame of the fade back");
        at += ANIMATION_FRAME;
        let landing = drive.tick_at(at, ANIMATION_FRAME);
        let tint = landing.painted.expect("the landing is painted");
        assert!(tint.distance(first_tint) < TABLE_STEP, "a sub-step landing");
        let level = drive.schedule.level(drive.minute(at));
        assert_eq!(landing.status.map(|status| status.level), Some(level));
        assert_eq!(tint, drive.look.at_level(level));
    }

    #[test]
    fn the_startup_fade_by_night_asks_for_a_frame_every_33_ms_and_paints_each() {
        // The driver's `closed-night`: the clock starts at 23:30, in the
        // night, and the engine starts neutral. With the fade pacing the
        // driver painted this fade 14 times, against 34 before. The pacing
        // does not touch a quick fade: it still asks for a frame every
        // `ANIMATION_FRAME` and paints each, so the later frames come from the
        // real timer.
        DIAL.set(23.0 * 60.0 + 30.0);
        let (mut worker, log) = worker(dial);
        let t0 = Instant::now();
        let mut ticks = 1;
        let mut wait = worker.tick_at(t0, ANIMATION_FRAME, &mut |_| {});
        while wait == ANIMATION_FRAME {
            let at = t0 + ticks * ANIMATION_FRAME;
            wait = worker.tick_at(at, ANIMATION_FRAME, &mut |_| {});
            ticks += 1;
            assert!(ticks < 100, "the fade does not end");
        }
        // 1.2 s in frames of 33 ms.
        assert_eq!(paints(&log), ticks as usize, "a paint for each frame");
        assert!((36..=38).contains(&ticks), "{ticks} frames");
        let last = log.lock().unwrap().applied.last().copied();
        assert_eq!(last, Some(night_settings().night.tint()));
        assert_eq!(looks(&log), 0, "no check inside the fade");
    }

    #[test]
    fn a_held_tick_sends_no_status_and_every_status_carries_the_tint_it_shows() {
        let Slow {
            mut drive, paint, ..
        } = slow();
        let held = drive.tick_at(paint.at + millis(500), millis(500));
        assert!(held.painted.is_none() && held.status.is_none(), "{held:?}");

        for (what, settings, start, end) in [
            ("in", fade_of(120), EVENING, minutes(120)),
            ("out", fade_of(20), 7.0 * 60.0, minutes(20)),
        ] {
            let mut drive = Drive::new(settings, start);
            let beats = drive.run(end + secs(5));
            let mut last = None;
            for beat in &beats {
                let Some(status) = beat.status else { continue };
                // The level is the tint's, never the unpainted level.
                let level_tint = drive.look.at_level(status.level);
                assert!(
                    level_tint.distance(status.tint) < 1e-6,
                    "{what}: {status:?}"
                );
                if let Some(tint) = beat.painted {
                    assert_eq!(status.tint, tint, "{what}: the status of a paint");
                }
                // A status comes with a change, and the fade is behind its
                // level, never ahead: in, the level rises; out, it falls.
                assert_ne!(last, Some(status), "{what}: a status repeated");
                if let Some(Status { level, .. }) = last {
                    assert!(
                        (status.level >= level) == (what == "in"),
                        "{what}: {status:?}"
                    );
                }
                last = Some(status);
            }
        }
    }

    #[test]
    fn a_two_hour_fade_wakes_a_closed_app_no_oftener_than_the_level_pace() {
        let mut drive = Drive::new(fade_of(120), EVENING);
        let beats = drive.run(minutes(120) + secs(5));
        let mut pacer = Pacer::default();
        let mut statuses = Vec::new();
        let mut woken = Vec::new();
        for beat in &beats {
            let Some(status) = beat.status else { continue };
            statuses.push(beat.at);
            let reach = pacer.reach(&Event::Status(status), drive.t0 + beat.at);
            if reach == Reach::App {
                woken.push(beat.at);
            }
        }
        // The first status and the screen turning tinted wake the app
        // whenever they come; the rest only move the level.
        assert!(woken.len() > 3 && statuses.len() > 100);
        for pair in woken[2..].windows(2) {
            assert!(pair[1] - pair[0] >= LEVEL_PACE, "{pair:?}");
            // And the app sees the level move at the first paint that comes
            // after the pace: no later one.
            let next = statuses.iter().find(|at| **at >= pair[0] + LEVEL_PACE);
            assert_eq!(next, Some(&pair[1]), "woken at {:?}", pair[0]);
        }
        let seen = woken.len() as u64;
        assert!(
            seen <= 2 + 2 * 60 * 60 / LEVEL_PACE.as_secs() + 1,
            "{seen} wakes"
        );
    }

    /// The result, the class and the relative priority of `thread`. The
    /// class is read as a raw number: a value `libc::qos_class_t` does not
    /// list would be undefined behavior in that enum.
    #[cfg(target_os = "macos")]
    fn qos_of(thread: libc::pthread_t) -> (libc::c_int, u32, libc::c_int) {
        let (mut class, mut priority) = (0_u32, 0);
        // SAFETY: `thread` is a live pthread of this process; `class` and
        // `priority` are valid for writes, and `class` has the size of the
        // `repr(u32)` enum it is read through.
        let result = unsafe {
            libc::pthread_get_qos_class_np(
                thread,
                (&raw mut class).cast::<libc::qos_class_t>(),
                &mut priority,
            )
        };
        (result, class, priority)
    }

    /// The calling thread's QoS class, as a raw number, if it reads.
    #[cfg(target_os = "macos")]
    fn own_class() -> Option<u32> {
        // SAFETY: `pthread_self` takes no arguments and returns the calling
        // thread's own handle.
        let (result, class, _) = qos_of(unsafe { libc::pthread_self() });
        (result == 0).then_some(class)
    }

    #[cfg(target_os = "macos")]
    fn utility() -> Option<u32> {
        Some(libc::qos_class_t::QOS_CLASS_UTILITY as u32)
    }

    #[cfg(target_os = "macos")]
    fn initiated() -> Option<u32> {
        Some(libc::qos_class_t::QOS_CLASS_USER_INITIATED as u32)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_engine_thread_runs_at_utility_qos() {
        use std::os::unix::thread::JoinHandleExt;

        let (engine, log) = spawned(noon);
        // The class is set before the worker's first tick, which sends this.
        until_status(&engine, "the first status", |_| true);
        let thread = engine.thread.as_ref().expect("the engine's thread");
        let (result, class, priority) = qos_of(thread.as_pthread_t());
        assert_eq!(result, 0);
        assert_eq!(Some(class), utility(), "the engine thread's class");
        assert_eq!(priority, 0);
        // The read tells classes apart: a thread this test moves to another
        // class reads that one.
        let other = std::thread::spawn(|| {
            set_qos(true);
            own_class()
        });
        assert_eq!(other.join().unwrap(), initiated(), "the other thread");
        shuts_down_and_restores(engine, &log);
    }

    /// What the thread tests' clock reads, as the bits of an `f64` minute of
    /// the day: moved by hand.
    static SLOW_FADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn slow_fade() -> f64 {
        f64::from_bits(SLOW_FADE.load(std::sync::atomic::Ordering::SeqCst))
    }

    #[test]
    fn shutdown_restores_a_slow_fade_asleep_between_its_paints() {
        use std::sync::atomic::Ordering::SeqCst;
        // 6400 K in a two-hour fade: a table step takes about nine minutes
        // there, so the engine sleeps the length of its checks between
        // paints.
        let settings = Settings {
            night: Look {
                kelvin: 6400,
                brightness: 100,
            },
            ..fade_of(120)
        };
        let minute = EVENING + 60.0;
        let tint_at = |minute: f64| {
            let schedule = settings.schedule();
            settings.night.at_level(schedule.level(minute))
        };
        assert!(tint_at(minute).distance(Tint::NEUTRAL) >= TABLE_STEP);
        // A hundredth of a minute on, the curve is not a table step on.
        assert!(tint_at(minute + 0.01).distance(tint_at(minute)) < TABLE_STEP);

        // The engine starts neutral and fades up to the schedule's level
        // first, a frame at a time; it sleeps once it has landed there.
        let level = settings.schedule().level(minute);
        let landed = |status: &Status| status.level == level;

        // Asleep between two paints.
        SLOW_FADE.store(minute.to_bits(), SeqCst);
        let (engine, log) = spawned_on(settings.clone(), slow_fade);
        until_status(&engine, "the landing", landed);
        shuts_down_and_restores(engine, &log);

        // With a step held back: the fade's level has moved on, and the
        // tick that read it painted nothing.
        SLOW_FADE.store(minute.to_bits(), SeqCst);
        let (engine, log) = spawned_on(settings, slow_fade);
        until_status(&engine, "the landing", landed);
        SLOW_FADE.store((minute + 0.01).to_bits(), SeqCst);
        // The check's own timer may have looked already: wait for a look
        // after this count, which a tick at the new minute makes.
        let (painted, looked) = (paints(&log), looks(&log));
        engine.send(Command::Refresh);
        until("the refresh's look", || looks(&log) > looked);
        assert_eq!(paints(&log), painted, "the step was held");
        shuts_down_and_restores(engine, &log);
    }

    #[test]
    fn red_now_after_a_long_sleep_still_fades() {
        // By day the engine sleeps a minute, and `run` hands `tick` the time
        // since its last tick as `elapsed`. A Red Now that wakes it after a
        // sleep as long as a quick fade, or longer, then steps the whole
        // fade in one tick and jumps to red. This test fails until the fade
        // counts its time from its own start.
        //
        // The one real wait of these tests: `run` reads the real clock, and
        // a sleep of this length cannot return early, so `elapsed` is at
        // least the quick fade's whole length (a step of 1.0 or more).
        let (engine, log) = spawned(noon);
        until_status(&engine, "the first status", |_| true);
        std::thread::sleep(QUICK_FADE + millis(100));
        engine.send(Command::RedNow(true));
        let mut levels = Vec::new();
        until("the fade to red", || {
            levels.extend(engine.poll().into_iter().filter_map(|event| match event {
                Event::Status(status) => Some(status.level),
                Event::Error(_) => None,
            }));
            levels.last() == Some(&1.0)
        });
        let between = levels.iter().filter(|level| 0.0 < **level && **level < 1.0);
        assert!(between.count() >= 3, "Red Now jumped to red: {levels:?}");
        shuts_down_and_restores(engine, &log);
    }

    #[test]
    fn red_now_off_after_a_long_sleep_still_fades() {
        // The same the other way. Once Red Now has landed, the engine sleeps
        // until its first check, `CHECK_FIRST` on, and a Red Now off that
        // wakes it after a sleep longer than a quick fade used to jump back.
        let (engine, log) = spawned(noon);
        engine.send(Command::RedNow(true));
        until_status(&engine, "red", |status| status.level == 1.0);
        std::thread::sleep(QUICK_FADE + millis(100));
        engine.send(Command::RedNow(false));
        let mut levels = Vec::new();
        until("the fade back", || {
            levels.extend(engine.poll().into_iter().filter_map(|event| match event {
                Event::Status(status) => Some(status.level),
                Event::Error(_) => None,
            }));
            levels.last() == Some(&0.0)
        });
        let between = levels.iter().filter(|level| 0.0 < **level && **level < 1.0);
        assert!(between.count() >= 3, "Red Now off jumped back: {levels:?}");
        shuts_down_and_restores(engine, &log);
    }

    /// One wake of `worker`, `since` its last tick, as `run` makes it: the
    /// commands, then a tick. Returns the time the tick gave the quick fade,
    /// and the level of the status it sent.
    fn wake<const N: usize>(
        worker: &mut Worker,
        since: Duration,
        commands: [Command; N],
    ) -> (Duration, Option<f32>) {
        for command in commands {
            worker.handle(command);
        }
        let elapsed = worker.fade_time(since);
        let mut level = None;
        worker.tick(elapsed, &mut |event| {
            if let Event::Status(status) = event {
                level = Some(status.level);
            }
        });
        (elapsed, level)
    }

    #[test]
    fn a_wake_that_changes_the_aim_starts_a_quick_fade_one_frame_in() {
        // Each wake comes a minute after the last tick. One whose commands
        // change what the ticks aim at starts a quick fade at its own tick:
        // a frame's step, however long the sleep before it.
        let frame = ANIMATION_FRAME.as_secs_f32() / QUICK_FADE.as_secs_f32();
        DIAL.set(12.0 * 60.0);
        let (mut worker, _log) = worker(dial);
        settle(&mut worker);
        let on = wake(&mut worker, MAX_SLEEP, [Command::RedNow(true)]);
        assert_eq!(on, (ANIMATION_FRAME, Some(frame)), "Red Now on");
        // While the fade runs, its frames are the ticks, a command or not.
        let off = wake(&mut worker, millis(10), [Command::RedNow(false)]);
        assert_eq!(off.0, millis(10), "Red Now off mid-fade");
        settle(&mut worker);

        wake(&mut worker, MAX_SLEEP, [Command::RedNow(true)]);
        settle(&mut worker);
        let off = wake(&mut worker, MAX_SLEEP, [Command::RedNow(false)]);
        assert_eq!(off, (ANIMATION_FRAME, Some(1.0 - frame)), "Red Now off");
        settle(&mut worker);

        let preview = wake(&mut worker, MAX_SLEEP, [Command::Preview(Some(held()))]);
        assert_eq!(preview.1, Some(1.0), "a preview paints at once");
        let let_go = wake(&mut worker, MAX_SLEEP, [Command::Preview(None)]);
        assert_eq!(
            let_go,
            (ANIMATION_FRAME, Some(1.0 - frame)),
            "a preview let go"
        );
        settle(&mut worker);

        // Settings whose night holds noon.
        let late = Settings {
            start: ClockTime::new(11, 0),
            ..night_settings()
        };
        let configured = wake(&mut worker, MAX_SLEEP, [Command::Configure(late.clone())]);
        assert_eq!(configured, (ANIMATION_FRAME, Some(frame)), "new settings");
        settle(&mut worker);

        // Wakes that change no aim land at once, and here move nothing.
        let same: [(&str, Vec<Command>); 4] = [
            ("no command", vec![]),
            ("a refresh", vec![Command::Refresh]),
            ("the same settings", vec![Command::Configure(late)]),
            (
                "Red Now on and off, at full night",
                vec![Command::RedNow(true), Command::RedNow(false)],
            ),
        ];
        for (what, commands) in same {
            for command in commands {
                worker.handle(command);
            }
            assert_eq!(worker.fade_time(MAX_SLEEP), QUICK_FADE, "{what}");
        }

        // The first tick, before which nothing was aimed at: the startup
        // fade by night.
        DIAL.set(23.0 * 60.0);
        let (mut worker, _log) = self::worker(dial);
        assert_eq!(
            wake(&mut worker, Duration::ZERO, []),
            (ANIMATION_FRAME, Some(frame)),
            "the startup fade"
        );
    }

    #[test]
    fn a_wake_that_changes_no_aim_lands_at_once_however_short_its_sleep() {
        // An `Instant` does not count the time the Mac slept, so a wake from
        // a night's sleep can come less than a quick fade after the last
        // tick: each wake here comes half a second after it.
        let short = millis(500);
        assert!(short < QUICK_FADE);
        let (night, morning) = (23.0 * 60.0, 8.0 * 60.0);
        let red = night_settings().night.at_level(1.0);
        let (mut worker, log) = worker(dial);

        // The Mac sleeps at full night and wakes in the morning, its tables
        // reset by macOS. The wake's refresh paints none of the red back.
        DIAL.set(night);
        settle(&mut worker);
        assert_eq!(worker.shown, red);
        let (painted, restored) = (paints(&log), restores(&log));
        DIAL.set(morning);
        assert_eq!(
            wake(&mut worker, short, [Command::Refresh]),
            (QUICK_FADE, Some(0.0)),
            "a refresh"
        );
        assert_eq!(paints(&log), painted, "a red was painted");
        assert_eq!(restores(&log), restored + 1);

        // The same with the timer's wake, which brings no command.
        DIAL.set(night);
        settle(&mut worker);
        let painted = paints(&log);
        DIAL.set(morning);
        assert_eq!(
            wake(&mut worker, short, []),
            (QUICK_FADE, Some(0.0)),
            "no command"
        );
        assert_eq!(paints(&log), painted, "a red was painted");

        // A clock that jumps into the night lands at its full look at once.
        DIAL.set(night);
        assert_eq!(
            wake(&mut worker, short, [Command::Refresh]),
            (QUICK_FADE, Some(1.0)),
            "into the night"
        );
        assert_eq!(paints(&log), painted + 1);
        assert_eq!(worker.shown, red);
    }

    #[test]
    fn the_startup_fade_by_night_still_fades() {
        // `run`'s first tick starts the quick fade, which no command does.
        let (engine, log) = spawned(midnight);
        let mut levels = Vec::new();
        until("the startup fade", || {
            levels.extend(engine.poll().into_iter().filter_map(|event| match event {
                Event::Status(status) => Some(status.level),
                Event::Error(_) => None,
            }));
            levels.last() == Some(&1.0)
        });
        let between = levels.iter().filter(|level| 0.0 < **level && **level < 1.0);
        assert!(between.count() >= 3, "the startup fade jumped: {levels:?}");
        shuts_down_and_restores(engine, &log);
    }

    /// How many times [`counted_noon`] was read.
    static NOON_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn counted_noon() -> f64 {
        NOON_READS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        noon()
    }

    #[test]
    fn a_drag_reads_the_clock_once_a_tick() {
        use std::sync::atomic::Ordering::SeqCst;
        let (mut worker, _log) = worker(counted_noon);
        settle(&mut worker);
        let before = NOON_READS.load(SeqCst);
        for kelvin in [2000, 2100, 2200] {
            let look = Look { kelvin, ..held() };
            wake(&mut worker, millis(16), [Command::Preview(Some(look))]);
        }
        assert_eq!(NOON_READS.load(SeqCst) - before, 3);
    }

    /// Each call to a [`Classes`] screen, and the class its thread read then.
    #[cfg(target_os = "macos")]
    type Calls = Arc<Mutex<Vec<(&'static str, Option<u32>)>>>;

    /// A screen that notes its thread's class at each call: the engine's, at
    /// that moment of its loop. A paint waits while a test holds `gate`, so
    /// that the test can send a command in the middle of a fade.
    #[cfg(target_os = "macos")]
    struct Classes {
        calls: Calls,
        gate: Arc<Mutex<()>>,
    }

    #[cfg(target_os = "macos")]
    impl Screen for Classes {
        fn apply(&mut self, _tint: Tint) -> Result<usize, String> {
            self.calls.lock().unwrap().push(("paint", own_class()));
            drop(self.gate.lock());
            Ok(1)
        }
        fn intact(&mut self) -> bool {
            self.calls.lock().unwrap().push(("check", own_class()));
            true
        }
        fn restore(&mut self) {
            self.calls.lock().unwrap().push(("restore", own_class()));
        }
        fn display_count(&mut self) -> usize {
            1
        }
    }

    /// An engine on a [`Classes`] screen and the default settings, with its
    /// calls and its gate.
    #[cfg(target_os = "macos")]
    fn classes_on(clock: fn() -> f64) -> (Engine, Calls, Arc<Mutex<()>>) {
        let calls = Calls::default();
        let gate = Arc::new(Mutex::new(()));
        let screen = Classes {
            calls: calls.clone(),
            gate: gate.clone(),
        };
        let engine = Engine::spawn(Box::new(screen), Settings::default(), clock, |_| {});
        (engine, calls, gate)
    }

    /// The classes of the calls of kind `what`, in order.
    #[cfg(target_os = "macos")]
    fn classes(calls: &[(&str, Option<u32>)], what: &str) -> Vec<Option<u32>> {
        calls
            .iter()
            .filter(|(kind, _)| *kind == what)
            .map(|(_, class)| *class)
            .collect()
    }

    /// How many calls of kind `what` the screen has had.
    #[cfg(target_os = "macos")]
    fn count(calls: &Calls, what: &str) -> usize {
        classes(&calls.lock().unwrap(), what).len()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_quick_fade_waits_at_user_initiated_qos_and_the_rest_at_utility() {
        let (mut engine, calls, _gate) = classes_on(noon);
        until_status(&engine, "the first status", |_| true);
        engine.send(Command::RedNow(true));
        until_status(&engine, "red", |status| status.level == 1.0);
        // A check at once, after the landing.
        engine.send(Command::Refresh);
        until("the check", || count(&calls, "check") > 0);
        engine.shutdown();

        let calls = calls.lock().unwrap().clone();
        let paints = classes(&calls, "paint");
        assert!(paints.len() > 3, "{calls:?}");
        // The tick that starts the fade paints its first frame, and the
        // thread moves up after it, for the wait; every later frame, the
        // landing too, is painted there.
        assert_eq!(paints[0], utility(), "{calls:?}");
        assert!(paints[1..].iter().all(|class| *class == initiated()));
        // Back at utility once it has landed: the refresh's check, and any
        // that its recheck or the backoff makes before the shutdown.
        let checks = classes(&calls, "check");
        assert!(checks.iter().all(|class| *class == utility()), "{calls:?}");
        // The restore, which the app waits for in `join`, moves back up.
        assert_eq!(classes(&calls, "restore"), [initiated()], "{calls:?}");
        assert_eq!(calls.last(), Some(&("restore", initiated())), "{calls:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_preview_waits_at_user_initiated_qos_until_it_ends_even_mid_fade() {
        let (mut engine, calls, gate) = classes_on(noon);
        until_status(&engine, "the first status", |_| true);
        // Red Now's first frame waits at the gate, so the preview comes in
        // the middle of its fade.
        let closed = gate.lock().unwrap();
        engine.send(Command::RedNow(true));
        until("the fade's first frame", || count(&calls, "paint") == 1);
        engine.send(Command::Preview(Some(held())));
        drop(closed);
        until_status(&engine, "the preview", |status| {
            status.tint == held().tint()
        });
        // The drag moves on, and a refresh's check comes while it holds.
        let next = Look {
            kelvin: 2400,
            ..held()
        };
        engine.send(Command::Preview(Some(next)));
        until_status(&engine, "the drag", |status| status.tint == next.tint());
        engine.send(Command::Refresh);
        until("the preview's check", || count(&calls, "check") > 0);
        // Let go: Red Now asks for the preview's level, so the night look
        // lands at once.
        engine.send(Command::Preview(None));
        let night = Look::default().at_level(1.0);
        until_status(&engine, "the night look", |status| status.tint == night);
        let looked = count(&calls, "check");
        engine.send(Command::Refresh);
        until("the check after it", || count(&calls, "check") > looked);
        engine.shutdown();

        let calls = calls.lock().unwrap().clone();
        // The fade's first frame at utility; the preview, the drag and the
        // night look at user-initiated QoS, where the preview held it.
        let paints = classes(&calls, "paint");
        let expected = [utility(), initiated(), initiated(), initiated()];
        assert_eq!(paints, expected, "{calls:?}");
        // The checks while the preview held, then those after it ended.
        let end = calls
            .iter()
            .rposition(|(what, _)| *what == "paint")
            .unwrap();
        let during = classes(&calls[..end], "check");
        assert!(!during.is_empty(), "{calls:?}");
        assert!(
            during.iter().all(|class| *class == initiated()),
            "{calls:?}"
        );
        let after = classes(&calls[end..], "check");
        assert!(!after.is_empty(), "{calls:?}");
        assert!(after.iter().all(|class| *class == utility()), "{calls:?}");
        assert_eq!(calls.last(), Some(&("restore", initiated())), "{calls:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn red_now_reversed_mid_fade_drops_back_to_utility_once_it_lands() {
        // In the schedule's fade, so the screen stays tinted, and checked.
        let level = Settings::default().schedule().level(mid_fade());
        assert!(0.0 < level && level < 1.0);
        let (mut engine, calls, gate) = classes_on(mid_fade);
        until_status(&engine, "the startup fade's landing", |status| {
            status.level == level
        });
        // Red Now's first frame waits at the gate, so Red Now off comes in
        // the middle of its fade.
        let closed = gate.lock().unwrap();
        let before = count(&calls, "paint");
        engine.send(Command::RedNow(true));
        until("the fade's first frame", || count(&calls, "paint") > before);
        engine.send(Command::RedNow(false));
        drop(closed);
        until_status(&engine, "the fade back", |status| status.level == level);
        let looked = count(&calls, "check");
        engine.send(Command::Refresh);
        until("the check after it", || count(&calls, "check") > looked);
        engine.shutdown();

        let calls = calls.lock().unwrap().clone();
        // Red Now's first frame at utility, and the way back, its landing
        // too, at user-initiated QoS.
        let paints = classes(&calls, "paint");
        assert!(paints.len() > before + 1, "{calls:?}");
        assert_eq!(paints[before], utility(), "{calls:?}");
        assert!(
            paints[before + 1..]
                .iter()
                .all(|class| *class == initiated()),
            "{calls:?}"
        );
        // At rest before Red Now and after its reversal: every check at
        // utility.
        let checks = classes(&calls, "check");
        assert!(checks.len() > looked, "{calls:?}");
        assert!(checks.iter().all(|class| *class == utility()), "{calls:?}");
        assert_eq!(calls.last(), Some(&("restore", initiated())), "{calls:?}");
    }
}
