//! What a close does with the surfaces the window last showed.
//!
//! The layer keeps its contents until the window goes, and the view takes the layer with it: a layer
//! cleared with the window up shows the window bare, in 20 of 30 closes captured.
//!
//! That leaves the surface. By the time the window has gone nothing in this process retains it but
//! `IOSurfaceIsInUse` still says yes, so another process holds it, and this one is charged for it, as
//! the one that made it. Nothing done to the layer reaches that holder any more: a flush, a clear from
//! here, and ordering the window out and then clearing all read the same. A close inside a burst has
//! a spare the layer showed a frame ago, and the holder may keep that one too.
//!
//! So a close parks here every surface its painter still holds: the one shown, the spare, and one a
//! failed frame left half drawn. [`empty_closed_surface`] throws their contents away once the window
//! has gone. Those bytes stop counting at once, whoever goes on holding a surface. A close to a quit
//! never gets there: the process ends with them, as it does with what a glow window leaves.
//!
//! The cell holds one close's surfaces. A close with none leaves it as it is, as a destroy and then a
//! drop both come here, and a close over one that nothing emptied drops those.
//!
//! The main thread's only, read through `try_with`, as `spare.rs` is, for the same reason.

use std::cell::Cell;
use std::ptr;

use objc2_core_foundation::CFRetained;
use objc2_io_surface::{IOSurfacePurgeabilityState, IOSurfaceRef};

thread_local! {
    static CLOSED: Cell<Vec<CFRetained<IOSurfaceRef>>> = const { Cell::new(Vec::new()) };
}

/// Parks a closing window's surfaces, which no command buffer may still render into. With this
/// thread's locals gone, they go with the closure.
pub(super) fn park(surfaces: Vec<CFRetained<IOSurfaceRef>>) {
    if !surfaces.is_empty() {
        let _ = CLOSED.try_with(|closed| closed.set(surfaces));
    }
}

/// Parks one small real surface, as a close does, and says whether the system gave one. For the app's
/// test that its close reaches [`empty_closed_surface`] through the real constructor: no window is
/// needed, and a demo build only, so a shipped one has no such function.
#[cfg(feature = "demo")]
#[doc(hidden)]
pub fn park_a_surface_for_a_test() -> bool {
    let Some(surface) = super::new_surface(8, 8) else {
        return false;
    };
    park(vec![surface]);
    true
}

/// Throws away the contents of the surfaces a closed window left, and says whether there was one. The
/// caller knows that the window is off the screen, or this would blank it: it is `fastframe_shell`'s
/// `Resident::window_gone`, which comes after `eframe::run_native` has returned, and that drops the
/// window, whose `Drop` in the patched winit orders it out and flushes that first.
pub fn empty_closed_surface() -> bool {
    let surfaces = CLOSED.try_with(Cell::take).unwrap_or_default();
    for surface in &surfaces {
        // SAFETY: `surface` is alive for the call, the state is one `IOSurfaceTypes.h` names, and the
        // old state's pointer is `__nullable` (`IOSurfaceRef.h`). No frame draws into it: `take_down`
        // parks only once every slot's command buffer has reached a final status, and the next window
        // has surfaces of its own.
        let status = unsafe {
            surface.set_purgeable(
                IOSurfacePurgeabilityState::PurgeableEmpty.bits(),
                ptr::null_mut(),
            )
        };
        if status != libc::KERN_SUCCESS {
            log::warn!("a closed window's surface was not emptied: kern_return_t {status}");
        }
    }
    !surfaces.is_empty()
}
