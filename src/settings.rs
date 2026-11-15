//! What the user chose, kept as readable JSON in the config folder.

use std::path::Path;

use crate::color::Look;
use crate::schedule::{ClockTime, Schedule};

pub const FADE_MAX_MINUTES: u16 = 120;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)] // missing fields take defaults, so old files keep loading
pub struct Settings {
    /// "Turn red automatically": follow the daily window.
    pub schedule_on: bool,
    pub start: ClockTime,
    pub end: ClockTime,
    pub fade_minutes: u16,
    /// How red, and how dim, the night gets.
    #[serde(flatten)]
    pub night: Look,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schedule_on: true,
            start: ClockTime::new(21, 0),
            end: ClockTime::new(7, 0),
            fade_minutes: 20,
            night: Look::default(),
        }
    }
}

/// Copies the settings file an earlier name of the app kept at `old` to
/// `path`, while `path` does not exist yet. The old file stays.
pub fn carry_over(old: &Path, path: &Path) {
    if path.exists() || !old.exists() {
        return;
    }
    match std::fs::copy(old, path) {
        Ok(_) => log::info!("settings carried over from {}", old.display()),
        Err(error) => log::warn!(
            "unable to carry over settings from {}: {error}",
            old.display()
        ),
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                log::warn!("unable to read settings at {}: {error}", path.display());
                return Self::default();
            }
        };
        let mut settings: Self = serde_json::from_str(&text).unwrap_or_else(|error| {
            log::warn!("settings at {} are unreadable: {error}", path.display());
            Self::default()
        });
        settings.clamp();
        settings
    }

    /// Pulls hand-edited values back into the ranges the controls offer.
    pub fn clamp(&mut self) {
        self.fade_minutes = self.fade_minutes.min(FADE_MAX_MINUTES);
        self.night = self.night.clamped();
    }

    pub fn schedule(&self) -> Schedule {
        Schedule {
            start: self.start,
            end: self.end,
            fade_minutes: self.fade_minutes,
        }
    }

    pub fn to_json(&self) -> Vec<u8> {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let settings = Settings {
            start: ClockTime::new(22, 30),
            night: Look {
                kelvin: 1200,
                ..Look::default()
            },
            ..Settings::default()
        };
        let text = String::from_utf8(settings.to_json()).unwrap();
        assert!(text.contains("\"start\": \"22:30\""), "{text}");
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back, settings);
    }

    #[test]
    fn old_or_partial_files_still_load() {
        let settings: Settings = serde_json::from_str(r#"{ "start": "20:15" }"#).unwrap();
        assert_eq!(settings.start, ClockTime::new(20, 15));
        assert_eq!(settings.end, Settings::default().end);
    }

    #[test]
    fn hand_edited_values_are_clamped() {
        let dir = std::env::temp_dir().join(format!("sunset-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{ "fade_minutes": 9000, "kelvin": 20, "brightness": 0 }"#,
        )
        .unwrap();
        let settings = Settings::load(&path);
        assert_eq!(settings.fade_minutes, FADE_MAX_MINUTES);
        assert_eq!(settings.night.kelvin, crate::color::PURE_RED_KELVIN);
        assert_eq!(settings.night.brightness, crate::color::MIN_BRIGHTNESS);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The window once came in several themes, and files still name one.
    #[test]
    fn a_file_naming_a_theme_still_loads() {
        let settings: Settings =
            serde_json::from_str(r#"{ "theme": "spotifast", "start": "20:15" }"#).unwrap();
        assert_eq!(settings.start, ClockTime::new(20, 15));
        let text = String::from_utf8(settings.to_json()).unwrap();
        assert!(!text.contains("theme"), "{text}");
    }

    #[test]
    fn old_settings_are_carried_over_once() {
        let dir = std::env::temp_dir().join(format!("arrakis-carry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (old, path) = (dir.join("old.json"), dir.join("settings.json"));
        carry_over(&old, &path);
        assert!(!path.exists(), "nothing to carry over");
        std::fs::write(&old, r#"{ "start": "20:15" }"#).unwrap();
        carry_over(&old, &path);
        assert_eq!(Settings::load(&path).start, ClockTime::new(20, 15));
        std::fs::write(&old, r#"{ "start": "19:00" }"#).unwrap();
        carry_over(&old, &path);
        assert_eq!(Settings::load(&path).start, ClockTime::new(20, 15));
        assert!(old.exists(), "the old file stays");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let settings = Settings::load(Path::new("/nonexistent/sunset/settings.json"));
        assert_eq!(settings, Settings::default());
    }
}
