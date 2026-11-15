//! The thumper's clock: when a hold on the Temperature slider's Desert
//! stop beats (`ui::input` says what the thumper is). Pure, in seconds: no
//! egui and no AppKit here. `ui::input` keeps the hold's start and its count
//! from one frame to the next, and the app plays the thump.
//!
//! The beats come `HOLD_DELAY` after the hold began, then `PAIR_GAP` later,
//! and again each `PAIR_PERIOD` after those: 1.5, 1.68, 2.5, 2.68, 3.5 s and
//! so on.

/// How long a hold waits for its first beat, in seconds.
pub const HOLD_DELAY: f64 = 1.5;
/// How far the second thump of a pair comes after the first, in seconds.
pub const PAIR_GAP: f64 = 0.18;
/// How far a pair starts after the one before, in seconds.
pub const PAIR_PERIOD: f64 = 1.0;

/// A beat this close ahead of the frame counts as come. Frame times and beat
/// times are sums of decimals that a float rounds; a microsecond is far
/// below what a hand can feel.
const SLACK: f64 = 1e-6;

/// What the clock says for one frame of a hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beat {
    /// A beat has come that was not yet counted: one thump plays.
    pub due: bool,
    /// The beats of this hold that have come by now. Every one counts as
    /// played, so a late frame plays one thump, never a burst.
    pub played: u32,
    /// When the next beat comes, on the clock of the hold's start.
    pub next: f64,
}

/// Beat `n` of a hold, from 0, in seconds after the hold began.
fn beat_at(n: u32) -> f64 {
    let gap = if n % 2 == 1 { PAIR_GAP } else { 0.0 };
    HOLD_DELAY + f64::from(n / 2) * PAIR_PERIOD + gap
}

/// A frame at `now` of a hold that began at `began`, with `played` of its
/// beats counted so far, in seconds on one clock.
pub fn beat(began: f64, now: f64, played: u32) -> Beat {
    let held = now - began;
    let mut come = played;
    while beat_at(come) <= held + SLACK {
        come += 1;
    }
    Beat {
        due: come > played,
        played: come,
        next: began + beat_at(come),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames at `times`, in seconds after a hold that began at 0, each
    /// carrying the count of the one before: the thumps they play, and the
    /// next beat after each.
    fn frames(times: &[f64]) -> Vec<(bool, f64)> {
        let mut played = 0;
        times
            .iter()
            .map(|&now| {
                let beat = beat(0.0, now, played);
                played = beat.played;
                (beat.due, beat.next)
            })
            .collect()
    }

    #[test]
    fn the_first_beat_waits_a_second_and_a_half() {
        assert_eq!(
            beat(0.0, 1.49, 0),
            Beat {
                due: false,
                played: 0,
                next: 1.5,
            }
        );
    }

    #[test]
    fn the_thumps_come_in_pairs_each_second() {
        assert_eq!(
            frames(&[1.5, 1.68, 2.5, 2.68, 3.5]),
            [
                (true, 1.68),
                (true, 2.5),
                (true, 2.68),
                (true, 3.5),
                (true, 3.68),
            ]
        );
    }

    #[test]
    fn a_beat_already_counted_is_not_due_again() {
        let after = frames(&[1.5, 1.51, 1.6, 1.67, 1.679]);
        assert_eq!(after[0], (true, 1.68));
        for (due, next) in &after[1..] {
            assert!(!due, "a beat played twice in {after:?}");
            assert_eq!(*next, 1.68);
        }
    }

    #[test]
    fn a_late_frame_plays_one_thump_and_counts_every_beat_it_missed() {
        // 1.5, 1.68, 2.5 and 2.68 s have all passed by 3.0 s.
        assert_eq!(
            beat(0.0, 3.0, 0),
            Beat {
                due: true,
                played: 4,
                next: 3.5,
            }
        );
        // The next frame before 3.5 s plays nothing.
        assert!(!beat(0.0, 3.2, 4).due);
    }

    #[test]
    fn a_new_hold_waits_the_second_and_a_half_again() {
        // A hold began at 10 s and beat to 13 s; the next began at 20 s.
        assert!(beat(10.0, 13.0, 0).due);
        assert_eq!(
            beat(20.0, 21.49, 0),
            Beat {
                due: false,
                played: 0,
                next: 21.5,
            }
        );
        assert!(beat(20.0, 21.5, 0).due);
    }

    /// The times of pair `index` (from 0), in seconds after a hold began, as
    /// the constants have them.
    fn pair(index: u32) -> (f64, f64) {
        let first = HOLD_DELAY + f64::from(index) * PAIR_PERIOD;
        (first, first + PAIR_GAP)
    }

    /// Two floats that are the same time, to a nanosecond.
    fn assert_same_time(got: f64, want: f64) {
        assert!((got - want).abs() < 1e-9, "{got} s, wanted {want} s");
    }

    #[test]
    fn the_beats_follow_the_constants() {
        let began = 7.25;
        let mut played = 0;
        for index in 0..5 {
            for at in [pair(index).0, pair(index).1] {
                // A hundredth of a second short of the beat, nothing is due
                // and the beat is next; on it, one is.
                let short = beat(began, began + at - 0.01, played);
                assert!(!short.due, "a beat due at {at} s, a hundredth early");
                assert_eq!(short.played, played);
                assert_same_time(short.next, began + at);
                let on = beat(began, began + at, played);
                assert!(on.due, "no beat due at {at} s");
                played = on.played;
            }
        }
        assert_eq!(played, 10);
        // And the next pair starts a pair period after the last.
        assert_same_time(beat(began, began + pair(4).1, 10).next, began + pair(5).0);
    }

    #[test]
    fn a_frame_before_the_hold_began_has_nothing_due() {
        let early = beat(10.0, 9.0, 0);
        assert!(!early.due);
        assert_eq!(early.played, 0);
        assert_same_time(early.next, 10.0 + HOLD_DELAY);
    }

    #[test]
    fn a_very_late_frame_plays_one_thump_and_counts_all_the_pairs_it_missed() {
        // Frames come once the finger is down and only when something asks
        // for one: a hold could be 99 pairs old at its next frame, between
        // that pair's second thump and the next pair's first.
        let pairs = 99;
        let (_, last_second) = pair(pairs - 1);
        let now = last_second + (PAIR_PERIOD - PAIR_GAP) / 2.0;
        let late = beat(0.0, now, 0);
        assert!(late.due, "no thump due after {now} s");
        assert_eq!(late.played, 2 * pairs);
        assert_same_time(late.next, pair(pairs).0);
        // The next frame at once, with that count, plays nothing more.
        let again = beat(0.0, now, late.played);
        assert!(!again.due);
        assert_eq!(again, Beat { due: false, ..late });
    }

    #[test]
    fn a_beat_counted_by_an_earlier_frame_at_the_same_time_is_not_due_again() {
        for at in [pair(0).0, pair(0).1, pair(3).0] {
            let first = beat(0.0, at, 0);
            let second = beat(0.0, at, first.played);
            assert!(!second.due, "the beat at {at} s played twice");
            assert_eq!(second.played, first.played);
            assert_eq!(second.next, first.next);
        }
    }

    #[test]
    fn a_nanosecond_short_of_a_beat_on_a_large_clock_counts_it_and_a_millisecond_does_not() {
        // Egui's time is a day's seconds and more; a sum of decimals there
        // is not exact to a nanosecond.
        let began = 86_400.0;
        for (played, at) in [(0, pair(0).0), (1, pair(0).1), (2, pair(1).0)] {
            assert!(
                beat(began, began + at - 1e-9, played).due,
                "a nanosecond short of {at} s"
            );
            assert!(
                !beat(began, began + at - 1e-3, played).due,
                "a millisecond short of {at} s"
            );
        }
    }
}
