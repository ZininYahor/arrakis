//! What `tests/idle.rs`, `tests/observers.rs` and `tests/closed_keeps.rs`
//! share: their cases run one after another on the main thread, under a
//! watchdog, and wait on the headless pump (`arrakis::idle`) with the bounds
//! below.
//!
//! The bounds are wide on purpose: a loaded machine may hold any one step
//! for half a second ([`STALL`]). A stall only makes a wait longer.
//! - A wait that must last its whole deadline may fall short of it by
//!   [`SLACK`], and overrun it by [`LATE`].
//! - "At once" is under [`AT_ONCE`], and every wait it is told apart from is
//!   at least twice that.
//! - A wait that a wake ends is checked against when the wake was sent: it
//!   ends after it, and at once.

// Each binary uses its own part of this module.
#![allow(dead_code)]

use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use arrakis::idle;
use dispatch2::{DispatchQueue, DispatchTime};
use fastframe_shell::Headless;

/// Fails the whole test, naming the case that was running.
macro_rules! check {
    ($ok:expr, $($why:tt)+) => {
        if !$ok {
            $crate::pump::fail(&format!($($why)+));
        }
    };
}

/// A case that runs this long never gets its stop or its wake: it fails
/// instead of hanging.
pub const CASE_LIMIT: Duration = Duration::from_secs(30);
/// The longest a loaded machine may hold any one step. No bound below may
/// fail for it.
pub const STALL: Duration = Duration::from_millis(500);
/// What load may add to a wait: twice a [`STALL`].
pub const LATE: Duration = Duration::from_millis(1000);
/// A wait that should end at once ends within this: a [`STALL`], and as much
/// again.
pub const AT_ONCE: Duration = Duration::from_millis(1000);
/// What a wait that should last its whole deadline may fall short by: the
/// timer's clock is not `Instant`'s. Load never shortens a wait.
pub const SLACK: Duration = Duration::from_millis(50);
/// A deadline that only a wake should end, far beyond [`AT_ONCE`].
pub const LONG: Duration = Duration::from_secs(4);
/// A deadline a wait should last whole, against an outcome that ends at
/// once: twice [`AT_ONCE`].
pub const WHOLE: Duration = Duration::from_secs(2);
/// The slice the shell hands `idle`.
pub const SLICE: Duration = Duration::from_millis(150);

/// The case that runs now, and since when: what the watchdog reports.
static CURRENT: Mutex<Option<(&'static str, Instant)>> = Mutex::new(None);

/// Runs one case. The watchdog counts its time from here.
pub fn case(name: &'static str, body: impl FnOnce()) {
    *CURRENT.lock().expect("the current case") = Some((name, Instant::now()));
    body();
    println!("ok   {name}");
}

pub fn fail(why: &str) -> ! {
    let name = CURRENT
        .lock()
        .ok()
        .and_then(|current| *current)
        .map_or("before the first case", |(name, _)| name);
    eprintln!("FAIL {name}: {why}");
    process::exit(1)
}

/// A thread that ends the test when one case runs too long: a stop or a
/// wake that never comes would otherwise hang it.
pub fn watch() {
    thread::spawn(|| {
        loop {
            thread::sleep(Duration::from_millis(250));
            let current = *CURRENT.lock().expect("the current case");
            if let Some((name, since)) = current
                && since.elapsed() > CASE_LIMIT
            {
                eprintln!(
                    "FAIL {name}: still running after {CASE_LIMIT:?}; its stop or its wake \
                     never came"
                );
                process::exit(1);
            }
        }
    });
}

pub fn timed(work: impl FnOnce()) -> Duration {
    let started = Instant::now();
    work();
    started.elapsed()
}

/// How long `idle(slice)` takes when the tick asked to wait `deadline`.
pub fn wait_for(deadline: Duration, slice: Duration) -> Duration {
    timed(|| {
        idle::publish(Headless::WaitFor(deadline));
        idle::idle(slice);
    })
}

/// Whether a wait lasted `deadline`: short of it by [`SLACK`] at most, and
/// over it by [`LATE`] at most.
pub fn lasts_whole(waited: Duration, deadline: Duration) -> bool {
    waited + SLACK >= deadline && waited < deadline + LATE
}

/// Whether a wait that `ended` was ended by a wake sent at `woke`: after it,
/// and at once.
pub fn ended_by(woke: Instant, ended: Instant) -> bool {
    ended >= woke && ended - woke < AT_ONCE
}

/// How far `ended` is from `woke`, in words, for a failure.
pub fn apart(woke: Instant, ended: Instant) -> String {
    if ended >= woke {
        format!("{:?} after", ended - woke)
    } else {
        format!("{:?} before", woke - ended)
    }
}

/// Runs `work` on the main queue after `delay`: in whatever loop the main
/// thread runs by then. The main queue drains only inside a run loop.
pub fn after(delay: Duration, work: impl FnOnce() + Send + 'static) {
    let when = DispatchTime::try_from(delay).expect("a delay in range");
    DispatchQueue::main().after(when, work).expect("queued");
}

/// A moment that a block or another thread notes, for the main thread.
#[derive(Clone, Default)]
pub struct Moment(Arc<Mutex<Option<Instant>>>);

impl Moment {
    pub fn note(&self) {
        *self.0.lock().expect("the moment") = Some(Instant::now());
    }

    pub fn get(&self) -> Instant {
        self.0
            .lock()
            .expect("the moment")
            .unwrap_or_else(|| fail("the block that notes the moment never ran"))
    }
}

/// Goes up once the main thread runs a loop. A block on the main queue runs
/// only in one, and the main thread of these tests runs nothing but the
/// pump, so a thread that waits for the gate reaches the pump that is
/// running, never a wait that has not started.
#[derive(Clone)]
pub struct Gate(Arc<AtomicBool>);

impl Gate {
    /// Call it before the `idle` that the gate is for.
    pub fn new() -> Self {
        let up = Arc::new(AtomicBool::new(false));
        let flag = up.clone();
        DispatchQueue::main().exec_async(move || flag.store(true, Ordering::SeqCst));
        Self(up)
    }

    pub fn wait(&self) {
        while !self.0.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
    }
}
