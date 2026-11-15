//! `--demo-diff`: two PNGs compared pixel by pixel, every frame of a film
//! too, to show that a change left the window's pictures alone, or moved
//! them only so far and only where it may. Demo builds only; it reads files
//! and touches no display.
//!
//! Frames are compared as the file stores them: a film's frames must cover
//! the same area and show for as long in both files.

use std::fmt;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::theme;
use crate::ui::view::COVER_RECT;

/// What passes.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The largest difference a channel may show, out of 255.
    pub tolerance: u8,
    /// Where differences may be, up to the tolerance. Outside it every pixel
    /// must match exactly. Without it the tolerance holds everywhere.
    pub within: Option<Area>,
    /// Where to write the amplified difference of the frame where the
    /// comparison fails, or of the frame that differs most when it passes.
    pub out: Option<PathBuf>,
}

/// A rectangle of pixels, written `x,y,width,height`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Area {
    /// The x just past the area, saturating at `u32::MAX`.
    fn right(&self) -> u32 {
        self.x.saturating_add(self.width)
    }

    /// The y just past the area, saturating at `u32::MAX`.
    fn bottom(&self) -> u32 {
        self.y.saturating_add(self.height)
    }

    pub fn contains(&self, x: u32, y: u32) -> bool {
        (self.x..self.right()).contains(&x) && (self.y..self.bottom()).contains(&y)
    }
}

impl FromStr for Area {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let numbers = text
            .split(',')
            .map(|part| part.trim().parse::<u32>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("{text:?} is not x,y,width,height: {error}"))?;
        match numbers[..] {
            [x, y, width, height] => Ok(Self {
                x,
                y,
                width,
                height,
            }),
            _ => Err(format!("{text:?} is not x,y,width,height")),
        }
    }
}

impl fmt::Display for Area {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{},{},{}", self.x, self.y, self.width, self.height)
    }
}

/// A PNG's frames as RGBA: one for a still, each of a film's.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<Frame>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// The part of the picture the frame covers: all of it for a still.
    pub area: Area,
    /// How long a film shows the frame, in seconds as a fraction; 0/1 for a
    /// still.
    pub delay: (u16, u16),
    /// The frame's pixels, row by row.
    pub pixels: Vec<[u8; 4]>,
}

/// The differences the counts are given above.
pub const THRESHOLDS: [u8; 7] = [0, 1, 2, 4, 8, 16, 32];
const CHANNELS: [&str; 4] = ["red", "green", "blue", "alpha"];

/// How a part of two pictures differs, over every frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Region {
    /// The pixels compared.
    pub pixels: u64,
    /// For each of [`THRESHOLDS`], the pixels whose largest channel
    /// difference is above it.
    pub above: [u64; THRESHOLDS.len()],
    /// The first pixel with the largest difference; none when every pixel
    /// matches.
    pub largest: Option<Spot>,
}

/// Where two pictures differ most.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub frame: usize,
    pub x: u32,
    pub y: u32,
    /// 0 to 3: red, green, blue, alpha.
    pub channel: usize,
    pub difference: u8,
    pub a: [u8; 4],
    pub b: [u8; 4],
}

impl Region {
    fn add(&mut self, frame: usize, (x, y): (u32, u32), a: [u8; 4], b: [u8; 4]) {
        self.pixels += 1;
        let (channel, difference) = (0..4)
            .map(|channel| (channel, a[channel].abs_diff(b[channel])))
            .fold(
                (0, 0),
                |most, next| if next.1 > most.1 { next } else { most },
            );
        for (count, threshold) in self.above.iter_mut().zip(THRESHOLDS) {
            *count += u64::from(difference > threshold);
        }
        if difference > self.largest_difference() {
            self.largest = Some(Spot {
                frame,
                x,
                y,
                channel,
                difference,
                a,
                b,
            });
        }
    }

    /// The largest difference in any channel, 0 when every pixel matches.
    pub fn largest_difference(&self) -> u8 {
        self.largest.map_or(0, |spot| spot.difference)
    }
}

/// What a comparison found.
#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    /// Inside `within`, or the whole picture without it.
    pub inside: Region,
    /// Outside `within`; none without it.
    pub outside: Option<Region>,
    /// Inside is within the tolerance and outside matches exactly.
    pub passed: bool,
}

/// Compares `a` and `b` pixel by pixel. An error says why they cannot be:
/// their sizes, frame counts, frame areas or frame delays differ.
pub fn compare(a: &Picture, b: &Picture, options: &Options) -> Result<Comparison, String> {
    if (a.width, a.height) != (b.width, b.height) {
        return Err(format!(
            "the sizes differ: {}x{} and {}x{}",
            a.width, a.height, b.width, b.height
        ));
    }
    if a.frames.len() != b.frames.len() {
        return Err(format!(
            "the frame counts differ: {} and {}",
            a.frames.len(),
            b.frames.len()
        ));
    }
    let mut inside = Region::default();
    let mut outside = options.within.map(|_| Region::default());
    for (index, (frame_a, frame_b)) in a.frames.iter().zip(&b.frames).enumerate() {
        if frame_a.area != frame_b.area {
            return Err(format!(
                "frame {index} covers {} in one and {} in the other",
                frame_a.area, frame_b.area
            ));
        }
        if frame_a.delay != frame_b.delay {
            return Err(format!(
                "frame {index} shows for {}/{} s in one and {}/{} s in the other",
                frame_a.delay.0, frame_a.delay.1, frame_b.delay.0, frame_b.delay.1
            ));
        }
        let area = frame_a.area;
        for (i, (&pixel_a, &pixel_b)) in frame_a.pixels.iter().zip(&frame_b.pixels).enumerate() {
            let at = (
                area.x + i as u32 % area.width.max(1),
                area.y + i as u32 / area.width.max(1),
            );
            let region = match (options.within, outside.as_mut()) {
                (Some(within), Some(outside)) if !within.contains(at.0, at.1) => outside,
                _ => &mut inside,
            };
            region.add(index, at, pixel_a, pixel_b);
        }
    }
    let passed = inside.largest_difference() <= options.tolerance
        && outside
            .as_ref()
            .is_none_or(|outside| outside.largest_difference() == 0);
    Ok(Comparison {
        inside,
        outside,
        passed,
    })
}

/// Reads a PNG, still or animated, into RGBA frames.
pub fn read(path: &Path) -> Result<Picture, String> {
    let shown = path.display();
    let file = File::open(path).map_err(|error| format!("{shown}: {error}"))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("{shown}: {error}"))?;
    let (width, height) = (reader.info().width, reader.info().height);
    // A film's first frame is usually its default image too; when it is
    // not, the default image is read as a frame of its own.
    let count = match reader.info().animation_control {
        None => 1,
        Some(control) => {
            control.num_frames as usize + usize::from(reader.info().frame_control.is_none())
        }
    };
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| format!("{shown}: too large"))?;
    let mut buffer = vec![0; size];
    let mut frames = Vec::with_capacity(count);
    for _ in 0..count {
        let output = reader
            .next_frame(&mut buffer)
            .map_err(|error| format!("{shown}, frame {}: {error}", frames.len()))?;
        let (area, delay) = match reader.info().frame_control {
            Some(control) if reader.info().animation_control.is_some() => (
                Area {
                    x: control.x_offset,
                    y: control.y_offset,
                    width: control.width,
                    height: control.height,
                },
                (control.delay_num, control.delay_den),
            ),
            _ => (
                Area {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
                (0, 1),
            ),
        };
        let pixels = rgba(&buffer[..output.buffer_size()], output.color_type)
            .ok_or_else(|| format!("{shown}: {:?} pixels", output.color_type))?;
        frames.push(Frame {
            area,
            delay,
            pixels,
        });
    }
    Ok(Picture {
        width,
        height,
        frames,
    })
}

/// 8-bit samples of `color_type` as RGBA.
fn rgba(samples: &[u8], color_type: png::ColorType) -> Option<Vec<[u8; 4]>> {
    use png::ColorType::{Grayscale, GrayscaleAlpha, Rgb, Rgba};
    Some(match color_type {
        Grayscale => samples.iter().map(|&g| [g, g, g, 255]).collect(),
        GrayscaleAlpha => samples
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[g, a]| [g, g, g, a])
            .collect(),
        Rgb => samples
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        Rgba => samples.as_chunks::<4>().0.to_vec(),
        png::ColorType::Indexed => return None,
    })
}

/// Where the cover is in a picture `width` by `height` pixels: a shot of
/// the window, or a frame of the evening film with the window centered on
/// its screen. None for any other size, such as the Scale plate.
pub fn cover_area(width: u32, height: u32) -> Option<Area> {
    let fits = |points: egui::Vec2| {
        let scale = width as f32 / points.x;
        ((points.y * scale).round() as u32 == height).then_some(scale)
    };
    let window_at = |scale: f32| {
        // As `demo::on_screen` centers the window, in whole pixels.
        let window = (theme::WINDOW * scale).round();
        egui::vec2(
            ((width as f32 - window.x) / 2.0).floor().max(0.0),
            ((height as f32 - window.y) / 2.0).floor().max(0.0),
        )
    };
    let (scale, offset) = match fits(theme::WINDOW) {
        Some(scale) => (scale, egui::Vec2::ZERO),
        None => {
            let scale = fits(crate::demo::SCREEN)?;
            (scale, window_at(scale))
        }
    };
    let min = (COVER_RECT.min.to_vec2() * scale + offset).floor();
    let max = (COVER_RECT.max.to_vec2() * scale + offset).ceil();
    Some(Area {
        x: min.x as u32,
        y: min.y as u32,
        width: (max.x - min.x) as u32,
        height: (max.y - min.y) as u32,
    })
}

/// The difference between two frames of one area, amplified to be seen:
/// `a` in dim gray, each pixel that differs in magenta, brighter the more it
/// differs, and `within` outlined in cyan. RGB, the frame's size.
pub fn amplified(a: &Frame, b: &Frame, within: Option<Area>) -> Vec<u8> {
    let area = a.area;
    let mut rgb = Vec::with_capacity(a.pixels.len() * 3);
    for (i, (pa, pb)) in a.pixels.iter().zip(&b.pixels).enumerate() {
        let (x, y) = (
            area.x + i as u32 % area.width.max(1),
            area.y + i as u32 / area.width.max(1),
        );
        let difference = (0..4).map(|c| pa[c].abs_diff(pb[c])).max().unwrap_or(0);
        let edge = within.is_some_and(|within| {
            within.contains(x, y)
                && (x == within.x
                    || y == within.y
                    || x.saturating_add(1) == within.right()
                    || y.saturating_add(1) == within.bottom())
        });
        let pixel = if difference > 0 {
            let bright = (64 + u32::from(difference) * 16).min(255) as u8;
            [bright, 0, bright]
        } else if edge {
            [0, 200, 200]
        } else {
            let luma = (u32::from(pa[0]) * 3 + u32::from(pa[1]) * 6 + u32::from(pa[2])) / 10;
            [(luma / 4) as u8; 3]
        };
        rgb.extend_from_slice(&pixel);
    }
    rgb
}

/// The frame `--out` pictures. Where a region fails: the frame of its
/// largest difference, the region that is furthest past its allowance (the
/// tolerance inside, nothing outside) if both fail. Otherwise, the frame
/// that differs most, or the first when the pictures match.
fn frame_to_show(comparison: &Comparison, tolerance: u8) -> usize {
    let regions = [
        Some((&comparison.inside, tolerance)),
        comparison.outside.as_ref().map(|outside| (outside, 0)),
    ];
    let spots = regions
        .into_iter()
        .flatten()
        .filter_map(|(region, allowed)| region.largest.map(|spot| (spot, allowed)));
    let failing = spots
        .clone()
        .filter(|(spot, allowed)| spot.difference > *allowed)
        .max_by_key(|(spot, allowed)| spot.difference - allowed);
    failing
        .or_else(|| spots.max_by_key(|(spot, _)| spot.difference))
        .map_or(0, |(spot, _)| spot.frame)
}

/// Runs `--demo-diff`: prints what differs and returns the exit code: 0
/// within the tolerance, 1 past it or when the pictures cannot be compared,
/// 2 when a file cannot be read or the difference image written.
pub fn run(path_a: &Path, path_b: &Path, options: &Options) -> i32 {
    let (a, b) = match (read(path_a), read(path_b)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("demo-diff: {error}");
            return 2;
        }
    };
    let frames = |picture: &Picture| match picture.frames.len() {
        1 => "1 frame".to_owned(),
        count => format!("{count} frames"),
    };
    println!(
        "a: {} ({}x{}, {})",
        path_a.display(),
        a.width,
        a.height,
        frames(&a)
    );
    println!(
        "b: {} ({}x{}, {})",
        path_b.display(),
        b.width,
        b.height,
        frames(&b)
    );
    let same_bytes = std::fs::read(path_a).ok() == std::fs::read(path_b).ok();
    println!(
        "The files are {}byte-identical.",
        if same_bytes { "" } else { "not " }
    );
    match cover_area(a.width, a.height) {
        Some(cover) => println!("The cover in pictures of this size: {cover} (for --within)."),
        None => println!("No cover in a picture of this size."),
    }
    let comparison = match compare(&a, &b, options) {
        Ok(comparison) => comparison,
        Err(error) => {
            println!("Cannot compare them pixel by pixel: {error}.");
            return 1;
        }
    };
    match options.within {
        Some(within) => {
            print_region(&format!("Inside {within}"), &comparison.inside);
            if let Some(outside) = &comparison.outside {
                print_region("Outside it", outside);
            }
        }
        None => print_region("The whole picture", &comparison.inside),
    }
    if let Some(out) = &options.out {
        let worst = frame_to_show(&comparison, options.tolerance);
        let (frame_a, frame_b) = (&a.frames[worst], &b.frames[worst]);
        if let Err(error) = write_rgb(
            out,
            frame_a.area.width,
            frame_a.area.height,
            &amplified(frame_a, frame_b, options.within),
        ) {
            eprintln!("demo-diff: {}: {error}", out.display());
            return 2;
        }
        println!(
            "Wrote frame {worst}'s difference, amplified, to {}.",
            out.display()
        );
    }
    let rule = match options.within {
        Some(within) => format!(
            "at most {} inside {within}, none outside it",
            options.tolerance
        ),
        None => format!("at most {}", options.tolerance),
    };
    if comparison.passed {
        println!("Passed: {rule}.");
        0
    } else {
        println!("Failed: the allowance is {rule}.");
        1
    }
}

fn print_region(name: &str, region: &Region) {
    let share = |count: u64| 100.0 * count as f64 / region.pixels.max(1) as f64;
    match region.largest {
        None => println!("{name}: {} pixels, none differ.", region.pixels),
        Some(spot) => {
            println!(
                "{name}: {} pixels; the largest difference is {} of 255, in {} at {},{} of frame {} \
                 (a {:?}, b {:?}).",
                region.pixels,
                spot.difference,
                CHANNELS[spot.channel],
                spot.x,
                spot.y,
                spot.frame,
                spot.a,
                spot.b
            );
            let counts: Vec<String> = THRESHOLDS
                .iter()
                .zip(region.above)
                .map(|(threshold, count)| {
                    format!("above {threshold}: {count} ({:.3}%)", share(count))
                })
                .collect();
            println!("  {}", counts.join(", "));
        }
    }
}

fn write_rgb(path: &Path, width: u32, height: u32, rgb: &[u8]) -> Result<(), String> {
    let text = |error: png::EncodingError| error.to_string();
    let file = File::create(path).map_err(|error| error.to_string())?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(text)?;
    writer.write_image_data(rgb).map_err(text)?;
    writer.finish().map_err(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAY: [u8; 4] = [100, 120, 140, 255];

    /// A fresh folder for a test's files, removed when it goes out of scope.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("arrakis-diff-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("a folder for the test's files");
            Self(path)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn area(x: u32, y: u32, width: u32, height: u32) -> Area {
        Area {
            x,
            y,
            width,
            height,
        }
    }

    /// A still `width` by `height`, every pixel [`GRAY`].
    fn flat(width: u32, height: u32) -> Picture {
        Picture {
            width,
            height,
            frames: vec![Frame {
                area: area(0, 0, width, height),
                delay: (0, 1),
                pixels: vec![GRAY; (width * height) as usize],
            }],
        }
    }

    /// A film of full frames, one for each of `delays`, in milliseconds.
    fn film(width: u32, height: u32, delays: &[u16]) -> Picture {
        let mut picture = flat(width, height);
        let frame = picture.frames.remove(0);
        picture.frames = delays
            .iter()
            .map(|&ms| Frame {
                delay: (ms, 1000),
                ..frame.clone()
            })
            .collect();
        picture
    }

    /// `picture` with the pixel at `x`,`y` of its frame `index` replaced.
    fn with(picture: &Picture, index: usize, (x, y): (u32, u32), pixel: [u8; 4]) -> Picture {
        let mut changed = picture.clone();
        let frame = &mut changed.frames[index];
        let at = (y - frame.area.y) * frame.area.width + (x - frame.area.x);
        frame.pixels[at as usize] = pixel;
        changed
    }

    /// `pixel` with `channel` raised by `by`.
    fn raised(pixel: [u8; 4], channel: usize, by: u8) -> [u8; 4] {
        let mut raised = pixel;
        raised[channel] += by;
        raised
    }

    fn options(tolerance: u8) -> Options {
        Options {
            tolerance,
            ..Options::default()
        }
    }

    /// Writes `picture` as a PNG: a still as RGBA, a film as an animated PNG.
    fn write(path: &Path, picture: &Picture) {
        let file = BufWriter::new(File::create(path).expect("a test file"));
        let mut encoder = png::Encoder::new(file, picture.width, picture.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let film = picture.frames.len() > 1;
        if film {
            encoder
                .set_animated(picture.frames.len() as u32, 0)
                .expect("an animation");
        }
        let mut writer = encoder.write_header().expect("a header");
        for frame in &picture.frames {
            if film {
                writer
                    .set_frame_delay(frame.delay.0, frame.delay.1)
                    .expect("a delay");
                writer
                    .set_frame_dimension(frame.area.width, frame.area.height)
                    .expect("a size");
                writer
                    .set_frame_position(frame.area.x, frame.area.y)
                    .expect("a position");
            }
            writer
                .write_image_data(&frame.pixels.concat())
                .expect("pixels");
        }
        writer.finish().expect("a finished PNG");
    }

    /// Writes a still of `color` samples, as they are.
    fn write_raw(path: &Path, size: (u32, u32), color: png::ColorType, samples: &[u8]) {
        let file = BufWriter::new(File::create(path).expect("a test file"));
        let mut encoder = png::Encoder::new(file, size.0, size.1);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("a header");
        writer.write_image_data(samples).expect("pixels");
        writer.finish().expect("a finished PNG");
    }

    // compare

    #[test]
    fn identical_pictures_pass_with_nothing_to_report() {
        let a = flat(4, 3);
        let found = compare(&a, &a.clone(), &Options::default()).expect("comparable");
        assert!(found.passed);
        assert_eq!(found.inside.pixels, 12);
        assert_eq!(found.inside.largest, None);
        assert_eq!(found.inside.above, [0; THRESHOLDS.len()]);
        assert_eq!(found.outside, None);
    }

    #[test]
    fn a_difference_passes_only_up_to_the_tolerance() {
        let a = flat(4, 3);
        let off = raised(GRAY, 1, 3);
        let b = with(&a, 0, (2, 1), off);
        assert!(!compare(&a, &b, &options(2)).unwrap().passed);
        let found = compare(&a, &b, &options(3)).unwrap();
        assert!(found.passed);
        assert_eq!(
            found.inside.largest,
            Some(Spot {
                frame: 0,
                x: 2,
                y: 1,
                channel: 1,
                difference: 3,
                a: GRAY,
                b: off,
            })
        );
    }

    #[test]
    fn the_largest_spot_is_the_first_of_the_largest_in_reading_order() {
        let a = flat(4, 3);
        let b = with(&a, 0, (3, 0), raised(GRAY, 0, 4));
        let b = with(&b, 0, (1, 1), raised(GRAY, 2, 7));
        let b = with(&b, 0, (2, 2), raised(GRAY, 0, 7));
        let spot = compare(&a, &b, &Options::default())
            .unwrap()
            .inside
            .largest
            .expect("a spot");
        assert_eq!(
            (spot.x, spot.y, spot.channel, spot.difference),
            (1, 1, 2, 7)
        );
    }

    #[test]
    fn the_largest_spot_names_the_frame_and_the_alpha_channel() {
        let a = film(2, 2, &[100, 100]);
        let faint = with(&a, 0, (0, 0), raised(GRAY, 0, 4));
        let b = with(&faint, 1, (1, 1), [100, 120, 140, 246]);
        let spot = compare(&a, &b, &Options::default())
            .unwrap()
            .inside
            .largest
            .expect("a spot");
        assert_eq!(
            (spot.frame, spot.x, spot.y, spot.channel, spot.difference),
            (1, 1, 1, 3, 9)
        );
        // The same difference in both frames: the first frame is the spot.
        let b = with(&a, 0, (0, 1), raised(GRAY, 0, 9));
        let b = with(&b, 1, (0, 0), raised(GRAY, 0, 9));
        let spot = compare(&a, &b, &Options::default())
            .unwrap()
            .inside
            .largest
            .expect("a spot");
        assert_eq!((spot.frame, spot.x, spot.y), (0, 0, 1));
    }

    #[test]
    fn a_pixel_counts_under_each_threshold_its_difference_exceeds() {
        let a = flat(4, 3);
        // One pixel off by 5 in two channels, and one off by 1.
        let b = with(&a, 0, (0, 0), [105, 125, 140, 255]);
        let b = with(&b, 0, (1, 0), raised(GRAY, 2, 1));
        let region = compare(&a, &b, &Options::default()).unwrap().inside;
        let expected =
            THRESHOLDS.map(|threshold| u64::from(5 > threshold) + u64::from(1 > threshold));
        assert_eq!(region.above, expected);
        // The pixel off by 5 straddles the thresholds, so this checks both
        // sides of the comparison.
        assert!(THRESHOLDS.iter().any(|&threshold| threshold < 5));
        assert!(THRESHOLDS.iter().any(|&threshold| threshold >= 5));
        // The count is of pixels, not of channels.
        assert!(region.above.iter().all(|&count| count <= 2));
    }

    #[test]
    fn a_difference_inside_the_area_may_reach_the_tolerance() {
        let a = flat(4, 4);
        let within = area(1, 1, 2, 2);
        let b = with(&a, 0, (2, 2), raised(GRAY, 0, 3));
        let with_area = |tolerance| Options {
            tolerance,
            within: Some(within),
            out: None,
        };
        let found = compare(&a, &b, &with_area(3)).unwrap();
        assert!(found.passed);
        assert_eq!(found.inside.pixels, 4);
        assert_eq!(found.inside.largest_difference(), 3);
        let outside = found.outside.expect("the part outside");
        assert_eq!(outside.pixels, 12);
        assert_eq!(outside.largest, None);
        assert!(!compare(&a, &b, &with_area(2)).unwrap().passed);
    }

    #[test]
    fn any_difference_outside_the_area_fails_whatever_the_tolerance() {
        let a = flat(4, 4);
        let b = with(&a, 0, (2, 2), raised(GRAY, 0, 2));
        let b = with(&b, 0, (0, 0), raised(GRAY, 0, 1));
        let within = Options {
            tolerance: 255,
            within: Some(area(1, 1, 2, 2)),
            out: None,
        };
        let found = compare(&a, &b, &within).unwrap();
        assert!(!found.passed);
        assert_eq!(found.inside.largest_difference(), 2);
        let spot = found.outside.unwrap().largest.expect("a spot outside");
        assert_eq!((spot.x, spot.y, spot.difference), (0, 0, 1));
        // The same pictures pass when the tolerance holds everywhere.
        let found = compare(&a, &b, &options(255)).unwrap();
        assert!(found.passed);
        assert_eq!(found.outside, None);
    }

    #[test]
    fn pictures_that_cannot_be_laid_over_each_other_are_an_error() {
        let a = flat(4, 3);
        let error = |b: &Picture| compare(&a, b, &Options::default()).expect_err("an error");
        assert!(error(&flat(3, 4)).contains("sizes"));
        assert!(error(&film(4, 3, &[100, 100])).contains("frame counts"));
        let mut moved = a.clone();
        moved.frames[0].area.x = 1;
        assert!(error(&moved).contains("covers"));
        let mut slower = a.clone();
        slower.frames[0].delay = (5, 100);
        assert!(error(&slower).contains("shows for"));
    }

    // read

    #[test]
    fn stills_of_any_color_type_come_back_as_rgba() {
        let folder = Folder::new("stills");
        let whole = |picture: &Picture| {
            let [frame] = &picture.frames[..] else {
                panic!("one frame, got {}", picture.frames.len());
            };
            assert_eq!(frame.area, area(0, 0, picture.width, picture.height));
            assert_eq!(frame.delay, (0, 1));
            frame.pixels.clone()
        };

        let path = folder.file("rgba.png");
        write_raw(
            &path,
            (2, 1),
            png::ColorType::Rgba,
            &[1, 2, 3, 4, 5, 6, 7, 8],
        );
        let picture = read(&path).unwrap();
        assert_eq!((picture.width, picture.height), (2, 1));
        assert_eq!(whole(&picture), [[1, 2, 3, 4], [5, 6, 7, 8]]);

        let path = folder.file("rgb.png");
        write_raw(&path, (2, 1), png::ColorType::Rgb, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(
            whole(&read(&path).unwrap()),
            [[1, 2, 3, 255], [4, 5, 6, 255]]
        );

        let path = folder.file("gray.png");
        write_raw(&path, (2, 2), png::ColorType::Grayscale, &[7, 8, 9, 10]);
        assert_eq!(
            whole(&read(&path).unwrap()),
            [
                [7, 7, 7, 255],
                [8, 8, 8, 255],
                [9, 9, 9, 255],
                [10, 10, 10, 255]
            ]
        );
    }

    #[test]
    fn a_film_comes_back_frame_by_frame_with_its_delays() {
        let folder = Folder::new("film");
        let path = folder.file("film.png");
        let mut film = film(3, 2, &[100, 250, 40]);
        film.frames[1].pixels[4] = [9, 8, 7, 255];
        // A frame may cover part of the picture, further in.
        film.frames[2].area = area(1, 1, 2, 1);
        film.frames[2].pixels = vec![[1, 1, 1, 255], [2, 2, 2, 255]];
        write(&path, &film);
        let read_back = read(&path).unwrap();
        let delays: Vec<_> = read_back.frames.iter().map(|frame| frame.delay).collect();
        assert_eq!(delays, [(100, 1000), (250, 1000), (40, 1000)]);
        assert_eq!(read_back, film);
    }

    #[test]
    fn films_that_differ_only_in_one_frame_say_which() {
        let folder = Folder::new("films");
        let a = film(3, 2, &[100, 100, 100]);
        let b = with(&a, 2, (2, 1), raised(GRAY, 0, 20));
        write(&folder.file("a.png"), &a);
        write(&folder.file("b.png"), &b);
        let (a, b) = (
            read(&folder.file("a.png")).unwrap(),
            read(&folder.file("b.png")).unwrap(),
        );
        let found = compare(&a, &b, &Options::default()).unwrap();
        assert!(!found.passed);
        let spot = found.inside.largest.expect("a spot");
        assert_eq!((spot.frame, spot.x, spot.y), (2, 2, 1));
    }

    #[test]
    fn a_missing_file_is_an_error_that_names_it() {
        let folder = Folder::new("missing");
        let path = folder.file("nothing.png");
        let error = read(&path).expect_err("no such file");
        assert!(error.contains(&path.display().to_string()), "{error}");
    }

    // Area

    #[test]
    fn an_area_parses_from_four_numbers() {
        assert_eq!("1,2,3,4".parse(), Ok(area(1, 2, 3, 4)));
        assert_eq!(" 1, 2 ,3,4".parse(), Ok(area(1, 2, 3, 4)));
        for bad in ["1,2,3", "1,2,3,4,5", "a,b,c,d", "", "1,2,3,-4"] {
            assert!(bad.parse::<Area>().is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn an_area_prints_as_it_parses() {
        let area = area(36, 8, 272, 272);
        assert_eq!(area.to_string().parse(), Ok(area));
    }

    #[test]
    fn an_area_holds_its_first_row_and_column_and_stops_at_its_size() {
        let a = area(2, 3, 4, 5);
        assert!(a.contains(2, 3));
        assert!(a.contains(5, 7));
        assert!(a.contains(2, 7) && a.contains(5, 3));
        assert!(!a.contains(6, 3), "one past the width");
        assert!(!a.contains(2, 8), "one past the height");
        assert!(!a.contains(1, 3) && !a.contains(2, 2));
    }

    // cover_area

    /// The cover's rectangle in a picture `scale` times the window's size.
    fn scaled_cover(scale: u32) -> Area {
        let scale = scale as f32;
        let rect = COVER_RECT;
        area(
            (rect.min.x * scale) as u32,
            (rect.min.y * scale) as u32,
            (rect.width() * scale) as u32,
            (rect.height() * scale) as u32,
        )
    }

    fn window_pixels(scale: u32) -> (u32, u32) {
        (
            (theme::WINDOW.x * scale as f32) as u32,
            (theme::WINDOW.y * scale as f32) as u32,
        )
    }

    #[test]
    fn the_cover_is_the_rectangle_scaled_in_a_shot_of_the_window() {
        for scale in [1, 2] {
            let (width, height) = window_pixels(scale);
            assert_eq!(cover_area(width, height), Some(scaled_cover(scale)));
        }
    }

    #[test]
    fn the_cover_is_offset_by_the_centered_window_in_the_evening_film() {
        let (width, height) = (1600, 1360);
        let (window_width, window_height) = window_pixels(2);
        let (left, top) = ((width - window_width) / 2, (height - window_height) / 2);
        let cover = scaled_cover(2);
        assert_eq!(
            cover_area(width, height),
            Some(area(
                cover.x + left,
                cover.y + top,
                cover.width,
                cover.height
            ))
        );
    }

    #[test]
    fn there_is_no_cover_in_a_picture_of_another_size() {
        // The Scale plate, and a size that is neither a shot nor a film.
        assert_eq!(cover_area(1600, 368), None);
        assert_eq!(cover_area(100, 100), None);
    }

    #[test]
    fn the_cover_area_is_where_the_window_draws_the_cover() {
        use crate::ui::headless::Headless;

        let mut window = Headless::new(2.0);
        window.frame();
        let frame = window.frame();
        // The font atlas is egui's first texture; the cover's six are the
        // others, back to front: the back, the maw, the rim's light, the
        // shade at the sand, the gap and the front.
        let covers: Vec<_> = frame.cover_meshes().into_iter().map(|(_, m)| m).collect();
        assert_eq!(covers.len(), 6, "six pictures besides the text");
        // The back fills the cover's rect; the maw's moves with the rise.
        let corners = covers[0].vertices.iter().map(|vertex| vertex.pos * 2.0);
        let min_x = corners.clone().map(|c| c.x).fold(f32::MAX, f32::min);
        let min_y = corners.clone().map(|c| c.y).fold(f32::MAX, f32::min);
        let max_x = corners.clone().map(|c| c.x).fold(f32::MIN, f32::max);
        let max_y = corners.map(|c| c.y).fold(f32::MIN, f32::max);
        let (width, height) = window_pixels(2);
        assert_eq!(
            cover_area(width, height),
            Some(area(
                min_x.round() as u32,
                min_y.round() as u32,
                (max_x - min_x).round() as u32,
                (max_y - min_y).round() as u32,
            ))
        );
    }

    // amplified

    fn pixel_of(rgb: &[u8], frame: &Frame, x: u32, y: u32) -> [u8; 3] {
        let at = ((y * frame.area.width + x) * 3) as usize;
        [rgb[at], rgb[at + 1], rgb[at + 2]]
    }

    #[test]
    fn the_amplified_difference_shows_what_differs_and_where_it_may() {
        let bright = [200, 200, 200, 255];
        let a = Frame {
            area: area(0, 0, 6, 5),
            delay: (0, 1),
            pixels: vec![bright; 30],
        };
        let mut b = a.clone();
        b.pixels[1] = raised(bright, 0, 1); // (1, 0)
        b.pixels[3] = raised(bright, 1, 10); // (3, 0)
        b.pixels[4 * 6 + 5] = raised(bright, 2, 4); // (5, 4)
        let within = area(1, 1, 4, 3);
        let rgb = amplified(&a, &b, Some(within));
        assert_eq!(rgb.len(), 6 * 5 * 3);
        let at = |x, y| pixel_of(&rgb, &a, x, y);

        // A differing pixel is magenta, brighter the more it differs.
        let (small, large) = (at(1, 0), at(3, 0));
        for magenta in [small, large, at(5, 4)] {
            assert_eq!(magenta[1], 0);
            assert_eq!(magenta[0], magenta[2]);
            assert!(magenta[0] > 0);
        }
        assert!(large[0] > small[0], "{large:?} against {small:?}");
        assert!(at(5, 4)[0] > small[0]);
        // The outline of `within` is cyan, its inside and the rest dim gray.
        for (x, y) in [(1, 1), (4, 1), (1, 3), (4, 3), (2, 1), (3, 3), (1, 2)] {
            assert_eq!(at(x, y), [0, 200, 200], "outline at {x},{y}");
        }
        for (x, y) in [(2, 2), (3, 2), (0, 0), (0, 4), (5, 2), (2, 4)] {
            let gray = at(x, y);
            assert!(gray[0] == gray[1] && gray[1] == gray[2], "{gray:?}");
            assert!(gray[0] < 200, "dim: {gray:?}");
            assert!(gray[0] > 0, "still there: {gray:?}");
        }
        // Without an area there is no outline.
        let plain = amplified(&a, &b, None);
        assert_eq!(pixel_of(&plain, &a, 1, 1), at(0, 0));
    }

    #[test]
    fn an_area_past_u32_max_outlines_the_edges_that_are_in_the_picture() {
        // `Area::contains` stops such an area at u32::MAX; so does the
        // outline, whose right and bottom edges are then far past the
        // picture. Adding the corner without saturating overflowed.
        let a = Frame {
            area: area(0, 0, 6, 5),
            delay: (0, 1),
            pixels: vec![GRAY; 30],
        };
        let within = area(2, 2, u32::MAX, u32::MAX);
        assert!(within.contains(5, 4));
        let rgb = amplified(&a, &a.clone(), Some(within));
        let at = |x, y| pixel_of(&rgb, &a, x, y);
        // The left and top edges are in the picture.
        for (x, y) in [(2, 2), (2, 3), (2, 4), (3, 2), (5, 2)] {
            assert_eq!(at(x, y), [0, 200, 200], "outline at {x},{y}");
        }
        // The right and bottom ones are not, and the picture's own edges
        // are no outline.
        for (x, y) in [(3, 3), (5, 3), (5, 4), (4, 4), (0, 0)] {
            assert_ne!(at(x, y), [0, 200, 200], "no outline at {x},{y}");
        }
    }

    #[test]
    fn an_area_ends_where_its_edges_are_even_past_u32_max() {
        let past = area(u32::MAX - 1, 1, 10, 3);
        assert_eq!((past.right(), past.bottom()), (u32::MAX, 4));
        assert!(past.contains(u32::MAX - 1, 1));
        assert!(!past.contains(u32::MAX, 1), "the end is not inside");
    }

    // run

    #[test]
    fn run_exits_zero_within_the_tolerance_and_one_past_it() {
        let folder = Folder::new("run");
        let a = flat(4, 3);
        let b = with(&a, 0, (1, 1), raised(GRAY, 0, 5));
        let (path_a, path_same, path_b) = (
            folder.file("a.png"),
            folder.file("same.png"),
            folder.file("b.png"),
        );
        write(&path_a, &a);
        write(&path_same, &a);
        write(&path_b, &b);
        assert_eq!(run(&path_a, &path_same, &Options::default()), 0);
        assert_eq!(run(&path_a, &path_b, &options(0)), 1);
        assert_eq!(run(&path_a, &path_b, &options(4)), 1);
        assert_eq!(run(&path_a, &path_b, &options(5)), 0);
        let outside = Options {
            tolerance: 255,
            within: Some(area(2, 0, 2, 3)),
            out: None,
        };
        assert_eq!(run(&path_a, &path_b, &outside), 1);
    }

    #[test]
    fn run_exits_one_for_pictures_that_cannot_be_compared() {
        let folder = Folder::new("sizes");
        let (path_a, path_b) = (folder.file("a.png"), folder.file("b.png"));
        write(&path_a, &flat(4, 3));
        write(&path_b, &flat(3, 4));
        assert_eq!(run(&path_a, &path_b, &options(255)), 1);
    }

    #[test]
    fn run_exits_two_for_a_missing_file() {
        let folder = Folder::new("gone");
        let (path, gone) = (folder.file("a.png"), folder.file("gone.png"));
        write(&path, &flat(4, 3));
        assert_eq!(run(&path, &gone, &Options::default()), 2);
        assert_eq!(run(&gone, &path, &Options::default()), 2);
    }

    #[test]
    fn run_writes_the_worst_frames_difference_at_its_size() {
        let folder = Folder::new("out");
        // The second frame covers only part of the picture.
        let mut a = film(6, 4, &[100, 100]);
        a.frames[1].area = area(1, 1, 3, 2);
        a.frames[1].pixels = vec![GRAY; 6];
        let b = with(&a, 1, (2, 2), raised(GRAY, 0, 9));
        let (path_a, path_b, out) = (
            folder.file("a.png"),
            folder.file("b.png"),
            folder.file("out.png"),
        );
        write(&path_a, &a);
        write(&path_b, &b);
        let with_out = Options {
            out: Some(out.clone()),
            ..Options::default()
        };
        assert_eq!(run(&path_a, &path_b, &with_out), 1);
        assert_eq!(png_size(&out), (3, 2));
    }

    /// The width and height of the PNG at `path`.
    fn png_size(path: &Path) -> (u32, u32) {
        let decoder = png::Decoder::new(BufReader::new(File::open(path).expect("the output")));
        let info = decoder.read_info().expect("a PNG").info().clone();
        (info.width, info.height)
    }

    /// A film of two frames: the first covers the picture (6 by 4), the
    /// second only 3 by 2 pixels of it, from 1,1. The size of the PNG that
    /// `--out` writes tells which frame it pictured.
    fn two_sizes_of_frame() -> Picture {
        let mut film = film(6, 4, &[100, 100]);
        film.frames[1].area = area(1, 1, 3, 2);
        film.frames[1].pixels = vec![GRAY; 6];
        film
    }

    #[test]
    fn run_writes_the_frame_where_a_region_fails_not_the_one_that_differs_most() {
        let folder = Folder::new("out-failed");
        let a = two_sizes_of_frame();
        // In frame 0, inside `within`, a difference of 9, which the tolerance
        // of 10 allows. In frame 1, outside it, a difference of 1, which
        // nothing allows: the comparison fails there.
        let within = area(0, 0, 3, 3);
        let b = with(&a, 0, (1, 1), raised(GRAY, 0, 9));
        let b = with(&b, 1, (3, 2), raised(GRAY, 0, 1));
        let (path_a, path_b, out) = (
            folder.file("a.png"),
            folder.file("b.png"),
            folder.file("out.png"),
        );
        write(&path_a, &a);
        write(&path_b, &b);
        let options = Options {
            tolerance: 10,
            within: Some(within),
            out: Some(out.clone()),
        };
        let found = compare(&a, &b, &options).expect("comparable");
        assert_eq!(found.inside.largest.map(|spot| spot.frame), Some(0));
        assert_eq!(
            found.outside.and_then(|r| r.largest).map(|s| s.frame),
            Some(1)
        );
        assert_eq!(run(&path_a, &path_b, &options), 1);
        assert_eq!(png_size(&out), (3, 2), "frame 1, where the outside fails");
    }

    #[test]
    fn run_writes_the_frame_that_differs_most_when_the_comparison_passes() {
        let folder = Folder::new("out-passed");
        let a = two_sizes_of_frame();
        // Both within the tolerance of 10; the larger is in frame 0.
        let b = with(&a, 0, (1, 1), raised(GRAY, 0, 9));
        let b = with(&b, 1, (3, 2), raised(GRAY, 0, 2));
        let (path_a, path_b, out) = (
            folder.file("a.png"),
            folder.file("b.png"),
            folder.file("out.png"),
        );
        write(&path_a, &a);
        write(&path_b, &b);
        let options = Options {
            tolerance: 10,
            out: Some(out.clone()),
            ..Options::default()
        };
        assert_eq!(run(&path_a, &path_b, &options), 0);
        assert_eq!(png_size(&out), (6, 4), "frame 0, which differs most");
    }

    #[test]
    fn the_frame_to_show_is_where_a_region_fails_else_where_it_differs_most() {
        let a = film(4, 3, &[100, 100, 100]);
        // Columns 0 and 1 are inside; the tolerance inside is 4.
        let within = area(0, 0, 2, 3);
        let shown = |changes: &[(usize, (u32, u32), u8)]| {
            let b = changes.iter().fold(a.clone(), |b, &(frame, at, by)| {
                with(&b, frame, at, raised(GRAY, 0, by))
            });
            let options = Options {
                tolerance: 4,
                within: Some(within),
                out: None,
            };
            let found = compare(&a, &b, &options).expect("comparable");
            frame_to_show(&found, options.tolerance)
        };
        // Nothing differs: the first frame.
        assert_eq!(shown(&[]), 0);
        // It passes: the frame that differs most, 3 in frame 1, 4 in frame 2.
        assert_eq!(shown(&[(1, (0, 0), 3), (2, (1, 1), 4)]), 2);
        // Only the inside fails, by 5 in frame 1; frame 2 differs by 4.
        assert_eq!(shown(&[(1, (0, 0), 5), (2, (1, 1), 4)]), 1);
        // Only the outside fails, by 1 in frame 0, though the inside differs
        // by 4 in frame 2.
        assert_eq!(shown(&[(0, (3, 2), 1), (2, (1, 1), 4)]), 0);
        // Both fail: the region further past its allowance. Inside by 6 in
        // frame 0 is 2 past the tolerance; outside by 3 in frame 2 is 3 past
        // nothing.
        assert_eq!(shown(&[(0, (0, 0), 6), (2, (3, 2), 3)]), 2);
    }

    #[test]
    fn run_exits_two_when_the_difference_image_cannot_be_written() {
        let folder = Folder::new("nowhere");
        let (path_a, path_b) = (folder.file("a.png"), folder.file("b.png"));
        write(&path_a, &flat(4, 3));
        write(&path_b, &flat(4, 3));
        let into_nothing = Options {
            out: Some(folder.file("no/such/folder/out.png")),
            ..Options::default()
        };
        assert_eq!(run(&path_a, &path_b, &into_nothing), 2);
    }
}
