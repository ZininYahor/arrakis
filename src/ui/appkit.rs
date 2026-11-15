//! AppKit, for the window: the animation its close would start, what its last
//! frame leaves held once it has closed, and the helper process that a window
//! starts.

use std::ptr::NonNull;

use objc2::rc::autoreleasepool;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
// AppKit's OpenGL is deprecated, and it is what the stack draws with today.
#[allow(deprecated)]
use objc2_app_kit::{
    NSBackingStoreType, NSOpenGLContext, NSOpenGLPFAAllowOfflineRenderers, NSOpenGLPFAColorSize,
    NSOpenGLPFADoubleBuffer, NSOpenGLPFAOpenGLProfile, NSOpenGLPixelFormat,
    NSOpenGLPixelFormatAttribute, NSOpenGLProfileVersion4_1Core, NSView, NSWindow,
    NSWindowAnimationBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSDictionary, NSNumber, NSPoint, NSRect, NSSize, NSString, NSUserDefaults};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// The pixel format of [`release_closed_window`]'s context, with the zero
/// that ends such a list: the OpenGL the window draws with, and a back buffer
/// for its one flush to present. An undrawn one will do, so nothing here asks
/// for depth or stencil. Public only for `tests/closed_keeps.rs`, whose
/// context must be of the same format.
///
/// `NSOpenGLPFAAllowOfflineRenderers` is there because glutin asks for it on
/// the window's own context, and this one must match it. It does not make the
/// app safe on a Mac with two GPUs: Apple's QA1734
/// (`https://developer.apple.com/library/archive/qa/qa1734/_index.html`) says
/// that an app that creates an OpenGL context moves such a Mac to the
/// discrete GPU until it quits, and names only the `Info.plist` key
/// `NSSupportsAutomaticGraphicsSwitching`, not this attribute. `scripts/
/// Info.plist` has no such key. No dual-GPU Mac was tried.
#[doc(hidden)]
#[allow(deprecated)]
pub const FLUSH_FORMAT: [NSOpenGLPixelFormatAttribute; 7] = [
    NSOpenGLPFAOpenGLProfile,
    NSOpenGLProfileVersion4_1Core,
    NSOpenGLPFADoubleBuffer,
    NSOpenGLPFAColorSize,
    32,
    NSOpenGLPFAAllowOfflineRenderers,
    0,
];

/// The side of the window that [`release_closed_window`] flushes, in points.
/// The surfaces that outlive the flush are this small, 8 x 8 px at 2x.
const FLUSH_SIDE: f64 = 4.0;

/// Lets go of what the closed window's last frame still holds, with the
/// window gone. [`crate::app::App`] calls it once for each glow window that
/// has closed with the app living on.
///
/// Two holders stay after eframe drops the window, and nothing in the app
/// makes them let go while the window stays closed:
/// - **The window's `NSOpenGLContext` stays current on the main thread.**
///   Neither eframe nor glutin clears it, and it holds one GL buffer. Making
///   another context current lets it go.
/// - **AppKit's private CGL context keeps the last blit's textures** until
///   the next flush through that path, and they hold the surfaces. One
///   `flushBuffer` of any other view lets them go, undrawn.
///
/// So one new context, on a window of [`FLUSH_SIDE`] points that is never
/// ordered in, answers both, and then goes with the rest of what is made
/// here. It takes 4 to 10 ms on the main thread, with the window already
/// gone.
///
/// A Metal window holds neither, so it runs only after a glow window, which
/// the fallback still opens. After a Metal window it would only load OpenGL.
///
/// It paints nothing and touches no display: none of `CGDisplay*` or
/// `CGSet*` is called here, so it cannot move the tint.
///
/// If AppKit gives no pixel format or context, it returns before it has made
/// a window or moved the current context: the app stays as it would have
/// been without it. Off the main thread it does nothing: AppKit's windows
/// are the main thread's.
pub fn release_closed_window() {
    // SAFETY: the list ends with its zero, and the two attributes in it that
    // take a value have theirs.
    unsafe { release_closed_window_with(&FLUSH_FORMAT) };
}

/// [`release_closed_window`], asking for `attributes`. Public only for
/// `tests/closed_keeps.rs`, which asks for a format that AppKit refuses.
///
/// # Safety
///
/// `attributes` ends with a zero, and each attribute in it that takes a value
/// has one. AppKit is handed no length: it reads the list until the zero, and
/// a value-taking attribute at the end makes it read past the slice.
#[doc(hidden)]
#[allow(deprecated)]
pub unsafe fn release_closed_window_with(attributes: &[NSOpenGLPixelFormatAttribute]) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // The main thread has no pool here, so what AppKit autoreleases on the
    // way would keep the window and its context for good.
    autoreleasepool(|_| {
        // SAFETY: the caller's list ends as AppKit needs, and the pointer has
        // the whole of it. AppKit only reads it, during the call.
        let format = unsafe {
            NSOpenGLPixelFormat::initWithAttributes(
                NSOpenGLPixelFormat::alloc(),
                NonNull::from(attributes).cast(),
            )
        };
        let Some(format) = format else {
            return;
        };
        let Some(context) =
            NSOpenGLContext::initWithFormat_shareContext(NSOpenGLContext::alloc(), &format, None)
        else {
            return;
        };
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(FLUSH_SIDE, FLUSH_SIDE));
        // SAFETY: a window made on the main thread. AppKit would release it at
        // its close, under the `Retained` that owns it; the next call turns
        // that off before anything can close it.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: this only takes that release away; `Retained` still owns the
        // window, and releases it at the pool's end at the latest.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
        if let Some(view) = window.contentView() {
            context.setView(Some(&view), mtm);
            context.makeCurrentContext();
            context.flushBuffer();
            context.clearDrawable();
            NSOpenGLContext::clearCurrentContext();
        }
        window.close();
    });
}

/// Keeps AppKit from animating `frame`'s window out as it closes.
///
/// eframe drops the window after the pass that closes it, and winit orders
/// it out and closes it as it goes. On macOS 26.6 that order-out starts
/// AppKit's window transform animation on a GCD thread, and the animation
/// does not end. It holds the closed window with its views and layers, and
/// blocks the thread: about 0.37 MB and one thread for each close, with no
/// limit. With no animation behavior, the order-out starts none, and the
/// window is freed as it closes.
///
/// `ui::Window::logic` calls it in the pass that closes the window, and in
/// no other:
/// - **Not as the window is made.** eframe makes it hidden and orders it in
///   after the first frame, so the behavior would also take away the
///   animation the window opens with.
/// - **Not in `eframe::App::on_exit`,** which eframe calls once before it
///   drops the window. It gets no `Frame`, so each app creator would have to
///   keep the window for it, and the driver's `Driven` to pass it on, where
///   one forgotten would bring the growth back unseen.
///
/// A window that goes without a close pass ends the process with it, so no
/// closed window is left to pile up:
/// - an error out of `eframe::run_native`, which `fastframe_shell`'s
///   `Shell::run` returns at once, and `main` after it;
/// - the system ending the app;
/// - the demo's plate, whose process takes one shot and ends.
///
/// A new way for a window to go, with the process left running, must pass
/// through a close pass too, or it costs a thread each time.
///
/// The lookup of the window from the frame mirrors `fastframe_titlebar`'s
/// private `macos::appkit::align` (v0.13.0). The crate's public API only
/// moves the traffic lights, and eframe's `Frame::winit_window` leads to the
/// same handle, so neither gives the `NSWindow`.
pub(super) fn skip_close_animation(frame: &eframe::Frame) {
    let Ok(handle) = frame.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: eframe supplies its window's live NSView, and this runs on
    // eframe's main thread.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    if let Some(window) = view.window() {
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    }
}

/// The user default that AppKit reads before it starts `AutoFill (arrakis)`
/// (`com.apple.SafariPlatformSupport.Helper`). An undocumented key, set
/// through a public API.
const AUTOFILL_KEY: &str = "NSAutoFillHeuristicsEnabled";

/// Keeps AppKit from starting its AutoFill helper with the window.
///
/// With the key left alone, a window starts `AutoFill (arrakis)`, which checks
/// in as the window opens: within 3 s, 9.3 MiB in a row of Activity Monitor of
/// its own while a window is open. It had gone 0.5 s after the last of five
/// closes, with the app running on, and whether each open starts it again is
/// open.
///
/// What the key does to typing: the owner typed in Start and End with the key
/// registered, by hand, on 2026-10-31, and digits, `:`, the arrows and the wheel all
/// worked, on the US and the Russian layouts, with no new helper on a text
/// focus or across three or four reopens. That is one run on one Mac. Start and End
/// are `input::Spinner`s, with no IME asked for, but their digits and keys
/// come through AppKit's text input on winit's view, so nothing in the code
/// isolates them from what the key changes.
///
/// `registerDefaults:` answers the key in this process's registration domain
/// and writes nothing to the preferences, so the user's own defaults and
/// other apps stay as they are. That domain has the lowest precedence: a
/// stored YES overrides this call, and a stored NO makes a real check pass
/// without exercising it. If a macOS drops the key, the helper comes back,
/// and nothing else changes.
///
/// `entrypoint::wire` calls it first of all, on the way to every window of
/// the app and the driver: the key must be answered before the first window,
/// and first of all, so that no AppKit object has read it already. A demo
/// shot does not go through `wire`, so its window starts the helper as ever.
pub(crate) fn skip_autofill_helper() {
    autoreleasepool(|_| {
        let key = NSString::from_str(AUTOFILL_KEY);
        let off = NSNumber::new_bool(false);
        let off: &AnyObject = &off;
        let table = NSDictionary::from_slices(&[&*key], &[off]);
        // SAFETY: the table is `NSString` to `NSNumber`, which `NSUserDefaults`
        // takes as a registration table.
        unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&table) };
    });
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use objc2::rc::Retained;

    use super::*;

    /// The value of the key, or `None` where it has none.
    fn as_bool(value: Option<Retained<AnyObject>>) -> Option<bool> {
        let value = value?;
        let number = value
            .downcast_ref::<NSNumber>()
            .expect("the key's answer is a number");
        Some(number.boolValue())
    }

    /// What `defaults` prints for `args`, or `None` where it says that the
    /// pair does not exist. Any other failure panics, so that a read that
    /// read nothing is never taken for an absent key.
    fn read(args: &[&str]) -> Option<String> {
        let output = Command::new("/usr/bin/defaults")
            .args(args)
            .output()
            .expect("macOS's `defaults`");
        if output.status.success() {
            return Some(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("does not exist"),
            "`defaults` failed on {args:?}: {}, {stderr}",
            output.status
        );
        None
    }

    /// What the user's preferences store for the key, in this process's domain
    /// and the global one, as another process finds it: `defaults` reads what
    /// `cfprefsd` and the disk hold, and never this process's registration
    /// domain. An unbundled executable's domain is its name.
    fn stored() -> [Option<String>; 2] {
        let exe = std::env::current_exe().expect("the test's executable");
        let name = exe.file_name().and_then(|name| name.to_str());
        let name = name.expect("the executable's name is text");
        [name, "-g"].map(|domain| read(&["read", domain, AUTOFILL_KEY]))
    }

    #[test]
    fn the_defaults_answer_no_for_the_autofill_key_and_store_nothing() {
        // Whatever this machine stores already: a `defaults write -g` of the
        // key is an obvious thing to try.
        let before = stored();

        skip_autofill_helper();

        let key = NSString::from_str(AUTOFILL_KEY);
        let defaults = NSUserDefaults::standardUserDefaults();
        // The registration domain, which lives and dies with this process,
        // holds the NO...
        // SAFETY: a constant that Foundation exports and nothing writes.
        let domain = unsafe { objc2_foundation::NSRegistrationDomain };
        let registered = defaults.volatileDomainForName(domain);
        assert_eq!(as_bool(registered.objectForKey(&key)), Some(false));
        // ...which is the answer, unless a stored value overrides it...
        if before == [None, None] {
            assert_eq!(as_bool(defaults.objectForKey(&key)), Some(false));
        }
        // ...and nothing was stored for the app or for anyone.
        assert_eq!(
            stored(),
            before,
            "the call changed what the user's defaults store for `{AUTOFILL_KEY}`"
        );
    }

    #[test]
    fn a_key_that_no_domain_holds_reads_as_nothing() {
        assert_eq!(read(&["read", "-g", "ArrakisNoSuchKey"]), None);
    }

    #[test]
    #[should_panic(expected = "`defaults` failed")]
    fn a_failed_read_of_the_defaults_is_not_taken_for_an_absent_key() {
        read(&["no-such-verb"]);
    }
}
