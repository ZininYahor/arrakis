//! The Temperature slider's detents: where a drag catches the value, holds
//! it and lets it go, where the keys stop, and when a tick is due. Pure math
//! in positions from 0 to 1, with its distances in points of the track's
//! travel. No egui here: `ui::input` keeps the hold from one frame to the
//! next, and the app plays the tick.

use std::ops::RangeInclusive;

/// A drag within this many points of a detent catches it.
pub const CATCH: f32 = 3.0;
/// A caught detent holds the value until the drag is more than this many
/// points from it.
pub const RELEASE: f32 = 5.0;
/// The right end's zone holds the value until the drag is more than this
/// many points short of the zone.
pub const END_RELEASE: f32 = 2.0;
/// A tick sooner than this after the last one, in seconds, is dropped: at
/// most 20 a second, when scrubbing hard. One exactly this long after plays.
pub const TICK_GAP: f64 = 0.05;

/// A slider's detents, in positions from 0 to 1. Its stops are these and the
/// two ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detents<'a> {
    /// The detents inside the track, from the left.
    pub inner: &'a [f32],
    /// Where the right end's zone begins: a drag anywhere past it holds the
    /// value at the end.
    pub end_zone: f32,
    /// The value next to the right end, short of its zone. The keys step
    /// between the two as neighbors: the value keeps to a grid, and all of
    /// the stretch between them reads as one or the other, so a step shorter
    /// than that stretch would land back where it began.
    pub short_of_end: f32,
}

impl<'a> Detents<'a> {
    /// The detents on a track whose travel is `travel` points long, which
    /// turns their distances into positions.
    pub fn along(self, travel: f32) -> Snap<'a> {
        Snap {
            detents: self,
            point: 1.0 / travel,
        }
    }

    /// Where an arrow or a page key's step of `by` from `value` lands: on
    /// the first detent inside the track that it would pass, or else `by`
    /// away. A step from a detent moves on. Toward the right end, a step
    /// into its zone, or one from [`Self::short_of_end`], lands on the end;
    /// back from the zone, a step lands on `short_of_end` or past it.
    pub fn step(&self, value: f32, by: f32) -> f32 {
        let to = (value + by).clamp(0.0, 1.0);
        let passed = self.inner.iter().copied().filter(|&at| {
            if by > 0.0 {
                value < at && at <= to
            } else {
                to <= at && at < value
            }
        });
        if by > 0.0 {
            let to = passed.fold(to, f32::min);
            if to >= self.end_zone || value >= self.short_of_end {
                1.0
            } else {
                to
            }
        } else {
            let to = passed.fold(to, f32::max);
            if value >= self.end_zone {
                to.min(self.short_of_end)
            } else {
                to
            }
        }
    }

    /// Where ⌥ with an arrow goes from `value`: the next stop toward the
    /// right end when `rightward`, else toward the left, among the ends and
    /// the detents. None at the end it goes toward.
    pub fn jump(&self, value: f32, rightward: bool) -> Option<f32> {
        let stops = std::iter::once(0.0)
            .chain(self.inner.iter().copied())
            .chain(std::iter::once(1.0));
        if rightward {
            stops.filter(|&stop| stop > value).reduce(f32::min)
        } else {
            stops.filter(|&stop| stop < value).reduce(f32::max)
        }
    }
}

/// What a drag carries from one frame to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hold {
    /// The detent that holds the value, counted from the left; the right
    /// end's zone comes after the inner ones.
    pub detent: Option<usize>,
    /// Where the pointer was, as a position, before the snap.
    pub pointer: f32,
}

/// Where a press or a drag's move leaves the value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapped {
    /// The value's new position.
    pub to: f32,
    /// It arrived on a detent or passed one. Whether the tick plays is up to
    /// [`may_tick`].
    pub tick: bool,
    /// What the next frame of the drag starts from.
    pub hold: Hold,
}

/// The detents of a track of a known travel.
#[derive(Clone, Copy, Debug)]
pub struct Snap<'a> {
    detents: Detents<'a>,
    /// One point of the travel, as a position.
    point: f32,
}

/// One detent: where it puts the value, and the raw positions that catch it
/// and that keep it once caught.
struct Zone {
    at: f32,
    catch: RangeInclusive<f32>,
    keep: RangeInclusive<f32>,
}

impl Zone {
    /// Whether a move from `from` to `to` passes clean over the catch, from
    /// one side of it to the other.
    fn crossed(&self, from: f32, to: f32) -> bool {
        let (low, high) = (*self.catch.start(), *self.catch.end());
        (from < low && to > high) || (from > high && to < low)
    }
}

impl Snap<'_> {
    /// The detents from the left, the right end's zone last.
    fn zones(&self) -> impl Iterator<Item = Zone> + '_ {
        let (catch, release) = (CATCH * self.point, RELEASE * self.point);
        let end = self.detents.end_zone;
        let inner = self.detents.inner.iter().map(move |&at| Zone {
            at,
            catch: at - catch..=at + catch,
            keep: at - release..=at + release,
        });
        inner.chain(std::iter::once(Zone {
            at: 1.0,
            catch: end..=1.0,
            keep: end - END_RELEASE * self.point..=1.0,
        }))
    }

    /// A press at `raw`, the value being at `value`. A press that lands in a
    /// detent's catch holds the value there, and ticks unless the value was
    /// on it already. A press within the release of the detent the value is
    /// on keeps it, as a drag that holds it would.
    pub fn press(&self, value: f32, raw: f32) -> Snapped {
        for (index, zone) in self.zones().enumerate() {
            let on = zone.at == value;
            if zone.catch.contains(&raw) || (on && zone.keep.contains(&raw)) {
                return snapped(raw, Some(index), zone.at, !on);
            }
        }
        snapped(raw, None, raw, false)
    }

    /// A drag's move to `raw` from where `hold` left it. A held detent keeps
    /// the value while the drag stays within its release, and ticks no
    /// more. Otherwise a move that lands in a detent's catch holds the value
    /// there, and a move that passes clean over one follows the pointer; both
    /// tick, once however many detents the move meets. Pulling free of the
    /// held detent passes none.
    pub fn drag(&self, hold: Hold, raw: f32) -> Snapped {
        let mut tick = false;
        for (index, zone) in self.zones().enumerate() {
            if hold.detent == Some(index) {
                if zone.keep.contains(&raw) {
                    return snapped(raw, Some(index), zone.at, false);
                }
            } else if zone.catch.contains(&raw) {
                return snapped(raw, Some(index), zone.at, true);
            } else {
                tick |= zone.crossed(hold.pointer, raw);
            }
        }
        snapped(raw, None, raw, tick)
    }
}

/// The value at `to`, held by `detent`, with the pointer at `raw`.
fn snapped(raw: f32, detent: Option<usize>, to: f32, tick: bool) -> Snapped {
    Snapped {
        to,
        tick,
        hold: Hold {
            detent,
            pointer: raw,
        },
    }
}

/// Whether a tick due at `now` plays, the last one having played at `last`,
/// in seconds.
pub fn may_tick(last: Option<f64>, now: f64) -> bool {
    last.is_none_or(|last| now - last >= TICK_GAP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{kelvin_at_redness, pure_red_from, redness_of};
    use crate::ui::DESERT_KELVIN;

    /// The Temperature track's travel, in points.
    const TRAVEL: f32 = 372.0;

    /// The Temperature slider's detents, as the window has them.
    fn detents() -> Detents<'static> {
        *crate::ui::DETENTS
    }

    /// The kelvins of the detents inside the track, from the stop table.
    fn kelvins() -> Vec<u16> {
        crate::ui::detent_stops()
            .iter()
            .map(|stop| stop.kelvin)
            .collect()
    }

    fn at(kelvin: u16) -> f32 {
        redness_of(kelvin)
    }

    /// `count` points of the travel, as a position.
    fn points(count: f32) -> f32 {
        count / TRAVEL
    }

    /// A press and a drag on the Temperature track, frame by frame.
    struct Drag {
        hold: Hold,
        to: f32,
        ticks: usize,
    }

    impl Drag {
        /// A press at `raw`, the value being at `value`.
        fn press(value: f32, raw: f32) -> Self {
            let snapped = detents().along(TRAVEL).press(value, raw);
            Self {
                hold: snapped.hold,
                to: snapped.to,
                ticks: usize::from(snapped.tick),
            }
        }

        /// A press at `raw` while the value is on no detent.
        fn from(raw: f32) -> Self {
            Self::press(-1.0, raw)
        }

        /// The pointer moves to `raw` in one frame.
        fn to(&mut self, raw: f32) {
            let snapped = detents().along(TRAVEL).drag(self.hold, raw);
            self.hold = snapped.hold;
            self.to = snapped.to;
            self.ticks += usize::from(snapped.tick);
        }
    }

    #[test]
    fn the_owner_chose_three_detents_incandescent_desert_and_ember() {
        // The owner's choice. The other tests read the detents from the stop
        // table. Desert's own kelvin is pinned in `ui`'s tests.
        assert_eq!(kelvins(), [2700, DESERT_KELVIN, 1200]);
    }

    #[test]
    fn the_detents_catch_and_release_at_these_temperatures() {
        // They round-trip, so a held value reads its own kelvin.
        for kelvin in kelvins() {
            assert_eq!(kelvin_at_redness(redness_of(kelvin)), kelvin);
        }
        // The catch and release of each, which a detent added or moved needs
        // too.
        let spec = [
            (2700, (2750, 2650), (2790, 2620)),
            (DESERT_KELVIN, (1790, 1730), (1810, 1710)),
            (1200, (1220, 1180), (1230, 1170)),
        ];
        assert_eq!(spec.map(|(kelvin, ..)| kelvin).to_vec(), kelvins());
        for (kelvin, catch, release) in spec {
            let off = |by: f32| kelvin_at_redness(at(kelvin) + points(by));
            assert_eq!((off(-CATCH), off(CATCH)), catch, "{kelvin} K's catch");
            assert_eq!(
                (off(-RELEASE), off(RELEASE)),
                release,
                "{kelvin} K's release"
            );
        }
        // Pure red's zone is the last 10.4 points, where the scale already
        // reads 850 K.
        let zone = pure_red_from();
        let length = (1.0 - zone) * TRAVEL;
        assert!((length - 10.4).abs() < 0.06, "a zone of {length} points");
        assert_eq!(kelvin_at_redness(zone + 1e-4), 850);
        assert_eq!(kelvin_at_redness(zone - 1e-4), 860);
    }

    #[test]
    fn a_move_over_detents_ticks_once_and_does_not_snap() {
        for (from, to) in [(3000, 2500), (3000, 1000), (6500, 2500)] {
            let mut drag = Drag::from(at(from));
            drag.to(at(to));
            assert_eq!((drag.ticks, drag.to), (1, at(to)), "{from} K to {to} K");
            assert_eq!(drag.hold.detent, None);
        }
    }

    #[test]
    fn a_drag_catches_a_detent_holds_it_and_lets_go() {
        // Each from a value clear of every detent, on the side toward day.
        for (detent, from) in [(1200, 1300), (DESERT_KELVIN, 2000)] {
            let mut drag = Drag::from(at(from));
            for step in 1.. {
                let raw = at(from) + points(step as f32);
                // How far past the detent the pointer is, toward red.
                let past = (raw - at(detent)) * TRAVEL;
                if past > RELEASE + 3.0 {
                    break;
                }
                drag.to(raw);
                if (-CATCH..=RELEASE).contains(&past) {
                    assert_eq!(drag.to, at(detent), "{past} points past {detent} K");
                    assert_eq!(kelvin_at_redness(drag.to), detent);
                } else {
                    assert_eq!(drag.to, raw, "{past} points past {detent} K");
                }
            }
            assert_eq!(drag.ticks, 1, "from {from} K over {detent} K");
        }
    }

    #[test]
    fn a_held_detent_ticks_again_only_once_left_and_caught_again() {
        let mut drag = Drag::from(at(1300));
        drag.to(at(1200) - points(2.0));
        assert_eq!((drag.ticks, drag.to), (1, at(1200)));
        for off in [4.0, -4.0, 4.9, -4.9, 0.0, 2.0] {
            drag.to(at(1200) + points(off));
            assert_eq!((drag.ticks, drag.to), (1, at(1200)), "{off} points off");
        }
        // Out past the release, then back inside it but outside the catch.
        for off in [6.0, 4.0] {
            drag.to(at(1200) + points(off));
            assert_eq!((drag.ticks, drag.to), (1, at(1200) + points(off)));
        }
        drag.to(at(1200) + points(2.0));
        assert_eq!((drag.ticks, drag.to), (2, at(1200)));
        // Pulling free across the held detent passes nothing.
        drag.to(at(1200) - points(6.0));
        assert_eq!((drag.ticks, drag.to), (2, at(1200) - points(6.0)));
    }

    #[test]
    fn pure_red_catches_across_its_zone_and_lets_go_two_points_short() {
        let zone = pure_red_from();
        for raw in [zone + 1e-4, zone + points(5.0), 1.0] {
            let mut drag = Drag::from(at(1000));
            drag.to(raw);
            assert_eq!((drag.ticks, drag.to), (1, 1.0), "{raw}");
            assert_eq!(kelvin_at_redness(drag.to), 850);
        }
        let mut drag = Drag::from(at(1000));
        drag.to(zone + points(1.0));
        drag.to(zone - points(1.5));
        assert_eq!((drag.ticks, drag.to), (1, 1.0));
        drag.to(zone - points(2.5));
        assert_eq!((drag.ticks, drag.to), (1, zone - points(2.5)));
    }

    #[test]
    fn a_press_on_a_detent_snaps_and_ticks_unless_the_value_is_on_it() {
        for kelvin in [2700, DESERT_KELVIN] {
            for off in [-2.5, 0.0, 2.5] {
                let drag = Drag::press(at(1500), at(kelvin) + points(off));
                assert_eq!((drag.ticks, drag.to), (1, at(kelvin)), "{kelvin} K {off}");
            }
        }
        // On the knob held at 1200 K, or near enough that a drag would keep it.
        for off in [0.0, 4.0] {
            let drag = Drag::press(at(1200), at(1200) + points(off));
            assert_eq!((drag.ticks, drag.to), (0, at(1200)), "{off} points off");
        }
        // Away from every detent the value goes to the press.
        let drag = Drag::press(at(1200), at(2000));
        assert_eq!((drag.ticks, drag.to, drag.hold.detent), (0, at(2000), None));
    }

    #[test]
    fn daylight_is_no_detent() {
        // A drag to the left end from anywhere left of Incandescent passes
        // nothing, so it neither ticks nor snaps.
        for from in [at(6000), at(4000)] {
            let mut drag = Drag::from(from);
            drag.to(0.0);
            assert_eq!((drag.ticks, drag.to, drag.hold.detent), (0, 0.0, None));
        }
        // Nor does it hold the value as it nears the end, step by step.
        let mut drag = Drag::from(points(20.0));
        for near in [5.0, 2.0, 0.5, 0.0] {
            drag.to(points(near));
            assert_eq!((drag.ticks, drag.to), (0, points(near)), "{near} points in");
        }
        // A press there ticks no more than a press anywhere else clear of
        // the detents, whether the value is at the end or not.
        for value in [0.0, at(1500)] {
            let drag = Drag::press(value, 0.0);
            assert_eq!((drag.ticks, drag.to, drag.hold.detent), (0, 0.0, None));
        }
    }

    #[test]
    fn a_press_in_pure_reds_zone_snaps_to_the_end_unless_the_value_is_there() {
        let zone = pure_red_from();
        for raw in [zone + 1e-4, zone + points(5.0), 1.0] {
            let drag = Drag::press(at(1000), raw);
            assert_eq!((drag.ticks, drag.to), (1, 1.0), "{raw}");
            assert_eq!(kelvin_at_redness(drag.to), 850);
            let drag = Drag::press(1.0, raw);
            assert_eq!(
                (drag.ticks, drag.to),
                (0, 1.0),
                "{raw} with the value there"
            );
        }
        // Short of the zone a press is just a press.
        let short = zone - points(1.0);
        let drag = Drag::press(at(1000), short);
        assert_eq!((drag.ticks, drag.to, drag.hold.detent), (0, short, None));
        // With the value at the end, the press keeps it up to the same two
        // points short that a drag would.
        let drag = Drag::press(1.0, zone - points(1.5));
        assert_eq!((drag.ticks, drag.to), (0, 1.0));
        let drag = Drag::press(1.0, zone - points(2.5));
        assert_eq!((drag.ticks, drag.to), (0, zone - points(2.5)));
    }

    #[test]
    fn a_press_between_the_catch_and_the_release_only_keeps_what_the_value_is_on() {
        for kelvin in kelvins() {
            for off in [-4.9, -3.5, 3.5, 4.9] {
                let raw = at(kelvin) + points(off);
                // On the detent, the press keeps it, silently.
                let mut drag = Drag::press(at(kelvin), raw);
                assert_eq!((drag.ticks, drag.to), (0, at(kelvin)), "{kelvin} K {off}");
                // And the drag that follows holds it as one that began there.
                drag.to(at(kelvin) + points(off.signum() * 4.5));
                assert_eq!((drag.ticks, drag.to), (0, at(kelvin)), "{kelvin} K {off}");
                // Elsewhere, the press is outside the catch: it goes where it
                // landed, with no tick.
                let drag = Drag::press(at(1500), raw);
                assert_eq!(
                    (drag.ticks, drag.to),
                    (0, raw),
                    "{kelvin} K {off} elsewhere"
                );
            }
            // Beyond the release the value is let go, even from the detent.
            for off in [-5.1, 5.1] {
                let raw = at(kelvin) + points(off);
                let drag = Drag::press(at(kelvin), raw);
                assert_eq!((drag.ticks, drag.to), (0, raw), "{kelvin} K {off}");
                assert_eq!(drag.hold.detent, None);
            }
        }
    }

    #[test]
    fn pulling_free_of_a_detent_into_another_ticks_once() {
        // Ember into Desert, which is next to it, and Ember into
        // Incandescent and back, over Desert.
        for (held, into) in [(1200, DESERT_KELVIN), (1200, 2700), (2700, 1200)] {
            let mut drag = Drag::from(at(held) + points(10.0));
            drag.to(at(held));
            assert_eq!((drag.ticks, drag.to), (1, at(held)), "caught {held} K");
            // One move out of the held detent and into the other's catch,
            // over whatever lies between: one tick, for the landing.
            drag.to(at(into) + points(1.0));
            assert_eq!(
                (drag.ticks, drag.to),
                (2, at(into)),
                "{held} K into {into} K"
            );
        }
        // Into pure red's zone as well.
        let mut drag = Drag::from(at(1300));
        drag.to(at(1200));
        drag.to(pure_red_from() + points(1.0));
        assert_eq!((drag.ticks, drag.to), (2, 1.0));
    }

    #[test]
    fn ticks_come_at_most_twenty_a_second() {
        assert!(may_tick(None, 0.0));
        assert!(!may_tick(Some(1.0), 1.03));
        assert!(may_tick(Some(1.0), 1.06));
    }

    #[test]
    fn a_tick_fifty_milliseconds_on_plays_and_one_dated_before_the_last_does_not() {
        // A millisecond either side of the limit.
        assert!(may_tick(Some(1.0), 1.051));
        assert!(!may_tick(Some(1.0), 1.049));
        // A gap of exactly the limit plays: `TICK_GAP - 0.0` is `TICK_GAP` to
        // the bit, whatever f64 makes of 0.05.
        assert!(may_tick(Some(0.0), TICK_GAP));
        // A time before the last is no time to play.
        assert!(!may_tick(Some(1.0), 0.5));
    }

    #[test]
    fn a_step_stops_on_a_detent_it_would_pass() {
        let detents = detents();
        let step = 0.02;
        // → from 1250 K lands on 1200 K, and the next → leaves it.
        let on = detents.step(at(1250), step);
        assert_eq!(kelvin_at_redness(on), 1200);
        assert_eq!(kelvin_at_redness(detents.step(at(1200), step)), 1160);
        // → from 1830 K would pass Desert, so it stops there, and the next →
        // leaves it.
        assert_eq!(detents.step(at(1830), step), at(DESERT_KELVIN));
        assert_eq!(
            kelvin_at_redness(detents.step(at(DESERT_KELVIN), step)),
            1690
        );
        // ← too, and the pages of four steps.
        assert_eq!(detents.step(at(1700), -step), at(DESERT_KELVIN));
        assert_eq!(detents.step(at(1200) + 0.01, -step), at(1200));
        assert_eq!(detents.step(at(1200) - 0.05, 4.0 * step), at(1200));
        assert_eq!(detents.step(at(2700) + 0.05, -4.0 * step), at(2700));
        // Clear of the detents a step is a step, within the track.
        assert_eq!(detents.step(at(2000), step), at(2000) + step);
        assert_eq!(detents.step(0.99, step), 1.0);
        assert_eq!(detents.step(0.01, -step), 0.0);
    }

    #[test]
    fn the_keys_step_between_pure_red_and_the_value_next_to_it() {
        let detents = detents();
        let step = 0.02;
        // → from 870 K lands on 860 K, the last value short of the zone.
        assert_eq!(kelvin_at_redness(detents.step(at(870), step)), 860);
        // From 860 K a plain step would land short of the zone, which reads
        // 860 K again; it goes to the end.
        assert!(at(860) + step < pure_red_from());
        assert_eq!(kelvin_at_redness(at(860) + step), 860);
        assert_eq!(detents.step(at(860), step), 1.0);
        // A step or a page that lands in the zone goes to the end too.
        assert_eq!(detents.step(pure_red_from() - points(1.0), step), 1.0);
        assert_eq!(detents.step(at(880), 4.0 * step), 1.0);
        // Back from the end a plain step would still be in the zone. From
        // anywhere in it a step lands on 860 K; a page goes past it.
        assert!(1.0 - step >= pure_red_from());
        for value in [1.0, pure_red_from() + points(1.0)] {
            assert_eq!(detents.step(value, -step), at(860), "from {value}");
        }
        assert_eq!(kelvin_at_redness(detents.step(1.0, -4.0 * step)), 870);
    }

    #[test]
    fn option_arrows_jump_between_the_stops() {
        let detents = detents();
        let walk = |mut value: f32, rightward: bool| {
            let mut kelvins = vec![];
            while let Some(next) = detents.jump(value, rightward) {
                kelvins.push(kelvin_at_redness(next));
                value = next;
            }
            (kelvins, value)
        };
        let desert = DESERT_KELVIN;
        assert_eq!(walk(at(4000), true), (vec![2700, desert, 1200, 850], 1.0));
        assert_eq!(walk(1.0, false), (vec![1200, desert, 2700, 6500], 0.0));
    }
}
