//! The names eframe's wgpu integration needs, with no wgpu behind them: [`winit::Painter`] draws
//! egui's meshes through Metal into BGRA IOSurfaces that a sublayer of the window's view shows, and
//! [`hold`] paces a drag at the display's refresh, as a swap does for GL.
//!
//! The patch in the root's `Cargo.toml` puts this crate in the real `egui-wgpu`'s place. On every
//! other target it compiles to nothing, with no dependency, so eframe's `Renderer::Wgpu` stays
//! macOS's.

#![cfg(target_os = "macos")]
// No panic of our own outside tests: under `panic = "abort"` one ends the process, with the menu bar and
// the schedule. `clippy.toml` holds what these lints cannot see.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::indexing_slicing,
        clippy::string_slice,
    )
)]
#![cfg_attr(test, allow(clippy::disallowed_macros, clippy::disallowed_methods))]

use std::fmt;

mod failure;
mod painter;
// Written by `scripts/build-metallib.sh`. The build reads the length, the tests the hashes and the
// target, and only a review reads the compiler's line: each constant that a build leaves unread says
// so, on itself.
mod shader_stamp;

pub mod hold;

#[cfg(feature = "demo")]
pub use failure::{FailAt, fail_at, fail_detached};
pub use failure::{Failure, failure, has_device};
pub use painter::{METALLIB_REFUSED, empty_closed_surface, shader_from};
#[cfg(feature = "demo")]
pub use painter::{REST, park_a_surface_for_a_test};

/// What eframe asks for by the path `egui_wgpu::winit::Painter`.
pub mod winit {
    pub use crate::painter::Painter;
}

/// The real crate has no such constant, and the app names it. So when eframe moves on and cargo drops
/// the patch, with only a warning, the build stops instead of bringing wgpu in.
pub const METAL_PAINTER: bool = true;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SurfaceConfig;

impl SurfaceConfig {
    pub const LOW_LATENCY: Self = Self;
}

pub mod wgpu {}

#[derive(Default)]
pub struct CreateNew {
    pub display_handle: Option<Box<dyn std::any::Any>>,
}

impl Clone for CreateNew {
    fn clone(&self) -> Self {
        Self::default()
    }
}

#[derive(Clone)]
pub enum WgpuSetup {
    CreateNew(CreateNew),
}

impl Default for WgpuSetup {
    fn default() -> Self {
        Self::CreateNew(CreateNew::default())
    }
}

#[derive(Clone, Default)]
pub struct WgpuConfiguration {
    pub surface: SurfaceConfig,
    pub wgpu_setup: WgpuSetup,
}

impl WgpuConfiguration {
    #[must_use]
    pub fn with_surface_config(mut self, surface: SurfaceConfig) -> Self {
        self.surface = surface;
        self
    }
}

#[derive(Clone)]
pub struct RenderState {
    pub surface_config: SurfaceConfig,
}

/// No value exists: the painter returns no error to eframe.
#[derive(Debug)]
pub enum WgpuError {}

impl fmt::Display for WgpuError {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

impl std::error::Error for WgpuError {}

#[derive(Clone, Default)]
pub struct RendererOptions {
    pub msaa_samples: u32,
    pub depth_stencil_format: Option<()>,
    pub dithering: bool,
}

pub fn depth_format_from_bits(_depth: u8, _stencil: u8) -> Option<()> {
    None
}
