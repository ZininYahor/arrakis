//! Which painter a window gets, and what the app reads of a failure. A window gets the Metal painter
//! while the process has seen no failure and the Mac has a device for it, and eframe's GL painter
//! otherwise: GL is linked anyway, as `fastframe-shell` needs it. No code here touches a display.

/// The painter a window has, from what eframe gave the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowPainter {
    Metal,
    Glow,
}

impl WindowPainter {
    /// The Metal painter has no GL, and its `render_state` is `None`, so only `gl` tells the two apart.
    pub fn of(cc: &eframe::CreationContext<'_>) -> Self {
        if cc.gl.is_some() {
            Self::Glow
        } else {
            Self::Metal
        }
    }

    /// The name a drive's report gives it, which `scripts/painter-fallback.sh` reads:
    /// `tests/painter_fallback_script.rs` holds the two together.
    #[cfg(feature = "demo")]
    pub fn name(self) -> &'static str {
        match self {
            Self::Metal => "metal",
            Self::Glow => "glow",
        }
    }
}

/// A demo build's choice of painter.
#[cfg(all(feature = "demo", target_os = "macos"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum DemoRenderer {
    /// eframe's GL painter, the fallback and the pixels' reference.
    Glow,
    /// The Metal painter, which a window of the app gets on a Mac with a device for it, until it
    /// fails once.
    Metal,
}

/// `--demo-renderer`, set once before the first window.
#[cfg(all(feature = "demo", target_os = "macos"))]
pub(crate) static DEMO_RENDERER: std::sync::OnceLock<DemoRenderer> = std::sync::OnceLock::new();

/// Whether the next window can have the Metal painter. The flag goes first: once it is up no window
/// needs the look for a device.
#[cfg(target_os = "macos")]
fn metal_works() -> bool {
    !failed() && eframe::egui_wgpu::has_device()
}

/// The choice itself, which a shipped build and a demo run both make.
#[cfg(target_os = "macos")]
fn renderer_when(metal_works: impl FnOnce() -> bool) -> eframe::Renderer {
    if metal_works() {
        eframe::Renderer::Wgpu
    } else {
        eframe::Renderer::Glow
    }
}

/// Which painter the next window gets.
#[cfg(target_os = "macos")]
pub fn renderer() -> eframe::Renderer {
    #[cfg(feature = "demo")]
    {
        renderer_for(DEMO_RENDERER.get().copied(), metal_works)
    }
    #[cfg(not(feature = "demo"))]
    {
        renderer_when(metal_works)
    }
}

/// Which painter the next window gets: glow is the only one here.
#[cfg(not(target_os = "macos"))]
pub fn renderer() -> eframe::Renderer {
    eframe::Renderer::Glow
}

/// The painter for what `--demo-renderer` chose, on top of the app's own choice. Without the flag, the
/// app's. With `metal` it is the app's too, so a run that asked for it and cannot have it says so on
/// the `painter:` line, and glow's own choice looks for no device, which a measured glow window would
/// pay for.
#[cfg(all(feature = "demo", target_os = "macos"))]
pub(crate) fn renderer_for(
    chosen: Option<DemoRenderer>,
    metal_works: impl FnOnce() -> bool,
) -> eframe::Renderer {
    if chosen == Some(DemoRenderer::Glow) {
        eframe::Renderer::Glow
    } else {
        renderer_when(metal_works)
    }
}

/// Lets go of what a closed Metal window's last surfaces cost. Only with the window gone: this would
/// blank one still on the screen. With no such surface it does nothing and loads nothing.
#[cfg(target_os = "macos")]
pub fn empty_closed_surface() {
    eframe::egui_wgpu::empty_closed_surface();
}

/// There is no Metal painter here.
#[cfg(not(target_os = "macos"))]
pub fn empty_closed_surface() {}

/// Whether the Metal painter has failed in this process.
#[cfg(target_os = "macos")]
pub fn failed() -> bool {
    eframe::egui_wgpu::failure().is_some()
}

/// Whether the Metal painter has failed in this process: there is none here.
#[cfg(not(target_os = "macos"))]
pub fn failed() -> bool {
    false
}

/// Why the painter failed, and when, for a drive's report.
#[cfg(all(feature = "demo", target_os = "macos"))]
pub fn failure() -> Option<(String, std::time::Instant)> {
    eframe::egui_wgpu::failure().map(|failure| (failure.why.clone(), failure.at))
}

/// Why the painter failed, and when: there is none here.
#[cfg(all(feature = "demo", not(target_os = "macos")))]
pub fn failure() -> Option<(String, std::time::Instant)> {
    None
}

/// Where a window's shader came from, for a drive's report: `file` (`shader.metallib`) or `source`, as
/// the Metal painter last said, and none on glow, which has a shader of its own.
#[cfg(all(feature = "demo", target_os = "macos"))]
pub fn shader_from(painter: WindowPainter) -> Option<&'static str> {
    match painter {
        WindowPainter::Metal => eframe::egui_wgpu::shader_from(),
        WindowPainter::Glow => None,
    }
}

/// There is no Metal painter here.
#[cfg(all(feature = "demo", not(target_os = "macos")))]
pub fn shader_from(_painter: WindowPainter) -> Option<&'static str> {
    None
}

/// Whether this run fails a frame that comes after a rest (`ARRAKIS_METAL_FAIL=rest`), for which a
/// drive must leave its window at rest.
#[cfg(all(feature = "demo", target_os = "macos"))]
pub fn fails_at_rest() -> bool {
    eframe::egui_wgpu::fail_at() == Some(eframe::egui_wgpu::FailAt::Rest)
}

/// No painter fails here.
#[cfg(all(feature = "demo", not(target_os = "macos")))]
pub fn fails_at_rest() -> bool {
    false
}

/// How long after the one before a frame counts as coming from rest, which is the painter's own word
/// on it: the `rest` drive leaves its window so for the painter to fail that frame.
#[cfg(all(feature = "demo", target_os = "macos"))]
pub const AT_REST: std::time::Duration = eframe::egui_wgpu::REST;

/// No painter fails here, so no drive leaves its window at rest.
#[cfg(all(feature = "demo", not(target_os = "macos")))]
pub const AT_REST: std::time::Duration = std::time::Duration::ZERO;

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// The choice in every build, a shipped one too: `demo` only stands in front of it.
    #[test]
    fn the_next_window_gets_metal_when_it_works_and_glow_when_it_does_not() {
        assert_eq!(renderer_when(|| true), eframe::Renderer::Wgpu);
        assert_eq!(renderer_when(|| false), eframe::Renderer::Glow);
    }

    /// Only `entrypoint::run` sets `DEMO_RENDERER`, and this binary never runs it: setting it here
    /// would leak into every other test.
    #[test]
    fn a_window_gets_the_wgpu_painter_when_no_demo_run_chose_one() {
        #[cfg(feature = "demo")]
        assert_eq!(DEMO_RENDERER.get(), None);
        if eframe::egui_wgpu::has_device() {
            assert!(matches!(renderer(), eframe::Renderer::Wgpu));
        } else {
            eprintln!("skipped: no Metal device here, so a window gets glow");
            assert!(matches!(renderer(), eframe::Renderer::Glow));
        }
    }

    #[cfg(feature = "demo")]
    #[test]
    fn the_flag_chooses_the_painter_and_without_it_a_window_gets_metal() {
        let works = || true;
        assert_eq!(renderer_for(None, works), eframe::Renderer::Wgpu);
        assert_eq!(
            renderer_for(Some(DemoRenderer::Metal), works),
            eframe::Renderer::Wgpu
        );
        assert_eq!(
            renderer_for(Some(DemoRenderer::Glow), works),
            eframe::Renderer::Glow
        );
    }

    #[cfg(feature = "demo")]
    #[test]
    fn with_no_device_or_a_flag_up_every_choice_but_none_is_glow() {
        // `metal_works` says no: no device, or the flag is up. `--demo-renderer metal` cannot have
        // Metal then, and the `painter:` line says glow drew.
        let no = || false;
        assert_eq!(renderer_for(None, no), eframe::Renderer::Glow);
        assert_eq!(
            renderer_for(Some(DemoRenderer::Metal), no),
            eframe::Renderer::Glow
        );
        assert_eq!(
            renderer_for(Some(DemoRenderer::Glow), no),
            eframe::Renderer::Glow
        );
    }

    /// Glow compiles a shader of its own, so whatever the Metal painter last said is not a glow
    /// window's.
    #[cfg(feature = "demo")]
    #[test]
    fn a_glow_window_has_no_shader_from_the_metal_painter() {
        assert_eq!(shader_from(WindowPainter::Glow), None);
    }

    #[cfg(feature = "demo")]
    #[test]
    fn a_measured_glow_window_pays_for_no_look_for_a_device() {
        use std::cell::Cell;

        let looks = Cell::new(0);
        let look = || {
            looks.set(looks.get() + 1);
            true
        };
        assert_eq!(
            renderer_for(Some(DemoRenderer::Glow), look),
            eframe::Renderer::Glow
        );
        assert_eq!(looks.get(), 0, "glow asked for a device");
        // The look is the only thing that tells these two from glow's: it is made, once.
        assert_eq!(renderer_for(None, look), eframe::Renderer::Wgpu);
        assert_eq!(looks.get(), 1);
        assert_eq!(
            renderer_for(Some(DemoRenderer::Metal), look),
            eframe::Renderer::Wgpu
        );
        assert_eq!(looks.get(), 2);
    }
}
