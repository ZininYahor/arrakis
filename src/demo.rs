//! Pictures for design review and the README: fixed scenes, a clock the
//! scene moves by hand, no real displays touched. Built only with the
//! `demo` feature.
//!
//! macOS takes screenshots before the gamma table, so a capture never shows
//! the tint. A still of the tinted window is saved twice: as drawn, and as
//! the panel would show it under the live tint, to check the window stays
//! legible through its own red. A film tints each frame with the tint that
//! was live when it was captured.
//!
//! egui's capture holds only what egui draws, so every picture of the window
//! gets the traffic lights stamped in and the window's corners rounded, as
//! AppKit draws them. A plate is a picture of its own in a window of its own:
//! the Scale plate, left square, and the README's banner and its plate of
//! figures, their corners rounded as the window's.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use egui::{
    Align2, Color32, ColorImage, Galley, Id, Painter, Pos2, Rect, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};

use crate::app::App;
use crate::color::{self, Look, PURE_RED_KELVIN, Tint};
use crate::model::Action;
use crate::schedule::ClockTime;
use crate::settings::Settings;
use crate::theme::{self, Palette};
use crate::ui::{self, cover, paint, view};

/// A fixed situation to photograph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Scene {
    /// Mid-afternoon with the defaults: normal colors, red at 21:00.
    Day,
    /// Half past eleven at night, the default 1200 K look on.
    Night,
    /// Half past eleven at night, as `Night`, with the Temperature slider
    /// focused: its ringed knob and focus ring show.
    Focused,
    /// Half past eleven at night, as `Night`, with the night look on the
    /// Desert stop: its tick and name lit, the name midway between its
    /// neighbors', and the readout saying 1760 K.
    Desert,
    /// Pure red at 60% brightness, forced on with Red Now.
    PureRed,
    /// The schedule switched off.
    Off,
    /// One evening as an animated PNG: the 21:00 fade, the Temperature
    /// slider dragged to pure red, and the 07:00 fade back.
    Evening,
    /// The Temperature scale, stretched, under the ranges Night Shift and
    /// this app reach.
    Scale,
    /// The README's banner: the cover, wide, as the window shows it at the
    /// default settings, under the app's name and its line.
    Banner,
    /// The README's plate of figures: three readouts and three strips of
    /// before and after, drawn with the window's track and palette.
    Bench,
}

/// The scene's clock, in minutes since midnight, as `f64` bits.
static MINUTE: AtomicU64 = AtomicU64::new(0);

/// The scene's clock, for the engine and the status line.
pub fn clock() -> f64 {
    f64::from_bits(MINUTE.load(Ordering::Relaxed))
}

fn set_clock(minute: f64) {
    MINUTE.store(minute.to_bits(), Ordering::Relaxed);
}

fn at(hour: u16, minute: u16) -> f64 {
    f64::from(ClockTime::new(hour, minute).minutes())
}

/// One picture: where the clock stands, the night look if it changes, and
/// how long a film shows the frame.
#[derive(Clone, Copy, Debug)]
struct Moment {
    minute: f64,
    night: Option<Look>,
    hold_ms: u16,
}

impl Moment {
    fn at(minute: f64, hold_ms: u16) -> Self {
        Self {
            minute,
            night: None,
            hold_ms,
        }
    }
}

impl Scene {
    /// Settings, and whether Red Now is on, for a scene of the window.
    pub fn set_up(self) -> (Settings, bool) {
        let mut settings = Settings::default();
        let red_now = match self {
            Scene::PureRed => {
                settings.night = Look {
                    kelvin: PURE_RED_KELVIN,
                    brightness: 60,
                };
                true
            }
            Scene::Desert => {
                settings.night = Look {
                    kelvin: ui::DESERT_KELVIN,
                    ..Look::default()
                };
                false
            }
            Scene::Off => {
                settings.schedule_on = false;
                false
            }
            _ => false,
        };
        set_clock(self.moments()[0].minute);
        (settings, red_now)
    }

    /// The plate the scene is, when it is a picture of its own rather than
    /// the window.
    pub fn plate(self) -> Option<PlateKind> {
        match self {
            Scene::Scale => Some(SCALE_PLATE),
            Scene::Banner => Some(BANNER_PLATE),
            Scene::Bench => Some(BENCH_PLATE),
            Scene::Day
            | Scene::Night
            | Scene::Focused
            | Scene::Desert
            | Scene::PureRed
            | Scene::Off
            | Scene::Evening => None,
        }
    }

    fn moments(self) -> Vec<Moment> {
        match self {
            Scene::Day | Scene::PureRed | Scene::Scale | Scene::Banner | Scene::Bench => {
                vec![Moment::at(at(14, 20), 0)]
            }
            Scene::Night | Scene::Focused | Scene::Desert => vec![Moment::at(at(23, 30), 0)],
            Scene::Off => vec![Moment::at(at(22, 0), 0)],
            Scene::Evening => evening(),
        }
    }
}

/// The film's script, on the default settings: normal at 20:55, the
/// 20-minute fade from 21:00, the slider dragged from 1200 K to pure red,
/// the fade back from 07:00, and normal again. About ten seconds.
fn evening() -> Vec<Moment> {
    const HOLD: u16 = 1400;
    const FADE_FRAME: u16 = 160;
    const DRAG_FRAME: u16 = 140;
    const DRAG_STEPS: u16 = 6;

    let mut moments = vec![Moment::at(at(20, 55), HOLD)];
    moments.extend((1..=10).map(|step| Moment::at(at(21, 2 * step), FADE_FRAME)));
    moments.push(Moment::at(at(21, 45), HOLD));
    let from = color::redness_of(Look::default().kelvin);
    moments.extend((1..=DRAG_STEPS).map(|step| {
        let redness = from + (1.0 - from) * f32::from(step) / f32::from(DRAG_STEPS);
        Moment {
            minute: at(21, 46),
            night: Some(Look {
                kelvin: color::kelvin_at_redness(redness),
                ..Look::default()
            }),
            hold_ms: if step == DRAG_STEPS { 1800 } else { DRAG_FRAME },
        }
    }));
    moments.extend((1..=10).map(|step| Moment::at(at(7, 2 * step), FADE_FRAME)));
    moments.push(Moment::at(at(7, 30), HOLD));
    moments
}

/// Long enough for the window to open, take focus and upload its fonts.
const FIRST_SETTLE_SECONDS: f64 = 2.0;
/// Long enough for the engine's quick fade after the clock moves.
const SETTLE_SECONDS: f64 = 0.8;
/// The screen around the window in a film.
pub(crate) const SCREEN: Vec2 = vec2(800.0, 680.0);
/// The desktop behind the window in a film, an evening sky from top to
/// bottom. Only the pictures show it.
const DESKTOP: (Color32, Color32) = (
    Color32::from_rgb(0x2E, 0x3C, 0x5E),
    Color32::from_rgb(0x8E, 0x9C, 0xB8),
);
/// The corners AppKit rounds a window to, in points.
const CORNER: f32 = 12.0;
/// The Scale plate, in points.
pub const PLATE: Vec2 = vec2(800.0, 184.0);
/// The banner, in points: the cover's square at its right is as tall.
pub const BANNER: Vec2 = vec2(800.0, 300.0);
/// The plate of figures, in points: as wide as the banner, as tall as the mock
/// the owner picked (2026-11-15).
pub const BENCH: Vec2 = vec2(800.0, 338.0);

/// A scene that is a picture of its own rather than the window: the size of
/// its window, in points, how it is drawn, and whether its shot rounds the
/// corners as the window's are. A plate's window has no title bar, so no shot
/// of one stamps the traffic lights.
#[derive(Clone, Copy)]
pub struct PlateKind {
    pub size: Vec2,
    draw: fn(&Ui, &Palette),
    rounded: bool,
}

const SCALE_PLATE: PlateKind = PlateKind {
    size: PLATE,
    draw: scale_plate,
    rounded: false,
};

/// Rounded, so that the picture sits on GitHub's white page and its dark one
/// alike, as the window's shots do.
const BANNER_PLATE: PlateKind = PlateKind {
    size: BANNER,
    draw: banner_plate,
    rounded: true,
};

/// Rounded as the banner is: the two stand one above the other in the README.
const BENCH_PLATE: PlateKind = PlateKind {
    size: BENCH,
    draw: bench_plate,
    rounded: true,
};

struct Frame {
    image: Arc<ColorImage>,
    tint: Tint,
    hold_ms: u16,
}

/// Plays a scene's moments, one screenshot each, then saves them.
pub struct Shot {
    path: PathBuf,
    scene: Scene,
    moments: Vec<Moment>,
    /// The moment being set up or photographed.
    next: usize,
    /// When it began, in egui time.
    since: Option<f64>,
    requested: bool,
    frames: Vec<Frame>,
    /// Pixels per point of the captures.
    scale: f32,
}

impl Shot {
    pub fn new(path: PathBuf, scene: Scene) -> Self {
        Self {
            path,
            scene,
            moments: scene.moments(),
            next: 0,
            since: None,
            requested: false,
            frames: Vec::new(),
            scale: 1.0,
        }
    }

    /// Call once per frame, with the app when the scene is the window.
    /// Returns true once every picture is saved.
    pub fn frame(&mut self, ctx: &egui::Context, mut app: Option<&mut App>) -> bool {
        if self.next == self.moments.len() {
            // Saved already; the window is on its way out.
            return true;
        }
        let now = ctx.input(|input| input.time);
        let image = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image
            && self.requested
        {
            self.scale = ctx.pixels_per_point();
            self.frames.push(Frame {
                image,
                tint: app.as_ref().map_or(Tint::NEUTRAL, |app| app.status.tint),
                hold_ms: self.moments[self.next].hold_ms,
            });
            self.next += 1;
            self.since = None;
            self.requested = false;
            if self.next == self.moments.len() {
                if let Err(error) = self.save() {
                    log::error!("unable to save {}: {error}", self.path.display());
                }
                return true;
            }
        }

        let since = *self.since.get_or_insert_with(|| {
            let moment = self.moments[self.next];
            if self.next == 0 {
                // Lit traffic lights, as the window looks in use.
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            set_clock(moment.minute);
            if let Some(app) = app.as_mut() {
                if let Some(look) = moment.night {
                    app.apply(
                        Action::SetNight {
                            look,
                            dragging: false,
                        },
                        Some(ctx),
                    );
                }
                app.clock_moved();
            }
            now
        });
        if self.scene == Scene::Focused {
            // Every frame: the window's own focus handling may drop it while
            // the window opens.
            ctx.memory_mut(|memory| memory.request_focus(Id::new(view::TEMPERATURE)));
        }
        let settle = if self.next == 0 {
            FIRST_SETTLE_SECONDS
        } else {
            SETTLE_SECONDS
        };
        if !self.requested && now - since >= settle {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        ctx.request_repaint();
        false
    }

    fn save(&self) -> Result<(), String> {
        if self.scene == Scene::Evening {
            return self.save_film();
        }
        let frame = &self.frames[0];
        let image = self.framed(&frame.image);
        let [width, height] = image.size;
        write_png(&self.path, width, height, &rgba(&image, Tint::NEUTRAL))?;
        if !frame.tint.is_neutral() {
            let stem = self
                .path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("shot");
            let twin = self.path.with_file_name(format!("{stem}-tinted.png"));
            write_png(&twin, width, height, &rgba(&image, frame.tint))?;
        }
        Ok(())
    }

    /// The frames as an animated PNG, each holding for its moment's time,
    /// looping forever.
    fn save_film(&self) -> Result<(), String> {
        let text = |error: png::EncodingError| error.to_string();
        let size = [
            (SCREEN.x * self.scale).round() as usize,
            (SCREEN.y * self.scale).round() as usize,
        ];
        let file = File::create(&self.path).map_err(|error| error.to_string())?;
        let mut encoder = png::Encoder::new(BufWriter::new(file), size[0] as u32, size[1] as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::High);
        encoder
            .set_animated(self.frames.len() as u32, 0)
            .map_err(text)?;
        let mut writer = encoder.write_header().map_err(text)?;
        for frame in &self.frames {
            writer.set_frame_delay(frame.hold_ms, 1000).map_err(text)?;
            writer
                .write_image_data(&on_screen(&self.framed(&frame.image), frame.tint, size))
                .map_err(text)?;
        }
        writer.finish().map_err(text)?;
        log::info!(
            "saved {} frames to {}",
            self.frames.len(),
            self.path.display()
        );
        Ok(())
    }

    /// A capture of the window as AppKit shows it: the traffic lights in,
    /// the corners rounded off. A plate is left as drawn, its corners rounded
    /// as the window's if it asks for that.
    fn framed(&self, image: &ColorImage) -> ColorImage {
        let mut image = image.clone();
        match self.scene.plate() {
            None => {
                stamp_traffic_lights(&mut image, self.scale);
                round_corners(&mut image, CORNER * self.scale);
            }
            Some(plate) if plate.rounded => round_corners(&mut image, CORNER * self.scale),
            Some(_) => {}
        }
        image
    }
}

/// The close, minimize and zoom buttons where `fastframe_titlebar` keeps
/// them: 12-point circles from 16 points in, 20 apart, centered in the
/// title bar. The window is not resizable, so zoom is gray.
fn stamp_traffic_lights(image: &mut ColorImage, scale: f32) {
    const LIGHTS: [(u8, u8, u8); 3] = [(0xFF, 0x5F, 0x57), (0xFE, 0xBC, 0x2E), (0x8C, 0x8C, 0x8C)];
    let radius = 6.0 * scale;
    let [width, height] = image.size;
    for (index, (red, green, blue)) in LIGHTS.into_iter().enumerate() {
        let center = (
            (16.0 + 6.0 + 20.0 * index as f32) * scale,
            theme::TITLE_BAR / 2.0 * scale,
        );
        let light = Color32::from_rgb(red, green, blue);
        let reach = radius.ceil() as usize + 1;
        let (cx, cy) = (center.0 as usize, center.1 as usize);
        for y in cy.saturating_sub(reach)..(cy + reach).min(height) {
            for x in cx.saturating_sub(reach)..(cx + reach).min(width) {
                let dx = x as f32 + 0.5 - center.0;
                let dy = y as f32 + 0.5 - center.1;
                // A pixel's worth of soft edge, as AppKit draws them.
                let cover = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                if cover > 0.0 {
                    let pixel = &mut image.pixels[y * width + x];
                    *pixel = lerp(*pixel, light, cover);
                }
            }
        }
    }
}

/// Clears the pixels outside a rounding of `radius` pixels at each corner,
/// with a pixel of soft edge.
fn round_corners(image: &mut ColorImage, radius: f32) {
    let [width, height] = image.size;
    let reach = radius.ceil() as usize;
    for y in 0..reach.min(height) {
        for x in 0..reach.min(width) {
            let dx = radius - (x as f32 + 0.5);
            let dy = radius - (y as f32 + 0.5);
            let cover = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            if cover < 1.0 {
                for (cx, cy) in [
                    (x, y),
                    (width - 1 - x, y),
                    (x, height - 1 - y),
                    (width - 1 - x, height - 1 - y),
                ] {
                    let pixel = &mut image.pixels[cy * width + cx];
                    *pixel = pixel.gamma_multiply(cover);
                }
            }
        }
    }
}

fn lerp(from: Color32, to: Color32, t: f32) -> Color32 {
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color32::from_rgb(
        mix(from.r(), to.r()),
        mix(from.g(), to.g()),
        mix(from.b(), to.b()),
    )
}

/// The image through `tint`, as RGBA with straight alpha.
fn rgba(image: &ColorImage, tint: Tint) -> Vec<u8> {
    image
        .pixels
        .iter()
        .flat_map(|pixel| {
            let [red, green, blue, alpha] = pixel.to_srgba_unmultiplied();
            let [red, green, blue, _] = tint.apply(Color32::from_rgb(red, green, blue)).to_array();
            [red, green, blue, alpha]
        })
        .collect()
}

/// `pixel`, whose color is premultiplied by its alpha, laid over `ground`.
fn over(pixel: Color32, ground: Color32) -> Color32 {
    let clear = 1.0 - f32::from(pixel.a()) / 255.0;
    let mix = |top: u8, under: u8| (f32::from(top) + f32::from(under) * clear).round() as u8;
    Color32::from_rgb(
        mix(pixel.r(), ground.r()),
        mix(pixel.g(), ground.g()),
        mix(pixel.b(), ground.b()),
    )
}

/// The window centered on an evening desktop of `size` pixels, with its
/// shadow under it, all of it seen through `tint`, as RGB.
fn on_screen(window: &ColorImage, tint: Tint, size: [usize; 2]) -> Vec<u8> {
    let [width, height] = size;
    let [window_width, window_height] = window.size;
    let left = width.saturating_sub(window_width) / 2;
    let top = height.saturating_sub(window_height) / 2;
    // The shadow falls a little below the window and fades out softly.
    let scale = window_width as f32 / theme::WINDOW.x;
    let (drop, spread) = (12.0 * scale, 20.0 * scale);
    let half = vec2(window_width as f32, window_height as f32) / 2.0 - vec2(4.0, 4.0) * scale;
    let center = pos2(left as f32, top as f32 + drop) + half + vec2(4.0, 4.0) * scale;
    let radius = CORNER * scale;
    let shade_at = |point: egui::Pos2| {
        // How far `point` is outside the shadow's rounded rectangle.
        let q = (point - center).abs() - (half - Vec2::splat(radius));
        let outside = q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - radius;
        0.55 * (-(outside.max(0.0) / spread).powi(2)).exp()
    };
    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        let sky = lerp(DESKTOP.0, DESKTOP.1, y as f32 / height as f32);
        for x in 0..width {
            let point = pos2(x as f32 + 0.5, y as f32 + 0.5);
            let mut color = lerp(sky, Color32::BLACK, shade_at(point));
            if (left..left + window_width).contains(&x) && (top..top + window_height).contains(&y) {
                color = over(window.pixels[(y - top) * window_width + (x - left)], color);
            }
            let color = tint.apply(color);
            rgb.extend_from_slice(&[color.r(), color.g(), color.b()]);
        }
    }
    rgb
}

fn write_png(path: &Path, width: usize, height: usize, rgba: &[u8]) -> Result<(), String> {
    let file = File::create(path).map_err(|error| error.to_string())?;
    encode_png(BufWriter::new(file), width, height, rgba)?;
    log::info!("saved {}", path.display());
    Ok(())
}

/// `rgba`, `width` by `height` pixels with straight alpha, as a PNG written
/// to `out`: how the demo build's still pictures and the app's icon are saved.
/// Every error reaches the caller, the last chunk's and `out`'s flush too: a
/// drop would lose them, and the picture would be reported as saved.
pub(crate) fn encode_png(
    out: impl Write,
    width: usize,
    height: usize,
    rgba: &[u8],
) -> Result<(), String> {
    let mut encoder = png::Encoder::new(out, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    writer
        .write_image_data(rgba)
        .map_err(|error| error.to_string())?;
    // Writes the IEND chunk and flushes `out`, a buffer included.
    writer.finish().map_err(|error| error.to_string())
}

/// A window that draws one plate, its shot's scene, and photographs it.
pub struct Plate {
    shot: Shot,
    /// How the plate is drawn: its scene's [`PlateKind`]'s.
    draw: fn(&Ui, &Palette),
    /// Whether the Metal painter has failed. A plate has no `App`, so nothing
    /// else reads that, and a painter that paints nothing brings no screenshot:
    /// the shot would ask for frames for ever. A unit test gives this a flag
    /// of its own.
    painter_failed: fn() -> bool,
}

impl Plate {
    /// The window of `shot`'s scene, which is the plate `kind`: what
    /// `Scene::plate` says, which a caller has read already for the window's
    /// size.
    pub fn new(shot: Shot, kind: PlateKind) -> Self {
        Self {
            draw: kind.draw,
            shot,
            painter_failed: crate::painter::failed,
        }
    }

    /// Closes the window once the picture is saved, or once none can come:
    /// `entrypoint::plate_outcome` then ends the run with an error.
    fn close_when_done(&mut self, ctx: &egui::Context) {
        if (self.painter_failed)() || self.shot.frame(ctx, None) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for Plate {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.close_when_done(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        (self.draw)(ui, &Palette::DARK);
    }

    /// The window's cap on the texture side (`ui::cap_texture_side`), so that
    /// this plate's atlas is made at the cap too.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        ui::cap_texture_side(raw_input);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Palette::DARK.window.to_normalized_gamma_f32()
    }
}

/// About where Night Shift's warmest setting sits.
const NIGHT_SHIFT_KELVIN: u16 = 2700;

/// The Temperature slider's scale, stretched: what each position does to
/// white, under how far Night Shift and this app go.
fn scale_plate(ui: &Ui, palette: &Palette) {
    let painter = ui.painter();
    let plate = Rect::from_min_size(pos2(0.0, 0.0), PLATE);
    painter.rect_filled(plate, 0, palette.window);
    paint::text(
        painter,
        pos2(48.0, 30.0),
        Align2::LEFT_CENTER,
        "Temperature",
        theme::bold(16.0),
        palette.text,
    );

    let well = Rect::from_min_max(pos2(48.0, 104.0), pos2(PLATE.x - 48.0, 136.0));
    let x_at = |kelvin: u16| (well.min.x + well.width() * color::redness_of(kelvin)).round();

    bracket(
        ui,
        well.min.x,
        x_at(NIGHT_SHIFT_KELVIN),
        60.0,
        "Night Shift: to about 2700 K",
        palette,
    );
    bracket(
        ui,
        well.min.x,
        well.max.x,
        84.0,
        "Arrakis: to pure red",
        palette,
    );

    // What white becomes at each position, a point at a time. A fade to
    // pure red passes through every redness, between the table's 10 K
    // steps too.
    let mut x = well.min.x;
    while x < well.max.x {
        let redness = (x + 0.5 - well.min.x) / well.width();
        painter.rect_filled(
            Rect::from_min_max(pos2(x, well.min.y), pos2(x + 1.0, well.max.y)),
            0,
            color::scale_tint(redness).light(),
        );
        x += 1.0;
    }
    // The window's ground over the strip's corners rounds them off.
    painter.rect_stroke(
        well.expand(4.0),
        10,
        Stroke::new(4.0, palette.window),
        StrokeKind::Inside,
    );

    let tick_top = well.max.y + 6.0;
    for kelvin in [6500, 5000, 4000, 3000, 2000, 1500, 1000] {
        painter.rect_filled(
            Rect::from_min_size(pos2(x_at(kelvin), tick_top), vec2(1.0, 4.0)),
            0,
            palette.dim,
        );
    }
    let label_y = tick_top + 16.0;
    for (kelvin, label) in [
        (6500, "6500 K"),
        (NIGHT_SHIFT_KELVIN, "2700 K"),
        (1200, "1200 K, the default"),
    ] {
        painter.rect_filled(
            Rect::from_min_size(pos2(x_at(kelvin), tick_top), vec2(1.0, 7.0)),
            0,
            palette.secondary,
        );
        paint::text(
            painter,
            pos2(x_at(kelvin), label_y),
            Align2::CENTER_CENTER,
            label,
            theme::regular(12.5),
            palette.secondary,
        );
    }
    painter.rect_filled(
        Rect::from_min_size(pos2(well.max.x - 1.0, tick_top), vec2(1.0, 7.0)),
        0,
        palette.secondary,
    );
    paint::text(
        painter,
        pos2(well.max.x, label_y),
        Align2::RIGHT_CENTER,
        "Pure red",
        theme::semibold(12.5),
        palette.text,
    );
}

/// A dimension line from `left` to `right` at `y`, with end ticks and the
/// label set into its middle.
fn bracket(ui: &Ui, left: f32, right: f32, y: f32, label: &str, palette: &Palette) {
    let painter = ui.painter();
    painter.rect_filled(
        Rect::from_min_max(pos2(left, y), pos2(right, y + 1.0)),
        0,
        palette.dim,
    );
    for x in [left, right - 1.0] {
        painter.rect_filled(
            Rect::from_min_size(pos2(x, y - 4.0), vec2(1.0, 9.0)),
            0,
            palette.dim,
        );
    }
    let font = theme::medium(13.0);
    let size = paint::measure(painter, label, font.clone());
    let middle = ((left + right) / 2.0).round();
    let gap = Rect::from_center_size(pos2(middle, y), size + vec2(16.0, 0.0));
    painter.rect_filled(gap, 0, palette.window);
    paint::text(
        painter,
        pos2(middle, y),
        Align2::CENTER_CENTER,
        label,
        font,
        palette.text,
    );
}

/// The name's size, in points: a quarter of the banner's height, as on the
/// sketch the owner picked (2026-11-15). The banner is a picture on a web
/// page, not the window, so its text is not held to the red channel alone:
/// each ink reads 4.5:1 or more in luminance over the lightest sky it is
/// drawn on, which is what a person on that page sees.
const NAME_SIZE: f32 = 75.0;
/// The line's size, in points. It is set in `Palette::text` like the name:
/// `Palette::secondary` reads 4.45:1 in luminance over the lightest sky under
/// a line this low.
const LINE_SIZE: f32 = 22.0;
/// From the banner's left edge to the ink of the name and of the line, in
/// points.
const TEXT_MARGIN: f32 = 48.0;
/// From the banner's top to the name's baseline, in points.
const NAME_BASELINE: f32 = 135.0;
/// From the banner's top to the line's baseline, in points.
const LINE_BASELINE: f32 = 170.0;
/// The banner's name, and its line.
const NAME: &str = "Arrakis";
const LINE: &str = "Night Shift, but it means it.";

/// The look the banner shows the cover at: the night look of the default
/// settings, which the window's cover shows by day (`--demo-scene day`).
fn banner_look() -> Look {
    Settings::default().night
}

/// The tint the GPU draws the banner's picture through: the night look's, as
/// the window's cover is drawn while the displays are not tinted
/// (`ui::preview_tint`). Its red multiplier is 1.
pub(crate) fn banner_tint() -> Tint {
    banner_look().tint()
}

/// The banner's size in pixels at `pixels_per_point`.
fn banner_pixels(pixels_per_point: f32) -> [usize; 2] {
    [BANNER.x, BANNER.y].map(|side| (side * pixels_per_point).round() as usize)
}

/// The banner's picture at `pixels_per_point`, before its tint: [`BANNER`]
/// in pixels, with the cover's square as tall as it, in the window's scene,
/// with the maw as far out as at the default night look
/// (`cover::banner::picture`). Made once for each size in the process and
/// kept: the cover's layers take a moment to draw at 600 pixels, and every
/// egui context that shows the banner, a test's included, asks for the same
/// picture.
pub(crate) fn banner_picture(pixels_per_point: f32) -> Arc<ColorImage> {
    static KEPT: Mutex<Vec<([usize; 2], Arc<ColorImage>)>> = Mutex::new(Vec::new());
    let size = banner_pixels(pixels_per_point);
    // Held while the picture is made, so that two callers of one size make it
    // once between them.
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((_, picture)) = kept.iter().find(|(kept_size, _)| *kept_size == size) {
        return Arc::clone(picture);
    }
    let [width, height] = size;
    let rise = cover::rise_of(color::redness_of(banner_look().kelvin));
    let picture = Arc::new(cover::banner::picture(
        width,
        height,
        &view::COVER_SCENE,
        rise,
    ));
    kept.push((size, Arc::clone(&picture)));
    picture
}

/// The README's banner: its picture through the night look's tint, as the
/// window's cover is drawn, and the app's name and line over the sky at the
/// left, untinted, as the window's text is.
fn banner_plate(ui: &Ui, palette: &Palette) {
    let painter = ui.painter();
    let texture = banner_texture(ui.ctx());
    let all = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
    let plate = Rect::from_min_size(Pos2::ZERO, BANNER);
    painter.image(
        texture.id(),
        plate,
        all,
        banner_tint().apply(Color32::WHITE),
    );
    for (at, galley, ink) in banner_lines(painter, palette) {
        painter.galley(at, galley, ink);
    }
}

/// The banner's picture on the GPU, one texel to a pixel, so that nothing
/// resamples it. Uploaded on the first frame for the display's scale, and kept
/// in the context's memory; the picture itself is kept by [`banner_picture`].
fn banner_texture(ctx: &egui::Context) -> TextureHandle {
    let id = Id::new("banner");
    let size = banner_pixels(ctx.pixels_per_point());
    let kept = ctx.data(|data| data.get_temp::<TextureHandle>(id));
    if let Some(texture) = kept.filter(|texture| texture.size() == size) {
        return texture;
    }
    let picture = banner_picture(ctx.pixels_per_point());
    let texture = ctx.load_texture("banner", picture, TextureOptions::NEAREST);
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

/// The banner's name and line, laid out: where each galley goes, and its
/// ink. The name is in the weight of the window's big readout
/// (`view::now_playing`), the line under it. Each stands on its baseline, at
/// [`NAME_BASELINE`] and [`LINE_BASELINE`], and by its ink's left edge, the
/// galley's `mesh_bounds`, on [`TEXT_MARGIN`]: a glyph's bearing would set a
/// large letter's ink further in than a small one's.
pub(crate) fn banner_lines(
    painter: &Painter,
    palette: &Palette,
) -> [(Pos2, Arc<Galley>, Color32); 2] {
    let lay = |text: &str, font, ink| painter.layout_no_wrap(text.to_owned(), font, ink);
    let name = lay(NAME, theme::bold(NAME_SIZE), palette.text);
    let line = lay(LINE, theme::medium(LINE_SIZE), palette.text);
    // Where a galley goes for its ink to start on the margin, and its first
    // row's baseline to be `baseline` down.
    let placed = |galley: &Galley, baseline: f32| {
        pos2(
            TEXT_MARGIN - galley.mesh_bounds.min.x,
            baseline - baseline_of(galley),
        )
    };
    let name_at = placed(&name, NAME_BASELINE);
    let line_at = placed(&line, LINE_BASELINE);
    [(name_at, name, palette.text), (line_at, line, palette.text)]
}

/// How far below its top a galley's first row stands on its baseline.
fn baseline_of(galley: &Galley) -> f32 {
    galley
        .rows
        .first()
        .and_then(|row| Some(row.pos.y + row.glyphs.first()?.pos.y))
        .unwrap_or_default()
}

/// Text standing on a baseline: `text` with its baseline at `y`, and its left
/// edge at `x` (or its right edge, when `right`). The mock of the plate of
/// figures gives each line's baseline, which `paint::text`'s anchors cannot
/// say. Returns the text's width.
fn stand(
    painter: &Painter,
    (x, y): (f32, f32),
    right: bool,
    text: &str,
    font: egui::FontId,
    ink: Color32,
) -> f32 {
    let galley = painter.layout_no_wrap(text.to_owned(), font, ink);
    let width = galley.size().x;
    let left = if right { x - width } else { x };
    // On whole device pixels, so that the glyphs stay crisp.
    let pixels = painter.pixels_per_point();
    let snap = |at: f32| (at * pixels).round() / pixels;
    painter.galley(
        pos2(snap(left), snap(y - baseline_of(&galley))),
        galley,
        ink,
    );
    width
}

/// Two readings of one thing: the earlier, which the plate's strip stands for
/// whole, and the latest, which lights it as far as its share.
#[derive(Clone, Copy, Debug)]
struct Pair {
    was: f32,
    latest: f32,
}

impl Pair {
    /// How much of its strip the latest reading lights, from 0 to 1: its
    /// share of the earlier one.
    fn share(self) -> f32 {
        self.latest / self.was
    }
}

/// The figures of the plate of figures, in one table: each as it was read,
/// before the rounding the plate prints. Each comment gives the reading (the
/// figure, the build, the date and the recipe) and then the document of this
/// repository that records it, so that it still says enough where the
/// document is absent. Every reading is of one Mac, a MacBook Pro with an M1
/// Pro, on a 120 Hz built-in display, one sitting each, and the energy is the
/// CPU's alone.
struct Figures {
    /// A drag's frames a second.
    frames_a_second: f32,
    /// The count of tests the plate prints as a floor, with a "+".
    tests_floor: u32,
    /// The network crates and the async runtimes in the normal tree.
    network_and_async: u32,
    /// The crates in the normal tree.
    tree_crates: u32,
    /// CPU energy a frame in a drag, in µJ.
    energy: Pair,
    /// What the real app holds with its window open, in MiB.
    memory: Pair,
    /// A cold first window, in ms.
    cold_window: Pair,
}

const FIGURES: Figures = Figures {
    // 119.67 frames a second [119.63 to 119.67], the median of 3 runs: 30 s of
    // `--demo-drive drag` on `NullScreen`, on the Metal painter, with the
    // bundle's `MallocSpaceEfficient=1`, read by `scripts/measure/energy-drag.py`
    // on 2026-11-12, 22:49 to 22:52, with rustc 1.98.1 and Xcode's SDK 27.2.
    frames_a_second: 119.67,
    // 1011 passed and 0 failed by `cargo test --features demo` (one more test
    // is `--ignored`: it paints the real displays), read on 2026-11-13. The
    // plate prints a floor, so that a test added later does not stale it.
    tests_floor: 1000,
    // 0, read on 2026-11-15. The names of the normal tree for the Mac:
    //   cargo tree -e normal --locked --target aarch64-apple-darwin \
    //     --prefix none --format '{p}'
    // with the paths cut off, sorted and made unique, number 169 (the app
    // itself among them; two versions of a crate count apart), and none is
    // tokio, async-std, smol, async-executor, futures-executor, futures,
    // hyper, reqwest, ureq, curl, h2, rustls, native-tls, socket2 or mio, or
    // any other crate whose name says network or async runtime (`smol_str` is
    // a string type; `pollster`, which eframe takes, is a `block_on` for one
    // future, with no scheduler and no reactor). `rg 'std::net' src crates`
    // finds nothing, and `fastframe-instance`, which has a TCP path, builds
    // it only off Unix: on macOS its one socket is a Unix socket in a
    // per-user folder, for the single-instance check.
    network_and_async: 0,
    // 169 crates, the same recipe, counted with
    //   ... | rg -v '^$' | sed -E 's/ \(.*//' | sort -u | wc -l
    // The empty line that `cargo tree` prints between the trees of the
    // workspace's two members (`arrakis`, then `egui-wgpu`) is no crate: a
    // count that keeps it reads 170.
    tree_crates: 169,
    energy: Pair {
        // 510.2 µJ a frame [504.8 to 516.6] on the GL painter: the release
        // build before the Metal painter, the median of 3 runs of the drag
        // above, in turns with the Metal painter's 201.4 [193.6 to 214.9], on
        // 2026-11-07, with the flag.
        was: 510.2,
        // 239.4 µJ a frame [229.6 to 264.7] on the Metal painter, the median of
        // 3 runs in turns with the base build, on 2026-11-12: the reading of
        // the frames above, after the design pass, which cost +22.5 µJ (+10%)
        // in these turns: before it the Metal painter read 201.4 against GL's
        // 510.2. The GL painter's energy after the pass is not read, so this
        // pair mixes two days, and its latest is today's code only.
        latest: 239.4,
    },
    memory: Pair {
        // 32.28 MiB of `phys_footprint` with the window open 90 s, on the GL
        // painter with the bundle's flag, the median of 3 runs of
        // `scripts/measure/real-cycles.sh` (the release build, the real
        // displays, no tint), read on 2026-10-31 (03:24 to 04:42).
        was: 32.28,
        // 19.20 MiB, the same state, on the Metal painter with the flag, the
        // median of 3 runs of `real-cycles.sh`, read in turns on 2026-11-07,
        // 11:47 to 12:22, before the design pass. The two readings come from
        // two sittings a week apart, not from one turn.
        latest: 19.20,
    },
    cold_window: Pair {
        // 226.5 ms [226.1 to 331.2] from the request to the first frame of the
        // first window after a cold start, with the shader compiled from its
        // source, the median of 3 turns of
        // `--demo-drive open-close --demo-seconds 20` on `NullScreen`
        // (`opens[0].to_first_frame_ms`), each a real shader-cache miss, on
        // 2026-11-07, 09:01 to 09:14.
        was: 226.5,
        // 151.6 ms [145.0 to 163.5], the same recipe and turns, with the shader
        // compiled ahead and loaded from its file.
        latest: 151.6,
    },
};

/// A figure as the plate prints it: `figure` rounded to `decimals` places,
/// halves up (`format!` rounds a half to even, which would print 226.5 as 226).
fn printed(figure: f32, decimals: usize) -> String {
    let scale = 10f32.powi(decimals as i32);
    format!("{:.decimals$}", (figure * scale).round() / scale)
}

/// The plate of figures' margin, and the width of the strips' column, in
/// points: the mock the owner picked gives its places, one unit to a point.
const BENCH_MARGIN: f32 = 32.0;
/// Where each readout's column starts, and how far apart they stand.
const READOUT_STEP: f32 = 250.0;
/// The first strip's baseline, and the step between the three.
const ROW_FIRST: f32 = 170.0;
const ROW_STEP: f32 = 48.0;
/// The strips: their left edge and their length, whole points.
const STRIP_LEFT: f32 = 290.0;
const STRIP_LENGTH: f32 = 300.0;

/// The plate of figures: three readouts, a rule under them and three strips of
/// before and after, as the owner picked them in a mock. Each strip is
/// the window's Temperature track (`paint::spectrum`, with the window's veil),
/// with no knob, ticks or names: its whole length stands for what the reading
/// was, and it is lit as far as the latest reading's share of that.
fn bench_plate(ui: &Ui, palette: &Palette) {
    let painter = ui.painter();
    painter.rect_filled(Rect::from_min_size(Pos2::ZERO, BENCH), 0, palette.window);
    let right = BENCH.x - BENCH_MARGIN;
    let figures = &FIGURES;

    let readouts = [
        (
            printed(figures.frames_a_second, 1),
            "frames a second",
            "a drag on a 120 Hz panel".to_owned(),
        ),
        (
            format!("{}+", figures.tests_floor),
            "tests",
            "pixels, color curve and contrast, pinned".to_owned(),
        ),
        (
            figures.network_and_async.to_string(),
            "network crates, async runtimes",
            format!(
                "of {} crates in the tree; no telemetry",
                figures.tree_crates
            ),
        ),
    ];
    for (index, (figure, caption, line)) in readouts.into_iter().enumerate() {
        let x = BENCH_MARGIN + READOUT_STEP * index as f32;
        stand(
            painter,
            (x, 72.0),
            false,
            &figure,
            theme::bold(46.0),
            palette.text,
        );
        stand(
            painter,
            (x, 96.0),
            false,
            caption,
            theme::semibold(14.0),
            palette.secondary,
        );
        stand(
            painter,
            (x, 114.0),
            false,
            &line,
            theme::regular(11.5),
            palette.faint,
        );
    }
    painter.rect_filled(
        Rect::from_min_max(pos2(BENCH_MARGIN, 134.0), pos2(right, 135.0)),
        0,
        palette.outline,
    );

    let rows = [
        (
            "CPU energy a frame, dragging",
            "its own Metal painter, where GL was",
            "µJ",
            figures.energy,
        ),
        (
            "Memory, window open",
            "the same change, read a week apart",
            "MiB",
            figures.memory,
        ),
        (
            "First window, cold",
            "the shader compiled ahead",
            "ms",
            figures.cold_window,
        ),
    ];
    for (index, (label, how, unit, pair)) in rows.into_iter().enumerate() {
        let y = ROW_FIRST + ROW_STEP * index as f32;
        stand(
            painter,
            (BENCH_MARGIN, y),
            false,
            label,
            theme::semibold(15.0),
            palette.text,
        );
        stand(
            painter,
            (BENCH_MARGIN, y + 17.0),
            false,
            how,
            theme::regular(12.0),
            palette.faint,
        );
        let strip = Rect::from_min_max(
            pos2(STRIP_LEFT, y - theme::SLIDER_TRACK),
            pos2(STRIP_LEFT + STRIP_LENGTH, y),
        );
        paint::spectrum(
            painter,
            strip,
            pair.share(),
            |color| palette.veiled(color),
            |position| color::scale_tint(position).light(),
        );
        stand(
            painter,
            (strip.max.x, y + 17.0),
            true,
            &format!("was {}", printed(pair.was, 0)),
            theme::regular(11.0),
            palette.dim,
        );
        let unit_width = stand(
            painter,
            (right, y + 6.0),
            true,
            &format!(" {unit}"),
            theme::semibold(13.0),
            palette.secondary,
        );
        stand(
            painter,
            (right - unit_width, y + 6.0),
            true,
            &printed(pair.latest, 0),
            theme::bold(24.0),
            palette.text,
        );
    }

    stand(
        painter,
        (BENCH_MARGIN, 318.0),
        false,
        "MacBook Pro, M1 Pro · energy is the CPU's · read 31 Oct to 15 Nov 2026",
        theme::regular(11.0),
        palette.dim,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_app;
    use crate::ui::testing;

    #[test]
    fn the_focused_scene_is_the_night_at_half_past_eleven() {
        let (settings, red_now) = Scene::Focused.set_up();
        // The default settings, with no Red Now, at 23:30.
        assert_eq!(settings, Settings::default());
        assert!(!red_now);
        assert_eq!(clock(), 23.0 * 60.0 + 30.0);
        // Inside the default night and past its fade, so the screen is
        // tinted and the shot's `-tinted` twin is saved.
        let level = settings.schedule().level(clock());
        assert_eq!(level, 1.0);
        assert!(!settings.night.at_level(level).is_neutral());
        // It is the night's moment, and only that.
        assert_eq!(Scene::Focused.moments().len(), 1);
        assert_eq!(
            Scene::Focused.moments()[0].minute,
            Scene::Night.moments()[0].minute
        );
    }

    #[test]
    fn the_desert_scene_is_the_night_with_the_temperature_on_the_desert_stop() {
        let (settings, red_now) = Scene::Desert.set_up();
        // The default settings with only the night's temperature moved onto
        // the Desert stop: its brightness and the schedule are the night's.
        assert_eq!(settings.night.kelvin, ui::DESERT_KELVIN);
        let defaults = Settings::default();
        assert_eq!(settings.night.brightness, defaults.night.brightness);
        assert_eq!(
            Settings {
                night: defaults.night,
                ..settings
            },
            defaults
        );
        assert!(!red_now);
        // At 23:30, the night's moment and only that, tinted.
        assert_eq!(clock(), 23.0 * 60.0 + 30.0);
        assert_eq!(Scene::Desert.moments().len(), 1);
        assert_eq!(
            Scene::Desert.moments()[0].minute,
            Scene::Night.moments()[0].minute
        );
        let level = settings.schedule().level(clock());
        assert_eq!(level, 1.0);
        assert!(!settings.night.at_level(level).is_neutral());
    }

    /// The label of the node that has the keyboard after one frame of the
    /// window under `shot`, if it has one.
    fn focused_label(ctx: &egui::Context, shot: &mut Shot, app: &mut App) -> Option<String> {
        let output = testing::output(ctx, vec![], |ui| {
            shot.frame(ui.ctx(), Some(&mut *app));
            crate::ui::draw(app, ui);
        });
        let update = output.platform_output.accesskit_update?;
        update
            .nodes
            .iter()
            .find(|(id, _)| *id == update.focus)
            .and_then(|(_, node)| node.label().map(str::to_owned))
    }

    #[test]
    fn the_focused_scene_keeps_the_temperature_slider_focused() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut app = test_app(clock);
        let (settings, _) = Scene::Focused.set_up();
        app.settings = settings;
        let path = std::env::temp_dir().join("arrakis-never-saved.png");
        let mut shot = Shot::new(path, Scene::Focused);
        for frame in 0..3 {
            assert_eq!(
                focused_label(&ctx, &mut shot, &mut app).as_deref(),
                Some("Temperature: 1200 K, Ember"),
                "frame {frame}"
            );
        }
        // The window's own focus handling may drop it while it opens: the
        // scene asks again on the next frame.
        ctx.memory_mut(|memory| memory.surrender_focus(Id::new(view::TEMPERATURE)));
        assert_eq!(
            focused_label(&ctx, &mut shot, &mut app).as_deref(),
            Some("Temperature: 1200 K, Ember")
        );
    }

    /// An image of a window `height` points tall at `scale`, all `ground`.
    fn window_image(scale: f32, height: f32, ground: Color32) -> ColorImage {
        let size = [
            (theme::WINDOW.x * scale) as usize,
            (height * scale) as usize,
        ];
        ColorImage::filled(size, ground)
    }

    /// The pixels of `image` that are not `ground`, as (x, y).
    fn changed(image: &ColorImage, ground: Color32) -> Vec<(usize, usize)> {
        let width = image.size[0];
        image
            .pixels
            .iter()
            .enumerate()
            .filter(|(_, pixel)| **pixel != ground)
            .map(|(at, _)| (at % width, at / width))
            .collect()
    }

    /// The traffic lights are the system's: they sit centered in the title
    /// bar, however tall the window under them is.
    /// So the same lights are stamped on windows of two heights, and a light
    /// placed from the image's middle or foot would differ between them.
    #[test]
    fn the_traffic_lights_do_not_follow_the_windows_height() {
        let ground = Palette::DARK.window;
        for scale in [1.0, 2.0] {
            let lit_at = |height: f32| {
                let mut image = window_image(scale, height, ground);
                stamp_traffic_lights(&mut image, scale);
                changed(&image, ground)
            };
            let lit = lit_at(theme::WINDOW.y);
            assert!(!lit.is_empty(), "no light at scale {scale}");
            assert_eq!(
                lit,
                lit_at(theme::WINDOW.y * 1.5),
                "the lights follow the height at scale {scale}"
            );
        }
    }

    /// The window's bottom corners are rounded off like its top ones, at the
    /// height the window has and at any other.
    #[test]
    fn the_bottom_corners_are_rounded_like_the_top_ones_at_any_height() {
        let ground = Palette::DARK.window;
        for scale in [1.0, 2.0] {
            for window_height in [theme::WINDOW.y, theme::WINDOW.y * 1.5] {
                let mut image = window_image(scale, window_height, ground);
                let radius = CORNER * scale;
                round_corners(&mut image, radius);
                let [width, height] = image.size;
                let pixel = |x: usize, y: usize| image.pixels[y * width + x];
                // The bottom corners are clear, and mirror the top ones.
                for (x, y) in [(0, height - 1), (width - 1, height - 1)] {
                    assert_eq!(
                        pixel(x, y).a(),
                        0,
                        "corner ({x}, {y}) at scale {scale}, height {window_height}"
                    );
                }
                let clear_of = radius.ceil() as usize;
                for y in 0..clear_of {
                    for x in 0..clear_of {
                        assert_eq!(pixel(x, y), pixel(x, height - 1 - y), "({x}, {y})");
                        assert_eq!(
                            pixel(width - 1 - x, y),
                            pixel(width - 1 - x, height - 1 - y)
                        );
                    }
                }
            }
        }
    }

    /// The Scale plate is an `eframe::App` of its own, so it needs the
    /// window's cap on the texture side: without it, `--demo-scene scale`
    /// makes the font atlas as wide as the painter's limit, 16384 on this Mac.
    #[test]
    fn the_scale_plate_makes_its_atlas_as_narrow_as_the_windows_cap() {
        const PAINTER_LIMIT: usize = 16384;
        let path = std::env::temp_dir().join("arrakis-never-saved.png");
        let mut plate = Plate::new(Shot::new(path, Scene::Scale), SCALE_PLATE);
        // The atlas width after a pass of the plate's drawing, with the hook
        // run on the input first or not.
        let mut atlas_after = |hooked: bool| {
            let ctx = testing::context();
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), PLATE)),
                max_texture_side: Some(PAINTER_LIMIT),
                ..Default::default()
            };
            if hooked {
                eframe::App::raw_input_hook(&mut plate, &ctx, &mut raw);
            }
            let mut output = ctx.run_ui(raw, |ui| scale_plate(ui, &Palette::DARK));
            output.textures_delta.clear();
            ctx.fonts(|fonts| fonts.font_image_size())[0]
        };
        // The control: a pass that skips the hook gets the painter's limit whole.
        assert_eq!(atlas_after(false), PAINTER_LIMIT);
        let width = atlas_after(true);
        assert!(width < PAINTER_LIMIT, "the plate's atlas is {width} wide");
    }

    /// No screenshot ever comes from a painter that paints nothing, and a plate has no `App` to read
    /// the flag, so without this its shot asks for frames for ever.
    #[test]
    fn a_plate_on_a_failed_painter_closes_where_a_healthy_one_goes_on() {
        let closes_asked = |painter_failed: fn() -> bool| {
            let path = std::env::temp_dir().join("arrakis-never-saved.png");
            let mut plate = Plate::new(Shot::new(path, Scene::Scale), SCALE_PLATE);
            plate.painter_failed = painter_failed;
            let output = testing::context().run_logic(&egui::RawInput::default(), |ctx| {
                plate.close_when_done(ctx);
            });
            output
                .viewport_commands
                .into_values()
                .flatten()
                .filter(|command| matches!(command, egui::ViewportCommand::Close))
                .count()
        };
        assert_eq!(closes_asked(|| true), 1);
        assert_eq!(closes_asked(|| false), 0, "its shot has not been taken yet");
    }

    /// The shot of `scene` that no test saves.
    fn shot_of(scene: Scene) -> Shot {
        Shot::new(std::env::temp_dir().join("arrakis-never-saved.png"), scene)
    }

    /// One pass of the plate `kind`, at `scale` pixels to the point, over a
    /// screen of the plate's size.
    fn pass_of(kind: PlateKind, scale: f32) -> testing::Pass {
        testing::pass_over(kind.size, scale, |ui| (kind.draw)(ui, &Palette::DARK))
    }

    /// The texts a pass painted, in paint order.
    fn texts(pass: &testing::Pass) -> Vec<&egui::epaint::TextShape> {
        (pass.shapes.iter())
            .filter_map(|(_, shape)| match shape {
                egui::Shape::Text(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    /// The meshes a pass painted that show a texture of the plate's own: not
    /// the font atlas, which every text draws with, nor the white pixel that
    /// the strips and the rectangles use.
    fn images(pass: &testing::Pass) -> Vec<&egui::Mesh> {
        (pass.shapes.iter())
            .filter_map(|(_, shape)| match shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    Some(&**mesh)
                }
                _ => None,
            })
            .collect()
    }

    /// The `src`'s `<img>` in the README: its `width` and its `alt`.
    fn readme_image(src: &str) -> (String, String) {
        let readme = include_str!("../README.md");
        let from = readme
            .find(&format!("<img src=\"{src}\""))
            .unwrap_or_else(|| panic!("the README has no <img> of {src}"));
        let tag = &readme[from..from + readme[from..].find('>').expect("a closed tag")];
        let attribute = |name: &str| {
            let key = format!(" {name}=\"");
            let start = tag.find(&key).unwrap_or_else(|| panic!("{src}: no {name}")) + key.len();
            let end = start + tag[start..].find('"').expect("a closed quote");
            tag[start..end].to_owned()
        };
        (attribute("width"), attribute("alt"))
    }

    /// Whether `word` stands in `text` as a word of its own: a number is not
    /// found inside a longer one (19 is not in 119.7).
    fn has_word(text: &str, word: &str) -> bool {
        text.split(|c: char| !(c.is_alphanumeric() || c == '.'))
            .map(|found| found.trim_matches('.'))
            .any(|found| found == word)
    }

    /// The relative luminance of `color`, from its linear channels.
    fn luminance(color: Color32) -> f32 {
        let linear = egui::Rgba::from(color);
        0.2126 * linear.r() + 0.7152 * linear.g() + 0.0722 * linear.b()
    }

    /// The contrast of two colors in luminance, from 1 to 21.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let [a, b] = [a, b].map(luminance);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// `pixel` as the GPU shows it under a vertex color of `tint`: each channel
    /// multiplied, in 8 bits.
    fn through(tint: Color32, pixel: Color32) -> Color32 {
        let mix = |tint: u8, pixel: u8| (f32::from(tint) * f32::from(pixel) / 255.0).round() as u8;
        Color32::from_rgb(
            mix(tint.r(), pixel.r()),
            mix(tint.g(), pixel.g()),
            mix(tint.b(), pixel.b()),
        )
    }

    #[test]
    fn the_banner_is_as_wide_as_the_readme_shows_it_and_twice_that_in_pixels() {
        for (src, plate, name) in [
            ("docs/banner.png", BANNER, "the banner"),
            ("docs/bench.png", BENCH, "the plate of figures"),
        ] {
            let (width, _) = readme_image(src);
            assert_eq!(width, format!("{}", plate.x), "{name}: shown at 1x");
        }
        // What the banner's texture holds is what a plate of that size
        // needs, at the display's scale: a pixel to a pixel.
        let at_two = banner_picture(2.0);
        assert_eq!(at_two.size, [BANNER.x as usize * 2, BANNER.y as usize * 2]);
        let at_one = banner_picture(1.0);
        assert_eq!(at_one.size, [BANNER.x as usize, BANNER.y as usize]);
    }

    /// The PNG committed at `src`: its color type and bit depth as stored, its
    /// size in pixels and its samples.
    fn committed_picture(src: &str) -> (png::ColorType, png::BitDepth, [usize; 2], Vec<u8>) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(src);
        let file = std::fs::File::open(&path).unwrap_or_else(|error| panic!("{src}: {error}"));
        let mut reader = png::Decoder::new(std::io::BufReader::new(file))
            .read_info()
            .unwrap_or_else(|error| panic!("{src}: {error}"));
        let info = reader.info();
        let (color, depth) = (info.color_type, info.bit_depth);
        let size = [info.width as usize, info.height as usize];
        let mut samples = vec![0; reader.output_buffer_size().expect("a size")];
        let frame =
            (reader.next_frame(&mut samples)).unwrap_or_else(|error| panic!("{src}: {error}"));
        samples.truncate(frame.buffer_size());
        (color, depth, size, samples)
    }

    /// What the README shows is what a scene's window draws, at two pixels to
    /// the point: each picture has its scene's size, which `Scene::plate` says,
    /// and is cut at its corners as a shot of that scene cuts a plain ground.
    #[test]
    fn the_committed_pictures_are_their_scenes_size_at_two_pixels_to_the_point_and_cut_at_the_corners()
     {
        let scale = 2.0;
        let ground = Color32::from_rgb(10, 20, 30);
        let alphas =
            |image: &ColorImage| -> Vec<u8> { image.pixels.iter().map(|p| p.a()).collect() };
        for (src, scene) in [
            ("docs/banner.png", Scene::Banner),
            ("docs/bench.png", Scene::Bench),
        ] {
            let size = scene.plate().expect("a plate").size;
            let pixels = [size.x, size.y].map(|side| (side * scale) as usize);
            let (color, depth, found, samples) = committed_picture(src);
            assert_eq!(
                (color, depth),
                (png::ColorType::Rgba, png::BitDepth::Eight),
                "{src}"
            );
            assert_eq!(
                found, pixels,
                "{src}: {size:?} points at {scale} to the point"
            );
            let picture = ColorImage::from_rgba_unmultiplied(found, &samples);
            let [width, height] = found;
            let alpha = |x: usize, y: usize| picture.pixels[y * width + x].a();
            for (x, y) in [
                (0, 0),
                (width - 1, 0),
                (0, height - 1),
                (width - 1, height - 1),
            ] {
                assert_eq!(alpha(x, y), 0, "{src}: corner ({x}, {y})");
            }
            assert_eq!(alpha(width / 2, height / 2), 255, "{src}: the middle");
            // The same picture drawn without the element: a plain ground, cut
            // by the shot as the scene's picture is. Its alpha is the picture's
            // to the pixel, the soft edge of each corner included.
            let mut shot = shot_of(scene);
            shot.scale = scale;
            let bare = shot.framed(&ColorImage::filled(pixels, ground));
            assert_eq!(
                alphas(&picture),
                alphas(&bare),
                "{src}: not cut as a shot of the scene cuts a plain ground"
            );
            // The control: a ground that is not cut reads differently.
            assert_ne!(
                alphas(&picture),
                alphas(&ColorImage::filled(pixels, ground)),
                "{src}"
            );
        }
    }

    /// Every egui context that shows the banner asks for the one picture of
    /// its size, and the plate's texture is that picture, not a copy.
    #[test]
    fn the_banners_picture_is_made_once_for_each_size_and_the_plate_shows_that_one() {
        let at_two = banner_picture(2.0);
        assert!(
            Arc::ptr_eq(&at_two, &banner_picture(2.0)),
            "a second call made the picture again"
        );
        assert!(
            Arc::ptr_eq(&at_two, &testing::banner_picture_2x()),
            "the helper's picture is not the one at two"
        );
        // Another size is another picture, made once as well.
        let at_one = banner_picture(1.0);
        assert!(!Arc::ptr_eq(&at_two, &at_one));
        assert_ne!(at_one.size, at_two.size);
        assert!(Arc::ptr_eq(&at_one, &banner_picture(1.0)));
        // The texture a plate sets is the kept picture itself.
        let pass = pass_of(BANNER_PLATE, 2.0);
        let image = images(&pass)[0].texture_id;
        let (_, delta) = (pass.textures.iter())
            .find(|(id, _)| *id == image)
            .expect("the image's texture is set");
        match &delta.image {
            egui::ImageData::Color(shown) => assert!(Arc::ptr_eq(shown, &at_two)),
        }
    }

    /// A plate always draws: it takes its drawing from the kind it is made
    /// with, whatever scene its shot is of.
    #[test]
    fn each_plate_paints_something_and_takes_its_drawing_from_its_kind() {
        use clap::ValueEnum;
        let mut kinds = 0;
        for scene in Scene::value_variants() {
            let Some(kind) = scene.plate() else { continue };
            kinds += 1;
            let plate = Plate::new(shot_of(*scene), kind);
            let pass = testing::pass_over(kind.size, 2.0, |ui| (plate.draw)(ui, &Palette::DARK));
            assert!(!texts(&pass).is_empty(), "{scene:?} has no text");
            let ground = (pass.shapes.iter())
                .filter(|(_, shape)| match shape {
                    egui::Shape::Rect(rect) => rect.fill != Color32::TRANSPARENT,
                    egui::Shape::Mesh(_) => true,
                    _ => false,
                })
                .count();
            assert!(ground > 0, "{scene:?} paints neither a rect nor an image");
        }
        assert_eq!(kinds, 3);
        // The drawing is the kind's, not the shot's scene's: the bench's kind
        // over a shot of the Scale scene draws the bench's texts, not the Scale
        // plate's.
        let said = |pass: &testing::Pass| -> Vec<String> {
            texts(pass)
                .iter()
                .map(|t| t.galley.text().to_owned())
                .collect()
        };
        let plate = Plate::new(shot_of(Scene::Scale), BENCH_PLATE);
        let drawn = testing::pass_over(BENCH, 2.0, |ui| (plate.draw)(ui, &Palette::DARK));
        assert_eq!(said(&drawn), said(&pass_of(BENCH_PLATE, 2.0)));
        assert_ne!(said(&drawn), said(&pass_of(SCALE_PLATE, 2.0)));
    }

    #[test]
    fn the_readme_says_what_the_banner_says() {
        let (_, alt) = readme_image("docs/banner.png");
        for line in [NAME, LINE] {
            assert!(alt.contains(line), "the banner's alt text lacks {line:?}");
        }
    }

    /// The pictures that are plates of their own, the banner and the plate of
    /// figures, are cut at the window's corners and have no title bar, so no
    /// traffic lights; the Scale plate is left as drawn; a window's shot gets
    /// the lights and the corners.
    #[test]
    fn a_plate_keeps_its_size_and_a_window_shot_gets_its_traffic_lights() {
        let ground = Color32::from_rgb(10, 20, 30);
        let scale = 2.0;
        let sized = |size: Vec2| [(size.x * scale) as usize, (size.y * scale) as usize];
        let shot_at = |scene: Scene| {
            let mut shot = shot_of(scene);
            shot.scale = scale;
            shot
        };
        // Where the lights go, on a ground of any size.
        let lights_on = |size: [usize; 2]| {
            let mut lit = ColorImage::filled(size, ground);
            stamp_traffic_lights(&mut lit, scale);
            changed(&lit, ground)
        };
        for scene in [Scene::Banner, Scene::Bench] {
            let plate = scene.plate().expect("a plate");
            assert!(plate.rounded, "{scene:?}");
            let size = sized(plate.size);
            let framed = shot_at(scene).framed(&ColorImage::filled(size, ground));
            assert_eq!(framed.size, size, "{scene:?}");
            let [width, height] = size;
            let pixel = |x: usize, y: usize| framed.pixels[y * width + x];
            for (x, y) in [
                (0, 0),
                (width - 1, 0),
                (0, height - 1),
                (width - 1, height - 1),
            ] {
                assert_eq!(pixel(x, y).a(), 0, "{scene:?}: corner ({x}, {y})");
            }
            assert_eq!(
                pixel(width / 2, height / 2),
                ground,
                "{scene:?}: the middle"
            );
            let lights = lights_on(size);
            assert!(!lights.is_empty());
            for (x, y) in lights {
                assert_eq!(pixel(x, y), ground, "{scene:?}: a light at ({x}, {y})");
            }
        }
        // The Scale plate: as drawn.
        let kind = Scene::Scale.plate().expect("a plate");
        assert_eq!(kind.size, PLATE);
        assert!(!kind.rounded);
        let size = sized(PLATE);
        let drawn = ColorImage::filled(size, ground);
        assert_eq!(shot_at(Scene::Scale).framed(&drawn).pixels, drawn.pixels);
        // The control: a window's shot gets its lights and its corners.
        let size = sized(theme::WINDOW);
        let window = shot_at(Scene::Day).framed(&ColorImage::filled(size, ground));
        for (x, y) in lights_on(size) {
            assert_ne!(
                window.pixels[y * size[0] + x],
                ground,
                "no light at ({x}, {y})"
            );
        }
        assert_eq!(window.pixels[0].a(), 0);
    }

    #[test]
    fn the_scale_plate_lays_out_its_title_and_paints_no_image() {
        let pass = pass_of(SCALE_PLATE, 2.0);
        let said: Vec<String> = texts(&pass)
            .iter()
            .map(|t| t.galley.text().to_owned())
            .collect();
        assert!(said.iter().any(|text| text == "Temperature"), "{said:?}");
        assert!(images(&pass).is_empty());
    }

    #[test]
    fn the_banner_plate_paints_its_picture_over_its_rect_and_lays_out_the_name_and_the_line() {
        let scale = 2.0;
        let pass = pass_of(BANNER_PLATE, scale);
        let pictures = images(&pass);
        assert_eq!(pictures.len(), 1, "one image");
        let bounds = Rect::from_points(
            &pictures[0]
                .vertices
                .iter()
                .map(|v| v.pos)
                .collect::<Vec<_>>(),
        );
        assert_eq!(bounds, Rect::from_min_size(Pos2::ZERO, BANNER));
        // Its texture holds a pixel for each of the plate's, at this scale.
        let (_, delta) = (pass.textures.iter())
            .find(|(id, _)| *id == pictures[0].texture_id)
            .expect("the image's texture is set");
        assert_eq!(delta.image.size(), banner_pixels(scale));
        let said: Vec<&str> = texts(&pass).iter().map(|t| t.galley.text()).collect();
        assert_eq!(said, [NAME, LINE]);
    }

    /// The banner's two inks, read off what the plate draws: the name and the
    /// line with their ink rects in points, and the color each is drawn in.
    fn banner_inks(pass: &testing::Pass) -> Vec<(String, Rect, Color32, f32)> {
        (texts(pass).into_iter())
            .map(|text| {
                let ink = text.galley.mesh_bounds.translate(text.pos.to_vec2());
                let baseline = text.pos.y + baseline_of(&text.galley);
                (
                    text.galley.text().to_owned(),
                    ink,
                    testing::drawn(text),
                    baseline,
                )
            })
            .collect()
    }

    #[test]
    fn the_banners_inks_read_4_5_to_1_in_luminance_over_the_sky_they_stand_on() {
        let scale = 2.0;
        let pass = pass_of(BANNER_PLATE, scale);
        let picture = testing::banner_picture_2x();
        let [width, _] = picture.size;
        // The picture is seen through its look's tint, the image mesh's vertex
        // color, which the GPU multiplies in. The inks are not tinted.
        let tint = images(&pass)[0].vertices[0].color;
        let left_of_the_square = width - picture.size[1];
        let inks = banner_inks(&pass);
        assert_eq!(inks.len(), 2);
        for (said, ink, color, _) in inks {
            assert_eq!(
                color,
                Palette::DARK.text,
                "{said:?} is set in the window's text"
            );
            // The pixels under the ink's rect, rounded out.
            let (x0, x1) = (
                (ink.min.x * scale).floor() as usize,
                (ink.max.x * scale).ceil() as usize,
            );
            let (y0, y1) = (
                (ink.min.y * scale).floor() as usize,
                (ink.max.y * scale).ceil() as usize,
            );
            assert!(
                x1 <= left_of_the_square,
                "{said:?} reaches the cover's square"
            );
            let (mut lightest, mut darkest) = (Color32::BLACK, Color32::WHITE);
            for y in y0..y1 {
                for x in x0..x1 {
                    let pixel = picture.pixels[y * width + x];
                    // Clear of the dunes and of the sun: the sky of its row.
                    assert_eq!(
                        pixel,
                        picture.pixels[y * width + left_of_the_square - 1],
                        "{said:?}: not the sky at ({x}, {y})"
                    );
                    let seen = through(tint, pixel);
                    if luminance(seen) > luminance(lightest) {
                        lightest = seen;
                    }
                    if luminance(seen) < luminance(darkest) {
                        darkest = seen;
                    }
                }
            }
            for (name, ground) in [("lightest", lightest), ("darkest", darkest)] {
                let ratio = contrast(color, ground);
                assert!(ratio >= 4.5, "{said:?} over the {name} sky: {ratio:.2}:1");
            }
            // The control: a dimmer ink fails the same reading.
            let dim = contrast(Palette::DARK.dim, lightest);
            assert!(
                dim < 4.5,
                "{said:?}: the check passes a quiet ink, {dim:.2}:1"
            );
        }
    }

    #[test]
    fn the_banners_ink_starts_on_the_margin_and_stands_on_its_baseline() {
        let scale = 2.0;
        // One device pixel, in points.
        let pixel = 1.0 / scale;
        let inks = banner_inks(&pass_of(BANNER_PLATE, scale));
        for (said, ink, _, baseline) in inks {
            let wanted = if said == NAME {
                NAME_BASELINE
            } else {
                LINE_BASELINE
            };
            assert!(
                (ink.min.x - TEXT_MARGIN).abs() <= pixel,
                "{said:?}: ink at {}",
                ink.min.x
            );
            assert!(
                (baseline - wanted).abs() <= pixel,
                "{said:?}: baseline at {baseline}"
            );
        }
    }

    /// The least contrast in luminance, against `ground`, of the inks that the
    /// texts of `pass` are drawn in.
    fn least_contrast(pass: &testing::Pass, ground: Color32) -> f32 {
        (texts(pass).into_iter())
            .map(|text| contrast(testing::drawn(text), ground))
            .fold(f32::INFINITY, f32::min)
    }

    /// The plate of figures is a picture on a web page, never seen through the
    /// tint, so its inks are held to 4.5:1 in luminance, not in the red
    /// channel.
    #[test]
    fn every_ink_the_plate_of_figures_sets_text_in_reads_4_5_to_1_in_luminance_on_the_window() {
        let palette = Palette::DARK;
        let pass = pass_of(BENCH_PLATE, 2.0);
        // The ground is what the plate fills first, over the whole of it.
        let ground = (pass.shapes.iter())
            .find_map(|(_, shape)| match shape {
                egui::Shape::Rect(rect) if rect.rect == Rect::from_min_size(Pos2::ZERO, BENCH) => {
                    Some(rect.fill)
                }
                _ => None,
            })
            .expect("the plate fills its ground");
        assert_eq!(ground, palette.window);
        let mut inks: Vec<Color32> = vec![];
        for text in texts(&pass) {
            let ink = testing::drawn(text);
            if !inks.contains(&ink) {
                inks.push(ink);
            }
        }
        // The plate sets text in these four, and each is read.
        for (name, ink) in [
            ("text", palette.text),
            ("secondary", palette.secondary),
            ("faint", palette.faint),
            ("dim", palette.dim),
        ] {
            assert!(inks.contains(&ink), "no text of the plate is in `{name}`");
        }
        for ink in inks {
            let ratio = contrast(ink, ground);
            assert!(ratio >= 4.5, "{ink:?} on the window: {ratio:.2}:1");
        }
        // The control: a text in the outline's ink, a hairline's, fails the
        // same reading.
        let quiet = testing::pass_over(BENCH, 2.0, |ui| {
            stand(
                ui.painter(),
                (BENCH_MARGIN, 100.0),
                false,
                "A line in the outline's ink",
                theme::regular(11.0),
                palette.outline,
            );
        });
        let ratio = least_contrast(&quiet, ground);
        assert!(ratio < 4.5, "the check passes a quiet ink: {ratio:.2}:1");
    }

    #[test]
    fn the_banner_is_drawn_through_the_tint_that_the_windows_cover_is() {
        // The window's cover shows the default night look, and its textures are
        // drawn through that look's tint, its vertex color.
        let mut window = crate::ui::headless::Headless::new(2.0);
        let frame = window.frame();
        let windows = frame.cover_meshes()[0].1.vertices[0].color;
        let banner = images(&pass_of(BANNER_PLATE, 2.0))[0].vertices[0].color;
        assert_eq!(banner, windows);
        // The control: that is a tint, not white. Red is left whole, so the
        // picture's own reds are what the page shows.
        assert_ne!(windows, Color32::WHITE);
        assert_eq!(windows.r(), 255);
    }

    /// The figures are read off the texts the plate draws, so a rounding that
    /// went wrong in the drawing shows here, not only in a table.
    #[test]
    fn the_readme_names_the_figures_the_plate_draws() {
        let (_, alt) = readme_image("docs/bench.png");
        let pass = pass_of(BENCH_PLATE, 2.0);
        let said: Vec<&str> = texts(&pass).iter().map(|t| t.galley.text()).collect();
        // A figure is a text that is a number alone: a readout's, or a row's
        // latest reading. The tests' floor is drawn with a plus, and the alt
        // text says more than that: nothing more is pinned, as the count of
        // tests moves. The readout of "0" counts the network crates, which the
        // alt text says as "no".
        let figures: Vec<&str> = (said.iter().copied())
            .filter(|text| text.trim_end_matches('+').parse::<f32>().is_ok() && *text != "0")
            .map(|text| text.trim_end_matches('+'))
            .collect();
        // The earlier readings of the three rows, which are drawn "was 510".
        let earlier: Vec<&str> = (said.iter())
            .filter_map(|text| text.strip_prefix("was "))
            .collect();
        // The crates in the tree stand in the line under the third readout.
        let line = (said.iter())
            .find(|text| text.contains("crates in the tree"))
            .expect("the line that counts the crates in the tree");
        let in_tree: Vec<&str> = (line.split(|c: char| !c.is_ascii_digit()))
            .filter(|number| !number.is_empty())
            .collect();
        // Two readouts and three rows, and three rows' earlier readings, so
        // that no figure goes unread.
        assert_eq!(figures.len(), 5, "{figures:?}");
        assert_eq!(earlier.len(), 3, "{earlier:?}");
        assert_eq!(in_tree.len(), 1, "{line:?}");
        for figure in figures.iter().chain(&earlier).chain(&in_tree) {
            assert!(
                has_word(&alt, figure),
                "{figure} is drawn on the plate and is not in the alt text: {alt}"
            );
        }
        // A half is drawn rounded up: 226.5 ms is the cold window's earlier
        // reading, and `format!("{:.0}")` would have drawn the even 226.
        assert_eq!(
            earlier[2],
            format!("{}", (FIGURES.cold_window.was + 0.5).floor())
        );
        // The control: a number is not found inside a longer one.
        assert!(!has_word("on 119.7 frames", "19"));
    }

    /// The first of the plate of figures' three strips' rects, and the step
    /// to the next.
    fn strip_of(row: usize) -> Rect {
        let y = ROW_FIRST + ROW_STEP * row as f32;
        Rect::from_min_max(
            pos2(STRIP_LEFT, y - theme::SLIDER_TRACK),
            pos2(STRIP_LEFT + STRIP_LENGTH, y),
        )
    }

    #[test]
    fn each_strip_is_lit_up_to_the_latest_readings_share_of_its_length() {
        let scale = 2.0;
        // A device pixel, in points.
        let pixel = 1.0 / scale;
        let scale_at = |position: f32| color::scale_tint(position).light();
        let pass = pass_of(BENCH_PLATE, scale);
        let pairs = [FIGURES.energy, FIGURES.memory, FIGURES.cold_window];
        for (row, pair) in pairs.into_iter().enumerate() {
            let strip = strip_of(row);
            let wanted = STRIP_LEFT + STRIP_LENGTH * (pair.latest / pair.was);
            let edge = testing::lit_edge(&pass.shapes, strip, scale_at);
            assert!(
                (edge - wanted).abs() <= pixel,
                "strip {row}: lit to {edge}, wanted {wanted} (within {pixel})"
            );
            // The lit end lies inside its track: some of the strip is lit and
            // some is dim, whatever the readings are.
            assert!(
                strip.min.x < edge && edge < strip.max.x,
                "strip {row}: lit to {edge}, in {} to {}",
                strip.min.x,
                strip.max.x
            );
            // The control: a strip drawn lit whole is read to its end.
            let whole = testing::pass_over(BENCH, scale, |ui| {
                paint::spectrum(
                    ui.painter(),
                    strip,
                    1.0,
                    |color| Palette::DARK.veiled(color),
                    scale_at,
                );
            });
            let end = testing::lit_edge(&whole.shapes, strip, scale_at);
            assert!(
                end >= strip.max.x - pixel,
                "strip {row}: lit whole to {end}"
            );
            // The control: a strip drawn lit to a tenth more is read as such,
            // so the reading is not within a pixel of anything.
            let longer = testing::pass_over(BENCH, scale, |ui| {
                paint::spectrum(
                    ui.painter(),
                    strip,
                    pair.share() + 0.1,
                    |color| Palette::DARK.veiled(color),
                    scale_at,
                );
            });
            let other = testing::lit_edge(&longer.shapes, strip, scale_at);
            assert!((other - wanted).abs() > pixel, "strip {row}: {other}");
        }
    }

    #[test]
    fn the_plate_of_figures_lays_out_three_readouts_three_rows_and_a_footer_and_paints_no_image() {
        let pass = pass_of(BENCH_PLATE, 2.0);
        assert!(images(&pass).is_empty());
        let baseline = |text: &egui::epaint::TextShape| text.pos.y + baseline_of(&text.galley);
        let lowest_row = ROW_FIRST + ROW_STEP * 2.0;
        let (mut readouts, mut rows, mut footer) = (vec![], vec![vec![]; 3], vec![]);
        for text in texts(&pass) {
            let y = baseline(text);
            if y < ROW_FIRST - 20.0 {
                readouts.push(text);
            } else if y > lowest_row + 20.0 {
                footer.push(text);
            } else {
                let row = ((y - ROW_FIRST + 20.0) / ROW_STEP).floor() as usize;
                rows[row].push(text);
            }
        }
        // Each readout has a figure, its caption and a line, in a column of its own.
        for column in 0..3 {
            let x = BENCH_MARGIN + READOUT_STEP * column as f32;
            let lines = readouts
                .iter()
                .filter(|text| (text.pos.x - x).abs() < 1.0)
                .count();
            assert_eq!(lines, 3, "readout {column}");
        }
        assert_eq!(readouts.len(), 9);
        // Each row has its label and its line, the earlier reading, and the
        // latest with its unit.
        for (row, texts) in rows.iter().enumerate() {
            assert_eq!(texts.len(), 5, "row {row}");
        }
        assert_eq!(footer.len(), 1);
    }

    #[test]
    fn nothing_on_the_plate_of_figures_overlaps_or_overruns() {
        let scale = 2.0;
        // Half a device pixel: where text is put on whole pixels.
        let snap = 0.5 / scale;
        let pass = pass_of(BENCH_PLATE, scale);
        let right = BENCH.x - BENCH_MARGIN;
        let strip_right = STRIP_LEFT + STRIP_LENGTH;
        // The text's left and right edge, and its baseline.
        let placed = |text: &egui::epaint::TextShape| {
            let width = text.galley.size().x;
            (
                text.pos.x,
                text.pos.x + width,
                text.pos.y + baseline_of(&text.galley),
            )
        };
        let spans: Vec<(f32, f32, f32)> = texts(&pass).into_iter().map(placed).collect();
        // The readouts: each ends before the next column starts, and the last
        // before the right margin.
        for &(left, end, y) in spans.iter().filter(|s| s.2 < ROW_FIRST - 20.0) {
            let column = ((left - BENCH_MARGIN) / READOUT_STEP).round();
            let limit = if column < 2.0 {
                BENCH_MARGIN + READOUT_STEP * (column + 1.0)
            } else {
                right
            };
            assert!(
                end < limit,
                "a readout at {left}, baseline {y}, ends at {end}, past {limit}"
            );
        }
        // The rows: a label on the baseline of the strip, the value and its
        // unit 6 points under it, and a line and the earlier reading 17.
        for row in 0..3 {
            let y = ROW_FIRST + ROW_STEP * row as f32;
            // The texts on a baseline, left to right.
            let on = |down: f32| {
                let mut found: Vec<_> = (spans.iter())
                    .filter(|s| (s.2 - (y + down)).abs() < 1.0)
                    .copied()
                    .collect();
                found.sort_by(|a, b| a.0.total_cmp(&b.0));
                found
            };
            let (label, lines, values) = (on(0.0), on(17.0), on(6.0));
            assert_eq!(
                (label.len(), lines.len(), values.len()),
                (1, 2, 2),
                "row {row}"
            );
            // The label and its line stand on the left, at least 16 points short
            // of the strip; the earlier reading ends on the strip's right end
            // and begins after the line.
            let (how, was) = (lines[0], lines[1]);
            for (what, (_, end, _)) in [("label", label[0]), ("line", how)] {
                assert!(
                    end + 16.0 <= STRIP_LEFT,
                    "row {row}: the {what} ends at {end}"
                );
            }
            assert!(
                (was.1 - strip_right).abs() <= snap,
                "row {row}: `was` ends at {}",
                was.1
            );
            assert!(how.1 < was.0, "row {row}: the line runs into `was`");
            // The value and its unit stand on the right: from the strip's end to
            // the margin, the unit last.
            let (value, unit) = (values[0], values[1]);
            assert!(
                (unit.1 - right).abs() <= snap,
                "row {row}: the unit ends at {}",
                unit.1
            );
            assert!(
                value.1 <= unit.0 + snap,
                "row {row}: the value runs into its unit"
            );
            assert!(
                value.0 > strip_right,
                "row {row}: the value starts at {}",
                value.0
            );
        }
    }

    /// The crates of the lines of `cargo tree --prefix none --format '{p}'`:
    /// each line's `name vVERSION`, cut where its ` (` begins (a path, a
    /// proc-macro mark, the `(*)` of a subtree shown before), once each. Two
    /// versions of a crate count apart.
    fn crates_of_tree(tree: &str) -> std::collections::BTreeSet<String> {
        (tree.lines())
            .map(|line| line.split(" (").next().unwrap_or(line).trim())
            .filter(|krate| !krate.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// The crates among `crates` whose name is a network crate or an async
    /// runtime, as the comment of `FIGURES.network_and_async` names them: a
    /// whole name, so that `smol_str` is not `smol`.
    fn network_or_async(crates: &std::collections::BTreeSet<String>) -> Vec<&str> {
        const NAMED: [&str; 15] = [
            "tokio",
            "async-std",
            "smol",
            "async-executor",
            "futures-executor",
            "futures",
            "hyper",
            "reqwest",
            "ureq",
            "curl",
            "h2",
            "rustls",
            "native-tls",
            "socket2",
            "mio",
        ];
        (crates.iter())
            .map(String::as_str)
            .filter(|krate| NAMED.contains(&krate.split(' ').next().unwrap_or(krate)))
            .collect()
    }

    #[test]
    fn the_filter_finds_a_runtime_and_a_network_crate_in_a_tree_that_has_them() {
        let tree = "\
arrakis v0.1.0 (/somewhere/arrakis)
eframe v0.34.1
smol_str v0.3.2
pollster v0.4.0
serde_derive v1.0.228 (proc-macro)
mio v1.0.4
tokio v1.47.1
mio v1.0.4 (*)
smol_str v0.3.2 (*)
mio v0.8.11

eframe v0.34.1 (*)
";
        let crates = crates_of_tree(tree);
        // The path, the proc-macro mark and the repeated `(*)` are cut, a
        // second version of a crate counts apart, and the blank line that
        // `cargo tree` prints between the trees of a workspace's members is
        // no crate.
        assert_eq!(crates.len(), 8, "{crates:?}");
        assert!(crates.contains("serde_derive v1.0.228"));
        assert_eq!(
            network_or_async(&crates),
            ["mio v0.8.11", "mio v1.0.4", "tokio v1.47.1"]
        );
        // A tree with neither finds nothing.
        let calm = crates_of_tree("arrakis v0.1.0 (/x)\nsmol_str v0.3.2\npollster v0.4.0\n");
        assert!(network_or_async(&calm).is_empty());
    }

    /// The plate's "169 crates in the tree" and its zero network crates and
    /// async runtimes are the build's: this reads the normal tree of the Mac's
    /// target from `Cargo.lock`, as the comment of `FIGURES.network_and_async`
    /// does. `--offline` keeps it off the network; it builds nothing and
    /// writes nothing.
    #[test]
    fn the_plates_crate_count_and_its_zero_are_the_builds_normal_tree() {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = match std::process::Command::new(&cargo)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(["tree", "-e", "normal", "--locked", "--offline"])
            .args(["--target", "aarch64-apple-darwin"])
            .args(["--prefix", "none", "--format", "{p}"])
            .output()
        {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipped: no cargo to read the tree with ({error})");
                return;
            }
            Err(error) => panic!("cargo tree did not start: {error}"),
        };
        assert!(
            output.status.success(),
            "cargo tree failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let tree = String::from_utf8(output.stdout).expect("cargo tree prints UTF-8");
        let crates = crates_of_tree(&tree);
        let found = network_or_async(&crates);
        let bring_up_to_date = "Bring up to date FIGURES.tree_crates and \
            FIGURES.network_and_async in src/demo.rs, with the count in their comment, then \
            take docs/bench.png again (`--demo-scene bench`) and bring up to date the alt text \
            of that picture in the README";
        assert_eq!(
            crates.len() as u32,
            FIGURES.tree_crates,
            "{} distinct crates in the build's normal tree, and the plate says {}. \
            {bring_up_to_date}",
            crates.len(),
            FIGURES.tree_crates
        );
        assert!(
            found.is_empty(),
            "a network crate or an async runtime is in the tree: {found:?}. {bring_up_to_date}"
        );
        assert_eq!(
            found.len() as u32,
            FIGURES.network_and_async,
            "{bring_up_to_date}"
        );
    }

    #[test]
    fn the_other_scenes_leave_the_keyboard_alone() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut app = test_app(clock);
        let path = std::env::temp_dir().join("arrakis-never-saved.png");
        let mut shot = Shot::new(path, Scene::Night);
        for _ in 0..3 {
            assert_ne!(
                focused_label(&ctx, &mut shot, &mut app).as_deref(),
                Some("Temperature: 1200 K, Ember")
            );
        }
    }
}
