//! The README's banner, drawn by the cover's own code: the cover's picture,
//! as tall as the banner and flush with its right edge, and the cover's sky
//! and dunes going on to its left edge. Demo builds only: `--demo-scene
//! banner` (`crate::demo`) shows it, through the window's tint, under the
//! app's name.
//!
//! The cover's square is its six layers (`Layers::images`) blended on the CPU
//! as the GPU blends their textures in [`super::draw`], by [`super::blended`],
//! the one copy of that blend, which the cover's tests use as well. The
//! stretch left of it is sampled through the cover's own [`layer`], a whole
//! cover's width at a time, each square evaluating the cover's shapes a cover
//! further left: the pixel grid and the antialiasing run on across the seam.
//! There the cover's own dunes have run out flat, so the stretch adds the
//! dunes of [`FAR_BEYOND`] and [`NEAR_BEYOND`], which have run out themselves
//! by the cover's left edge.

use egui::{Color32, ColorImage, Pos2, vec2};

use super::shapes::{DUNES, FAR_BEYOND, GRID, NEAR_BEYOND};
use super::{Layers, SUN_CENTER, blended, center, in_front, layer, over, sky_with_sun};
use crate::theme::Cover;

/// The banner, `width` by `height` pixels, `width` no less than `height`: the
/// cover `height` pixels square at its right, for `scene` with the maw out by
/// `rise` thousandths, its layers blended as the GPU blends them; and to its
/// left the cover's sky and dunes going on, sampled as the cover samples
/// them. Opaque, and seen through no tint: the GPU tints it as it tints the
/// cover.
pub(crate) fn picture(width: usize, height: usize, scene: &Cover, rise: u16) -> ColorImage {
    let cover = blended(
        &Layers::new(height, *scene).images(),
        rise,
        Some(rise),
        Some(rise),
    );
    let left = width.saturating_sub(height);
    let stretch = stretch(left, height, scene);
    let pixels = (0..width * height)
        .map(|i| {
            let (x, y) = (i % width, i / width);
            match x.checked_sub(left) {
                Some(x) => cover.pixels[y * height + x],
                None => stretch[y * left + x],
            }
        })
        .collect();
    ColorImage::new([width, height], pixels)
}

/// The sky and the dunes `width` pixels wide left of a cover `height` pixels
/// square, on the cover's grid: sampled a cover's width at a time through
/// [`layer`], each square a whole cover further left than the last, and the
/// last cut at the stretch's left edge.
fn stretch(width: usize, height: usize, scene: &Cover) -> Vec<Color32> {
    let scale = GRID / height as f32;
    let mut pixels = vec![Color32::TRANSPARENT; width * height];
    for k in 1..=width.div_ceil(height) {
        let shift = vec2(k as f32 * GRID, 0.0);
        let back = layer(height, |q| far_sand(q - shift, scene));
        let front = layer(height, |q| near_sand(q - shift, scene));
        // The stretch's column of the square's first, which may lie left of
        // the stretch.
        let first = width as isize - (k * height) as isize;
        for y in 0..height {
            for x in 0..height {
                let Ok(column) = usize::try_from(first + x as isize) else {
                    continue;
                };
                let i = y * height + x;
                let air = sky_with_sun(scene, center(i, height) - shift, SUN_CENTER, scale);
                pixels[y * width + column] = over(front[i], over(back[i], air));
            }
        }
    }
    pixels
}

/// The far sand at `q`, a point of the grid: the cover's far dune with its
/// slip face, as the cover's `far_dune` paints it, and the far dunes beyond.
fn far_sand(q: Pos2, scene: &Cover) -> Option<Color32> {
    let far = &DUNES[0];
    if far.covers(q, 0.0) {
        Some(if far.in_lee(q) { scene.lee } else { scene.far })
    } else {
        (FAR_BEYOND.iter().any(|dune| dune.covers(q, 0.0))).then_some(scene.far)
    }
}

/// The near sand at `q`: the cover's front (the near dune, the mound and the
/// ground) and the near dune beyond.
fn near_sand(q: Pos2, scene: &Cover) -> Option<Color32> {
    (in_front(q, 0.0) || NEAR_BEYOND.iter().any(|dune| dune.covers(q, 0.0))).then_some(scene.near)
}
