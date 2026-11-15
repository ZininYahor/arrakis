//! What the crate does when Metal fails: one flag for the process, raised once, that the app reads to
//! open its next window on glow. The painter never returns an error and never panics, so this is all
//! a failure does. The look for a device is here too, as no device is the same answer.

use std::sync::{OnceLock, Weak};
use std::time::Instant;

use ::winit_crate::window::Window;
use dispatch2::DispatchQueue;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSArray;
use objc2_metal::{MTLDevice, MTLGPUFamily};

/// Why the painter gave up, and when.
#[derive(Debug)]
pub struct Failure {
    pub why: String,
    pub at: Instant,
}

/// Raised once, by the first failure, and never lowered: every later window of the process is glow's.
/// The next launch tries Metal again.
#[derive(Debug, Default)]
pub(crate) struct Flag(OnceLock<Failure>);

impl Flag {
    const fn new() -> Self {
        Self(OnceLock::new())
    }

    pub(crate) fn is_up(&self) -> bool {
        self.0.get().is_some()
    }

    /// Raises it, unless a failure did already, and says why. The later reason goes unsaid in the
    /// flag: a stalled GPU fails every wait after the first.
    fn raise(&self, why: &str) {
        let mut first = false;
        self.0.get_or_init(|| {
            first = true;
            Failure {
                why: why.to_owned(),
                at: Instant::now(),
            }
        });
        if first {
            log::error!("the Metal painter failed, and the next window is glow's: {why}");
        }
    }

    #[cfg(test)]
    pub(crate) fn why(&self) -> Option<&str> {
        self.0.get().map(|failure| failure.why.as_str())
    }
}

/// The process's flag. A test of the crate keeps a flag of its own, as the other tests' painters would
/// otherwise stop painting.
static FLAG: Flag = Flag::new();

pub(crate) fn process_flag() -> &'static Flag {
    &FLAG
}

/// The failure of this process, once there is one.
pub fn failure() -> Option<&'static Failure> {
    FLAG.0.get()
}

/// Every failure of the painter ends here, on any thread. It raises `flag` and asks for the frame that
/// reads it: a window at rest has no next frame until input or the clock's minute.
///
/// The ask goes through the main queue on purpose, and never waits. `Window::request_redraw` off the
/// main thread blocks until the main thread answers (winit's `maybe_queue_on_main` calls
/// `maybe_wait_on_main`), so on Metal's thread it would hold up the handlers that the main thread's
/// bounded wait is waiting for. It upgrades on the main thread too, so that if this were the window's
/// last reference, it would not go on Metal's thread.
pub(crate) fn fail(flag: &Flag, why: &str, window: Weak<Window>) {
    flag.raise(why);
    if window.strong_count() > 0 {
        DispatchQueue::main().exec_async(move || {
            if let Some(window) = window.upgrade() {
                window.request_redraw();
            }
        });
    }
}

// The crate's own, as `objc2_metal`'s binding panics on nil and whether a Mac with no Metal returns nil
// is unread. "This API will not cause the system to switch devices" (`objc2-metal`'s `device.rs`, from
// `Metal/MTLDevice.h`), where the default device's call moves a dual-GPU Mac to its high power GPU.
//
// The signature is the header's: `MTL_EXTERN NSArray <id<MTLDevice>> *MTLCopyAllDevices(void) ...
// NS_RETURNS_RETAINED;` (`Metal/MTLDevice.h:137`), so no arguments and one object pointer back, to an
// `NSArray` of objects that conform to `MTLDevice`, as the declared type says. The header's nonnull
// region rules nil out, and `*mut` keeps a nil a value all the same: it reaches `Retained::from_raw`,
// which reads it. `objc2-metal` declares the same pointer and the same `C-unwind`, as an Objective-C
// exception may unwind through the call.
#[link(name = "Metal", kind = "framework")]
unsafe extern "C-unwind" {
    fn MTLCopyAllDevices() -> *mut NSArray<ProtocolObject<dyn MTLDevice>>;
}

/// Whether this Mac has a device the painter runs on: Apple's GPUs alone, so
/// an Intel Mac stays on glow. It asks once for the process: a window's start pays for the first look,
/// and the answer does not move.
pub fn has_device() -> bool {
    #[cfg(feature = "demo")]
    if fail_at() == Some(FailAt::Device) {
        return false;
    }
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(look_for_a_device)
}

fn look_for_a_device() -> bool {
    autoreleasepool(|_| {
        // SAFETY: the call takes nothing. Its name holds `Copy`, so the array is ours to release
        // ("NS_RETURNS_RETAINED", as `objc2-metal` reads the header), and `from_raw` takes that
        // reference, or gives `None` on nil.
        let devices = unsafe { Retained::from_raw(MTLCopyAllDevices()) };
        devices.is_some_and(|devices| {
            devices
                .iter()
                .any(|device| device.supportsFamily(MTLGPUFamily::Apple1))
        })
    })
}

/// `ARRAKIS_METAL_FAIL`, which makes each failure in a demo build: `scripts/painter-fallback.sh`.
#[cfg(feature = "demo")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailAt {
    /// The look for a device finds none.
    Device,
    /// The pipeline fails as the painter starts, before the first pass.
    Pipeline,
    /// `shader.metallib` will not load, so the shader compiles from its source, and the window stays
    /// on Metal: the one failure that is no fallback to glow.
    Metallib,
    /// The window's first `set_window` fails, before the first pass.
    Surface,
    /// The first Metal window's first frame fails.
    Frame,
    /// That frame's wait times out.
    Stall,
    /// The first frame that comes at least 1 s after the one before fails.
    Rest,
}

#[cfg(feature = "demo")]
impl FailAt {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "device" => Some(Self::Device),
            "pipeline" => Some(Self::Pipeline),
            "metallib" => Some(Self::Metallib),
            "surface" => Some(Self::Surface),
            "frame" => Some(Self::Frame),
            "stall" => Some(Self::Stall),
            "rest" => Some(Self::Rest),
            _ => None,
        }
    }
}

/// The failure this run was asked to make. A name that means nothing says so, and makes none.
#[cfg(feature = "demo")]
pub fn fail_at() -> Option<FailAt> {
    let value = std::env::var("ARRAKIS_METAL_FAIL").ok()?;
    let at = FailAt::parse(&value);
    if at.is_none() {
        log::warn!("ARRAKIS_METAL_FAIL={value}: no such failure, so none is made");
    }
    at
}

/// Raises the process's flag with no window and asks for no frame, for `tests/painter_fallback.rs`,
/// which has no painter to fail.
#[cfg(feature = "demo")]
pub fn fail_detached(why: &str) {
    FLAG.raise(why);
}

/// `ARRAKIS_METAL_TEAR=1`: every frame is one flat grey, 96 or 100 by turns, and no mesh is drawn, for
/// `scripts/measure/tear-captures.sh` ("Tears").
#[cfg(feature = "demo")]
pub(crate) fn tear() -> bool {
    std::env::var_os("ARRAKIS_METAL_TEAR").is_some_and(|value| value == "1")
}

/// The flag and the look for a device, with no window. Every flag here is the test's own: the process's
/// would stop the other tests' painters. Nothing sets `ARRAKIS_METAL_FAIL` or `ARRAKIS_METAL_TEAR` in
/// this process (`set_var` in libtest's threads is unsound): `scripts/painter-fallback.sh` reads those.
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_new_flag_is_down_and_the_first_failure_raises_it() {
        let flag = Flag::default();
        assert!(!flag.is_up());
        assert_eq!(flag.why(), None);
        let before = Instant::now();
        flag.raise("the first");
        assert!(flag.is_up());
        assert_eq!(flag.why(), Some("the first"));
        let raised = flag.0.get().map(|failure| failure.at);
        assert!(raised.is_some_and(|at| at >= before), "{raised:?}");
    }

    #[test]
    fn a_second_failure_keeps_the_first_ones_reason_and_time() {
        let flag = Flag::default();
        flag.raise("the first");
        let first_at = flag.0.get().map(|failure| failure.at);
        // Longer than the clock's tick, so a time taken again would differ.
        std::thread::sleep(Duration::from_millis(5));
        flag.raise("the second");
        flag.raise("the third");
        assert_eq!(flag.why(), Some("the first"));
        assert_eq!(flag.0.get().map(|failure| failure.at), first_at);
    }

    #[test]
    fn two_flags_are_independent() {
        let (one, other) = (Flag::default(), Flag::default());
        one.raise("one");
        assert!(one.is_up());
        assert!(!other.is_up(), "a test's flag does not reach another's");
        assert!(failure().is_none(), "nor does it reach the process's");
    }

    #[test]
    fn a_failure_with_no_window_raises_the_flag_and_does_not_wait_for_the_main_thread() {
        let flag: &'static Flag = Box::leak(Box::default());
        // A window that is gone: nothing to ask a frame of. This runs on libtest's thread, with the
        // main thread blocked in libtest, so a wait on it would never end.
        fail(flag, "no window", Weak::new());
        assert_eq!(flag.why(), Some("no window"));
        fail(flag, "again", Weak::new());
        assert_eq!(flag.why(), Some("no window"), "the first reason stands");
    }

    #[test]
    fn this_mac_has_a_device_and_the_answer_does_not_move() {
        if std::env::var_os("ARRAKIS_METAL_FAIL").is_some() {
            eprintln!("skipped: ARRAKIS_METAL_FAIL is set, and it can make the look find none");
            return;
        }
        let apple_gpu = objc2_metal::MTLCopyAllDevices()
            .iter()
            .any(|device| device.supportsFamily(MTLGPUFamily::Apple1));
        if cfg!(target_arch = "aarch64") {
            assert!(apple_gpu, "an Apple silicon Mac has an Apple GPU");
        }
        // The second answer comes from the first look's, and is the same.
        assert_eq!(has_device(), apple_gpu);
        assert_eq!(has_device(), apple_gpu);
    }

    #[cfg(feature = "demo")]
    #[test]
    fn each_failure_has_its_name_and_no_other_name_makes_one() {
        for (name, at) in [
            ("device", FailAt::Device),
            ("pipeline", FailAt::Pipeline),
            ("metallib", FailAt::Metallib),
            ("surface", FailAt::Surface),
            ("frame", FailAt::Frame),
            ("stall", FailAt::Stall),
            ("rest", FailAt::Rest),
        ] {
            assert_eq!(FailAt::parse(name), Some(at), "{name}");
        }
        for name in ["", "Frame", "frames", " rest", "rest ", "all", "1", "metal"] {
            assert_eq!(FailAt::parse(name), None, "{name:?}");
        }
    }
}
