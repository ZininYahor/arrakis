//! Platforms without a tint yet: everything compiles, nothing is painted.

use crate::color::Tint;
use crate::engine::Screen;

#[derive(Default)]
pub struct Displays;

impl Displays {
    pub fn new() -> Self {
        Self
    }
}

impl Screen for Displays {
    fn apply(&mut self, _tint: Tint) -> Result<usize, String> {
        Err("tinting the screen is only supported on macOS".to_owned())
    }

    fn intact(&mut self) -> bool {
        true
    }

    fn restore(&mut self) {}

    fn display_count(&mut self) -> usize {
        0
    }
}

pub fn watch_reconfiguration(_callback: impl Fn() + Send + Sync + 'static) {}

pub fn watch_system(_callback: impl Fn() + Send + Sync + 'static) {}
