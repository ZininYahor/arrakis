//! The headless wait on the real AppKit loop (`src/idle.rs`).
//!
//! AppKit runs its event loop on the main thread only, and libtest's threads
//! are not it, so this test is its own `main` (`harness = false`). It opens
//! no window and touches no display: only the pump `arrakis::idle::idle`
//! runs, and a stand-in for winit's loop, which is `NSApp.run` ended by
//! `idle`'s own stop.
//!
//! The failures `idle` has to rule out:
//! - a stop that reaches winit's loop (cases 9, 10 and 13),
//! - a wake that is lost (cases 7, 8, 9, 11, 12 and 13),
//! - a window wake that ends the wait, which it must leave alone (case 14).
//!
//! The bounds are wide on purpose; they, the watchdog and the waits are in
//! `tests/pump/mod.rs`, which `tests/observers.rs` shares.
//!
//! A thread a case spawns is joined before the case ends, so a wake that
//! lands late cannot stop a later wait.

#[cfg(target_os = "macos")]
#[macro_use]
mod pump;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    cases::main();
}

#[cfg(target_os = "macos")]
mod cases {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};

    use arrakis::idle;
    use fastframe_shell::{Headless, Waker};
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    use crate::pump::{
        AT_ONCE, Gate, LONG, Moment, SLACK, SLICE, STALL, WHOLE, after, apart, case, ended_by,
        lasts_whole, timed, wait_for, watch,
    };

    pub fn main() {
        let mtm =
            MainThreadMarker::new().expect("a test without a harness runs on the main thread");
        watch();
        // The first run of `NSApp` finishes launching the app, once. A case
        // is about the pump, not about that.
        idle::publish(Headless::Wait);
        idle::idle(Duration::from_millis(10));

        case(
            "5. a wait without a wake runs its whole deadline, not the slice",
            || {
                let waited = wait_for(WHOLE, SLICE);
                check!(
                    lasts_whole(waited, WHOLE),
                    "waited {waited:?} for a {WHOLE:?} deadline and a {SLICE:?} slice"
                );
            },
        );

        case(
            "6. with no deadline published it keeps to the slice",
            || {
                // A deadline an earlier tick set is gone once a tick asks for no
                // end.
                idle::publish(Headless::WaitFor(Duration::from_secs(5)));
                idle::publish(Headless::Wait);
                let slice = Duration::from_millis(300);
                let waited = timed(|| idle::idle(slice));
                check!(
                    lasts_whole(waited, slice),
                    "waited {waited:?} for a {slice:?} slice, after a 5 s deadline was cleared"
                );
            },
        );

        case(
            "6b. a deadline nearer than the slice, or past, keeps to the slice",
            || {
                let slice = Duration::from_millis(300);
                for deadline in [Duration::from_millis(30), Duration::ZERO] {
                    let waited = wait_for(deadline, slice);
                    check!(
                        lasts_whole(waited, slice),
                        "waited {waited:?} for a {slice:?} slice and a {deadline:?} deadline"
                    );
                }
            },
        );

        case("7. an interrupt from another thread ends the wait", || {
            let waker = Waker::default();
            let pump = Gate::new();
            let waking = thread::spawn(move || {
                pump.wait();
                thread::sleep(Duration::from_millis(100));
                let woke = Instant::now();
                idle::wake(&waker);
                woke
            });
            wait_for(LONG, SLICE);
            let ended = Instant::now();
            let woke = waking.join().expect("the waking thread");
            check!(
                ended_by(woke, ended),
                "a {LONG:?} wait ended {} the wake",
                apart(woke, ended)
            );
        });

        case(
            "8. an interrupt from the main thread, inside the pump, ends it",
            || {
                // A menu-bar click's path: muda calls the wake on the main
                // thread, from a block the pump runs.
                let woke = Moment::default();
                let noted = woke.clone();
                after(Duration::from_millis(100), move || {
                    noted.note();
                    idle::interrupt();
                });
                wait_for(LONG, SLICE);
                let ended = Instant::now();
                let woke = woke.get();
                check!(
                    ended_by(woke, ended),
                    "a {LONG:?} wait ended {} the interrupt",
                    apart(woke, ended)
                );
            },
        );

        case(
            "9. an interrupt while no pump runs stops nothing, and the next wait returns at once",
            || {
                // winit's loop, as far as `idle` can tell: a `run` that is
                // not its own, ended by a stop of its own.
                let own_stop = Duration::from_millis(1500);
                let interrupting = thread::spawn(|| {
                    thread::sleep(Duration::from_millis(100));
                    let sent = Instant::now();
                    idle::interrupt();
                    sent
                });
                let foreign = timed(|| {
                    after(own_stop, stop_app);
                    NSApplication::sharedApplication(mtm).run();
                });
                let ended = Instant::now();
                let sent = interrupting.join().expect("the interrupting thread");
                check!(
                    sent < ended,
                    "the interrupt came {} the loop ended: the case proves nothing",
                    apart(sent, ended)
                );
                check!(
                    lasts_whole(foreign, own_stop),
                    "a loop that is not the pump's ran {foreign:?}; its own stop is at {own_stop:?}"
                );

                let at_once = wait_for(LONG, SLICE);
                check!(
                    at_once < AT_ONCE,
                    "the {LONG:?} wait after an interrupt that stopped nothing took {at_once:?}"
                );

                // The interrupt is answered, and its block ran already: it
                // stops nothing more.
                let waited = wait_for(WHOLE, SLICE);
                check!(
                    lasts_whole(waited, WHOLE),
                    "waited {waited:?} for a {WHOLE:?} deadline after the interrupt was answered"
                );
            },
        );

        case("10a. the timer is cancelled after an early return", || {
            // No pump runs: this leaves the interrupt for the next wait.
            idle::interrupt();
            let at_once = wait_for(WHOLE, SLICE);
            check!(
                at_once < AT_ONCE,
                "the {WHOLE:?} wait took {at_once:?} with an interrupt pending"
            );
            // A timer left over from that wait would stop this one when the
            // first deadline came: `stopRun:` acts on any pump that runs.
            // (The interrupt's block runs in it and finds nothing left to
            // answer.)
            let deadline = WHOLE + AT_ONCE + STALL;
            let waited = wait_for(deadline, SLICE);
            check!(
                lasts_whole(waited, deadline),
                "waited {waited:?} for a {deadline:?} deadline: a timer from the early return stopped it"
            );
        });

        case(
            "10b. the timer is cancelled after a stop that was not its own",
            || {
                let stopped = Moment::default();
                let noted = stopped.clone();
                after(Duration::from_millis(100), move || {
                    noted.note();
                    stop_app();
                });
                wait_for(WHOLE, SLICE);
                let ended = Instant::now();
                let stopped = stopped.get();
                check!(
                    ended_by(stopped, ended),
                    "a {WHOLE:?} wait ended {} an outside stop",
                    apart(stopped, ended)
                );
                // The first wait's timer would stop this one when the first
                // deadline came.
                let deadline = WHOLE + AT_ONCE + STALL;
                let waited = wait_for(deadline, SLICE);
                check!(
                    lasts_whole(waited, deadline),
                    "waited {waited:?} for a {deadline:?} deadline: the earlier wait's timer stopped it"
                );
            },
        );

        case(
            "11. a burst of wakes ends one wait, and the waits after it settle",
            || {
                const THREADS: usize = 8;
                let waker = Waker::default();
                let pump = Gate::new();
                let together = Arc::new(Barrier::new(THREADS));
                let waking: Vec<_> = (0..THREADS)
                    .map(|_| {
                        let (waker, pump, together) =
                            (waker.clone(), pump.clone(), together.clone());
                        thread::spawn(move || {
                            pump.wait();
                            together.wait();
                            let woke = Instant::now();
                            idle::wake(&waker);
                            woke
                        })
                    })
                    .collect();
                wait_for(LONG, SLICE);
                let ended = Instant::now();
                let first = waking
                    .into_iter()
                    .map(|thread| thread.join().expect("a waking thread"))
                    .min()
                    .expect("eight wakes");
                check!(
                    ended_by(first, ended),
                    "a {LONG:?} wait ended {} the first of a burst of wakes",
                    apart(first, ended)
                );
                settles("the burst");
            },
        );

        case(
            "12. paced wakes, a quick fade's, end at most one wait a pace, and the last is answered",
            || {
                // The engine's status through a quick fade: every 33 ms for
                // 1.2 s.
                const WAKES: usize = 36;
                let waker = Waker::default();
                let pump = Gate::new();
                let last = Arc::new(AtomicBool::new(false));
                let waking = {
                    let last = last.clone();
                    thread::spawn(move || {
                        pump.wait();
                        for wake in 0..WAKES {
                            if wake + 1 == WAKES {
                                last.store(true, Ordering::SeqCst);
                            }
                            idle::wake_paced(&waker);
                            thread::sleep(Duration::from_millis(33));
                        }
                    })
                };
                let started = Instant::now();
                let mut waits = 0u32;
                loop {
                    let waited = wait_for(LONG, SLICE);
                    waits += 1;
                    // A paced wake was on its way through each of them.
                    check!(
                        waited < AT_ONCE,
                        "wait {waits} of the fade took {waited:?} of its {LONG:?}"
                    );
                    if last.load(Ordering::SeqCst) {
                        break;
                    }
                }
                let took = started.elapsed();
                waking.join().expect("the waking thread");
                // Each wait lasts a pace at least, however the wakes came:
                // unpaced, there would be one a wake.
                let most = (took.as_secs_f64() / (idle::PACE.as_secs_f64() * 0.75)) as u32 + 1;
                check!(
                    waits <= most && waits < WAKES as u32 / 2,
                    "{WAKES} paced wakes over {took:?} ended {waits} waits, more than {most}"
                );
                settles("the fade");
            },
        );

        case(
            "13. a paced wake while no pump runs stops nothing, and the next wait lasts a pace",
            || {
                let own_stop = Duration::from_millis(800);
                let waking = thread::spawn(|| {
                    thread::sleep(Duration::from_millis(100));
                    let sent = Instant::now();
                    idle::interrupt_paced();
                    sent
                });
                let foreign = timed(|| {
                    after(own_stop, stop_app);
                    NSApplication::sharedApplication(mtm).run();
                });
                let ended = Instant::now();
                let sent = waking.join().expect("the waking thread");
                check!(
                    sent < ended,
                    "the paced wake came {} the loop ended: the case proves nothing",
                    apart(sent, ended)
                );
                check!(
                    lasts_whole(foreign, own_stop),
                    "a loop that is not the pump's ran {foreign:?}; its own stop is at {own_stop:?}"
                );

                let paced = wait_for(LONG, SLICE);
                check!(
                    paced + SLACK >= idle::PACE && paced < AT_ONCE,
                    "the {LONG:?} wait after a paced wake took {paced:?}, not a pace"
                );
                let waited = wait_for(WHOLE, SLICE);
                check!(
                    lasts_whole(waited, WHOLE),
                    "waited {waited:?} for a {WHOLE:?} deadline after the paced wake was answered"
                );
            },
        );

        case(
            "14. a window wake, from another thread and from inside the pump, leaves the wait alone",
            || {
                // The engine's level-only status, with no window open: it
                // waits in the channel for the app's next tick.
                let waker = Waker::default();
                let pump = Gate::new();
                let sent = Arc::new(AtomicUsize::new(0));
                let waking = {
                    let (waker, sent) = (waker.clone(), sent.clone());
                    thread::spawn(move || {
                        pump.wait();
                        thread::sleep(Duration::from_millis(100));
                        idle::wake_window(&waker);
                        sent.fetch_add(1, Ordering::SeqCst);
                    })
                };
                let inside = sent.clone();
                after(Duration::from_millis(200), move || {
                    idle::wake_window(&waker);
                    inside.fetch_add(1, Ordering::SeqCst);
                });
                let waited = wait_for(WHOLE, SLICE);
                waking.join().expect("the waking thread");
                check!(
                    sent.load(Ordering::SeqCst) == 2,
                    "{} of the 2 window wakes ran during a {WHOLE:?} wait",
                    sent.load(Ordering::SeqCst)
                );
                check!(
                    lasts_whole(waited, WHOLE),
                    "waited {waited:?} for a {WHOLE:?} deadline with window wakes sent"
                );
                // Nothing was left pending for the wait after it.
                let next = wait_for(WHOLE, SLICE);
                check!(
                    lasts_whole(next, WHOLE),
                    "waited {next:?} for a {WHOLE:?} deadline after the window wakes"
                );
            },
        );

        println!("all cases ok");
    }

    /// After a burst whose wakes all came before it returned: a wake that
    /// landed after the pump ended may be left for the next wait to answer.
    /// That one returns at once, or waits its whole deadline. The one after
    /// it always waits.
    fn settles(burst: &str) {
        let first = wait_for(WHOLE, SLICE);
        check!(
            first < AT_ONCE || lasts_whole(first, WHOLE),
            "the wait after {burst} took {first:?}: neither at once nor its whole {WHOLE:?}"
        );
        let second = wait_for(WHOLE, SLICE);
        check!(
            lasts_whole(second, WHOLE),
            "the second wait after {burst} took {second:?}, not its whole {WHOLE:?}"
        );
    }

    /// Ends the `NSApp.run` that is running with `idle`'s own stop, which
    /// does not look whose loop it is. Call it from the main queue.
    fn stop_app() {
        idle::stop(MainThreadMarker::new().expect("a main-queue block runs on the main thread"));
    }
}
