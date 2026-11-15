//! The surface neither shown nor being drawn: the spare of a burst, on the main thread only, so that a
//! window at rest holds one surface.
//!
//! - A frame takes the spare to draw into, and makes a surface when there is none.
//! - A frame of a burst parks the surface the layer stopped showing. A frame out of a burst parks it
//!   only while the link runs, whose pause will drop it, and otherwise drops it itself. With the link
//!   not ticking though `running` is true, the display asleep for one, that keeps a second surface
//!   until a tick or the close: read from the code only.
//! - The link's pause hands the main thread one block that captures nothing, which empties the cell
//!   unless a frame has started the link again by then: the next pause does it. The main thread runs
//!   one block or frame at a time, so a frame and the drop never meet, and the link's thread holds no
//!   surface.
//! - A close takes what is here to `close.rs`'s cell, and a block that comes after that finds nothing.
//!
//! A thread's locals may be gone at its exit, and a painter may be dropped then. So every read goes
//! through `try_with`, and finds no spare.

use std::cell::Cell;

use super::Target;

thread_local! {
    static SPARE: Cell<Option<Target>> = const { Cell::new(None) };
}

/// The spare, leaving the cell empty.
pub(super) fn take() -> Option<Target> {
    SPARE.try_with(Cell::take).ok().flatten()
}

/// Parks `target`. With this thread's locals gone, it goes with the closure.
pub(super) fn park(target: Target) {
    let _ = SPARE.try_with(|spare| spare.set(Some(target)));
}

/// Empties the cell, and says whether that dropped a surface.
pub(crate) fn drop_spare() -> bool {
    take().is_some()
}
