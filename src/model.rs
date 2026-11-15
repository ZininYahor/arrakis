//! Everything the window and the menu-bar item can ask for. Views push
//! these; only `App::apply` acts on them.

use crate::color::Look;
use crate::schedule::ClockTime;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// A new night look. `dragging` is true while the primary button is
    /// down on a slider, from the frame it goes down, so the real screen
    /// previews it at once.
    SetNight {
        look: Look,
        dragging: bool,
    },
    /// The slider was let go.
    EndPreview,
    /// The Temperature slider's value caught a detent or passed one under
    /// the pointer: one tick on the trackpad.
    Detent,
    /// A beat of the Temperature slider's thumper (see `ui::input`): one
    /// thump on the trackpad.
    Thump,
    /// The pointer's primary button went down on a button in the window:
    /// one press on the trackpad. The keys and VoiceOver never push it.
    Press,
    SetSchedule(bool),
    SetStart(ClockTime),
    SetEnd(ClockTime),
    SetFade(u16),
    RedNow(bool),
    ShowWindow,
    HideWindow,
    Quit,
}
