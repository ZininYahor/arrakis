//! The cover's teeth: small and many, in rings that recede into the throat,
//! as the film's sandworm has them.
//!
//! Ring `k`, for `k` from 0 at the throat's edge to `RINGS - 1`, stands at
//! `THROAT · STEP^k` and holds `TEETH` teeth, each pointing in. So a ring's
//! teeth are spaced in proportion to its radius, and their length, most of
//! the gap to the next ring in, shrinks with it. Each ring is turned `TURN`
//! of a tooth from the one outside it, so its teeth stand in the gaps
//! between those outside. The deeper the ring, the more its teeth are lost
//! in the throat's dark.
//!
//! In log-polar terms, the rings counted inward, `ln(THROAT / r) / ln(1 /
//! STEP)`, against the angle counted in teeth, the teeth sit on a staggered
//! lattice of unit cells: a point tests the one tooth of its own cell, not
//! all of them.

use std::f32::consts::TAU;

use egui::Vec2;

use super::shapes::THROAT;

/// The teeth in each ring.
const TEETH: u16 = 40;
/// The rings of teeth, so 240 teeth in all. Inside the last the throat is
/// clear.
const RINGS: u16 = 6;
/// Each ring's radius as a share of the one outside it: ring `k` stands at
/// `THROAT · STEP^k`.
const STEP: f32 = 0.84;
/// How far each ring is turned from the one outside it, in teeth: half a
/// tooth, so the rings stagger.
const TURN: f32 = 0.5;
/// A tooth's length, as a share of the gap from its ring to the next in.
const LENGTH: f32 = 0.9;
/// A tooth's width at its base, as a share of the space from one tooth to
/// the next in its ring. It narrows to a point at its tip.
const WIDTH: f32 = 0.55;
/// How far the teeth of the last ring are lost in the throat's dark: those
/// of ring `k` are `DEPTH · k / (RINGS - 1)` of the way from the tooth's
/// color to the throat's.
const DEPTH: f32 = 0.85;

/// The teeth: for a point `v` of the throat, measured from the maw's center
/// in the shapes' units, how far the tooth there is lost in the dark, from 0
/// in the outer ring to `DEPTH` in the last, or None where no tooth is.
pub(super) fn teeth() -> impl Fn(Vec2) -> Option<f32> {
    // The log-polar distance from one ring to the next.
    let step = -STEP.ln();
    move |v: Vec2| {
        let r = v.length();
        if r >= THROAT || r <= 0.0 {
            return None;
        }
        // The rings counted inward from the throat's edge: the whole part is
        // the ring, the rest how far along the gap to the next ring in.
        let ring = (THROAT / r).ln() / step;
        let k = ring.floor();
        let along = ring - k;
        if k >= f32::from(RINGS) || along >= LENGTH {
            return None;
        }
        // The angle counted in teeth, with this ring's turn: a tooth stands
        // at every whole number, and the point is `off` of a tooth from the
        // nearest.
        let across = v.y.atan2(v.x) / TAU * f32::from(TEETH) + TURN * k;
        let off = (across - across.round()).abs();
        (off < WIDTH / 2.0 * (1.0 - along / LENGTH)).then_some(DEPTH * k / (f32::from(RINGS) - 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples round a circle: the angle runs a whole turn in `SAMPLES`
    /// steps, fine enough to resolve the narrowest tooth.
    const SAMPLES: usize = 40 * 256;

    /// The radius of the point `ring` rings in from the throat's edge: a whole
    /// number is a ring's base, and the fraction how far along its gap.
    fn radius(ring: f32) -> f32 {
        THROAT * STEP.powf(ring)
    }

    /// Whether the point at `turn`, a share of the way round, at `radius`
    /// holds a tooth.
    fn tooth_at(turn: f32, radius: f32) -> Option<f32> {
        teeth()(Vec2::angled(turn * TAU) * radius)
    }

    /// The samples round the circle at `radius` that hold a tooth, one
    /// turn's worth, starting just past the axis.
    fn round(radius: f32) -> Vec<bool> {
        (0..SAMPLES)
            .map(|i| tooth_at((i as f32 + 0.5) / SAMPLES as f32, radius).is_some())
            .collect()
    }

    /// The teeth round a circle: for each run of samples that hold one, where
    /// its middle is, as a share of a turn, and its width in turns. A run that
    /// crosses the wrap at the start is one tooth, not two.
    fn runs(hits: &[bool]) -> Vec<(f32, f32)> {
        let n = hits.len();
        // Start the walk at a sample with no tooth in it, so no run is cut.
        let Some(start) = hits.iter().position(|hit| !hit) else {
            return vec![(0.0, 1.0)];
        };
        let mut runs = Vec::new();
        let mut from = None;
        for j in 0..=n {
            let hit = j < n && hits[(start + j) % n];
            match (hit, from) {
                (true, None) => from = Some(j),
                (false, Some(first)) => {
                    let (len, middle) =
                        (j - first, (start + first) as f32 + (j - first) as f32 / 2.0);
                    runs.push(((middle / n as f32).rem_euclid(1.0), len as f32 / n as f32));
                    from = None;
                }
                _ => {}
            }
        }
        runs
    }

    /// How far apart two points of a turn are, the short way round.
    fn apart(a: f32, b: f32) -> f32 {
        let d = (a - b).rem_euclid(1.0);
        d.min(1.0 - d)
    }

    #[test]
    fn there_is_no_tooth_outside_the_rings() {
        // At the throat's edge, on the axes, where the length is exact, and
        // past it all round.
        for v in [Vec2::X, Vec2::Y, -Vec2::X, -Vec2::Y] {
            assert_eq!(teeth()(v * THROAT), None, "{v:?} at the edge");
        }
        for radius in [THROAT * 1.001, THROAT * 1.1, THROAT * 1.5] {
            assert!(round(radius).iter().all(|hit| !hit), "{radius}");
        }
        assert_eq!(teeth()(Vec2::ZERO), None, "the center");
        // Inside the last ring the throat is clear, all the way in.
        let last = radius(f32::from(RINGS));
        for share in [0.99, 0.9, 0.5, 0.1, 0.01] {
            assert!(
                round(last * share).iter().all(|hit| !hit),
                "{share} of the way in from the last ring"
            );
        }
    }

    #[test]
    fn six_rings_of_forty_teeth() {
        // The owner's pick (2026-10-23): 40 teeth to a ring, in six
        // rings, counted round a circle near each ring's base.
        for k in 0..6 {
            let teeth = runs(&round(radius(k as f32 + 0.05)));
            assert_eq!(teeth.len(), 40, "ring {k}");
        }
        assert!(
            runs(&round(radius(6.05))).is_empty(),
            "a seventh ring holds a tooth"
        );
    }

    #[test]
    fn each_ring_stands_in_the_gaps_of_the_one_outside() {
        let middles = |k: usize| -> Vec<f32> {
            runs(&round(radius(k as f32 + 0.05)))
                .into_iter()
                .map(|(middle, _)| middle)
                .collect()
        };
        // A turn of half a tooth of 40 is a share of `1 / 80`, and a sample
        // is `1 / SAMPLES`: the middle of a run is known to that.
        let half = 1.0 / 80.0;
        let slack = 2.0 / SAMPLES as f32;
        for k in 0..5 {
            let (outer, inner) = (middles(k), middles(k + 1));
            for middle in &inner {
                let nearest = outer
                    .iter()
                    .map(|other| apart(*middle, *other))
                    .fold(f32::MAX, f32::min);
                assert!(
                    (nearest - half).abs() < slack,
                    "ring {}: a tooth is {nearest} of a turn from the nearest outside",
                    k + 1
                );
            }
        }
    }

    #[test]
    fn a_tooth_narrows_from_its_base_to_its_tip() {
        // The width of the first tooth of ring 2, at points along its gap.
        let width = |along: f32| {
            let teeth = runs(&round(radius(2.0 + along)));
            assert_eq!(teeth.len(), 40, "{along} of the way along");
            teeth[0].1
        };
        let (base, middle, tip) = (width(0.05), width(0.45), width(0.85));
        assert!(
            base > middle && middle > tip && tip > 0.0,
            "{base} {middle} {tip}"
        );
        assert!(
            tip < base / 2.0,
            "the tip ({tip}) is not narrower than the base ({base})"
        );
    }

    #[test]
    fn a_gap_lies_between_the_rings() {
        // A tooth spans most of the gap to the next ring in and not all: past
        // it, a whole circle holds none, outside each ring.
        for k in 0..6 {
            for along in [0.92, 0.95, 0.97] {
                let ring = k as f32 + along;
                assert!(
                    round(radius(ring)).iter().all(|hit| !hit),
                    "ring {k}, {along} of the way along"
                );
            }
        }
    }

    #[test]
    fn the_deeper_a_ring_the_more_its_teeth_are_lost_in_the_dark() {
        // How lost the first tooth of each ring is, a little in from its base.
        let lost: Vec<f32> = (0..6)
            .map(|k| {
                let ring = k as f32 + 0.25;
                let (middle, _) = runs(&round(radius(ring)))[0];
                tooth_at(middle, radius(ring)).expect("the tooth's middle holds it")
            })
            .collect();
        assert_eq!(lost[0], 0.0, "the outer ring is the tooth's own color");
        assert!(lost.windows(2).all(|pair| pair[0] < pair[1]), "{lost:?}");
        assert!(lost[5] < 1.0, "the last ring is lost entirely: {lost:?}");
    }
}
