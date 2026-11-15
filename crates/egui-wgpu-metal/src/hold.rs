//! The hold: what a swap does for GL, told by a display link. A frame that comes on the heels of
//! another waits for the display's next refresh, on a condition variable, so a drag runs at the
//! screen's rate and no frame's CPU carries a `sleep`'s wake-up.
//!
//! - A frame after a gap of [`BURST`] does not wait and starts nothing, as a swap with nothing
//!   queued does not block: a window at rest takes no wake-ups.
//! - Each window makes its link at the second frame of its first burst, so never in a first frame.
//! - The link pauses itself at the [`QUIET`]th refresh with no frame, and that tick hands the main
//!   thread one block that drops the spare surface: one wake for each burst's end, and none at rest.
//! - No frame waits longer than [`LONGEST`], so a display that stops refreshing (asleep, a closed
//!   lid), or a hidden window, slows the frames and hangs nothing. A frame that reaches it starts
//!   the link again, and the next frame waits too.
//!
//! The logic here knows a link only as [`Link`]. `link` holds the real one, on a thread of its own.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ::winit_crate::window::Window;

mod link;

/// A frame later than this after the last one is not part of an animation.
const BURST: Duration = Duration::from_millis(100);
/// Refreshes with no frame before the link pauses itself: about 270 ms at 120 Hz.
const QUIET: u32 = 32;
/// The longest a frame waits: 20 f/s when no refresh comes.
const LONGEST: Duration = Duration::from_millis(50);

/// What the hold asks of a link.
trait Link {
    /// Lets it tick, from a pause or a stall. False when it never will: frames then go unheld.
    fn start(&mut self) -> bool;
    /// Ends it, with no wait, before its window goes.
    fn close(&mut self);
}

/// The counters of a run, for a demo build's report.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    /// Refreshes the links told of.
    pub ticks: u64,
    /// Starts a link agreed to: one for each burst it ran in, and one for each timeout.
    pub starts: u32,
    /// Frames that waited for a refresh.
    pub waits: u32,
    /// Waits that reached [`LONGEST`].
    pub timeouts: u32,
    /// The longest a link that was made took, on the main thread, with its thread started.
    pub longest_make: Option<Duration>,
    /// Pauses whose block found a spare surface on the main thread, and dropped it.
    pub spare_drops: u32,
}

/// What a frame's wait came to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Waited {
    /// How long it waited.
    pub(crate) took: Duration,
    /// Whether the surface this frame stopped showing goes to the spare's cell. So a burst's frames do
    /// no more than turn two surfaces over. A lone frame parks only while `running` is true, with a
    /// pause to come that will drop it. See [`Beat::wait`] for when none comes.
    pub(crate) parks: bool,
}

impl Waited {
    const fn in_burst(took: Duration) -> Self {
        Self { took, parks: true }
    }
}

struct Beats {
    /// Refreshes since a frame last came to wait.
    quiet: u32,
    /// A frame started the link, and neither its pause nor a close has come since.
    running: bool,
    stats: Stats,
}

/// Where a link's ticks reach the frames. It holds no pointer to a painter, a surface or a view.
struct Clock {
    beats: Mutex<Beats>,
    ticked: Condvar,
    /// The longest a frame waits: [`LONGEST`], unless a test must not race it.
    longest: Duration,
}

/// The process's clock: the app has one window, and the link's target is told of nothing else.
static CLOCK: Clock = Clock::new();

impl Clock {
    const fn new() -> Self {
        Self {
            beats: Mutex::new(Beats {
                quiet: 0,
                running: false,
                stats: Stats {
                    ticks: 0,
                    starts: 0,
                    waits: 0,
                    timeouts: 0,
                    longest_make: None,
                    spare_drops: 0,
                },
            }),
            ticked: Condvar::new(),
            longest: LONGEST,
        }
    }

    fn beats(&self) -> MutexGuard<'_, Beats> {
        self.beats.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One refresh. True once, at the [`QUIET`]th with no frame since: the link should pause now.
    fn tick(&self) -> bool {
        let mut beats = self.beats();
        beats.stats.ticks += 1;
        beats.quiet = beats.quiet.saturating_add(1);
        let pause = beats.quiet == QUIET;
        if pause {
            beats.running = false;
        }
        drop(beats);
        self.ticked.notify_all();
        pause
    }

    /// The pause's block, on the main thread: `drop_spare` empties the spare's cell and says whether
    /// that dropped a surface. A frame may have started the link again since the tick: the spare it
    /// parked is the next pause's to drop. Only this thread sets `running` to true, so a read of false
    /// stands while the surface goes, and the clock's lock is not held across that for a tick to wait on.
    fn drop_spare_at_pause(&self, drop_spare: impl FnOnce() -> bool) {
        let running = self.beats().running;
        if !running && drop_spare() {
            self.beats().stats.spare_drops += 1;
        }
    }
}

/// How far a window's link has come.
enum Made<L> {
    Not,
    Yes(L),
    /// It could not be made or run: this window's frames go unheld.
    Never,
}

/// One window's frames against the clock, and that window's link.
struct Beat<L> {
    last_frame: Option<Instant>,
    /// The ticks when the last frame went on.
    seen: u64,
    link: Made<L>,
}

impl<L> Default for Beat<L> {
    fn default() -> Self {
        Self {
            last_frame: None,
            seen: 0,
            link: Made::Not,
        }
    }
}

impl<L: Link> Beat<L> {
    /// After a frame is presented: waits for the next refresh if this frame is part of a burst, and
    /// says how long it waited. Making or starting the link is the frame's own cost, not the wait.
    ///
    /// A frame of a burst parks the surface it stopped showing, with a link or without one, so no
    /// frame of a burst makes a surface once it has two. A frame out of a burst parks it only while
    /// the link runs, whose pause will drop it. The tick that sets `running` to false does so before
    /// its block goes, and this runs on the thread the block runs on: either this read comes first,
    /// and the block finds what this frame parks, or it comes after, and nothing is parked. A frame of
    /// a burst between that tick and its block sets `running` again, and the block leaves the spare to
    /// the next pause.
    ///
    /// Nobody has read a frame out of a burst while `running` is true and the link is not ticking, the
    /// display asleep for one: it parks, and holds a second surface until a tick or the close. That is
    /// one surface at most, as the next frame takes it.
    fn wait(&mut self, clock: &Clock, make: impl FnOnce() -> Option<L>) -> Waited {
        let begin = Instant::now();
        let burst = self
            .last_frame
            .is_some_and(|last| begin.duration_since(last) <= BURST);
        self.last_frame = Some(begin);
        if !burst {
            return Waited {
                took: Duration::ZERO,
                parks: clock.beats().running,
            };
        }
        if matches!(self.link, Made::Not) {
            let made = make();
            let took = begin.elapsed();
            if made.is_some() {
                let mut beats = clock.beats();
                beats.stats.longest_make = beats.stats.longest_make.max(Some(took));
            }
            self.link = made.map_or(Made::Never, Made::Yes);
        }
        if !matches!(self.link, Made::Yes(_)) {
            return Waited::in_burst(Duration::ZERO);
        }
        let mut beats = clock.beats();
        beats.quiet = 0;
        if !beats.running {
            beats.running = true;
            self.seen = beats.stats.ticks;
            drop(beats);
            self.start_link(clock);
            return Waited::in_burst(Duration::ZERO);
        }
        let seen = self.seen;
        if beats.stats.ticks == seen {
            beats.stats.waits += 1;
        }
        // One limit for the whole wait: a spurious wake-up does not start it again.
        let (mut beats, wait) = clock
            .ticked
            .wait_timeout_while(beats, clock.longest, |beats| beats.stats.ticks == seen)
            .unwrap_or_else(PoisonError::into_inner);
        let timed_out = wait.timed_out();
        if timed_out {
            beats.stats.timeouts += 1;
        }
        self.seen = beats.stats.ticks;
        drop(beats);
        let end = Instant::now();
        self.last_frame = Some(end);
        if timed_out {
            // A display with no refresh, or a hidden window. `running` stays, so every frame waits
            // and they keep to 20 f/s, and a stalled link is started again.
            self.start_link(clock);
        }
        Waited::in_burst(end.duration_since(begin))
    }

    /// Lets the link tick, with no lock of the clock's held. A link that says it never will leaves
    /// this window's frames unheld.
    fn start_link(&mut self, clock: &Clock) {
        let Made::Yes(link) = &mut self.link else {
            return;
        };
        if link.start() {
            clock.beats().stats.starts += 1;
        } else {
            clock.beats().running = false;
            self.link = Made::Never;
        }
    }

    /// The window is going. A later window makes a link of its own.
    fn close(&mut self, clock: &Clock) {
        if let Made::Yes(link) = &mut self.link {
            link.close();
            let mut beats = clock.beats();
            beats.running = false;
            beats.quiet = 0;
        }
        *self = Self::default();
    }
}

/// One window's hold, on the real link and the process's clock.
#[derive(Default)]
pub(crate) struct Hold(Beat<link::DisplayLink>);

impl Hold {
    /// See [`Beat::wait`].
    pub(crate) fn wait(&mut self, window: &Window) -> Waited {
        self.0.wait(&CLOCK, || link::DisplayLink::make(window))
    }

    /// See [`Beat::close`].
    pub(crate) fn close(&mut self) {
        self.0.close(&CLOCK);
    }
}

/// The counters so far, over every window of the process.
pub fn stats() -> Stats {
    CLOCK.beats().stats
}

#[cfg(test)]
mod tests;
