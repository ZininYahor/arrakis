//! CoreGraphics transfer tables.
//!
//! Each display's own table (identity, or a calibration from its ColorSync
//! profile) is read once, before the first paint, and every tint is that
//! table scaled per channel. Normal colors come back by writing it again:
//! `CGDisplayRestoreColorSyncSettings` leaves a table this process set in
//! place (macOS 26, tested), and is reported to blank some screens.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::OnceLock;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceDidTerminateApplicationNotification, NSWorkspaceDidWakeNotification,
    NSWorkspaceScreensDidWakeNotification, NSWorkspaceSessionDidBecomeActiveNotification,
};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayChangeSummaryFlags, CGDisplayGammaTableCapacity,
    CGDisplayRegisterReconfigurationCallback, CGError, CGGetDisplayTransferByTable,
    CGGetOnlineDisplayList, CGSetDisplayTransferByTable,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSSystemClockDidChangeNotification,
    NSSystemTimeZoneDidChangeNotification,
};

use super::{TABLE_SIZE, ramp};
use crate::color::Tint;
use crate::engine::Screen;

/// More displays than a Mac can drive.
const MAX_DISPLAYS: usize = 32;
/// How far a read-back table may differ from ours and still count as ours:
/// macOS may resample it to the hardware size.
const TOLERANCE: f32 = 0.01;
/// Where along a table to compare it with another.
const PROBES: [f32; 5] = [0.1, 0.3, 0.5, 0.75, 1.0];

/// Every online display, hardware mirrors included: a mirror that is not
/// drawn to still shows its own table.
pub(crate) fn online_displays() -> Result<Vec<CGDirectDisplayID>, String> {
    let mut ids = [0 as CGDirectDisplayID; MAX_DISPLAYS];
    let mut count = 0u32;
    // SAFETY: both pointers are valid for `MAX_DISPLAYS` ids and one count.
    let error =
        unsafe { CGGetOnlineDisplayList(MAX_DISPLAYS as u32, ids.as_mut_ptr(), &mut count) };
    if error != CGError::Success {
        return Err(format!("listing displays failed (CGError {})", error.0));
    }
    Ok(ids[..(count as usize).min(MAX_DISPLAYS)].to_vec())
}

/// One display's transfer table, a curve per channel from black to white.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Table {
    red: Vec<f32>,
    green: Vec<f32>,
    blue: Vec<f32>,
}

impl Table {
    fn identity() -> Self {
        let line = ramp(1.0).to_vec();
        Self {
            red: line.clone(),
            green: line.clone(),
            blue: line,
        }
    }

    pub(crate) fn read(display: CGDirectDisplayID) -> Option<Self> {
        let capacity = CGDisplayGammaTableCapacity(display).max(TABLE_SIZE as u32) as usize;
        let mut red = vec![0.0f32; capacity];
        let mut green = vec![0.0f32; capacity];
        let mut blue = vec![0.0f32; capacity];
        let mut samples = 0u32;
        // SAFETY: the three tables hold `capacity` values each.
        let error = unsafe {
            CGGetDisplayTransferByTable(
                display,
                capacity as u32,
                red.as_mut_ptr(),
                green.as_mut_ptr(),
                blue.as_mut_ptr(),
                &mut samples,
            )
        };
        let samples = (samples as usize).min(capacity);
        if error != CGError::Success || samples < 2 {
            return None;
        }
        for channel in [&mut red, &mut green, &mut blue] {
            channel.truncate(samples);
        }
        Some(Self { red, green, blue })
    }

    pub(crate) fn scaled(&self, tint: Tint) -> Self {
        let scale = |channel: &[f32], by: f32| {
            channel
                .iter()
                .map(|value| value * by.clamp(0.0, 1.0))
                .collect()
        };
        Self {
            red: scale(&self.red, tint.red),
            green: scale(&self.green, tint.green),
            blue: scale(&self.blue, tint.blue),
        }
    }

    fn write(&self, display: CGDirectDisplayID) -> CGError {
        // SAFETY: the three tables are the same length and outlive the call.
        unsafe {
            CGSetDisplayTransferByTable(
                display,
                self.red.len() as u32,
                self.red.as_ptr(),
                self.green.as_ptr(),
                self.blue.as_ptr(),
            )
        }
    }

    /// Whether two tables draw the same curves, whatever their sizes.
    pub(crate) fn matches(&self, other: &Self) -> bool {
        [
            (&self.red, &other.red),
            (&self.green, &other.green),
            (&self.blue, &other.blue),
        ]
        .into_iter()
        .all(|(a, b)| {
            PROBES
                .iter()
                .all(|&at| (sample(a, at) - sample(b, at)).abs() <= TOLERANCE)
        })
    }
}

/// The curve's value `at` of the way along, between the nearest entries.
fn sample(channel: &[f32], at: f32) -> f32 {
    let position = at.clamp(0.0, 1.0) * (channel.len() - 1) as f32;
    let below = position.floor() as usize;
    let above = (below + 1).min(channel.len() - 1);
    let fraction = position - below as f32;
    channel[below] + (channel[above] - channel[below]) * fraction
}

#[derive(Default)]
pub struct Displays {
    /// Each display's table before the first paint.
    originals: HashMap<CGDirectDisplayID, Table>,
    /// What we last painted, to check the displays still show it.
    painted: Option<Tint>,
}

impl Displays {
    pub fn new() -> Self {
        Self::default()
    }

    fn original(&mut self, display: CGDirectDisplayID) -> &Table {
        self.originals.entry(display).or_insert_with(|| {
            Table::read(display).unwrap_or_else(|| {
                log::warn!("unable to read display {display}'s color table; assuming none");
                Table::identity()
            })
        })
    }
}

impl Screen for Displays {
    fn apply(&mut self, tint: Tint) -> Result<usize, String> {
        let displays = online_displays()?;
        let mut painted = 0;
        let mut last_error = None;
        for display in displays {
            let error = self.original(display).scaled(tint).write(display);
            if error == CGError::Success {
                painted += 1;
            } else {
                last_error = Some(error.0);
            }
        }
        self.painted = Some(tint);
        match (painted, last_error) {
            (0, Some(code)) => Err(format!("macOS refused the color table (CGError {code})")),
            (0, None) => Err("no displays are connected".to_owned()),
            (count, _) => Ok(count),
        }
    }

    fn intact(&mut self) -> bool {
        let Some(tint) = self.painted else {
            return true;
        };
        let Ok(displays) = online_displays() else {
            return false;
        };
        displays.into_iter().all(|display| {
            let Some(original) = self.originals.get(&display) else {
                // Connected since the last paint.
                return false;
            };
            Table::read(display).is_some_and(|shown| shown.matches(&original.scaled(tint)))
        })
    }

    fn restore(&mut self) {
        self.painted = None;
        for (display, original) in &self.originals {
            let error = original.write(*display);
            if error != CGError::Success {
                log::debug!(
                    "display {display} gone or refused its table back (CGError {})",
                    error.0
                );
            }
        }
    }

    fn display_count(&mut self) -> usize {
        online_displays()
            .map(|displays| displays.len())
            .unwrap_or(0)
    }
}

static ON_RECONFIGURE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

unsafe extern "C-unwind" fn reconfigured(
    _display: CGDirectDisplayID,
    flags: CGDisplayChangeSummaryFlags,
    _user_info: *mut c_void,
) {
    // The callback comes twice: before the change and after it. Only the
    // second one finds the new state.
    if flags.contains(CGDisplayChangeSummaryFlags::BeginConfigurationFlag) {
        return;
    }
    if let Some(callback) = ON_RECONFIGURE.get() {
        callback();
    }
}

/// Calls `callback` after a display is added, removed, or changes mode,
/// when macOS may have reset its table. Call once, on the main thread: the
/// notification arrives through the main run loop.
pub fn watch_reconfiguration(callback: impl Fn() + Send + Sync + 'static) {
    if ON_RECONFIGURE.set(Box::new(callback)).is_err() {
        return;
    }
    // SAFETY: `reconfigured` matches the callback type and ignores
    // `user_info`, which is null.
    let error = unsafe {
        CGDisplayRegisterReconfigurationCallback(Some(reconfigured), std::ptr::null_mut())
    };
    if error != CGError::Success {
        log::warn!("unable to watch display changes (CGError {})", error.0);
    }
}

static ON_SYSTEM: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// The notification center a notice is posted on.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Center {
    /// `NSWorkspace`'s own.
    Workspace,
    /// Foundation's default one.
    Foundation,
}

impl Center {
    /// This process's center of the kind.
    pub fn get(self) -> Retained<NSNotificationCenter> {
        match self {
            Self::Workspace => NSWorkspace::sharedWorkspace().notificationCenter(),
            Self::Foundation => NSNotificationCenter::defaultCenter(),
        }
    }
}

/// The notices [`watch_system`] listens for, each with its center: the one
/// list that it and `tests/observers.rs` read.
#[doc(hidden)]
pub fn system_notices() -> [(Center, &'static NSNotificationName); 6] {
    // SAFETY: AppKit's and Foundation's own constants, which their
    // frameworks define and never change.
    unsafe {
        [
            (Center::Workspace, NSWorkspaceDidWakeNotification),
            (Center::Workspace, NSWorkspaceScreensDidWakeNotification),
            (
                Center::Workspace,
                NSWorkspaceSessionDidBecomeActiveNotification,
            ),
            (
                Center::Workspace,
                NSWorkspaceDidTerminateApplicationNotification,
            ),
            (Center::Foundation, NSSystemClockDidChangeNotification),
            (Center::Foundation, NSSystemTimeZoneDidChangeNotification),
        ]
    }
}

/// Calls `callback` when macOS may have reset the tables, or moved the
/// clock under the schedule, without a display change ([`system_notices`]):
/// - the Mac woke, or its screens did;
/// - the user switched back to this session;
/// - an app quit, a gamma app among them, which may take its table with it;
/// - the clock or the time zone changed. A new time zone first drops the
///   zone the clock keeps (`engine::time_zone_changed`), so whatever reads
///   the clock after the callback reads the new one.
///
/// Call once, on the main thread; a later call watches nothing more. The
/// notifications come through the main run loop, which both the window's
/// loop and the headless pump (`crate::idle`) run. The callback runs where
/// macOS posts them, so it must be safe on any thread.
pub fn watch_system(callback: impl Fn() + Send + Sync + 'static) {
    if ON_SYSTEM.set(Box::new(callback)).is_err() {
        return;
    }
    let block = RcBlock::new(|notification: NonNull<NSNotification>| {
        // SAFETY: the center hands the block the notice it posts, alive for
        // the call.
        let name = unsafe { notification.as_ref() }.name();
        // SAFETY: Foundation's own constant.
        if *name == *unsafe { NSSystemTimeZoneDidChangeNotification } {
            crate::engine::time_zone_changed();
        }
        if let Some(callback) = ON_SYSTEM.get() {
            callback();
        }
    });
    for (center, name) in system_notices() {
        // SAFETY: no object to filter on. With no queue the block runs on
        // the posting thread, and it may: it captures nothing, drops the
        // clock's zone behind a lock, and calls a callback that is
        // `Send + Sync`.
        let observer = unsafe {
            center
                .get()
                .addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
        };
        // Watched for the life of the process, as the display watch is: the
        // registration is never removed, and its token is never released.
        std::mem::forget(observer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_of_different_sizes_match_by_shape() {
        let small = Table::identity().scaled(Tint {
            red: 1.0,
            green: 0.5,
            blue: 0.0,
        });
        let line: Vec<f32> = (0..1024).map(|index| index as f32 / 1023.0).collect();
        let large = Table {
            red: line.clone(),
            green: line.iter().map(|value| value * 0.5).collect(),
            blue: vec![0.0; 1024],
        };
        assert!(small.matches(&large));
        assert!(!small.matches(&Table::identity()));
    }

    /// Paints the real displays with a tint too faint to see, reads it back,
    /// and restores. Run by hand: `cargo test -- --ignored real_displays`.
    #[test]
    #[ignore = "changes the real displays' color tables for a moment"]
    fn real_displays_take_a_tint_and_give_it_back() {
        let mut displays = Displays::new();
        let faint = Tint {
            red: 1.0,
            green: 0.995,
            blue: 0.99,
        };
        let painted = displays.apply(faint);
        let intact = displays.intact();
        displays.restore();
        let display = online_displays().unwrap()[0];
        let back = Table::read(display).unwrap();
        assert!(
            painted.as_ref().is_ok_and(|count| *count > 0),
            "{painted:?}"
        );
        assert!(intact, "the table read back differs from the one set");
        assert!(
            back.matches(&displays.originals[&display]),
            "restore left the tint"
        );
        // The faint tint is inside the matching tolerance; check its top
        // blue entry, 1% down while tinted, is back exactly.
        let original = &displays.originals[&display];
        assert!(
            (back.blue[back.blue.len() - 1] - original.blue[original.blue.len() - 1]).abs() < 1e-4
        );
    }
}
