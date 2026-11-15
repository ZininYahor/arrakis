//! A window's egui side, headless, at 2 pixels per point unless its `scale`
//! says otherwise: egui's pass over `ui::draw` and the actions it pushed, as
//! eframe runs them before painting, without GL. The same pattern as the
//! benchmarks' `frame::Window`.

use std::time::Instant;

use arrakis::app::App;
use arrakis::{probe, theme, ui};
use egui::epaint::{ImageData, Primitive};
use egui::{ColorImage, Event, Pos2, RawInput, Rect, TextureId, ViewportId};

/// The bytes of the cover's six images at `size` pixels square, as a
/// window's first frame sends them to the GPU: four bytes to a pixel. The
/// rim's light and the shade at the sand are each the same box at every
/// rise, so their bytes are too.
#[allow(dead_code)] // Some test binaries count no bytes.
pub fn cover_bytes(size: usize) -> u64 {
    let night = arrakis::settings::Settings::default().night.kelvin;
    let images = probe::cover_images(probe::cover_layers(size), night);
    images
        .iter()
        .map(|image| 4 * image.pixels.len() as u64)
        .sum()
}

/// The bytes of the rim's light's whole box at `size` pixels square, four to
/// a pixel: what a window's first frame sends of it, and the most that a
/// frame at a new rise sends of it.
#[allow(dead_code)] // Some test binaries count no bytes.
pub fn rim_bytes(size: usize) -> u64 {
    let night = arrakis::settings::Settings::default().night.kelvin;
    let [.., rim, _] = probe::cover_images(probe::cover_layers(size), night);
    4 * rim.pixels.len() as u64
}

/// The bytes of the shade's whole box at `size` pixels square, four to a
/// pixel: what a window's first frame sends of it, and the most that a frame
/// at a new rise sends of it.
#[allow(dead_code)] // Some test binaries count no bytes.
pub fn shade_bytes(size: usize) -> u64 {
    let night = arrakis::settings::Settings::default().night.kelvin;
    let [.., shade] = probe::cover_images(probe::cover_layers(size), night);
    4 * shade.pixels.len() as u64
}

pub struct Window {
    ctx: egui::Context,
    pub app: App,
    /// Pixels to the point of the display the window is on; the next frame
    /// uses it, as it does when the window moves to another display.
    pub scale: f32,
    started: Instant,
}

impl Window {
    /// A window before its first frame, for `app`, on a 2x display.
    pub fn new(app: App) -> Self {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        Self {
            ctx,
            app,
            scale: 2.0,
            started: Instant::now(),
        }
    }

    /// One frame, with `events` as its input.
    #[allow(dead_code)] // Some test binaries look at the textures instead.
    pub fn frame(&mut self, events: Vec<Event>) {
        self.frame_textures(events);
    }

    /// One frame, and the images it sets for the renderer besides egui's
    /// font atlas: the cover's, in the order they are loaded. A window's
    /// first frame sets six; a frame at a new rise sets the rim's light, and
    /// the shade where the rise moves it, each as the rows from the first
    /// that holds something in the old picture or the new to the last.
    pub fn frame_textures(&mut self, events: Vec<Event>) -> Vec<ColorImage> {
        let mut output = self.run(events);
        // No renderer here to take the textures: copy the cover's, and drop
        // the rest.
        let atlas = TextureId::default();
        let mut textures: Vec<_> = (output.textures_delta.set.iter())
            .filter(|(id, _)| **id != atlas)
            .flat_map(|(id, deltas)| deltas.iter().map(move |delta| (*id, delta)))
            .map(|(id, delta)| match &delta.image {
                ImageData::Color(image) => (id, ColorImage::clone(image)),
            })
            .collect();
        output.textures_delta.clear();
        // The set is a map. The ids count up from the order the textures were
        // loaded in.
        textures.sort_by_key(|(id, _)| *id);
        textures.into_iter().map(|(_, image)| image).collect()
    }

    /// One frame; where it draws the cover's maw: the top of the maw's rect
    /// in points, which moves with each rise, or None where the frame draws
    /// no maw (the day look draws the back and the front alone); and the
    /// sizes of the images it sets besides egui's font atlas, one for each
    /// set, in no order. The cover's meshes come back to front: the back,
    /// the maw, the rim's light, the shade, the gap and the front.
    #[allow(dead_code)] // Only the cover's counts look at the sets.
    pub fn frame_sets(&mut self, events: Vec<Event>) -> (Option<f32>, Vec<[usize; 2]>) {
        let mut output = self.run(events);
        let atlas = TextureId::default();
        let sets = (output.textures_delta.set.iter())
            .filter(|(id, _)| **id != atlas)
            .flat_map(|(_, deltas)| deltas.iter())
            .map(|delta| delta.image.size())
            .collect();
        output.textures_delta.clear();
        let meshes: Vec<_> = (self.ctx)
            .tessellate(output.shapes, output.pixels_per_point)
            .into_iter()
            .filter_map(|clipped| match clipped.primitive {
                Primitive::Mesh(mesh) if mesh.texture_id != atlas => Some(mesh),
                _ => None,
            })
            .collect();
        let maw = (meshes.len() == 6).then(|| {
            (meshes[1].vertices.iter())
                .map(|vertex| vertex.pos.y)
                .fold(f32::INFINITY, f32::min)
        });
        (maw, sets)
    }

    /// The egui pass over `ui::draw` and the actions it pushed, as eframe
    /// runs them before painting.
    fn run(&mut self, events: Vec<Event>) -> egui::FullOutput {
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, theme::WINDOW)),
            time: Some(self.started.elapsed().as_secs_f64()),
            events,
            ..Default::default()
        };
        let viewport = input.viewports.entry(ViewportId::ROOT).or_default();
        viewport.native_pixels_per_point = Some(self.scale);
        viewport.focused = Some(true);
        let app = &mut self.app;
        self.ctx.run_ui(input, |ui| {
            ui::draw(app, ui);
            app.apply_actions(Some(ui.ctx()));
        })
    }
}
