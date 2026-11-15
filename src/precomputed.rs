//! What the app would compute at every start, computed once and kept as bytes
//! in `src/precomputed/`, which `include_bytes!` puts in the binary:
//!
//! | File | What | Drawn by |
//! |---|---|---|
//! | `tray-template-36.rgba` | the macOS template icon, 36 by 36 pixels, RGBA, straight alpha | `icon::draw_template` |
//! | `color-table.f32le` | the black-body table: a row of red, green and blue for each 10 K, as little-endian `f32` | `color::build_table` |
//!
//! The full-color icon (`icon::tray`) is not here: `fastframe-tray` never asks
//! for it on macOS, where it draws the template alone, so there is nothing to
//! save at the start.
//!
//! The code that draws them stays: it draws every other icon size, and the
//! golden tests below draw each file again and compare the two, the icon byte
//! for byte and the table within a tolerance (`tests::TABLE_TOLERANCE`), so
//! the bytes cannot drift from the code.
//!
//! **Regenerating.** A change to the mark in `src/icon.rs` or to the curve in
//! `src/color.rs` fails a golden test, whose message gives the commands. A
//! changed mark has one more, for the owner's pin (`ARRAKIS_BLESS_MARK`),
//! which the mark's own tests hold and this module never writes: it has a step
//! of its own, so that no other step can rewrite it by accident.
//!
//! **A file of the wrong length,** left by a change to the table's range or
//! step or by a cut-off rewrite, stops every build but the tests'. It would put
//! a broken icon in the menu bar, or a table that is not the curve on the
//! displays. The tests build all the same, so that the rewrite can run: there
//! the table decodes to the day's colors (`decode_table`), its golden test
//! reports the file as stale, and the color tests fail beside it.

use crate::color::TABLE_LEN;

/// The command that rewrites the files, as the build's refusal and the golden
/// tests' messages give it.
macro_rules! bless_command {
    () => {
        "ARRAKIS_BLESS=1 cargo test --features demo --lib precomputed"
    };
}

/// The menu-bar icon's side on macOS, in pixels: what `fastframe-tray` asks
/// the template icon for. Every other size is drawn when asked for.
pub(crate) const TRAY_SIZE: usize = 36;

/// The template icon: `TRAY_SIZE` pixels on a side, four bytes each.
pub(crate) const TRAY_TEMPLATE: &[u8] = include_bytes!("precomputed/tray-template-36.rgba");

/// The length of the table's file: `TABLE_LEN` rows of three `f32`.
const TABLE_BYTES: usize = TABLE_LEN * 3 * 4;

const COLOR_TABLE_BYTES: &[u8] = include_bytes!("precomputed/color-table.f32le");

// Every build but the tests' refuses a file of the wrong length, and names the
// command that rewrites it. The tests build, so that the command can run.
#[cfg(not(test))]
const _: () = {
    assert!(
        TRAY_TEMPLATE.len() == TRAY_SIZE * TRAY_SIZE * 4,
        concat!(
            "src/precomputed/tray-template-36.rgba is not TRAY_SIZE by TRAY_SIZE RGBA pixels. ",
            "Rewrite it with `",
            bless_command!(),
            "`, then run `cargo test --features demo`."
        )
    );
    assert!(
        COLOR_TABLE_BYTES.len() == TABLE_BYTES,
        concat!(
            "src/precomputed/color-table.f32le is not TABLE_LEN rows of three f32. ",
            "Rewrite it with `",
            bless_command!(),
            "`, then run `cargo test --features demo`."
        )
    );
};

/// The black-body table, decoded while the compiler builds the binary, so
/// reading it costs nothing at run time.
pub(crate) static COLOR_TABLE: [[f32; 3]; TABLE_LEN] = decode_table(COLOR_TABLE_BYTES);

/// Rows of three little-endian `f32` into the table. Bytes of another length
/// give the day's colors in every row, which leave a screen as it is: only a
/// test build gets that far (see the module's doc), and the golden test
/// reports the file as stale.
const fn decode_table(bytes: &[u8]) -> [[f32; 3]; TABLE_LEN] {
    let mut table = [[1.0; 3]; TABLE_LEN];
    if bytes.len() != TABLE_BYTES {
        return table;
    }
    let mut row = 0;
    while row < TABLE_LEN {
        let mut channel = 0;
        while channel < 3 {
            let at = (row * 3 + channel) * 4;
            table[row][channel] =
                f32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
            channel += 1;
        }
        row += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use super::*;
    use crate::{color, icon};

    /// The commands that rewrite the files and then check them, as the
    /// failure message gives them. The second runs every test, so it checks
    /// the mark against the approved one too.
    const REGENERATE: &str = bless_command!();
    const CHECK: &str = "cargo test --features demo";

    const TEMPLATE_FILE: &str = "tray-template-36.rgba";
    const TABLE_FILE: &str = "color-table.f32le";

    /// How far a channel of the embedded table may stray from what
    /// `color::build_table` computes now. `exp` and `powf` may round the last
    /// bit differently on another libm, CPU or toolchain, and Linux builds this
    /// crate too. One step of an `f32` below 1 is at most 6e-8, so this allows
    /// about 16 of them, far below the 1/1023 that a display's table resolves
    /// (`color::TABLE_STEP`). The two ends are exact all the same
    /// (`the_tables_ends_are_exact`).
    const TABLE_TOLERANCE: f32 = 1e-6;

    /// Whether `value`, the switch's setting, asks to rewrite the files: only
    /// the value `1` does, so a stray `ARRAKIS_BLESS` cannot overwrite them.
    fn blessing_from(value: Option<&OsStr>) -> bool {
        value.is_some_and(|value| value == "1")
    }

    /// Whether this run rewrites the files instead of checking them.
    fn blessing() -> bool {
        blessing_from(std::env::var_os("ARRAKIS_BLESS").as_deref())
    }

    /// The table as its file holds it.
    fn table_bytes(table: &[[f32; 3]]) -> Vec<u8> {
        table
            .iter()
            .flatten()
            .flat_map(|channel| channel.to_le_bytes())
            .collect()
    }

    /// The failure message for `file`, which is not what `drawn_by` gives now,
    /// in the way `detail` says.
    fn stale(file: &str, drawn_by: &str, detail: &str) -> String {
        format!(
            "src/precomputed/{file} is not what `{drawn_by}` gives now ({detail}). If the change \
             is meant, rewrite the files, then run every test, which builds them in and checks \
             the mark against the approved one:\n    {REGENERATE}\n    {CHECK}"
        )
    }

    /// Why `embedded`, the bytes of `file` as built into the binary, are not
    /// `fresh`, what the code draws now, byte for byte; None when they are
    /// the same.
    fn bytes_mismatch(file: &str, drawn_by: &str, embedded: &[u8], fresh: &[u8]) -> Option<String> {
        if embedded == fresh {
            return None;
        }
        let first = (embedded.iter().zip(fresh))
            .position(|(a, b)| a != b)
            .unwrap_or(embedded.len().min(fresh.len()));
        let detail = format!(
            "{} bytes embedded, {} drawn, first difference at byte {first}",
            embedded.len(),
            fresh.len(),
        );
        Some(stale(file, drawn_by, &detail))
    }

    /// Why `embedded`, the table's file, is not `fresh`, the table computed
    /// now: its length differs, or a channel is further than
    /// `TABLE_TOLERANCE` from it, as a NaN is. None when it is within.
    fn table_mismatch(embedded: &[u8], fresh: &[[f32; 3]]) -> Option<String> {
        let drawn_by = "color::build_table()";
        let wanted = fresh.len() * 3 * 4;
        if embedded.len() != wanted {
            let detail = format!("{} bytes embedded, {wanted} computed", embedded.len());
            return Some(stale(TABLE_FILE, drawn_by, &detail));
        }
        let decoded = decode_table(embedded);
        for (row, (old, new)) in decoded.iter().zip(fresh).enumerate() {
            for (channel, name) in ["red", "green", "blue"].into_iter().enumerate() {
                let (old, new) = (old[channel], new[channel]);
                // False for a NaN, so that one fails too.
                let within = (old - new).abs() <= TABLE_TOLERANCE;
                if !within {
                    let detail = format!(
                        "row {row}, {name}: {old} embedded, {new} computed, more than \
                         {TABLE_TOLERANCE} apart"
                    );
                    return Some(stale(TABLE_FILE, drawn_by, &detail));
                }
            }
        }
        None
    }

    /// Passes when `mismatch` holds no reason. Otherwise it fails with the
    /// reason, or with `ARRAKIS_BLESS=1` writes `fresh` to `file` and passes. A
    /// file that is not stale is never written, so a rewrite on another
    /// machine leaves a table within the tolerance as it is.
    fn golden(file: &str, mismatch: Option<String>, fresh: &[u8]) {
        let Some(why) = mismatch else {
            return;
        };
        if blessing() {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/precomputed")
                .join(file);
            std::fs::write(&path, fresh)
                .unwrap_or_else(|error| panic!("unable to write {}: {error}", path.display()));
        } else {
            panic!("{why}");
        }
    }

    #[test]
    fn a_stale_file_fails_with_the_command_that_rewrites_it() {
        assert_eq!(
            bytes_mismatch("a.rgba", "draw", &[1, 2, 3], &[1, 2, 3]),
            None
        );
        let why = bytes_mismatch("a.rgba", "draw", &[1, 2, 3], &[1, 2, 4]).expect("a difference");
        assert!(why.contains("src/precomputed/a.rgba"), "{why}");
        assert!(why.contains("first difference at byte 2"), "{why}");
        assert!(why.contains(REGENERATE), "{why}");
        assert!(why.contains(CHECK), "{why}");
        // The same start, a different length.
        let why =
            bytes_mismatch("a.rgba", "draw", &[1, 2, 3], &[1, 2, 3, 4]).expect("a difference");
        assert!(why.contains("3 bytes embedded, 4 drawn"), "{why}");
        assert!(why.contains("first difference at byte 3"), "{why}");
    }

    #[test]
    fn only_the_value_one_rewrites_the_files() {
        for value in ["", "0", "true", "yes", "11", " 1", "1 "] {
            assert!(!blessing_from(Some(OsStr::new(value))), "{value:?}");
        }
        assert!(!blessing_from(None), "unset");
        assert!(blessing_from(Some(OsStr::new("1"))));
    }

    #[test]
    fn the_embedded_template_icon_is_the_one_drawn_now() {
        let fresh = icon::draw_template(TRAY_SIZE);
        let mismatch = bytes_mismatch(
            TEMPLATE_FILE,
            "icon::draw_template(36)",
            TRAY_TEMPLATE,
            &fresh,
        );
        golden(TEMPLATE_FILE, mismatch, &fresh);
    }

    #[test]
    fn the_embedded_color_table_is_the_one_computed_now() {
        let fresh = color::build_table();
        let mismatch = table_mismatch(COLOR_TABLE_BYTES, &fresh);
        golden(TABLE_FILE, mismatch, &table_bytes(&fresh));
    }

    /// The row the next two tests move: one in the middle of the table.
    const MIDDLE: usize = TABLE_LEN / 2;

    /// A last bit rounded another way is not a stale table.
    #[test]
    fn a_table_within_the_tolerance_is_not_stale() {
        let fresh = color::build_table();
        assert_eq!(table_mismatch(&table_bytes(&fresh), &fresh), None);
        let nudges: [fn(f32) -> f32; 3] =
            [f32::next_up, f32::next_down, |c| c + 0.9 * TABLE_TOLERANCE];
        for nudge in nudges {
            let mut moved = fresh.clone();
            moved[MIDDLE][1] = nudge(moved[MIDDLE][1]);
            assert_ne!(moved, fresh);
            assert_eq!(table_mismatch(&table_bytes(&moved), &fresh), None);
        }
    }

    #[test]
    fn a_table_past_the_tolerance_fails_with_the_commands() {
        let fresh = color::build_table();
        for (wrong, shown) in [
            (fresh[MIDDLE][1] + 2.0 * TABLE_TOLERANCE, None),
            (f32::NAN, Some("NaN")),
        ] {
            let mut moved = fresh.clone();
            moved[MIDDLE][1] = wrong;
            let why = table_mismatch(&table_bytes(&moved), &fresh).expect("a stale table");
            assert!(why.contains("src/precomputed/color-table.f32le"), "{why}");
            assert!(why.contains(&format!("row {MIDDLE}, green")), "{why}");
            assert!(why.contains(REGENERATE), "{why}");
            assert!(why.contains(CHECK), "{why}");
            if let Some(shown) = shown {
                assert!(why.contains(shown), "{why}");
            }
        }
    }

    /// The case where the table's range or step changed, or a rewrite was cut
    /// off: the file is reported by its length, not by a first value.
    #[test]
    fn a_table_of_another_length_fails_with_the_commands() {
        let fresh = color::build_table();
        let bytes = table_bytes(&fresh);
        let longer = [bytes.as_slice(), &table_bytes(&[[1.0; 3]])].concat();
        let files: [&[u8]; 3] = [&bytes[..bytes.len() - 1], &[], &longer];
        for embedded in files {
            let why = table_mismatch(embedded, &fresh).expect("a stale table");
            let detail = format!(
                "{} bytes embedded, {} computed",
                embedded.len(),
                bytes.len()
            );
            assert!(why.contains(&detail), "{why}");
            assert!(why.contains(REGENERATE), "{why}");
            assert!(why.contains(CHECK), "{why}");
        }
    }

    /// Bytes of another length decode, so that the tests build and the
    /// rewrite can run, and to the day's colors, which leave a screen as it
    /// is.
    #[test]
    fn a_file_of_another_length_decodes_to_the_days_colors() {
        let bytes = table_bytes(&color::build_table());
        let day = [[1.0; 3]; TABLE_LEN];
        let longer = [bytes.as_slice(), &[0; 12]].concat();
        let files: [&[u8]; 4] = [&bytes[..bytes.len() - 1], &bytes[..12], &[], &longer];
        for embedded in files {
            assert_eq!(decode_table(embedded), day, "{} bytes", embedded.len());
        }
    }

    /// The decoder, on bytes made here, whatever the file holds.
    #[test]
    fn the_table_decodes_to_the_rows_it_was_encoded_from() {
        let rows = color::build_table();
        assert_eq!(rows.len(), TABLE_LEN);
        assert_eq!(
            decode_table(&table_bytes(&rows)).as_slice(),
            rows.as_slice()
        );
    }

    /// What the product needs exactly, in the table the app reads: pure red
    /// at the night end (green and blue at 0) and normal colors at the day
    /// end. The ends are exact on any libm, as the middle rows need not be
    /// (`TABLE_TOLERANCE`): red is the largest channel divided by itself,
    /// green and blue clip to 0 at 850 K, and 6500 K is the white divided by
    /// itself, which `powf` leaves at 1.
    #[test]
    fn the_tables_ends_are_exact() {
        assert_eq!(COLOR_TABLE[0], [1.0, 0.0, 0.0]);
        assert_eq!(COLOR_TABLE[TABLE_LEN - 1], [1.0, 1.0, 1.0]);
    }
}
