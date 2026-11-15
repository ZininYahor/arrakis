//! What the window plays on the trackpad: a detent's tick, a beat of the
//! thumper, and a button's press. Other platforms play nothing.
//!
//! **Two ways to play.** Each [`Feel`] names both:
//! - the private actuator of `MultitouchSupport.framework`, the owner's
//!   choice in a haptic lab on 2026-10-23: stronger and crisper than any
//!   public pattern. It is undocumented, so a macOS update may close it;
//! - the public pattern that `NSHapticFeedbackManager`'s default performer
//!   plays at once (`NSHapticFeedbackPerformanceTime::Now`). `DrawCompleted`
//!   would wait for AppKit to draw the window, which egui's GL view may
//!   present without.
//!
//! **Who hears it.** The public patterns play only under a finger on a
//! Force Touch trackpad. The private actuator moves the trackpad with no
//! finger on it, and the owner could hear it so. It plays only for a press
//! that came from a Force Touch trackpad, which [`watch_pointer`] learns
//! from each left mouse-down: a device that also sends pressure events
//! (`associatedEventsMask`). A press from a mouse, a tablet, or any press
//! before the watch began gets the public pattern, which a mouse user never
//! feels or hears. The keys and VoiceOver play nothing: the window asks for
//! haptics only from the pointer.
//!
//! **One trackpad.** The press does not say which trackpad it came from, so
//! the private actuator plays only while exactly one multitouch device can
//! actuate, which is then the one under the finger; it opens that device's
//! actuator. With two Force Touch trackpads, such as a Magic Trackpad beside
//! a MacBook's, the public pattern plays, under the finger that pressed,
//! rather than moving a pad nobody touches. The devices are counted as the
//! actuator opens, and again at the first haptic of each press from a
//! trackpad while it is open, so a trackpad that comes later turns the
//! private path off from its first press.
//!
//! **The fallback.** A missing framework or function, no single device that
//! can actuate, a device or actuator that will not open, or an actuation
//! that fails, plays the public pattern instead, so a macOS update that
//! closes the private path leaves the ticks as they were before it. A
//! failed actuation closes the actuator and reopens it once at most each
//! [`REOPEN_AFTER`]; see [`private::Actuator`].
//! [`PRIVATE_ACTUATOR`] turns the private path off in one place.
//!
//! **The main thread.** `App::apply` calls these on the main thread, after
//! the frame's UI pass. AppKit expects its feedback there, the monitor
//! reports there, and the actuator's handle lives in a `thread_local` there,
//! so it never crosses threads. Off the main thread nothing plays: a play
//! returns before it reaches either. The probe's and the driver's tests call
//! these on a thread of their own, where the probe's counts that no play got
//! past (`reached`, in the demo build's tests); `test_app` gives the app
//! sinks that play nothing.

use std::time::Duration;

/// What one haptic plays: an actuation of the trackpad's private actuator,
/// and the public pattern that plays instead when the private path cannot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feel {
    /// The actuation, as `MTActuatorActuate` numbers them.
    pub actuation: i32,
    /// Its strength, as `MTActuatorActuate` takes it.
    pub intensity: f32,
    /// The public pattern that stands in for it.
    pub pattern: Pattern,
}

/// The public patterns, as `NSHapticFeedbackPattern` names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    Alignment,
    LevelChange,
}

/// A detent's tick: actuation 4 at 2.0, or `Alignment`.
pub const TICK: Feel = Feel {
    actuation: 4,
    intensity: 2.0,
    pattern: Pattern::Alignment,
};

/// A thump of the thumper: actuation 6 at 2.0, or `LevelChange`.
pub const THUMP: Feel = Feel {
    actuation: 6,
    intensity: 2.0,
    pattern: Pattern::LevelChange,
};

/// A button's press: the detent's tick, until the owner chooses another.
/// This is the one place to change it.
pub const PRESS: Feel = TICK;

/// Whether the private actuator plays at all. False plays the public
/// patterns everywhere, as before it.
pub const PRIVATE_ACTUATOR: bool = true;

/// How long after one try to open the private actuator the next may come,
/// once a try has failed or an open actuator has stopped answering.
///
/// Sleep and a trackpad that goes away and back close the actuator. The
/// first tick after that finds it closed and reopens it at once, since the
/// last try was long before. A reopen that fails means the device is still
/// settling, which takes seconds (a Bluetooth trackpad reconnecting), and
/// trying on every tick of a scrub, up to 20 a second, would spend an
/// IOKit round trip on each. Five seconds bounds that to one try per 5 s,
/// while the public patterns fill the gap.
pub const REOPEN_AFTER: Duration = Duration::from_secs(5);

/// One tick, as the Temperature slider's detent catches the value or the
/// value passes one.
pub fn align() {
    play(TICK);
}

/// One thump, a beat of the Temperature slider's hold on its Desert stop
/// (see `ui::input`).
pub fn thump() {
    play(THUMP);
}

/// One press, as a button in the window takes a click from the pointer.
pub fn press() {
    play(PRESS);
}

/// Starts telling a press on a Force Touch trackpad from one on a mouse
/// (see the module's "Who hears it"). Call it on the main thread once the
/// app runs and before the first press: the entry point does as it builds a
/// window. Later calls do nothing.
pub fn watch_pointer() {
    #[cfg(target_os = "macos")]
    macos::watch_pointer();
}

fn play(feel: Feel) {
    #[cfg(target_os = "macos")]
    macos::play(feel);
    #[cfg(not(target_os = "macos"))]
    let _ = feel;
}

/// How many plays on this thread got past the main thread's guard, on their
/// way to the actuator or AppKit: a test that calls the sinks off the main
/// thread reads none. The probe's test reads it, so it is in the demo
/// build's tests only.
#[cfg(all(test, feature = "demo", target_os = "macos"))]
pub(crate) fn reached() -> u32 {
    macos::REACHED.get()
}

/// Off macOS, a play reaches nothing.
#[cfg(all(test, feature = "demo", not(target_os = "macos")))]
pub(crate) fn reached() -> u32 {
    0
}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::{Cell, RefCell};
    use std::ptr::NonNull;
    use std::time::Instant;

    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSEvent, NSEventMask, NSHapticFeedbackManager, NSHapticFeedbackPattern,
        NSHapticFeedbackPerformanceTime, NSHapticFeedbackPerformer,
    };

    use super::private::{self, Actuator};
    use super::{Feel, PRIVATE_ACTUATOR, Pattern};

    thread_local! {
        /// The private actuator, on the main thread alone: the raw handles
        /// it holds make it neither `Send` nor `Sync`.
        static ACTUATOR: RefCell<Actuator> =
            const { RefCell::new(Actuator::new(private::load, private::tell)) };
        /// Whether the last left mouse-down came from a Force Touch trackpad.
        static ON_TRACKPAD: Cell<bool> = const { Cell::new(false) };
        /// Whether a left mouse-down has come since the last private play,
        /// so that the actuator counts the trackpads again.
        static NEW_PRESS: Cell<bool> = const { Cell::new(false) };
        /// The monitor that sets [`ON_TRACKPAD`], kept for the app's life.
        static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    }

    #[cfg(all(test, feature = "demo"))]
    thread_local! {
        /// The plays on this thread that got past the main thread's guard,
        /// on their way to the actuator or AppKit: see [`super::reached`].
        pub(super) static REACHED: Cell<u32> = const { Cell::new(0) };
    }

    /// Plays `feel`: the private actuation for a press from a Force Touch
    /// trackpad when the actuator answers, the public pattern otherwise.
    pub fn play(feel: Feel) {
        // AppKit's feedback and the actuator both belong to the main thread.
        if MainThreadMarker::new().is_none() {
            return;
        }
        #[cfg(all(test, feature = "demo"))]
        REACHED.set(REACHED.get() + 1);
        let private = PRIVATE_ACTUATOR
            && ON_TRACKPAD.get()
            && ACTUATOR.with_borrow_mut(|actuator| {
                if NEW_PRESS.replace(false) {
                    actuator.new_press();
                }
                actuator.play(feel.actuation, feel.intensity, Instant::now())
            });
        if !private {
            NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(
                pattern(feel.pattern),
                NSHapticFeedbackPerformanceTime::Now,
            );
        }
    }

    fn pattern(pattern: Pattern) -> NSHapticFeedbackPattern {
        match pattern {
            Pattern::Alignment => NSHapticFeedbackPattern::Alignment,
            Pattern::LevelChange => NSHapticFeedbackPattern::LevelChange,
        }
    }

    /// Whether a mouse-down whose `associatedEventsMask` is `mask` came from
    /// a Force Touch trackpad: a device that also sends pressure events. A
    /// mouse sends none, and a tablet carries its pressure in the mouse
    /// events themselves.
    pub(super) fn from_trackpad(mask: NSEventMask) -> bool {
        mask.contains(NSEventMask::Pressure)
    }

    /// Installs a local monitor of left mouse-downs that records where each
    /// came from, once, on the main thread.
    pub fn watch_pointer() {
        if MainThreadMarker::new().is_none() || MONITOR.with_borrow(Option::is_some) {
            return;
        }
        let handler = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit hands the monitor a live event for the length
            // of the call. The monitor takes only left mouse-downs, and
            // `associatedEventsMask` is defined for mouse events.
            let mask = unsafe { event.as_ref() }.associatedEventsMask();
            ON_TRACKPAD.set(from_trackpad(mask));
            NEW_PRESS.set(true);
            event.as_ptr()
        });
        // SAFETY: the handler returns the event it was given, a valid
        // pointer, so AppKit dispatches it as it would have with no monitor.
        // AppKit copies the block and runs it on the main thread.
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::LeftMouseDown,
                &handler,
            )
        };
        MONITOR.set(monitor);
    }
}

/// The trackpad's private actuator, from `MultitouchSupport.framework`,
/// loaded with `dlopen` and `dlsym` and never linked.
///
/// [`Actuator`] keeps it open and decides when the private path plays; the
/// caller plays the public pattern whenever it says no. The functions come
/// through [`Symbols`], so a test can hand it ones that fail at any step
/// with no framework at all.
#[cfg(target_os = "macos")]
pub(crate) mod private {
    use std::ffi::{CStr, c_void};
    use std::ptr;
    use std::time::Instant;

    use super::REOPEN_AFTER;

    /// An object the framework hands out, an `MTDeviceRef` or an
    /// `MTActuatorRef`: CoreFoundation objects, freed with `CFRelease`.
    pub type Ref = *mut c_void;

    /// The functions that play an actuation, with the signatures that
    /// worked in the owner's haptic lab on macOS 26.6.2 (Apple Silicon, the
    /// built-in trackpad), which come from HapticKey
    /// (github.com/niw/HapticKey); the two that find the devices, whose
    /// signatures are read from their code in macOS 26.6's shared cache; and
    /// CoreFoundation's `CFRelease` and the two `CFArray` reads.
    #[derive(Clone, Copy)]
    pub struct Symbols {
        /// `MTDeviceCreateList`: every multitouch device, in a `CFArray`
        /// owned by the caller, or null.
        pub create_list: unsafe extern "C" fn() -> Ref,
        /// `CFArrayGetCount`.
        pub count: unsafe extern "C" fn(*const c_void) -> isize,
        /// `CFArrayGetValueAtIndex`: an item, which the array owns.
        pub item: unsafe extern "C" fn(*const c_void, isize) -> *const c_void,
        /// `MTDeviceSupportsActuation`: whether the device has an actuator
        /// to play, as a Force Touch trackpad does; false for null.
        pub actuates: unsafe extern "C" fn(Ref) -> bool,
        /// `MTDeviceGetDeviceID`: the device's id, into the second argument;
        /// zero on success.
        pub device_id: unsafe extern "C" fn(Ref, *mut u64) -> i32,
        /// `MTActuatorCreateFromDeviceID`: the device's actuator, owned by
        /// the caller, or null.
        pub create_actuator: unsafe extern "C" fn(u64) -> Ref,
        /// `MTActuatorOpen`: zero on success.
        pub open: unsafe extern "C" fn(Ref) -> i32,
        /// `MTActuatorActuate(actuator, actuation, 0, 0.0, intensity)`:
        /// zero on success.
        pub actuate: unsafe extern "C" fn(Ref, i32, u32, f32, f32) -> i32,
        /// `MTActuatorClose`.
        pub close: unsafe extern "C" fn(Ref) -> i32,
        /// `CFRelease`, for the list and the actuator.
        pub release: unsafe extern "C" fn(*const c_void),
    }

    /// Where the framework's functions stand.
    #[derive(Clone, Copy)]
    enum Loaded {
        /// Not looked for yet: the first private play looks.
        Untried,
        /// The framework or a function is missing. A running app never gains
        /// it, so nobody looks again.
        Missing,
        Found(Symbols),
    }

    /// The private actuator, opened once and kept for the app's life.
    ///
    /// - The first [`Actuator::play`] loads the functions; with one
    ///   missing, every play says no.
    /// - It opens the actuator of the one device that can actuate, and
    ///   keeps it open. With none, or more than one, it opens nothing.
    /// - The first play after [`Actuator::new_press`] counts the devices
    ///   again, and closes the actuator unless its device is still the one.
    /// - An actuation that fails closes the actuator and reopens it, then
    ///   tries once more; a retry that fails closes it again.
    /// - A try to open comes at most once each [`REOPEN_AFTER`]: a failed
    ///   one says no until then.
    ///
    /// It tells, through `tell`, when the private path starts or stops
    /// working, never per tick: it works from an actuation that succeeds,
    /// and stops at an open or an actuation that fails.
    pub struct Actuator {
        load: fn() -> Option<Symbols>,
        tell: fn(bool),
        loaded: Loaded,
        /// The open actuator, or null.
        actuator: Ref,
        /// The id of the device whose actuator is open.
        device: u64,
        /// When an open was last tried.
        tried: Option<Instant>,
        /// Whether a press has come since the devices were last counted.
        pressed: bool,
        /// Whether the private path worked at its last open or actuation,
        /// so that it tells only of a change.
        working: Option<bool>,
    }

    impl Actuator {
        /// An actuator that finds its functions with `load` at its first
        /// play, and tells whether it works with `tell`.
        pub const fn new(load: fn() -> Option<Symbols>, tell: fn(bool)) -> Self {
            Self {
                load,
                tell,
                loaded: Loaded::Untried,
                actuator: ptr::null_mut(),
                device: 0,
                tried: None,
                pressed: false,
                working: None,
            }
        }

        /// A press from a trackpad has come: the next play counts the
        /// devices again before it plays on the open actuator.
        pub fn new_press(&mut self) {
            self.pressed = true;
        }

        /// Plays `actuation` at `intensity`, at `now`. False when the private
        /// path could not, and the caller plays the public pattern instead.
        pub fn play(&mut self, actuation: i32, intensity: f32, now: Instant) -> bool {
            let Some(symbols) = self.symbols() else {
                return false;
            };
            // A second trackpad may have come since the open, or this one
            // gone: closed, the actuator opens again only on the one there
            // is.
            if std::mem::take(&mut self.pressed)
                && self.is_open()
                // SAFETY: `symbols` holds functions with these signatures
                // (see `load`), on the main thread, where this actuator lives.
                && unsafe { sole_device(symbols) } != Some(self.device)
            {
                self.close(symbols);
            }
            if self.actuator.is_null() && !self.open(symbols, now) {
                return false;
            }
            if self.actuate(symbols, actuation, intensity) {
                return true;
            }
            // Sleep, or the trackpad going away and back, may have closed it.
            self.close(symbols);
            if self.open(symbols, now) && self.actuate(symbols, actuation, intensity) {
                return true;
            }
            self.close(symbols);
            false
        }

        /// Whether an actuator is open now.
        pub fn is_open(&self) -> bool {
            !self.actuator.is_null()
        }

        fn symbols(&mut self) -> Option<Symbols> {
            if let Loaded::Untried = self.loaded {
                self.loaded = match (self.load)() {
                    Some(symbols) => Loaded::Found(symbols),
                    None => {
                        log::warn!(
                            "the trackpad's private actuator is missing; playing the public patterns"
                        );
                        Loaded::Missing
                    }
                };
            }
            match self.loaded {
                Loaded::Found(symbols) => Some(symbols),
                Loaded::Untried | Loaded::Missing => None,
            }
        }

        /// Opens the actuator of the one device that can actuate, unless the
        /// last try came less than [`REOPEN_AFTER`] before `now`.
        fn open(&mut self, symbols: Symbols, now: Instant) -> bool {
            if self
                .tried
                .is_some_and(|tried| now.saturating_duration_since(tried) < REOPEN_AFTER)
            {
                return false;
            }
            self.tried = Some(now);
            // SAFETY: `symbols` holds functions with these signatures (see
            // `load`), on the main thread, where this actuator lives.
            match unsafe { open_sole(symbols) } {
                Some((actuator, device)) => {
                    self.actuator = actuator;
                    self.device = device;
                    true
                }
                None => {
                    self.working(false);
                    false
                }
            }
        }

        fn actuate(&mut self, symbols: Symbols, actuation: i32, intensity: f32) -> bool {
            // SAFETY: the actuator is open: `open_sole` opened it, and
            // nothing has closed it since (`close` nulls it). The arguments
            // are the lab's: `(actuator, id, 0, 0.0, intensity)`.
            let worked =
                unsafe { (symbols.actuate)(self.actuator, actuation, 0, 0.0, intensity) } == 0;
            self.working(worked);
            worked
        }

        fn close(&mut self, symbols: Symbols) {
            if self.actuator.is_null() {
                return;
            }
            // SAFETY: the actuator is open, and this owns it (it came from
            // a Create function); nothing uses it after this, since it is
            // nulled.
            unsafe {
                (symbols.close)(self.actuator);
                (symbols.release)(self.actuator);
            }
            self.actuator = ptr::null_mut();
        }

        /// Tells of a change in whether the private path works.
        fn working(&mut self, now: bool) {
            if self.working != Some(now) {
                self.working = Some(now);
                (self.tell)(now);
            }
        }
    }

    /// Logs whether the private path works, for the app's [`Actuator`].
    pub fn tell(working: bool) {
        if working {
            log::info!("the trackpad's private actuator plays");
        } else {
            log::warn!(
                "the trackpad's private actuator cannot play (not exactly one Force Touch \
                 trackpad, or it does not answer); playing the public patterns"
            );
        }
    }

    /// The actuator of the one device that can actuate, open, and that
    /// device's id; `None` with no such device, or more than one. Frees what
    /// it made on the way to a failure.
    ///
    /// # Safety
    ///
    /// `symbols` holds functions with the signatures its fields give.
    unsafe fn open_sole(symbols: Symbols) -> Option<(Ref, u64)> {
        // SAFETY: the caller vouches for the signatures. The actuator is
        // checked for null before use, and released if it will not open.
        unsafe {
            let device = sole_device(symbols)?;
            let actuator = (symbols.create_actuator)(device);
            if actuator.is_null() {
                return None;
            }
            if (symbols.open)(actuator) != 0 {
                (symbols.release)(actuator);
                return None;
            }
            Some((actuator, device))
        }
    }

    /// The id of the one multitouch device that can actuate; `None` with
    /// none, more than one, or no id.
    ///
    /// # Safety
    ///
    /// `symbols` holds functions with the signatures its fields give.
    unsafe fn sole_device(symbols: Symbols) -> Option<u64> {
        // SAFETY: the caller vouches for the signatures. The list is checked
        // for null, read only within its count, and released once, after
        // the last read of a device it owns.
        unsafe {
            let list = (symbols.create_list)();
            if list.is_null() {
                return None;
            }
            let mut sole = None;
            let mut more = false;
            for index in 0..(symbols.count)(list) {
                let device = (symbols.item)(list, index).cast_mut();
                if (symbols.actuates)(device) {
                    more = sole.is_some();
                    if more {
                        break;
                    }
                    sole = Some(device);
                }
            }
            let id = sole.filter(|_| !more).and_then(|device| {
                let mut id = 0;
                ((symbols.device_id)(device, &mut id) == 0 && id != 0).then_some(id)
            });
            (symbols.release)(list);
            id
        }
    }

    const FRAMEWORK: &CStr =
        c"/System/Library/PrivateFrameworks/MultitouchSupport.framework/MultitouchSupport";

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(object: *const c_void);
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    }

    /// The framework's functions, or `None` if it or one of them is missing.
    /// The app's [`Actuator`] calls it on the main thread, at the first
    /// private play.
    pub fn load() -> Option<Symbols> {
        // SAFETY: a constant, NUL-terminated path. The handle is never
        // closed, so the image stays loaded, and every function found in it
        // stays valid, for the process's life.
        let handle = unsafe { libc::dlopen(FRAMEWORK.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return None;
        }
        let find = |name: &CStr| {
            // SAFETY: a live handle from `dlopen`, and a NUL-terminated name.
            let found = unsafe { libc::dlsym(handle, name.as_ptr()) };
            (!found.is_null()).then_some(found)
        };
        // SAFETY: each name is a C function of the framework with the
        // signature its field gives: the one the owner's haptic lab called
        // on this macOS, or, for the list and `MTDeviceSupportsActuation`,
        // the one their code in macOS 26.6's shared cache shows (no
        // argument and an object back; one object and a `bool` back). A
        // code address and a data pointer have one size and representation
        // on macOS. A function that changed its signature in an update is
        // the private API's risk (see the module): the signatures are not
        // checked.
        unsafe {
            Some(Symbols {
                create_list: std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> Ref>(
                    find(c"MTDeviceCreateList")?,
                ),
                count: CFArrayGetCount,
                item: CFArrayGetValueAtIndex,
                actuates: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(Ref) -> bool>(
                    find(c"MTDeviceSupportsActuation")?,
                ),
                device_id: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(Ref, *mut u64) -> i32,
                >(find(c"MTDeviceGetDeviceID")?),
                create_actuator: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(u64) -> Ref>(
                    find(c"MTActuatorCreateFromDeviceID")?,
                ),
                open: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(Ref) -> i32>(find(
                    c"MTActuatorOpen",
                )?),
                actuate: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(Ref, i32, u32, f32, f32) -> i32,
                >(find(c"MTActuatorActuate")?),
                close: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(Ref) -> i32>(find(
                    c"MTActuatorClose",
                )?),
                release: CFRelease,
            })
        }
    }
}

/// The private actuator's state machine over fake framework functions, and
/// who hears it. Nothing here reaches AppKit or the private framework: the
/// fakes below stand in for `MultitouchSupport`, and `private::load`, `play`,
/// `align`, `thump`, `press` and `watch_pointer` never run.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::time::{Duration, Instant};

    use objc2_app_kit::NSEventMask;

    use super::macos::from_trackpad;
    use super::private::{Actuator, Ref, Symbols};
    use super::*;

    /// The list, the devices and the actuator the fakes hand out: distinct
    /// addresses, never dereferenced. The values differ so that nothing may
    /// merge the statics.
    static LIST: u8 = 1;
    static DEVICES: [u8; 3] = [2, 3, 4];
    static ACTUATOR: u8 = 5;

    /// The id the first fake device reports; the next ones report the ids
    /// after it.
    const DEVICE_ID: u64 = 7;

    fn list() -> Ref {
        (&raw const LIST).cast_mut().cast()
    }

    /// The fake device at `index` in the list.
    fn device(index: usize) -> Ref {
        (&raw const DEVICES[index]).cast_mut().cast()
    }

    /// Where `device` is in the list.
    fn index_of(device: Ref) -> usize {
        address(device).wrapping_sub(address(self::device(0)))
    }

    fn actuator_ref() -> Ref {
        (&raw const ACTUATOR).cast_mut().cast()
    }

    fn address(object: Ref) -> usize {
        object as usize
    }

    /// One call of a fake function, with what it was given. The reads of
    /// the list and of the devices' kind are left out.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Call {
        CreateList,
        DeviceId(usize),
        CreateActuator(u64),
        Open(usize),
        Actuate {
            actuator: usize,
            actuation: i32,
            flags: u32,
            position: f32,
            intensity: f32,
        },
        Close(usize),
        Release(usize),
    }

    /// The step of opening the fakes fail at.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Failure {
        /// `create_list` returns null.
        CreateList,
        /// `device_id` returns non-zero, and writes an id.
        DeviceIdError,
        /// `device_id` returns zero and writes an id of zero.
        DeviceIdZero,
        /// `create_actuator` returns null.
        CreateActuator,
        /// `open` returns non-zero.
        Open,
    }

    /// What the fakes did and how they fail, on this test's thread.
    #[derive(Default)]
    struct Record {
        calls: Vec<Call>,
        /// The devices in the list, each true if it can actuate.
        devices: Vec<bool>,
        /// The step that fails, while one does.
        failure: Option<Failure>,
        /// How many of the next actuations fail.
        failing_actuations: u32,
        /// How many times a loader ran.
        loads: usize,
        /// What the actuator told, in order.
        told: Vec<bool>,
    }

    thread_local! {
        static RECORD: RefCell<Record> = RefCell::new(Record::default());
    }

    fn record(call: Call) {
        RECORD.with_borrow_mut(|record| record.calls.push(call));
    }

    /// The devices in the list from now on, each true if it can actuate.
    fn devices(devices: &[bool]) {
        RECORD.with_borrow_mut(|record| record.devices = devices.to_vec());
    }

    /// Everything the actuator has told so far, which is forgotten.
    fn told() -> Vec<bool> {
        RECORD.with_borrow_mut(|record| std::mem::take(&mut record.told))
    }

    fn tell(working: bool) {
        RECORD.with_borrow_mut(|record| record.told.push(working));
    }

    fn fails_at(step: Failure) -> bool {
        RECORD.with_borrow(|record| record.failure == Some(step))
    }

    /// The fakes fail at `step` from now on, or at none.
    fn fail(step: Option<Failure>) {
        RECORD.with_borrow_mut(|record| record.failure = step);
    }

    /// The next `count` actuations fail.
    fn fail_actuations(count: u32) {
        RECORD.with_borrow_mut(|record| record.failing_actuations = count);
    }

    /// Every call so far, which are forgotten.
    fn calls() -> Vec<Call> {
        RECORD.with_borrow_mut(|record| std::mem::take(&mut record.calls))
    }

    fn loads() -> usize {
        RECORD.with_borrow(|record| record.loads)
    }

    unsafe extern "C" fn create_list() -> Ref {
        record(Call::CreateList);
        if fails_at(Failure::CreateList) {
            std::ptr::null_mut()
        } else {
            list()
        }
    }

    // A panic would abort the tests from these C functions, so they never
    // assert: a misuse shows in the calls that the fakes record.

    /// The list's length, for the fake list alone.
    unsafe extern "C" fn count(array: *const c_void) -> isize {
        let devices = RECORD.with_borrow(|record| record.devices.len());
        if array == list().cast_const() {
            devices as isize
        } else {
            0
        }
    }

    unsafe extern "C" fn item(_: *const c_void, index: isize) -> *const c_void {
        device(index as usize).cast_const()
    }

    unsafe extern "C" fn actuates(device: Ref) -> bool {
        RECORD
            .with_borrow(|record| record.devices.get(index_of(device)).copied())
            .unwrap_or(false)
    }

    unsafe extern "C" fn device_id(device: Ref, id: *mut u64) -> i32 {
        record(Call::DeviceId(address(device)));
        let own = DEVICE_ID + index_of(device) as u64;
        let (id_to_write, status) = match RECORD.with_borrow(|record| record.failure) {
            Some(Failure::DeviceIdError) => (own, 1),
            Some(Failure::DeviceIdZero) => (0, 0),
            _ => (own, 0),
        };
        // SAFETY: the actuator under test passes the address of a live `u64`.
        unsafe { *id = id_to_write };
        status
    }

    unsafe extern "C" fn create_actuator(id: u64) -> Ref {
        record(Call::CreateActuator(id));
        if fails_at(Failure::CreateActuator) {
            std::ptr::null_mut()
        } else {
            actuator_ref()
        }
    }

    unsafe extern "C" fn open(actuator: Ref) -> i32 {
        record(Call::Open(address(actuator)));
        i32::from(fails_at(Failure::Open))
    }

    unsafe extern "C" fn actuate(
        actuator: Ref,
        actuation: i32,
        flags: u32,
        position: f32,
        intensity: f32,
    ) -> i32 {
        record(Call::Actuate {
            actuator: address(actuator),
            actuation,
            flags,
            position,
            intensity,
        });
        RECORD.with_borrow_mut(|record| {
            let failing = record.failing_actuations > 0;
            record.failing_actuations = record.failing_actuations.saturating_sub(1);
            i32::from(failing)
        })
    }

    unsafe extern "C" fn close(actuator: Ref) -> i32 {
        record(Call::Close(address(actuator)));
        0
    }

    unsafe extern "C" fn release(object: *const c_void) {
        record(Call::Release(object as usize));
    }

    const FAKES: Symbols = Symbols {
        create_list,
        count,
        item,
        actuates,
        device_id,
        create_actuator,
        open,
        actuate,
        close,
        release,
    };

    /// A loader that finds the fakes, and counts its runs.
    fn found() -> Option<Symbols> {
        RECORD.with_borrow_mut(|record| record.loads += 1);
        Some(FAKES)
    }

    /// A loader that finds nothing, as with a missing framework or function.
    fn missing() -> Option<Symbols> {
        RECORD.with_borrow_mut(|record| record.loads += 1);
        None
    }

    /// An actuator over the fakes, with a record that starts clean, and one
    /// device that can actuate: cargo gives each test its own thread, but
    /// `--test-threads=1` need not.
    fn fresh(load: fn() -> Option<Symbols>) -> Actuator {
        RECORD.with_borrow_mut(|record| {
            *record = Record {
                devices: vec![true],
                ..Record::default()
            }
        });
        Actuator::new(load, tell)
    }

    /// `play` of the tick, which is what the app plays most.
    fn play(actuator: &mut Actuator, now: Instant) -> bool {
        actuator.play(TICK.actuation, TICK.intensity, now)
    }

    /// The calls that find the device at `index`, the one that can actuate.
    fn finding(index: usize) -> [Call; 3] {
        [
            Call::CreateList,
            Call::DeviceId(address(device(index))),
            Call::Release(address(list())),
        ]
    }

    /// The calls of a successful open, from the device at `index`, the one
    /// that can actuate.
    fn opening_from(index: usize) -> Vec<Call> {
        let mut calls = finding(index).to_vec();
        calls.extend([
            Call::CreateActuator(DEVICE_ID + index as u64),
            Call::Open(address(actuator_ref())),
        ]);
        calls
    }

    /// The calls of a successful open, from the first device.
    fn opening() -> Vec<Call> {
        opening_from(0)
    }

    /// The calls of a count that finds no device to open: none can
    /// actuate, or more than one can.
    fn finding_none() -> [Call; 2] {
        [Call::CreateList, Call::Release(address(list()))]
    }

    /// The call of a successful tick.
    fn ticking() -> Call {
        Call::Actuate {
            actuator: address(actuator_ref()),
            actuation: 4,
            flags: 0,
            position: 0.0,
            intensity: 2.0,
        }
    }

    /// The calls of closing the open actuator.
    fn closing() -> [Call; 2] {
        let actuator = address(actuator_ref());
        [Call::Close(actuator), Call::Release(actuator)]
    }

    fn after(seconds: f64) -> Duration {
        Duration::from_secs_f64(seconds)
    }

    #[test]
    fn the_fakes_hand_out_distinct_objects() {
        let objects = [list(), device(0), device(1), device(2), actuator_ref()];
        for (index, object) in objects.iter().enumerate() {
            assert!(!object.is_null());
            assert!(!objects[index + 1..].contains(object), "{index}");
        }
        assert_eq!((index_of(device(0)), index_of(device(2))), (0, 2));
    }

    #[test]
    fn a_missing_framework_says_no_and_is_looked_for_once() {
        let mut actuator = fresh(missing);
        let t = Instant::now();
        for later in [0.0, 1.0, 10.0, 100.0] {
            assert!(!play(&mut actuator, t + after(later)), "{later} s on");
        }
        assert_eq!(loads(), 1);
        assert_eq!(calls(), vec![]);
        assert!(!actuator.is_open());
    }

    #[test]
    fn a_found_framework_is_loaded_once_however_many_plays_come() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        for later in [0.0, 0.1, 0.2, 30.0] {
            assert!(play(&mut actuator, t + after(later)), "{later} s on");
        }
        assert_eq!(loads(), 1);
    }

    #[test]
    fn a_play_opens_the_actuator_of_the_one_device_and_actuates_it() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        assert!(!actuator.is_open());
        assert!(play(&mut actuator, t));
        assert!(actuator.is_open());
        let mut expected = opening();
        expected.push(ticking());
        // The list is released as soon as the device's id is read, and the
        // actuator stays: nothing else is released, and nothing is closed.
        assert_eq!(calls(), expected);
    }

    /// The trackpad among other multitouch devices, such as a Magic Mouse,
    /// which cannot actuate.
    #[test]
    fn the_one_device_that_can_actuate_is_the_one_opened() {
        for (list, index) in [
            (vec![false, true], 1),
            (vec![false, false, true], 2),
            (vec![true, false], 0),
        ] {
            let mut actuator = fresh(found);
            devices(&list);
            assert!(play(&mut actuator, Instant::now()), "{list:?}");
            let mut expected = opening_from(index);
            expected.push(ticking());
            assert_eq!(calls(), expected, "{list:?}");
        }
    }

    /// No device that can actuate, or two, such as a Magic Trackpad beside
    /// a MacBook's: the press does not say which was touched, so nothing
    /// opens, and the public pattern plays under the finger that pressed.
    #[test]
    fn none_or_more_than_one_device_that_can_actuate_opens_nothing() {
        for list in [
            vec![],
            vec![false],
            vec![false, false],
            vec![true, true],
            vec![true, false, true],
        ] {
            let mut actuator = fresh(found);
            devices(&list);
            let t = Instant::now();
            assert!(!play(&mut actuator, t), "{list:?}");
            assert!(!actuator.is_open(), "{list:?}");
            assert_eq!(calls(), finding_none(), "{list:?}");
            assert_eq!(told(), vec![false], "{list:?}");
            // A try to open, like any other: the next comes 5 s on.
            assert!(!play(&mut actuator, t + after(4.999)), "{list:?}");
            assert_eq!(calls(), vec![], "{list:?}");
            assert!(!play(&mut actuator, t + REOPEN_AFTER), "{list:?}");
            assert_eq!(calls(), finding_none(), "{list:?}");
        }
    }

    /// The devices are counted again at a new press, and only then: a tick
    /// of the press under way counts nothing.
    #[test]
    fn a_second_trackpad_after_the_open_closes_the_actuator_at_the_next_press() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        assert!(play(&mut actuator, t));
        calls();
        devices(&[true, true]);
        assert!(play(&mut actuator, t + after(1.0)));
        assert_eq!(calls(), vec![ticking()], "within the press");
        actuator.new_press();
        assert!(!play(&mut actuator, t + after(10.0)));
        assert!(!actuator.is_open());
        // The count, the close, and the try to open that the count stops.
        let mut expected = finding_none().to_vec();
        expected.extend(closing());
        expected.extend(finding_none());
        assert_eq!(calls(), expected);
        assert_eq!(told(), vec![true, false]);
        // The second trackpad goes: the next try to open, 5 s after the
        // last, opens the one there is.
        devices(&[true]);
        actuator.new_press();
        assert!(!play(&mut actuator, t + after(14.999)));
        assert_eq!(calls(), vec![]);
        assert!(play(&mut actuator, t + after(15.0)));
        let mut expected = opening();
        expected.push(ticking());
        assert_eq!(calls(), expected);
    }

    #[test]
    fn a_new_press_with_the_same_one_device_keeps_the_actuator() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        assert!(play(&mut actuator, t));
        calls();
        actuator.new_press();
        assert!(play(&mut actuator, t + after(1.0)));
        let mut expected = finding(0).to_vec();
        expected.push(ticking());
        assert_eq!(calls(), expected);
        assert!(actuator.is_open());
        // The press is counted once.
        assert!(play(&mut actuator, t + after(1.1)));
        assert_eq!(calls(), vec![ticking()]);
    }

    /// The trackpad that was opened has gone, and another is the one now.
    #[test]
    fn a_new_press_with_another_one_device_opens_its_actuator() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        assert!(play(&mut actuator, t));
        calls();
        devices(&[false, true]);
        actuator.new_press();
        assert!(play(&mut actuator, t + after(10.0)));
        let mut expected = finding(1).to_vec();
        expected.extend(closing());
        expected.extend(opening_from(1));
        expected.push(ticking());
        assert_eq!(calls(), expected);
    }

    #[test]
    fn a_second_play_opens_nothing_and_actuates_again() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        assert!(play(&mut actuator, t));
        calls();
        assert!(play(&mut actuator, t + after(0.01)));
        assert_eq!(calls(), vec![ticking()]);
        assert!(play(&mut actuator, t + after(60.0)));
        assert_eq!(calls(), vec![ticking()]);
        assert!(actuator.is_open());
    }

    #[test]
    fn an_actuation_gets_the_feel_it_was_asked_for() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        for feel in [TICK, THUMP, PRESS] {
            assert!(actuator.play(feel.actuation, feel.intensity, t));
        }
        let actuated: Vec<(i32, u32, f32, f32)> = calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Actuate {
                    actuation,
                    flags,
                    position,
                    intensity,
                    ..
                } => Some((actuation, flags, position, intensity)),
                _ => None,
            })
            .collect();
        assert_eq!(
            actuated,
            vec![
                (TICK.actuation, 0, 0.0, TICK.intensity),
                (THUMP.actuation, 0, 0.0, THUMP.intensity),
                (PRESS.actuation, 0, 0.0, PRESS.intensity),
            ]
        );
    }

    /// The calls that `step` leaves before it fails, from a fresh actuator.
    fn calls_until(step: Failure) -> Vec<Call> {
        let actuator = address(actuator_ref());
        let mut calls = finding(0).to_vec();
        match step {
            Failure::CreateList => return vec![Call::CreateList],
            // The list is released once the device's id is read, whatever it
            // says.
            Failure::DeviceIdError | Failure::DeviceIdZero => {}
            Failure::CreateActuator => calls.push(Call::CreateActuator(DEVICE_ID)),
            // An actuator that will not open is released, once.
            Failure::Open => calls.extend([
                Call::CreateActuator(DEVICE_ID),
                Call::Open(actuator),
                Call::Release(actuator),
            ]),
        }
        calls
    }

    const STEPS: [Failure; 5] = [
        Failure::CreateList,
        Failure::DeviceIdError,
        Failure::DeviceIdZero,
        Failure::CreateActuator,
        Failure::Open,
    ];

    #[test]
    fn each_open_step_that_fails_says_no_and_frees_what_it_made() {
        for step in STEPS {
            let mut actuator = fresh(found);
            fail(Some(step));
            assert!(!play(&mut actuator, Instant::now()), "{step:?}");
            assert!(!actuator.is_open(), "{step:?}");
            let made = calls();
            assert_eq!(made, calls_until(step), "{step:?}");
            assert!(
                !made.iter().any(|call| matches!(call, Call::Actuate { .. })),
                "{step:?} actuated"
            );
            // Nothing is released twice, and nothing is closed: no actuator
            // was ever open.
            let released: Vec<&Call> = made
                .iter()
                .filter(|call| matches!(call, Call::Release(_)))
                .collect();
            for (index, call) in released.iter().enumerate() {
                assert!(!released[index + 1..].contains(call), "{step:?} {call:?}");
            }
            assert!(
                !made.iter().any(|call| matches!(call, Call::Close(_))),
                "{step:?}"
            );
        }
    }

    #[test]
    fn a_failed_open_blocks_the_next_until_five_seconds_on() {
        for step in STEPS {
            let mut actuator = fresh(found);
            fail(Some(step));
            let t = Instant::now();
            assert!(!play(&mut actuator, t));
            calls();
            // A hair short of the interval, nothing is tried: not a function
            // of the framework, whatever else is asked.
            assert!(!play(&mut actuator, t + after(4.999)), "{step:?}");
            assert_eq!(calls(), vec![], "{step:?}");
            assert!(!actuator.is_open(), "{step:?}");
            // On the interval, it is: `<` blocks and `>=` tries.
            assert!(!play(&mut actuator, t + REOPEN_AFTER), "{step:?}");
            assert_eq!(calls(), calls_until(step), "{step:?} at 5 s");
        }
    }

    #[test]
    fn the_interval_counts_from_the_last_try() {
        let mut actuator = fresh(found);
        fail(Some(Failure::CreateList));
        let t = Instant::now();
        assert!(!play(&mut actuator, t));
        assert!(!play(&mut actuator, t + after(5.0)));
        calls();
        // 9.999 s is 4.999 s after the try at 5 s, and 10 s is 5 s after it.
        assert!(!play(&mut actuator, t + after(9.999)));
        assert_eq!(calls(), vec![]);
        assert!(!play(&mut actuator, t + after(10.0)));
        assert_eq!(calls(), vec![Call::CreateList]);
    }

    #[test]
    fn an_open_that_comes_after_a_failed_one_plays() {
        let mut actuator = fresh(found);
        fail(Some(Failure::Open));
        let t = Instant::now();
        assert!(!play(&mut actuator, t));
        // The device comes back.
        fail(None);
        calls();
        assert!(play(&mut actuator, t + REOPEN_AFTER));
        assert!(actuator.is_open());
        let mut expected = opening();
        expected.push(ticking());
        assert_eq!(calls(), expected);
    }

    #[test]
    fn a_failed_actuation_long_after_the_open_closes_reopens_and_plays_again() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        assert!(play(&mut actuator, t));
        calls();
        fail_actuations(1);
        assert!(play(&mut actuator, t + after(10.0)));
        let mut expected = vec![ticking()];
        expected.extend(closing());
        expected.extend(opening());
        expected.push(ticking());
        assert_eq!(calls(), expected);
        assert!(actuator.is_open());
        // The reopened actuator plays on with no more opening.
        assert!(play(&mut actuator, t + after(10.01)));
        assert_eq!(calls(), vec![ticking()]);
    }

    #[test]
    fn a_failed_actuation_that_fails_again_after_the_reopen_says_no() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        assert!(play(&mut actuator, t));
        calls();
        fail_actuations(99);
        assert!(!play(&mut actuator, t + after(10.0)));
        // The retry that fails closes the actuator it reopened.
        let mut expected = vec![ticking()];
        expected.extend(closing());
        expected.extend(opening());
        expected.push(ticking());
        expected.extend(closing());
        assert_eq!(calls(), expected);
        assert!(!actuator.is_open());
        // The try to open was at 10 s: none comes before 15 s, and until
        // then nothing is called at all.
        for later in [11.0, 12.0, 14.999] {
            assert!(!play(&mut actuator, t + after(later)), "{later} s");
            assert_eq!(calls(), vec![], "{later} s");
        }
        // At 15 s it reopens, and the actuation fails once more.
        assert!(!play(&mut actuator, t + after(15.0)));
        let mut expected = opening();
        expected.push(ticking());
        expected.extend(closing());
        assert_eq!(calls(), expected);
        assert!(!actuator.is_open());
    }

    /// A device that opens but never actuates: the private path stops
    /// working once, and is never said to work, however often it reopens.
    #[test]
    fn an_actuator_that_opens_but_never_actuates_tells_so_once() {
        let mut actuator = fresh(found);
        fail_actuations(u32::MAX);
        let t = Instant::now();
        for later in [0.0, 1.0, 5.0, 10.0, 15.0, 30.0] {
            assert!(!play(&mut actuator, t + after(later)), "{later} s");
        }
        let opened = calls()
            .iter()
            .filter(|c| matches!(c, Call::Open(_)))
            .count();
        assert_eq!(opened, 5, "at 0, 5, 10, 15 and 30 s");
        assert_eq!(told(), vec![false]);
    }

    /// It tells as the private path starts to work, as it stops, and as it
    /// works again; never per play.
    #[test]
    fn the_actuator_tells_only_of_a_change() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        assert!(play(&mut actuator, t));
        assert!(play(&mut actuator, t + after(0.1)));
        assert_eq!(told(), vec![true]);
        // An actuation and its retry fail.
        fail_actuations(2);
        assert!(!play(&mut actuator, t + after(10.0)));
        assert!(!play(&mut actuator, t + after(11.0)));
        assert_eq!(told(), vec![false]);
        // The next open, 5 s after the last try, works.
        assert!(play(&mut actuator, t + after(15.0)));
        assert!(play(&mut actuator, t + after(15.1)));
        assert_eq!(told(), vec![true]);
    }

    #[test]
    fn a_failed_actuation_soon_after_the_open_closes_and_waits_to_reopen() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        assert!(play(&mut actuator, t));
        calls();
        fail_actuations(1);
        assert!(!play(&mut actuator, t + after(1.0)));
        let mut expected = vec![ticking()];
        expected.extend(closing());
        assert_eq!(calls(), expected);
        assert!(!actuator.is_open());
        // The open was tried 1 s ago: the next is 5 s after it.
        assert!(!play(&mut actuator, t + after(4.999)));
        assert_eq!(calls(), vec![]);
        assert!(play(&mut actuator, t + after(5.0)));
        let mut expected = opening();
        expected.push(ticking());
        assert_eq!(calls(), expected);
    }

    #[test]
    fn a_closed_actuator_is_not_closed_or_released_again() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        assert!(play(&mut actuator, t));
        calls();
        // The actuation fails at 10 s, and so does the reopen.
        fail_actuations(1);
        fail(Some(Failure::CreateList));
        assert!(!play(&mut actuator, t + after(10.0)));
        let mut expected = vec![ticking()];
        expected.extend(closing());
        expected.push(Call::CreateList);
        assert_eq!(calls(), expected);
        assert!(!actuator.is_open());
        // Later plays try to open and fail: nothing is closed or released.
        for later in [10.5, 15.0, 15.1, 20.0, 60.0] {
            assert!(!play(&mut actuator, t + after(later)), "{later} s");
        }
        let later = calls();
        assert!(
            later.iter().all(|call| matches!(call, Call::CreateList)),
            "{later:?}"
        );
        assert_eq!(later.len(), 3, "tries at 15, 20 and 60 s only: {later:?}");
    }

    #[test]
    fn an_actuator_is_closed_and_released_once_for_each_time_it_was_opened() {
        let mut actuator = fresh(found);
        let t = Instant::now();
        assert!(play(&mut actuator, t));
        // Three rounds of: an actuation fails 10 s after the last open, and
        // the reopen takes.
        for round in 1..4 {
            fail_actuations(1);
            let now = t + after(10.0 * f64::from(round));
            assert!(play(&mut actuator, now), "round {round}");
        }
        let all = calls();
        let opened = all.iter().filter(|c| matches!(c, Call::Open(_))).count();
        let closed = all.iter().filter(|c| matches!(c, Call::Close(_))).count();
        let released = all
            .iter()
            .filter(|c| *c == &Call::Release(address(actuator_ref())))
            .count();
        // The first play opens, and each later one closes and reopens. The
        // last is still open.
        assert_eq!((opened, closed, released), (4, 3, 3));
        assert!(actuator.is_open());
    }

    #[test]
    fn a_mask_with_pressure_events_is_a_trackpads() {
        assert!(from_trackpad(NSEventMask::Pressure));
        assert!(from_trackpad(
            NSEventMask::Pressure | NSEventMask::LeftMouseDown
        ));
    }

    #[test]
    fn a_mask_without_pressure_events_is_not_a_trackpads() {
        assert!(!from_trackpad(NSEventMask::empty()));
        assert!(!from_trackpad(NSEventMask::LeftMouseDown));
    }

    /// The owner's choices from the haptic lab, pinned in one place.
    #[test]
    fn the_feels_are_the_ones_the_owner_chose() {
        assert_eq!(
            TICK,
            Feel {
                actuation: 4,
                intensity: 2.0,
                pattern: Pattern::Alignment,
            }
        );
        assert_eq!(
            THUMP,
            Feel {
                actuation: 6,
                intensity: 2.0,
                pattern: Pattern::LevelChange,
            }
        );
        assert_eq!(PRESS, TICK);
        const { assert!(PRIVATE_ACTUATOR) };
        assert_eq!(REOPEN_AFTER, Duration::from_secs(5));
    }
}
