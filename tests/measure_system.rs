//! `scripts/measure/energy-system.py`'s arms, against stand-ins: which builds it runs and reads, and where it
//! finds them. Nothing real runs here: no drive, no app, no `powermetrics`, no `sudo`, and the clock is a
//! stand-in that moves only when the script sleeps or a drive "runs". What these tests read is the plumbing
//! of the arms: the `final` arm, the roots the environment points at, and the verdict set against `cap`.
#![cfg(target_os = "macos")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const SYSTEM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/scripts/measure/energy-system.py"
);

/// A new folder with a `tmp` for `TMPDIR`, removed when it goes out of scope.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("arrakis-system-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("tmp")).expect("a folder");
        Self { root }
    }

    fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// A stand-in drive build under `root`, where the script looks for one.
    fn build(&self, root: &Path) {
        let binary = root.join("arrakis-measure/drive/release/arrakis");
        fs::create_dir_all(binary.parent().expect("a folder")).expect("a folder");
        fs::write(&binary, "a drive that never runs").expect("a binary");
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

    fn say(&self) -> String {
        format!("{}{}", self.stdout(), self.stderr())
    }
}

/// Loads energy-system.py as a module, with no bytecode written beside it. Its argument 1 is the script.
const LOAD: &str = r#"
import importlib.util
import json
import os
import sys

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("energy_system", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
"#;

fn python(sandbox: &Sandbox, code: &str, env: &[(&str, &str)]) -> Run {
    let output = Command::new("python3")
        .args(["-I", "-c", &format!("{LOAD}{code}")])
        .arg(SYSTEM)
        .env("TMPDIR", sandbox.tmp())
        .envs(env.iter().copied())
        .output()
        .expect("python3 runs");
    Run(output)
}

fn json_of(run: &Run) -> Value {
    let last = run.stdout();
    let last = last.lines().last().unwrap_or_default().to_owned();
    serde_json::from_str(&last).unwrap_or_else(|err| panic!("{err}: {}", run.say()))
}

#[test]
fn importing_energy_system_runs_nothing_and_without_arguments_it_says_its_usage() {
    let sandbox = Sandbox::new("import");
    let run = python(&sandbox, "print(json.dumps(callable(module.main)))", &[]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert_eq!(run.stdout().trim(), "true");
    for args in [&[][..], &["run"][..], &["run", "a", "3", "cap", "x"][..]] {
        let output = Command::new("python3")
            .arg(SYSTEM)
            .args(args)
            .env("TMPDIR", sandbox.tmp())
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .expect("python3 runs");
        let said = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {said}");
        assert!(
            said.starts_with("The whole machine's power in a drag"),
            "{args:?}: {said}"
        );
    }
}

#[test]
fn the_roots_are_the_night_before_and_the_final_build_and_the_environment_points_any_at_another() {
    let sandbox = Sandbox::new("roots");
    let code = r#"
tmp = os.environ["TMPDIR"]
plain = module.arm_roots({"TMPDIR": tmp}, set())
set_up = module.arm_roots({
    "TMPDIR": tmp,
    "ENERGY_SYSTEM_ROOT_FINAL": "/arms/final-abc1234",
    "ENERGY_SYSTEM_ROOT_METAL": "/arms/metal",
    "ENERGY_SYSTEM_ROOT_": "/nothing",
    "ENERGY_DRAG_ROOT_CAP": "/not/this/script's",
}, {"final"})
relative = {"TMPDIR": tmp, "ENERGY_SYSTEM_ROOT_FINAL": "relative/final"}
refused = {}
for used in ({"cap", "final"}, {"final"}):
    try:
        module.arm_roots(relative, used)
        refused[",".join(sorted(used))] = None
    except SystemExit as err:
        refused[",".join(sorted(used))] = str(err)
ignored = module.arm_roots(relative, {"cap", "metal"})
print(json.dumps({
    "tmp": tmp,
    "plain": {k: str(v) for k, v in plain.items()},
    "set_up": {k: str(v) for k, v in set_up.items()},
    "refused": refused,
    "ignored_final": str(ignored["final"]),
}))
"#;
    let run = python(&sandbox, code, &[]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let got = json_of(&run);
    let tmp = got["tmp"].as_str().expect("tmp");
    // The `cap` and `metal` roots stay where they were, and `final` is where energy-drag.py's is.
    assert_eq!(got["plain"]["cap"], format!("{tmp}/t1/cap"));
    assert_eq!(got["plain"]["metal"], format!("{tmp}/t1/metal"));
    assert_eq!(got["plain"]["final"], format!("{tmp}/arms/final"));
    assert_eq!(got["plain"].as_object().expect("roots").len(), 3);
    // The variable sets a root, and a bare prefix or another script's variable is none.
    assert_eq!(got["set_up"]["final"], "/arms/final-abc1234");
    assert_eq!(got["set_up"]["metal"], "/arms/metal");
    assert_eq!(got["set_up"]["cap"], format!("{tmp}/t1/cap"));
    assert_eq!(got["set_up"].as_object().expect("roots").len(), 3);
    // A relative root ends the run when its root is one the arms use, and is ignored otherwise.
    for used in ["cap,final", "final"] {
        assert!(
            got["refused"][used].as_str().is_some_and(
                |said| said.contains("ENERGY_SYSTEM_ROOT_FINAL must be an absolute path")
            ),
            "{used}: {got}"
        );
    }
    assert_eq!(got["ignored_final"], format!("{tmp}/arms/final"));
}

/// Runs `main` with a clock that moves only when the script sleeps, a `pgrep` that sees no app, and a drive
/// that notes its command and environment, writes a healthy report and "runs" 30.5 s. `ran` has each drive.
const FAKE_RUN: &str = r#"
import subprocess
import time

clock = [1000.0]
time.time = lambda: clock[0]
time.sleep = lambda seconds: clock.__setitem__(0, clock[0] + seconds)
ran = []

class Done:
    returncode = 0

def fake_run(cmd, **kwargs):
    env = kwargs["env"]
    ran.append({"binary": cmd[0], "flag": env.get("MallocSpaceEfficient"),
                "surfaces": env.get("SOFT_SURFACES"), "home": env["HOME"]})
    report = cmd[cmd.index("--demo-report") + 1]
    with open(report, "w") as f:
        f.write(json.dumps({"finished": True, "elapsed_s": 30.6, "unseen_frames": 0}))
    clock[0] += 30.5  # exact in binary, so that no sum of the clock falls a rounding short of 30 s
    return Done()

subprocess.run = fake_run
module.pgrep_arrakis = lambda: []
"#;

fn rows_of(path: &Path) -> Vec<Vec<String>> {
    fs::read_to_string(path)
        .expect("the runs file")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.split('\t').map(str::to_owned).collect())
        .collect()
}

#[test]
fn the_final_build_is_an_arm_that_runs_from_the_root_the_session_names() {
    let sandbox = Sandbox::new("run-final");
    let (cap, last) = (
        sandbox.tmp().join("t1/cap"),
        sandbox.root.join("elsewhere/arm-abc1234"),
    );
    sandbox.build(&cap);
    sandbox.build(&last);
    let runs = sandbox.root.join("runs.tsv");
    let code = format!(
        "{FAKE_RUN}\nmodule.main(['run', {runs:?}, '2', 'cap,final,idle'])\nprint(json.dumps(ran))\n",
        runs = runs.display().to_string()
    );
    let run = python(
        &sandbox,
        &code,
        &[("ENERGY_SYSTEM_ROOT_FINAL", &last.to_string_lossy())],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let ran = json_of(&run);
    let binaries: Vec<&str> = ran
        .as_array()
        .expect("drives")
        .iter()
        .map(|drive| drive["binary"].as_str().expect("a binary"))
        .collect();
    // In turns, `cap` then `final` (and `idle`, which runs no drive), each from its own root.
    let drive = |root: &Path| {
        root.join("arrakis-measure/drive/release/arrakis")
            .display()
            .to_string()
    };
    assert_eq!(
        binaries,
        [drive(&cap), drive(&last), drive(&cap), drive(&last)]
    );
    // The final build runs with the flag, as `cap` does, and with no second surface.
    for drive in ran.as_array().expect("drives") {
        assert_eq!(drive["flag"], "1", "{drive}");
        assert!(drive["surfaces"].is_null(), "{drive}");
    }
    assert!(
        ran[1]["home"]
            .as_str()
            .is_some_and(|home| home.starts_with(&last.to_string_lossy().into_owned())),
        "{ran}"
    );
    // The file has a line for each run, labeled, in turns, and none is spoiled.
    let rows = rows_of(&runs);
    let labels: Vec<(&str, &str)> = rows
        .iter()
        .map(|row| (row[0].as_str(), row[1].as_str()))
        .collect();
    assert_eq!(
        labels,
        [
            ("cap", "1"),
            ("final", "1"),
            ("idle", "1"),
            ("cap", "2"),
            ("final", "2"),
            ("idle", "2")
        ]
    );
    assert!(rows.iter().all(|row| row[6].is_empty()), "{rows:?}");
}

#[test]
fn without_arms_named_the_runs_are_t1s_and_do_not_need_a_final_build() {
    let sandbox = Sandbox::new("run-default");
    sandbox.build(&sandbox.tmp().join("t1/cap"));
    sandbox.build(&sandbox.tmp().join("t1/metal"));
    // No build at `$TMPDIR/arms/final`: the default arms do not look for one.
    let runs = sandbox.root.join("runs.tsv");
    let code = format!(
        "{FAKE_RUN}\nmodule.main(['run', {runs:?}, '1'])\nprint(json.dumps(ran))\n",
        runs = runs.display().to_string()
    );
    let run = python(&sandbox, &code, &[]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let ran = json_of(&run);
    let drives = ran.as_array().expect("drives");
    // `metal-2` is `metal` with two surfaces.
    assert_eq!(
        drives
            .iter()
            .map(|drive| drive["surfaces"].as_str())
            .collect::<Vec<_>>(),
        [None, None, Some("2")]
    );
    let labels: Vec<String> = rows_of(&runs)
        .into_iter()
        .map(|row| row[0].clone())
        .collect();
    assert_eq!(labels, ["cap", "metal", "metal-2", "idle"]);
}

#[test]
fn an_arm_that_is_not_one_or_has_no_build_ends_the_run_before_any_drive_starts() {
    let sandbox = Sandbox::new("run-refused");
    sandbox.build(&sandbox.tmp().join("t1/cap"));
    let runs = sandbox.root.join("runs.tsv");
    for (arms, said) in [
        ("cap,nothing", "no arm 'nothing'"),
        ("cap,final", "no drive build of final in"),
    ] {
        let code = format!(
            "{FAKE_RUN}\ntry:\n    module.main(['run', {runs:?}, '1', {arms:?}])\nfinally:\n    print(json.dumps(ran))\n",
            runs = runs.display().to_string()
        );
        let run = python(&sandbox, &code, &[]);
        assert_eq!(run.code(), Some(1), "{arms}: {}", run.say());
        assert!(run.stderr().contains(said), "{arms}: {}", run.say());
        assert_eq!(json_of(&run), serde_json::json!([]), "{arms}: a drive ran");
        assert!(!runs.exists(), "{arms}: a runs file was written");
    }
    // A relative root for the final build ends it too, before the build is looked for.
    let code = format!(
        "{FAKE_RUN}\nmodule.main(['run', {runs:?}, '1', 'final'])\n",
        runs = runs.display().to_string()
    );
    let run = python(
        &sandbox,
        &code,
        &[("ENERGY_SYSTEM_ROOT_FINAL", "relative/final")],
    );
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.stderr()
            .contains("ENERGY_SYSTEM_ROOT_FINAL must be an absolute path"),
        "{}",
        run.say()
    );
}

/// Writes the runs file of `labels`, three turns each and none spoiled, and gives the code that has `read`
/// read it with a stand-in `measure`: an arm's power is the `power` of its label plus its turn, so its range
/// is that plus 1 to 3.
fn read_code(runs: &Path, labels: &[&str], power: &str) -> String {
    let mut lines = String::from("# label\tturn\tstart\tend\telapsed_s\tseconds\tspoiled\n");
    for label in labels {
        for turn in 1..=3 {
            lines.push_str(&format!("{label}\t{turn}\t1000.0\t1030.0\t30.6\t30\t\n"));
        }
    }
    fs::write(runs, lines).expect("the runs file");
    format!(
        r#"
POWER = {power}
module.load_samples = lambda path: []

def fake_measure(samples, run):
    value = POWER[run["label"]] + run["turn"]
    m = {{"n": 20, "cpu": value, "gpu": 0.0, "ane": None, "combined": None, "cpu+gpu": value, "all": value}}
    for name in ("windowserver", "arrakis"):
        m[name + "_seen"] = 20
        for field in module.ROW_FIELDS:
            m[name + "." + field] = 1.0
    return m

module.measure = fake_measure
module.main(["read", "unused.plist", {runs:?}])
"#,
        runs = runs.display().to_string()
    )
}

#[test]
fn read_sets_the_final_build_against_cap_with_the_same_rule_as_metal() {
    let sandbox = Sandbox::new("read-final");
    let runs = sandbox.root.join("runs.tsv");
    // `cap` reads 101 to 103 mW, `final` 91 to 93: below, the ranges apart. `metal` is absent altogether.
    let code = read_code(
        &runs,
        &["cap", "final", "idle"],
        r#"{"cap": 100.0, "final": 90.0, "idle": 10.0}"#,
    );
    let run = python(&sandbox, &code, &[]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let said = run.stdout();
    assert!(
        said.contains("  final    n=3  runs 91, 92, 93  range 91-93  median 92  net 80"),
        "{said}"
    );
    assert!(
        said.contains("  final    (the final build): go: below glow's, the ranges apart"),
        "{said}"
    );
    assert!(!said.contains("NOTE: fewer than 3"), "{said}");
    assert!(!said.contains("metal"), "{said}");

    // Above cap, the ranges apart: the same words as for `metal`, and both are set against it together.
    let code = read_code(
        &runs,
        &["cap", "metal", "final", "idle"],
        r#"{"cap": 100.0, "metal": 150.0, "final": 100.0, "idle": 10.0}"#,
    );
    let run = python(&sandbox, &code, &[]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let said = run.stdout();
    assert!(
        said.contains(
            "  metal    (the shipped shape on one surface): STOP: above glow's, the ranges apart"
        ),
        "{said}"
    );
    assert!(
        said.contains(
            "  final    (the final build): go: the ranges overlap, so no measurable loss"
        ),
        "{said}"
    );
}

#[test]
fn read_needs_cap_idle_and_one_arm_to_set_against_them() {
    let sandbox = Sandbox::new("read-missing");
    let runs = sandbox.root.join("runs.tsv");
    for (labels, said) in [
        (
            vec!["cap", "idle"],
            "no good run of metal, metal-2, final to set against cap",
        ),
        (vec!["final", "idle"], "no good run of cap"),
        (vec!["cap", "final"], "no good run of idle"),
    ] {
        let code = read_code(
            &runs,
            &labels,
            r#"{"cap": 100.0, "final": 90.0, "idle": 10.0}"#,
        );
        let run = python(&sandbox, &code, &[]);
        assert_eq!(run.code(), Some(1), "{labels:?}: {}", run.say());
        assert!(run.stderr().contains(said), "{labels:?}: {}", run.say());
    }
}
