//! The daily red window: when it starts, when it ends, and how far into
//! the red the screen is at any minute of the day.

use std::fmt;

pub const MINUTES_PER_DAY: u16 = 24 * 60;

/// A time of day to the minute. Saved as `"HH:MM"` so the settings file
/// stays readable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockTime(u16);

impl ClockTime {
    pub const fn new(hour: u16, minute: u16) -> Self {
        Self(((hour % 24) * 60 + minute % 60) % MINUTES_PER_DAY)
    }

    /// Wraps around midnight in both directions.
    pub fn from_minutes(minutes: i32) -> Self {
        Self(minutes.rem_euclid(i32::from(MINUTES_PER_DAY)) as u16)
    }

    pub const fn minutes(self) -> u16 {
        self.0
    }

    pub const fn hour(self) -> u16 {
        self.0 / 60
    }

    pub const fn minute(self) -> u16 {
        self.0 % 60
    }

    pub fn parse(text: &str) -> Option<Self> {
        let (hour, minute) = text.trim().split_once(':')?;
        let hour: u16 = hour.parse().ok()?;
        let minute: u16 = minute.parse().ok()?;
        (hour < 24 && minute < 60).then(|| Self::new(hour, minute))
    }
}

impl fmt::Display for ClockTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.hour(), self.minute())
    }
}

impl serde::Serialize for ClockTime {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for ClockTime {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("expected HH:MM, got {text:?}")))
    }
}

/// One window a day, which may cross midnight, with a fade at each end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Schedule {
    pub start: ClockTime,
    pub end: ClockTime,
    pub fade_minutes: u16,
}

/// Where the day stands against the schedule, for the status line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// The window is empty: start and end are the same minute.
    Never,
    /// Outside the window; red starts at `next_start`.
    Day { next_start: ClockTime },
    /// Fading in, or fully red; normal colors return from `until`.
    Night { until: ClockTime },
    /// Fading back out after the window ended.
    Morning,
}

impl Schedule {
    fn length(&self) -> f64 {
        f64::from(ClockTime::from_minutes(i32::from(self.end.0) - i32::from(self.start.0)).0)
    }

    /// The fade the window has room for: it must finish inside the window
    /// and before the next start, or the level would jump at the wrap.
    fn effective_fade(&self) -> f64 {
        let length = self.length();
        f64::from(self.fade_minutes)
            .min(length)
            .min(f64::from(MINUTES_PER_DAY) - length)
    }

    /// How far into the night the screen is, from 0 (normal colors) to 1
    /// (the full chosen red), at `minute_of_day` (fractional minutes since
    /// local midnight).
    pub fn level(&self, minute_of_day: f64) -> f32 {
        let length = self.length();
        if length == 0.0 {
            return 0.0;
        }
        let day = f64::from(MINUTES_PER_DAY);
        let since_start = (minute_of_day - f64::from(self.start.0)).rem_euclid(day);
        let fade = self.effective_fade();
        let ramp = |minutes: f64| {
            if fade <= 0.0 {
                1.0
            } else {
                (minutes / fade).clamp(0.0, 1.0)
            }
        };
        let level = if since_start < length {
            ramp(since_start)
        } else {
            let after_end = since_start - length;
            ramp(length) * (1.0 - ramp(after_end))
        };
        level as f32
    }

    pub fn phase(&self, minute_of_day: f64) -> Phase {
        let length = self.length();
        if length == 0.0 {
            return Phase::Never;
        }
        let day = f64::from(MINUTES_PER_DAY);
        let since_start = (minute_of_day - f64::from(self.start.0)).rem_euclid(day);
        if since_start < length {
            Phase::Night { until: self.end }
        } else if since_start - length < self.effective_fade() {
            Phase::Morning
        } else {
            Phase::Day {
                next_start: self.start,
            }
        }
    }

    /// Minutes until the level next starts to change, or 0 while it is
    /// changing. Lets the engine sleep through the steady hours.
    pub fn minutes_until_change(&self, minute_of_day: f64) -> f64 {
        let length = self.length();
        if length == 0.0 {
            return f64::from(MINUTES_PER_DAY);
        }
        let day = f64::from(MINUTES_PER_DAY);
        let since_start = (minute_of_day - f64::from(self.start.0)).rem_euclid(day);
        let fade = self.effective_fade();
        if since_start < fade || (since_start >= length && since_start < length + fade) {
            0.0
        } else if since_start < length {
            length - since_start
        } else {
            day - since_start
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u16, minute: u16) -> f64 {
        f64::from(ClockTime::new(hour, minute).minutes())
    }

    fn evening() -> Schedule {
        Schedule {
            start: ClockTime::new(21, 0),
            end: ClockTime::new(7, 0),
            fade_minutes: 20,
        }
    }

    #[test]
    fn clock_time_round_trips_through_text() {
        let time = ClockTime::parse("07:05").unwrap();
        assert_eq!(time, ClockTime::new(7, 5));
        assert_eq!(time.to_string(), "07:05");
        assert_eq!(ClockTime::parse("24:00"), None);
        assert_eq!(ClockTime::parse("seven"), None);
        assert_eq!(ClockTime::from_minutes(-1), ClockTime::new(23, 59));
    }

    #[test]
    fn window_crossing_midnight_fades_in_and_out() {
        let schedule = evening();
        assert_eq!(schedule.level(at(20, 59)), 0.0);
        assert_eq!(schedule.level(at(21, 0)), 0.0);
        assert!((schedule.level(at(21, 10)) - 0.5).abs() < 1e-6);
        assert_eq!(schedule.level(at(21, 20)), 1.0);
        assert_eq!(schedule.level(at(0, 0)), 1.0);
        assert_eq!(schedule.level(at(6, 59)), 1.0);
        assert!((schedule.level(at(7, 10)) - 0.5).abs() < 1e-6);
        assert_eq!(schedule.level(at(7, 20)), 0.0);
        assert_eq!(schedule.level(at(12, 0)), 0.0);
    }

    #[test]
    fn same_day_window_works_too() {
        let schedule = Schedule {
            start: ClockTime::new(13, 0),
            end: ClockTime::new(14, 0),
            fade_minutes: 0,
        };
        assert_eq!(schedule.level(at(12, 59)), 0.0);
        assert_eq!(schedule.level(at(13, 0)), 1.0);
        assert_eq!(schedule.level(at(13, 59)), 1.0);
        assert_eq!(schedule.level(at(14, 0)), 0.0);
    }

    #[test]
    fn empty_window_never_turns_red() {
        let schedule = Schedule {
            start: ClockTime::new(9, 0),
            end: ClockTime::new(9, 0),
            fade_minutes: 10,
        };
        assert_eq!(schedule.level(at(9, 0)), 0.0);
        assert_eq!(schedule.phase(at(9, 0)), Phase::Never);
    }

    #[test]
    fn fade_longer_than_the_window_never_jumps() {
        let schedule = Schedule {
            start: ClockTime::new(22, 0),
            end: ClockTime::new(22, 10),
            fade_minutes: 60,
        };
        let mut previous = schedule.level(0.0);
        for step in 1..=(24 * 60 * 4) {
            let level = schedule.level(f64::from(step) / 4.0);
            assert!((level - previous).abs() < 0.05, "jump at step {step}");
            previous = level;
        }
    }

    #[test]
    fn phase_names_the_next_boundary() {
        let schedule = evening();
        assert_eq!(
            schedule.phase(at(12, 0)),
            Phase::Day {
                next_start: ClockTime::new(21, 0)
            }
        );
        assert_eq!(
            schedule.phase(at(23, 0)),
            Phase::Night {
                until: ClockTime::new(7, 0)
            }
        );
        assert_eq!(schedule.phase(at(7, 5)), Phase::Morning);
    }

    #[test]
    fn engine_can_sleep_through_steady_hours() {
        let schedule = evening();
        assert_eq!(schedule.minutes_until_change(at(21, 5)), 0.0);
        assert_eq!(schedule.minutes_until_change(at(23, 0)), 8.0 * 60.0);
        assert_eq!(schedule.minutes_until_change(at(12, 0)), 9.0 * 60.0);
        assert_eq!(schedule.minutes_until_change(at(7, 5)), 0.0);
    }
}
