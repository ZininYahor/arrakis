//! `scripts/painter-check.sh`, on macOS, run under `sh` with a stand-in for the demo build: a
//! script that writes the pictures as small files and opens no window, so nothing here tints a
//! screen, takes the focus or paints a display. `TMPDIR` is a new folder in every test, which is
//! also where the script puts its `home`, its `tmp` and the `winid` that `clang` builds: no
//! test reads or writes the real ones.
//!
//! A real window is not captured here, so the script's `screencapture` never runs. The captures the
//! script finds are files that the stand-in leaves in the window's folder, to read how the script
//! chooses the settled picture and compares the two: what it does with what it is given, not that
//! a window shows the same pixels.

#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/painter-check.sh");

const SCENES: [&str; 4] = ["night", "day", "focused", "desert"];
const PICTURES: [&str; 7] = [
    "night",
    "night-tinted",
    "day",
    "focused",
    "focused-tinted",
    "desert",
    "desert-tinted",
];

/// The stand-in for the demo build. It records how it was run and the `HOME` and `TMPDIR` it was
/// given, says which painter drew a shot, writes the picture `--demo-shot` names (and the tinted twin
/// that a shot of a night scene also leaves), and compares two files byte for byte for `--demo-diff`.
///
/// - `FAKE_FAIL=<picture>:<painter>` makes that shot fail.
/// - `FAKE_SAYS=<picture>:<painter>:<other>` makes that shot say another painter drew it.
/// - `FAKE_ALSO=<picture>:<painter>:<other>` makes that shot say a second painter too, after its own:
///   a Metal window that failed and opened again on glow.
/// - `FAKE_NO_TINT=1` leaves out the tinted twins.
/// - `FAKE_DIFFER=<picture>:<painter>` writes a different picture there.
/// - `FAKE_WINDOW=same|differ|unsettled` leaves captures of a window beside the shot named
///   `shot.png`, as `screencapture` would: a fade-in first, then the settled window twice. `differ`
///   settles on another picture for metal, and `unsettled` leaves two captures that are not alike.
const APP: &str = r#"
here=$(dirname "$0")
echo "$@" >>"$here/calls"
echo "$HOME $TMPDIR" >>"$here/env"
shot= scene= painter= diff= a= b=
while [ $# -gt 0 ]; do
    case "$1" in
    --demo-shot) shot=$2; shift ;;
    --demo-scene) scene=$2; shift ;;
    --demo-renderer) painter=$2; shift ;;
    --demo-diff) diff=1; a=$2; b=$3; shift 2 ;;
    esac
    shift
done
if [ -n "$diff" ]; then
    if cmp -s "$a" "$b"; then exit 0; fi
    echo "1 pixels differ by more than 0"
    exit 1
fi
name=$(basename "$shot" .png)
if [ "${FAKE_FAIL-}" = "$name:$painter" ]; then
    echo "the stand-in cannot draw $name" >&2
    exit 1
fi
says=$painter
case "${FAKE_SAYS-}" in "$name:$painter:"*) says=${FAKE_SAYS##*:} ;; esac
echo "painter: $says" >&2
case "${FAKE_ALSO-}" in "$name:$painter:"*) echo "painter: ${FAKE_ALSO##*:}" >&2 ;; esac
picture() {
    body="picture $1"
    if [ "${FAKE_DIFFER-}" = "$1:$painter" ]; then body="another picture $1"; fi
    printf %s "$body" >"$2"
}
picture "$name" "$shot"
if [ -z "${FAKE_NO_TINT-}" ]; then
    case "$scene" in night|focused|desert) picture "$name-tinted" "${shot%.png}-tinted.png" ;; esac
fi
if [ "$name" = shot ]; then
    dir=$(dirname "$shot")
    case "${FAKE_WINDOW-}" in
    same)
        printf 'fade-in %s' "$painter" >"$dir/0001.png"
        printf settled >"$dir/0002.png"
        printf settled >"$dir/0003.png" ;;
    differ)
        printf 'fade-in %s' "$painter" >"$dir/0001.png"
        printf 'settled %s' "$painter" >"$dir/0002.png"
        printf 'settled %s' "$painter" >"$dir/0003.png" ;;
    unsettled)
        printf 'one %s' "$painter" >"$dir/0001.png"
        printf 'two %s' "$painter" >"$dir/0002.png" ;;
    esac
fi
"#;

/// A new folder with a `bin` for the stand-in and an empty `tmp`, removed when it goes out of scope.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "arrakis-painter-check-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("bin")).expect("a bin folder");
        fs::create_dir_all(root.join("tmp")).expect("a tmp folder");
        Self { root }
    }

    /// The script's `TMPDIR`.
    fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// The folder that takes the pictures.
    fn pictures(&self) -> PathBuf {
        self.root.join("pictures")
    }

    fn app(&self) -> PathBuf {
        let path = self.root.join("bin/arrakis");
        executable(&path, APP);
        path
    }

    /// What the stand-in was run with, as it ran, one run to a line.
    fn calls(&self) -> Vec<String> {
        self.recorded("calls")
    }

    fn recorded(&self, file: &str) -> Vec<String> {
        fs::read_to_string(self.root.join("bin").join(file))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The script, with the stand-in for the build, the pictures in `pictures()` and `env` set.
    fn run(&self, env: &[(&str, &str)]) -> Run {
        self.run_in(Some(&self.pictures()), &self.app(), env)
    }

    fn run_in(&self, folder: Option<&Path>, app: &Path, env: &[(&str, &str)]) -> Run {
        let output = Command::new("sh")
            .arg(SCRIPT)
            .args(folder)
            .env("PAINTER_CHECK_BIN", app)
            .env("TMPDIR", self.tmp())
            .env_remove("FAKE_FAIL")
            .env_remove("FAKE_SAYS")
            .env_remove("FAKE_ALSO")
            .env_remove("FAKE_NO_TINT")
            .env_remove("FAKE_DIFFER")
            .env_remove("FAKE_WINDOW")
            .envs(env.iter().copied())
            .output()
            .expect("sh runs");
        Run(output)
    }

    /// The script with no build named, so it asks `cargo`: a stand-in, whose `metadata` gives
    /// `target`, and which notes where it ran. A `CARGO_TARGET_DIR` that says otherwise is set too.
    fn run_through_cargo(&self, target: Option<&Path>) -> Run {
        let fakes = self.root.join("fakes");
        fs::create_dir_all(&fakes).expect("a folder for the fakes");
        executable(&fakes.join("cargo"), CARGO);
        let path = std::env::var("PATH").expect("a PATH");
        let output = Command::new("sh")
            .arg(SCRIPT)
            .arg(self.pictures())
            .current_dir(&self.root)
            .env_remove("PAINTER_CHECK_BIN")
            .env("CARGO_TARGET_DIR", self.root.join("a guess"))
            .env("FAKE_TARGET", target.unwrap_or(Path::new("")))
            .env("PATH", format!("{}:{path}", fakes.display()))
            .env("TMPDIR", self.tmp())
            .output()
            .expect("sh runs");
        Run(output)
    }
}

/// A script at `path`, ready to run.
fn executable(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("a stand-in");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable");
}

/// The stand-in for cargo: `build` does nothing, and `metadata` prints what the real one does for
/// the target directory, or nothing when `FAKE_TARGET` is empty.
const CARGO: &str = r#"
echo "$PWD $1" >>"$(dirname "$0")/calls"
if [ "$1" = metadata ] && [ -n "$FAKE_TARGET" ]; then
    printf '{"packages":[],"target_directory":"%s","version":1}\n' "$FAKE_TARGET"
fi
"#;

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Run(Output);

impl Run {
    fn code(&self) -> Option<i32> {
        self.0.status.code()
    }

    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.0.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.0.stderr).into_owned()
    }

    /// Lines of stdout that begin with `prefix`.
    fn lines(&self, prefix: &str) -> Vec<String> {
        self.stdout()
            .lines()
            .filter(|line| line.starts_with(prefix))
            .map(str::to_owned)
            .collect()
    }
}

#[test]
fn the_script_is_valid_shell() {
    let status = Command::new("sh")
        .args(["-n", SCRIPT])
        .status()
        .expect("sh runs");
    assert!(status.success());
}

#[test]
fn pictures_that_are_the_same_with_no_window_to_capture_exit_two_as_unread() {
    let sandbox = Sandbox::new("unread");
    let run = sandbox.run(&[]);
    assert_eq!(run.code(), Some(2), "{}\n{}", run.stdout(), run.stderr());
    let stderr = run.stderr();
    assert!(
        stderr.contains("UNREAD: real window: no capture"),
        "{stderr}"
    );
    // The seven pictures were read and are the same.
    let same = run.lines("same: ");
    assert_eq!(same.len(), 7, "{same:?}");
    for picture in PICTURES {
        assert!(
            same.contains(&format!("same: {picture}")),
            "{picture}: {same:?}"
        );
    }
    assert!(run.lines("DIFFERENT").is_empty(), "{}", run.stdout());
}

#[test]
fn every_scene_is_shot_through_both_painters_and_every_picture_compared_exactly() {
    let sandbox = Sandbox::new("calls");
    sandbox.run(&[]);
    let out = sandbox.pictures();
    let calls = sandbox.calls();
    for painter in ["glow", "metal"] {
        for scene in SCENES {
            let want = format!(
                "--demo-shot {out}/{painter}/{scene}.png --demo-scene {scene} --demo-renderer {painter}",
                out = out.display()
            );
            assert!(calls.contains(&want), "missing {want:?} in {calls:#?}");
            assert!(out.join(painter).join(format!("{scene}.png")).is_file());
        }
        // The night window, shot once more for its captures.
        let window = format!(
            "--demo-shot {out}/{painter}/window/shot.png --demo-scene night --demo-renderer {painter}",
            out = out.display()
        );
        assert!(calls.contains(&window), "missing {window:?} in {calls:#?}");
    }
    for picture in PICTURES {
        let want = format!(
            "--demo-diff {out}/glow/{picture}.png {out}/metal/{picture}.png --tolerance 0",
            out = out.display()
        );
        assert!(calls.contains(&want), "missing {want:?} in {calls:#?}");
    }
    // 8 scene shots, 2 window shots and 7 comparisons, and nothing else.
    assert_eq!(calls.len(), 17, "{calls:#?}");
}

#[test]
fn the_shots_run_with_a_home_and_a_tmpdir_of_their_own() {
    let sandbox = Sandbox::new("home");
    sandbox.run(&[]);
    let work = sandbox.tmp().join("arrakis-painter-check");
    let want = format!(
        "{} {}",
        work.join("home").display(),
        work.join("tmp").display()
    );
    let seen = sandbox.recorded("env");
    assert_eq!(seen.len(), 17, "{seen:?}");
    // The settings and the single-instance slot of the real home and tmp are never the shots'.
    assert!(
        seen.iter().all(|line| *line == want),
        "{seen:#?}\nwanted {want}"
    );
}

#[test]
fn without_a_folder_the_pictures_go_under_tmpdir() {
    let sandbox = Sandbox::new("default-folder");
    let run = sandbox.run_in(None, &sandbox.app(), &[]);
    assert_eq!(run.code(), Some(2), "{}\n{}", run.stdout(), run.stderr());
    let work = sandbox.tmp().join("arrakis-painter-check");
    let stamped: Vec<_> = fs::read_dir(&work)
        .expect("the script's folder")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.join("glow/night.png").is_file())
        .collect();
    assert_eq!(stamped.len(), 1, "{stamped:?}");
    assert!(stamped[0].join("metal/desert-tinted.png").is_file());
    assert!(
        work.join("winid").is_file(),
        "winid is built where the script keeps it"
    );
}

#[test]
fn a_difference_exits_one_and_names_the_picture() {
    // A scene, a tinted twin, and the one scene that has no twin.
    for picture in ["night", "focused-tinted", "day"] {
        let sandbox = Sandbox::new(&format!("differ-{picture}"));
        let differ = format!("{picture}:metal");
        let run = sandbox.run(&[("FAKE_DIFFER", &differ)]);
        assert_eq!(
            run.code(),
            Some(1),
            "{picture}: {}\n{}",
            run.stdout(),
            run.stderr()
        );
        let different = run.lines("DIFFERENT");
        assert_eq!(different.len(), 1, "{picture}: {different:?}");
        assert!(
            different[0].starts_with(&format!("DIFFERENT: {picture}: ")),
            "{different:?}"
        );
        // What --demo-diff printed is passed on.
        assert!(different[0].contains("1 pixels differ"), "{different:?}");
        assert_eq!(run.lines("same: ").len(), 6, "{}", run.stdout());
        // No window to capture, but a difference was found: still 1, not 2.
        assert!(run.stderr().contains("UNREAD"), "{}", run.stderr());
    }
}

#[test]
fn a_shot_that_fails_exits_one_and_names_the_scene_and_the_painter() {
    for (scene, painter) in [("focused", "metal"), ("desert", "glow")] {
        let sandbox = Sandbox::new(&format!("fail-{scene}-{painter}"));
        let fail = format!("{scene}:{painter}");
        let run = sandbox.run(&[("FAKE_FAIL", &fail)]);
        assert_eq!(
            run.code(),
            Some(1),
            "{scene} on {painter}: {}",
            run.stderr()
        );
        let stderr = run.stderr();
        let log = sandbox
            .pictures()
            .join(painter)
            .join(format!("{scene}.log"));
        assert!(
            stderr.contains(&format!(
                "the {scene} shot on {painter} failed: see {}",
                log.display()
            )),
            "{stderr}"
        );
        // The log has what the stand-in said, and nothing was compared.
        let said = fs::read_to_string(&log).expect("the shot's log");
        assert!(said.contains("cannot draw"), "{said}");
        assert!(run.lines("same: ").is_empty(), "{}", run.stdout());
        assert!(run.lines("DIFFERENT").is_empty(), "{}", run.stdout());
    }
}

#[test]
fn a_night_window_shot_that_fails_exits_one() {
    let sandbox = Sandbox::new("fail-window");
    let run = sandbox.run(&[("FAKE_FAIL", "shot:metal")]);
    assert_eq!(run.code(), Some(1), "{}\n{}", run.stdout(), run.stderr());
    let stderr = run.stderr();
    assert!(
        stderr.contains("the night shot on metal failed"),
        "{stderr}"
    );
    assert!(stderr.contains("window/app.log"), "{stderr}");
}

#[test]
fn a_missing_build_is_refused_before_anything_is_shot() {
    let sandbox = Sandbox::new("no-build");
    let missing = sandbox.root.join("bin/no-such-arrakis");
    let run = sandbox.run_in(Some(&sandbox.pictures()), &missing, &[]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert!(
        run.stderr().contains(&format!("no {}", missing.display())),
        "{}",
        run.stderr()
    );
    assert!(sandbox.calls().is_empty());
}

#[test]
fn a_window_that_settles_the_same_in_both_painters_is_the_same_byte_for_byte() {
    let sandbox = Sandbox::new("window-same");
    let run = sandbox.run(&[("FAKE_WINDOW", "same")]);
    assert_eq!(run.code(), Some(0), "{}\n{}", run.stdout(), run.stderr());
    // The fade-in first differs between the two, as a window's does: the settled one is compared.
    assert!(
        run.lines("same: real window")
            .iter()
            .any(|line| line.contains("held for 2 and 2 captures")),
        "{}",
        run.stdout()
    );
    assert!(run.stdout().contains("0 differ"), "{}", run.stdout());
    assert!(!run.stderr().contains("UNREAD"), "{}", run.stderr());
}

#[test]
fn a_window_that_settles_on_another_picture_is_a_difference() {
    let sandbox = Sandbox::new("window-differ");
    let run = sandbox.run(&[("FAKE_WINDOW", "differ")]);
    assert_eq!(run.code(), Some(1), "{}\n{}", run.stdout(), run.stderr());
    let different = run.lines("DIFFERENT: real window");
    assert_eq!(different.len(), 1, "{}", run.stdout());
    // It names a settled picture of each, never the fade-in (0001).
    assert!(different[0].contains("/glow/window/000"), "{different:?}");
    assert!(different[0].contains("/metal/window/000"), "{different:?}");
    assert!(!different[0].contains("/0001.png"), "{different:?}");
    assert!(run.stdout().contains("1 differ"), "{}", run.stdout());
}

#[test]
fn captures_that_never_hold_still_are_not_a_settled_window() {
    let sandbox = Sandbox::new("window-unsettled");
    let run = sandbox.run(&[("FAKE_WINDOW", "unsettled")]);
    assert_eq!(run.code(), Some(1), "{}\n{}", run.stdout(), run.stderr());
    let different = run.lines("DIFFERENT: real window: no settled capture");
    assert_eq!(different.len(), 1, "{}", run.stdout());
}

// ---- which painter drew it ----

#[test]
fn a_shot_on_the_other_painter_stops_the_check_and_names_the_scene() {
    let sandbox = Sandbox::new("says-other");
    // The arm that would otherwise compare Metal with Metal, then the reverse.
    for (says, scene, asked) in [
        ("focused:glow:metal", "focused", "glow"),
        ("desert:metal:glow", "desert", "metal"),
    ] {
        let run = sandbox.run(&[("FAKE_SAYS", says)]);
        assert_eq!(run.code(), Some(1), "{}\n{}", run.stdout(), run.stderr());
        let stderr = run.stderr();
        let want = format!("the {scene} shot asked for {asked} and another painter drew it");
        assert!(stderr.contains(&want), "{stderr}");
        assert!(run.lines("same: ").is_empty(), "{}", run.stdout());
    }
}

#[test]
fn a_metal_shot_whose_window_failed_and_opened_again_on_glow_stops_the_check() {
    // The log says `painter: metal`, then `painter: glow`: the picture is glow's, and a Metal-against-glow
    // comparison that took it for Metal's would pass for the wrong reason.
    let sandbox = Sandbox::new("says-twice");
    for (also, scene) in [
        ("night:metal:glow", "night"),
        ("desert:metal:glow", "desert"),
    ] {
        let run = sandbox.run(&[("FAKE_ALSO", also)]);
        assert_eq!(run.code(), Some(1), "{}\n{}", run.stdout(), run.stderr());
        let want = format!("the {scene} shot asked for metal and another painter drew it");
        assert!(run.stderr().contains(&want), "{}", run.stderr());
        assert!(run.lines("same: ").is_empty(), "{}", run.stdout());
    }
}

#[test]
fn a_shot_that_says_its_own_painter_twice_stops_the_check_too() {
    // `check_painter` wants that one line and no other, so a window that said its painter twice is not
    // read as drawn by it.
    let sandbox = Sandbox::new("says-twice-same");
    let run = sandbox.run(&[("FAKE_ALSO", "focused:glow:glow")]);
    assert_eq!(run.code(), Some(1), "{}\n{}", run.stdout(), run.stderr());
    assert!(
        run.stderr()
            .contains("the focused shot asked for glow and another painter drew it"),
        "{}",
        run.stderr()
    );
}

#[test]
fn a_shot_that_says_no_painter_stops_the_check() {
    let sandbox = Sandbox::new("says-nothing");
    let run = sandbox.run(&[("FAKE_SAYS", "night:glow:")]);
    assert_eq!(run.code(), Some(1), "{}\n{}", run.stdout(), run.stderr());
    assert!(run.stderr().contains("the night shot asked for glow"));
}

#[test]
fn a_night_window_on_the_other_painter_stops_the_check() {
    let sandbox = Sandbox::new("says-window");
    let run = sandbox.run(&[("FAKE_SAYS", "shot:metal:glow"), ("FAKE_WINDOW", "same")]);
    assert_eq!(run.code(), Some(1), "{}\n{}", run.stdout(), run.stderr());
    let stderr = run.stderr();
    assert!(
        stderr.contains("the night window shot asked for metal"),
        "{stderr}"
    );
    assert!(run.lines("same: real window").is_empty());
}

// ---- a folder used before ----

#[test]
fn old_captures_do_not_vote_when_a_second_run_captures_nothing() {
    let sandbox = Sandbox::new("stale-captures");
    let first = sandbox.run(&[("FAKE_WINDOW", "same")]);
    assert_eq!(first.code(), Some(0), "{}", first.stderr());
    assert_eq!(first.lines("same: real window").len(), 1);
    // As with Screen Recording revoked: this run leaves no capture.
    let second = sandbox.run(&[]);
    assert_eq!(second.code(), Some(2), "{}", second.stdout());
    assert!(second.stderr().contains("UNREAD"), "{}", second.stderr());
    assert!(second.lines("same: real window").is_empty());
    for painter in ["glow", "metal"] {
        let window = sandbox.pictures().join(painter).join("window");
        assert!(!window.join("0002.png").exists(), "{painter}");
    }
}

#[test]
fn an_old_tinted_twin_is_not_compared_when_no_shot_wrote_one() {
    let sandbox = Sandbox::new("stale-tinted");
    assert_eq!(sandbox.run(&[]).lines("same: ").len(), 7);
    let second = sandbox.run(&[("FAKE_NO_TINT", "1")]);
    assert_eq!(second.lines("same: ").len(), 4, "{}", second.stdout());
    for picture in ["night-tinted", "focused-tinted", "desert-tinted"] {
        let lines = second.lines(&format!("DIFFERENT: {picture}"));
        assert_eq!(lines.len(), 1, "{picture}: {}", second.stdout());
    }
}

#[test]
fn a_run_leaves_what_it_did_not_write_in_the_callers_folder() {
    let sandbox = Sandbox::new("callers");
    let out = sandbox.pictures();
    let theirs = [
        "notes.txt",
        "night.png",
        "other/night.png",
        "glow/notes.png",
        "glow/night-old.png",
        "metal/window/keep.png",
        "metal/window/12345.png",
    ];
    fs::create_dir_all(out.join("glow")).expect("a painter's folder");
    fs::create_dir_all(out.join("metal/window")).expect("a window's folder");
    fs::create_dir_all(out.join("other")).expect("another folder");
    for name in theirs {
        fs::write(out.join(name), "the caller's").expect("a file of the caller's");
    }
    sandbox.run(&[]);
    for name in theirs {
        let kept = fs::read_to_string(out.join(name)).unwrap_or_default();
        assert_eq!(kept, "the caller's", "{name}");
    }
}

// ---- paths ----

#[test]
fn a_folder_with_a_space_in_its_name_compares_the_same_window_as_the_same() {
    let sandbox = Sandbox::new("space");
    let folder = sandbox.root.join("my pictures");
    let run = sandbox.run_in(Some(&folder), &sandbox.app(), &[("FAKE_WINDOW", "same")]);
    assert_eq!(run.code(), Some(0), "{}\n{}", run.stdout(), run.stderr());
    assert_eq!(run.lines("same: real window").len(), 1, "{}", run.stdout());
}

#[test]
fn a_folder_with_a_space_in_its_name_still_sees_a_different_window() {
    let sandbox = Sandbox::new("space-differ");
    let folder = sandbox.root.join("my pictures");
    let run = sandbox.run_in(Some(&folder), &sandbox.app(), &[("FAKE_WINDOW", "differ")]);
    assert_eq!(run.code(), Some(1), "{}\n{}", run.stdout(), run.stderr());
    let different = run.lines("DIFFERENT: real window");
    assert_eq!(different.len(), 1, "{}", run.stdout());
    assert!(
        different[0].contains("my pictures/glow/window/"),
        "{different:?}"
    );
}

#[test]
fn the_build_is_found_where_cargo_metadata_says() {
    let sandbox = Sandbox::new("metadata");
    let target = sandbox.root.join("where cargo builds");
    let bin = target.join("debug");
    fs::create_dir_all(&bin).expect("a build's folder");
    executable(&bin.join("arrakis"), APP);
    let run = sandbox.run_through_cargo(Some(&target));
    assert_eq!(run.code(), Some(2), "{}\n{}", run.stdout(), run.stderr());
    assert_eq!(run.lines("same: ").len(), 7, "the stand-in drew them");
    let calls = fs::read_to_string(bin.join("calls")).expect("the build there ran");
    assert_eq!(calls.lines().count(), 17, "{calls}");
}

#[test]
fn cargo_runs_from_the_repository_whatever_the_callers_folder() {
    let sandbox = Sandbox::new("metadata-root");
    let target = sandbox.root.join("target");
    let bin = target.join("debug");
    fs::create_dir_all(&bin).expect("a build's folder");
    executable(&bin.join("arrakis"), APP);
    sandbox.run_through_cargo(Some(&target));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("the repository");
    let calls = fs::read_to_string(sandbox.root.join("fakes/calls")).expect("cargo's calls");
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        [
            format!("{} build", root.display()),
            format!("{} metadata", root.display())
        ]
    );
}

#[test]
fn no_target_directory_from_cargo_stops_the_check() {
    let sandbox = Sandbox::new("metadata-none");
    let run = sandbox.run_through_cargo(None);
    assert_eq!(run.code(), Some(1), "{}\n{}", run.stdout(), run.stderr());
    assert!(
        run.stderr()
            .contains("cargo metadata did not give the target directory"),
        "{}",
        run.stderr()
    );
}
