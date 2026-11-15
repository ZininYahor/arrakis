//! The app's icon, drawn from the app's own code: the cover's sun and dunes,
//! with no worm, seen through the 1200 K night tint, the sun in the palette's
//! accent gold outside the tint, set a little lower than on the cover, into
//! the valley of the dunes. The picture sits on macOS's icon grid: a 1024
//! pixel canvas with an 824 pixel body, a superellipse close to a rounded
//! square of about 185 pixels' radius, and clear around it.
//!
//! `--demo-icon` writes the 1024 pixel master PNG, and `assets/icon.png` is
//! that file. `scripts/bundle.sh` makes the bundle's `.icns` from it. The
//! icon reaches macOS through the bundle's `Info.plist` alone: nothing in the
//! app sets it at run time.

use std::path::Path;

use crate::color::{Look, MAX_BRIGHTNESS};
use crate::demo::encode_png;
use crate::theme::{Cover, Palette};
use crate::ui::cover::scenery;

/// The master's side, in pixels: macOS's icon canvas.
pub const CANVAS: usize = 1024;
/// The side of the icon's body, in pixels: macOS's icon grid leaves a margin
/// around it for the shadow the system draws.
pub const BODY: usize = 824;
/// The clear border of the canvas, on each side.
const MARGIN: usize = (CANVAS - BODY) / 2;
/// The exponent of the superellipse that outlines the body. 5 sits within two
/// pixels of an 185 pixel corner radius on the diagonal, and its corners run
/// into the sides smoothly, as a rounded square of continuous corners does.
const CORNER_EXPONENT: f32 = 5.0;
/// The look the picture is seen through, the app's default night: 1200 K.
/// Named here, so that a change of the default does not repaint the icon.
/// `the_palette_reads_through_red` (`src/theme.rs`) checks the icon through it.
pub(crate) const NIGHT: Look = Look {
    kelvin: 1200,
    brightness: MAX_BRIGHTNESS,
};
/// How many grid units lower than the cover the sun is set: with the maw
/// gone, nothing hides the sky between the sun and the dunes.
pub(crate) const SUN_DROP: f32 = 1.4;

/// The cover's colors for the icon: its sky and dunes through the night
/// tint, and the sun in the palette's accent, outside it.
fn scene(palette: &Palette) -> Cover {
    let night = NIGHT.tint();
    let cover = palette.cover;
    Cover {
        sky_top: night.apply(cover.sky_top),
        sky_low: night.apply(cover.sky_low),
        sun: palette.accent,
        far: night.apply(cover.far),
        lee: night.apply(cover.lee),
        near: night.apply(cover.near),
        // The worm is not drawn.
        ..cover
    }
}

/// How much of the pixel at (`px`, `py`) of the canvas the body covers, from
/// 0 to 1: the superellipse's edge, antialiased by the pixel's distance from
/// it.
fn coverage(px: usize, py: usize) -> f32 {
    let n = CORNER_EXPONENT;
    let half = BODY as f32 / 2.0;
    let mid = CANVAS as f32 / 2.0;
    let x = (px as f32 + 0.5 - mid).abs() / half;
    let y = (py as f32 + 0.5 - mid).abs() / half;
    let g = (x.powf(n) + y.powf(n)).powf(1.0 / n);
    if g < 1e-4 {
        return 1.0;
    }
    // The gradient of g, in pixels: how far a step of 1 in g is.
    let step =
        (x.powf(n - 1.0).powi(2) + y.powf(n - 1.0).powi(2)).sqrt() / (half * g.powf(n - 1.0));
    (0.5 - (g - 1.0) / step).clamp(0.0, 1.0)
}

/// The master as RGBA with straight alpha, [`CANVAS`] pixels square: the
/// picture of [`BODY`] pixels, clipped to the body's outline, in the middle.
pub fn render(palette: &Palette) -> Vec<u8> {
    let art = scenery(BODY, &scene(palette), SUN_DROP);
    let mut rgba = vec![0u8; CANVAS * CANVAS * 4];
    for py in 0..CANVAS {
        for px in 0..CANVAS {
            let cover = coverage(px, py);
            if cover <= 0.0 {
                continue;
            }
            let x = px.saturating_sub(MARGIN).min(BODY - 1);
            let y = py.saturating_sub(MARGIN).min(BODY - 1);
            let [r, g, b, _] = art[y * BODY + x].to_srgba_unmultiplied();
            let at = (py * CANVAS + px) * 4;
            rgba[at..at + 4].copy_from_slice(&[r, g, b, (cover * 255.0).round() as u8]);
        }
    }
    rgba
}

/// The master as the bytes of a PNG file.
pub fn encode(palette: &Palette) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    encode_png(&mut png, CANVAS, CANVAS, &render(palette))?;
    Ok(png)
}

/// `--demo-icon`: writes the master to `path` and returns the exit code.
/// Leaves the displays and the settings alone, and opens no window.
pub fn run(path: &Path) -> i32 {
    let saved = encode(&Palette::DARK)
        .and_then(|png| std::fs::write(path, png).map_err(|error| error.to_string()));
    match saved {
        Ok(()) => {
            println!("saved {}", path.display());
            0
        }
        Err(error) => {
            eprintln!("demo-icon: {}: {error}", path.display());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, BufWriter, Write};
    use std::path::PathBuf;
    use std::sync::LazyLock;

    use egui::Color32;

    use super::*;
    use crate::diff;

    /// The master as drawn from the dark palette: drawing it takes a moment in
    /// a debug build, so the tests share one.
    static MASTER: LazyLock<Vec<u8>> = LazyLock::new(|| render(&Palette::DARK));

    /// The committed master, `assets/icon.png`.
    fn committed() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/icon.png")
    }

    /// The pixel at (`x`, `y`) of `rgba`, a master's pixels.
    fn pixel(rgba: &[u8], x: usize, y: usize) -> [u8; 4] {
        let at = (y * CANVAS + x) * 4;
        rgba[at..at + 4].try_into().expect("four bytes")
    }

    fn alpha(x: usize, y: usize) -> u8 {
        pixel(&MASTER, x, y)[3]
    }

    /// The night tint the picture is seen through.
    fn night(color: Color32) -> Color32 {
        NIGHT.tint().apply(color)
    }

    /// An opaque pixel of `color`.
    fn opaque(color: Color32) -> [u8; 4] {
        let [r, g, b, _] = color.to_array();
        [r, g, b, 255]
    }

    /// A folder of its own for a test's files, removed when it goes out of
    /// scope.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("arrakis-icon-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("a folder for the test's files");
            Self(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The committed master must be the icon as the code draws it now, or a
    /// change to the cover, the palette or the encoder leaves a stale icon in
    /// the bundle. The bytes also depend on the `png` crate and its
    /// compression, both pinned by `Cargo.lock`.
    #[test]
    fn the_committed_master_is_the_icon_as_drawn_now() {
        let drawn = encode(&Palette::DARK).expect("the master encodes");
        let committed = std::fs::read(committed()).expect("assets/icon.png");
        assert!(
            drawn == committed,
            "assets/icon.png is not the icon as drawn now ({} bytes drawn, {} committed); \
             draw it again with: cargo run --features demo -- --demo-icon assets/icon.png",
            drawn.len(),
            committed.len()
        );
    }

    #[test]
    fn the_master_is_a_1024_pixel_square_and_a_still() {
        let master = diff::read(&committed()).expect("assets/icon.png reads");
        assert_eq!((master.width, master.height), (1024, 1024));
        assert_eq!(master.frames.len(), 1);
        assert_eq!(MASTER.len(), CANVAS * CANVAS * 4);
    }

    #[test]
    fn the_body_sits_in_a_clear_margin_on_a_pixel_boundary() {
        for (x, y) in [(0, 0), (50, 512), (98, 512), (99, 512)] {
            assert_eq!(alpha(x, y), 0, "({x}, {y}) is in the margin");
        }
        // The sides' edge falls on the boundary at x = 100: no half-covered
        // pixel there, the antialiased ones are at the corners.
        for (x, y) in [(100, 512), (101, 512), (512, 512), (923, 512), (512, 100)] {
            assert_eq!(alpha(x, y), 255, "({x}, {y}) is in the body");
        }
        for (x, y) in [(924, 512), (512, 99), (512, 924)] {
            assert_eq!(alpha(x, y), 0, "({x}, {y}) is in the margin");
        }
    }

    #[test]
    fn the_corners_are_round_and_antialiased() {
        // A superellipse of exponent 5 passes through about (153.3, 153.3);
        // a square corner would make (150, 150) opaque.
        assert_eq!(alpha(150, 150), 0);
        assert_eq!(alpha(158, 158), 255);
        let edge: Vec<u8> = (150..=158).map(|n| alpha(n, n)).collect();
        assert!(
            edge.iter().any(|a| (1..=254).contains(a)),
            "no half-covered pixel on the diagonal: {edge:?}"
        );
        assert!(
            edge.windows(2).all(|pair| pair[0] <= pair[1]),
            "the coverage does not grow into the body: {edge:?}"
        );
    }

    #[test]
    fn the_outline_is_symmetric_about_both_axes() {
        for y in 0..CANVAS {
            for x in 0..CANVAS {
                let a = alpha(x, y);
                assert_eq!(
                    a,
                    alpha(CANVAS - 1 - x, y),
                    "({x}, {y}) across the vertical"
                );
                assert_eq!(
                    a,
                    alpha(x, CANVAS - 1 - y),
                    "({x}, {y}) across the horizontal"
                );
            }
        }
    }

    #[test]
    fn the_sun_is_the_accent_outside_the_tint() {
        let accent = opaque(Palette::DARK.accent);
        for (x, y) in [(512, 567), (512, 741), (512, 512)] {
            assert_eq!(pixel(&MASTER, x, y), accent, "({x}, {y}) is in the sun");
        }
    }

    #[test]
    fn the_ground_is_the_near_dune_through_the_night_tint() {
        let ground = opaque(night(Palette::DARK.cover.near));
        assert_eq!(pixel(&MASTER, 512, 901), ground);
        assert_ne!(
            ground,
            opaque(Palette::DARK.cover.near),
            "the tint changes it"
        );
    }

    /// The sun is set lower than the cover's: the cover puts it at (9, 8.8)
    /// of the grid and the icon at (9, 10.2), and the radius is 5.28. Canvas
    /// (512, 292) is (9, 4.2), which the cover's sun would cover; canvas
    /// (512, 741) is (9, 14.0), which the cover's sun ends short of.
    #[test]
    fn the_sun_is_lower_than_the_covers_into_the_valley() {
        let accent = opaque(Palette::DARK.accent);
        let top = pixel(&MASTER, 512, 292);
        assert_eq!(top[3], 255);
        assert_ne!(top, accent, "the sky shows above the sun");
        // The sky, from the top color to the one at the ground, through the
        // night tint: grid y 4.2 of the ground's 17.
        let cover = Palette::DARK.cover;
        let sky = night(cover.sky_top).lerp_to_gamma(night(cover.sky_low), 4.2 / 17.0);
        for (channel, (got, wanted)) in top.into_iter().zip(opaque(sky)).enumerate() {
            assert!(
                got.abs_diff(wanted) <= 2,
                "channel {channel}: {top:?} is not {sky:?}"
            );
        }
        assert_eq!(
            pixel(&MASTER, 512, 741),
            accent,
            "the sun reaches the valley"
        );
    }

    /// The worm is not in the picture: none of its colors, seen through the
    /// night tint, is anywhere in the master.
    #[test]
    fn the_worm_is_not_drawn() {
        let cover = Palette::DARK.cover;
        let worm_colors = [
            ("the lip", opaque(night(cover.worm))),
            ("a tooth", opaque(night(cover.tooth))),
            ("the throat", opaque(night(cover.throat))),
        ];
        let scene_colors = [
            cover.sky_top,
            cover.sky_low,
            cover.far,
            cover.lee,
            cover.near,
        ]
        .map(|color| opaque(night(color)));
        for (name, color) in worm_colors {
            // The check means something only while the worm's color is not
            // one the scenery holds.
            assert!(
                !scene_colors.contains(&color),
                "{name} is the color of the scenery"
            );
            let at = MASTER.as_chunks::<4>().0.iter().position(|p| *p == color);
            assert_eq!(at, None, "{name} ({color:?}) is in the master");
        }
    }

    #[test]
    fn the_sun_follows_the_palettes_accent() {
        let palette = Palette {
            accent: Color32::from_rgb(1, 2, 3),
            ..Palette::DARK
        };
        let picture = render(&palette);
        assert_eq!(pixel(&picture, 512, 567), [1, 2, 3, 255]);
        assert_eq!(
            pixel(&picture, 512, 901),
            pixel(&MASTER, 512, 901),
            "the ground does not move with the accent"
        );
    }

    #[test]
    fn the_ground_follows_the_covers_near_dune_and_the_sun_stays() {
        let near = Color32::from_rgb(200, 100, 50);
        let palette = Palette {
            cover: Cover {
                near,
                ..Palette::DARK.cover
            },
            ..Palette::DARK
        };
        let picture = render(&palette);
        assert_eq!(pixel(&picture, 512, 901), opaque(night(near)));
        assert_ne!(pixel(&picture, 512, 901), pixel(&MASTER, 512, 901));
        assert_eq!(pixel(&picture, 512, 567), opaque(Palette::DARK.accent));
    }

    /// The straight alpha of a pixel survives the PNG: a clear pixel keeps
    /// its color, as the body's edge pixels do.
    #[test]
    fn a_png_keeps_its_pixels_and_their_straight_alpha() {
        let rgba = [
            [255, 0, 0, 255],
            [0, 255, 0, 128],
            [10, 20, 30, 0],
            [1, 2, 3, 254],
        ];
        let mut png = Vec::new();
        encode_png(&mut png, 2, 2, &rgba.concat()).expect("2 by 2 encodes");
        let folder = Folder::new("png");
        let path = folder.0.join("two.png");
        std::fs::write(&path, png).expect("the PNG is written");
        let picture = diff::read(&path).expect("the PNG reads back");
        assert_eq!((picture.width, picture.height), (2, 2));
        assert_eq!(picture.frames.len(), 1);
        assert_eq!(picture.frames[0].pixels, rgba);
    }

    /// A writer that takes `room` bytes and refuses every write past them.
    struct Full {
        room: usize,
        taken: usize,
    }

    impl Write for Full {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.taken + bytes.len() > self.room {
                return Err(io::Error::other("no room"));
            }
            self.taken += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A 2 by 2 picture as the bytes of a PNG.
    fn two_by_two() -> (Vec<u8>, Vec<u8>) {
        let rgba = [[9, 8, 7, 255]; 4].concat();
        let mut png = Vec::new();
        encode_png(&mut png, 2, 2, &rgba).expect("2 by 2 encodes");
        (rgba, png)
    }

    /// The end of the file, the IEND chunk, is written when the picture is
    /// finished and not when the encoder is dropped: a drop would lose its
    /// error, and the caller would log a picture without an end as saved.
    #[test]
    fn a_png_that_runs_out_of_room_at_its_end_is_an_error() {
        let (rgba, png) = two_by_two();
        // The IEND chunk is the last 12 bytes: 4 of length, 4 of name, 4 of CRC.
        for room in [png.len() - 12, png.len() - 4, png.len() - 1] {
            let result = encode_png(Full { room, taken: 0 }, 2, 2, &rgba);
            assert!(result.is_err(), "{room} of {} bytes fit", png.len());
        }
        encode_png(
            Full {
                room: png.len(),
                taken: 0,
            },
            2,
            2,
            &rgba,
        )
        .expect("the whole file fits");
    }

    /// The tail of a buffered file is written when the buffer is flushed,
    /// which a drop would also lose.
    #[test]
    fn a_png_whose_buffer_cannot_be_flushed_is_an_error() {
        let (rgba, png) = two_by_two();
        // The buffer holds the whole picture, so nothing reaches the writer
        // before the flush.
        let buffered = |room| BufWriter::with_capacity(png.len() * 2, Full { room, taken: 0 });
        assert!(encode_png(buffered(png.len() - 1), 2, 2, &rgba).is_err());
        encode_png(buffered(png.len()), 2, 2, &rgba).expect("the whole file fits");
    }

    #[test]
    fn run_writes_the_master_to_a_new_file_and_returns_0() {
        let folder = Folder::new("run");
        let path = folder.0.join("icon.png");
        assert_eq!(run(&path), 0);
        let written = std::fs::read(&path).expect("the master is written");
        assert!(
            written == encode(&Palette::DARK).expect("the master encodes"),
            "the file is not the master"
        );
    }

    #[test]
    fn run_returns_1_and_writes_nothing_for_a_folder_that_is_not_there() {
        let folder = Folder::new("run-missing");
        let path = folder.0.join("no/such/folder/icon.png");
        assert_eq!(run(&path), 1);
        assert!(!path.exists());
        assert!(
            !path.parent().expect("a folder").exists(),
            "no folder is made"
        );
    }
}
