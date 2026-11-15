//! What the app keeps with its window closed: the timer of each headless tick
//! (`src/idle.rs`) and what the closed window's last frame holds
//! (`ui::appkit::release_closed_window`), and what a headless frame
//! autoreleases.
//!
//! AppKit's windows and loop are the main thread's, and libtest's threads are
//! not it, so this test is its own `main` (`harness = false`), as
//! `tests/idle.rs` is. Its main thread has no autorelease pool, as the app's
//! has none under these calls: anything autoreleased outside a pool stays,
//! which is what the timer's cases look for. So a case asks AppKit for an
//! object only inside a pool of its own, as [`current`] does, or it would hold
//! the very thing the case wants gone.
//!
//! It orders no window in and touches no display.
//!
//! **What the flush gives back is not read here.** A window that was never
//! shown does not leave what a shown one does: after one of 1024 x 1024 pt,
//! flushed and closed, its surfaces stay whether or not the release flushes.
//! The megabytes come from `--demo-drive open-close`. Case 5 pins the flush
//! itself, by what holds as it happens.

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
    // AppKit's OpenGL is deprecated, and it is what the code under test uses.
    #![allow(deprecated)]

    use std::cell::RefCell;
    use std::ffi::CStr;
    use std::ptr::NonNull;
    use std::sync::OnceLock;
    use std::thread;
    use std::time::Duration;

    use arrakis::app::{App, Haptics, NullScreen, Signals};
    use arrakis::drive::{self, Scenario};
    use arrakis::engine::Engine;
    use arrakis::idle;
    use arrakis::model::Action;
    use arrakis::painter::WindowPainter;
    use arrakis::settings::Settings;
    use arrakis::ui::appkit::{FLUSH_FORMAT, release_closed_window, release_closed_window_with};
    use block2::RcBlock;
    use fastframe_shell::{Closed, Headless, Resident, Shell, Waker};
    use fastframe_tray::Tray;
    use objc2::rc::{Retained, Weak, autoreleasepool};
    use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, NSObject, Sel};
    use objc2::{AnyThread, ClassType, MainThreadMarker, msg_send, sel};
    use objc2_app_kit::{
        NSApplication, NSOpenGLContext, NSOpenGLPFAOpenGLProfile, NSOpenGLPixelFormat, NSScreen,
        NSWindow, NSWindowWillCloseNotification,
    };
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSSize, NSTimer};

    use crate::pump::{case, wait_for, watch};

    /// How many ticks a timer's case runs: a leak of one timer a tick shows
    /// at the first, and the rest are for a timer that goes only some of the
    /// time.
    const TICKS: usize = 50;
    /// A tick's wait in the timer's cases: only long enough to run the loop.
    const TICK: Duration = Duration::from_millis(1);
    /// The class of the target that `idle` gives each tick's timer.
    const TICK_TARGET: &CStr = c"ArrakisIdleStop";

    pub fn main() {
        let mtm =
            MainThreadMarker::new().expect("a test without a harness runs on the main thread");
        watch();
        // The first run of `NSApp` finishes launching the app, once. A case
        // is about what the closed app keeps, not about that.
        idle::publish(Headless::Wait);
        idle::idle(Duration::from_millis(10));
        watch_closes();
        watch_flushes();

        // The release: this comes first, while no context has ever been made
        // in this process.
        case(
            "1. with no context ever made, the release leaves none",
            || {
                check!(!any_current(), "a context was current before any was made");
                release_closed_window();
                check!(!any_current(), "a context stayed current");
                check!(closes().len() == 1, "the release closes the window it made");
            },
        );

        case("2. the release survives being called twice", || {
            release_closed_window();
            release_closed_window();
            check!(!any_current(), "a context stayed current");
            check!(
                closes().len() == 2,
                "each call closes one window of its own"
            );
        });

        case("3. its window is never shown, and gone", || {
            let before = open_windows(mtm);
            release_closed_window();
            let closes = closes();
            check!(closes.len() == 1, "{} windows closed", closes.len());
            let close = &closes[0];
            check!(!close.visible, "the window was visible as it closed");
            check!(close.window.load().is_none(), "the window is still alive");
            check!(
                open_windows(mtm) == before,
                "the app holds {} windows, and held {before}",
                open_windows(mtm)
            );
        });

        // The size is a choice, not something AppKit needs, so this pins it
        // and no other case repeats it.
        case("4. its window is 4 x 4 pt, as chosen", || {
            release_closed_window();
            let closes = closes();
            check!(closes.len() == 1, "{} windows closed", closes.len());
            check!(
                closes[0].size == NSSize::new(4.0, 4.0),
                "the window was {:?} as it closed",
                closes[0].size
            );
        });

        case("5. the context a closed window left current goes", || {
            let left = left_current();
            check!(is_current(&left), "the context did not stay current");
            release_closed_window();
            check!(!any_current(), "a context stayed current");
            check!(left.load().is_none(), "the left context is still alive");
            closes();
        });

        case(
            "6. it flushes once, its own context, on the unseen window",
            || {
                flushes();
                release_closed_window();
                let flushes = flushes();
                let closes = closes();
                check!(flushes.len() == 1, "{} flushes", flushes.len());
                check!(closes.len() == 1, "{} windows closed", closes.len());
                let flush = &flushes[0];
                check!(flush.current, "the context it flushed was not current");
                check!(!flush.visible, "the window was visible at the flush");
                check!(
                    flush.size == Some(closes[0].size),
                    "the context's window at the flush: {:?}",
                    flush.size
                );
            },
        );

        case(
            "7. a format AppKit refuses leaves the app as it was",
            || {
                let before = open_windows(mtm);
                let left = left_current();
                // A profile no renderer offers.
                // SAFETY: the list ends with a zero, and its one attribute that
                // takes a value has it.
                unsafe { release_closed_window_with(&[NSOpenGLPFAOpenGLProfile, 0xBAD, 0]) };
                check!(is_current(&left), "the left context did not stay current");
                check!(closes().is_empty(), "a window was made with no context");
                check!(
                    open_windows(mtm) == before,
                    "the app holds {} windows, and held {before}",
                    open_windows(mtm)
                );
                release_closed_window();
                closes();
            },
        );

        case("8. off the main thread the release does nothing", || {
            let left = left_current();
            thread::spawn(release_closed_window)
                .join()
                .expect("the release's thread");
            check!(is_current(&left), "the left context did not stay current");
            check!(closes().is_empty(), "a window was made off the main thread");
            release_closed_window();
            closes();
        });

        // Its wiring, in the app that every window of the real app and the
        // driver hands back to the shell.
        case(
            "9. the app releases once for each window, none at a hidden start",
            || {
                let mut app = drive::app(
                    Scenario::OpenClose,
                    Settings::default(),
                    None,
                    Signals::default(),
                    |_| {},
                );

                // A hidden start: the shell says the window has gone, and none
                // has been.
                let left = left_current();
                app.window_gone();
                check!(is_current(&left), "a hidden start released");
                check!(closes().is_empty(), "a hidden start made a window");

                app.attach(WindowPainter::Glow);
                app.window_gone();
                check!(!any_current(), "a closed window was not released");
                check!(left.load().is_none(), "the left context is still alive");
                check!(closes().len() == 1, "a closed window: one release");

                // And no window since.
                let left = left_current();
                app.window_gone();
                check!(is_current(&left), "the same window released twice");
                check!(closes().is_empty(), "the same window released twice");

                // The next window takes it again.
                app.attach(WindowPainter::Glow);
                app.window_gone();
                check!(left.load().is_none(), "the next window was not released");
                check!(closes().len() == 1, "the next window: one release");
            },
        );

        case("10. a window that closes to a quit is not released", || {
            let left = left_current();
            let app = drive::app(
                Scenario::OpenClose,
                Settings::default(),
                // No menu-bar item, so the window's close ends the app.
                None,
                Signals::default(),
                |_| {},
            );
            let waker = Waker::default();
            let ctx = egui::Context::default();
            Shell::new(app, &waker)
                .run(|lease| {
                    lease.take(&ctx).attach(WindowPainter::Glow);
                    Ok::<(), ()>(())
                })
                .expect("the shell's run");
            check!(is_current(&left), "the quit released");
            check!(closes().is_empty(), "the quit made a window");
            release_closed_window();
            closes();
        });

        // The real app has a menu-bar item, so a close there lives on unless
        // a quit is under way. The first case shows that the two after it
        // would see a release.
        case(
            "11. a tray-backed app's close to the menu bar releases",
            || {
                let left = left_current();
                let closed = tray_window(&[Action::HideWindow]);
                check!(closed == Closed::Hide, "the window closed to {closed:?}");
                check!(left.load().is_none(), "the left context is still alive");
                check!(closes().len() == 1, "a closed window: one release");
            },
        );

        case("12. a quit from a tray-backed app is not released", || {
            let left = left_current();
            let closed = tray_window(&[Action::Quit]);
            check!(closed == Closed::Quit, "the window closed to {closed:?}");
            check!(is_current(&left), "the quit released");
            check!(closes().is_empty(), "the quit made a window");
            release_closed_window();
            closes();
        });

        case(
            "13. a hide after a quit in one pass is still a quit",
            || {
                let left = left_current();
                let closed = tray_window(&[Action::Quit, Action::HideWindow]);
                check!(closed == Closed::Quit, "the window closed to {closed:?}");
                check!(is_current(&left), "the quit released");
                check!(closes().is_empty(), "the quit made a window");
                release_closed_window();
                closes();
            },
        );

        // The timer. Each case reads the timers that were made, as weak
        // references: a timer the tick leaves autoreleased is still alive
        // when the tick returns.
        watch_timers();

        case(
            "14. the timer's probe notes no other target's timer",
            || {
                autoreleasepool(|_| {
                    let target = NSObject::new();
                    // SAFETY: `NSObject` has `description`, which takes nothing it
                    // would misread, and the timer never fires: it is invalidated
                    // at once.
                    let timer = unsafe {
                        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                            60.0,
                            &target,
                            sel!(description),
                            None,
                            false,
                        )
                    };
                    timer.invalidate();
                });
                let timers = made_timers();
                check!(timers.is_empty(), "{} timers noted", timers.len());
            },
        );

        // This shows that the probe sees the leak, for the two cases below.
        // It costs this process the one timer, as a tick cost the app before.
        case(
            "15. a timer made with no pool stays after its last release",
            || {
                let class = AnyClass::get(TICK_TARGET).expect("idle's class, from the first tick");
                // SAFETY: `new` takes nothing and returns a retained instance.
                let target: Retained<AnyObject> = unsafe { msg_send![class, new] };
                // SAFETY: `idle`'s class has the selector, and the timer never
                // fires: it is invalidated at once.
                let timer = unsafe {
                    NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                        60.0,
                        &target,
                        sel!(stopRun:),
                        None,
                        false,
                    )
                };
                timer.invalidate();
                drop(timer);
                let timers = made_timers();
                check!(timers.len() == 1, "{} timers seen", timers.len());
                check!(
                    timers[0].load().is_some(),
                    "the timer is gone, so this probe sees no leak"
                );
            },
        );

        case(
            "16. the timer of a tick that pumps is gone when it returns",
            || {
                for tick in 0..TICKS {
                    wait_for(TICK, TICK);
                    let timers = made_timers();
                    check!(
                        timers.len() == 1,
                        "tick {tick} made {} timers",
                        timers.len()
                    );
                    check!(timers[0].load().is_none(), "tick {tick} left its timer");
                }
            },
        );

        case(
            "17. the timer of a tick a wake ended before its pump is gone too",
            || {
                for tick in 0..TICKS {
                    idle::publish(Headless::WaitFor(Duration::from_secs(30)));
                    idle::interrupt();
                    idle::idle(Duration::from_secs(30));
                    let timers = made_timers();
                    check!(
                        timers.len() == 1,
                        "tick {tick} made {} timers",
                        timers.len()
                    );
                    check!(timers[0].load().is_none(), "tick {tick} left its timer");
                }
            },
        );

        // What a headless frame autoreleases, by the same probe. This shows
        // that it sees the leak, and costs this process the one object.
        case("18. what is autoreleased with no pool stays", || {
            autoreleased_probe("the case");
            let probes = probes();
            check!(probes.len() == 1, "{} objects noted", probes.len());
            check!(
                probes[0].object.load().is_some(),
                "the object is gone, so this probe sees no leak"
            );
        });

        // A frame's first part (the clock, where the tray's labels are set)
        // and its last (the main screen, for a window asked for).
        case(
            "19. what a headless frame autoreleases is gone when it returns",
            || {
                watch_screens();
                let settings = Settings::default();
                let engine = Engine::spawn(Box::new(NullScreen), settings.clone(), noon, |_| {});
                let mut app = App::new(
                    settings,
                    None,
                    engine,
                    None,
                    Signals::default(),
                    probing_clock,
                    Haptics::TRACKPAD,
                );
                app.apply(Action::ShowWindow, None);
                let answer = app.headless_frame(&egui::Context::default());
                check!(answer == Headless::Show, "the frame answered {answer:?}");
                let probes = probes();
                for from in [CLOCK, MAIN_SCREEN] {
                    check!(
                        probes.iter().any(|probe| probe.from == from),
                        "the frame did not reach {from}"
                    );
                }
                for probe in &probes {
                    check!(
                        probe.object.load().is_none(),
                        "what {} autoreleased stayed",
                        probe.from
                    );
                }
            },
        );
    }

    /// One window of a tray-backed app through a real `Shell`, with `actions`
    /// applied in it: says how the window closed. An app that lives on sees a
    /// quit at its first headless frame, which ends the shell's run.
    fn tray_window(actions: &[Action]) -> Closed {
        let signals = Signals::default();
        // Only a headless frame reads this here: this test runs no window's
        // pass, whose frame would.
        signals
            .injected
            .lock()
            .expect("nothing else holds it")
            .push(Action::Quit);
        let app = drive::app(
            Scenario::OpenClose,
            Settings::default(),
            Some(unseen_tray()),
            signals,
            |_| {},
        );
        let waker = Waker::default();
        let ctx = egui::Context::default();
        let mut closed = None;
        Shell::new(app, &waker)
            .run(|lease| {
                let mut app = lease.take(&ctx);
                app.attach(WindowPainter::Glow);
                for action in actions {
                    app.apply(action.clone(), None);
                }
                closed = Some(app.closed());
                Ok::<(), ()>(())
            })
            .expect("the shell's run");
        closed.expect("the shell opens one window")
    }

    /// A real `Tray` that shows nothing. Off the main thread, `create_item`
    /// gives up the settings it keeps for the item and makes none, so the
    /// main thread's `attach` finds none to make or to bring forward
    /// (fastframe-tray's macOS `Host`, at the tag in `Cargo.toml`).
    fn unseen_tray() -> Tray {
        thread::spawn(|| {
            let config = fastframe_tray::Config {
                id: "arrakis-closed-keeps",
                title: "Arrakis".to_owned(),
                icon: arrakis::icon::tray,
                template_icon: None,
                themed_icon: false,
                menu: App::tray_menu(),
            };
            let mut tray = Tray::spawn(config, || {}).expect("macOS always gives a tray");
            tray.create_item();
            tray
        })
        .join()
        .expect("the tray's thread")
    }

    /// Whether any context is current on this thread.
    fn any_current() -> bool {
        current().is_some()
    }

    /// Whether `context` is the one current on this thread.
    fn is_current(context: &Weak<NSOpenGLContext>) -> bool {
        current().is_some_and(|current| context.load().is_some_and(|context| current == context))
    }

    /// The context current on this thread. In a pool: AppKit's getter
    /// autoreleases it.
    fn current() -> Option<Retained<NSOpenGLContext>> {
        autoreleasepool(|_| NSOpenGLContext::currentContext())
    }

    /// The windows the app holds. In a pool, for the same reason.
    fn open_windows(mtm: MainThreadMarker) -> usize {
        autoreleasepool(|_| NSApplication::sharedApplication(mtm).windows().len())
    }

    /// A new context, with nothing but this thread's current slot holding it,
    /// as a closed window leaves its own.
    fn left_current() -> Weak<NSOpenGLContext> {
        autoreleasepool(|_| {
            // SAFETY: the list is valid for the call and ends with a zero.
            let format = unsafe {
                NSOpenGLPixelFormat::initWithAttributes(
                    NSOpenGLPixelFormat::alloc(),
                    NonNull::from(&FLUSH_FORMAT).cast(),
                )
            }
            .expect("a pixel format");
            let context = NSOpenGLContext::initWithFormat_shareContext(
                NSOpenGLContext::alloc(),
                &format,
                None,
            )
            .expect("a context");
            context.makeCurrentContext();
            Weak::from_retained(&context)
        })
    }

    /// One window that closed, as it was when it did.
    struct Close {
        visible: bool,
        size: NSSize,
        window: Weak<NSWindow>,
    }

    /// One `flushBuffer`, as things were when it was called.
    struct Flush {
        /// Whether the context it was called on was the current one.
        current: bool,
        /// Whether the context's window was visible.
        visible: bool,
        /// The size of the context's window, if it had one.
        size: Option<NSSize>,
    }

    thread_local! {
        /// The windows that closed since [`closes`] last read them.
        static CLOSES: RefCell<Vec<Close>> = const { RefCell::new(Vec::new()) };
        /// The flushes since [`flushes`] last read them.
        static FLUSHES: RefCell<Vec<Flush>> = const { RefCell::new(Vec::new()) };
        /// The timers made since [`made_timers`] last read them.
        static TIMERS: RefCell<Vec<Weak<NSTimer>>> = const { RefCell::new(Vec::new()) };
        /// The objects made since [`probes`] last read them.
        static PROBES: RefCell<Vec<Probe>> = const { RefCell::new(Vec::new()) };
    }

    /// The windows that closed since the last call.
    fn closes() -> Vec<Close> {
        CLOSES.take()
    }

    /// The flushes since the last call.
    fn flushes() -> Vec<Flush> {
        FLUSHES.take()
    }

    /// The timers made since the last call.
    fn made_timers() -> Vec<Weak<NSTimer>> {
        TIMERS.take()
    }

    /// The objects made since the last call.
    fn probes() -> Vec<Probe> {
        PROBES.take()
    }

    /// Notes each window as AppKit is about to close it, when it is still
    /// visible if it ever was: `close` orders it out after this notice.
    fn watch_closes() {
        let block = RcBlock::new(|notification: NonNull<NSNotification>| {
            // SAFETY: the center hands the block the notice it posts, alive
            // for the call.
            let object = unsafe { notification.as_ref() }.object();
            let window = object.and_then(|object| object.downcast::<NSWindow>().ok());
            if let Some(window) = window {
                CLOSES.with_borrow_mut(|closes| {
                    closes.push(Close {
                        visible: window.isVisible(),
                        size: window.frame().size,
                        window: Weak::from_retained(&window),
                    });
                });
            }
        });
        // SAFETY: the name is AppKit's own constant, and no object is
        // filtered on. With no queue the block runs on the posting thread,
        // the main thread for a window.
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSWindowWillCloseNotification),
                None,
                None,
                &block,
            )
        };
        // Watched for the life of the test.
        std::mem::forget(observer);
    }

    /// The signature of `-[NSOpenGLContext flushBuffer]`.
    type FlushBuffer = unsafe extern "C-unwind" fn(*const NSOpenGLContext, Sel);

    /// The method as AppKit has it, before [`watch_flushes`].
    static FLUSH_BUFFER: OnceLock<FlushBuffer> = OnceLock::new();

    /// Notes each `flushBuffer` from here on, as it is called.
    fn watch_flushes() {
        let method = NSOpenGLContext::class()
            .instance_method(sel!(flushBuffer))
            .expect("NSOpenGLContext's flushBuffer");
        // SAFETY: `noting_flush` has the method's signature, and `Imp` is only
        // the form the runtime takes any implementation in; `FlushBuffer` is
        // the signature of what it gives back.
        unsafe {
            let new = std::mem::transmute::<FlushBuffer, Imp>(noting_flush);
            let original = method.set_implementation(new);
            let _ = FLUSH_BUFFER.set(std::mem::transmute::<Imp, FlushBuffer>(original));
        }
    }

    /// The method, noting how things stand first.
    unsafe extern "C-unwind" fn noting_flush(context: *const NSOpenGLContext, selector: Sel) {
        let mtm = MainThreadMarker::new().expect("a flush on the main thread");
        // SAFETY: the runtime passes the live receiver of the call.
        let receiver = unsafe { &*context };
        autoreleasepool(|_| {
            let window = receiver.view(mtm).and_then(|view| view.window());
            let flush = Flush {
                current: current().is_some_and(|current| *current == *receiver),
                visible: window.as_ref().is_some_and(|window| window.isVisible()),
                size: window.as_ref().map(|window| window.frame().size),
            };
            FLUSHES.with_borrow_mut(|flushes| flushes.push(flush));
        });
        let original = FLUSH_BUFFER.get().expect("the original method");
        // SAFETY: the arguments of a call to the method, passed on to it.
        unsafe { original(context, selector) };
    }

    /// The signature of `+[NSTimer
    /// scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:]`.
    type Scheduled = unsafe extern "C-unwind" fn(
        *const AnyClass,
        Sel,
        f64,
        *mut AnyObject,
        Sel,
        *mut AnyObject,
        Bool,
    ) -> *mut NSTimer;

    /// The class method as Foundation has it, before [`watch_timers`].
    static SCHEDULED: OnceLock<Scheduled> = OnceLock::new();

    /// Notes each tick's timer from here on: `idle` makes its own and shows it
    /// to no one, and this leaves it free of any seam for a test.
    fn watch_timers() {
        let method = NSTimer::class()
            .class_method(sel!(scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:))
            .expect("NSTimer's class method");
        // SAFETY: `noting_timer` has the method's signature, and `Imp` is only
        // the form the runtime takes any implementation in; `Scheduled` is the
        // signature of what it gives back.
        unsafe {
            let new = std::mem::transmute::<Scheduled, Imp>(noting_timer);
            let original = method.set_implementation(new);
            let _ = SCHEDULED.set(std::mem::transmute::<Imp, Scheduled>(original));
        }
    }

    /// The class method, noting the timer it returns if it is a tick's: this
    /// is process-wide, and another framework's timer says nothing of a tick.
    /// A weak reference holds nothing back: the timer stays in the autorelease
    /// pool the method put it in, and goes when that pool does.
    unsafe extern "C-unwind" fn noting_timer(
        class: *const AnyClass,
        selector: Sel,
        interval: f64,
        target: *mut AnyObject,
        action: Sel,
        info: *mut AnyObject,
        repeats: Bool,
    ) -> *mut NSTimer {
        let original = SCHEDULED.get().expect("the original method");
        // SAFETY: the arguments of a call to the method, passed on to it.
        let timer = unsafe { original(class, selector, interval, target, action, info, repeats) };
        // SAFETY: the caller's target, a live object or nil.
        let target_class = unsafe { target.as_ref() }.map(|target| target.class());
        if target_class.is_some_and(|class| class.name() == TICK_TARGET) {
            // SAFETY: the method returned a live timer or nil.
            if let Some(retained) = unsafe { Retained::retain(timer) } {
                TIMERS.with_borrow_mut(|timers| timers.push(Weak::from_retained(&retained)));
            }
        }
        timer
    }

    /// Where [`probing_clock`] made its object.
    const CLOCK: &str = "the clock";
    /// Where [`probing_screens`] made its object.
    const MAIN_SCREEN: &str = "the main screen's lookup";

    /// One object made and autoreleased under a call, as AppKit's are.
    struct Probe {
        from: &'static str,
        object: Weak<NSObject>,
    }

    /// Makes an object, notes it and leaves it in whatever pool is there, or
    /// in none: a weak reference holds nothing back.
    fn autoreleased_probe(from: &'static str) {
        let object = NSObject::new();
        PROBES.with_borrow_mut(|probes| {
            probes.push(Probe {
                from,
                object: Weak::from_retained(&object),
            });
        });
        let _ = Retained::autorelease_ptr(object);
    }

    /// The engine's clock in the frame's case.
    fn noon() -> f64 {
        12.0 * 60.0
    }

    /// The app's clock in the frame's case: `sync_tray` reads it, just before
    /// it sets the tray's labels.
    fn probing_clock() -> f64 {
        autoreleased_probe(CLOCK);
        noon()
    }

    /// The signature of `+[NSScreen screens]`.
    type Screens = unsafe extern "C-unwind" fn(*const AnyClass, Sel) -> *mut AnyObject;

    /// The class method as AppKit has it, before [`watch_screens`].
    static SCREENS: OnceLock<Screens> = OnceLock::new();

    /// Probes each `+[NSScreen screens]` from here on, which a frame calls
    /// when a window is asked for.
    fn watch_screens() {
        let method = NSScreen::class()
            .class_method(sel!(screens))
            .expect("NSScreen's screens");
        // SAFETY: `probing_screens` has the method's signature, and `Imp` is
        // only the form the runtime takes any implementation in; `Screens` is
        // the signature of what it gives back.
        unsafe {
            let new = std::mem::transmute::<Screens, Imp>(probing_screens);
            let original = method.set_implementation(new);
            let _ = SCREENS.set(std::mem::transmute::<Imp, Screens>(original));
        }
    }

    /// The class method, with a probe.
    unsafe extern "C-unwind" fn probing_screens(
        class: *const AnyClass,
        selector: Sel,
    ) -> *mut AnyObject {
        autoreleased_probe(MAIN_SCREEN);
        let original = SCREENS.get().expect("the original method");
        // SAFETY: the arguments of a call to the method, passed on to it.
        unsafe { original(class, selector) }
    }
}
