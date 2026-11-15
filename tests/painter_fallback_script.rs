//! `scripts/painter-fallback.sh` against a stand-in for the demo build, which copies a canned report to
//! its `--demo-report` path. The script itself reads each failure on real windows: the process alive,
//! the tray up, the preview let go and the next window on glow. What these tests read is the script's
//! own reading of a report: that it holds what its table says, denies what the table denies, and
//! names each denial. The stand-in opens no window, and no drive runs.
#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(feature = "demo")]
use arrakis::drive::{Scenario, Spoiled};
#[cfg(feature = "demo")]
use clap::ValueEnum;
use serde_json::{Value, json};

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/painter-fallback.sh");

/// The failures with a drive, in the order the script runs them. The word `plate` comes after them.
const FAILURES: [&str; 7] = [
    "device", "pipeline", "surface", "frame", "stall", "metallib", "rest",
];

/// What the real build ends a plate with on a failed painter, as `main` prints its error.
const PLATE_SAID: &str = "this plate has no picture: ARRAKIS_METAL_FAIL=frame";

/// The stand-in for the demo build. It records the failure it was asked to make
/// (`ARRAKIS_METAL_FAIL`) and its arguments, and the `HOME` and `TMPDIR` it was given, then copies
/// `$FAKE_REPORTS/<failure>.json` to the `--demo-report` path, if there is such a file, and prints
/// `$FAKE_REPORTS/<failure>.log` as its log, if there is that.
///
/// - `FAKE_EXIT=<n>` is its exit status; by default 0.
///
/// Asked for `--demo-shot`, it acts as the real build's plate on a failed painter: says `$PLATE_SAID`
/// and exits 1. `FAKE_PLATE` makes it do otherwise:
///
/// - `hang` never ends, as a plate that does not read the flag;
/// - `clean` exits 0 and says nothing;
/// - `silent` exits 1 and says nothing;
/// - `picture` leaves a picture.
const APP: &str = r#"
here=$(dirname "$0")
echo "$ARRAKIS_METAL_FAIL $*" >>"$here/calls"
echo "$HOME $TMPDIR" >>"$here/env"
report=
shot=
while [ $# -gt 0 ]; do
    case "$1" in
    --demo-report) report=$2; shift ;;
    --demo-shot) shot=$2; shift ;;
    esac
    shift
done
if [ -n "$shot" ]; then
    case "${FAKE_PLATE-}" in
    hang) exec sleep 60 ;;
    clean) exit 0 ;;
    silent) exit 1 ;;
    picture) echo picture >"$shot" ;;
    esac
    echo "Error: AppCreation(\"the Metal painter failed, so $PLATE_SAID\")" >&2
    exit 1
fi
if [ -f "$FAKE_REPORTS/$ARRAKIS_METAL_FAIL.json" ]; then
    cp "$FAKE_REPORTS/$ARRAKIS_METAL_FAIL.json" "$report"
fi
if [ -f "$FAKE_REPORTS/$ARRAKIS_METAL_FAIL.log" ]; then
    cat "$FAKE_REPORTS/$ARRAKIS_METAL_FAIL.log" >&2
fi
exit "${FAKE_EXIT-0}"
"#;

/// The stand-in for cargo: `build` notes where it ran and fails when `FAKE_BUILD_FAILS` is set, and
/// `metadata` prints what the real one does for the target directory, or nothing when `FAKE_TARGET`
/// is empty.
const CARGO: &str = r#"
echo "$PWD $*" >>"$(dirname "$0")/calls"
if [ "$1" = build ] && [ -n "$FAKE_BUILD_FAILS" ]; then
    exit 101
fi
if [ "$1" = metadata ] && [ -n "$FAKE_TARGET" ]; then
    printf '{"packages":[],"target_directory":"%s","version":1}\n' "$FAKE_TARGET"
fi
"#;

/// A new folder with a `bin` for the stand-in, a `reports` folder for the canned reports and an empty
/// `tmp`, removed when it goes out of scope.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "arrakis-painter-fallback-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        for folder in ["bin", "reports", "tmp"] {
            fs::create_dir_all(root.join(folder)).expect("a folder");
        }
        Self { root }
    }

    /// A sandbox whose seven reports hold, with the one warning a `metallib` run logs.
    fn holding(name: &str) -> Self {
        let sandbox = Self::new(name);
        for failure in FAILURES {
            sandbox.put(failure, &report(failure));
        }
        sandbox.log("metallib", &refused(1));
        sandbox
    }

    /// The script's `TMPDIR`.
    fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// The folder that takes each run's report and log.
    fn out(&self) -> PathBuf {
        self.root.join("out")
    }

    /// The script's own work folder: the runs' `HOME` and `TMPDIR`.
    fn work(&self) -> PathBuf {
        self.tmp().join("arrakis-painter-fallback")
    }

    fn app(&self) -> PathBuf {
        let path = self.root.join("bin/arrakis");
        executable(&path, APP);
        path
    }

    /// What the stand-in will copy as the report of `failure`.
    fn put(&self, failure: &str, report: &Value) {
        let text = serde_json::to_string_pretty(report).expect("a report");
        fs::write(
            self.root.join("reports").join(format!("{failure}.json")),
            text,
        )
        .expect("a canned report");
    }

    /// What the stand-in will print as the log of `failure`.
    fn log(&self, failure: &str, said: &str) {
        fs::write(
            self.root.join("reports").join(format!("{failure}.log")),
            said,
        )
        .expect("a canned log");
    }

    /// Changes the canned report of `failure`.
    fn change(&self, failure: &str, change: impl FnOnce(&mut Value)) {
        let mut value = report(failure);
        change(&mut value);
        self.put(failure, &value);
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

    /// The script with the stand-in for the build and its reports in `out()`.
    fn run(&self, env: &[(&str, &str)]) -> Run {
        self.run_in(Some(&self.out()), &self.app(), env)
    }

    fn run_in(&self, folder: Option<&Path>, app: &Path, env: &[(&str, &str)]) -> Run {
        let output = Command::new("sh")
            .arg(SCRIPT)
            .args(folder)
            .env("PAINTER_FALLBACK_BIN", app)
            .env("FAKE_REPORTS", self.root.join("reports"))
            .env("PLATE_SAID", PLATE_SAID)
            .env("TMPDIR", self.tmp())
            .env_remove("PAINTER_FALLBACK_ONLY")
            .env_remove("PAINTER_FALLBACK_PLATE_LIMIT")
            .env_remove("FAKE_EXIT")
            .env_remove("FAKE_PLATE")
            .envs(env.iter().copied())
            .output()
            .expect("sh runs");
        Run(output)
    }

    /// The script with no build named, so it asks `cargo`: a stand-in, whose `metadata` gives
    /// `target`, and which notes where it ran. A `CARGO_TARGET_DIR` that says otherwise is set too.
    fn run_through_cargo(&self, target: Option<&Path>, build_fails: bool) -> Run {
        let fakes = self.root.join("fakes");
        fs::create_dir_all(&fakes).expect("a folder for the fakes");
        executable(&fakes.join("cargo"), CARGO);
        let path = std::env::var("PATH").expect("a PATH");
        let output = Command::new("sh")
            .arg(SCRIPT)
            .arg(self.out())
            .current_dir(&self.root)
            .env_remove("PAINTER_FALLBACK_BIN")
            .env("PAINTER_FALLBACK_ONLY", "device")
            .env("CARGO_TARGET_DIR", self.root.join("a guess"))
            .env("FAKE_REPORTS", self.root.join("reports"))
            .env("FAKE_TARGET", target.unwrap_or(Path::new("")))
            .env("FAKE_BUILD_FAILS", if build_fails { "1" } else { "" })
            .env("PATH", format!("{}:{path}", fakes.display()))
            .env("TMPDIR", self.tmp())
            .output()
            .expect("sh runs");
        Run(output)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A script at `path`, ready to run.
fn executable(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("a stand-in");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable");
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

    /// The line of stdout that begins with `prefix`, the only one.
    fn line(&self, prefix: &str) -> String {
        let lines: Vec<String> = self
            .stdout()
            .lines()
            .filter(|line| line.starts_with(prefix))
            .map(str::to_owned)
            .collect();
        match &lines[..] {
            [line] => line.clone(),
            _ => panic!(
                "{} lines begin with {prefix:?}: {}",
                lines.len(),
                self.stdout()
            ),
        }
    }

    /// Everything the run printed, for an assert's message.
    fn say(&self) -> String {
        format!("{}\n{}", self.stdout(), self.stderr())
    }
}

/// One window of a drive's report, as `Open` serializes it, of the fields the script reads.
fn window(first_frame_ms: f64) -> Value {
    json!({
        "renderer": "glow",
        "shader": Value::Null,
        "tray": true,
        "to_first_frame_ms": first_frame_ms,
        "frames": 20,
        "open_ms": 800.0,
    })
}

/// A log that says `times` times that the file does not serve, in the painter's own words, among other
/// lines.
fn refused(times: usize) -> String {
    let warning = format!(
        "[WARN  egui_wgpu::painter] {} (no library from the file: Invalid library file), so every \
         painter of this process compiles the shader from its source\n",
        eframe::egui_wgpu::METALLIB_REFUSED
    );
    format!("painter: metal\n{}drive: done\n", warning.repeat(times))
}

/// A report of `failure` that holds what the script's table wants: three windows for the five drives
/// that open and close, one for `frame` and `rest`, all glow's, each with the menu-bar item, except
/// under `metallib`, where every one is metal's with its shader from the source; one fallback,
/// replacing the first window, except under `device` and `metallib`, where no painter failed; the
/// engine gave the displays back, and under `frame`, the only drive tinted, checked them after the
/// fallback; no frame went unseen; and it took the time its drive takes, so that the driver flags
/// nothing (`spoiled` is null; each failure has the time that a drive of it read, below). The
/// fields are those of `Report` (`src/drive.rs`).
fn report(failure: &str) -> Value {
    let windows = if matches!(failure, "rest" | "frame") {
        1
    } else {
        3
    };
    let mut opens: Vec<Value> = (0..windows).map(|_| window(40.0)).collect();
    if failure == "metallib" {
        for open in &mut opens {
            open["renderer"] = json!("metal");
            open["shader"] = json!("source");
        }
    }
    let fallbacks = if matches!(failure, "device" | "metallib") {
        json!([])
    } else {
        json!([{
            "why": format!("ARRAKIS_METAL_FAIL={failure}"),
            "frames": 2,
            "at_open": 0,
            "screen_checks": 0,
        }])
    };
    let checks = if failure == "frame" { 2 } else { 0 };
    // Each failure's own time, as a drive of it read on 2026-11-12: inside the
    // bound of the drive the script starts, 9 to 13.75 s for the
    // `open-close` of the first five, 9 to 11 s for `open-night`, 5 to 7 s for `open-idle`.
    let elapsed_s = match failure {
        "device" => 9.76,
        "pipeline" => 9.90,
        "surface" => 9.88,
        "stall" => 10.94,
        "metallib" => 9.84,
        "frame" => 9.05,
        "rest" => 5.05,
        other => panic!("no canned time for {other}"),
    };
    json!({
        "finished": true,
        "elapsed_s": elapsed_s,
        "spoiled": Value::Null,
        "unseen_frames": 0,
        "opens": opens,
        "fallbacks": fallbacks,
        "screen": { "checks": checks, "paints": 0, "restores": 1 },
    })
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
fn seven_reports_and_a_plate_that_hold_exit_zero_and_each_run_says_so() {
    let sandbox = Sandbox::holding("holds");
    let run = sandbox.run(&[]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    for failure in FAILURES {
        let line = run.line(&format!("{failure}: "));
        assert!(line.starts_with(&format!("{failure}: held: ")), "{line}");
        // Each failure's own canned time, in the script's line to two decimals.
        let elapsed = report(failure)["elapsed_s"].as_f64().expect("a time");
        assert!(line.contains(&format!("elapsed_s={elapsed:.2}")), "{line}");
        assert!(line.contains("unseen_frames=0"), "{line}");
        assert!(!line.contains("COVERED"), "{line}");
    }
    assert!(run.line("device: ").contains("windows=3 fallbacks=0"));
    assert!(run.line("frame: ").contains("windows=1 fallbacks=1"));
    assert!(run.line("rest: ").contains("windows=1 fallbacks=1"));
    // The one failure that stays on Metal, and its one warning.
    assert!(
        run.line("metallib: ")
            .contains("windows=3 fallbacks=0 warnings=1")
    );
    assert!(!run.line("surface: ").contains("warnings="));
    let line = run.line("plate: ");
    assert!(line.starts_with("plate: held: elapsed_s="), "{line}");
    assert!(line.ends_with(" exit=1"), "{line}");
    // The fallback's window and why, for the reader of the line.
    let line = run.line("surface: ");
    assert!(line.contains("shown_after_failure_ms=40"), "{line}");
    assert!(line.contains("why='ARRAKIS_METAL_FAIL=surface'"), "{line}");
    assert!(!run.line("device: ").contains("shown_after_failure_ms"));
    assert_eq!(
        run.stdout().lines().last(),
        Some(
            format!(
                "painter-fallback.sh: every run held: {}",
                sandbox.out().display()
            )
            .as_str()
        )
    );
    assert_eq!(run.stderr(), "");
}

#[test]
fn each_failure_gets_its_drive_and_its_own_variable_and_a_report_in_its_folder() {
    let sandbox = Sandbox::holding("calls");
    sandbox.run(&[]);
    let out = sandbox.out();
    let drive = |failure: &str, scenario: &str, seconds: u32| {
        format!(
            "{failure} --demo-drive {scenario} --demo-seconds {seconds} --demo-report {}",
            out.join(failure).join("report.json").display()
        )
    };
    assert_eq!(
        sandbox.calls(),
        [
            drive("device", "open-close", 9),
            drive("pipeline", "open-close", 9),
            drive("surface", "open-close", 9),
            drive("frame", "open-night", 9),
            drive("stall", "open-close", 9),
            drive("metallib", "open-close", 9),
            drive("rest", "open-idle", 5),
            format!(
                "frame --demo-shot {} --demo-scene scale",
                out.join("plate/plate.png").display()
            ),
        ]
    );
    for failure in FAILURES {
        assert!(out.join(failure).join("report.json").is_file(), "{failure}");
        assert!(out.join(failure).join("app.log").is_file(), "{failure}");
    }
}

#[test]
fn the_drives_run_with_a_home_and_a_tmpdir_of_their_own() {
    let sandbox = Sandbox::holding("home");
    sandbox.run(&[]);
    let work = sandbox.work();
    let want = format!(
        "{} {}",
        work.join("home").display(),
        work.join("tmp").display()
    );
    let seen = sandbox.recorded("env");
    assert_eq!(seen.len(), 8, "seven drives and a plate: {seen:?}");
    // The settings and the single-instance slot of the real home and tmp are never the drives'.
    assert!(
        seen.iter().all(|line| *line == want),
        "{seen:#?}\nwanted {want}"
    );
}

#[test]
fn only_the_named_failures_run_in_the_order_named() {
    let sandbox = Sandbox::holding("only");
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "rest frame")]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let calls = sandbox.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[0].starts_with("rest --demo-drive open-idle --demo-seconds 5 "));
    assert!(calls[1].starts_with("frame --demo-drive open-night --demo-seconds 9 "));
}

#[test]
fn an_unknown_failure_stops_the_script_with_exit_one() {
    let sandbox = Sandbox::holding("unknown");
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "nope")]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.stderr()
            .contains("painter-fallback.sh: no such failure: nope"),
        "{}",
        run.stderr()
    );
    assert!(sandbox.calls().is_empty(), "{:?}", sandbox.calls());
    assert!(run.stdout().is_empty(), "{}", run.stdout());
}

/// A change to a report that the table denies, the failure it is made to, and what the run's line says.
type Denial = (&'static str, &'static str, fn(&mut Value), &'static str);

const DENIALS: [Denial; 26] = [
    (
        "a Metal window",
        "surface",
        |r| r["opens"][1]["renderer"] = json!("metal"),
        "window 1 is metal's",
    ),
    (
        "a window of no painter",
        "stall",
        |r| r["opens"][2]["renderer"] = json!(""),
        "window 2 is 's",
    ),
    (
        "a first window on Metal",
        "rest",
        |r| r["opens"][0]["renderer"] = json!("metal"),
        "window 0 is metal's",
    ),
    (
        "a window without the menu-bar item",
        "pipeline",
        |r| r["opens"][2]["tray"] = json!(false),
        "window 2 has no menu-bar item",
    ),
    (
        "a run that did not finish",
        "surface",
        |r| r["finished"] = json!(false),
        "it did not finish by itself",
    ),
    (
        "no restore",
        "stall",
        |r| r["screen"]["restores"] = json!(0),
        "the engine did not give the displays back",
    ),
    (
        "too few windows",
        "surface",
        |r| {
            r["opens"].as_array_mut().unwrap().pop();
        },
        "2 windows, not 3",
    ),
    (
        "a second window on the night's drive",
        "frame",
        |r| r["opens"].as_array_mut().unwrap().push(window(40.0)),
        "2 windows, not 1",
    ),
    (
        "a why of another failure",
        "surface",
        |r| r["fallbacks"][0]["why"] = json!("ARRAKIS_METAL_FAIL=frame"),
        "the fallback's why is 'ARRAKIS_METAL_FAIL=frame'",
    ),
    (
        "a why that is Metal's own, not the failure the run made",
        "rest",
        |r| r["fallbacks"][0]["why"] = json!("a command buffer failed"),
        "the fallback's why is 'a command buffer failed'",
    ),
    (
        "no checks at all on the night's drive",
        "frame",
        |r| r["screen"]["checks"] = json!(0),
        "the engine checked nothing after the fallback",
    ),
    (
        "checks only before the fallback",
        "frame",
        |r| {
            r["fallbacks"][0]["screen_checks"] = json!(2);
            r["screen"]["checks"] = json!(2);
        },
        "the engine checked nothing after the fallback",
    ),
    (
        "too many windows",
        "device",
        |r| r["opens"].as_array_mut().unwrap().push(window(40.0)),
        "4 windows, not 3",
    ),
    (
        "a second window at rest",
        "rest",
        |r| r["opens"].as_array_mut().unwrap().push(window(40.0)),
        "2 windows, not 1",
    ),
    (
        "no fallback",
        "frame",
        |r| r["fallbacks"] = json!([]),
        "0 fallbacks, not 1",
    ),
    (
        "no fallback at rest",
        "rest",
        |r| r["fallbacks"] = json!([]),
        "0 fallbacks, not 1",
    ),
    (
        "a fallback under device",
        "device",
        |r| r["fallbacks"] = json!([{ "why": "x", "frames": 1, "at_open": 0 }]),
        "1 fallbacks, not 0",
    ),
    (
        "two fallbacks",
        "stall",
        |r| {
            let f = r["fallbacks"][0].clone();
            r["fallbacks"].as_array_mut().unwrap().push(f);
        },
        "2 fallbacks, not 1",
    ),
    (
        "a replacement past the end",
        "frame",
        |r| r["fallbacks"][0]["at_open"] = json!(3),
        "the window after the failure showed at None ms",
    ),
    (
        "a replacement with no first frame",
        "surface",
        |r| r["opens"][0]["to_first_frame_ms"] = Value::Null,
        "the window after the failure showed at None ms",
    ),
    (
        "a replacement that showed late",
        "pipeline",
        |r| r["opens"][0]["to_first_frame_ms"] = json!(1500.0),
        "the window after the failure showed at 1500.0 ms",
    ),
    (
        "a replacement at rest that showed late",
        "rest",
        |r| r["opens"][0]["to_first_frame_ms"] = json!(1001.0),
        "the window after the failure showed at 1001.0 ms",
    ),
    (
        "a glow window where the file failed",
        "metallib",
        |r| r["opens"][1]["renderer"] = json!("glow"),
        "window 1 is glow's",
    ),
    (
        "a shader from the file where the file failed",
        "metallib",
        |r| r["opens"][0]["shader"] = json!("file"),
        "window 0's shader is from 'file'",
    ),
    (
        "no shader where the file failed",
        "metallib",
        |r| r["opens"][2]["shader"] = Value::Null,
        "window 2's shader is from None",
    ),
    (
        "a fallback to glow where the file failed",
        "metallib",
        |r| r["fallbacks"] = json!([{ "why": "x", "frames": 1, "at_open": 0 }]),
        "1 fallbacks, not 0",
    ),
];

#[test]
fn each_denial_exits_one_and_names_itself() {
    for (what, failure, change, says) in DENIALS {
        let sandbox = Sandbox::holding("denial");
        sandbox.change(failure, change);
        let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", failure)]);
        assert_eq!(run.code(), Some(1), "{what}: {}", run.say());
        let line = run.line(&format!("{failure}: "));
        assert!(line.contains(says), "{what}: wanted {says:?} in {line}");
        assert!(
            !line.starts_with(&format!("{failure}: held")),
            "{what}: {line}"
        );
        assert!(
            run.stderr()
                .contains("painter-fallback.sh: 1 runs did not hold: see"),
            "{what}: {}",
            run.stderr()
        );
        assert!(
            run.stdout().lines().all(|l| !l.contains("every run held")),
            "{what}"
        );
    }
}

#[test]
fn a_denial_in_one_run_leaves_the_others_to_run_and_say_held() {
    let sandbox = Sandbox::holding("one-bad");
    sandbox.change("frame", |r| r["finished"] = json!(false));
    let run = sandbox.run(&[]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert_eq!(sandbox.calls().len(), 8, "every failure ran, and the plate");
    assert!(run.line("plate: ").contains(": held: "));
    for failure in FAILURES.iter().filter(|failure| **failure != "frame") {
        assert!(
            run.line(&format!("{failure}: ")).contains(": held: "),
            "{failure}"
        );
    }
    assert!(run.line("frame: ").contains("it did not finish by itself"));
    assert!(
        run.stderr().contains(&format!(
            "painter-fallback.sh: 1 runs did not hold: see {}",
            sandbox.out().display()
        )),
        "{}",
        run.stderr()
    );
}

#[test]
fn every_denial_of_one_run_is_named_in_its_line() {
    let sandbox = Sandbox::holding("many-bad");
    sandbox.change("stall", |r| {
        r["finished"] = json!(false);
        r["opens"][1]["renderer"] = json!("metal");
        r["opens"][2]["tray"] = json!(false);
        r["screen"]["restores"] = json!(0);
    });
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "stall")]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    let line = run.line("stall: ");
    for says in [
        "it did not finish by itself",
        "window 1 is metal's",
        "window 2 has no menu-bar item",
        "the engine did not give the displays back",
    ] {
        assert!(line.contains(says), "{says:?} missing from {line}");
    }
    assert!(
        run.stderr().contains("1 runs did not hold"),
        "one run, however many denials: {}",
        run.stderr()
    );
}

#[test]
fn two_runs_that_do_not_hold_are_counted_as_two() {
    let sandbox = Sandbox::holding("two-bad");
    sandbox.change("device", |r| r["finished"] = json!(false));
    sandbox.change("rest", |r| r["screen"]["restores"] = json!(0));
    let run = sandbox.run(&[]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.stderr().contains("2 runs did not hold"),
        "{}",
        run.stderr()
    );
}

#[test]
fn the_window_that_replaced_the_failed_one_is_the_one_read_for_its_first_frame() {
    // `at_open` is the index in `opens` of the window after the failure. The script must read that
    // entry, not the first, and not the slowest.
    let slow_at_one = |failure: &str, at_open: u64| {
        let sandbox = Sandbox::holding("at-open");
        sandbox.change(failure, |r| {
            r["fallbacks"][0]["at_open"] = json!(at_open);
            r["opens"][1]["to_first_frame_ms"] = json!(2000.0);
        });
        sandbox.run(&[("PAINTER_FALLBACK_ONLY", failure)])
    };
    let run = slow_at_one("surface", 1);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.line("surface: ").contains("showed at 2000.0 ms"),
        "{}",
        run.stdout()
    );
    // The slow window is not the replacement: the run holds.
    let run = slow_at_one("surface", 0);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert!(run.line("surface: ").contains("shown_after_failure_ms=40"));
    let run = slow_at_one("surface", 2);
    assert_eq!(run.code(), Some(0), "{}", run.say());
}

/// The script reads the report's own words for the two painters, so a word that changes on one side
/// shows here and not as a drive that says every window is someone else's.
#[cfg(feature = "demo")]
#[test]
fn the_names_a_report_gives_its_painters_are_the_ones_the_script_reads() {
    use arrakis::painter::WindowPainter;

    let sandbox = Sandbox::holding("names");
    let run_with = |painter: WindowPainter| {
        sandbox.change("rest", |r| {
            r["opens"][0]["renderer"] = json!(painter.name());
        });
        sandbox.run(&[("PAINTER_FALLBACK_ONLY", "rest")])
    };
    let run = run_with(WindowPainter::Glow);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let run = run_with(WindowPainter::Metal);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    let line = run.line("rest: ");
    assert!(line.contains("window 0 is metal's"), "{line}");
}

/// The app makes a painter for each of the three windows, and the process says once that the file does
/// not serve, in the painter's own words (`METALLIB_REFUSED`), which the stand-in prints: no warning,
/// two, or another text is a run that did not hold.
#[test]
fn a_run_on_a_file_that_fails_holds_with_one_warning_in_the_painters_words() {
    let sandbox = Sandbox::holding("warnings");
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "metallib")]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let not_one = |said: &str, says: &str| {
        sandbox.log("metallib", said);
        let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "metallib")]);
        assert_eq!(run.code(), Some(1), "{}", run.say());
        let line = run.line("metallib: ");
        assert!(line.contains(says), "wanted {says:?} in {line}");
    };
    not_one(
        &refused(0),
        "0 warnings that the file does not serve, not 1",
    );
    not_one(
        &refused(2),
        "2 warnings that the file does not serve, not 1",
    );
    not_one(
        &refused(1).replace(eframe::egui_wgpu::METALLIB_REFUSED, "the file did not load"),
        "0 warnings that the file does not serve, not 1",
    );
}

#[test]
fn a_night_drive_only_needs_checks_after_its_fallback_where_the_run_is_tinted() {
    // The table reads `screen.checks` on `frame` alone: the others run by day, where a screen gets
    // none, and hold with no checks at all.
    let sandbox = Sandbox::holding("checks");
    for failure in ["surface", "rest"] {
        assert_eq!(report(failure)["screen"]["checks"], json!(0));
        let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", failure)]);
        assert_eq!(run.code(), Some(0), "{failure}: {}", run.say());
    }
    // One check after the fallback is enough.
    sandbox.change("frame", |r| {
        r["fallbacks"][0]["screen_checks"] = json!(4);
        r["screen"]["checks"] = json!(5);
    });
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "frame")]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
}

/// The word `plate` alone, with what the stand-in does for it and how long the script gives it.
fn run_plate(sandbox: &Sandbox, fake: &str, limit: &str) -> Run {
    sandbox.run(&[
        ("PAINTER_FALLBACK_ONLY", "plate"),
        ("FAKE_PLATE", fake),
        ("PAINTER_FALLBACK_PLATE_LIMIT", limit),
    ])
}

#[test]
fn a_plate_that_ends_with_an_error_and_no_picture_holds_and_its_log_keeps_what_it_said() {
    let sandbox = Sandbox::holding("plate-holds");
    let run = run_plate(&sandbox, "", "20");
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let log = fs::read_to_string(sandbox.out().join("plate/app.log")).expect("the plate's log");
    assert!(log.contains(PLATE_SAID), "{log}");
    // It is no drive, so it runs with no report and no `--demo-seconds`.
    let calls = sandbox.calls();
    assert!(!calls[0].contains("--demo-report"), "{calls:?}");
    assert!(!calls[0].contains("--demo-seconds"), "{calls:?}");
}

#[test]
fn a_plate_that_never_ends_is_killed_at_the_limit_and_denied() {
    let sandbox = Sandbox::holding("plate-hangs");
    let started = std::time::Instant::now();
    let run = run_plate(&sandbox, "hang", "1");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "the stand-in sleeps for 60 s, so the script waited for it: {:?}",
        started.elapsed()
    );
    assert_eq!(run.code(), Some(1), "{}", run.say());
    let line = run.line("plate: ");
    assert!(line.contains("it did not end within 1 s"), "{line}");
    assert!(line.ends_with(" exit=None"), "{line}");
}

#[test]
fn a_plate_that_exits_zero_says_nothing_or_leaves_a_picture_is_denied() {
    for (fake, says) in [
        ("clean", "it exited 0"),
        ("clean", "it did not say why"),
        ("silent", "it did not say why"),
        ("picture", "it left a picture"),
    ] {
        let sandbox = Sandbox::holding("plate-denied");
        let run = run_plate(&sandbox, fake, "20");
        assert_eq!(run.code(), Some(1), "{fake}: {}", run.say());
        let line = run.line("plate: ");
        assert!(line.contains(says), "{fake}: wanted {says:?} in {line}");
    }
}

#[test]
fn a_picture_left_by_an_earlier_run_is_not_this_plates() {
    let sandbox = Sandbox::holding("plate-stale");
    let earlier = sandbox.out().join("plate");
    fs::create_dir_all(&earlier).expect("an earlier run's folder");
    fs::write(earlier.join("plate.png"), "an earlier picture").expect("an earlier picture");
    let run = run_plate(&sandbox, "", "20");
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert!(!earlier.join("plate.png").exists());
}

#[test]
fn a_replacement_that_showed_within_a_second_holds_and_one_over_it_does_not() {
    let shown = |ms: f64| {
        let sandbox = Sandbox::holding("limit");
        sandbox.change("frame", |r| r["opens"][0]["to_first_frame_ms"] = json!(ms));
        sandbox.run(&[("PAINTER_FALLBACK_ONLY", "frame")])
    };
    let run = shown(999.0);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert!(run.line("frame: ").contains("shown_after_failure_ms=999"));
    let run = shown(1000.0);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let run = shown(1000.5);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(run.line("frame: ").contains("showed at 1000.5 ms"));
}

#[test]
fn a_run_that_wrote_no_report_exits_one_and_names_its_log() {
    let sandbox = Sandbox::holding("no-report");
    fs::remove_file(sandbox.root.join("reports/surface.json")).expect("a canned report");
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "surface")]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert_eq!(
        run.line("surface: "),
        format!(
            "surface: no report: see {}",
            sandbox.out().join("surface/app.log").display()
        )
    );
    assert!(
        run.stderr().contains(&format!(
            "painter-fallback.sh: 1 runs did not hold: see {}",
            sandbox.out().display()
        )),
        "{}",
        run.stderr()
    );
}

#[test]
fn an_empty_report_is_no_report() {
    let sandbox = Sandbox::holding("empty-report");
    fs::write(sandbox.root.join("reports/frame.json"), "").expect("an empty report");
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "frame")]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(run.line("frame: ").starts_with("frame: no report: see "));
}

#[test]
fn a_report_left_by_an_earlier_run_is_not_this_runs() {
    let sandbox = Sandbox::holding("stale");
    // An earlier run in the same folder held; this one's drive writes nothing.
    let earlier = sandbox.out().join("frame");
    fs::create_dir_all(&earlier).expect("an earlier run's folder");
    fs::write(
        earlier.join("report.json"),
        serde_json::to_string(&report("frame")).expect("a report"),
    )
    .expect("an earlier report");
    fs::write(earlier.join("app.log"), "an earlier log").expect("an earlier log");
    fs::remove_file(sandbox.root.join("reports/frame.json")).expect("a canned report");
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "frame")]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.line("frame: ").starts_with("frame: no report"),
        "{}",
        run.stdout()
    );
    assert!(!earlier.join("report.json").exists());
}

#[test]
fn the_apps_exit_status_is_not_what_the_script_reads() {
    // A drive that the driver had to end also writes its report, with `finished` false; one that wrote
    // a report that holds is read as it says, whatever the status.
    let sandbox = Sandbox::holding("exit");
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "frame"), ("FAKE_EXIT", "3")]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    sandbox.change("frame", |r| r["finished"] = json!(false));
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "frame"), ("FAKE_EXIT", "0")]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(run.line("frame: ").contains("it did not finish by itself"));
}

#[test]
fn a_covered_window_says_so_and_does_not_fail_the_run_by_itself() {
    let sandbox = Sandbox::holding("covered");
    sandbox.change("rest", |r| r["unseen_frames"] = json!(4));
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "rest")]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let line = run.line("rest: ");
    assert!(line.contains(": held: "), "{line}");
    assert!(line.contains("unseen_frames=4 COVERED"), "{line}");
}

#[test]
fn a_denied_run_that_was_also_covered_says_both() {
    let sandbox = Sandbox::holding("covered-denied");
    sandbox.change("device", |r| {
        r["unseen_frames"] = json!(1);
        r["finished"] = json!(false);
    });
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "device")]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    let line = run.line("device: ");
    assert!(line.contains("it did not finish by itself"), "{line}");
    assert!(line.contains("unseen_frames=1 COVERED"), "{line}");
}

// The tests that read the driver's own reason and bound (`arrakis::drive`, the demo build's) are
// the ones with the feature; the rest of the file runs in a plain `cargo test`.

/// The reason that the driver gives a drive of `scenario` for `seconds` that took `elapsed_s`.
#[cfg(feature = "demo")]
fn reason(scenario: Scenario, seconds: u64, elapsed_s: f64) -> String {
    Spoiled::of(scenario, seconds, elapsed_s)
        .expect("a time out of the bound")
        .to_string()
}

#[cfg(feature = "demo")]
#[test]
fn a_drive_that_the_machine_slowed_fails_the_run_and_names_the_reason() {
    let sandbox = Sandbox::holding("slowed");
    // `device` is `open-close` for 9 s, 3 cycles: 9.000 to 13.750 s. The display slept for 17 minutes.
    let why = reason(Scenario::OpenClose, 9, 1020.0);
    sandbox.change("device", |r| {
        r["elapsed_s"] = json!(1020.0);
        r["spoiled"] = json!(why);
    });
    let run = sandbox.run(&[]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    let line = run.line("device: ");
    assert!(
        line.contains("spoiled: elapsed_s 1020.000 s is outside 9.000 to 13.750 s"),
        "{line}"
    );
    assert!(line.contains(&format!("spoiled: {why}")), "{line}");
    assert!(!line.starts_with("device: held"), "{line}");
    // The other runs are read as they were, and the plate too.
    for failure in FAILURES.iter().filter(|failure| **failure != "device") {
        assert!(
            run.line(&format!("{failure}: "))
                .starts_with(&format!("{failure}: held: ")),
            "{failure}: {}",
            run.say()
        );
    }
    assert!(run.line("plate: ").contains(": held: "));
    assert!(
        run.stderr()
            .contains("painter-fallback.sh: 1 runs did not hold: see"),
        "{}",
        run.stderr()
    );
}

#[test]
fn a_report_whose_spoiled_is_null_or_is_not_there_holds() {
    // The canned reports have the field the driver writes, null.
    let sandbox = Sandbox::holding("not-spoiled");
    assert!(report("rest")["spoiled"].is_null());
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "rest")]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert!(run.line("rest: ").contains(": held: "), "{}", run.say());

    // A driver built before the key writes none, and its drive counts as in its time.
    sandbox.change("rest", |r| {
        r.as_object_mut().expect("an object").remove("spoiled");
    });
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "rest")]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert!(run.line("rest: ").contains(": held: "), "{}", run.say());
}

#[cfg(feature = "demo")]
#[test]
fn a_denied_run_that_was_also_spoiled_says_both() {
    let sandbox = Sandbox::holding("spoiled-denied");
    let why = reason(Scenario::OpenClose, 9, 1020.0);
    sandbox.change("device", |r| {
        r["elapsed_s"] = json!(1020.0);
        r["spoiled"] = json!(why);
        r["finished"] = json!(false);
    });
    let run = sandbox.run(&[("PAINTER_FALLBACK_ONLY", "device")]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    let line = run.line("device: ");
    assert!(line.contains("it did not finish by itself"), "{line}");
    assert!(line.contains(&format!("spoiled: {why}")), "{line}");
}

#[cfg(feature = "demo")]
#[test]
fn every_canned_report_is_in_the_time_of_the_drive_the_script_gave_it() {
    // The drives are the script's own, as the stand-in recorded them: a canned report that the
    // driver would flag for the drive it stands for would be one the driver cannot produce.
    let sandbox = Sandbox::holding("canned-time");
    let run = sandbox.run(&[]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let mut read: Vec<String> = Vec::new();
    for call in sandbox.calls() {
        let words: Vec<&str> = call.split_whitespace().collect();
        // `<failure> --demo-drive <scenario> --demo-seconds <seconds> --demo-report <path>`; the
        // plate's call has a shot in its place.
        let [
            failure,
            "--demo-drive",
            scenario,
            "--demo-seconds",
            seconds,
            ..,
        ] = words[..]
        else {
            continue;
        };
        let scenario = Scenario::from_str(scenario, false)
            .unwrap_or_else(|error| panic!("{call}: no such scenario: {error}"));
        let seconds: u64 = seconds.parse().expect("a whole number of seconds");
        let elapsed_s = report(failure)["elapsed_s"].as_f64().expect("a time");
        assert_eq!(
            Spoiled::of(scenario, seconds, elapsed_s),
            None,
            "{failure}: {seconds} s of {scenario:?} took {elapsed_s} s"
        );
        read.push(failure.to_owned());
    }
    assert_eq!(read, FAILURES);
}

#[test]
fn a_missing_build_is_refused_before_anything_runs() {
    let sandbox = Sandbox::holding("no-bin");
    let missing = sandbox.root.join("bin/not-there");
    let run = sandbox.run_in(Some(&sandbox.out()), &missing, &[]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.stderr()
            .contains(&format!("painter-fallback.sh: no {}", missing.display())),
        "{}",
        run.stderr()
    );
    assert!(sandbox.calls().is_empty());
}

#[test]
fn the_reports_go_under_tmpdir_when_no_folder_is_named() {
    let sandbox = Sandbox::holding("default-folder");
    let run = sandbox.run_in(None, &sandbox.app(), &[("PAINTER_FALLBACK_ONLY", "frame")]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let last = run.stdout().lines().last().expect("a last line").to_owned();
    let out = last
        .strip_prefix("painter-fallback.sh: every run held: ")
        .expect("the folder is named");
    assert!(
        Path::new(out).starts_with(sandbox.work()),
        "{out} is not under {}",
        sandbox.work().display()
    );
    assert!(Path::new(out).join("frame/report.json").is_file());
}

#[test]
fn a_folder_with_a_space_in_its_name_holds_the_same() {
    let sandbox = Sandbox::holding("space");
    let folder = sandbox.root.join("a folder with spaces");
    let run = sandbox.run_in(Some(&folder), &sandbox.app(), &[]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert!(folder.join("rest/report.json").is_file());
    assert!(
        sandbox
            .calls()
            .iter()
            .all(|call| call.contains("a folder with spaces")),
        "{:?}",
        sandbox.calls()
    );
}

#[test]
fn the_build_is_found_where_cargo_metadata_says_and_built_from_the_repository() {
    let sandbox = Sandbox::holding("metadata");
    let target = sandbox.root.join("where cargo builds");
    let bin = target.join("debug");
    fs::create_dir_all(&bin).expect("a build's folder");
    executable(&bin.join("arrakis"), APP);
    let run = sandbox.run_through_cargo(Some(&target), false);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let calls = fs::read_to_string(bin.join("calls")).expect("the build there ran");
    assert_eq!(calls.lines().count(), 1, "{calls}");

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("the repository");
    let cargo = fs::read_to_string(sandbox.root.join("fakes/calls")).expect("cargo's calls");
    assert_eq!(
        cargo.lines().collect::<Vec<_>>(),
        [
            format!("{} build --features demo", root.display()),
            format!("{} metadata --format-version 1 --no-deps", root.display()),
        ]
    );
}

#[test]
fn a_build_that_fails_stops_the_script_before_any_drive() {
    let sandbox = Sandbox::holding("build-fails");
    let target = sandbox.root.join("target");
    let bin = target.join("debug");
    fs::create_dir_all(&bin).expect("a build's folder");
    executable(&bin.join("arrakis"), APP);
    let run = sandbox.run_through_cargo(Some(&target), true);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.stderr()
            .contains("painter-fallback.sh: the demo build failed"),
        "{}",
        run.stderr()
    );
    assert!(
        !bin.join("calls").exists(),
        "a drive ran after a failed build"
    );
}

#[test]
fn no_target_directory_from_cargo_stops_the_script() {
    let sandbox = Sandbox::holding("metadata-none");
    let run = sandbox.run_through_cargo(None, false);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.stderr()
            .contains("painter-fallback.sh: cargo metadata did not give the target directory"),
        "{}",
        run.stderr()
    );
}
