//! The window's egui side, headless, for the library's own tests: egui's pass
//! over `ui::draw` with its textures kept and its shapes tessellated, and no
//! renderer to take either. The integration tests and the benchmarks are
//! other crates, with their own copies (`tests/common/mod.rs`,
//! `benches/paths.rs`).

use egui::epaint::{ImageDelta, Mesh, Primitive};
use egui::{Pos2, Rect, TextureId};

use crate::app::{App, test_app};
use crate::color::Look;
use crate::model::Action;

/// A window before its first frame, at `scale` pixels to the point.
pub(crate) struct Headless {
    ctx: egui::Context,
    pub(crate) app: App,
    /// Pixels to the point of the display the window is on; the next frame
    /// uses it, as it does when the window moves to another display.
    pub(crate) scale: f32,
}

/// What one frame hands the renderer: the textures to set and free, and the
/// meshes to draw with the clip rect of each, back to front.
pub(crate) struct Frame {
    pub(crate) set: Vec<(TextureId, ImageDelta)>,
    pub(crate) free: Vec<TextureId>,
    pub(crate) meshes: Vec<(Rect, Mesh)>,
}

fn noon() -> f64 {
    12.0 * 60.0
}

impl Headless {
    pub(crate) fn new(scale: f32) -> Self {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        Self {
            ctx,
            app: test_app(noon),
            scale,
        }
    }

    /// The night look at `kelvin`, as the Temperature slider sets it.
    pub(crate) fn night(&mut self, kelvin: u16) {
        let look = Look {
            kelvin,
            ..self.app.settings.night
        };
        let action = Action::SetNight {
            look,
            dragging: false,
        };
        self.app.apply(action, None);
    }

    pub(crate) fn frame(&mut self) -> Frame {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, crate::theme::WINDOW)),
            ..Default::default()
        };
        let viewport = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        viewport.native_pixels_per_point = Some(self.scale);
        viewport.focused = Some(true);
        let app = &mut self.app;
        let mut output = self.ctx.run_ui(input, |ui| crate::ui::draw(app, ui));
        // A delta that is dropped with anything in it panics in debug
        // builds: take it apart.
        let textures = &mut output.textures_delta;
        let set = (std::mem::take(&mut textures.set).into_iter())
            .flat_map(|(id, deltas)| deltas.into_iter().map(move |delta| (id, delta)))
            .collect();
        let free = std::mem::take(&mut textures.free).into_iter().collect();
        let meshes = (self
            .ctx
            .tessellate(output.shapes, output.pixels_per_point)
            .into_iter())
        .filter_map(|clipped| match clipped.primitive {
            Primitive::Mesh(mesh) => Some((clipped.clip_rect, mesh)),
            Primitive::Callback(_) => None,
        })
        .collect();
        Frame { set, free, meshes }
    }
}

impl Frame {
    /// The textures the window set besides the font atlas, egui's first: in
    /// this window, the cover's.
    pub(crate) fn cover_textures(&self) -> Vec<&(TextureId, ImageDelta)> {
        let atlas = TextureId::default();
        self.set.iter().filter(|(id, _)| *id != atlas).collect()
    }

    /// The cover's meshes, in the order they are drawn: those that are not
    /// text.
    pub(crate) fn cover_meshes(&self) -> Vec<&(Rect, Mesh)> {
        let atlas = TextureId::default();
        (self.meshes.iter())
            .filter(|(_, mesh)| mesh.texture_id != atlas)
            .collect()
    }
}
