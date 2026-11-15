//! What the tests of the drawing share: a frame of a context, and the
//! shapes it leaves, in the order they were painted; and the whole window
//! through a series of frames.

use egui::epaint::{CircleShape, PathShape, RectShape, TextShape, Vertex};
use egui::{
    Color32, Context, Event, FullOutput, Id, Key, Mesh, Modifiers, Painter, PointerButton, Pos2,
    RawInput, Rect, Shape, Stroke, Ui,
};

use crate::app::{App, test_app};
use crate::color::Look;
use crate::theme;
use crate::ui::draw;

/// Every shape in `shapes`, in paint order, with the nested ones laid flat.
fn flat(shapes: impl IntoIterator<Item = Shape>) -> Vec<Shape> {
    let mut out = vec![];
    for shape in shapes {
        match shape {
            Shape::Vec(inner) => out.extend(flat(inner)),
            shape => out.push(shape),
        }
    }
    out
}

/// The meshes among `shapes`.
pub fn meshes(shapes: &[Shape]) -> Vec<&Mesh> {
    shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Mesh(mesh) => Some(&**mesh),
            _ => None,
        })
        .collect()
}

/// The filled and stroked circles among `shapes`.
pub fn circles(shapes: &[Shape]) -> Vec<&CircleShape> {
    shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Circle(circle) => Some(circle),
            _ => None,
        })
        .collect()
}

/// The rectangles among `shapes`, filled or stroked.
pub fn rects(shapes: &[Shape]) -> Vec<&RectShape> {
    shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Rect(rect) => Some(rect),
            _ => None,
        })
        .collect()
}

/// The text among `shapes`.
pub fn texts(shapes: &[Shape]) -> Vec<&TextShape> {
    shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Text(text) => Some(text),
            _ => None,
        })
        .collect()
}

/// The paths among `shapes`: polygons and polylines, filled or stroked.
pub fn paths(shapes: &[Shape]) -> Vec<&PathShape> {
    shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Path(path) => Some(path),
            _ => None,
        })
        .collect()
}

/// The line segments among `shapes`, with their strokes.
pub fn segments(shapes: &[Shape]) -> Vec<([Pos2; 2], Stroke)> {
    shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::LineSegment { points, stroke } => Some((*points, *stroke)),
            _ => None,
        })
        .collect()
}

/// The color a text was laid out in. A fade of the painter (`Ui::disable`,
/// `Ui::set_opacity`) never touches it: [`drawn`] reads what reached the
/// screen.
pub fn ink(text: &TextShape) -> Color32 {
    text.galley.job.sections[0].format.color
}

/// The color a text is drawn in: its glyphs' vertices, where the painter's
/// opacity lands (`epaint::shape_transform::adjust_colors`), and the shape's
/// own opacity on top. A text has one color, so a vertex of another is a
/// failure.
pub fn drawn(text: &TextShape) -> Color32 {
    let mut colors = text
        .galley
        .rows
        .iter()
        .flat_map(|row| row.visuals.mesh.vertices.iter().map(|vertex| vertex.color));
    let first = colors.next().expect("a text with glyphs has vertices");
    for color in colors {
        assert_eq!(
            color,
            first,
            "{:?} is drawn in two colors",
            text.galley.text()
        );
    }
    first.gamma_multiply(text.opacity_factor)
}

/// Where a text's middle fell.
pub fn middle(text: &TextShape) -> Pos2 {
    text.pos + text.galley.size() / 2.0
}

/// The corners of the triangles of a strip's mesh that lie left of `split`
/// and right of it. A triangle across `split` is a soft edge, and fails.
pub fn sides(mesh: &Mesh, split: f32) -> (Vec<Vertex>, Vec<Vertex>) {
    let (mut left, mut right) = (vec![], vec![]);
    for triangle in mesh.indices.chunks(3) {
        let corners =
            [triangle[0], triangle[1], triangle[2]].map(|index| mesh.vertices[index as usize]);
        let xs = corners.map(|corner| corner.pos.x);
        let (least, most) = (
            xs.into_iter().fold(f32::MAX, f32::min),
            xs.into_iter().fold(f32::MIN, f32::max),
        );
        if most <= split + 1e-3 {
            left.extend(corners);
        } else if least >= split - 1e-3 {
            right.extend(corners);
        } else {
            panic!("a triangle from {least} to {most} crosses the edge at {split}");
        }
    }
    (left, right)
}

/// Where the lit part of the strip over `strip` ends, read off what was
/// painted: the right-most place that shows a color `color_at` gives its
/// position, which a veiled color never is. A round end shows only beside the
/// body, and only as far as its clip rectangle. The edge is `strip`'s left
/// edge when nothing is lit.
pub fn lit_edge(shapes: &[(Rect, Shape)], strip: Rect, color_at: impl Fn(f32) -> Color32) -> f32 {
    let radius = strip.height() / 2.0;
    let (first, last) = (strip.min.x + radius, strip.max.x - radius);
    let position = |x: f32| (x - strip.min.x) / strip.width();
    let mut edge = strip.min.x;
    for (clip, shape) in shapes {
        match shape {
            Shape::Mesh(mesh) => {
                for vertex in &mesh.vertices {
                    let on_strip = (strip.min.y..=strip.max.y).contains(&vertex.pos.y);
                    if on_strip && vertex.color == color_at(position(vertex.pos.x)) {
                        edge = edge.max(vertex.pos.x);
                    }
                }
            }
            Shape::Circle(disc) if disc.radius == radius && disc.center.y == strip.center().y => {
                let at = position(disc.center.x).round();
                if disc.fill != color_at(at) {
                    continue;
                }
                let (from, to) = if at == 0.0 {
                    (strip.min.x, first)
                } else {
                    (last, strip.max.x)
                };
                let reach = (disc.center.x + disc.radius).min(clip.max.x).min(to);
                if reach > from {
                    edge = edge.max(reach);
                }
            }
            _ => {}
        }
    }
    edge
}

/// A context with the window's fonts and style.
pub fn context() -> Context {
    let ctx = Context::default();
    theme::install(&ctx);
    ctx
}

/// Runs one frame of `ctx` over a window-sized screen with `events`, and
/// returns all that `draw` left of it: its shapes, and the screen reader's
/// tree if `ctx` has it on.
pub fn output(ctx: &Context, events: Vec<Event>, draw: impl FnMut(&mut Ui)) -> FullOutput {
    run(ctx, None, true, events, draw)
}

/// Like [`output`], at `time` seconds. Without it egui takes a frame to be
/// 1/60 s after the last, which the tests of anything timed cannot use.
pub fn output_at(
    ctx: &Context,
    time: f64,
    events: Vec<Event>,
    draw: impl FnMut(&mut Ui),
) -> FullOutput {
    run(ctx, Some(time), true, events, draw)
}

/// Like [`output_at`], with the window not focused: the frame's
/// `RawInput::focused` is `false`, as winit reports a window that lost the
/// keyboard. `Event::WindowFocused` alone does not change what
/// `InputState::focused` reads, so a test of an unfocused frame needs this.
pub fn output_unfocused_at(
    ctx: &Context,
    time: f64,
    events: Vec<Event>,
    draw: impl FnMut(&mut Ui),
) -> FullOutput {
    run(ctx, Some(time), false, events, draw)
}

fn run(
    ctx: &Context,
    time: Option<f64>,
    focused: bool,
    events: Vec<Event>,
    mut draw: impl FnMut(&mut Ui),
) -> FullOutput {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, theme::WINDOW)),
        time,
        focused,
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| draw(ui));
    // No renderer here to take the font atlas.
    output.textures_delta.clear();
    output
}

/// The shapes in `output`, in paint order, with the nested ones laid flat.
/// They leave `output`.
pub fn shapes(output: &mut FullOutput) -> Vec<Shape> {
    flat(
        std::mem::take(&mut output.shapes)
            .into_iter()
            .map(|clipped| clipped.shape),
    )
}

/// What a screen reader is told in `output`: each label, with where its
/// widget is when the tree says. The context must have been asked for the
/// tree with `enable_accesskit`. The tree leaves `output`.
pub fn told(output: &mut FullOutput) -> Vec<(String, Option<Rect>)> {
    let update = output
        .platform_output
        .accesskit_update
        .take()
        .expect("accesskit tree");
    update
        .nodes
        .iter()
        .filter_map(|(_, node)| {
            let bounds = node.bounds().map(|b| {
                Rect::from_min_max(
                    Pos2::new(b.x0 as f32, b.y0 as f32),
                    Pos2::new(b.x1 as f32, b.y1 as f32),
                )
            });
            Some((node.label()?.to_owned(), bounds))
        })
        .collect()
}

/// The labels a screen reader is told in `output` (see [`told`]).
pub fn labels(output: &mut FullOutput) -> Vec<String> {
    told(output).into_iter().map(|(label, _)| label).collect()
}

/// The primary button going down, or coming up, at `at`.
pub fn button(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// `key` going down, with `modifiers` held.
pub fn key_down(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// Runs one frame of `ctx` as [`output`] does, and returns what `draw`
/// painted.
pub fn frame(ctx: &Context, events: Vec<Event>, draw: impl FnMut(&mut Ui)) -> Vec<Shape> {
    shapes(&mut output(ctx, events, draw))
}

/// Like [`frame`], with the clip rectangle each shape was painted under.
pub fn frame_clipped(
    ctx: &Context,
    events: Vec<Event>,
    draw: impl FnMut(&mut Ui),
) -> Vec<(Rect, Shape)> {
    clipped(output(ctx, events, draw))
}

/// The shapes of `output` in paint order, each with the clip rectangle it
/// was painted under.
fn clipped(output: FullOutput) -> Vec<(Rect, Shape)> {
    let mut out = vec![];
    for clipped in output.shapes {
        out.extend(
            flat([clipped.shape])
                .into_iter()
                .map(|shape| (clipped.clip_rect, shape)),
        );
    }
    out
}

/// A context set to `pixels_per_point`, through one frame.
fn scaled_context(pixels_per_point: f32) -> Context {
    let ctx = context();
    ctx.set_pixels_per_point(pixels_per_point);
    // The scale takes hold at the start of the next pass.
    frame(&ctx, vec![], |_| {});
    ctx
}

/// What `paint` draws on a painter of a screen at `pixels_per_point`.
pub fn painted(pixels_per_point: f32, paint: impl Fn(&Painter)) -> Vec<Shape> {
    let ctx = scaled_context(pixels_per_point);
    frame(&ctx, vec![], |ui| {
        assert_eq!(ui.painter().pixels_per_point(), pixels_per_point);
        paint(ui.painter());
    })
}

/// Like [`painted`], with the clip rectangle each shape was painted under.
pub fn painted_clipped(pixels_per_point: f32, paint: impl Fn(&Painter)) -> Vec<(Rect, Shape)> {
    let ctx = scaled_context(pixels_per_point);
    frame_clipped(&ctx, vec![], |ui| {
        assert_eq!(ui.painter().pixels_per_point(), pixels_per_point);
        paint(ui.painter());
    })
}

/// What one pass of a plate's drawing leaves: its shapes in paint order with
/// the clip rectangle of each, and the textures it sets, which a plate that
/// draws an image makes.
#[cfg(feature = "demo")]
pub struct Pass {
    pub shapes: Vec<(Rect, Shape)>,
    pub textures: Vec<(egui::TextureId, egui::epaint::ImageDelta)>,
}

/// One pass of `draw` over a screen of `size` points at `pixels_per_point`,
/// the size of a plate's window, with the window's fonts. A pass before it
/// lets the scale take hold, as [`painted`]'s does.
#[cfg(feature = "demo")]
pub fn pass_over(size: egui::Vec2, pixels_per_point: f32, mut draw: impl FnMut(&mut Ui)) -> Pass {
    let ctx = context();
    ctx.set_pixels_per_point(pixels_per_point);
    frame(&ctx, vec![], |_| {});
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        focused: true,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| draw(ui));
    // A delta that is dropped with anything in it panics in debug builds:
    // take it apart, as `Headless::frame` does.
    let textures = (std::mem::take(&mut output.textures_delta.set).into_iter())
        .flat_map(|(id, deltas)| deltas.into_iter().map(move |delta| (id, delta)))
        .collect();
    output.textures_delta.clear();
    Pass {
        shapes: clipped(output),
        textures,
    }
}

/// The README's banner at two pixels to the point, as the scene draws it:
/// `demo::banner_picture` makes it once for the whole test binary, since
/// building the cover's layers at 600 pixels takes a moment in a test build.
#[cfg(feature = "demo")]
pub fn banner_picture_2x() -> std::sync::Arc<egui::ColorImage> {
    crate::demo::banner_picture(2.0)
}

/// The clock of the app a [`Screen`] shows: noon.
fn noon() -> f64 {
    12.0 * 60.0
}

/// The whole window (`ui::draw` on a test app) through a series of frames of
/// one context, so that hover, focus and a pointer's press carry from one
/// frame to the next.
pub struct Screen {
    pub ctx: Context,
    pub app: App,
    /// The id of the last frame's root `Ui`, which the drag strip's widget is
    /// named under.
    root: Id,
}

impl Screen {
    /// The window at noon with the night look `night`.
    pub fn new(night: Look) -> Self {
        let mut app = test_app(noon);
        app.settings.night = night;
        Self::of(app)
    }

    /// The window of `app`, after two frames: the first lays the controls
    /// out, so that the pointer finds them, and the second lets the switch
    /// settle.
    pub fn of(app: App) -> Self {
        let mut screen = Self {
            ctx: context(),
            app,
            root: Id::NULL,
        };
        screen.output(vec![]);
        screen.output(vec![]);
        screen
    }

    /// The screen reader is on, so that each frame's output carries its tree.
    pub fn with_screen_reader(self) -> Self {
        self.ctx.enable_accesskit();
        self
    }

    /// One frame with `events`, and all it leaves.
    pub fn output(&mut self, events: Vec<Event>) -> FullOutput {
        let (app, root) = (&mut self.app, &mut self.root);
        output(&self.ctx, events, |ui| {
            *root = ui.id();
            draw(app, ui);
        })
    }

    /// One frame with `events`, and the shapes it painted.
    pub fn frame(&mut self, events: Vec<Event>) -> Vec<Shape> {
        shapes(&mut self.output(events))
    }

    /// What a frame at rest paints.
    pub fn at_rest(&mut self) -> Vec<Shape> {
        self.frame(vec![])
    }

    /// A frame at rest, with the clip rectangle of each shape.
    pub fn at_rest_clipped(&mut self) -> Vec<(Rect, Shape)> {
        clipped(self.output(vec![]))
    }

    /// The pointer moves to `at` and rests there.
    pub fn hover(&mut self, at: Pos2) -> Vec<Shape> {
        self.frame(vec![Event::PointerMoved(at)]);
        self.at_rest()
    }

    /// The pointer moves to `at` and presses.
    pub fn press(&mut self, at: Pos2) {
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![button(at, true)]);
    }

    /// The pointer lets go at `at`.
    pub fn release(&mut self, at: Pos2) {
        self.frame(vec![button(at, false)]);
    }

    /// The pointer presses at `from` and moves to `to`, in hand: the frame of
    /// the move.
    pub fn drag(&mut self, from: Pos2, to: Pos2) -> Vec<Shape> {
        let clipped = self.drag_clipped(from, to);
        clipped.into_iter().map(|(_, shape)| shape).collect()
    }

    /// Like [`Self::drag`], with the clip rectangle of each shape.
    pub fn drag_clipped(&mut self, from: Pos2, to: Pos2) -> Vec<(Rect, Shape)> {
        self.press(from);
        clipped(self.output(vec![Event::PointerMoved(to)]))
    }

    /// The id of the drag strip's widget.
    pub fn strip(&self) -> Id {
        self.root.with("fastframe-titlebar-drag")
    }

    /// The widget `id` has the pointer's button down on it.
    pub fn pressed(&self, id: Id) -> bool {
        self.ctx
            .read_response(id)
            .is_some_and(|response| response.is_pointer_button_down_on())
    }
}
