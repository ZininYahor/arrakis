//! The wait between headless ticks, with the window closed: AppKit's event
//! loop, run until the tick's deadline or until a wake cuts it short.
//!
//! fastframe-shell's `pause` hands the app's `idle` slices of at most
//! `HEADLESS_TICK` (150 ms) and looks at the [`Waker`] between them, because
//! `fastframe_tray::idle` cannot be cut short. That woke the main thread
//! about 6.7 times a second with nothing to do. Here `App::headless_frame`
//! publishes when its wait ends ([`publish`]), [`idle`] runs one pump until
//! then whatever slice it is handed, and a wake from any thread cuts the pump
//! short. There are two kinds:
//! - [`wake`] stops it at once: a menu-bar click, a second launch, a display
//!   change;
//! - [`wake_paced`] stops it no sooner than a [`PACE`] after it began: the
//!   engine's status, which a quick fade reports every 33 ms.
//!
//! A third, [`wake_window`], leaves it alone and only draws an open window:
//! the engine's statuses that a closed window has no use for.
//!
//! `pause` then sees the waker's flag, or no time left, and returns. Nothing
//! else in the shell changes.
//!
//! This module goes away when fastframe-shell hands `idle` the whole wait and
//! fastframe-tray offers a pump that a wake interrupts.

use std::cell::Cell;
use std::time::{Duration, Instant};

use fastframe_shell::{Headless, Waker};

#[cfg(target_os = "macos")]
#[doc(hidden)]
pub use macos::stop;
#[cfg(target_os = "macos")]
pub use macos::{idle, interrupt, interrupt_paced};
#[cfg(not(target_os = "macos"))]
pub use stub::{idle, interrupt, interrupt_paced};

/// The shortest headless wait that a paced wake ([`wake_paced`]) ends: the
/// engine's status ends at most one wait in this long, as often as
/// fastframe-shell's slices looked at the waker before. A quick fade, which
/// reports every 33 ms for 1.2 s, then ends about 8 waits instead of 36.
pub const PACE: Duration = fastframe_shell::HEADLESS_TICK;

thread_local! {
    /// When the wait that the last headless tick asked for ends. The shell's
    /// thread both publishes it and reads it.
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Notes when the wait that `headless` asks for ends, for [`idle`], and
/// returns `headless`. `App::headless_frame` passes every answer through
/// here, so a deadline never outlives the tick that set it.
pub fn publish(headless: Headless) -> Headless {
    DEADLINE.set(deadline(headless, Instant::now()));
    headless
}

/// The deadline [`publish`] last noted on this thread, for tests of the
/// code that publishes it.
#[cfg(test)]
pub(crate) fn published() -> Option<Instant> {
    DEADLINE.get()
}

/// Wakes the app: the window's next frame, or the end of the shell's
/// headless wait, at once. Every wake in the app goes through here, through
/// [`wake_paced`] or through [`wake_window`].
///
/// The order matters: the waker's flag is up before the pump stops, so the
/// shell's `pause` finds it when [`idle`] returns. Stopped first, `pause`
/// could find no flag and pump again until the deadline, a minute away.
pub fn wake(waker: &Waker) {
    waker.wake();
    interrupt();
}

/// Wakes the app for news that can wait a [`PACE`]: the engine's status.
/// The window, if one is open, gets its next frame at once; a headless wait
/// ends no sooner than a pace after it began, so the engine ends a few waits
/// a second at most, however often it reports. A menu-bar click meanwhile
/// still ends the wait at once ([`wake`]).
///
/// In the same order as [`wake`], for the same reason.
pub fn wake_paced(waker: &Waker) {
    waker.wake();
    interrupt_paced();
}

/// Draws an open window's next frame at once, and leaves a closed app's
/// headless wait alone: the engine's statuses that only move the level,
/// within `engine::LEVEL_PACE` of its last wake (`engine::Reach::Window`).
/// The status waits in the engine's channel for the app's next tick, or for
/// the window that opens next.
///
/// egui's `request_repaint_after` with no delay is its `request_repaint`.
pub fn wake_window(waker: &Waker) {
    waker.wake_after(Duration::ZERO);
}

/// What a wake asks of the running pump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum Cut {
    /// End it now: [`wake`].
    Now,
    /// End it a [`PACE`] after it started, or now if that has passed:
    /// [`wake_paced`].
    Paced,
}

/// How long from `now` the pump that `started` should still run once a wake
/// asks for `cut`, its timer being `due` in that long. A wake only ever
/// brings the end nearer: never past the deadline.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn fire_in(cut: Cut, due: Duration, started: Instant, now: Instant) -> Duration {
    let wanted = match cut {
        Cut::Now => Duration::ZERO,
        Cut::Paced => (started + PACE).saturating_duration_since(now),
    };
    wanted.min(due)
}

/// When the wait that `headless` asks for at `now` ends: only
/// [`Headless::WaitFor`] has an end. A wait too long for an `Instant` has
/// none, and [`idle`] then keeps to the shell's slices.
fn deadline(headless: Headless, now: Instant) -> Option<Instant> {
    match headless {
        Headless::WaitFor(wait) => now.checked_add(wait),
        Headless::Wait | Headless::Show | Headless::Quit => None,
    }
}

/// How long one pump runs from `now`: until `deadline`, and never less than
/// the shell's `slice`. Past the deadline, or with none, that is the slice
/// itself. The shell counts its wait from a moment just after the deadline
/// was set, so a pump that stopped at the deadline alone would leave it a
/// sliver of wait, and be called again at once.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn pump_for(deadline: Option<Instant>, now: Instant, slice: Duration) -> Duration {
    deadline.map_or(slice, |deadline| {
        deadline.saturating_duration_since(now).max(slice)
    })
}

/// The pump, modeled on `fastframe_tray::idle`'s: one
/// `-[NSApplication run]`, and a timer that stops it with `stop:` and a
/// posted event.
///
/// # The two failures it rules out
///
/// - **A stop must never reach winit's loop**, which runs the window on the
///   same `NSApp`: that would end the window's event loop.
///   - Both ways to stop (the timer's `stopRun:`, and the blocks
///     [`cut_short`] and [`cut_paced`], which move the timer) act only while
///     [`PUMP`] holds a timer. Only [`idle`] sets it, just before its own
///     `run`, and clears it just after, on the main thread, so it is never
///     set while a window's loop runs.
///   - The timer is invalidated before [`idle`] returns, however the pump
///     ended, so a deadline left over from an early stop never fires later.
///   - The main queue runs in the run loop's common modes, so a block can
///     run while a menu-bar menu is open. It never stops anything there: it
///     moves the timer's fire date, and the timer runs in the default mode
///     only, so it fires once the menu has closed.
/// - **A wake must never be missed**, or a menu-bar click would wait for the
///   deadline, a minute away. Each kind of wake has its own flag and block:
///   [`PENDING`] and [`cut_short`] for [`interrupt`], [`PACED`] and
///   [`cut_paced`] for [`interrupt_paced`]. Sharing one, a click that came
///   just after the engine's status would wait out its pace.
///   - [`raise`] raises the flag, and queues the block only when the flag
///     was down; [`super::wake`] and [`super::wake_paced`] raise the
///     waker's flag before both.
///   - A queued block runs only when the main thread runs a run loop. One
///     that runs while a pump does lowers the flag and stops the pump: at
///     once, or a pace after it started.
///   - One that runs while no pump does, between two headless ticks or in
///     winit's loop, leaves its flag raised. The next [`idle`] sets [`PUMP`]
///     first and then looks at both flags. `PENDING` raised, it returns
///     without pumping, and the shell ticks; `PACED` raised, its pump lasts
///     a pace at most. A wake whose block ran after the tick's drain, were
///     AppKit ever to run the loop in between, is caught here too.
///   - So a raised flag always has an answer coming: the block that the
///     wake which raised it queued, still to run, or the next [`idle`]. A
///     wake that finds its flag raised queues nothing, and that answer covers
///     it as well: its waker's flag went up before it looked, so the tick the
///     answer brings sees it. A burst of wakes queues one block, not one
///     each.
///   - So an interrupt while no pump runs stops nothing, and the next wait
///     does not start.
#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use dispatch2::DispatchQueue;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::{NSDate, NSPoint, NSTimer};

    use super::{Cut, DEADLINE, fire_in, pump_for};

    /// A wake that no pump has answered ([`interrupt`]). Any thread raises
    /// it, before it queues [`cut_short`]. It is lowered only where a
    /// headless tick is sure to follow: by an [`idle`] that returns without
    /// pumping, or by a [`cut_short`] that stops a pump.
    pub(super) static PENDING: AtomicBool = AtomicBool::new(false);

    /// A paced wake that no pump has answered ([`interrupt_paced`]): the
    /// same, for [`cut_paced`]. An [`idle`] lowers it as it starts too:
    /// returning without pumping, the tick that follows answers both, and
    /// pumping, it ends its pump a pace after it began.
    pub(super) static PACED: AtomicBool = AtomicBool::new(false);

    /// The blocks [`raise`] has queued, for the test that a burst queues
    /// one. libtest's main thread runs no loop, so there they never run.
    #[cfg(test)]
    pub(super) static QUEUED: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    thread_local! {
        /// The running pump, held only while [`idle`] is inside its own
        /// `-[NSApplication run]`: the main-thread-only flag that every way
        /// to stop checks.
        static PUMP: RefCell<Option<Running>> = const { RefCell::new(None) };
    }

    /// The running pump: its deadline timer, and when it started, which a
    /// paced wake counts its [`super::PACE`] from.
    struct Running {
        timer: Retained<NSTimer>,
        started: Instant,
    }

    impl Running {
        /// Brings the timer's fire date nearer as `cut` asks; never later.
        fn hasten(&self, cut: Cut) {
            // Already due, or a date that makes no sense: leave it be.
            let Ok(due) = Duration::try_from_secs_f64(self.timer.fireDate().timeIntervalSinceNow())
            else {
                return;
            };
            let fire = fire_in(cut, due, self.started, Instant::now());
            if fire < due {
                self.timer
                    .setFireDate(&NSDate::dateWithTimeIntervalSinceNow(fire.as_secs_f64()));
            }
        }
    }

    /// Waits until the published deadline, ignoring the shell's `slice`
    /// unless the deadline is nearer, or none was published; a wake from any
    /// thread ([`interrupt`]) cuts it short. Pass it to `Shell::idle`.
    ///
    /// Off the main thread there is no AppKit loop to run, and it sleeps the
    /// slice, as `fastframe_tray::idle` does.
    pub fn idle(slice: Duration) {
        let Some(mtm) = MainThreadMarker::new() else {
            std::thread::sleep(slice);
            return;
        };
        // The main thread has no pool of its own here, and `run` drains only
        // its inner ones. Without this, the autoreleased timer of each tick
        // stays for good: 200 B a tick, 0.3 to 0.5 MB a day of uptime.
        autoreleasepool(|_| pump(mtm, slice));
    }

    /// One tick's wait, inside its pool: see [`idle`].
    fn pump(mtm: MainThreadMarker, slice: Duration) {
        let started = Instant::now();
        let wait = pump_for(DEADLINE.get(), started, slice);
        let target = Stop::new(mtm);
        // SAFETY: `stopRun:` is defined on `Stop`, takes one object (the
        // timer) and returns nothing; the timer keeps its target alive until
        // it is invalidated.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                wait.as_secs_f64(),
                &target,
                sel!(stopRun:),
                None,
                false,
            )
        };
        let _pump = Pump::start(Running { timer, started });
        // The pump is in place before these looks: a block that runs from
        // here on (only inside `run`) finds it, and one that ran before left
        // its flag raised.
        let pending = PENDING.swap(false, Ordering::SeqCst);
        let paced = PACED.swap(false, Ordering::SeqCst);
        if pending {
            // The tick that follows answers the paced wake as well: its
            // waker's flag is up.
            return;
        }
        if paced {
            PUMP.with_borrow(|pump| {
                if let Some(pump) = pump {
                    pump.hasten(Cut::Paced);
                }
            });
        }
        NSApplication::sharedApplication(mtm).run();
    }

    /// Cuts the running pump short from any thread, at once. Call it after
    /// the waker's `wake` ([`super::wake`] does both).
    ///
    /// It queues [`cut_short`] on the main queue, which stops the pump if
    /// one runs when the block does, and otherwise stops nothing: the next
    /// [`idle`] then returns at once.
    pub fn interrupt() {
        raise(&PENDING, cut_short);
    }

    /// Cuts the running pump short from any thread, a [`super::PACE`] after
    /// it started at the soonest. Call it after the waker's `wake`
    /// ([`super::wake_paced`] does both).
    ///
    /// It queues [`cut_paced`], which ends the pump a pace after its start
    /// if one runs when the block does. Otherwise the next [`idle`] pumps a
    /// pace at most.
    pub fn interrupt_paced() {
        raise(&PACED, cut_paced);
    }

    /// Raises `flag` and queues `block` on the main queue, unless `flag` was
    /// raised already: then the block a wake queued before is still to run,
    /// or the next [`idle`] finds the flag, and that answers this wake too.
    fn raise(flag: &'static AtomicBool, block: fn()) {
        if !flag.swap(true, Ordering::SeqCst) {
            #[cfg(test)]
            QUEUED.fetch_add(1, Ordering::SeqCst);
            DispatchQueue::main().exec_async(block);
        }
    }

    /// On the main thread: moves the running pump's deadline to now, if a
    /// pump runs and a wake is pending.
    fn cut_short() {
        answer(&PENDING, Cut::Now);
    }

    /// On the main thread: moves the running pump's deadline to a pace
    /// after its start, or now if that has passed, if a pump runs and a
    /// paced wake is pending.
    fn cut_paced() {
        answer(&PACED, Cut::Paced);
    }

    /// Lowers `flag` and brings the running pump's end nearer as `cut`
    /// asks, if a pump runs and `flag` is raised. The timer then fires in
    /// the default mode, so after a menu the user holds open, never inside
    /// it.
    fn answer(flag: &AtomicBool, cut: Cut) {
        PUMP.with_borrow(|pump| {
            if let Some(pump) = pump
                && pump.timer.isValid()
                && flag.swap(false, Ordering::SeqCst)
            {
                pump.hasten(cut);
            }
        });
    }

    /// The running pump in [`PUMP`], from just before `run` until just
    /// after it. Dropping it invalidates the timer, however the pump ended,
    /// so a deadline left over never fires in a later loop.
    struct Pump;

    impl Pump {
        fn start(running: Running) -> Self {
            PUMP.set(Some(running));
            Self
        }
    }

    impl Drop for Pump {
        fn drop(&mut self) {
            if let Some(running) = PUMP.take() {
                running.timer.invalidate();
            }
        }
    }

    define_class!(
        /// The pump's timer target: ends the pump's `run`, and nothing else.
        ///
        /// With [`stop`], a copy of fastframe-tray's private `HeadlessStop`
        /// (`crates/fastframe-tray/src/macos.rs` at tag v0.13.0,
        /// `d5e1a99`), which Arrakis cannot call, plus the guard that only
        /// the pump's own loop is stopped.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "ArrakisIdleStop"]
        struct Stop;

        impl Stop {
            #[unsafe(method(stopRun:))]
            fn stop_run(&self, _timer: Option<&AnyObject>) {
                // Only the pump's own loop: never winit's.
                if PUMP.with_borrow(Option::is_some) {
                    stop(self.mtm());
                }
            }
        }
    );

    impl Stop {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            // SAFETY: `NSObject`'s designated initializer, called once.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// Ends `-[NSApplication run]` at once: `stop:` takes effect after the
    /// next event, and a timer is not one, so a no-op event is posted for
    /// `run` to return on.
    ///
    /// It mirrors `HeadlessStop::stopRun:` in fastframe-tray
    /// (`crates/fastframe-tray/src/macos.rs` at tag v0.13.0, `d5e1a99`);
    /// move the two together. Public only for `tests/idle.rs`, which ends a
    /// loop that is not the pump's with it. It stops whatever loop runs: the
    /// pump calls it only through [`Stop`]'s guard.
    pub fn stop(mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        app.stop(None);
        if let Some(event) =
            NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                NSEventType::ApplicationDefined,
                NSPoint::new(0.0, 0.0),
                NSEventModifierFlags::empty(),
                0.0,
                0,
                None,
                0,
                0,
                0,
            )
        {
            app.postEvent_atStart(&event, true);
        }
    }
}

/// Elsewhere the shell's slices stand: v1 is macOS only.
#[cfg(not(target_os = "macos"))]
mod stub {
    use std::time::Duration;

    /// fastframe-tray's wait, which nothing cuts short: the shell looks at
    /// its waker between slices.
    pub fn idle(slice: Duration) {
        fastframe_tray::idle(slice);
    }

    /// Nothing to cut short: see [`idle`].
    pub fn interrupt() {}

    /// Nothing to cut short: see [`idle`].
    pub fn interrupt_paced() {}
}

/// The pure parts and the fallback off the main thread: libtest runs these on
/// worker threads, where AppKit's loop cannot run. `tests/idle.rs` runs the
/// pump itself on the main thread.
#[cfg(test)]
mod tests {
    use super::*;

    /// A slice to hand `pump_for`: only its size against the deadline counts.
    const SLICE: Duration = Duration::from_millis(150);

    const ALL: [Headless; 4] = [
        Headless::Wait,
        Headless::WaitFor(Duration::from_secs(5)),
        Headless::Show,
        Headless::Quit,
    ];

    #[test]
    fn a_timed_wait_ends_that_long_from_now() {
        let now = Instant::now();
        for wait in [
            Duration::ZERO,
            Duration::from_millis(1),
            Duration::from_secs(60),
        ] {
            assert_eq!(deadline(Headless::WaitFor(wait), now), Some(now + wait));
        }
    }

    #[test]
    fn only_a_timed_wait_has_a_deadline() {
        let now = Instant::now();
        for headless in [Headless::Wait, Headless::Show, Headless::Quit] {
            assert_eq!(deadline(headless, now), None, "{headless:?}");
        }
    }

    #[test]
    fn a_wait_too_long_for_an_instant_has_no_deadline() {
        let now = Instant::now();
        assert_eq!(deadline(Headless::WaitFor(Duration::MAX), now), None);
    }

    #[test]
    fn without_a_deadline_a_pump_runs_the_slice() {
        assert_eq!(pump_for(None, Instant::now(), SLICE), SLICE);
    }

    #[test]
    fn a_pump_runs_until_a_deadline_farther_than_the_slice() {
        let now = Instant::now();
        for ahead in [Duration::from_secs(1), Duration::from_secs(60)] {
            assert!(ahead > SLICE);
            assert_eq!(pump_for(Some(now + ahead), now, SLICE), ahead);
        }
    }

    #[test]
    fn a_pump_never_runs_less_than_the_slice() {
        let base = Instant::now();
        let now = base + Duration::from_secs(10);
        // Nearer than the slice, a hair before `now`, exactly `now`, and
        // already past by a little and by a lot.
        for deadline in [
            now + SLICE / 3,
            now + Duration::from_nanos(1),
            now,
            now - Duration::from_millis(1),
            base,
        ] {
            assert_eq!(pump_for(Some(deadline), now, SLICE), SLICE, "{deadline:?}");
        }
    }

    #[test]
    fn a_deadline_exactly_one_slice_away_is_the_slice_too() {
        let now = Instant::now();
        assert_eq!(pump_for(Some(now + SLICE), now, SLICE), SLICE);
    }

    #[test]
    fn a_wake_ends_the_pump_at_once_whenever_it_comes() {
        let base = Instant::now();
        let now = base + PACE * 10;
        for started in [now, now - Duration::from_millis(10), base] {
            assert_eq!(
                fire_in(Cut::Now, Duration::from_secs(60), started, now),
                Duration::ZERO
            );
        }
    }

    #[test]
    fn a_paced_wake_ends_the_pump_a_pace_after_it_started() {
        let started = Instant::now();
        let deadline = Duration::from_secs(60);
        // Just after the start, and partway through the pace.
        assert_eq!(fire_in(Cut::Paced, deadline, started, started), PACE);
        let later = started + PACE / 3;
        assert_eq!(
            fire_in(Cut::Paced, deadline, started, later),
            PACE - PACE / 3
        );
        // A pump that has run a pace or more ends now.
        for ran in [PACE, PACE * 2, Duration::from_secs(30)] {
            assert_eq!(
                fire_in(Cut::Paced, deadline, started, started + ran),
                Duration::ZERO,
                "{ran:?}"
            );
        }
    }

    #[test]
    fn no_wake_moves_the_end_past_the_deadline() {
        let started = Instant::now();
        for due in [Duration::ZERO, PACE / 2] {
            for cut in [Cut::Now, Cut::Paced] {
                assert!(
                    fire_in(cut, due, started, started) <= due,
                    "{cut:?} {due:?}"
                );
            }
        }
        assert_eq!(fire_in(Cut::Paced, PACE / 2, started, started), PACE / 2);
    }

    /// The blocks a burst of interrupts queues, from eight threads at once,
    /// with `flag` lowered first, as an answered wake leaves it.
    #[cfg(target_os = "macos")]
    fn queued_by_a_burst(flag: &std::sync::atomic::AtomicBool, interrupt: fn()) -> usize {
        use std::sync::atomic::Ordering;
        use std::sync::{Arc, Barrier};

        flag.store(false, Ordering::SeqCst);
        let before = macos::QUEUED.load(Ordering::SeqCst);
        let together = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let together = together.clone();
                std::thread::spawn(move || {
                    together.wait();
                    for _ in 0..25 {
                        interrupt();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("an interrupting thread");
        }
        assert!(flag.load(Ordering::SeqCst), "the burst left its flag down");
        macos::QUEUED.load(Ordering::SeqCst) - before
    }

    /// Held by a test that reads or moves the wakes' flags and count, which
    /// are the process's: libtest runs the tests on several threads.
    #[cfg(target_os = "macos")]
    static FLAGS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(target_os = "macos")]
    fn flags_alone() -> std::sync::MutexGuard<'static, ()> {
        FLAGS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_window_wake_leaves_a_closed_apps_wait_alone() {
        use std::sync::atomic::Ordering;

        let _alone = flags_alone();
        macos::PENDING.store(false, Ordering::SeqCst);
        macos::PACED.store(false, Ordering::SeqCst);
        let before = macos::QUEUED.load(Ordering::SeqCst);

        // No window is attached: nothing is asked of the app at all.
        let waker = Waker::default();
        wake_window(&waker);
        assert_eq!(
            macos::QUEUED.load(Ordering::SeqCst),
            before,
            "a block queued"
        );
        assert!(!macos::PENDING.load(Ordering::SeqCst), "a wake is pending");
        assert!(
            !macos::PACED.load(Ordering::SeqCst),
            "a paced wake is pending"
        );

        // The same counters see the other two wakes, so they would see this
        // one if it ended the wait.
        wake(&waker);
        assert_eq!(macos::QUEUED.load(Ordering::SeqCst), before + 1);
        assert!(macos::PENDING.load(Ordering::SeqCst));
        wake_paced(&waker);
        assert!(macos::PACED.load(Ordering::SeqCst));
        macos::PENDING.store(false, Ordering::SeqCst);
        macos::PACED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn a_window_wake_asks_an_open_window_for_a_frame() {
        // What the windowing backend hears of a repaint request.
        let ctx = egui::Context::default();
        let delays = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let heard = delays.clone();
        ctx.set_request_repaint_callback(move |request| {
            heard.lock().unwrap().push(request.delay);
        });
        let waker = Waker::default();
        waker.attach(&ctx);
        wake_window(&waker);
        assert_eq!(*delays.lock().unwrap(), [Duration::ZERO], "a frame at once");
        waker.detach();
        wake_window(&waker);
        assert_eq!(
            delays.lock().unwrap().len(),
            1,
            "a closed window hears nothing"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_burst_of_wakes_queues_one_block_of_each_kind() {
        // One test for both kinds: they share the count. The blocks never
        // run here, so the flags stay raised until lowered by hand.
        let _alone = flags_alone();
        assert_eq!(queued_by_a_burst(&macos::PENDING, interrupt), 1);
        assert_eq!(queued_by_a_burst(&macos::PACED, interrupt_paced), 1);
        // Once answered, the next wake queues again.
        assert_eq!(queued_by_a_burst(&macos::PENDING, interrupt), 1);
    }

    #[test]
    fn publish_returns_what_it_is_given() {
        for headless in ALL {
            assert_eq!(publish(headless), headless);
        }
        assert_eq!(
            publish(Headless::WaitFor(Duration::MAX)),
            Headless::WaitFor(Duration::MAX)
        );
    }

    #[test]
    fn publish_notes_when_a_timed_wait_ends() {
        let wait = Duration::from_secs(5);
        let before = Instant::now();
        publish(Headless::WaitFor(wait));
        let after = Instant::now();
        let ends = DEADLINE.get().expect("a timed wait has a deadline");
        assert!(ends >= before + wait && ends <= after + wait, "{ends:?}");
    }

    #[test]
    fn a_later_answer_clears_the_deadline_a_timed_wait_set() {
        for later in [Headless::Wait, Headless::Show, Headless::Quit] {
            publish(Headless::WaitFor(Duration::from_secs(5)));
            assert!(DEADLINE.get().is_some());
            publish(later);
            assert_eq!(DEADLINE.get(), None, "{later:?}");
        }
        // One too long for an Instant clears it as well.
        publish(Headless::WaitFor(Duration::from_secs(5)));
        publish(Headless::WaitFor(Duration::MAX));
        assert_eq!(DEADLINE.get(), None);
    }

    #[test]
    fn off_the_main_thread_idle_keeps_to_the_slice() {
        // libtest never runs a test on the main thread, so this is the
        // fallback: with a deadline 5 s away, it must not wait for it.
        publish(Headless::WaitFor(Duration::from_secs(5)));
        let slice = Duration::from_millis(50);
        let started = Instant::now();
        idle(slice);
        let waited = started.elapsed();
        assert!(waited < Duration::from_secs(1), "{waited:?}");
        if cfg!(target_os = "macos") {
            // A sleep, so the shell's `pause` is never handed back a sliver.
            assert!(waited >= slice, "{waited:?}");
        }
    }
}
