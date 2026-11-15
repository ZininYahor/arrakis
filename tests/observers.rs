//! The notices that `gamma::watch_system` listens for, on the real
//! notification centers.
//!
//! The six notices (`gamma::system_notices`) are posted in this process, on
//! its own two centers: the workspace's and Foundation's. Nothing here
//! triggers a real system event (no sleep, wake, user switch, quitting app or
//! clock change), opens a window or touches a display. `watch_system`
//! registers once per process, so this is one binary and one sequence of
//! cases.
//!
//! The real centers carry real notices too: any app on the Mac quitting
//! posts one, at any moment. The callback counts and wakes for the test's own
//! posts alone ([`post`]), so no case depends on what the rest of the Mac
//! does.
//!
//! It is its own `main` (`harness = false`): AppKit's loop runs on the main
//! thread only, and libtest's threads are not it. The cases from 4 on need
//! that loop, which is the headless pump `arrakis::idle::idle` runs
//! (`NSApp.run`), as `tests/idle.rs` does. The two share their harness and
//! its wide bounds (`tests/pump/mod.rs`).

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
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{LazyLock, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use arrakis::gamma::{Center, system_notices, watch_system};
    use arrakis::{engine, idle};
    use fastframe_shell::{Headless, Waker};
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSWorkspaceWillSleepNotification;
    use objc2_foundation::{NSNotificationName, NSString, NSSystemTimeZoneDidChangeNotification};

    use crate::pump::{
        Gate, LONG, SLICE, after, apart, case, ended_by, fail, lasts_whole, wait_for, watch,
    };

    /// How many times the callback `watch_system` first got has run for the
    /// test's own posts.
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    /// How many times the callback a second `watch_system` call offers has run.
    static OTHER: AtomicUsize = AtomicUsize::new(0);
    /// Whether the callback does what the app's does: raise a flag and wake
    /// the app through `idle::wake`. Off, it only counts.
    static WAKING: AtomicBool = AtomicBool::new(false);
    /// The flag the callback raises while [`WAKING`].
    static RAISED: AtomicBool = AtomicBool::new(false);
    /// When the callback woke the app, while [`WAKING`].
    static WOKE: Mutex<Option<Instant>> = Mutex::new(None);
    /// The app's waker: no window is attached, as in a closed app.
    static WAKER: LazyLock<Waker> = LazyLock::new(Waker::default);

    thread_local! {
        /// Whether this thread is inside one of the test's own posts.
        static POSTING: Cell<bool> = const { Cell::new(false) };
    }

    /// The callback that `gamma::watch_system` is first given. It ignores a
    /// real notice, which may come in any case, from any app quitting.
    fn heard() {
        if !POSTING.get() {
            return;
        }
        CALLS.fetch_add(1, Ordering::SeqCst);
        if WAKING.load(Ordering::SeqCst) {
            *WOKE.lock().expect("the moment") = Some(Instant::now());
            RAISED.store(true, Ordering::SeqCst);
            idle::wake(&WAKER);
        }
    }

    /// Posts `name` on `center`, from the calling thread, as the test's own.
    /// An observer with no queue runs on this thread before the post returns,
    /// so the flag that tells [`heard`] the post is ours is up exactly while
    /// it runs. A real notice never finds it up: on another thread the flag
    /// is that thread's own, and this thread delivers nothing else until the
    /// post returns.
    fn post(center: Center, name: &NSNotificationName) {
        POSTING.set(true);
        // SAFETY: no object to post for.
        unsafe { center.get().postNotificationName_object(name, None) };
        POSTING.set(false);
    }

    /// The center a notice is not watched on.
    fn other(center: Center) -> Center {
        match center {
            Center::Workspace => Center::Foundation,
            Center::Foundation => Center::Workspace,
        }
    }

    pub fn main() {
        let _main =
            MainThreadMarker::new().expect("a test without a harness runs on the main thread");
        watch();
        // The first run of `NSApp` finishes launching the app, once. A case
        // is about the notices, not about that.
        idle::publish(Headless::Wait);
        idle::idle(Duration::from_millis(10));

        case("1. each of the six notices calls back once", || {
            watch_system(heard);
            for (center, name) in system_notices() {
                let before = CALLS.load(Ordering::SeqCst);
                post(center, name);
                let calls = CALLS.load(Ordering::SeqCst) - before;
                check!(calls == 1, "{name}: {calls} calls, not one");
            }
        });

        case(
            "2. an unwatched notice, and a watched one on the other center, call nothing",
            || {
                let before = CALLS.load(Ordering::SeqCst);
                // SAFETY: AppKit's own constant.
                post(Center::Workspace, unsafe {
                    NSWorkspaceWillSleepNotification
                });
                post(
                    Center::Foundation,
                    &NSString::from_str("ArrakisTestUnwatchedNotice"),
                );
                for (center, name) in system_notices() {
                    post(other(center), name);
                    let calls = CALLS.load(Ordering::SeqCst) - before;
                    check!(calls == 0, "{name}, on the other center: {calls} calls");
                }
            },
        );

        case("3. a second call to watch_system registers nothing", || {
            watch_system(|| {
                OTHER.fetch_add(1, Ordering::SeqCst);
            });
            let before = CALLS.load(Ordering::SeqCst);
            for (center, name) in system_notices() {
                post(center, name);
                check!(
                    OTHER.load(Ordering::SeqCst) == 0,
                    "{name}: the second callback ran"
                );
            }
            let calls = CALLS.load(Ordering::SeqCst) - before;
            check!(calls == 6, "the first callback ran {calls} times of 6");
        });

        case(
            "4. a notice posted from another thread ends the headless wait",
            || {
                quiet();
                let gate = Gate::new();
                let posting = thread::spawn(move || {
                    gate.wait();
                    thread::sleep(Duration::from_millis(100));
                    // Looked up on this thread: an `NSString` is not `Send`.
                    let (center, name) = system_notices()[0];
                    post(center, name);
                });
                waking();
                wait_for(LONG, SLICE);
                let ended = Instant::now();
                posting.join().expect("the posting thread");
                woke_the_app(ended, "a notice from another thread");
            },
        );

        case(
            "5. a notice posted inside the pump, from a block on the main queue, ends it",
            || {
                quiet();
                waking();
                after(Duration::from_millis(100), || {
                    let (center, name) = system_notices()[0];
                    post(center, name);
                });
                wait_for(LONG, SLICE);
                woke_the_app(Instant::now(), "a notice from the main queue");
            },
        );

        case(
            "6. the same for Foundation's center, the clock and the time zone",
            || {
                // The block looks its name up on the main thread, as an
                // `NSString` is not `Send`.
                for index in [4, 5] {
                    let (center, name) = system_notices()[index];
                    let what = name.to_string();
                    check!(
                        center == Center::Foundation,
                        "{what} is not on Foundation's center"
                    );
                    quiet();
                    waking();
                    after(Duration::from_millis(100), move || {
                        let (center, name) = system_notices()[index];
                        post(center, name);
                    });
                    wait_for(LONG, SLICE);
                    woke_the_app(Instant::now(), &what);
                }
            },
        );

        case(
            "7. the time-zone notice drops the clock's zone, and no other notice does",
            || {
                // SAFETY: Foundation's own constant.
                let zone_notice = unsafe { NSSystemTimeZoneDidChangeNotification };
                for (center, name) in system_notices() {
                    engine::local_minute_of_day();
                    check!(engine::zone_is_kept(), "the clock kept no zone");
                    post(center, name);
                    let dropped = !engine::zone_is_kept();
                    let expected = *name == *zone_notice;
                    check!(
                        dropped == expected,
                        "{name}: the clock's zone was {}",
                        if dropped { "dropped" } else { "kept" }
                    );
                }
            },
        );

        println!("all cases ok");
    }

    /// From here the callback does what the app's does: it raises a flag and
    /// wakes the app through `idle::wake`.
    fn waking() {
        RAISED.store(false, Ordering::SeqCst);
        *WOKE.lock().expect("the moment") = None;
        WAKING.store(true, Ordering::SeqCst);
    }

    /// After a headless wait that `waking` was for, which came back at
    /// `ended`: fails unless the callback ran for the test's post and raised
    /// its flag, and the wait ended after the callback woke the app, and at
    /// once.
    fn woke_the_app(ended: Instant, what: &str) {
        WAKING.store(false, Ordering::SeqCst);
        check!(
            RAISED.load(Ordering::SeqCst),
            "{what}: the callback did not run"
        );
        let woke = WOKE
            .lock()
            .expect("the moment")
            .unwrap_or_else(|| fail(&format!("{what}: the callback never woke the app")));
        check!(
            ended_by(woke, ended),
            "{what}: a {LONG:?} wait ended {} the callback woke the app",
            apart(woke, ended)
        );
    }

    /// A short wait with nothing sent lasts its whole deadline: no wake from
    /// an earlier case was left pending.
    fn quiet() {
        let deadline = Duration::from_millis(300);
        let waited = wait_for(deadline, SLICE);
        check!(
            lasts_whole(waited, deadline),
            "a {deadline:?} wait with nothing sent took {waited:?}: a wake was left pending"
        );
    }
}
