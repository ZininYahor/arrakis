//! The menu-bar mark, Sundown: a sun half sunk behind a long dune, the end of
//! the day, which is what the app does. It is drawn as shapes on an 18-point
//! grid and sampled at whatever size the menu bar asks for; the macOS
//! template at 36 pixels is also embedded in the binary, drawn once
//! (`precomputed`). The window's cover has shapes of its own
//! (`ui::cover::shapes`): the two share nothing.

use egui::{Color32, Pos2, pos2};

use crate::precomputed;

/// The mark's grid, in points.
const GRID: f32 = 18.0;
/// Samples per pixel along each side.
const SAMPLES: usize = 6;

/// The full-color mark's one color: the sun's.
const SUN: Color32 = Color32::from_rgb(0xFF, 0x8A, 0x3D);

/// The sun: its center and radius.
const SUN_CENTER: Pos2 = pos2(7.4, 8.9);
const SUN_RADIUS: f32 = 5.0;
/// Where the dune's line rests when there is no dune, its flat foot, and how
/// thick it is drawn.
const FOOT: f32 = 14.75;
const STROKE: f32 = 1.5;
/// What the line keeps the sun from, so the sun is cut clean by it and the
/// mark reads in one color.
const GAP: f32 = 1.0;
/// The dune the line follows.
const DUNE: Dune = Dune {
    crest: 12.2,
    height: 6.2,
    left: 11.0,
    right: 4.6,
};

/// A dune's profile: a long windward slope to the left of the crest, a short
/// slip face to the right.
struct Dune {
    crest: f32,
    height: f32,
    /// How far the slope runs out to the left of the crest, and to the right.
    left: f32,
    right: f32,
}

impl Dune {
    /// How high the dune stands at `x`.
    fn rise(&self, x: f32) -> f32 {
        let run = if x < self.crest {
            (self.crest - x) / self.left
        } else {
            (x - self.crest) / self.right
        };
        self.height * (1.0 - run.min(1.0).powf(1.6)).powf(1.9)
    }

    /// The dune's line, as a polyline from `x0` to `x1`, its flat foot at
    /// `foot`.
    fn line(&self, foot: f32, x0: f32, x1: f32) -> Vec<Pos2> {
        let steps = ((x1 - x0) / 0.1).ceil() as usize;
        (0..=steps)
            .map(|i| {
                let x = egui::lerp(x0..=x1, i as f32 / steps as f32);
                pos2(x, foot - self.rise(x))
            })
            .collect()
    }
}

/// How far `q` lies from the segment `a` to `b`.
fn segment(q: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((q - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0);
    q.distance(a + ab * t)
}

/// How far `q` lies from a polyline.
fn path(q: Pos2, points: &[Pos2]) -> f32 {
    points
        .windows(2)
        .map(|p| segment(q, p[0], p[1]))
        .fold(f32::MAX, f32::min)
}

/// How far from a sample the line can still change an answer.
/// [`Sundown::covers_at`] asks two things of `off`, the distance from the
/// sample to the line: whether `off <= STROKE / 2` (the line is drawn there)
/// and whether `off > STROKE / 2 + GAP` (the sun is clear of the line). Both
/// are settled once `off` is past the larger bound, `REACH`. So a segment
/// farther than `REACH` from the sample never matters: if the nearest segment
/// lies within `REACH`, every segment within `REACH` is measured and the
/// minimum is the same; if none does, whatever is measured is farther still,
/// and both answers stay as they were. A segment within `REACH` of the sample
/// has a point within `REACH` of it in x, so only the segments whose span in x
/// lies within `REACH` of the sample's x are measured.
const REACH: f32 = STROKE / 2.0 + GAP;
/// What is added to `REACH` when choosing the segments, for the rounding of
/// `f32` distances (about 1e-5 at this size): it can only add segments, never
/// drop one.
const REACH_SLACK: f32 = 1e-3;

/// The mark: the dune's line, and the sun above it.
struct Sundown {
    /// The line's points, from left to right: [`Sundown::near`] relies on it.
    line: Vec<Pos2>,
}

impl Sundown {
    fn new() -> Self {
        Self {
            line: DUNE.line(FOOT, 0.75, 17.25),
        }
    }

    /// Whether the mark covers `q`: the line, and the sun's disc where it
    /// lies above the line and a gap away from it.
    fn covers(&self, q: Pos2) -> bool {
        Self::covers_at(path(q, self.near(q)), q)
    }

    /// The part of the line that can change an answer at `q`: the points of
    /// the segments whose span in x lies within `REACH` of `q.x`.
    fn near(&self, q: Pos2) -> &[Pos2] {
        let reach = REACH + REACH_SLACK;
        let first = (self.line.partition_point(|p| p.x < q.x - reach)).saturating_sub(1);
        let last = (self.line.partition_point(|p| p.x <= q.x + reach)).min(self.line.len() - 1);
        &self.line[first..=last]
    }

    /// [`Sundown::covers`], given `off`, how far the line lies from `q`.
    fn covers_at(off: f32, q: Pos2) -> bool {
        if off <= STROKE / 2.0 {
            return true;
        }
        q.distance(SUN_CENTER) <= SUN_RADIUS && off > REACH && q.y < FOOT - DUNE.rise(q.x)
    }
}

/// `size` by `size` RGBA pixels, straight alpha: `ink` at each pixel, as
/// opaque as the share of the pixel the mark covers.
fn raster(size: usize, ink: Color32) -> Vec<u8> {
    let mark = Sundown::new();
    sampled(size, ink, |q| mark.covers(q))
}

/// [`raster`], with `covers` saying which samples the mark covers.
fn sampled(size: usize, ink: Color32, covers: impl Fn(Pos2) -> bool) -> Vec<u8> {
    let scale = GRID / size as f32;
    let sample = |n: usize, k: usize| (n as f32 + (k as f32 + 0.5) / SAMPLES as f32) * scale;
    let [r, g, b, a] = ink.to_array();
    let mut pixels = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let hits = (0..SAMPLES * SAMPLES)
                .filter(|k| covers(pos2(sample(x, k % SAMPLES), sample(y, k / SAMPLES))))
                .count();
            let coverage = hits as f32 / (SAMPLES * SAMPLES) as f32;
            pixels.extend([r, g, b, (f32::from(a) * coverage).round() as u8]);
        }
    }
    pixels
}

/// The full-color menu-bar icon, drawn at any size: the template's shape in
/// the sun's color. `fastframe-tray` asks for it only off macOS (32 pixels on
/// Windows, 64 on Linux): on macOS, with the template icon in its config, it
/// draws the template alone. So it is not embedded in the binary.
pub fn tray(size: usize) -> Vec<u8> {
    raster(size, SUN)
}

/// The macOS template icon: black on transparent, recolored by the menu bar.
/// At the macOS size, 36 pixels, it is copied from the bytes embedded in the
/// binary (`precomputed`), which a test keeps equal to [`draw_template`]; any
/// other size is drawn.
pub fn tray_template(size: usize) -> Vec<u8> {
    if size == precomputed::TRAY_SIZE {
        return precomputed::TRAY_TEMPLATE.to_vec();
    }
    draw_template(size)
}

/// The template icon, drawn: the source of the embedded bytes.
pub(crate) fn draw_template(size: usize) -> Vec<u8> {
    raster(size, Color32::BLACK)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The alpha channel of RGBA `pixels`.
    fn alphas(pixels: &[u8]) -> Vec<u8> {
        pixels.chunks(4).map(|p| p[3]).collect()
    }

    #[test]
    fn the_template_is_black_with_soft_edges() {
        let pixels = tray_template(36);
        assert_eq!(pixels.len(), 36 * 36 * 4);
        assert!(pixels.chunks(4).all(|p| p[..3] == [0, 0, 0]));
        assert!(pixels.chunks(4).any(|p| p[3] > 0 && p[3] < 0xFF));
    }

    #[test]
    fn the_template_at_the_macos_size_is_the_embedded_bytes_and_any_other_is_drawn() {
        assert_eq!(tray_template(36), precomputed::TRAY_TEMPLATE);
        for size in [18, 32, 44, 64] {
            assert_eq!(tray_template(size), draw_template(size), "{size} px");
        }
    }

    /// The sizes `fastframe-tray` asks for (36 on macOS, 32 on Windows, 64 on
    /// Linux), and the grid's own 18.
    const SIZES: [usize; 4] = [18, 32, 36, 64];

    #[test]
    fn the_color_icon_is_drawn_whole_at_every_size() {
        for size in SIZES {
            let pixels = tray(size);
            assert_eq!(pixels.len(), size * size * 4, "{size} px");
            // One color, the sun's, on soft edges.
            let colors: std::collections::BTreeSet<[u8; 3]> =
                pixels.chunks(4).map(|p| [p[0], p[1], p[2]]).collect();
            let [r, g, b, _] = SUN.to_array();
            assert_eq!(colors, [[r, g, b]].into(), "{size} px");
            assert!(pixels.chunks(4).any(|p| p[3] > 0 && p[3] < 0xFF));
        }
    }

    #[test]
    fn the_color_icon_has_the_template_shape() {
        for size in SIZES {
            assert_eq!(
                alphas(&tray(size)),
                alphas(&tray_template(size)),
                "{size} px"
            );
        }
    }

    /// The folder of the pin, and its two files: the 18-pixel template as
    /// ten-shade text, and the 36-pixel template's alpha, 36 by 36 bytes.
    ///
    /// They are the record of the owner's pick of 2026-11-11, made once from
    /// the mark's drawing in a mark sheet that is not in the repository. The
    /// alpha's SHA-256 was `7dd89ba162805fd9e5a02cdb06496a1f29eb4dacc4159057f626bd8d50cd333e`
    /// (`shasum -a 256`). No test compares the files with that drawing, which
    /// cannot be reached: from now on the files themselves are the reference.
    const PIN_DIR: &str = "src/icon";
    const PIN_TEXT: &str = "sundown-18.txt";
    const PIN_ALPHA: &str = "sundown-36.alpha";

    /// The command that rewrites the pin, as the failure message gives it. It
    /// is a step of its own, apart from `ARRAKIS_BLESS` (`precomputed`): the
    /// pin is the owner's approval, and a step that rewrote it along with the
    /// embedded template would pin nothing.
    const BLESS_MARK: &str =
        "ARRAKIS_BLESS_MARK=1 cargo test --features demo --lib the_menu_bar_mark_is_sundown";

    /// The 18-pixel alpha `alpha` as the pin's text writes it: each alpha
    /// is one of ten shades, written twice, a row between bars.
    fn sheet_text(alpha: &[u8]) -> String {
        const SHADES: &[u8] = b" .:-=+*#%@";
        let rows: Vec<String> = alpha
            .chunks(18)
            .map(|row| {
                let shades: String = row
                    .iter()
                    .flat_map(|&a| {
                        let shade = SHADES[(usize::from(a) * (SHADES.len() - 1) + 127) / 255];
                        [char::from(shade); 2]
                    })
                    .collect();
                format!("|{shades}|")
            })
            .collect();
        rows.join("\n")
    }

    /// Passes when each of `drawn`, a pin file's name and what the mark draws
    /// now, is what `dir` holds. Otherwise, with `bless`, it writes the stale
    /// files and passes; without, it fails with the reason, which names the
    /// owner's approval and the command that rewrites the pin. A file that is
    /// not stale is never written.
    fn keep_pin(dir: &Path, drawn: &[(&str, Vec<u8>)], bless: bool) -> Result<(), String> {
        let stale: Vec<String> = (drawn.iter())
            .filter(|(file, fresh)| std::fs::read(dir.join(file)).ok().as_ref() != Some(fresh))
            .map(|(file, fresh)| {
                if bless {
                    std::fs::write(dir.join(file), fresh)
                        .unwrap_or_else(|error| panic!("unable to write {file}: {error}"));
                }
                format!("{PIN_DIR}/{file}")
            })
            .collect();
        if stale.is_empty() || bless {
            return Ok(());
        }
        Err(format!(
            "{} not what the mark draws now. The pin holds the pixels the owner approved, so a \
             changed mark needs the owner's approval first. Only after it, and after \
             `ARRAKIS_BLESS=1 cargo test --features demo --lib precomputed`, rewrite the pin \
             with:\n    {BLESS_MARK}\nthen run every test.",
            stale.join(" and ") + if stale.len() == 1 { " is" } else { " are" }
        ))
    }

    #[test]
    fn the_menu_bar_mark_is_sundown() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(PIN_DIR);
        let text = sheet_text(&alphas(&draw_template(18)));
        let drawn = [
            (PIN_TEXT, text.into_bytes()),
            (PIN_ALPHA, alphas(&draw_template(36))),
        ];
        // Only the value `1` rewrites the pin, so a stray value cannot.
        let bless = std::env::var_os("ARRAKIS_BLESS_MARK").is_some_and(|value| value == "1");
        keep_pin(&dir, &drawn, bless).unwrap_or_else(|why| panic!("{why}"));

        // What the menu bar shows, the embedded bytes, is the pinned alpha too.
        let pinned = std::fs::read(dir.join(PIN_ALPHA)).expect("the pinned alpha");
        assert_eq!(pinned.len(), 36 * 36);
        assert_eq!(alphas(&tray_template(36)), pinned, "embedded");

        // Off macOS the same shape is drawn in the sun's orange, #FF8A3D.
        assert!(tray(36).chunks(4).all(|p| p[..3] == [0xFF, 0x8A, 0x3D]));
    }

    #[test]
    fn a_stale_pin_is_rewritten_only_with_the_marks_switch_on() {
        let dir = std::env::temp_dir().join(format!("arrakis-icon-pin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder under the temp dir");
        let drawn = [(PIN_TEXT, b"|  |".to_vec()), (PIN_ALPHA, vec![1, 2, 3])];
        let held = |file: &str| std::fs::read(dir.join(file)).ok();

        // The text differs and the alpha is missing: both are stale.
        std::fs::write(dir.join(PIN_TEXT), b"|##|").expect("a stale file");
        let why = keep_pin(&dir, &drawn, false).expect_err("a stale pin fails");
        assert!(why.contains(&format!("{PIN_DIR}/{PIN_TEXT}")), "{why}");
        assert!(why.contains(&format!("{PIN_DIR}/{PIN_ALPHA}")), "{why}");
        assert!(why.contains("the owner's approval first"), "{why}");
        assert!(why.contains(BLESS_MARK), "{why}");
        // Without the switch nothing is written.
        assert_eq!(held(PIN_TEXT), Some(b"|##|".to_vec()));
        assert_eq!(held(PIN_ALPHA), None);

        // With it, both files are written, and then nothing is stale.
        keep_pin(&dir, &drawn, true).expect("a rewrite passes");
        assert_eq!(held(PIN_TEXT), Some(drawn[0].1.clone()));
        assert_eq!(held(PIN_ALPHA), Some(drawn[1].1.clone()));
        keep_pin(&dir, &drawn, false).expect("a fresh pin passes without the switch");
        std::fs::remove_dir_all(&dir).expect("the folder goes");
    }

    /// The drawing before the cull, kept as the oracle: every sample measured
    /// against every segment of the line.
    fn plain(size: usize, ink: Color32) -> Vec<u8> {
        let mark = Sundown::new();
        sampled(size, ink, |q| Sundown::covers_at(path(q, &mark.line), q))
    }

    #[test]
    fn measuring_only_the_segments_near_a_sample_draws_the_same_pixels() {
        let mark = Sundown::new();
        assert!(
            mark.line.windows(2).all(|p| p[0].x <= p[1].x),
            "the line runs from left to right"
        );
        // It does cull: a sample measures under a quarter of the line.
        assert!(mark.near(SUN_CENTER).len() * 4 < mark.line.len());
        // The comparisons are of bytes, not `assert_eq!`, which would print them.
        for size in SIZES {
            let template = draw_template(size) == plain(size, Color32::BLACK);
            assert!(template, "template, {size} px");
            assert!(tray(size) == plain(size, SUN), "tray, {size} px");
        }
    }

    #[test]
    fn the_sun_and_the_line_are_solid_and_the_gap_between_them_open() {
        // The 36-pixel template's alpha at the pixel that holds a point of the
        // 18-point grid.
        let template = tray_template(36);
        let scale = 36.0 / GRID;
        let alpha_at = |q: Pos2| {
            let (x, y) = ((q.x * scale) as usize, (q.y * scale) as usize);
            template[(y * 36 + x) * 4 + 3]
        };

        // The sun's center is solid, and so is the line at its crest.
        assert_eq!(alpha_at(SUN_CENTER), 0xFF, "the sun");
        let crest = pos2(DUNE.crest, FOOT - DUNE.height);
        assert_eq!(alpha_at(crest), 0xFF, "the line");

        // Between the sun and the line, a gap of the line's own: the sun is cut
        // clean by it. Its lower part, behind the dune, is not drawn at all,
        // and the dune below the line is hollow.
        assert_eq!(alpha_at(pos2(8.75, 8.75)), 0, "the gap");
        let sunk = pos2(SUN_CENTER.x, SUN_CENTER.y + SUN_RADIUS - 0.1);
        assert_eq!(alpha_at(sunk), 0, "the sun behind the dune");
        assert_eq!(alpha_at(pos2(DUNE.crest, 12.0)), 0, "under the crest");

        // Nothing lies below the line's flat foot and its half stroke.
        let below = ((FOOT + STROKE / 2.0) * scale).ceil() as usize;
        assert!(
            template[below * 36 * 4..].chunks(4).all(|p| p[3] == 0),
            "rows {below} to 35"
        );
    }
}
