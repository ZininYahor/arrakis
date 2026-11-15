//! How red is red: color temperature to per-channel multipliers.
//!
//! The colors are computed rather than copied from a table: Planck's law
//! gives the spectrum of a black body, the analytic fit of the CIE 1931
//! color-matching functions by Wyman, Sloan and Shirley (2013) turns it
//! into XYZ, and the sRGB matrix into linear RGB. Each channel is divided by
//! the 6500 K result, so 6500 K leaves the screen untouched, and the largest
//! channel is scaled to 1. The result is gamma-encoded, because it scales
//! values in a display's transfer table, which are already encoded.
//!
//! Below about 850 K a black body's green falls outside the sRGB gamut and
//! clips to zero, leaving the red channel alone. That is the end of the
//! scale: "pure red".
//!
//! The app does not compute the table every 10 K at start: it reads it from
//! the bytes embedded in the binary (`precomputed`), which a test keeps within
//! a hair of what `build_table` computes, and its two ends exactly at pure red
//! and normal colors.

// Encoded multipliers every 10 K from 850 to 6500 K.
use crate::precomputed::COLOR_TABLE as TABLE;

pub const DAY_KELVIN: u16 = 6500;
/// Where green clips to zero.
pub const PURE_RED_KELVIN: u16 = 850;
pub const MIN_BRIGHTNESS: u8 = 20;
pub const MAX_BRIGHTNESS: u8 = 100;
const KELVIN_STEP: u16 = 10;
/// The usual display gamma, for encoding linear multipliers.
#[cfg(any(test, feature = "demo"))]
const DISPLAY_GAMMA: f64 = 2.2;
/// One step of a display's transfer table, which holds 1,024 values a
/// channel (10 bits). A tint that moves no channel by this much moves no
/// value of the table by a whole step.
pub const TABLE_STEP: f32 = 1.0 / 1023.0;

/// Per-channel multipliers for the transfer table, each from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tint {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

impl Tint {
    pub const NEUTRAL: Self = Self {
        red: 1.0,
        green: 1.0,
        blue: 1.0,
    };

    pub fn is_neutral(&self) -> bool {
        (self.red - 1.0).abs() < 1e-4
            && (self.green - 1.0).abs() < 1e-4
            && (self.blue - 1.0).abs() < 1e-4
    }

    /// The largest difference between `self` and `other` in one channel.
    /// Less than [`TABLE_STEP`] means a display would show them alike.
    pub fn distance(&self, other: Tint) -> f32 {
        (self.red - other.red)
            .abs()
            .max((self.green - other.green).abs())
            .max((self.blue - other.blue).abs())
    }

    /// What to draw so that, seen through a screen already tinted `live`,
    /// it looks like `self`. A channel `live` has turned off stays as it
    /// is, since nothing drawn can bring it back.
    pub fn through(&self, live: Tint) -> Tint {
        let channel = |want: f32, have: f32| {
            if have < 1e-3 {
                want
            } else {
                (want / have).clamp(0.0, 1.0)
            }
        };
        Tint {
            red: channel(self.red, live.red),
            green: channel(self.green, live.green),
            blue: channel(self.blue, live.blue),
        }
    }

    /// The color white light becomes under this tint. A strip of light shows
    /// it: it is what each position of the scale, or the chosen light, looks
    /// like on a screen, so it is white that goes through the tint.
    pub fn light(&self) -> egui::Color32 {
        self.apply(egui::Color32::WHITE)
    }

    /// The color a screen pixel of `color` shows under this tint.
    pub fn apply(&self, color: egui::Color32) -> egui::Color32 {
        let scale = |value: u8, by: f32| (f32::from(value) * by).round().clamp(0.0, 255.0) as u8;
        egui::Color32::from_rgba_unmultiplied(
            scale(color.r(), self.red),
            scale(color.g(), self.green),
            scale(color.b(), self.blue),
            color.a(),
        )
    }
}

/// The night's look: how red and how dim.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Look {
    /// Color temperature in kelvin, from 6500 down to 850 (pure red).
    pub kelvin: u16,
    /// Percent of full brightness, from 20 to 100.
    pub brightness: u8,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            kelvin: 1200,
            brightness: 100,
        }
    }
}

impl Look {
    pub const DAY: Self = Self {
        kelvin: DAY_KELVIN,
        brightness: MAX_BRIGHTNESS,
    };

    pub fn clamped(self) -> Self {
        Self {
            kelvin: self.kelvin.clamp(PURE_RED_KELVIN, DAY_KELVIN),
            brightness: self.brightness.clamp(MIN_BRIGHTNESS, MAX_BRIGHTNESS),
        }
    }

    pub fn is_pure_red(&self) -> bool {
        self.kelvin <= PURE_RED_KELVIN
    }

    /// `level` of the way from normal colors to this look: 0 is the day, 1
    /// is this look. Redness and brightness move together, so a fade passes
    /// through the oranges a sunset does instead of a pink wash.
    pub fn at_level(&self, level: f32) -> Tint {
        let level = level.clamp(0.0, 1.0);
        let redness = redness_of(self.kelvin) * level;
        let brightness = 1.0 + (f32::from(self.brightness) / 100.0 - 1.0) * level;
        let color = color_at_redness(redness);
        Tint {
            red: color[0] * brightness,
            green: color[1] * brightness,
            blue: color[2] * brightness,
        }
    }

    pub fn tint(&self) -> Tint {
        self.at_level(1.0)
    }
}

/// How many rows the table holds: every 10 K from 850 to 6500 K.
pub(crate) const TABLE_LEN: usize = ((DAY_KELVIN - PURE_RED_KELVIN) / KELVIN_STEP) as usize + 1;

/// Computes the table, from Planck's law to the encoded multipliers: the
/// source of the embedded table (`TABLE`). The app does not run it; the
/// golden test that keeps the two within a tolerance and the benchmarks do,
/// so a build without the `demo` feature leaves it out.
#[cfg(any(test, feature = "demo"))]
pub(crate) fn build_table() -> Vec<[f32; 3]> {
    let white = linear_rgb(f64::from(DAY_KELVIN));
    (PURE_RED_KELVIN..=DAY_KELVIN)
        .step_by(usize::from(KELVIN_STEP))
        .map(|kelvin| {
            let rgb = linear_rgb(f64::from(kelvin));
            let relative =
                [rgb[0] / white[0], rgb[1] / white[1], rgb[2] / white[2]].map(|c| c.max(0.0));
            let peak = relative.iter().copied().fold(f64::MIN, f64::max);
            relative.map(|c| ((c / peak).powf(1.0 / DISPLAY_GAMMA)) as f32)
        })
        .collect()
}

fn table_index(kelvin: u16) -> usize {
    usize::from((kelvin.clamp(PURE_RED_KELVIN, DAY_KELVIN) - PURE_RED_KELVIN) / KELVIN_STEP)
}

/// The encoded multipliers at `kelvin`, rounded to the table's 10 K.
pub fn color_at(kelvin: u16) -> [f32; 3] {
    TABLE[table_index(kelvin)]
}

/// How red a temperature looks, from 0 (6500 K) to 1 (pure red): the share
/// of green taken away. Green falls steadily all the way down, so a slider
/// on this scale spends its travel evenly instead of crowding the deep red
/// into its last few points.
pub fn redness_of(kelvin: u16) -> f32 {
    1.0 - color_at(kelvin)[1]
}

/// The temperature with the given redness, to the nearest 10 K.
pub fn kelvin_at_redness(redness: f32) -> u16 {
    let target_green = 1.0 - redness.clamp(0.0, 1.0);
    // Green rises with temperature: find the first entry at or above.
    let index = TABLE.partition_point(|color| color[1] < target_green);
    let index = index.min(TABLE.len() - 1);
    let pick = if index > 0
        && (TABLE[index - 1][1] - target_green).abs() < (TABLE[index][1] - target_green).abs()
    {
        index - 1
    } else {
        index
    };
    PURE_RED_KELVIN + pick as u16 * KELVIN_STEP
}

/// Where pure red begins on the redness scale: [`kelvin_at_redness`] gives
/// 850 K for any redness past this one. Green clips to zero at 850 K, so the
/// nearest step is 850 K from halfway down to 860 K's green.
pub fn pure_red_from() -> f32 {
    1.0 - color_at(PURE_RED_KELVIN + KELVIN_STEP)[1] / 2.0
}

/// The redness of 860 K, the last temperature short of pure red. Every
/// redness from halfway between 870 K's and this one up to
/// [`pure_red_from`] reads 860 K.
pub fn short_of_pure_red() -> f32 {
    redness_of(PURE_RED_KELVIN + KELVIN_STEP)
}

/// What the Temperature scale does to white at `redness`: the tint of a fade
/// that is `redness` of the way to pure red at full brightness. The scale
/// plate and the slider's strip both draw it.
pub fn scale_tint(redness: f32) -> Tint {
    Look {
        kelvin: PURE_RED_KELVIN,
        brightness: MAX_BRIGHTNESS,
    }
    .at_level(redness)
}

/// The encoded multipliers at a redness, interpolated between the table's
/// neighbors so a fade moves smoothly.
fn color_at_redness(redness: f32) -> [f32; 3] {
    let target_green = 1.0 - redness.clamp(0.0, 1.0);
    let index = TABLE.partition_point(|color| color[1] < target_green);
    if index == 0 {
        return TABLE[0];
    }
    if index >= TABLE.len() {
        return TABLE[TABLE.len() - 1];
    }
    let (below, above) = (TABLE[index - 1], TABLE[index]);
    let span = above[1] - below[1];
    let t = if span > 0.0 {
        (target_green - below[1]) / span
    } else {
        0.0
    };
    [0, 1, 2].map(|c| below[c] + (above[c] - below[c]) * t)
}

/// Linear sRGB of a black body at `kelvin` (unnormalized).
#[cfg(any(test, feature = "demo"))]
fn linear_rgb(kelvin: f64) -> [f64; 3] {
    let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
    for nm in (380..=780).step_by(5) {
        let nm = f64::from(nm);
        let power = planck(nm, kelvin);
        let (cx, cy, cz) = cie_1931(nm);
        x += power * cx;
        y += power * cy;
        z += power * cz;
    }
    [
        3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z,
        -0.969_266_0 * x + 1.876_010_8 * y + 0.041_556_0 * z,
        0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z,
    ]
}

/// Spectral radiance of a black body (Planck's law), per meter.
#[cfg(any(test, feature = "demo"))]
fn planck(nm: f64, kelvin: f64) -> f64 {
    const C1: f64 = 3.741_771_852e-16;
    const C2: f64 = 1.438_776_877e-2;
    let meters = nm * 1e-9;
    C1 / (meters.powi(5) * ((C2 / (meters * kelvin)).exp() - 1.0))
}

/// The CIE 1931 2° color-matching functions, as the multi-lobe Gaussian
/// fit of Wyman, Sloan and Shirley, "Simple Analytic Approximations to the
/// CIE XYZ Color Matching Functions" (JCGT, 2013).
#[cfg(any(test, feature = "demo"))]
fn cie_1931(nm: f64) -> (f64, f64, f64) {
    let lobe = |mu: f64, below: f64, above: f64| {
        let t = (nm - mu) / if nm < mu { below } else { above };
        (-0.5 * t * t).exp()
    };
    let x = 1.056 * lobe(599.8, 37.9, 31.0) + 0.362 * lobe(442.0, 16.0, 26.7)
        - 0.065 * lobe(501.1, 20.4, 26.2);
    let y = 0.821 * lobe(568.8, 46.9, 40.5) + 0.286 * lobe(530.9, 16.3, 31.1);
    let z = 1.217 * lobe(437.0, 11.8, 36.0) + 0.681 * lobe(459.0, 26.0, 13.8);
    (x, y, z)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tolerance: f32) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn day_is_neutral() {
        assert!(Look::DAY.tint().is_neutral());
        assert!(Look::default().at_level(0.0).is_neutral());
    }

    #[test]
    fn pure_red_has_no_green_or_blue() {
        let tint = Look {
            kelvin: PURE_RED_KELVIN,
            brightness: 100,
        }
        .tint();
        assert_eq!(tint.red, 1.0);
        assert_eq!(tint.green, 0.0);
        assert_eq!(tint.blue, 0.0);
    }

    #[test]
    fn follows_the_published_black_body_curve() {
        // Reference points from redshift's table (Ingo Thies, 2013), which
        // is computed the same way; within two hundredths everywhere.
        for (kelvin, green, blue) in [
            (1000, 0.182, 0.0),
            (1500, 0.423, 0.0),
            (2000, 0.544, 0.087),
            (3000, 0.720, 0.429),
            (4000, 0.829, 0.648),
            (5000, 0.902, 0.815),
        ] {
            let [red, g, b] = color_at(kelvin);
            assert_eq!(red, 1.0, "{kelvin} K red");
            assert!(close(g, green, 0.03), "{kelvin} K green {g} vs {green}");
            assert!(close(b, blue, 0.03), "{kelvin} K blue {b} vs {blue}");
        }
    }

    #[test]
    fn redness_scale_round_trips() {
        for kelvin in [850, 900, 1000, 1500, 1900, 3400, 6500] {
            let back = kelvin_at_redness(redness_of(kelvin));
            assert!(back.abs_diff(kelvin) <= 10, "{kelvin} came back as {back}");
        }
        assert_eq!(redness_of(DAY_KELVIN), 0.0);
        assert_eq!(redness_of(PURE_RED_KELVIN), 1.0);
    }

    #[test]
    fn fading_in_gets_steadily_redder_and_dimmer() {
        let look = Look {
            kelvin: 1000,
            brightness: 50,
        };
        let mut previous = look.at_level(0.0);
        for step in 1..=20 {
            let tint = look.at_level(step as f32 / 20.0);
            assert!(tint.green <= previous.green + 1e-6);
            assert!(tint.blue <= previous.blue + 1e-6);
            assert!(tint.red <= previous.red + 1e-6);
            previous = tint;
        }
        assert!(close(previous.red, 0.5, 1e-4));
    }

    #[test]
    fn the_light_of_a_tint_is_what_white_becomes() {
        use egui::Color32;
        assert_eq!(Tint::NEUTRAL.light(), Color32::WHITE);
        assert_eq!(scale_tint(1.0).light(), Color32::from_rgb(0xFF, 0, 0));
        let tint = Tint {
            red: 1.0,
            green: 0.5,
            blue: 0.2,
        };
        assert_eq!(tint.light(), Color32::from_rgb(0xFF, 0x80, 0x33));
        // Opaque, like white.
        assert_eq!(tint.light().a(), 255);
        // The scale's half way is the README plate's orange.
        assert_eq!(scale_tint(0.5).light(), Color32::from_rgb(0xFF, 0x80, 0));
    }

    #[test]
    fn the_scale_runs_from_white_through_orange_to_pure_red() {
        let white = egui::Color32::WHITE;
        // The README's plate, read off its pictures.
        assert!(scale_tint(0.0).is_neutral());
        assert_eq!(scale_tint(0.0).apply(white), white);
        assert_eq!(
            scale_tint(0.5).apply(white),
            egui::Color32::from_rgb(0xFF, 0x80, 0x00)
        );
        assert_eq!(
            scale_tint(1.0).apply(white),
            egui::Color32::from_rgb(0xFF, 0x00, 0x00)
        );
        // Red means red: the end of the scale has no green and no blue.
        let end = scale_tint(1.0);
        assert_eq!((end.red, end.green, end.blue), (1.0, 0.0, 0.0));
        // Every point keeps its red, and green and blue only fall.
        let mut previous = scale_tint(0.0);
        for step in 0..=200 {
            let tint = scale_tint(step as f32 / 200.0);
            assert_eq!(tint.apply(white).r(), 255, "red lost at step {step}");
            assert!(tint.green <= previous.green + 1e-6, "step {step}");
            assert!(tint.blue <= previous.blue + 1e-6, "step {step}");
            previous = tint;
        }
    }

    #[test]
    fn the_scale_is_a_fade_to_pure_red_at_full_brightness() {
        let pure_red = Look {
            kelvin: PURE_RED_KELVIN,
            brightness: MAX_BRIGHTNESS,
        };
        for redness in [0.0, 0.2, 0.5, 0.8, 1.0] {
            assert_eq!(scale_tint(redness), pure_red.at_level(redness));
        }
        // Past either end it stays on the end.
        assert_eq!(scale_tint(-1.0), scale_tint(0.0));
        assert_eq!(scale_tint(2.0), scale_tint(1.0));
    }

    #[test]
    fn preview_colors_follow_the_tint() {
        let teal = egui::Color32::from_rgb(0, 128, 128);
        let gray = egui::Color32::from_rgb(192, 192, 192);
        let red = Look {
            kelvin: PURE_RED_KELVIN,
            brightness: 100,
        }
        .tint();
        assert_eq!(red.apply(teal), egui::Color32::from_rgb(0, 0, 0));
        assert_eq!(red.apply(gray), egui::Color32::from_rgb(192, 0, 0));
    }

    #[test]
    fn preview_drawn_through_the_live_tint_shows_the_look() {
        let look = Look {
            kelvin: 2000,
            brightness: 80,
        }
        .tint();
        for live in [
            Tint::NEUTRAL,
            look,
            Look {
                kelvin: 3500,
                brightness: 100,
            }
            .tint(),
        ] {
            let seen = Tint {
                red: look.through(live).red * live.red,
                green: look.through(live).green * live.green,
                blue: look.through(live).blue * live.blue,
            };
            for (a, b) in [
                (seen.red, look.red),
                (seen.green, look.green),
                (seen.blue, look.blue),
            ] {
                assert!((a - b).abs() < 1e-4, "{live:?}: {seen:?} != {look:?}");
            }
        }
    }

    #[test]
    fn a_table_step_is_one_in_1023() {
        // The display's table holds 1,024 values a channel: 1,023 steps.
        assert_eq!(TABLE_STEP, 1.0 / 1023.0);
        assert!(close(1023.0 * TABLE_STEP, 1.0, 1e-6));
    }

    #[test]
    fn the_distance_between_tints_is_the_largest_channel_difference() {
        let a = Tint {
            red: 1.0,
            green: 0.5,
            blue: 0.25,
        };
        // The channels differ by 0.0, 0.2 and 0.1: the largest one counts,
        // whichever channel it is.
        let b = Tint {
            red: 1.0,
            green: 0.7,
            blue: 0.35,
        };
        assert!(close(a.distance(b), 0.2, 1e-6), "{}", a.distance(b));
        for (other, largest) in [
            (Tint { red: 0.9, ..a }, 0.1),
            (Tint { green: 0.3, ..a }, 0.2),
            (Tint { blue: 0.0, ..a }, 0.25),
            (
                Tint {
                    red: 0.9,
                    green: 0.45,
                    blue: 0.2,
                },
                0.1,
            ),
        ] {
            assert!(close(a.distance(other), largest, 1e-6), "{other:?}");
        }
        // The same either way round, and nothing for equal tints.
        assert_eq!(a.distance(b), b.distance(a));
        assert_eq!(a.distance(a), 0.0);
        assert_eq!(Tint::NEUTRAL.distance(Tint::NEUTRAL), 0.0);
        // A table step is the smallest distance a display can show.
        let step = Tint {
            green: 1.0 - TABLE_STEP,
            ..Tint::NEUTRAL
        };
        assert!(close(Tint::NEUTRAL.distance(step), TABLE_STEP, 1e-7));
    }
}
