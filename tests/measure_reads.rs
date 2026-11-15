//! `scripts/measure/energy-drag.py` and `scripts/measure/real-cycles.sh`, against stand-ins. Nothing real
//! runs here: no drive, no app, no display, no `footprint` of a real process, and the waits are stand-ins
//! that return at once. What these tests read is each script's flow: what it records beside a run, in what
//! order, and what its result line still says.
#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const DRAG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/scripts/measure/energy-drag.py"
);
const CYCLES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/scripts/measure/real-cycles.sh"
);

/// A new folder with a `bin` of stand-ins, a `tmp` for `TMPDIR` and a `root` for an arm, removed when it goes
/// out of scope.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("arrakis-reads-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for folder in ["bin", "tmp", "root"] {
            fs::create_dir_all(root.join(folder)).expect("a folder");
        }
        Self { root }
    }

    fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }

    fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// The arm's root.
    fn arm(&self) -> PathBuf {
        self.root.join("root")
    }

    fn write_executable(&self, path: &Path, body: &str) {
        fs::create_dir_all(path.parent().expect("a folder")).expect("a folder");
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("a stand-in");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable");
    }

    /// A stand-in named `name` on the `PATH` of the scripts.
    fn stand_in(&self, name: &str, body: &str) {
        self.write_executable(&self.bin().join(name), body);
    }

    /// What the stand-ins noted in `bin/events`, in order.
    fn events(&self) -> Vec<String> {
        fs::read_to_string(self.bin().join("events"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn path(&self) -> String {
        format!(
            "{}:{}",
            self.bin().display(),
            std::env::var("PATH").unwrap_or_default()
        )
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

// ---- energy-drag.py ----

/// Loads energy-drag.py as a module, with no bytecode written beside it. Its argument 1 is the script.
const LOAD: &str = r#"
import importlib.util
import json
import os
import sys

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("energy_drag", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
"#;

fn python(sandbox: &Sandbox, code: &str, args: &[&str], env: &[(&str, &str)]) -> Run {
    let output = Command::new("python3")
        .args(["-I", "-c", &format!("{LOAD}{code}")])
        .arg(DRAG)
        .args(args)
        .env("TMPDIR", sandbox.tmp())
        .envs(env.iter().copied())
        .output()
        .expect("python3 runs");
    Run(output)
}

fn json_of(run: &Run) -> Value {
    serde_json::from_str(run.stdout().trim()).unwrap_or_else(|err| panic!("{err}: {}", run.say()))
}

#[test]
fn importing_energy_drag_runs_nothing_and_without_arguments_it_says_its_usage() {
    let sandbox = Sandbox::new("drag-import");
    let run = python(
        &sandbox,
        "print(json.dumps(callable(module.main)))",
        &[],
        &[],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert_eq!(run.stdout().trim(), "true");
    for args in [&[][..], &["out.json"][..], &["a", "b", "c"][..]] {
        let output = Command::new("python3")
            .arg(DRAG)
            .args(args)
            .env("TMPDIR", sandbox.tmp())
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .expect("python3 runs");
        let said = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {said}");
        assert!(
            said.starts_with("A drag's CPU energy and core placement"),
            "{args:?}: {said}"
        );
    }
}

#[test]
fn an_arm_can_be_pointed_at_the_final_build_by_a_path_the_session_sets() {
    let sandbox = Sandbox::new("drag-roots");
    let code = r#"
tmp = os.environ["TMPDIR"]
plain = module.arm_roots({"TMPDIR": tmp})
set_up = module.arm_roots({
    "TMPDIR": tmp,
    "ENERGY_DRAG_ROOT_FINAL": "/arms/final-abc1234",
    "ENERGY_DRAG_ROOT_BISECT_MID": "/arms/mid",
    "ENERGY_DRAG_ROOT_RELEASE": "/arms/release-abc",
    "ENERGY_DRAG_ROOT_": "/nothing",
})
relative = {"TMPDIR": tmp, "ENERGY_DRAG_ROOT_FINAL": "relative/final"}
# A relative root ends the run for an arm in the order, with or without its +flag.
refused = {}
for order in (["cap", "final"], ["final+flag"]):
    try:
        module.arm_roots(relative, order)
        refused[",".join(order)] = None
    except SystemExit as err:
        refused[",".join(order)] = str(err)
# For an arm that is not in the order it is ignored, and the arm keeps its default.
ignored = module.arm_roots(relative, ["cap"])
print(json.dumps({
    "tmp": tmp,
    "plain": {k: str(v) for k, v in plain.items()},
    "set_up": {k: str(v) for k, v in set_up.items()},
    "refused": refused,
    "ignored_final": str(ignored["final"]),
}))
"#;
    let run = python(&sandbox, code, &[], &[]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let got = json_of(&run);
    let tmp = got["tmp"].as_str().expect("tmp");
    // The night's arms stay where they were, and "final" has a default of its own.
    assert_eq!(got["plain"]["cap"], format!("{tmp}/m-drive"));
    assert_eq!(got["plain"]["t1-metal"], format!("{tmp}/t1/metal"));
    assert_eq!(got["plain"]["release"], format!("{tmp}/arms/release"));
    assert_eq!(got["plain"]["final"], format!("{tmp}/arms/final"));
    // The variable sets an arm's root, adds an arm (underscores are hyphens), and a bare prefix is no arm.
    assert_eq!(got["set_up"]["final"], "/arms/final-abc1234");
    assert_eq!(got["set_up"]["bisect-mid"], "/arms/mid");
    assert_eq!(got["set_up"]["release"], "/arms/release-abc");
    assert_eq!(got["set_up"]["cap"], format!("{tmp}/m-drive"));
    assert_eq!(got["set_up"].as_object().expect("arms").len(), 9);
    // A relative root ends the run for an arm in the order, flag or not, and is ignored for another.
    for order in ["cap,final", "final+flag"] {
        assert!(
            got["refused"][order].as_str().is_some_and(
                |said| said.contains("ENERGY_DRAG_ROOT_FINAL must be an absolute path")
            ),
            "{order}: {got}"
        );
    }
    assert_eq!(got["ignored_final"], format!("{tmp}/arms/final"));
}

#[test]
fn a_relative_root_for_an_arm_in_the_order_ends_the_night_before_it_starts() {
    let sandbox = Sandbox::new("drag-relative");
    let drive = sandbox.arm().join("arrakis-measure/drive/release");
    sandbox.write_executable(&drive.join("arrakis"), DRIVE);
    let out = sandbox.root.join("out.json");
    let code = format!(
        "sys.argv = ['energy-drag.py', {out:?}, 'x,y']\nmodule.main()\n",
        out = out.display().to_string()
    );
    // `x` is fine and comes first; `y` is the one with the relative root: nothing runs for either.
    let run = python(
        &sandbox,
        &code,
        &[],
        &[
            ("ENERGY_DRAG_ROOT_X", &sandbox.arm().to_string_lossy()),
            ("ENERGY_DRAG_ROOT_Y", "relative/y"),
        ],
    );
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.stderr()
            .contains("ENERGY_DRAG_ROOT_Y must be an absolute path"),
        "{}",
        run.say()
    );
    assert!(!out.exists(), "a run was written");
    assert!(!drive.join("started").exists(), "the app was started");
}

const PMSET_AUDIO: &str = "Assertion status system-wide:\\n   PreventUserIdleSystemSleep    1\\nListed by owning process:\\n   pid 427(coreaudiod): [0x1] 00:07:27 PreventUserIdleSystemSleep named: \\\"com.apple.audio\\\"\\n\\tResources: audio-out BuiltInSpeakerDevice \\n";
const PMSET_QUIET: &str = "Assertion status system-wide:\\n   PreventUserIdleSystemSleep    1\\nListed by owning process:\\n   pid 356(powerd): [0x2] 01:51:27 PreventUserIdleSystemSleep named: \\\"Powerd\\\"\\n";

#[test]
fn the_machines_state_has_audio_out_the_load_averages_and_the_top_five_of_the_second_sample() {
    let sandbox = Sandbox::new("drag-state");
    // top's first sample counts CPU time since each process started and is no reading; `top` itself asked
    // for one row more than the five and is left out.
    let code = format!(
        r#"
PMSET = {{"audio": "{PMSET_AUDIO}", "quiet": "{PMSET_QUIET}"}}
TOP = (
    "Processes: 400 total\n"
    "PID    COMMAND          %CPU\n"
    "1      launchd          99.0\n"
    "2      first_sample     98.0\n"
    "Processes: 400 total\n"
    "PID    COMMAND          %CPU\n"
    "27967  rustc            85.6\n"
    "99023  Google Chrome He 24.2\n"
    "30000  top              9.9\n"
    "415    WindowServer     6.6 \n"
    "427    coreaudiod       5.8 \n"
    "757    com.apple.WebKit 5.1\n"
    "12     sixth            0.1\n"
)
asked = []

def runner(which):
    def run(*command):
        asked.append(list(command))
        return {{"pmset": PMSET[which], "top": TOP}}[command[0]]
    return run

audio = module.machine_state(runner("audio"))
quiet = module.machine_state(runner("quiet"))
unread = module.machine_state(lambda *command: None)
print(json.dumps({{"audio": audio, "quiet": quiet, "unread": unread, "asked": asked[:2],
    "line": module.machine_line("before", audio), "unread_line": module.machine_line("after", unread)}}))
"#
    );
    let run = python(&sandbox, &code, &[], &[]);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let got = json_of(&run);
    assert_eq!(got["audio"]["audio_out"], true);
    assert_eq!(got["quiet"]["audio_out"], false);
    // Three load averages, as `os.getloadavg` gives them.
    let load = got["audio"]["load"].as_array().expect("load");
    assert_eq!(load.len(), 3);
    assert!(load.iter().all(|x| x.as_f64().is_some_and(|x| x >= 0.0)));
    let top: Vec<(i64, &str, f64)> = got["audio"]["top"]
        .as_array()
        .expect("top")
        .iter()
        .map(|p| {
            (
                p["pid"].as_i64().expect("pid"),
                p["command"].as_str().expect("command"),
                p["cpu"].as_f64().expect("cpu"),
            )
        })
        .collect();
    assert_eq!(
        top,
        [
            (27967, "rustc", 85.6),
            (99023, "Google Chrome He", 24.2),
            (415, "WindowServer", 6.6),
            (427, "coreaudiod", 5.8),
            (757, "com.apple.WebKit", 5.1),
        ]
    );
    // What it asked the machine: pmset's assertions, and top's second sample by CPU.
    assert_eq!(
        got["asked"][0],
        serde_json::json!(["pmset", "-g", "assertions"])
    );
    let asked: Vec<&str> = got["asked"][1]
        .as_array()
        .expect("top's arguments")
        .iter()
        .map(|word| word.as_str().expect("a word"))
        .collect();
    assert!(
        asked.starts_with(&["top", "-l", "2"])
            && asked.windows(2).any(|pair| pair == ["-o", "cpu"]),
        "{asked:?}"
    );
    // A call that fails leaves its part null and the rest as it was.
    assert!(got["unread"]["audio_out"].is_null() && got["unread"]["top"].is_null());
    assert_eq!(got["unread"]["load"].as_array().expect("load").len(), 3);
    let line = got["line"].as_str().expect("a line");
    assert!(
        line.starts_with("  machine before: audio-out, load "),
        "{line}"
    );
    assert!(
        line.ends_with("top rustc 86%, Google Chrome He 24%, WindowServer 7%, coreaudiod 6%, com.apple.WebKit 5%"),
        "{line}"
    );
    let unread = got["unread_line"].as_str().expect("a line");
    assert!(
        unread.starts_with("  machine after: audio unread, load ")
            && unread.ends_with(", top unread"),
        "{unread}"
    );
}

/// `top` and `pmset` that print a name cut in the middle of a UTF-8 character: a lone lead byte, which no
/// decoder takes, in the name of the first process and in the line of the audio assertion.
const TOP_CUT: &str = r#"
printf 'PID    COMMAND          %%CPU\n'
printf '27967  caf\303 85.6\n'
printf '415    WindowServer     6.6\n'
"#;
const PMSET_CUT: &str = r#"
printf 'pid 427(core\303): [0x1] PreventUserIdleSystemSleep\n\tResources: audio-out BuiltInSpeakerDevice\n'
"#;

#[test]
fn output_that_is_not_utf8_or_arguments_that_cannot_be_passed_leave_their_part_null_and_end_nothing()
 {
    let sandbox = Sandbox::new("drag-cut");
    sandbox.stand_in("top", TOP_CUT);
    sandbox.stand_in("pmset", PMSET_CUT);
    let code = r#"
state = module.machine_state()
print(json.dumps({
    "state": state,
    "nul": module.sh("pm\0set", "-g"),
    "echo": module.sh("echo", "hi"),
    "missing": module.sh("arrakis-no-such-command"),
    "failing": module.sh("false"),
}))
"#;
    let run = python(
        &sandbox,
        code,
        &[],
        &[("PATH", &sandbox.path()), ("PYTHONUTF8", "1")],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());
    let got = json_of(&run);
    // The bad byte reads as U+FFFD and the rest of the output is read: the name keeps its good part.
    assert_eq!(got["state"]["audio_out"], true);
    let top = got["state"]["top"].as_array().expect("top is read");
    let commands: Vec<&str> = top
        .iter()
        .map(|row| row["command"].as_str().expect("a command"))
        .collect();
    assert_eq!(commands, ["caf\u{fffd}", "WindowServer"]);
    assert_eq!(top[0]["cpu"], 85.6);
    // An argument that cannot be passed (a NUL) is a ValueError, and a command that cannot run or fails
    // is an OSError or a CalledProcessError: each leaves null, and a command that works is read.
    assert!(got["nul"].is_null() && got["missing"].is_null() && got["failing"].is_null());
    assert_eq!(got["echo"], "hi\n");
}

/// The stand-in drive: notes that it started, waits on the `hold` FIFO until the script is waiting for it and
/// writes to it (so it cannot end before then, whatever the machine's speed), writes a healthy report to its
/// `--demo-report`, and notes that it finished.
const DRIVE: &str = r#"
here=$(dirname "$0")
while [ $# -gt 0 ]; do
    case "$1" in --demo-report) report=$2; shift ;; esac
    shift
done
echo started >>"$here/started"
read -r _ <"$here/hold"
printf '{"finished": true, "elapsed_s": 30.6, "unseen_frames": 0, "frames_per_second": 119.5, "frame_cpu_ms": {"p50": 0.4, "p90": 0.6}}' >"$report"
echo finished >>"$here/finished"
"#;

#[test]
fn each_row_keeps_the_machine_before_the_app_starts_and_again_once_the_drive_has_ended() {
    let sandbox = Sandbox::new("drag-rows");
    let drive = sandbox.arm().join("arrakis-measure/drive/release");
    sandbox.write_executable(&drive.join("arrakis"), DRIVE);
    let (started, finished, hold) = (
        drive.join("started"),
        drive.join("finished"),
        drive.join("hold"),
    );
    let out = sandbox.root.join("out.json");
    // The clock moves only when the script sleeps, so a run takes no time; counters and the machine are
    // stand-ins that note when they are asked. The stand-in drive is held on a FIFO until the script waits
    // for it, and `wait` notes itself when it returns: no wait on the clock, whatever the machine's speed.
    let code = format!(
        r#"
import subprocess
import time

started, finished, hold = {started:?}, {finished:?}, {hold:?}
os.mkfifo(hold)
events = []
clock = [0.0]
time.sleep = lambda seconds: clock.__setitem__(0, clock[0] + seconds)
time.monotonic = lambda: clock[0]
time.time = lambda: clock[0]

class Held(subprocess.Popen):
    released = False

    def wait(self, *args, **kwargs):
        if not self.released:
            self.released = True
            with open(hold, "w") as fifo:  # blocks until the stand-in is at its read, then lets it go on
                fifo.write("go\n")
        result = super().wait(*args, **kwargs)
        events.append("wait")
        return result

subprocess.Popen = Held

def fake_counters(pid):
    events.append("counters")
    k = events.count("counters")
    return {{"cpu_ns": k * 2e9, "p_cpu_ns": k * 1e9, "instr": k * 4e9, "p_instr": k * 2e9,
             "cycles": k * 4e9, "energy_nj": k * 5e9}}

def noted(path):
    return len(open(path).read().split()) if os.path.exists(path) else 0

def fake_state():
    events.append("state")
    n = events.count("state")
    return {{"audio_out": n % 2 == 0, "load": [float(n), 0.5, 0.25],
             "top": [{{"pid": n, "command": "proc%d" % n, "cpu": 10.0 * n}}],
             "apps_started": noted(started), "apps_finished": noted(finished)}}

module.counters = fake_counters
module.machine_state = fake_state
sys.argv = ["energy-drag.py", {out:?}, "x+flag,x"]
module.main()
print("EVENTS " + json.dumps(events))
"#,
        started = started.display().to_string(),
        finished = finished.display().to_string(),
        hold = hold.display().to_string(),
        out = out.display().to_string(),
    );
    // A root that is not an absolute path, for an arm that is not in the order, stops nothing.
    let run = python(
        &sandbox,
        &code,
        &[],
        &[
            ("ENERGY_DRAG_ROOT_X", &sandbox.arm().to_string_lossy()),
            ("ENERGY_DRAG_ROOT_LEFTOVER", "relative/leftover"),
        ],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert_eq!(run.stderr(), "");

    // The order of a run: the state, the app, the two reads, the wait for the drive's end, the state again.
    // The second run's follow.
    let stdout = run.stdout();
    let events: Vec<String> = stdout
        .lines()
        .find_map(|line| line.strip_prefix("EVENTS "))
        .map(|json| serde_json::from_str(json).expect("events"))
        .expect("the events");
    assert_eq!(
        events,
        [
            "state", "counters", "counters", "wait", "state", "state", "counters", "counters",
            "wait", "state"
        ]
    );

    // The file keeps both states of each run, and the second was taken once the drive had ended: `top`
    // takes two seconds and loads the machine, so while the drive still records it would be in the row.
    let rows: Value =
        serde_json::from_str(&fs::read_to_string(&out).expect("the file")).expect("JSON");
    let rows = rows.as_array().expect("rows");
    assert_eq!(rows.len(), 2);
    for (index, row) in rows.iter().enumerate() {
        assert!(row["spoiled"].is_null(), "{row}");
        let (before, after) = (&row["machine"]["before"], &row["machine"]["after"]);
        let n = 2 * index as i64;
        // The app had been started and had finished index times when the run began; it started once more
        // in the run, and had finished when the second state was taken.
        assert_eq!(before["apps_started"], index, "{row}");
        assert_eq!(before["apps_finished"], index, "{row}");
        assert_eq!(after["apps_started"], index + 1, "{row}");
        assert_eq!(after["apps_finished"], index + 1, "{row}");
        assert_eq!(before["load"][0], (n + 1) as f64, "{row}");
        assert_eq!(after["load"][0], (n + 2) as f64, "{row}");
        assert_eq!(before["audio_out"], false);
        assert_eq!(after["audio_out"], true);
    }

    // The result line of each run is followed by its two states, and the medians still print: the machine
    // is no measure.
    assert!(
        stdout.contains("  machine before: no audio-out, load 1.00 0.50 0.25, top proc1 10%\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  machine after: audio-out, load 2.00 0.50 0.25, top proc2 20%\n"),
        "{stdout}"
    );
    assert!(stdout.contains("x+flag n=1 of 1 fps=119.500"), "{stdout}");
    assert!(stdout.contains("\nx n=1 of 1 fps=119.500"), "{stdout}");
    assert!(
        !stdout.contains("machine="),
        "the medians took the machine for a measure: {stdout}"
    );
}

// ---- real-cycles.sh ----

/// The stand-in `axtool`: `fp <pid>` prints a MiB figure that rises with each read; `click` is noted.
const AXTOOL: &str = r#"
here=$FAKE_BIN
case "$1" in
fp)
    n=$(cat "$here/fp.count" 2>/dev/null || echo 0)
    n=$((n + 1))
    echo "$n" >"$here/fp.count"
    echo "read $n pid $2" >>"$here/events"
    echo "$n.50"
    ;;
click) echo "click $2 $3" >>"$here/events" ;;
esac
"#;

/// `footprint -p <pid> -f bytes`: notes its call and prints a table that names the call it was.
const FOOTPRINT: &str = r#"
here=$FAKE_BIN
n=$(cat "$here/footprint.count" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" >"$here/footprint.count"
echo "footprint $*" >>"$here/events"
echo "arrakis [$2]: 64-bit    Footprint: 30229320 B (16384 bytes per page)"
echo "  14221312 B  0 B  0 B  14  MALLOC_SMALL   (call $n)"
"#;

/// The stand-in app: notes its arguments, and becomes a `sleep` for `kill -TERM` to end.
const APP: &str = r#"
here=$FAKE_BIN
echo "app $*" >>"$here/events"
exec /bin/sleep 600
"#;

/// `osascript`: one window, a menu-bar item with a box, and nothing to say to a click.
const OSASCRIPT: &str = r#"
case "$*" in
*"count of windows"*) echo 1 ;;
*"position, size"*) echo "10, 0, 30, 24" ;;
esac
"#;

fn cycles_sandbox(name: &str) -> Sandbox {
    let sandbox = Sandbox::new(name);
    sandbox.stand_in("pgrep", "exit 1");
    // The waits return at once: the script's own sleeps are most of its five minutes.
    sandbox.stand_in("sleep", "exit 0");
    sandbox.stand_in("osascript", OSASCRIPT);
    sandbox.stand_in("footprint", FOOTPRINT);
    sandbox.write_executable(&sandbox.tmp().join("axtool"), AXTOOL);
    sandbox.write_executable(
        &sandbox.arm().join("arrakis-measure/real/release/arrakis"),
        APP,
    );
    sandbox
}

fn cycles(sandbox: &Sandbox, args: &[&str]) -> Run {
    let output = Command::new("sh")
        .arg(CYCLES)
        .args(args)
        .env("PATH", sandbox.path())
        .env("TMPDIR", sandbox.tmp())
        .env("FAKE_BIN", sandbox.bin())
        .env_remove("Q")
        .output()
        .expect("sh runs");
    Run(output)
}

const READS: [&str; 8] = [
    "never",
    "open5",
    "open30",
    "open90",
    "close1_2s",
    "close1_60s",
    "close7_2s",
    "close7_60s",
];

#[test]
fn each_read_is_followed_by_the_apps_footprint_in_bytes_in_a_file_of_its_own() {
    let sandbox = cycles_sandbox("cycles");
    let run = cycles(&sandbox, &["old", &sandbox.arm().to_string_lossy()]);
    assert_eq!(run.code(), Some(0), "{}", run.say());

    // The folder is named on stderr, and is the run's own, under the root.
    let folder_line = run
        .stderr()
        .lines()
        .find_map(|line| {
            line.strip_prefix("real-cycles.sh: the footprint files of this run are in ")
        })
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("{}", run.say()));
    let folder = PathBuf::from(&folder_line);
    assert!(
        folder.starts_with(sandbox.arm().join("arrakis-measure/cycles")),
        "{folder_line}"
    );
    assert!(
        folder
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("old-")),
        "{folder_line}"
    );

    // Eight files, one for each read, each the table of the footprint call that came right after that read.
    let mut names: Vec<String> = fs::read_dir(&folder)
        .expect("the folder")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    let mut wanted: Vec<String> = READS.iter().map(|read| format!("{read}.txt")).collect();
    wanted.sort();
    assert_eq!(names, wanted);
    for (index, read) in READS.iter().enumerate() {
        let table = fs::read_to_string(folder.join(format!("{read}.txt"))).expect("a table");
        assert!(
            table.contains(&format!("(call {})", index + 1)),
            "{read}: {table}"
        );
        assert!(table.contains("MALLOC_SMALL"), "{read}: {table}");
    }

    // In the order of the events: a read of phys_footprint, then footprint, for each of the eight, and the
    // clicks of the six reopens between the first close's reads and the last's. The footprint call has the
    // app's pid, and asks for bytes.
    let events = sandbox.events();
    let reads: Vec<&String> = events
        .iter()
        .filter(|event| event.starts_with("read ") || event.starts_with("footprint "))
        .collect();
    assert_eq!(reads.len(), 16, "{events:?}");
    for (index, pair) in reads.chunks(2).enumerate() {
        assert_eq!(
            pair[0],
            &format!("read {} pid {}", index + 1, pid_of(pair[1])),
            "{events:?}"
        );
        assert!(pair[1].starts_with("footprint -p "), "{events:?}");
        assert!(pair[1].ends_with(" -f bytes"), "{events:?}");
    }
    let apps = events
        .iter()
        .filter(|event| event.starts_with("app "))
        .count();
    assert_eq!(apps, 2, "{events:?}");

    // The result line is what it was: the arm, the time, the eight figures, then three counts.
    let stdout = run.stdout();
    let line = stdout.trim_end();
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    let words: Vec<&str> = line.split(' ').collect();
    assert_eq!(words[0], "old", "{line}");
    let keys: Vec<&str> = words[2..]
        .iter()
        .map(|word| word.split('=').next().expect("a key"))
        .collect();
    assert_eq!(
        keys,
        [
            "never",
            "open5",
            "open30",
            "open90",
            "close1_2s",
            "close1_60s",
            "close7_2s",
            "close7_60s",
            "windows_after_close1",
            "reopens",
            "windows_left"
        ],
        "{line}"
    );
    assert!(
        line.contains(" never=1.50 open5=2.50 open30=3.50 "),
        "{line}"
    );
    assert!(
        line.ends_with(" windows_after_close1=1 reopens=6 windows_left=1"),
        "{line}"
    );
}

fn pid_of(footprint_call: &str) -> &str {
    footprint_call
        .split_whitespace()
        .nth(2)
        .unwrap_or_else(|| panic!("{footprint_call}"))
}

#[test]
fn an_arm_that_names_more_than_one_folder_exits_two_before_the_app_starts() {
    let sandbox = cycles_sandbox("cycles-arm");
    for arm in ["a/b", "..", ".", ""] {
        let run = cycles(&sandbox, &[arm, &sandbox.arm().to_string_lossy()]);
        assert_eq!(run.code(), Some(2), "{arm:?}: {}", run.say());
        assert!(
            run.stderr().contains("an arm names one folder"),
            "{arm:?}: {}",
            run.say()
        );
    }
    assert!(sandbox.events().is_empty());
    assert!(!sandbox.arm().join("arrakis-measure/cycles").exists());
}
