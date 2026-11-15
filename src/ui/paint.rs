//! Drawing helpers: hairlines on whole device pixels, small glyphs drawn
//! as strokes, text placed by an anchor.

use std::sync::Arc;

use egui::text::{LayoutJob, TextWrapping};
use egui::{Align2, Color32, FontId, Galley, Mesh, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

use crate::theme::{
    SLIDER_TICK_WIDTH, SUNDOWN_LINE, SUNDOWN_LINE_DROP, SUNDOWN_LINE_WIDTH, SUNDOWN_RADIUS,
    SUNDOWN_SIDE,
};

/// Text with its `anchor` at `pos`; returns where it landed.
pub fn text(
    painter: &Painter,
    pos: Pos2,
    anchor: Align2,
    text: &str,
    font: FontId,
    color: Color32,
) -> Rect {
    painter.text(pos, anchor, text, font, color)
}

/// The size `text` takes in `font`.
pub fn measure(painter: &Painter, text: &str, font: FontId) -> egui::Vec2 {
    painter
        .layout_no_wrap(text.to_owned(), font, Color32::WHITE)
        .size()
}

/// The widest advance of the digits 0 to 9 in `font`. The face's digits are
/// not of one width (a 1 is narrower than an 8), so a box that must not move
/// while its number changes is as wide as its count of digits times this. It
/// reads ten glyphs and lays no text out.
pub fn widest_digit(painter: &Painter, font: &FontId) -> f32 {
    painter.ctx().fonts_mut(|fonts| {
        ('0'..='9')
            .map(|digit| fonts.glyph_width(font, digit))
            .fold(0.0, f32::max)
    })
}

/// `text` on one row no wider than `width`: what does not fit is cut and
/// ends in an ellipsis.
pub fn fit(painter: &Painter, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping::truncate_at_width(width);
    painter.layout_job(job)
}

/// Like [`text`], for a line that may be longer than `width`: see [`fit`].
pub fn text_fit(
    painter: &Painter,
    pos: Pos2,
    anchor: Align2,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> Rect {
    let galley = fit(painter, text, font, color, width);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

/// A horizontal line one device pixel tall, from `left` to `right`, with
/// its top at `y`.
pub fn hairline(painter: &Painter, left: f32, right: f32, y: f32, color: Color32) {
    let pixel = 1.0 / painter.pixels_per_point();
    painter.rect_filled(
        Rect::from_min_max(pos2(left, y), pos2(right, y + pixel)),
        0,
        color,
    );
}

/// A vertical mark [`SLIDER_TICK_WIDTH`] wide and `height` tall, whose edges
/// fall on whole device pixels, nearest to centered at `x`. Returns where its
/// center fell.
pub fn tick(painter: &Painter, x: f32, top: f32, height: f32, color: Color32) -> f32 {
    let pixels = painter.pixels_per_point();
    let half = SLIDER_TICK_WIDTH / 2.0;
    let left = ((x - half) * pixels).round() / pixels;
    painter.rect_filled(
        Rect::from_min_size(pos2(left, top), vec2(SLIDER_TICK_WIDTH, height)),
        0,
        color,
    );
    left + half
}

/// Where a strip over `rect` turns from lit to veiled, and the pieces it is
/// made of: a body between the centers of its round ends, and the ends.
struct Strip {
    rect: Rect,
    /// Half the strip's height: the ends' radius.
    radius: f32,
    /// The x of the left end's center and of the right end's.
    first: f32,
    last: f32,
    /// The x where the lit part ends.
    split: f32,
}

impl Strip {
    fn new(rect: Rect, value: f32) -> Self {
        let radius = rect.height() / 2.0;
        Self {
            rect,
            radius,
            first: rect.min.x + radius,
            last: rect.max.x - radius,
            split: rect.min.x + rect.width() * value,
        }
    }

    /// The strip's position, from 0 at its left to 1 at its right, at `x`.
    fn position(&self, x: f32) -> f32 {
        ((x - self.rect.min.x) / self.rect.width()).clamp(0.0, 1.0)
    }

    /// The round ends: a disc under each end of the body, whose part outside
    /// the body is lit up to the split and veiled after it. An end that the
    /// split falls in is drawn veiled, with its lit part over it, clipped at
    /// the split, so the edge sits at the value there too.
    fn ends(
        &self,
        painter: &Painter,
        veil: &impl Fn(Color32) -> Color32,
        color_at: &impl Fn(f32) -> Color32,
    ) {
        let y = self.rect.center().y;
        // Each end's center, the stretch of it that shows beside the body, and
        // its position on the strip.
        for (x, shows, at) in [
            (self.first, (self.rect.min.x, self.first), 0.0),
            (self.last, (self.last, self.rect.max.x), 1.0),
        ] {
            let (center, lit) = (pos2(x, y), color_at(at));
            if self.split >= shows.1 {
                painter.circle_filled(center, self.radius, lit);
                continue;
            }
            painter.circle_filled(center, self.radius, veil(lit));
            if self.split > shows.0 {
                painter
                    .with_clip_rect(Rect::everything_left_of(self.split))
                    .circle_filled(center, self.radius, lit);
            }
        }
    }
}

/// A strip over `rect` whose color at each position, from 0 at its left to
/// 1 at its right, is `color_at`: lit up to `value` and `veil`ed after it,
/// with a hard edge between the two. Its ends are round, of half the strip's
/// height.
///
/// The body is one mesh with a column of vertices for each point, shared by
/// the columns beside it, and a color for each: `color_at` runs once for each
/// column edge. Only the hard edge doubles its pair of vertices, one lit and
/// one veiled. The mesh's top and bottom edges fall on `rect`'s, so give it
/// whole points and it needs no antialiasing; the discs that round the ends
/// are drawn under it.
pub fn spectrum(
    painter: &Painter,
    rect: Rect,
    value: f32,
    veil: impl Fn(Color32) -> Color32,
    color_at: impl Fn(f32) -> Color32,
) {
    let strip = Strip::new(rect, value);
    let Strip {
        first, last, split, ..
    } = strip;
    strip.ends(painter, &veil, &color_at);
    // The columns are a point wide, the last one shorter if the body is not
    // a whole number of points.
    let columns = (last - first).ceil().max(1.0) as usize;
    // The column the hard edge falls in, if it falls inside the body.
    let edge_in = (split > first && split < last).then_some((split - first) as usize);
    // An edge is lit when the lit part reaches it.
    let lit = |x: f32| split >= last || (split > first && x <= split);
    let mut mesh = Mesh::default();
    let mut pair = |x: f32, color: Color32, joined: bool| {
        let at = mesh.vertices.len() as u32;
        mesh.colored_vertex(pos2(x, rect.min.y), color);
        mesh.colored_vertex(pos2(x, rect.max.y), color);
        if joined {
            mesh.add_triangle(at - 2, at, at + 1);
            mesh.add_triangle(at - 2, at + 1, at - 1);
        }
    };
    for column in 0..=columns {
        let x = (first + column as f32).min(last);
        let color = color_at(strip.position(x));
        pair(x, if lit(x) { color } else { veil(color) }, column > 0);
        if edge_in == Some(column) {
            // The lit side ends at the split, and the veiled side begins
            // there, with no column between them.
            let color = if split > x {
                let color = color_at(strip.position(split));
                pair(split, color, true);
                color
            } else {
                color
            };
            pair(split, veil(color), false);
        }
    }
    painter.add(mesh);
}

/// A strip like [`spectrum`], in one `color` along its whole length: the
/// lit part and the veiled part are a rectangle each, with no color to work
/// out for each point.
pub fn flat(
    painter: &Painter,
    rect: Rect,
    value: f32,
    veil: impl Fn(Color32) -> Color32,
    color: Color32,
) {
    let strip = Strip::new(rect, value);
    strip.ends(painter, &veil, &|_| color);
    let split = strip.split.clamp(strip.first, strip.last);
    let mut mesh = Mesh::default();
    for (from, to, color) in [
        (strip.first, split, color),
        (split, strip.last, veil(color)),
    ] {
        if to > from {
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(from, rect.min.y), pos2(to, rect.max.y)),
                color,
            );
        }
    }
    painter.add(mesh);
}

/// A chevron pointing up or down, `width` wide and `height` tall, centered
/// at `center`.
pub fn chevron(painter: &Painter, center: Pos2, width: f32, height: f32, up: bool, stroke: Stroke) {
    let half = width / 2.0;
    let rise = if up { -height / 2.0 } else { height / 2.0 };
    painter.add(Shape::line(
        vec![
            center + vec2(-half, -rise),
            center + vec2(0.0, rise),
            center + vec2(half, -rise),
        ],
        stroke,
    ));
}

/// How many segments round the Sundown glyph's sun arc.
const SUNDOWN_ARC: usize = 24;

/// Red Now's glyph: the sun half sunk behind a line, as the menu-bar mark is,
/// drawn in egui shapes at a key's size and centered on `center`.
pub fn sundown(painter: &Painter, center: Pos2, ink: Color32) {
    let horizon = center.y + SUNDOWN_SIDE;
    let sun = pos2(center.x, horizon);
    let points: Vec<Pos2> = (0..=SUNDOWN_ARC)
        .map(|step| {
            let angle = std::f32::consts::PI * (1.0 + step as f32 / SUNDOWN_ARC as f32);
            sun + SUNDOWN_RADIUS * vec2(angle.cos(), angle.sin())
        })
        .collect();
    painter.add(Shape::convex_polygon(points, ink, Stroke::NONE));
    let line = horizon + SUNDOWN_LINE_DROP;
    painter.line_segment(
        [
            pos2(center.x - SUNDOWN_LINE / 2.0, line),
            pos2(center.x + SUNDOWN_LINE / 2.0, line),
        ],
        Stroke::new(SUNDOWN_LINE_WIDTH, ink),
    );
}

/// `from` moved `t` of the way to `to`, in sRGB.
pub fn mix(from: Color32, to: Color32, t: f32) -> Color32 {
    from.lerp_to_gamma(to, t.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::color::{self, Tint};
    use crate::theme::Palette;
    use crate::ui::testing::{
        self, circles, meshes, painted, painted_clipped, paths, rects, segments, sides,
    };

    /// The Temperature strip: 6 points tall over the window's margins, on
    /// whole points, as a center line at y 292 gives it: not where the
    /// window puts the Temperature row, since the strip is drawn alone here.
    const STRIP: Rect = Rect {
        min: pos2(24.0, 289.0),
        max: pos2(396.0, 295.0),
    };

    /// What white becomes at a position of the scale, on a neutral screen.
    fn scale(position: f32) -> Color32 {
        color::scale_tint(position)
            .through(Tint::NEUTRAL)
            .apply(Color32::WHITE)
    }

    fn veil(color: Color32) -> Color32 {
        Palette::DARK.veiled(color)
    }

    /// The strip's position at a point of x.
    fn position(x: f32) -> f32 {
        (x - STRIP.min.x) / STRIP.width()
    }

    fn strip(value: f32) -> Vec<Shape> {
        painted(1.0, |painter| spectrum(painter, STRIP, value, veil, scale))
    }

    #[test]
    fn the_strip_shows_each_position_in_its_own_color() {
        let shapes = strip(1.0);
        let mesh = meshes(&shapes)[0];
        // The mesh has a column of colors for each point, so a vertex falls
        // on each of these positions.
        for at in [0.25, 0.5, 0.75] {
            let x = STRIP.min.x + STRIP.width() * at;
            let seen: Vec<Color32> = mesh
                .vertices
                .iter()
                .filter(|vertex| vertex.pos.x == x)
                .map(|vertex| vertex.color)
                .collect();
            assert!(!seen.is_empty(), "no vertex at x {x}");
            assert!(
                seen.iter().all(|color| *color == scale(at)),
                "at {at}: {seen:?}, not {:?}",
                scale(at)
            );
        }
        // Every vertex is the scale at its own x, whole strip lit.
        for vertex in &mesh.vertices {
            assert_eq!(
                vertex.color,
                scale(position(vertex.pos.x)),
                "at x {}",
                vertex.pos.x
            );
        }
        // The ends are the discs, in the scale's first and last colors:
        // white, and pure red. The middle is orange.
        let ends: Vec<Color32> = circles(&shapes).iter().map(|c| c.fill).collect();
        assert_eq!(
            ends,
            [
                Color32::from_rgb(0xFF, 0xFF, 0xFF),
                Color32::from_rgb(0xFF, 0x00, 0x00)
            ]
        );
        let middle = mesh.vertices.iter().find(|v| v.pos.x == 210.0).unwrap();
        assert_eq!(middle.color, Color32::from_rgb(0xFF, 0x80, 0x00));
    }

    #[test]
    fn the_strip_has_round_ends_and_edges_on_whole_points() {
        let shapes = strip(1.0);
        let mesh = meshes(&shapes)[0];
        // Top and bottom vertices fall on the rect's own edges, which are
        // whole points, so the mesh needs no antialiasing.
        for vertex in &mesh.vertices {
            assert!(
                vertex.pos.y == STRIP.min.y || vertex.pos.y == STRIP.max.y,
                "a vertex at y {}",
                vertex.pos.y
            );
        }
        assert_eq!(STRIP.min.y.fract(), 0.0);
        assert_eq!(STRIP.max.y.fract(), 0.0);
        // The mesh runs between the discs' centers, 3 points in from the
        // ends.
        let (least, most) = mesh
            .vertices
            .iter()
            .fold((f32::MAX, f32::MIN), |(l, m), v| {
                (l.min(v.pos.x), m.max(v.pos.x))
            });
        assert_eq!((least, most), (27.0, 393.0));
        // Two discs of radius 3, drawn under the mesh.
        let discs = circles(&shapes);
        assert_eq!(discs.len(), 2);
        for (disc, x) in discs.iter().zip([27.0, 393.0]) {
            assert_eq!(disc.radius, 3.0);
            assert_eq!(disc.center, pos2(x, 292.0));
        }
        let order: Vec<&str> = shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Circle(_) => Some("disc"),
                Shape::Mesh(_) => Some("mesh"),
                _ => None,
            })
            .collect();
        assert_eq!(order, ["disc", "disc", "mesh"]);
    }

    #[test]
    fn the_strip_is_lit_up_to_the_value_and_veiled_after_it() {
        // The value sits on a whole point, between points and at an odd
        // place; none leaves a soft edge.
        for value in [0.5, 0.2, 0.7523, 0.01, 0.99] {
            let split = STRIP.min.x + STRIP.width() * value;
            let shapes = strip(value);
            let mesh = meshes(&shapes)[0];
            // A hard edge: the pair of vertices at the split from each side.
            let at_split: Vec<_> = mesh
                .vertices
                .iter()
                .filter(|vertex| (vertex.pos.x - split).abs() < 1e-3)
                .collect();
            assert!(
                at_split.len() >= 4,
                "{value}: {} at the edge",
                at_split.len()
            );
            let (lit, veiled) = sides(mesh, split);
            assert!(!lit.is_empty(), "{value}: nothing lit");
            for vertex in lit {
                assert_eq!(vertex.color, scale(position(vertex.pos.x)), "{value}");
            }
            for vertex in &veiled {
                assert_eq!(
                    vertex.color,
                    veil(scale(position(vertex.pos.x))),
                    "{value}: x {}",
                    vertex.pos.x
                );
            }
            // Both colors of the edge are there, two vertices of each.
            let edge = scale(position(split));
            let count = |color: Color32| at_split.iter().filter(|v| v.color == color).count();
            assert_eq!(count(edge), 2, "{value}: lit pair");
            assert_eq!(count(veil(edge)), 2, "{value}: veiled pair");
        }
    }

    #[test]
    fn a_strip_at_the_ends_is_all_veiled_or_all_lit() {
        let ends = |shapes: &[Shape]| circles(shapes).iter().map(|c| c.fill).collect::<Vec<_>>();
        // Nothing lit: every column and the left disc are veiled; the right
        // disc, too.
        let shapes = strip(0.0);
        let mesh = meshes(&shapes)[0];
        for vertex in &mesh.vertices {
            assert_eq!(vertex.color, veil(scale(position(vertex.pos.x))));
        }
        assert_eq!(ends(&shapes), [veil(scale(0.0)), veil(scale(1.0))]);
        // All lit: no veiled color anywhere, the right disc lit.
        let shapes = strip(1.0);
        let mesh = meshes(&shapes)[0];
        for vertex in &mesh.vertices {
            assert_eq!(vertex.color, scale(position(vertex.pos.x)));
        }
        assert_eq!(ends(&shapes), [scale(0.0), scale(1.0)]);
    }

    fn lit_edge(shapes: &[(Rect, Shape)], color_at: impl Fn(f32) -> Color32) -> f32 {
        testing::lit_edge(shapes, STRIP, color_at)
    }

    /// The values that fall in the round ends of the strip, from its edge to
    /// 3 points in, and some that do not. 0.004 is 1.5 points in; 0.995 is
    /// 860 K, 1.9 points short of the right edge.
    const VALUES: [f32; 12] = [
        0.0, 0.002, 0.004, 0.0079, 0.01, 0.2, 0.5, 0.9921, 0.995, 0.998, 0.9999, 1.0,
    ];

    #[test]
    fn the_lit_edge_sits_at_the_value_all_along_the_strip_ends_included() {
        for value in VALUES {
            let split = STRIP.min.x + STRIP.width() * value;
            let shapes = painted_clipped(1.0, |painter| {
                spectrum(painter, STRIP, value, veil, scale);
            });
            let edge = lit_edge(&shapes, scale);
            assert!(
                (edge - split).abs() <= 0.5,
                "{value}: the lit part ends at {edge}, not at {split}"
            );
        }
    }

    #[test]
    fn a_round_end_that_the_value_falls_in_is_lit_up_to_it_and_veiled_after() {
        // 0.004: 1.5 points into the left end.
        let split = STRIP.min.x + STRIP.width() * 0.004;
        let shapes = painted_clipped(1.0, |painter| {
            spectrum(painter, STRIP, 0.004, veil, scale);
        });
        let discs: Vec<_> = shapes
            .iter()
            .filter_map(|(clip, shape)| match shape {
                Shape::Circle(disc) => Some((*clip, disc.center.x, disc.fill)),
                _ => None,
            })
            .collect();
        // The left end: veiled whole, then its lit color clipped at the
        // value. The right end is veiled whole.
        assert_eq!(discs.len(), 3, "{discs:?}");
        assert_eq!((discs[0].1, discs[0].2), (27.0, veil(scale(0.0))));
        assert_eq!((discs[1].1, discs[1].2), (27.0, scale(0.0)));
        assert_eq!(discs[1].0.max.x, split);
        assert_eq!((discs[2].1, discs[2].2), (393.0, veil(scale(1.0))));
        // The mesh between them is all veiled: the value is left of it.
        let mesh = meshes_of(&shapes);
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.color == veil(scale(position(v.pos.x))))
        );
        // 0.995: 1.9 points short of the right edge, the right end's lit part
        // is the one clipped.
        let split = STRIP.min.x + STRIP.width() * 0.995;
        let shapes = painted_clipped(1.0, |painter| {
            spectrum(painter, STRIP, 0.995, veil, scale);
        });
        let right: Vec<_> = shapes
            .iter()
            .filter_map(|(clip, shape)| match shape {
                Shape::Circle(disc) if disc.center.x == 393.0 => Some((*clip, disc.fill)),
                _ => None,
            })
            .collect();
        assert_eq!(right.len(), 2);
        assert_eq!(right[0].1, veil(scale(1.0)));
        assert_eq!(right[1].1, scale(1.0));
        assert_eq!(right[1].0.max.x, split);
        // The left end and the whole body are lit.
        let mesh = meshes_of(&shapes);
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.color == scale(position(v.pos.x)))
        );
    }

    /// The one mesh among `shapes`.
    fn meshes_of(shapes: &[(Rect, Shape)]) -> &Mesh {
        let found: Vec<&Mesh> = shapes
            .iter()
            .filter_map(|(_, shape)| match shape {
                Shape::Mesh(mesh) => Some(&**mesh),
                _ => None,
            })
            .collect();
        assert_eq!(found.len(), 1);
        found[0]
    }

    #[test]
    fn the_mesh_shares_its_vertices_and_runs_left_to_right() {
        // A column of 366 points has 367 edges, a pair of vertices each.
        let edges = 367;
        for (value, pairs) in [
            // All lit or all veiled: no hard edge in the body.
            (1.0, edges),
            (0.0, edges),
            (0.004, edges),
            // The split on an edge doubles that edge's pair.
            (0.5, edges + 1),
            // Between edges, it adds a lit pair and a veiled one.
            (0.7523, edges + 2),
            (0.01, edges + 2),
        ] {
            let calls = Cell::new(0);
            let shapes = painted_clipped(1.0, |painter| {
                spectrum(painter, STRIP, value, veil, |at| {
                    calls.set(calls.get() + 1);
                    scale(at)
                });
            });
            let mesh = meshes_of(&shapes);
            assert_eq!(mesh.vertices.len(), pairs * 2, "{value}");
            // Two triangles join each pair to the next, except across the
            // hard edge, where the pairs sit at the same x.
            let joins = pairs - 1 - usize::from(pairs > edges);
            assert_eq!(mesh.indices.len(), joins * 6, "{value}");
            let xs: Vec<f32> = mesh.vertices.iter().map(|v| v.pos.x).collect();
            assert!(xs.windows(2).all(|pair| pair[0] <= pair[1]), "{value}");
            // One color for each edge, one more for the split if it falls
            // between two, and one for each end.
            let wanted = edges + 2 + usize::from(pairs == edges + 2);
            assert_eq!(calls.get(), wanted, "{value}: colors worked out");
        }
    }

    #[test]
    fn a_flat_strip_is_two_rectangles_and_the_ends() {
        let color = Color32::from_rgb(0xFF, 0x80, 0x00);
        let flat_at =
            |value: f32| painted_clipped(1.0, |painter| flat(painter, STRIP, value, veil, color));
        // Half lit: a lit rectangle to the value and a veiled one after it,
        // four vertices each, with a hard edge and no other colors.
        let shapes = flat_at(0.5);
        let mesh = meshes_of(&shapes);
        assert_eq!(mesh.vertices.len(), 8);
        for (vertex, wanted) in mesh
            .vertices
            .iter()
            .zip([color, veil(color)].iter().flat_map(|c| [c; 4]))
        {
            assert_eq!(vertex.color, *wanted, "x {}", vertex.pos.x);
            assert!(vertex.pos.y == STRIP.min.y || vertex.pos.y == STRIP.max.y);
        }
        let (lit, veiled) = sides(mesh, 210.0);
        assert_eq!((lit.len(), veiled.len()), (6, 6));
        assert!(
            lit.iter()
                .all(|v| v.color == color && (27.0..=210.0).contains(&v.pos.x))
        );
        assert!(
            veiled
                .iter()
                .all(|v| v.color == veil(color) && (210.0..=393.0).contains(&v.pos.x))
        );
        // All lit or all veiled: one rectangle.
        for (value, wanted) in [(1.0, color), (0.0, veil(color)), (0.004, veil(color))] {
            let shapes = flat_at(value);
            let mesh = meshes_of(&shapes);
            assert_eq!(mesh.vertices.len(), 4, "{value}");
            assert!(mesh.vertices.iter().all(|v| v.color == wanted), "{value}");
        }
        // Its edge sits at the value, ends included, as a gradient's does.
        for value in VALUES {
            let split = STRIP.min.x + STRIP.width() * value;
            let edge = lit_edge(&flat_at(value), |_| color);
            assert!(
                (edge - split).abs() <= 0.5,
                "{value}: the lit part ends at {edge}, not at {split}"
            );
        }
    }

    #[test]
    fn a_tick_sits_on_whole_device_pixels() {
        for pixels in [1.0, 2.0] {
            for x in [24.0, 100.25, 137.8, 279.8, 396.0] {
                let center = Cell::new(f32::NAN);
                let shapes = painted(pixels, |painter| {
                    center.set(tick(painter, x, 297.0, 3.0, Color32::RED));
                });
                let marks = rects(&shapes);
                assert_eq!(marks.len(), 1, "{pixels}x at {x}");
                let mark = marks[0];
                // 1 point wide and 3 tall, from the top it was given.
                assert_eq!(mark.rect.width(), 1.0);
                assert_eq!(mark.rect.height(), 3.0);
                assert_eq!(mark.rect.min.y, 297.0);
                assert_eq!(mark.fill, Color32::RED);
                // The left edge falls on a device pixel.
                let device = mark.rect.min.x * pixels;
                assert!(
                    (device - device.round()).abs() < 1e-3,
                    "{pixels}x at {x}: the left edge is at pixel {device}"
                );
                // It is the pixel nearest to centered at x, and the center
                // it returns is where the mark's middle fell.
                assert_eq!(center.get(), mark.rect.center().x);
                assert!(
                    (center.get() - x).abs() <= 0.5 / pixels + 1e-3,
                    "{pixels}x: centered at {} for {x}",
                    center.get()
                );
            }
        }
    }

    /// Red Now's glyph at a key's center: the upper half of a disc of radius
    /// 4.6 whose flat side lies 2.5 points below the center, over a line 16
    /// long and 1.5 wide whose middle is 1.6 below that side. The numbers are
    /// written out here, not read from the code's constants.
    #[test]
    fn the_sundown_glyph_is_a_half_disc_over_a_line() {
        let center = pos2(40.0, 532.0);
        let ink = Color32::from_rgb(200, 150, 100);
        let shapes = painted(1.0, |painter| sundown(painter, center, ink));
        let (paths, lines) = (paths(&shapes), segments(&shapes));
        assert_eq!(shapes.len(), 2, "one polygon and one line: {shapes:?}");
        assert_eq!((paths.len(), lines.len()), (1, 1), "{shapes:?}");

        // The sun: filled in the ink with no edge, every point of it on the
        // circle of radius 4.6 about the point 2.5 below the center, and
        // none below that point: the arc runs from the left end of the flat
        // side over the top to its right end.
        let (sun, flat) = (paths[0], center.y + 2.5);
        assert_eq!(sun.fill, ink);
        assert_eq!(sun.stroke.width, 0.0, "the sun has no edge");
        assert!(sun.points.len() > 8, "a smooth arc: {:?}", sun.points);
        let around = pos2(center.x, flat);
        for point in &sun.points {
            assert!(point.y <= flat + 1e-3, "{point:?} lies below the flat side");
            assert!(
                (point.distance(around) - 4.6).abs() < 1e-3,
                "{point:?} is {} from {around:?}",
                point.distance(around)
            );
        }
        let (mut left, mut right, mut top) = (f32::MAX, f32::MIN, f32::MAX);
        for point in &sun.points {
            left = left.min(point.x);
            right = right.max(point.x);
            top = top.min(point.y);
        }
        assert!((left - (center.x - 4.6)).abs() < 1e-3, "left at {left}");
        assert!((right - (center.x + 4.6)).abs() < 1e-3, "right at {right}");
        assert!((top - (flat - 4.6)).abs() < 1e-3, "top at {top}");

        // The line: level, 16 long, centered under the key's center, 1.6
        // below the flat side, 1.5 wide, in the ink.
        let (points, stroke) = lines[0];
        assert_eq!(points[0].y, points[1].y, "level: {points:?}");
        assert!(((points[0].x - points[1].x).abs() - 16.0).abs() < 1e-3);
        assert!(((points[0].x + points[1].x) / 2.0 - center.x).abs() < 1e-3);
        assert!((points[0].y - (flat + 1.6)).abs() < 1e-3, "{points:?}");
        assert_eq!(stroke, Stroke::new(1.5, ink));
    }
}
