//! A frame's waits on the GPU, each with a limit: the one way this painter could leave the screen
//! stuck is a hang.
//!
//! What this adds to a healthy frame is kept small, as these waits sit on the frame's path and where a
//! frame runs moves its energy:
//! - One block and one [`Frames`] for the painter's life, so a frame allocates nothing of ours. Each
//!   frame adds that block twice to its command buffer.
//! - On Metal's own thread, the block runs twice a frame: a read of the status, the gate taken and let
//!   go, and a wake that finds nobody when the main thread is not waiting.
//! - The main thread sleeps only when the status it reads has not moved yet. Then this wake takes the
//!   place of Metal's own. It starts no thread and no timer: the limit is the sleep's own.
//! - Each read of the status is compared with `Error`, and a read that goes on to sleep first reads
//!   the flag. No send to Metal, no lock and no allocation of this runs on a healthy frame.

use std::ptr::NonNull;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use ::winit_crate::window::Window;
use block2::RcBlock;
use objc2::rc::autoreleasepool;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus};

use crate::failure::{self, Flag};

/// The longest the main thread waits for one command buffer. A timeout goes to [`failure::fail`].
const LONGEST: Duration = Duration::from_secs(1);

/// What Metal calls for both stages. It takes the buffer it was added to.
pub(super) type Handler = RcBlock<dyn Fn(NonNull<ProtocolObject<dyn MTLCommandBuffer>>)>;

/// How far a frame's wait asks a command buffer to have come.
#[derive(Clone, Copy, Debug)]
pub(super) enum Stage {
    /// On its way to the GPU, in order: what a swap gives.
    Scheduled,
    /// Done, so what it read is ours again.
    Completed,
}

impl Stage {
    /// The status only moves on, and its last step is `Completed` or `Error`: the stage after these
    /// never comes, and an error has finished too.
    pub(super) fn reached_by(self, status: MTLCommandBufferStatus) -> bool {
        let least = match self {
            Self::Scheduled => MTLCommandBufferStatus::Scheduled,
            Self::Completed => MTLCommandBufferStatus::Completed,
        };
        status.0 >= least.0
    }

    fn why(self) -> &'static str {
        match self {
            Self::Scheduled => "a frame was not scheduled in time",
            Self::Completed => "a frame did not complete in time",
        }
    }
}

/// Where Metal's thread tells the main thread's waits that a command buffer moved on. It holds no
/// pointer to a painter, so a handler that comes after the painter went touches only this.
pub(super) struct Frames {
    /// Held across a read of the status and a wake, so that no wake falls between that read and the
    /// sleep that follows it. It guards no data: the status is Metal's.
    gate: Mutex<()>,
    moved: Condvar,
    /// The longest a wait lasts: [`LONGEST`], unless a test must not spend it.
    longest: Duration,
    flag: &'static Flag,
    /// The window that a failure asks a frame of, once it has one.
    window: Mutex<Weak<Window>>,
}

impl Frames {
    pub(super) fn new(flag: &'static Flag) -> Self {
        Self::with_limit(flag, LONGEST)
    }

    pub(super) fn with_limit(flag: &'static Flag, longest: Duration) -> Self {
        Self {
            gate: Mutex::new(()),
            moved: Condvar::new(),
            longest,
            flag,
            window: Mutex::new(Weak::new()),
        }
    }

    fn gate(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn window(&self) -> MutexGuard<'_, Weak<Window>> {
        self.window.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// From now on a failure asks `window` for a frame. A lock no sleep holds: only [`Self::fail`] and
    /// this take it, and neither while it blocks.
    pub(super) fn set_window(&self, window: Weak<Window>) {
        *self.window() = window;
    }

    pub(super) fn failed(&self) -> bool {
        self.flag.is_up()
    }

    /// See [`failure::fail`].
    pub(super) fn fail(&self, why: &str) {
        let window = self.window().clone();
        failure::fail(self.flag, why, window);
    }

    /// Waits for `buffer` to reach `stage`, for at most the limit. With the flag up it does not wait:
    /// a stalled GPU costs one limit, not one for each frame in flight, and the failed painter draws
    /// nothing more that would need the frames to have finished. Metal keeps what a command buffer
    /// uses until it completes, so letting go of one early frees nothing it reads.
    pub(super) fn wait_for(&self, buffer: &ProtocolObject<dyn MTLCommandBuffer>, stage: Stage) {
        self.wait_on(stage, || buffer.status(), || why_it_failed(buffer));
    }

    /// [`Self::wait_for`], on what reads the buffer. A status of `Error` fails here: this thread can
    /// read it before the handler runs on Metal's, and with the flag still down `draw` would return
    /// well and `read_screen` hand out a texture the GPU gave up on. The condition runs under the
    /// gate (`wait_timeout_while` holds it for each call, the first included), so the failing does
    /// too: `why_it_failed` (`buffer.error()` and the formatting) and [`Self::fail`] (the flag, the
    /// `log::error!` of the first failure, a dispatch to the main queue when there is a window). It
    /// takes no gate of its own, only the window's lock, and waits for nothing, so a wake on Metal's
    /// thread waits at most for this call. A process takes this path at most a few times.
    fn wait_on(
        &self,
        stage: Stage,
        status: impl Fn() -> MTLCommandBufferStatus,
        why_it_failed: impl Fn() -> String,
    ) {
        self.wait_until(
            || {
                let status = status();
                if status == MTLCommandBufferStatus::Error {
                    self.fail(&why_it_failed());
                }
                stage.reached_by(status)
            },
            stage.why(),
        );
    }

    /// The wait of `ARRAKIS_METAL_FAIL=stall`: the same wait, on a stage that never comes.
    #[cfg(feature = "demo")]
    pub(super) fn stall(&self) {
        self.wait_until(|| false, "ARRAKIS_METAL_FAIL=stall");
    }

    /// A sleep ends when the wait's own stage comes, or when a failure elsewhere raises the flag: the
    /// handler of another buffer wakes it, and it waits no more than a wait that starts with the flag
    /// up. The flag is read after `reached`, so a stage already reached reads no flag.
    fn wait_until(&self, reached: impl Fn() -> bool, why: &str) {
        if self.failed() {
            return;
        }
        let (gate, wait) = self
            .moved
            .wait_timeout_while(self.gate(), self.longest, |()| !reached() && !self.failed())
            .unwrap_or_else(PoisonError::into_inner);
        drop(gate);
        if wait.timed_out() {
            self.fail(why);
        }
    }

    /// On Metal's thread: the status moved, so a wait reads it again.
    fn wake(&self) {
        drop(self.gate());
        self.moved.notify_all();
    }
}

// The block crosses to Metal's thread, and block2 does not ask this of what it captures.
const _: () = {
    const fn crosses_threads<T: Send + Sync>() {}
    crosses_threads::<Frames>();
};

/// The block for both stages of all the painter's frames. It reads a failed status at each, so a GPU
/// that gives up on a window at rest still reaches the flag.
pub(super) fn handler(frames: &Arc<Frames>) -> Handler {
    let frames = Arc::clone(frames);
    RcBlock::new(
        move |buffer: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
            // SAFETY: Metal passes the live command buffer this block was added to, for the call.
            let buffer = unsafe { buffer.as_ref() };
            if buffer.status() == MTLCommandBufferStatus::Error {
                frames.fail(&why_it_failed(buffer));
            }
            frames.wake();
        },
    )
}

/// What Metal says of a command buffer whose status is an error. Metal's thread has its own pools, and
/// this keeps what it autoreleases short all the same: it runs at most a few times in a process.
fn why_it_failed(buffer: &ProtocolObject<dyn MTLCommandBuffer>) -> String {
    autoreleasepool(|_| {
        buffer.error().map_or_else(
            || "a command buffer failed, and Metal does not say why".to_owned(),
            |error| format!("a command buffer failed: {}", error.localizedDescription()),
        )
    })
}

/// What needs no command buffer: the stages, a wait a failure elsewhere wakes, and what a wait does
/// with a status it reads, which here is forged, as no real `Error` can be made. The waits on real
/// Metal are read in `painter/tests.rs`, with the helpers of that file.
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Instant;

    use super::*;

    /// Far past what any of these takes, and a wait that ran it out would miss the bound below.
    const LIMIT: Duration = Duration::from_secs(10);

    fn frames_of_their_own(limit: Duration) -> (&'static Flag, Arc<Frames>) {
        // For the process's life, as the flag a painter holds is.
        let flag: &'static Flag = Box::leak(Box::default());
        (flag, Arc::new(Frames::with_limit(flag, limit)))
    }

    /// A wait that ran out its limit would take twice this, so that a slow machine cannot pass for a
    /// wait that ended at once.
    fn assert_it_did_not_wait(from: Instant, what: &str) {
        let waited = from.elapsed();
        assert!(waited < LIMIT / 2, "{what}: the wait took {waited:?}");
    }

    #[test]
    fn a_wait_asleep_on_one_buffer_ends_when_a_failure_on_another_wakes_it() {
        let (flag, frames) = frames_of_their_own(LIMIT);
        let handler = {
            let frames = Arc::clone(&frames);
            // What the handler does for an error on another buffer: raises the flag, then wakes.
            move || {
                std::thread::sleep(Duration::from_millis(50));
                frames.fail("buffer A failed");
                frames.wake();
            }
        };
        let from = Instant::now();
        let handler = std::thread::spawn(handler);
        // Buffer B, whose stage never comes.
        frames.wait_until(|| false, "buffer B never got there");
        assert_it_did_not_wait(from, "woken by buffer A's failure");
        handler.join().expect("the handler's thread");
        assert_eq!(flag.why(), Some("buffer A failed"));
    }

    #[test]
    fn a_wait_that_reads_an_error_first_raises_the_flag_itself_and_says_metals_reason() {
        for stage in [Stage::Scheduled, Stage::Completed] {
            let (flag, frames) = frames_of_their_own(LIMIT);
            let from = Instant::now();
            frames.wait_on(
                stage,
                || MTLCommandBufferStatus::Error,
                || "Metal's reason".to_owned(),
            );
            assert_it_did_not_wait(from, "an error has finished");
            // No handler ran here: the flag is the wait's own doing.
            assert_eq!(flag.why(), Some("Metal's reason"), "{stage:?}");
        }
    }

    #[test]
    fn a_wait_asleep_that_reads_an_error_when_it_wakes_raises_the_flag_itself() {
        for stage in [Stage::Scheduled, Stage::Completed] {
            let (flag, frames) = frames_of_their_own(LIMIT);
            let status = Arc::new(AtomicUsize::new(MTLCommandBufferStatus::Committed.0));
            let moves = {
                let (frames, status) = (Arc::clone(&frames), Arc::clone(&status));
                // A wake with the flag down: the handler has not got as far as its error yet.
                move || {
                    std::thread::sleep(Duration::from_millis(50));
                    status.store(MTLCommandBufferStatus::Error.0, Ordering::SeqCst);
                    frames.wake();
                }
            };
            let from = Instant::now();
            let moves = std::thread::spawn(moves);
            frames.wait_on(
                stage,
                || MTLCommandBufferStatus(status.load(Ordering::SeqCst)),
                || "Metal's reason".to_owned(),
            );
            assert_it_did_not_wait(from, "woken to read an error");
            moves.join().expect("the thread that moved the status");
            assert_eq!(flag.why(), Some("Metal's reason"), "{stage:?}");
        }
    }

    #[test]
    fn a_healthy_status_raises_nothing_and_never_asks_why() {
        for (stage, status) in [
            (Stage::Scheduled, MTLCommandBufferStatus::Scheduled),
            (Stage::Scheduled, MTLCommandBufferStatus::Completed),
            (Stage::Completed, MTLCommandBufferStatus::Completed),
        ] {
            let (flag, frames) = frames_of_their_own(LIMIT);
            let asked = AtomicBool::new(false);
            let from = Instant::now();
            frames.wait_on(
                stage,
                || status,
                || {
                    asked.store(true, Ordering::SeqCst);
                    String::new()
                },
            );
            assert_it_did_not_wait(from, "a stage reached");
            assert_eq!(flag.why(), None, "{stage:?}, {status:?}");
            assert!(!asked.load(Ordering::SeqCst), "{stage:?}, {status:?}");
        }
    }

    /// Every status Metal has, in the order a buffer moves through them.
    const STATUSES: [(&str, MTLCommandBufferStatus); 6] = [
        ("NotEnqueued", MTLCommandBufferStatus::NotEnqueued),
        ("Enqueued", MTLCommandBufferStatus::Enqueued),
        ("Committed", MTLCommandBufferStatus::Committed),
        ("Scheduled", MTLCommandBufferStatus::Scheduled),
        ("Completed", MTLCommandBufferStatus::Completed),
        ("Error", MTLCommandBufferStatus::Error),
    ];

    #[test]
    fn scheduled_is_reached_from_scheduled_on_and_completed_only_by_the_end() {
        // (status, reaches Scheduled, reaches Completed)
        let wanted = [
            (false, false),
            (false, false),
            (false, false),
            (true, false),
            (true, true),
            // A failed buffer has finished too: the stage after it never comes.
            (true, true),
        ];
        for ((name, status), (scheduled, completed)) in STATUSES.into_iter().zip(wanted) {
            assert_eq!(Stage::Scheduled.reached_by(status), scheduled, "{name}");
            assert_eq!(Stage::Completed.reached_by(status), completed, "{name}");
        }
    }

    #[test]
    fn each_stage_says_its_own_reason() {
        assert_ne!(Stage::Scheduled.why(), Stage::Completed.why());
        assert!(Stage::Scheduled.why().contains("scheduled"));
        assert!(Stage::Completed.why().contains("complete"));
    }
}
