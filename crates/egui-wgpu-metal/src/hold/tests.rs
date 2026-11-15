//! The hold's logic against a fake link, with a clock of its own in every test. No window, no Metal
//! and no display link: the real link's delivery has no test without a window and a display, and
//! the drives (`--demo-drive drag` and `open-idle` on `NullScreen`) are its proof.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::{BURST, Beat, Clock, LONGEST, Link, QUIET, Stats, Waited};

/// What the fake link and its maker were asked, and what they answer.
struct Probe {
    /// Times the maker was called.
    made: Cell<u32>,
    /// What the maker gives: a link, or none.
    can_make: Cell<bool>,
    /// How long the maker takes.
    make_takes: Cell<Duration>,
    /// Times a link was started.
    started: Cell<u32>,
    /// What `start` returns.
    can_start: Cell<bool>,
    /// Times a link was closed.
    closed: Cell<u32>,
}

struct Fake(Rc<Probe>);

impl Link for Fake {
    fn start(&mut self) -> bool {
        self.0.started.set(self.0.started.get() + 1);
        self.0.can_start.get()
    }

    fn close(&mut self) {
        self.0.closed.set(self.0.closed.get() + 1);
    }
}

/// One frame of `beat`, as `Hold::wait` makes it, with the fake for a link: what the wait came to.
fn waited(beat: &mut Beat<Fake>, clock: &Clock, probe: &Rc<Probe>) -> Waited {
    beat.wait(clock, || {
        probe.made.set(probe.made.get() + 1);
        thread::sleep(probe.make_takes.get());
        probe.can_make.get().then(|| Fake(Rc::clone(probe)))
    })
}

/// [`waited`], and how long it waited.
fn step(beat: &mut Beat<Fake>, clock: &Clock, probe: &Rc<Probe>) -> Duration {
    waited(beat, clock, probe).took
}

/// A moment far enough back to be past a burst.
fn long_ago() -> Instant {
    Instant::now()
        .checked_sub(BURST * 2)
        .expect("a clock that has run for 200 ms")
}

/// A window's hold: its beat, a clock of its own and the fake link's probe.
struct Rig {
    clock: Clock,
    beat: Beat<Fake>,
    probe: Rc<Probe>,
}

impl Rig {
    fn new() -> Self {
        Self {
            clock: Clock::new(),
            beat: Beat::default(),
            probe: Rc::new(Probe {
                made: Cell::new(0),
                can_make: Cell::new(true),
                make_takes: Cell::new(Duration::ZERO),
                started: Cell::new(0),
                can_start: Cell::new(true),
                closed: Cell::new(0),
            }),
        }
    }

    /// A window whose burst is under way, its link made and started.
    fn started() -> Self {
        let mut rig = Self::new();
        rig.first_frame();
        rig.frame();
        assert_eq!(rig.probe.started.get(), 1, "the burst starts the link");
        assert!(rig.running());
        rig
    }

    /// The window's first frame: there is no frame before it.
    fn first_frame(&mut self) -> Duration {
        assert!(self.beat.last_frame.is_none(), "a window's first frame");
        step(&mut self.beat, &self.clock, &self.probe)
    }

    /// A frame on the heels of the last: part of a burst. The last frame is set to now, so that
    /// a slow test machine cannot turn it into a gap.
    fn frame(&mut self) -> Duration {
        self.beat.last_frame = Some(Instant::now());
        step(&mut self.beat, &self.clock, &self.probe)
    }

    /// A frame long after the last, which is set back to say so, with no sleep.
    fn frame_after_gap(&mut self) -> Duration {
        self.beat.last_frame = Some(long_ago());
        step(&mut self.beat, &self.clock, &self.probe)
    }

    /// [`Self::first_frame`], and what its wait came to.
    fn first_frame_waited(&mut self) -> Waited {
        assert!(self.beat.last_frame.is_none(), "a window's first frame");
        waited(&mut self.beat, &self.clock, &self.probe)
    }

    /// [`Self::frame`], and what its wait came to.
    fn frame_waited(&mut self) -> Waited {
        self.beat.last_frame = Some(Instant::now());
        waited(&mut self.beat, &self.clock, &self.probe)
    }

    /// [`Self::frame_after_gap`], and what its wait came to.
    fn frame_after_gap_waited(&mut self) -> Waited {
        self.beat.last_frame = Some(long_ago());
        waited(&mut self.beat, &self.clock, &self.probe)
    }

    fn close(&mut self) {
        self.beat.close(&self.clock);
    }

    fn stats(&self) -> Stats {
        self.clock.beats().stats
    }

    fn running(&self) -> bool {
        self.clock.beats().running
    }

    fn quiet(&self) -> u32 {
        self.clock.beats().quiet
    }

    /// `count` refreshes with no frame, and how many of them said to pause the link.
    fn refreshes(&self, count: u32) -> usize {
        (0..count).filter(|_| self.clock.tick()).count()
    }
}

#[test]
fn a_window_that_has_drawn_one_frame_or_rests_waits_for_nothing_and_starts_nothing() {
    let mut rig = Rig::new();
    // The first frame of a window, and one long after another.
    assert_eq!(rig.first_frame(), Duration::ZERO);
    assert_eq!(rig.frame_after_gap(), Duration::ZERO);
    assert_eq!(rig.frame_after_gap(), Duration::ZERO);
    assert_eq!(
        rig.probe.made.get(),
        0,
        "no link for a frame outside a burst"
    );
    assert_eq!(rig.probe.started.get(), 0);
    assert!(!rig.running());
    assert_eq!(rig.stats(), Stats::default());
}

#[test]
fn a_gap_after_the_link_is_made_starts_and_waits_for_nothing_either() {
    let mut rig = Rig::started();
    // The link runs and no refresh has come: a frame in a burst would wait here, a gap does not.
    assert_eq!(rig.frame_after_gap(), Duration::ZERO);
    assert_eq!(rig.probe.started.get(), 1);
    assert_eq!(rig.stats().waits, 0);
    assert_eq!(
        rig.stats().timeouts,
        0,
        "a wait with no refresh would time out"
    );
    // The link paused at rest: a gap does not start it.
    assert_eq!(rig.refreshes(QUIET), 1);
    assert!(!rig.running());
    assert_eq!(rig.frame_after_gap(), Duration::ZERO);
    assert_eq!(rig.probe.started.get(), 1);
    assert_eq!(rig.stats().starts, 1);
    assert!(!rig.running());
    assert_eq!(rig.probe.made.get(), 1);
}

#[test]
fn the_second_frame_of_a_burst_makes_the_link_and_starts_it_with_no_wait() {
    let mut rig = Rig::new();
    rig.probe.make_takes.set(Duration::from_millis(20));
    assert_eq!(rig.first_frame(), Duration::ZERO);
    assert_eq!(rig.probe.made.get(), 0, "never in a window's first frame");
    assert_eq!(rig.frame(), Duration::ZERO);
    assert_eq!(rig.probe.made.get(), 1);
    assert_eq!(rig.probe.started.get(), 1);
    assert!(rig.running());
    assert_eq!(rig.quiet(), 0);
    let stats = rig.stats();
    assert_eq!((stats.starts, stats.waits, stats.timeouts), (1, 0, 0));
    // The make is timed, with its sleep in it.
    let make = stats.longest_make.expect("a make time");
    assert!(make >= Duration::from_millis(20), "{make:?}");
}

#[test]
fn the_longest_make_is_the_longest_of_two() {
    let mut rig = Rig::new();
    rig.probe.make_takes.set(Duration::from_millis(20));
    rig.first_frame();
    rig.frame();
    // A window closes and the next one's link is made at once. A sleep takes at least what it
    // is told, so the second make is the shorter whatever the machine does.
    rig.close();
    rig.probe.make_takes.set(Duration::ZERO);
    rig.first_frame();
    rig.frame();
    assert_eq!(rig.probe.made.get(), 2);
    let make = rig.stats().longest_make.expect("a make time");
    assert!(
        make >= Duration::from_millis(20),
        "the later, shorter make replaced the longer: {make:?}"
    );
}

#[test]
fn a_frame_with_no_refresh_yet_blocks_until_one_comes() {
    let mut rig = Rig::started();
    // No limit this test could reach: at `LONGEST`, a machine that kept the display's thread off the
    // CPU for 50 ms would turn its tick into a timeout. A frame that ignored the tick fails it, late.
    rig.clock.longest = Duration::from_secs(60);
    let stop = AtomicBool::new(false);
    let clock = &rig.clock;
    let waited = thread::scope(|scope| {
        // The display: it waits to see the frame blocked, which the frame counts under the lock
        // before it releases it, then ticks every millisecond until told to stop.
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                if clock.beats().stats.waits >= 1 {
                    clock.tick();
                }
                thread::sleep(Duration::from_millis(1));
            }
        });
        // As `Rig::frame`: a slow start of that thread cannot turn this frame into a gap.
        rig.beat.last_frame = Some(Instant::now());
        let waited = step(&mut rig.beat, &rig.clock, &rig.probe);
        stop.store(true, Ordering::Relaxed);
        waited
    });
    assert!(waited > Duration::ZERO);
    assert!(waited < rig.clock.longest, "the tick ended it: {waited:?}");
    let stats = rig.stats();
    assert_eq!(stats.waits, 1);
    assert_eq!(stats.timeouts, 0);
    assert!(stats.ticks >= 1);
    assert!(rig.running(), "a refresh came: the link goes on");
    assert_eq!(
        rig.probe.started.get(),
        1,
        "no start for a frame that waited"
    );
}

#[test]
fn a_frame_with_a_refresh_already_in_since_the_last_returns_at_once() {
    let mut rig = Rig::started();
    rig.clock.tick();
    // Nothing ticks again: a frame that blocked would wait out the limit.
    rig.frame();
    let stats = rig.stats();
    assert_eq!(stats.waits, 0, "it did not wait");
    assert_eq!(stats.timeouts, 0);
    assert_eq!(
        rig.quiet(),
        0,
        "a frame puts the count of quiet refreshes back"
    );
    assert!(rig.running());
}

#[test]
fn a_refresh_is_spent_by_the_frame_that_takes_it() {
    let mut rig = Rig::started();
    rig.clock.tick();
    rig.frame();
    assert_eq!(rig.stats().waits, 0);
    // No new refresh: the next frame has to wait for one, and none comes.
    rig.frame();
    let stats = rig.stats();
    assert_eq!((stats.waits, stats.timeouts), (1, 1));
}

#[test]
fn the_link_pauses_at_the_32nd_refresh_with_no_frame_as_the_spec_says() {
    let rig = Rig::started();
    // The owner's choice: about 270 ms at 120 Hz.
    for refresh in 1..=31 {
        assert!(!rig.clock.tick(), "refresh {refresh}");
        assert!(rig.running(), "refresh {refresh}");
    }
    assert!(rig.clock.tick(), "the 32nd");
    assert!(!rig.running());
    assert!(!rig.clock.tick(), "the 33rd");
    assert!(!rig.clock.tick(), "the 34th");
    assert!(!rig.running());
    assert_eq!(rig.stats().ticks, 34);
}

#[test]
fn a_frame_puts_the_count_of_quiet_refreshes_back_to_zero() {
    let mut rig = Rig::started();
    assert_eq!(rig.refreshes(QUIET / 2), 0);
    assert_eq!(rig.quiet(), QUIET / 2);
    rig.frame();
    assert_eq!(rig.quiet(), 0);
    // The next pause is a whole QUIET from the frame, not from the start.
    assert_eq!(rig.refreshes(QUIET - 1), 0);
    assert!(rig.running());
    assert_eq!(rig.refreshes(1), 1);
    assert!(!rig.running());
}

#[test]
fn a_burst_after_the_pause_starts_the_link_again_with_no_wait() {
    let mut rig = Rig::started();
    assert_eq!(rig.refreshes(QUIET), 1);
    assert!(!rig.running());
    assert_eq!(rig.frame(), Duration::ZERO);
    assert_eq!(rig.probe.started.get(), 2);
    assert_eq!(rig.probe.made.get(), 1, "the same link");
    let stats = rig.stats();
    assert_eq!((stats.starts, stats.waits, stats.timeouts), (2, 0, 0));
    assert!(rig.running());
    assert_eq!(rig.quiet(), 0);
}

#[test]
fn with_no_refresh_at_all_every_frame_waits_the_limit_and_starts_the_link_again() {
    let mut rig = Rig::started();
    // Nothing ticks: a display that is asleep, or a hidden window. One whole wait for each frame is
    // what makes 20 f/s, counted here rather than timed: only the wait's lower bound is asserted,
    // as a loaded machine may take longer.
    for frame in 1..=3 {
        let waited = rig.frame();
        assert!(waited >= LONGEST, "frame {frame}: {waited:?}");
        let stats = rig.stats();
        assert_eq!((stats.waits, stats.timeouts), (frame, frame));
        assert_eq!(stats.starts, 1 + frame, "a stalled link is started again");
        assert_eq!(rig.probe.started.get(), 1 + frame);
        assert!(rig.running(), "frame {frame}: the next frame waits too");
    }
    assert_eq!(rig.probe.made.get(), 1, "the same link");
}

#[test]
fn a_refresh_after_a_timeout_ends_the_stall_with_no_further_start() {
    let mut rig = Rig::started();
    rig.frame();
    assert_eq!(rig.probe.started.get(), 2);
    rig.clock.tick();
    rig.frame();
    let stats = rig.stats();
    assert_eq!((stats.waits, stats.timeouts), (1, 1), "it did not wait");
    assert_eq!(rig.probe.started.get(), 2);
    assert!(rig.running());
}

#[test]
fn a_timeout_leaves_the_link_ticking_and_its_pause_still_comes_at_the_32nd() {
    let mut rig = Rig::started();
    rig.frame();
    assert_eq!(rig.stats().timeouts, 1);
    assert!(rig.running());
    // The link may be ticking all the same, and then it has to be told to pause. Only that pause
    // and a close end `running`.
    assert_eq!(rig.refreshes(QUIET - 1), 0);
    assert!(rig.running());
    assert_eq!(rig.refreshes(1), 1);
    assert!(!rig.running());
}

#[test]
fn a_link_that_cannot_start_again_after_a_timeout_leaves_every_frame_unheld() {
    let mut rig = Rig::started();
    rig.probe.can_start.set(false);
    let waited = rig.frame();
    assert!(waited >= LONGEST, "{waited:?}");
    assert_eq!(rig.probe.started.get(), 2);
    assert!(!rig.running());
    for _ in 0..4 {
        assert_eq!(rig.frame(), Duration::ZERO);
    }
    assert_eq!(rig.probe.started.get(), 2, "no further start");
    let stats = rig.stats();
    assert_eq!(
        (stats.starts, stats.waits, stats.timeouts),
        (1, 1, 1),
        "only the first start counts, and only the one wait"
    );
}

#[test]
fn a_link_that_cannot_be_made_leaves_every_frame_unheld() {
    let mut rig = Rig::new();
    rig.probe.can_make.set(false);
    rig.first_frame();
    for _ in 0..4 {
        assert_eq!(rig.frame(), Duration::ZERO);
    }
    assert_eq!(rig.probe.made.get(), 1, "asked once, then never again");
    assert_eq!(rig.probe.started.get(), 0);
    let stats = rig.stats();
    assert_eq!((stats.starts, stats.waits, stats.timeouts), (0, 0, 0));
    assert_eq!(stats.longest_make, None, "no link was made");
    assert!(!rig.running());
}

#[test]
fn a_link_that_cannot_start_leaves_every_frame_unheld() {
    let mut rig = Rig::new();
    rig.probe.can_start.set(false);
    rig.first_frame();
    assert_eq!(rig.frame(), Duration::ZERO);
    assert_eq!(rig.probe.started.get(), 1);
    assert!(!rig.running());
    for _ in 0..4 {
        assert_eq!(rig.frame(), Duration::ZERO);
    }
    assert_eq!(rig.probe.started.get(), 1, "no further start");
    assert_eq!(rig.probe.made.get(), 1);
    let stats = rig.stats();
    assert_eq!((stats.waits, stats.timeouts), (0, 0));
    assert_eq!(stats.starts, 0, "a start the link refused did not happen");
    assert!(!rig.running());
}

#[test]
fn a_close_ends_the_link_once_and_stops_the_count() {
    let mut rig = Rig::started();
    assert_eq!(rig.refreshes(5), 0);
    assert_eq!(rig.quiet(), 5);
    rig.close();
    assert_eq!(rig.probe.closed.get(), 1);
    assert!(!rig.running());
    assert_eq!(rig.quiet(), 0);
    // eframe's `destroy` and the painter's `Drop` both reach it.
    rig.close();
    assert_eq!(rig.probe.closed.get(), 1);
}

#[test]
fn a_close_with_no_link_made_leaves_the_clock_alone() {
    let mut rig = Rig::new();
    // Another window's link runs on the process's clock.
    {
        let mut beats = rig.clock.beats();
        beats.running = true;
        beats.quiet = 7;
    }
    rig.close();
    rig.first_frame();
    rig.close();
    assert_eq!(rig.probe.closed.get(), 0);
    assert!(rig.running());
    assert_eq!(rig.quiet(), 7);
}

#[test]
fn a_close_after_a_link_that_could_not_be_made_leaves_the_clock_alone_and_the_next_burst_tries_again()
 {
    let mut rig = Rig::new();
    rig.probe.can_make.set(false);
    rig.first_frame();
    rig.frame();
    {
        let mut beats = rig.clock.beats();
        beats.running = true;
        beats.quiet = 7;
    }
    rig.close();
    assert_eq!(rig.probe.closed.get(), 0, "there was no link to close");
    assert!(rig.running());
    assert_eq!(rig.quiet(), 7);
    // A later window makes a link of its own.
    rig.probe.can_make.set(true);
    rig.clock.beats().running = false;
    rig.first_frame();
    rig.frame();
    assert_eq!(rig.probe.made.get(), 2);
    assert_eq!(rig.probe.started.get(), 1);
}

#[test]
fn after_a_close_a_burst_makes_a_new_link() {
    let mut rig = Rig::started();
    rig.close();
    // The window's frames begin again: the first is no burst.
    assert_eq!(rig.first_frame(), Duration::ZERO);
    assert_eq!(rig.probe.made.get(), 1);
    assert_eq!(rig.frame(), Duration::ZERO);
    assert_eq!(rig.probe.made.get(), 2, "a link of its own");
    assert_eq!(rig.probe.started.get(), 2);
    assert_eq!(rig.stats().starts, 2);
    assert!(rig.running());
    assert_eq!(rig.probe.closed.get(), 1);
}

// ---- what a wait says about parking ----
//
// `Waited::parks` says whether the surface a frame stopped showing goes to the spare's cell. A link's
// pause drops what is there, so a frame parks only where one will come, or in a burst, where the next
// frame takes it.

#[test]
fn a_lone_frame_with_the_link_not_running_parks_nothing() {
    // A window's first frame, and frames long after another, with no link ever made.
    let mut rig = Rig::new();
    let first = rig.first_frame_waited();
    assert_eq!((first.took, first.parks), (Duration::ZERO, false));
    assert!(!rig.frame_after_gap_waited().parks);
    assert!(!rig.running());

    // A link that paused at rest: nothing runs, and a frame after a gap does not start it.
    let mut rig = Rig::started();
    assert_eq!(rig.refreshes(QUIET), 1);
    assert!(!rig.running());
    let lone = rig.frame_after_gap_waited();
    assert_eq!((lone.took, lone.parks), (Duration::ZERO, false));

    // A window that closed: its next window's first frame.
    rig.close();
    assert!(!rig.first_frame_waited().parks);
}

#[test]
fn a_lone_frame_parks_exactly_while_the_link_runs() {
    let mut rig = Rig::started();
    // The link runs: the pause that ends it will drop what this frame parks.
    let lone = rig.frame_after_gap_waited();
    assert_eq!((lone.took, lone.parks), (Duration::ZERO, true));
    assert!(
        rig.running(),
        "the wait reads the clock and changes nothing"
    );
    assert_eq!(rig.probe.started.get(), 1);
    // The pause came: the next lone frame has nothing to drop it.
    assert_eq!(rig.refreshes(QUIET), 1);
    assert!(!rig.frame_after_gap_waited().parks);
    // A burst starts the link again, and a lone frame after it parks once more.
    rig.frame();
    assert!(rig.running());
    assert!(rig.frame_after_gap_waited().parks);
}

#[test]
fn a_frame_in_a_burst_parks_whether_it_starts_the_link_finds_a_refresh_in_or_waits() {
    let mut rig = Rig::new();
    rig.first_frame();
    // The second frame makes the link and starts it, with no wait.
    assert!(rig.frame_waited().parks, "the frame that starts the link");
    // A refresh is already in: the frame returns at once.
    rig.clock.tick();
    assert!(rig.frame_waited().parks, "a frame that finds a refresh in");
    assert_eq!(rig.stats().waits, 0, "it did not wait");
    // The link paused: the frame that starts it again.
    assert_eq!(rig.refreshes(QUIET), 1);
    assert!(!rig.running());
    assert!(rig.frame_waited().parks, "a frame after the pause");
    assert_eq!(rig.probe.started.get(), 2);
}

#[test]
fn a_frame_in_a_burst_that_waits_for_a_refresh_parks() {
    let mut rig = Rig::started();
    // As `a_frame_with_no_refresh_yet_blocks_until_one_comes` has it: no limit this test could reach.
    rig.clock.longest = Duration::from_secs(60);
    let stop = AtomicBool::new(false);
    let clock = &rig.clock;
    let came_to = thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                if clock.beats().stats.waits >= 1 {
                    clock.tick();
                }
                thread::sleep(Duration::from_millis(1));
            }
        });
        rig.beat.last_frame = Some(Instant::now());
        let came_to = waited(&mut rig.beat, &rig.clock, &rig.probe);
        stop.store(true, Ordering::Relaxed);
        came_to
    });
    assert!(came_to.took > Duration::ZERO);
    assert_eq!(rig.stats().waits, 1, "this frame waited");
    assert_eq!(rig.stats().timeouts, 0, "and a refresh ended it");
    assert!(came_to.parks);
}

#[test]
fn a_burst_frame_with_no_link_at_all_parks_all_the_same() {
    // A link that cannot be made: every frame of the burst parks, and no pause will come.
    let mut rig = Rig::new();
    rig.probe.can_make.set(false);
    rig.first_frame();
    for frame in 1..=3 {
        assert!(rig.frame_waited().parks, "frame {frame}, no link made");
    }
    assert!(!rig.running());
    assert_eq!(rig.probe.made.get(), 1);

    // A link that cannot start: the frame that asks, and the frames after it.
    let mut rig = Rig::new();
    rig.probe.can_start.set(false);
    rig.first_frame();
    for frame in 1..=3 {
        assert!(rig.frame_waited().parks, "frame {frame}, no start");
    }
    assert!(!rig.running());
    assert_eq!(rig.probe.started.get(), 1, "asked once");

    // A wait that timed out: nobody ticks, and the limit is short.
    let mut rig = Rig::started();
    rig.clock.longest = Duration::from_millis(5);
    assert!(rig.frame_waited().parks, "a wait that timed out");
    assert_eq!(rig.stats().timeouts, 1, "it did time out");
}

// ---- the pause ----

#[test]
fn the_pause_comes_once_at_the_32nd_quiet_refresh_of_each_start() {
    let mut rig = Rig::started();
    assert_eq!(rig.refreshes(QUIET - 1), 0, "refreshes 1 to 31");
    assert!(rig.running());
    assert!(rig.clock.tick(), "the 32nd");
    assert!(!rig.running());
    assert_eq!(rig.refreshes(100 - QUIET), 0, "refreshes 33 to 100");
    assert_eq!(rig.stats().ticks, 100);
    assert!(!rig.running());

    // A burst starts the link again: its own 32nd quiet refresh pauses it, once.
    rig.frame();
    assert!(rig.running());
    assert_eq!(rig.probe.started.get(), 2);
    assert_eq!(rig.refreshes(QUIET - 1), 0, "31 more");
    assert!(rig.running());
    assert_eq!(rig.refreshes(1), 1, "the 32nd of this start");
    assert!(!rig.running());
    assert_eq!(rig.refreshes(100), 0, "and not again in this pause");
    // The clock only says when to pause: the block that drops the spare counts the drop.
    assert_eq!(rig.stats().spare_drops, 0);
}

#[test]
fn a_frame_between_two_runs_of_31_quiet_refreshes_means_no_pause() {
    let mut rig = Rig::started();
    assert_eq!(rig.refreshes(31), 0);
    // A frame on the heels of a refresh, which takes it and counts again from zero.
    rig.frame();
    assert_eq!(rig.quiet(), 0);
    assert_eq!(rig.refreshes(31), 0);
    assert!(rig.running(), "62 refreshes, but never 32 in a row");
    assert_eq!(rig.refreshes(1), 1, "the 32nd since the frame");
}

// ---- the pause's block: `Clock::drop_spare_at_pause`, on the rig's own clock and no run loop ----

#[test]
fn the_pause_block_drops_nothing_and_counts_nothing_while_the_link_runs() {
    let rig = Rig::started();
    assert!(rig.running());
    let calls = Cell::new(0);

    rig.clock.drop_spare_at_pause(|| {
        calls.set(calls.get() + 1);
        true
    });

    assert_eq!(calls.get(), 0, "a running link keeps its spare");
    assert_eq!(rig.stats().spare_drops, 0);
}

#[test]
fn the_pause_block_counts_a_drop_that_dropped_a_surface_and_not_one_that_found_none() {
    let rig = Rig::started();
    assert_eq!(rig.refreshes(QUIET), 1, "the pause");
    assert!(!rig.running());
    let calls = Cell::new(0);

    rig.clock.drop_spare_at_pause(|| {
        calls.set(calls.get() + 1);
        true
    });
    assert_eq!(calls.get(), 1, "called once");
    assert_eq!(rig.stats().spare_drops, 1, "and counted");

    rig.clock.drop_spare_at_pause(|| {
        calls.set(calls.get() + 1);
        false
    });
    assert_eq!(calls.get(), 2, "called once more");
    assert_eq!(
        rig.stats().spare_drops,
        1,
        "and not counted: it found no spare"
    );
}

#[test]
fn the_pause_block_drops_with_no_lock_of_the_clocks_held_for_a_tick_to_wait_on() {
    // A clock of its own, as the link is not running. The closure stands for the surface's drop: a
    // refresh comes while it runs, and its tick must get through. Bounded, so that a drop under the
    // lock fails this test after the limit and does not hang it.
    const LIMIT: Duration = Duration::from_secs(5);
    let clock = Clock::new();
    let (ticked, tick_done) = std::sync::mpsc::channel();
    thread::scope(|scope| {
        clock.drop_spare_at_pause(|| {
            scope.spawn(|| {
                clock.tick();
                let _ = ticked.send(());
            });
            tick_done.recv_timeout(LIMIT).is_ok()
        });
    });
    let stats = clock.beats().stats;
    assert_eq!(stats.ticks, 1, "the tick ran");
    assert_eq!(
        stats.spare_drops, 1,
        "it ran while the surface was dropped, and the drop was counted"
    );
}

#[test]
fn a_block_that_finds_the_link_started_again_leaves_its_spare_to_the_next_pause() {
    let mut rig = Rig::started();
    assert_eq!(rig.refreshes(QUIET), 1, "the pause queues the block");
    // A burst starts the link again before the block runs on the main thread.
    rig.frame();
    assert!(rig.running());
    let calls = Cell::new(0);
    let drop_spare = || {
        calls.set(calls.get() + 1);
        true
    };

    rig.clock.drop_spare_at_pause(drop_spare);
    assert_eq!(calls.get(), 0, "the stale block drops nothing");
    assert_eq!(rig.stats().spare_drops, 0, "and counts no pause");

    assert_eq!(rig.refreshes(QUIET), 1, "the next pause");
    rig.clock.drop_spare_at_pause(drop_spare);
    assert_eq!(calls.get(), 1, "drops what the block left");
    assert_eq!(rig.stats().spare_drops, 1);
}
