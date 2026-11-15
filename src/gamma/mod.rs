//! The displays' color transfer tables: the one place the screen is
//! actually painted.
//!
//! macOS has a per-display gamma table that every pixel passes through on
//! its way to the panel. Scaling each channel of it tints everything,
//! including full-screen video and the cursor, which an overlay window
//! could not. macOS drops the table when the process that set it exits, so
//! a crash or a quit always gives the normal colors back.
//!
//! Only macOS is implemented; elsewhere [`Displays`] reports that tinting
//! is unsupported.

#[cfg(target_os = "macos")]
mod macos;
/// The notices `watch_system` listens for, for `tests/observers.rs`.
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub use macos::{Center, system_notices};
#[cfg(target_os = "macos")]
pub use macos::{Displays, watch_reconfiguration, watch_system};
/// The read path, for the benchmarks' probe: never `write`, `apply` or
/// `restore`.
#[cfg(all(target_os = "macos", feature = "demo"))]
pub(crate) use macos::{Table, online_displays};

#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
pub use unsupported::{Displays, watch_reconfiguration, watch_system};

/// Table entries sent per channel. macOS interpolates to the hardware size.
pub const TABLE_SIZE: usize = 256;

/// One channel's table: a straight ramp scaled by `multiplier`.
pub fn ramp(multiplier: f32) -> [f32; TABLE_SIZE] {
    std::array::from_fn(|index| index as f32 / (TABLE_SIZE - 1) as f32 * multiplier.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_runs_from_black_to_the_multiplier() {
        let table = ramp(0.5);
        assert_eq!(table[0], 0.0);
        assert!((table[TABLE_SIZE - 1] - 0.5).abs() < 1e-6);
        assert!(table.windows(2).all(|pair| pair[0] <= pair[1]));
    }
}
