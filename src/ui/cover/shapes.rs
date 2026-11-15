//! The cover's shapes, on its own grid of 18 units: the maw's lip and
//! throat, and the two dunes, with the dunes left of the cover that only the
//! README's banner draws. `super` samples them into layers. The menu-bar
//! mark (`crate::icon`) has a drawing of its own and takes nothing from here.

use egui::{Pos2, pos2};

pub(crate) const GRID: f32 = 18.0;
/// Where the sand meets the bottom of the cover.
pub(crate) const GROUND: f32 = 17.0;
/// What keeps the maw and the sand apart: the sand's keep-out.
pub(crate) const GAP: f32 = 0.9;

/// The maw: its center, and the outer and inner edge of its lip.
pub(crate) const MAW: Pos2 = pos2(10.4, 7.4);
pub(crate) const LIP: f32 = 5.5;
pub(crate) const THROAT: f32 = 3.9;

pub(crate) struct Dune {
    crest: f32,
    height: f32,
    /// How far the slope runs out to the left of the crest, and to the right.
    left: f32,
    right: f32,
    /// Whether the line of the crest runs down the slip face.
    ridge: bool,
}

/// Back to front.
pub(crate) const DUNES: [Dune; 2] = [
    Dune {
        crest: 13.2,
        height: 5.4,
        left: 8.0,
        right: 5.6,
        ridge: true,
    },
    Dune {
        crest: 3.4,
        height: 3.6,
        left: 4.5,
        right: 7.8,
        ridge: false,
    },
];

/// The far dunes that only the README's banner draws (`super::banner`), on its
/// stretch left of the cover, where the cover's own two run out flat, back to
/// front. Each has run out to nothing by the cover's left edge, `x = 0`, so the
/// cover inside the banner is the cover. They have no slip face: the cover's
/// is dark as it faces away from the sun, and these stand on the sun's other
/// side.
#[cfg(feature = "demo")]
pub(crate) const FAR_BEYOND: [Dune; 2] = [
    Dune {
        crest: -24.0,
        height: 3.4,
        left: 8.0,
        right: 6.5,
        ridge: false,
    },
    Dune {
        crest: -8.5,
        height: 4.4,
        left: 9.5,
        // To the cover's edge, under the foot of its near dune: no notch of
        // sky down to the ground between them.
        right: 8.5,
        ridge: false,
    },
];

/// The near dune that only the banner draws, in front of [`FAR_BEYOND`] and
/// run out by `x = 0` as they are.
#[cfg(feature = "demo")]
pub(crate) const NEAR_BEYOND: [Dune; 1] = [Dune {
    crest: -15.5,
    height: 2.6,
    left: 7.0,
    right: 8.0,
    ridge: false,
}];

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

    /// Whether the dune covers `q`, or comes within `margin` of it from
    /// below. Where there is no dune there is no margin either.
    pub(crate) fn covers(&self, q: Pos2, margin: f32) -> bool {
        let rise = self.rise(q.x);
        (rise > 0.0 || margin == 0.0) && q.y >= GROUND - rise - margin
    }

    /// How far inside the dune `q` stands, measured across its slope from
    /// its surface: 0 on the surface and negative above it. It is 0 or more
    /// exactly where [`Self::covers`] holds with no margin.
    pub(crate) fn depth(&self, q: Pos2) -> f32 {
        const STEP: f32 = 0.01;
        let slope = (self.rise(q.x + STEP) - self.rise(q.x - STEP)) / (2.0 * STEP);
        (q.y - (GROUND - self.rise(q.x))) / slope.hypot(1.0)
    }

    /// Where the line of the crest runs at the height of `q`, if this dune
    /// has one there.
    fn ridge_at(&self, q: Pos2) -> Option<f32> {
        let top = GROUND - self.height;
        let down = ((q.y - top) / self.height).clamp(0.0, 1.0);
        (self.ridge && q.y > top + 0.15)
            .then(|| self.crest - 0.2 + self.right * 0.42 * down.powf(0.75))
    }

    /// Whether `q` lies on the slip face, past the line of the crest.
    pub(crate) fn in_lee(&self, q: Pos2) -> bool {
        self.ridge_at(q).is_some_and(|x| q.x > x)
    }
}

/// What of the maw lies at a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Lip,
    /// Inside the lip: the dark of the gullet.
    Throat,
}

/// The lip and the throat: the maw alone, without any body.
pub(crate) fn maw(q: Pos2) -> Option<Part> {
    let r = q.distance(MAW);
    if r > LIP {
        return None;
    }
    Some(if r >= THROAT { Part::Lip } else { Part::Throat })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dunes_depth_is_zero_on_its_surface_and_grows_inward() {
        for dune in &DUNES {
            for step in 0..=180 {
                let x = step as f32 * 0.1;
                let surface = pos2(x, GROUND - dune.rise(x));
                assert!(dune.depth(surface).abs() < 1e-5, "x = {x}");
                // Below the surface it is positive, and more the lower, by
                // no more than the way straight down; above it, negative.
                let mut before = 0.0;
                for down in [0.05, 0.1, 0.3, 0.6, 1.0] {
                    let (inside, above) = (
                        surface + egui::vec2(0.0, down),
                        surface - egui::vec2(0.0, down),
                    );
                    let depth = dune.depth(inside);
                    assert!(
                        depth > before && depth <= down + 1e-5,
                        "x = {x}, {down} down: {depth}"
                    );
                    assert!(dune.depth(above) < 0.0, "x = {x}, {down} up");
                    assert_eq!(dune.covers(inside, 0.0), depth >= 0.0);
                    assert_eq!(dune.covers(above, 0.0), dune.depth(above) >= 0.0);
                    before = depth;
                }
            }
        }
        // On a slope it is the way across, not the way down: a 45-degree
        // slope would take a unit down to 0.71, and the far dune's flank is
        // steep enough to show.
        let dune = &DUNES[0];
        let flank = pos2(10.0, GROUND - dune.rise(10.0) + 1.0);
        assert!(dune.depth(flank) < 0.97, "{}", dune.depth(flank));
    }

    /// The dunes that only the banner draws stand left of the cover and have
    /// run out to nothing by its left edge, so the cover inside the banner is
    /// the cover alone.
    #[cfg(feature = "demo")]
    #[test]
    fn the_dunes_the_banner_adds_have_run_out_by_the_covers_left_edge() {
        for dune in FAR_BEYOND.iter().chain(&NEAR_BEYOND) {
            // Across the whole grid, every tenth of a unit.
            for step in 0..=180 {
                let x = step as f32 * 0.1;
                assert_eq!(dune.rise(x), 0.0, "x = {x}");
            }
            // Control: each stands somewhere, and the cover's own dunes do
            // rise over the grid, so a zero is not what `rise` always says.
            assert!(dune.rise(dune.crest) > 0.0, "crest {}", dune.crest);
        }
        for dune in &DUNES {
            assert!((0..=180).any(|step| dune.rise(step as f32 * 0.1) > 0.0));
        }
    }

    #[test]
    fn the_maw_stops_at_the_lip_and_holds_both_parts() {
        let mut parts = Vec::new();
        // Every tenth of a point on the grid.
        for y in 0..=180 {
            for x in 0..=180 {
                let q = pos2(x as f32 / 10.0, y as f32 / 10.0);
                let part = maw(q);
                assert_eq!(part.is_none(), q.distance(MAW) > LIP, "{q:?}");
                if let Some(part) = part
                    && !parts.contains(&part)
                {
                    parts.push(part);
                }
            }
        }
        // The grid reaches the lip and the throat.
        for wanted in [Part::Lip, Part::Throat] {
            assert!(parts.contains(&wanted), "{wanted:?}");
        }
    }
}
