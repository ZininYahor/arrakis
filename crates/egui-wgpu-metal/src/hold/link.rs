//! The real link: a `CADisplayLink` made from the window's view on the main thread, ticking on a
//! run loop of its own, as the hold blocks the main thread. Its rules:
//!
//! 1. The thread starts with the link and ends with it, and a failure logs and leaves frames unheld.
//! 2. It runs at user-interactive QoS, as the main thread waits on it.
//! 3. A Mach port is its run loop's first source: a run loop with no source exits at once.
//! 4. `addToRunLoop:forMode:` and every write of `paused` run on this thread. Apple documents neither
//!    as thread-safe, so the main thread hands them over.
//! 5. A tick touches only the process's clock, bar the pause at the end of a burst, which also hands
//!    the main run loop one block that captures nothing: it drops the spare surface there.
//! 6. A close calls `invalidate`, which Apple documents as thread-safe, then stops the run loop.
//!    Nobody joins the thread.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

use ::winit_crate::window::Window;
use block2::RcBlock;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::NSObject;
use objc2::{AnyThread, ClassType as _, define_class, msg_send, sel};
use objc2_app_kit::NSView;
use objc2_core_foundation::{
    CFRetained, CFRunLoop, CFRunLoopMode, kCFRunLoopCommonModes, kCFRunLoopDefaultMode,
};
use objc2_foundation::{NSDefaultRunLoopMode, NSObjectProtocol, NSPort, NSRunLoop};
use objc2_quartz_core::{CADisplayLink, CAFrameRateRange};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

use super::{CLOCK, Link};

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class has no ivars and no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    #[name = "ArrakisHoldTarget"]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        /// The hold's thread, once each refresh while the link runs.
        #[unsafe(method(tick:))]
        fn tick(&self, link: &CADisplayLink) {
            autoreleasepool(|_| {
                if CLOCK.tick() {
                    // This tick is on the thread that may write it (rule 4).
                    link.setPaused(true);
                    drop_spare_on_main();
                }
            });
        }
    }
);

impl Target {
    fn new() -> Retained<Self> {
        // SAFETY: NSObject's plain `init`, on a class with no ivars to set.
        unsafe { msg_send![Self::alloc(), init] }
    }
}

/// How far the hold's thread has come.
#[derive(Default)]
struct Thread {
    /// Its run loop, with the link on it, once it has one.
    run_loop: Option<CFRetained<CFRunLoop>>,
    /// It could not set its run loop up, so no tick will come.
    failed: bool,
    /// A close came, perhaps before the thread got this far.
    closed: bool,
}

/// What the main thread and the hold's thread share.
struct Shared {
    link: Retained<CADisplayLink>,
    thread: Mutex<Thread>,
}

// SAFETY: this crosses to the hold's thread, and only for these uses. `Retained`'s reference count is
// atomic. Off the hold's thread the link takes only `invalidate`, which Apple documents as thread-safe
// ("you can call it from a thread separate to the one in which the display link runs"), and a rate set
// before the thread starts; the rest runs on the hold's thread. A run loop's reference is shared only
// for `CFRunLoopPerformBlock` and `CFRunLoopWakeUp`, as "the functions in Core Foundation are generally
// thread-safe" (Apple's Threading Programming Guide); `stop` runs in a block on its own thread.
unsafe impl Send for Shared {}
// SAFETY: as above.
unsafe impl Sync for Shared {}

impl Shared {
    fn thread(&self) -> MutexGuard<'_, Thread> {
        self.thread.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The hold's thread: puts the link on this thread's run loop and says where that is, or finds
    /// that a close came first. True when the run loop should run. The lock spans the add, so that
    /// `invalidate` comes either before it, and the add is skipped, or after it.
    fn attach(&self) -> bool {
        let mut thread = self.thread();
        if thread.closed {
            return false;
        }
        // SAFETY: both are class methods that take nothing and return an object or nil; each goes to
        // an `Option`, as the bindings' bare `Retained` would panic on nil.
        let (current, port): (Option<Retained<NSRunLoop>>, Option<Retained<NSPort>>) = unsafe {
            (
                msg_send![NSRunLoop::class(), currentRunLoop],
                msg_send![NSPort::class(), port],
            )
        };
        let (Some(current), Some(port), Some(run_loop)) = (current, port, CFRunLoop::current())
        else {
            log::warn!("the hold's thread has no run loop; frames go unheld");
            thread.failed = true;
            return false;
        };
        // SAFETY: this thread's own run loop, so "within the context of the current thread", as
        // Apple asks of NSRunLoop. The port and the link are live, and the mode is AppKit's constant.
        unsafe {
            current.addPort_forMode(&port, NSDefaultRunLoopMode);
            self.link
                .addToRunLoop_forMode(&current, NSDefaultRunLoopMode);
        }
        thread.run_loop = Some(run_loop);
        true
    }
}

/// The hold's thread. A pool lets go of what setting up left autoreleased, before the run loop runs
/// for the link's life. The run loop has no pool of ours, so each tick and each block handed over
/// has its own: what either autoreleases would otherwise stay until the window closes.
fn run(shared: &Shared) {
    set_qos();
    if autoreleasepool(|_| shared.attach()) {
        CFRunLoop::run();
    }
}

fn set_qos() {
    // SAFETY: the call takes two plain values, a class from libc's own enum and a relative priority
    // of 0, within the -15..=0 it accepts. It changes only the calling thread's scheduling class, and
    // a thread std spawned is a plain pthread, which may set its own class.
    let error = unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0)
    };
    if error != 0 {
        // A late tick costs a frame up to `LONGEST`, and hangs nothing.
        log::warn!("unable to run the hold's thread at user-interactive QoS: error {error}");
    }
}

/// Runs `job` on `run_loop`'s thread, inside its run loop in `mode` and a pool of its own, and wakes it.
fn perform(run_loop: &CFRunLoop, mode: &CFRunLoopMode, job: impl Fn() + 'static) {
    let block = RcBlock::new(move || autoreleasepool(|_| job()));
    // SAFETY: the mode is a CFString, as the call asks, and the run loop runs it. CoreFoundation copies
    // the block, and the block owns all it captures.
    unsafe { run_loop.perform_block(Some(mode.as_ref()), Some(&block)) };
    run_loop.wake_up();
}

/// [`perform`], in the only mode the hold's run loop runs.
fn hand_over(run_loop: &CFRunLoop, job: impl Fn() + 'static) {
    // SAFETY: reading a constant that CoreFoundation sets at load and never writes.
    if let Some(mode) = unsafe { kCFRunLoopDefaultMode } {
        perform(run_loop, mode, job);
    }
}

/// Ends the run loop of a thread that runs one, so that the thread returns. A block, not
/// `CFRunLoopStop` from here: a stop before the thread enters its run loop is lost, and a block waits
/// for the run loop to run.
fn stop(run_loop: &CFRunLoop) {
    hand_over(run_loop, || {
        if let Some(current) = CFRunLoop::current() {
            current.stop();
        }
    });
}

/// The pause's other half: the main thread drops the spare surface, in AppKit's own modes, a drag's
/// included. It captures nothing, so no surface or painter crosses here. With no main run loop to find
/// the spare stays until the next lone frame takes it, or the close.
fn drop_spare_on_main() {
    let Some(main) = CFRunLoop::main() else {
        return;
    };
    // SAFETY: reading a constant that CoreFoundation sets at load and never writes.
    if let Some(mode) = unsafe { kCFRunLoopCommonModes } {
        perform(&main, mode, || {
            CLOCK.drop_spare_at_pause(crate::painter::drop_spare);
        });
    }
}

/// The rate of the screen the view is on: none while it is on no screen.
fn rate_of(view: &NSView) -> Option<f32> {
    let frames = view.window()?.screen()?.maximumFramesPerSecond();
    (frames > 0).then_some(frames as f32)
}

/// A range the system would otherwise "dynamically adjust" (`CADisplayLink.h`), held at one rate.
fn set_rate(link: &CADisplayLink, rate: f32) {
    link.setPreferredFrameRateRange(CAFrameRateRange {
        minimum: rate,
        maximum: rate,
        preferred: rate,
    });
}

/// What a start finds of the hold's thread.
enum Found {
    /// It could not set its run loop up, so no tick will come.
    Failed,
    /// It has not added the link yet, or is adding it now. The link has never paused, and starts at
    /// its rate when it is added.
    NotYet,
    /// Its run loop, which takes the start.
    Running(CFRetained<CFRunLoop>),
}

/// Looks without waiting: this is a frame's path, and `attach` holds the lock across calls that have
/// no limit. `start` and `close` both run on the main thread, so only `attach` can be holding it.
fn find(thread: &Mutex<Thread>) -> Found {
    let thread = match thread.try_lock() {
        Ok(thread) => thread,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => return Found::NotYet,
    };
    if thread.failed {
        return Found::Failed;
    }
    thread
        .run_loop
        .clone()
        .map_or(Found::NotYet, Found::Running)
}

/// A window's link, on the main thread. The link follows its view to another screen by itself.
pub(super) struct DisplayLink {
    view: Retained<NSView>,
    shared: Arc<Shared>,
}

impl DisplayLink {
    /// On the main thread. `None` logs: frames then go unheld, and still paint.
    pub(super) fn make(window: &Window) -> Option<Self> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        // SAFETY: winit hands out a live NSView, and retaining one is allowed on any thread.
        let view = unsafe { Retained::retain(handle.ns_view.cast::<NSView>().as_ptr()) }?;
        let target = Target::new();
        // SAFETY: macOS 14's call on a live view, on the main thread, and `tick:` is the target's,
        // with the signature a link calls. It goes to an `Option`, as the binding's bare `Retained`
        // would panic on nil.
        let link: Option<Retained<CADisplayLink>> =
            unsafe { msg_send![&*view, displayLinkWithTarget: &*target, selector: sel!(tick:)] };
        let Some(link) = link else {
            log::warn!("the view gave no display link; frames go unheld");
            return None;
        };
        if let Some(rate) = rate_of(&view) {
            // No other thread has the link yet.
            set_rate(&link, rate);
        }
        let shared = Arc::new(Shared {
            link,
            thread: Mutex::default(),
        });
        let on_thread = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("arrakis-hold".to_owned())
            .spawn(move || run(&on_thread));
        if let Err(error) = spawned {
            log::warn!("unable to start the hold's thread: {error}; frames go unheld");
            shared.link.invalidate();
            return None;
        }
        Some(Self { view, shared })
    }
}

impl Link for DisplayLink {
    fn start(&mut self) -> bool {
        let run_loop = match find(&self.shared.thread) {
            Found::Failed => return false,
            Found::NotYet => return true,
            Found::Running(run_loop) => run_loop,
        };
        let rate = rate_of(&self.view);
        let shared = Arc::clone(&self.shared);
        hand_over(&run_loop, move || {
            if let Some(rate) = rate {
                set_rate(&shared.link, rate);
            }
            shared.link.setPaused(false);
        });
        true
    }

    fn close(&mut self) {
        let run_loop = {
            // This one waits for `attach`, as `invalidate` must come before the add, which `closed`
            // then skips, or after it.
            let mut thread = self.shared.thread();
            thread.closed = true;
            thread.run_loop.take()
        };
        // It releases the target. A tick in flight moves only a clock nobody waits for.
        self.shared.link.invalidate();
        if let Some(run_loop) = run_loop {
            stop(&run_loop);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// A lock to cross to another thread, which a `Thread` may not do by itself, as a run loop's
    /// handle is not `Send`.
    struct Crossing<'a>(&'a Mutex<Thread>);

    // SAFETY: only `found_while_held`'s `Thread` crosses, which holds no run loop, only its flags.
    unsafe impl Send for Crossing<'_> {}

    impl Crossing<'_> {
        fn found(&self) -> &'static str {
            match find(self.0) {
                Found::Failed => "failed",
                Found::NotYet => "not yet",
                Found::Running(_) => "running",
            }
        }
    }

    /// What `find` answers on another thread while this one holds the lock, as `attach` would. A
    /// `find` that blocked fails here on the time limit rather than hanging the run.
    fn found_while_held(thread: &Mutex<Thread>) -> &'static str {
        assert!(thread.lock().expect("a free lock").run_loop.is_none());
        let crossing = Crossing(thread);
        let (answer, answered) = mpsc::channel();
        std::thread::scope(|scope| {
            let _held = thread.lock().expect("a free lock");
            scope.spawn(move || {
                let _ = answer.send(crossing.found());
            });
            let found = answered.recv_timeout(Duration::from_secs(60));
            found.expect("a start that waited for the thread's lock")
        })
    }

    #[test]
    fn a_start_while_the_thread_attaches_does_not_wait_and_says_the_link_will_start() {
        // Even a thread that is about to fail: the frame learns that at its next start.
        for failed in [false, true] {
            let thread = Mutex::new(Thread {
                failed,
                ..Thread::default()
            });
            assert_eq!(found_while_held(&thread), "not yet");
        }
    }

    #[test]
    fn a_start_finds_the_threads_state_with_the_lock_free() {
        let thread = Mutex::new(Thread::default());
        assert!(matches!(find(&thread), Found::NotYet), "no run loop yet");
        thread.lock().expect("a free lock").failed = true;
        assert!(matches!(find(&thread), Found::Failed));
        let running = Mutex::new(Thread {
            run_loop: CFRunLoop::current(),
            ..Thread::default()
        });
        assert!(matches!(find(&running), Found::Running(_)));
    }

    #[test]
    fn a_start_reads_through_a_poisoned_lock() {
        let thread = Mutex::new(Thread {
            failed: true,
            ..Thread::default()
        });
        let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = thread.lock();
            panic!("a panic that holds the lock, on purpose");
        }));
        assert!(died.is_err());
        assert!(thread.is_poisoned());
        assert!(matches!(find(&thread), Found::Failed));
    }

    /// The longest a test waits for a thread to answer. Nothing waits on it when the stop works: it
    /// only turns a stop that does not work into a failure instead of a hang. The thread's work is a
    /// port, a block and a run loop's pass, which take microseconds, so no load on the machine
    /// reaches it.
    const LIMIT: Duration = Duration::from_secs(30);

    /// A run loop's handle on the test's thread, to call `stop` with.
    struct Handle(CFRetained<CFRunLoop>);

    // SAFETY: only `stop` uses it here, from the test's thread: `CFRunLoopPerformBlock` and
    // `CFRunLoopWakeUp`, which "the functions in Core Foundation are generally thread-safe" covers, as
    // for `Shared` above. The thread that owns the run loop only runs it.
    unsafe impl Send for Handle {}

    /// When the stop comes, against the thread entering its run loop.
    #[derive(Clone, Copy, Debug)]
    enum Stop {
        BeforeTheRunLoopRuns,
        AfterItRuns,
    }

    /// A thread that does what `run` does with no link: puts a port on its run loop, the source that
    /// keeps a run loop running, and runs it. Then `stop` is called on its run loop at `when`, and the
    /// thread must end.
    fn stop_ends_the_thread(when: Stop) {
        let (publish, published) = mpsc::channel();
        let (go, go_on) = mpsc::channel::<()>();
        let (inside, is_inside) = mpsc::channel::<()>();
        let (finish, finished) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            // SAFETY: both are class methods that take nothing and return an object or nil, each
            // read into an `Option`, as `attach` reads them.
            let (current, port): (Option<Retained<NSRunLoop>>, Option<Retained<NSPort>>) = unsafe {
                (
                    msg_send![NSRunLoop::class(), currentRunLoop],
                    msg_send![NSPort::class(), port],
                )
            };
            let (Some(current), Some(port), Some(run_loop)) = (current, port, CFRunLoop::current())
            else {
                return;
            };
            // SAFETY: this thread's own run loop, and a live port, as in `attach`.
            unsafe { current.addPort_forMode(&port, NSDefaultRunLoopMode) };
            // A block the run loop runs once it runs, so the test knows when it does.
            hand_over(&run_loop, move || {
                let _ = inside.send(());
            });
            let _ = publish.send(Handle(run_loop));
            if go_on.recv().is_ok() {
                CFRunLoop::run();
            }
            let _ = finish.send(());
        });
        let handle = published
            .recv_timeout(LIMIT)
            .expect("the thread's run loop");
        match when {
            Stop::BeforeTheRunLoopRuns => {
                stop(&handle.0);
                go.send(()).expect("the thread is waiting to go");
            }
            Stop::AfterItRuns => {
                go.send(()).expect("the thread is waiting to go");
                is_inside
                    .recv_timeout(LIMIT)
                    .expect("the run loop ran its first block");
                // It goes on running until it is told to stop: the stop is what ends it.
                assert!(
                    finished.recv_timeout(Duration::from_millis(100)).is_err(),
                    "the thread ended by itself"
                );
                stop(&handle.0);
            }
        }
        finished
            .recv_timeout(LIMIT)
            .unwrap_or_else(|_| panic!("a stop {when:?} did not end the thread"));
    }

    #[test]
    fn a_stop_after_the_run_loop_runs_ends_the_thread() {
        stop_ends_the_thread(Stop::AfterItRuns);
    }

    #[test]
    fn a_stop_before_the_run_loop_runs_ends_the_thread_too() {
        // Rule 6: `CFRunLoopStop` before the thread enters its run loop would be lost, and a block
        // waits for the run loop to run.
        stop_ends_the_thread(Stop::BeforeTheRunLoopRuns);
    }
}
