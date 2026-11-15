//! `scripts/measure.sh`, run under `sh` with stand-ins first on `PATH`: a
//! `pgrep`, a `top` and the demo build's binary. `TMPDIR` is a new, empty
//! folder in every test, so the script finds no release build of the app to
//! start, whatever it decides: it never paints a display.

#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(feature = "demo")]
use arrakis::drive::Scenario;
#[cfg(feature = "demo")]
use clap::ValueEnum;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/measure.sh");
const COMMON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/measure/common.sh");

const SCENARIOS: [&str; 8] = [
    "closed-day",
    "closed-night",
    "closed-fade",
    "open-idle",
    "open-fade",
    "drag",
    "open-close",
    "start",
];

/// The scenarios whose subject is rest: a drive run settles before it
/// samples them.
const RESTING: [&str; 3] = ["closed-day", "closed-night", "open-idle"];

/// A new folder with a `bin` of stand-ins and an empty `tmp`, removed when it
/// goes out of scope.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "arrakis-measure-test-{}-{name}",
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

    /// Where the script keeps its files.
    fn measure(&self) -> PathBuf {
        self.tmp().join("arrakis-measure")
    }

    /// A stand-in program named `name` that runs `script`.
    fn fake(&self, name: &str, script: &str) -> PathBuf {
        self.executable(&self.root.join("bin").join(name), script)
    }

    fn executable(&self, path: &Path, script: &str) -> PathBuf {
        fs::create_dir_all(path.parent().expect("a folder")).expect("a folder");
        fs::write(path, format!("#!/bin/sh\n{script}\n")).expect("a stand-in");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable");
        path.to_owned()
    }

    /// A `pgrep` that finds nothing running.
    fn nothing_running(&self) {
        self.fake(
            "pgrep",
            "echo \"$@\" >>\"$(dirname \"$0\")/pgrep.args\"\nexit 1",
        );
    }

    /// A `pgrep` that finds process 4242.
    fn arrakis_running(&self) {
        self.fake(
            "pgrep",
            "echo \"$@\" >>\"$(dirname \"$0\")/pgrep.args\"\necho 4242",
        );
    }

    /// What the script was run with, as it ran.
    fn run(&self, args: &[&str]) -> Run {
        self.run_with(&[], args)
    }

    /// The same with `env` set; `SETTLE` and the bundle's environment are
    /// otherwise taken out of the environment, so that the script's own
    /// defaults hold.
    fn run_with(&self, env: &[(&str, &str)], args: &[&str]) -> Run {
        self.run_script(Path::new(SCRIPT), env, args)
    }

    /// The same with `script`, a copy of the script that may sit beside an
    /// `Info.plist` of its own.
    fn run_script(&self, script: &Path, env: &[(&str, &str)], args: &[&str]) -> Run {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let output = Command::new("sh")
            .arg(script)
            .args(args)
            .env_remove("SETTLE")
            .env_remove("NO_LSENVIRONMENT")
            .env_remove("MallocSpaceEfficient")
            .env_remove("ARRAKIS_TEST_KEY")
            .env("PATH", path)
            .env("TMPDIR", self.tmp())
            .envs(env.iter().copied())
            .output()
            .expect("sh runs");
        Run(output)
    }

    /// What a stand-in wrote to `file` in its folder, or nothing.
    fn recorded(&self, file: &str) -> String {
        fs::read_to_string(self.root.join("bin").join(file)).unwrap_or_default()
    }

    /// A copy of the script and of what it sources, in a folder of its own,
    /// beside an `Info.plist` that holds `plist`. Returns the copy.
    fn script_beside(&self, plist: &str) -> PathBuf {
        let scripts = self.root.join("repository/scripts");
        fs::create_dir_all(scripts.join("measure")).expect("a folder");
        fs::copy(SCRIPT, scripts.join("measure.sh")).expect("a copy");
        fs::copy(COMMON, scripts.join("measure/common.sh")).expect("a copy");
        fs::write(scripts.join("Info.plist"), plist).expect("an Info.plist");
        scripts.join("measure.sh")
    }
}

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
fn a_running_arrakis_refuses_every_real_run() {
    let sandbox = Sandbox::new("refuses");
    sandbox.arrakis_running();
    for scenario in ["s1", "s2", "s3", "s4"] {
        let run = sandbox.run(&["real", scenario]);
        assert_eq!(run.code(), Some(1), "{scenario}: {}", run.stderr());
        let stderr = run.stderr();
        assert!(stderr.contains("another arrakis is running"), "{stderr}");
        assert!(stderr.contains("4242"), "{stderr}");
    }
    // It asked for a process of exactly that name, ignoring case.
    let asked = sandbox.recorded("pgrep.args");
    assert!(asked.contains("-x arrakis"), "{asked}");
    // And started nothing: it wrote no settings and no runs.
    let written: Vec<_> = fs::read_dir(sandbox.measure())
        .expect("its folder")
        .collect();
    assert!(written.is_empty(), "{written:?}");
}

#[test]
fn with_nothing_running_a_real_run_still_needs_the_release_build() {
    let sandbox = Sandbox::new("no-binary");
    sandbox.nothing_running();
    let run = sandbox.run(&["real", "s2"]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    let stderr = run.stderr();
    assert!(!stderr.contains("another arrakis"), "{stderr}");
    let binary = sandbox.measure().join("real/release/arrakis");
    assert!(stderr.contains(&binary.display().to_string()), "{stderr}");
    assert!(stderr.contains("build it first"), "{stderr}");
    let written: Vec<_> = fs::read_dir(sandbox.measure())
        .expect("its folder")
        .collect();
    assert!(written.is_empty(), "started something: {written:?}");
}

#[test]
fn a_drive_run_needs_the_demo_build_and_knows_the_eight_scenarios() {
    let sandbox = Sandbox::new("no-driver");
    sandbox.nothing_running();
    let binary = sandbox.measure().join("drive/release/arrakis");
    for scenario in SCENARIOS {
        let run = sandbox.run(&["drive", scenario]);
        assert_eq!(run.code(), Some(1), "{scenario}: {}", run.stderr());
        let stderr = run.stderr();
        assert!(stderr.contains(&binary.display().to_string()), "{stderr}");
    }
}

#[test]
fn an_unknown_mode_or_scenario_prints_the_usage_and_exits_two() {
    let sandbox = Sandbox::new("usage");
    sandbox.nothing_running();
    let cases: [&[&str]; 8] = [
        &[],
        &["real"],
        &["bogus", "s1"],
        &["real", "s5"],
        &["real", "open-idle"],
        &["drive", "s1"],
        &["drive", "nonsense"],
        &["drive", "Open-Idle"],
    ];
    for args in cases {
        let run = sandbox.run(args);
        assert_eq!(run.code(), Some(2), "{args:?}: {}", run.stderr());
        let stderr = run.stderr();
        assert!(stderr.contains("measure.sh real"), "{args:?}: {stderr}");
        assert!(stderr.contains("measure.sh drive"), "{args:?}: {stderr}");
        // The usage says which scenarios settle first, and for how long.
        assert!(stderr.contains("SETTLE seconds (30"), "{args:?}: {stderr}");
        for resting in RESTING {
            assert!(stderr.contains(resting), "{args:?}: {stderr}");
        }
    }
}

#[test]
fn runs_and_seconds_must_be_positive_whole_numbers() {
    let sandbox = Sandbox::new("counts");
    sandbox.nothing_running();
    let cases: [&[&str]; 14] = [
        &["real", "s1", "0"],
        &["real", "s1", "abc"],
        &["drive", "open-idle", "0"],
        &["drive", "open-idle", "abc"],
        &["drive", "open-idle", "-1"],
        &["drive", "open-idle", "2x"],
        &["drive", "open-idle", "3", "0"],
        &["drive", "open-idle", "3", "abc"],
        // Zero however it is written.
        &["real", "s1", "00"],
        &["drive", "open-idle", "00"],
        &["drive", "open-idle", "000"],
        &["drive", "open-idle", "3", "00"],
        &["drive", "open-idle", "3", "0000"],
        &["drive", "open-idle", "3", "0.0"],
    ];
    for args in cases {
        let run = sandbox.run(args);
        assert_eq!(run.code(), Some(1), "{args:?}: {}", run.stderr());
        let stderr = run.stderr();
        assert!(
            stderr.contains("not a positive whole number"),
            "{args:?}: {stderr}"
        );
    }
}

/// The stand-in for the demo build: it counts its runs, to vary the numbers,
/// writes down how it was run and what it saw of the environment, and prints
/// a summary line. Its frames are 9, 100 and 10 for the first three runs, or
/// the run's line of a `frames` file beside it.
const DRIVER: &str = r#"
here=$(dirname "$0")
n=$(cat "$here/count" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" >"$here/count"
echo "$@" >"$here/args-$n"
echo "${MallocSpaceEfficient-unset} ${ARRAKIS_TEST_KEY-unset}" >"$here/env-$n"
case $n in 1) frames=9 ;; 2) frames=100 ;; *) frames=10 ;; esac
if [ -f "$here/frames" ]; then frames=$(sed -n "${n}p" "$here/frames"); fi
echo "scenario=$2 finished=true frames=$frames fps=- cover_builds=7"
"#;

/// The stand-in for `top -l N -s 5 -pid P -stats ...`: a header, another
/// process, and three samples of process P with the cumulative idle wakeups
/// going 100, 130, 160 and the memory 10M.
///
/// A watch of 7 samples, which the settle makes, sees a busy process
/// instead: start-up. Nothing but the settle asks for 7 in these tests.
const TOP: &str = r#"
echo "$@" >>"$(dirname "$0")/top.args"
samples=$2
while [ $# -gt 0 ]; do
    case "$1" in -pid) pid=$2 ;; esac
    shift
done
echo "PID COMMAND %CPU IDLEW POWER INSTRS MEM #TH"
echo "1 launchd 99.0 999 99.0 0 999G 99"
if [ "$samples" = 7 ]; then
    echo "$pid arrakis 80.0 100 90.0 1000 99M 9"
    echo "$pid arrakis 80.0 9100 90.0 2000 99M 9"
    echo "$pid arrakis 80.0 9600 90.0 3000 99M 9"
else
    echo "$pid arrakis 1.0 100 1.0 1000 10M 4"
    echo "$pid arrakis 3.0 130 2.0 2000 10M 5"
    echo "$pid arrakis 5.0 160 4.0 3000 10M 5"
fi
"#;

fn pairs(line: &str) -> BTreeSet<&str> {
    line.split(' ').collect()
}

#[test]
fn a_drive_run_prints_a_line_per_run_and_their_median() {
    let sandbox = Sandbox::new("drive");
    sandbox.nothing_running();
    sandbox.fake("top", TOP);
    sandbox.executable(&sandbox.measure().join("drive/release/arrakis"), DRIVER);

    // Drag is not about rest: no settle, `top` sampling from the launch.
    let run = sandbox.run(&["drive", "drag", "3", "5"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(!run.stderr().contains("did not finish"), "{}", run.stderr());
    let stdout = run.stdout();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 4, "three runs and a median: {stdout}");

    // Each run: the driver's summary, then what `top` saw of its process.
    // The idle wakeups per second are the 60 between the first and last
    // samples over their two 5 s intervals; power and CPU are the means
    // after the first sample; memory is 10M in KB.
    let measured = "idlew_per_s=6.000 power=3.000 cpu_percent=4.000 mem_kb=10240 threads=5";
    for (index, frames) in [9, 100, 10].into_iter().enumerate() {
        assert_eq!(
            lines[index],
            format!(
                "run {}: scenario=drag finished=true frames={frames} fps=- cover_builds=7 {measured}",
                index + 1
            )
        );
    }

    // The median of each number across the runs: 9, 100 and 10 sort as
    // numbers, not as text; a value that is missing every time stays missing.
    // An odd count prints as an even one does (see the test for it): 6, not
    // 6.000.
    let median = "idlew_per_s=6 power=3 cpu_percent=4 mem_kb=10240 threads=5";
    let expected = format!("median: frames=10 fps=- cover_builds=7 {median}");
    assert_eq!(pairs(lines[3]), pairs(&expected), "{}", lines[3]);

    // The driver was run as the flags spell it, with a report of its own.
    for run in 1..=3 {
        let args = fs::read_to_string(
            sandbox
                .measure()
                .join("drive/release")
                .join(format!("args-{run}")),
        )
        .expect("the driver's arguments");
        let report = sandbox.measure().join(format!("drag-{run}.json"));
        assert_eq!(
            args.trim(),
            format!(
                "--demo-drive drag --demo-seconds 5 --demo-report {}",
                report.display()
            )
        );
    }
    // `top` watched each run: a sample now and one every 5 s of its 5 s.
    let sampled = sandbox.recorded("top.args");
    assert_eq!(sampled.lines().count(), 3, "{sampled}");
    for call in sampled.lines() {
        assert!(call.starts_with("-l 2 -s 5 -pid "), "{call}");
        assert!(call.ends_with("-stats pid,command,cpu,idlew,power,instrs,mem,th"));
    }
    // The raw files are kept.
    let kept = fs::read_to_string(sandbox.measure().join("drag-runs.txt")).expect("the runs");
    assert_eq!(kept.lines().count(), 3);
}

/// What `--demo-seconds` each run of the stand-in driver was given, and the
/// `top` calls it made, as (settling, sampling) pairs: the `-l` counts.
fn drove(sandbox: &Sandbox, runs: usize) -> (Vec<String>, Vec<String>) {
    let seconds = (1..=runs)
        .map(|run| {
            let args = fs::read_to_string(
                sandbox
                    .measure()
                    .join("drive/release")
                    .join(format!("args-{run}")),
            )
            .expect("the driver's arguments");
            let mut words = args.split_whitespace();
            words
                .by_ref()
                .find(|word| *word == "--demo-seconds")
                .expect("a --demo-seconds flag");
            words.next().expect("its value").to_owned()
        })
        .collect();
    let samples = sandbox
        .recorded("top.args")
        .lines()
        .map(|call| call.split_whitespace().nth(1).unwrap_or("").to_owned())
        .collect();
    (seconds, samples)
}

#[test]
fn a_resting_scenario_settles_before_it_is_sampled() {
    for scenario in RESTING {
        let sandbox = Sandbox::new(&format!("settle-{scenario}"));
        sandbox.nothing_running();
        sandbox.fake("top", TOP);
        sandbox.executable(&sandbox.measure().join("drive/release/arrakis"), DRIVER);
        let run = sandbox.run(&["drive", scenario, "2", "20"]);
        assert_eq!(run.code(), Some(0), "{scenario}: {}", run.stderr());
        // The driver runs for the settle and the 20 s measured: 30 + 20.
        // `top` watches twice in each run: 7 samples to settle (30 s), whose
        // output is dropped, then 5 for the 20 s that are measured.
        let (seconds, samples) = drove(&sandbox, 2);
        assert_eq!(seconds, ["50", "50"], "{scenario}");
        assert_eq!(samples, ["7", "5", "7", "5"], "{scenario}");
        // What is measured is read from the second watch alone: the settle
        // saw a busy process (see the stand-in), and none of it is in the line.
        let stdout = run.stdout();
        let line = stdout.lines().next().expect("a run");
        let measured = "idlew_per_s=6.000 power=3.000 cpu_percent=4.000 mem_kb=10240 threads=5";
        assert!(line.ends_with(measured), "{line}");
    }
}

#[test]
fn a_scenario_about_activity_does_not_settle() {
    let active: Vec<&str> = SCENARIOS
        .into_iter()
        .filter(|scenario| !RESTING.contains(scenario) && *scenario != "start")
        .collect();
    assert_eq!(active.len(), 4, "closed-fade, open-fade, drag, open-close");
    for scenario in active {
        let sandbox = Sandbox::new(&format!("no-settle-{scenario}"));
        sandbox.nothing_running();
        sandbox.fake("top", TOP);
        sandbox.executable(&sandbox.measure().join("drive/release/arrakis"), DRIVER);
        let run = sandbox.run(&["drive", scenario, "2", "20"]);
        assert_eq!(run.code(), Some(0), "{scenario}: {}", run.stderr());
        let (seconds, samples) = drove(&sandbox, 2);
        assert_eq!(seconds, ["20", "20"], "{scenario}");
        assert_eq!(samples, ["5", "5"], "{scenario}");
    }
}

#[test]
fn the_settle_is_a_variable_a_whole_number_of_seconds() {
    let sandbox = Sandbox::new("settle-env");
    sandbox.nothing_running();
    sandbox.fake("top", TOP);
    sandbox.executable(&sandbox.measure().join("drive/release/arrakis"), DRIVER);
    let run = sandbox.run_with(&[("SETTLE", "10")], &["drive", "closed-day", "1", "5"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let (seconds, samples) = drove(&sandbox, 1);
    assert_eq!(seconds, ["15"]);
    assert_eq!(samples, ["3", "2"], "10 s settled, then 5 s");

    for bad in ["0", "00", "abc", "-5", "2.5"] {
        let run = sandbox.run_with(&[("SETTLE", bad)], &["drive", "closed-day", "1", "5"]);
        assert_eq!(run.code(), Some(1), "SETTLE={bad}: {}", run.stderr());
        let stderr = run.stderr();
        assert!(
            stderr.contains(&format!("not a positive whole number: SETTLE={bad}")),
            "{bad}: {stderr}"
        );
    }
}

#[test]
fn counts_with_leading_zeros_are_the_number_they_spell() {
    // 010 is ten, not the eight of an octal reading, and 05 is five.
    let sandbox = Sandbox::new("leading-zeros");
    sandbox.nothing_running();
    sandbox.fake("top", TOP);
    sandbox.executable(&sandbox.measure().join("drive/release/arrakis"), DRIVER);
    let run = sandbox.run(&["drive", "drag", "02", "010"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let (seconds, samples) = drove(&sandbox, 2);
    assert_eq!(seconds, ["10", "10"]);
    assert_eq!(samples, ["3", "3"]);
    assert_eq!(run.stdout().lines().count(), 3, "two runs and a median");
}

#[test]
fn a_median_prints_a_whole_number_whole_and_every_count_the_same_way() {
    let median = |frames: &[&str]| {
        let sandbox = Sandbox::new(&format!("median-{}", frames.join("-")));
        sandbox.nothing_running();
        sandbox.fake("top", TOP);
        let driver = sandbox.measure().join("drive/release");
        sandbox.executable(&driver.join("arrakis"), DRIVER);
        fs::write(driver.join("frames"), frames.join("\n") + "\n").expect("frames");
        let runs = frames.len().to_string();
        let run = sandbox.run(&["drive", "drag", &runs, "5"]);
        assert_eq!(run.code(), Some(0), "{frames:?}: {}", run.stderr());
        let stdout = run.stdout();
        let last = stdout.lines().last().expect("a median line");
        last.split(' ')
            .find_map(|pair| pair.strip_prefix("frames="))
            .unwrap_or_else(|| panic!("no frames in {last}"))
            .to_owned()
    };
    // An even count averages the middle two: 73,150,000, not 7.315e+07.
    assert_eq!(median(&["73100000", "73200000"]), "73150000");
    assert_eq!(
        median(&["1", "99999999", "100000001", "100000003"]),
        "100000000"
    );
    // A fraction stays one.
    assert_eq!(median(&["1", "2"]), "1.5");
    assert_eq!(median(&["10", "21", "1", "3"]), "6.5");
    // An odd count prints as a whole number the same way.
    assert_eq!(median(&["73100000", "73200000", "99999999"]), "73200000");
    assert_eq!(median(&["7", "9", "8"]), "8");
    // One run is its own median.
    assert_eq!(median(&["12345678"]), "12345678");
}

/// What the stand-in driver saw of `MallocSpaceEfficient` and
/// `ARRAKIS_TEST_KEY` in its run `run`, `unset` for one it did not see.
fn environment_of(sandbox: &Sandbox, run: usize) -> String {
    fs::read_to_string(
        sandbox
            .measure()
            .join("drive/release")
            .join(format!("env-{run}")),
    )
    .expect("the driver's environment")
    .trim()
    .to_owned()
}

/// A sandbox with the stand-in driver and `top`, ready for a drive run.
fn drive_sandbox(name: &str) -> Sandbox {
    let sandbox = Sandbox::new(name);
    sandbox.nothing_running();
    sandbox.fake("top", TOP);
    sandbox.executable(&sandbox.measure().join("drive/release/arrakis"), DRIVER);
    sandbox
}

/// An `Info.plist` laid out as the repository's is, whose `LSEnvironment`
/// holds `environment`, after a dictionary of another key that must not be
/// read.
fn plist_with(environment: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n\
         \t<key>Other</key>\n\t<dict>\n\t\t<key>NotMine</key>\n\t\t<string>x</string>\n\t</dict>\n\
         {environment}</dict>\n</plist>\n"
    )
}

#[test]
fn the_app_runs_with_the_bundles_environment_unless_told_not_to() {
    // The repository's own Info.plist holds MallocSpaceEfficient=1, the
    // owner's choice (`tests/bundle.rs` pins it): a bare run of the binary
    // has none, and a reading of it would be of a build that does not ship.
    let sandbox = drive_sandbox("bundle-env");
    let run = sandbox.run(&["drive", "drag", "1", "5"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(environment_of(&sandbox, 1), "1 unset");
    let stderr = run.stderr();
    assert!(
        stderr.contains("with the bundle's LSEnvironment")
            && stderr.contains(" MallocSpaceEfficient=1"),
        "{stderr}"
    );

    // The bundle's value wins over a shell's: LaunchServices does not ask it.
    let run = sandbox.run_with(
        &[("MallocSpaceEfficient", "0")],
        &["drive", "drag", "1", "5"],
    );
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(environment_of(&sandbox, 2), "1 unset");
}

#[test]
fn no_lsenvironment_takes_the_bundles_keys_out_and_only_those() {
    let sandbox = drive_sandbox("no-bundle-env");
    let run = sandbox.run_with(
        &[
            ("NO_LSENVIRONMENT", "1"),
            ("MallocSpaceEfficient", "1"),
            ("ARRAKIS_TEST_KEY", "kept"),
        ],
        &["drive", "drag", "1", "5"],
    );
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    // The shell had the flag, and the other arm of a comparison must not;
    // a variable the plist does not name is left alone.
    assert_eq!(environment_of(&sandbox, 1), "unset kept");
    let stderr = run.stderr();
    assert!(
        stderr.contains("NO_LSENVIRONMENT is set: the app runs without MallocSpaceEfficient=1"),
        "{stderr}"
    );
}

#[test]
fn every_key_of_the_plists_dictionary_is_exported_and_no_other() {
    let sandbox = drive_sandbox("several-keys");
    let plist = plist_with(
        "\t<key>LSEnvironment</key>\n\t<dict>\n\
         \t\t<key>MallocSpaceEfficient</key>\n\t\t<string>1</string>\n\
         \t\t<key>ARRAKIS_TEST_KEY</key>\n\t\t<string>a=b c</string>\n\t</dict>\n\
         \t<key>NotMineEither</key>\n\t<string>y</string>\n",
    );
    let script = sandbox.script_beside(&plist);
    let run = sandbox.run_script(&script, &[], &["drive", "drag", "1", "5"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    // A value keeps its `=` and its space.
    assert_eq!(environment_of(&sandbox, 1), "1 a=b c");
    let stderr = run.stderr();
    assert!(
        stderr.contains("MallocSpaceEfficient=1 ARRAKIS_TEST_KEY=a=b c"),
        "{stderr}"
    );
    assert!(!stderr.contains("NotMine"), "{stderr}");
}

#[test]
fn a_plist_without_an_lsenvironment_leaves_the_environment_as_it_is() {
    let sandbox = drive_sandbox("no-keys");
    for body in ["", "\t<key>LSEnvironment</key>\n\t<dict/>\n"] {
        let script = sandbox.script_beside(&plist_with(body));
        let run = sandbox.run_script(&script, &[], &["drive", "drag", "1", "5"]);
        assert_eq!(run.code(), Some(0), "{body:?}: {}", run.stderr());
        assert!(run.stderr().contains("no LSEnvironment in"), "{body:?}");
    }
    assert_eq!(environment_of(&sandbox, 1), "unset unset");
    assert_eq!(environment_of(&sandbox, 2), "unset unset");
}

#[test]
fn a_plist_the_script_cannot_read_stops_it_before_anything_starts() {
    let sandbox = drive_sandbox("bad-plist");
    let cases = [
        // LaunchServices takes strings only.
        "\t<key>LSEnvironment</key>\n\t<dict>\n\t\t<key>A</key>\n\t\t<integer>1</integer>\n\t</dict>\n",
        // A key without a value.
        "\t<key>LSEnvironment</key>\n\t<dict>\n\t\t<key>A</key>\n\t\t<key>B</key>\n\t\t<string>1</string>\n\t</dict>\n",
        // Not a name that `export` takes.
        "\t<key>LSEnvironment</key>\n\t<dict>\n\t\t<key>NOT A NAME</key>\n\t\t<string>1</string>\n\t</dict>\n",
        "\t<key>LSEnvironment</key>\n\t<dict>\n\t\t<key>1A</key>\n\t\t<string>1</string>\n\t</dict>\n",
    ];
    for body in cases {
        let script = sandbox.script_beside(&plist_with(body));
        let run = sandbox.run_script(&script, &[], &["drive", "drag", "1", "5"]);
        assert_eq!(run.code(), Some(1), "{body:?}: {}", run.stdout());
        let stderr = run.stderr();
        assert!(
            stderr.contains("cannot read the LSEnvironment") || stderr.contains("not a name"),
            "{body:?}: {stderr}"
        );
    }
    // A missing plist stops it too.
    let script = sandbox.script_beside("");
    fs::remove_file(script.with_file_name("Info.plist")).expect("removed");
    let run = sandbox.run_script(&script, &[], &["drive", "drag", "1", "5"]);
    assert_eq!(run.code(), Some(1), "{}", run.stdout());
    assert!(
        run.stderr().contains("no ") && run.stderr().contains("Info.plist"),
        "{}",
        run.stderr()
    );
    // The driver never started.
    assert!(!sandbox.measure().join("drive/release/count").exists());
}

/// The stand-in driver of a build that counts the frames it did not see: each run's line holds `unseen=`
/// and the number on the same line of the `unseen` file beside it, as `src/drive.rs`'s summary does, and
/// `frames=` with the number on the same line of the `frames` file.
const UNSEEN_DRIVER: &str = r#"
here=$(dirname "$0")
n=$(cat "$here/count" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" >"$here/count"
echo "scenario=$2 finished=true frames=$(sed -n "${n}p" "$here/frames") unseen=$(sed -n "${n}p" "$here/unseen")"
"#;

#[test]
fn a_run_whose_window_was_covered_is_left_out_of_the_median_and_named_with_why() {
    // The driver's own `unseen` is in every run's line. The runs' frames are 10, 100 and 40, so that the
    // median of the runs that held (what the script prints) differs from the median of all three (40).
    // `median` is the frames' median in the median line: of the runs that held.
    let cases: [(&[&str], &[usize], Option<&str>); 5] = [
        (&["0", "4", "0"], &[2], Some("25")),
        (&["7", "0", "2"], &[1, 3], Some("100")),
        (&["0", "0", "0"], &[], Some("40")),
        // `unseen=-` is not a count: no run to leave out.
        (&["-", "-", "-"], &[], Some("40")),
        // Every run spoiled: no median at all.
        (&["3", "1", "9"], &[1, 2, 3], None),
    ];
    for (counts, left_out, median) in cases {
        let sandbox = Sandbox::new("unseen");
        sandbox.nothing_running();
        sandbox.fake("top", TOP);
        sandbox.executable(
            &sandbox.measure().join("drive/release/arrakis"),
            UNSEEN_DRIVER,
        );
        fs::write(
            sandbox.measure().join("drive/release/unseen"),
            counts.join("\n"),
        )
        .expect("the counts");
        fs::write(
            sandbox.measure().join("drive/release/frames"),
            "10\n100\n40\n",
        )
        .expect("the frames");
        let run = sandbox.run(&["drive", "drag", "3", "5"]);
        assert_eq!(run.code(), Some(0), "{counts:?}: {}", run.stderr());
        let stdout = run.stdout();
        let lines: Vec<&str> = stdout.lines().collect();
        // Three runs, a line for each run left out, and the median (or what stands in its place).
        assert_eq!(lines.len(), 4 + left_out.len(), "{counts:?}: {stdout}");
        // Each run's line carries its count, and every run is printed, spoiled or not.
        for (index, count) in counts.iter().enumerate() {
            assert!(
                lines[index].contains(&format!(" unseen={count} ")),
                "{counts:?}: {}",
                lines[index]
            );
        }
        // Each run left out is named, in order, with its count and why, on stdout and before the median.
        for (line, run_number) in lines[3..].iter().zip(left_out) {
            let count = counts[run_number - 1];
            assert!(
                line.starts_with(&format!("SPOILED run {run_number} (unseen={count}: "))
                    && line.contains("the window was covered or minimized")
                    && line.ends_with("left out of the median"),
                "{counts:?}: {line}"
            );
        }
        let last = lines[lines.len() - 1];
        match median {
            Some(frames) => assert!(
                last.starts_with(&format!("median: frames={frames} ")),
                "{counts:?}: {last}"
            ),
            None => assert_eq!(
                last, "every run was spoiled, so there is no median",
                "{counts:?}"
            ),
        }
        assert!(
            !run.stderr().contains("SPOILED"),
            "{counts:?}: {}",
            run.stderr()
        );
    }
}

/// The stand-in driver of a build that flags a drive out of its time: each run's line holds
/// `spoiled=` with the run's line of the `spoiled` file beside it, `frames=` with its line of the
/// `frames` file and `unseen=` with its line of the `unseen` file (0 where there is none). The
/// driver writes `spoiled` third, after `scenario` and `finished`; with a `spoiled-after-frames`
/// file beside it, this one writes it after `frames`, to show that the script reads it by its key.
const SPOILED_DRIVER: &str = r#"
here=$(dirname "$0")
n=$(cat "$here/count" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" >"$here/count"
echo "$@" >"$here/args-$n"
spoiled=$(sed -n "${n}p" "$here/spoiled")
frames=$(sed -n "${n}p" "$here/frames")
unseen=0
if [ -f "$here/unseen" ]; then unseen=$(sed -n "${n}p" "$here/unseen"); fi
if [ -f "$here/spoiled-after-frames" ]; then
    echo "scenario=$2 finished=true frames=$frames spoiled=$spoiled unseen=$unseen"
else
    echo "scenario=$2 finished=true spoiled=$spoiled frames=$frames unseen=$unseen"
fi
"#;

/// Where `SPOILED_DRIVER` puts the `spoiled` pair in its line.
#[derive(Clone, Copy, Debug)]
enum Order {
    /// Third, as the driver writes it.
    AsTheDriverWrites,
    /// After `frames`, with `unseen` after it.
    AfterFrames,
}

/// A sandbox whose demo build is `SPOILED_DRIVER`, printing these runs' `spoiled` pairs and
/// `frames`. The `unseen` of each run is 0 unless the test writes the file.
fn spoiled_sandbox(name: &str, order: Order, spoiled: &[&str], frames: &[&str]) -> Sandbox {
    let sandbox = Sandbox::new(name);
    sandbox.nothing_running();
    sandbox.fake("top", TOP);
    let driver = sandbox.measure().join("drive/release");
    sandbox.executable(&driver.join("arrakis"), SPOILED_DRIVER);
    fs::write(driver.join("spoiled"), spoiled.join("\n") + "\n").expect("the pairs");
    fs::write(driver.join("frames"), frames.join("\n") + "\n").expect("the frames");
    if matches!(order, Order::AfterFrames) {
        fs::write(driver.join("spoiled-after-frames"), "").expect("the marker");
    }
    sandbox
}

/// A 5 s `drag` that the machine stopped for 17 minutes (bound 5 to 7 s).
const STOPPED: &str = "elapsed_s:1020.000,bound_s:5.000..7.000";
/// A 5 s `drag` that took 7.4 s.
const SLOW: &str = "elapsed_s:7.400,bound_s:5.000..7.000";

#[test]
fn a_run_that_the_machine_slowed_is_left_out_of_the_median_and_named_with_why() {
    // Three runs of a 5 s `drag`, whose frames are 10, 100 and 40, so that the median of the runs
    // that held (what the script prints) differs from the median of all three (40).
    let cases: [([&str; 3], &[usize], Option<&str>); 4] = [
        (["-", STOPPED, "-"], &[2], Some("25")),
        ([STOPPED, "-", SLOW], &[1, 3], Some("100")),
        // Every run spoiled: no median at all.
        ([STOPPED, SLOW, STOPPED], &[1, 2, 3], None),
        // None spoiled: all three are in the median.
        (["-", "-", "-"], &[], Some("40")),
    ];
    // Where the pair stands in the line makes no difference: the script reads it by its key.
    let cases = [Order::AsTheDriverWrites, Order::AfterFrames]
        .into_iter()
        .flat_map(|order| cases.into_iter().map(move |case| (order, case)));
    for (order, (pairs, left_out, median)) in cases {
        let sandbox = spoiled_sandbox("spoiled", order, &pairs, &["10", "100", "40"]);
        let run = sandbox.run(&["drive", "drag", "3", "5"]);
        let what = format!("{order:?} {pairs:?}");
        assert_eq!(run.code(), Some(0), "{what}: {}", run.stderr());
        let stdout = run.stdout();
        let lines: Vec<&str> = stdout.lines().collect();
        // Three runs, a line for each run left out, and the median (or what stands in its place).
        assert_eq!(lines.len(), 4 + left_out.len(), "{what}: {stdout}");
        // Every run is printed with its pair, spoiled or not.
        for (index, pair) in pairs.iter().enumerate() {
            assert!(
                lines[index].starts_with(&format!("run {}: ", index + 1))
                    && lines[index].contains(&format!(" spoiled={pair} ")),
                "{what}: {}",
                lines[index]
            );
        }
        // Each run left out is named, in order, with its pair and why, on stdout and before the
        // median.
        for (line, run_number) in lines[3..].iter().zip(left_out) {
            let pair = pairs[run_number - 1];
            assert!(
                line.starts_with(&format!("SPOILED run {run_number} (spoiled={pair}: "))
                    && line.contains("out of its time")
                    && line.ends_with("left out of the median"),
                "{what}: {line}"
            );
        }
        let last = lines[lines.len() - 1];
        match median {
            Some(frames) => {
                assert!(
                    last.starts_with(&format!("median: frames={frames} ")),
                    "{what}: {last}"
                );
                // The median line has no `spoiled` of its own: its key is skipped, as `scenario`'s.
                assert!(!last.contains("spoiled="), "{what}: {last}");
            }
            None => assert_eq!(
                last, "every run was spoiled, so there is no median",
                "{what}"
            ),
        }
        assert!(
            !run.stderr().contains("SPOILED"),
            "{what}: {}",
            run.stderr()
        );
    }
}

/// A 30 s `open-close` of 10 cycles whose eighth cycle broke off: the window did not close. It
/// ended at 31.7 s, inside its 30 to 36.5 s, so only the cycle spoils it.
const BROKE_OFF: &str = "cycles:7/10";

#[test]
fn a_run_that_broke_off_a_cycle_is_left_out_of_the_median_and_named_with_why() {
    for order in [Order::AsTheDriverWrites, Order::AfterFrames] {
        let pairs = ["-", BROKE_OFF, "-"];
        let sandbox = spoiled_sandbox("broke-off", order, &pairs, &["10", "100", "40"]);
        let run = sandbox.run(&["drive", "open-close", "3", "30"]);
        let what = format!("{order:?}");
        assert_eq!(run.code(), Some(0), "{what}: {}", run.stderr());
        let stdout = run.stdout();
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(
            lines.len(),
            5,
            "three runs, one left out, a median: {what}: {stdout}"
        );
        // Every run is printed with its pair, the broken one too.
        for (index, pair) in pairs.iter().enumerate() {
            assert!(
                lines[index].starts_with(&format!("run {}: ", index + 1))
                    && lines[index].contains(&format!(" spoiled={pair} ")),
                "{what}: {}",
                lines[index]
            );
        }
        // The run left out is named with its pair, and the words are true of this cause as of the
        // time's: the cycle, not the clock.
        let named = lines[3];
        assert!(
            named.starts_with(&format!("SPOILED run 2 (spoiled={BROKE_OFF}: "))
                && named.contains("a cycle broke off")
                && named.contains("fewer windows than asked")
                && named.ends_with("left out of the median"),
            "{what}: {named}"
        );
        // The median is over runs 1 and 3: frames 10 and 40.
        assert!(
            lines[4].starts_with("median: frames=25 "),
            "{what}: {}",
            lines[4]
        );
        assert!(!lines[4].contains("spoiled="), "{what}: {}", lines[4]);
        assert!(
            !run.stderr().contains("SPOILED"),
            "{what}: {}",
            run.stderr()
        );
    }
}

#[test]
fn a_line_with_no_spoiled_key_counts_as_in_its_bound() {
    // A driver built before the key, as the arms that a comparison reads may be: its runs are in
    // the median, whatever their frames, and nothing is named.
    let sandbox = Sandbox::new("no-spoiled-key");
    sandbox.nothing_running();
    sandbox.fake("top", TOP);
    let driver = sandbox.measure().join("drive/release");
    sandbox.executable(&driver.join("arrakis"), UNSEEN_DRIVER);
    fs::write(driver.join("unseen"), "0\n0\n0\n").expect("the counts");
    fs::write(driver.join("frames"), "10\n100\n40\n").expect("the frames");
    let run = sandbox.run(&["drive", "drag", "3", "5"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let stdout = run.stdout();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 4, "three runs and a median: {stdout}");
    assert!(
        lines.iter().all(|line| !line.contains("spoiled")),
        "{stdout}"
    );
    assert!(lines[3].starts_with("median: frames=40 "), "{}", lines[3]);
    assert!(!stdout.contains("SPOILED"), "{stdout}");
    assert!(!run.stderr().contains("SPOILED"), "{}", run.stderr());
}

#[test]
fn a_run_that_was_covered_and_slowed_is_named_for_each_and_left_out_once() {
    let sandbox = spoiled_sandbox(
        "covered-and-spoiled",
        Order::AsTheDriverWrites,
        &["-", STOPPED, "-"],
        &["10", "100", "40"],
    );
    fs::write(sandbox.measure().join("drive/release/unseen"), "0\n4\n0\n").expect("the counts");
    let run = sandbox.run(&["drive", "drag", "3", "5"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let stdout = run.stdout();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        6,
        "three runs, two causes and a median: {stdout}"
    );
    // Both causes, one line each, in the order the script checks them.
    assert!(
        lines[3].starts_with("SPOILED run 2 (unseen=4: ")
            && lines[3].contains("the window was covered or minimized")
            && lines[3].ends_with("left out of the median"),
        "{}",
        lines[3]
    );
    assert!(
        lines[4].starts_with(&format!("SPOILED run 2 (spoiled={STOPPED}: "))
            && lines[4].contains("out of its time")
            && lines[4].ends_with("left out of the median"),
        "{}",
        lines[4]
    );
    // It is left out once: the median is over runs 1 and 3, and the file of the runs that held has
    // their two lines.
    assert!(lines[5].starts_with("median: frames=25 "), "{}", lines[5]);
    let held = fs::read_to_string(sandbox.measure().join("drag-held.txt")).expect("the held runs");
    assert_eq!(held.lines().count(), 2, "{held}");
    assert!(!held.contains(" frames=100 "), "{held}");
}

/// The `start` is not driven for a number of seconds, and its deadline quit counts as spoiled like
/// any run out of its bound. It runs under `/usr/bin/time -l`, which only macOS has.
#[cfg(target_os = "macos")]
#[test]
fn a_start_is_read_for_spoiled_the_same_way_and_is_driven_with_no_seconds() {
    // 30.1 s of a 30 s `start`: its deadline quit it.
    const QUIT: &str = "elapsed_s:30.100,bound_s:..30.000";
    let sandbox = spoiled_sandbox(
        "start-spoiled",
        Order::AsTheDriverWrites,
        &["-", QUIT, "-"],
        &["2", "9", "4"],
    );
    let run = sandbox.run(&["drive", "start", "3"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    // With no `--demo-seconds`, so that the driver's 30 s default is the start's deadline.
    for number in 1..=3 {
        let args = fs::read_to_string(
            sandbox
                .measure()
                .join("drive/release")
                .join(format!("args-{number}")),
        )
        .expect("the driver's arguments");
        let report = sandbox.measure().join(format!("start-{number}.json"));
        assert_eq!(
            args.trim(),
            format!("--demo-drive start --demo-report {}", report.display())
        );
    }
    let stdout = run.stdout();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        5,
        "three runs, one left out and a median: {stdout}"
    );
    for (index, pair) in ["-", QUIT, "-"].iter().enumerate() {
        assert!(
            lines[index].starts_with(&format!("run {}: scenario=start ", index + 1))
                && lines[index].contains(&format!(" spoiled={pair} ")),
            "{}",
            lines[index]
        );
    }
    assert!(
        lines[3].starts_with(&format!("SPOILED run 2 (spoiled={QUIT}: "))
            && lines[3].ends_with("left out of the median"),
        "{}",
        lines[3]
    );
    // The median over runs 1 and 3: frames 2 and 4.
    assert!(lines[4].starts_with("median: frames=3 "), "{}", lines[4]);
    assert!(!lines[4].contains("spoiled="), "{}", lines[4]);
}

/// What the script hands the driver as its scenario is the flag's spelling of one the driver
/// knows. `start`, which runs under macOS's `/usr/bin/time -l`, is held by the test above.
#[cfg(feature = "demo")]
#[test]
fn every_scenario_the_script_hands_the_driver_is_one_it_knows() {
    for scenario in SCENARIOS
        .into_iter()
        .filter(|scenario| *scenario != "start")
    {
        let sandbox = Sandbox::new(&format!("known-{scenario}"));
        sandbox.nothing_running();
        sandbox.fake("top", TOP);
        sandbox.executable(&sandbox.measure().join("drive/release/arrakis"), DRIVER);
        let run = sandbox.run(&["drive", scenario, "1"]);
        assert_eq!(run.code(), Some(0), "{scenario}: {}", run.stderr());
        let args = fs::read_to_string(sandbox.measure().join("drive/release/args-1"))
            .expect("the driver's arguments");
        let words: Vec<&str> = args.split_whitespace().collect();
        let handed = words
            .iter()
            .position(|word| *word == "--demo-drive")
            .map(|at| words[at + 1])
            .expect("a --demo-drive flag with its scenario");
        assert_eq!(handed, scenario);
        assert!(
            Scenario::from_str(handed, false).is_ok(),
            "{handed} is no scenario of the driver"
        );
    }
}
