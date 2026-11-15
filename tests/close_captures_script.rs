//! `scripts/measure/close-captures.sh` against stand-ins for the demo build, `winid`, `screencapture` and
//! `pgrep`, and `scripts/measure/close-scan.py` on folders of pictures that these tests write. The
//! captures themselves are made on real windows: none opens here, no screen is recorded and no drive
//! runs. What these tests read is the script's flow, and the scan's sorting of a picture that a window
//! never was.
//!
//! Python runs with `PYTHONPYCACHEPREFIX` in the sandbox, so that a script which wrote its bytecode
//! (it would leave a `__pycache__` in `scripts/measure/`, which the repository does not ignore) shows it
//! where the test can look. Left alone, this Mac's Python writes to a prefix of its own, and a test
//! looking for `__pycache__` would never see it.
#![cfg(target_os = "macos")]

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const CAPTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/scripts/measure/close-captures.sh"
);
const SCAN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/measure/close-scan.py");
const TEAR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/measure/tear-scan.py");

/// The stand-in for the demo build. It records `HOME`, `TMPDIR`, the close shape it was given (or
/// `unset`) and its arguments, waits for the first capture if `FAKE_WAIT_FOR` names a file (so that the
/// capture loops, which run while it lives, take one; it clears the file first, so that a second
/// round waits for its own), copies `$FAKE_REPORT` to its `--demo-report` path
/// and exits with `FAKE_EXIT`, 0 by default. The wait is bounded, so a script that never captures ends
/// this after 20 s instead of hanging.
const APP: &str = r#"
here=$(dirname "$0")
echo "$HOME|$TMPDIR|${ARRAKIS_METAL_CLOSE-unset}|$*" >>"$here/calls"
report=
while [ $# -gt 0 ]; do
    case "$1" in
    --demo-report) report=$2; shift ;;
    esac
    shift
done
if [ -n "${FAKE_WAIT_FOR-}" ]; then
    rm -f "$FAKE_WAIT_FOR"
    waited=0
    while [ ! -f "$FAKE_WAIT_FOR" ] && [ "$waited" -lt 400 ]; do
        sleep 0.05
        waited=$((waited + 1))
    done
fi
cp "$FAKE_REPORT" "$report"
exit "${FAKE_EXIT-0}"
"#;

/// The stand-in for `screencapture`: records its arguments, writes `$FAKE_PNG` to the path it was given
/// last, and raises `FAKE_WAIT_FOR`, the flag the app waits for.
const SCREENCAPTURE: &str = r#"
echo "$*" >>"$(dirname "$0")/captures"
for path; do :; done
cp "$FAKE_PNG" "$path"
if [ -n "${FAKE_WAIT_FOR-}" ]; then
    : >"$FAKE_WAIT_FOR"
fi
"#;

/// A drive that hangs in its first `FAKE_HANG_ROUNDS` rounds (1 by default): it notes its call and its pid,
/// then becomes a `sleep`, so that the pid noted is the process a TERM ends, or with `FAKE_HANG=deaf`
/// takes no notice of TERM, as a hung app does, so that only a KILL ends it. In a later round it is
/// `APP`: it waits for the first capture, writes its report and exits.
///
/// It writes `ready` beside it when it is what the test says it is: after its call and pid are noted and,
/// for `deaf`, after its `trap '' TERM`. `FAKE_HANG_LATE` seconds of delay (none by default) come before
/// the `trap`, as a loaded machine's slow start would. `FROZEN_DATE` holds the script's deadline back until
/// `ready` is there, so a TERM never reaches the stand-in before it is the process the test describes.
const HANGING_APP: &str = r#"
here=$(dirname "$0")
echo "$*" >>"$here/calls"
echo $$ >>"$here/app.pids"
report=
while [ $# -gt 0 ]; do
    case "$1" in
    --demo-report) report=$2; shift ;;
    esac
    shift
done
rounds=$(wc -l <"$here/calls" | tr -d ' ')
if [ "$rounds" -le "${FAKE_HANG_ROUNDS-1}" ]; then
    if [ "${FAKE_HANG-sleep}" = deaf ]; then
        [ -z "${FAKE_HANG_LATE-}" ] || sleep "$FAKE_HANG_LATE"
        trap '' TERM
        : >"$here/ready"
        while :; do sleep 1; done
    fi
    : >"$here/ready"
    exec sleep 60
fi
rm -f "$FAKE_WAIT_FOR"
waited=0
while [ ! -f "$FAKE_WAIT_FOR" ] && [ "$waited" -lt 400 ]; do
    sleep 0.05
    waited=$((waited + 1))
done
cp "$FAKE_REPORT" "$report"
exit 0
"#;

/// A `date` that answers the script's `date +%s` with the time of its first call until `HANGING_APP` has
/// written `ready`, and with the real time from then on. The script's limit counts the seconds from its
/// first call, so a limit of two seconds starts to run only when the stand-in is ready for the TERM that
/// ends it. Any other call is the real `date`.
const FROZEN_DATE: &str = r#"
here=$(dirname "$0")
if [ "$*" != "+%s" ] || [ -e "$here/ready" ]; then
    exec /bin/date "$@"
fi
[ -f "$here/date.frozen" ] || /bin/date +%s >"$here/date.frozen"
cat "$here/date.frozen"
"#;

/// `SCREENCAPTURE` that also notes the pid of the capture loop that called it, and takes its time as the
/// real one does: `FAKE_CAPTURE_TAKES` seconds, 0.2 by default. So three loops do not spin for the
/// seconds a hung drive lives, and a test can set a call long enough that a loop left running after an
/// interrupt takes another picture later, and shows.
const TAKING_SCREENCAPTURE: &str = r#"
here=$(dirname "$0")
echo "$*" >>"$here/captures"
echo $PPID >>"$here/loops.pids"
for path; do :; done
cp "$FAKE_PNG" "$path"
if [ -n "${FAKE_WAIT_FOR-}" ]; then
    : >"$FAKE_WAIT_FOR"
fi
sleep "${FAKE_CAPTURE_TAKES-0.2}"
"#;

/// A `winid` that finds the window, number 7.
const WINID: &str = "echo 7";
/// A `winid` that finds none, as before a window opens.
const NO_WINDOW: &str = "exit 1";
/// A `pgrep` that finds nothing running, and notes what it was asked.
const NOTHING_RUNNING: &str = "echo \"$@\" >>\"$(dirname \"$0\")/pgrep.args\"\nexit 1";
/// A `pgrep` that finds process 4242.
const ARRAKIS_RUNNING: &str = "echo \"$@\" >>\"$(dirname \"$0\")/pgrep.args\"\necho 4242";

/// A new folder with a `bin` for the stand-ins, a `work` for the script's root, `pictures` for the
/// scan's folders, and a copy of the scan scripts, removed when it goes out of scope.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "arrakis-close-captures-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        for folder in ["bin", "work", "pictures", "scripts"] {
            fs::create_dir_all(root.join(folder)).expect("a folder");
        }
        for (from, to) in [(SCAN, "close-scan.py"), (TEAR, "tear-scan.py")] {
            fs::copy(from, root.join("scripts").join(to)).expect("a copy of the script");
        }
        Self { root }
    }

    fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }

    /// The script's `root` argument.
    fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    /// Where the stand-in drive's runs go for `label`.
    fn out(&self, label: &str) -> PathBuf {
        self.work().join("arrakis-measure/close").join(label)
    }

    /// The folder the scan reads.
    fn pictures(&self) -> PathBuf {
        self.root.join("pictures")
    }

    /// A stand-in named `name` in `bin`.
    fn stand_in(&self, name: &str, body: &str) -> PathBuf {
        let path = self.bin().join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("a stand-in");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
        path
    }

    /// What a stand-in noted, one call to a line. Empty when it was never called.
    fn noted(&self, file: &str) -> Vec<String> {
        fs::read_to_string(self.bin().join(file))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Whether any picture was taken or any drive run.
    fn anything_ran(&self) -> bool {
        !self.noted("captures").is_empty() || !self.noted("calls").is_empty()
    }

    /// Where Python is told to write bytecode, in place of `__pycache__` and of whatever prefix this Mac's
    /// Python sets by itself, so that the test can see whether a script wrote its own.
    fn bytecode(&self) -> PathBuf {
        self.root.join("bytecode")
    }

    /// Fails if either scan script wrote bytecode, which would leave a `__pycache__` in
    /// `scripts/measure/`, and the repository does not ignore one. The standard library's modules write
    /// theirs under the same prefix, so only the scripts' are looked for.
    fn assert_no_bytecode_of_the_scripts(&self) {
        let mut folders = vec![self.bytecode()];
        while let Some(folder) = folders.pop() {
            let Ok(entries) = fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.map(|entry| entry.expect("an entry")) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if entry.path().is_dir() {
                    folders.push(entry.path());
                } else {
                    assert!(
                        !name.starts_with("close-scan.") && !name.starts_with("tear-scan."),
                        "a script wrote its bytecode: {}",
                        entry.path().display()
                    );
                }
            }
        }
    }

    /// The script with every stand-in in place, `CLOSES` and `ROUNDS` set small, and `env` on top. The
    /// script runs the repository's own scan, which must write no bytecode.
    fn run_script(&self, args: &[&Path], env: &[(&str, &str)]) -> Script {
        let output = self.script_command(args, env).output().expect("sh runs");
        Script(output)
    }

    /// The command `run_script` runs.
    fn script_command(&self, args: &[&Path], env: &[(&str, &str)]) -> Command {
        let path = format!(
            "{}:{}",
            self.bin().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new("sh");
        command
            .arg(CAPTURES)
            .args(args)
            .env("PATH", path)
            .env("TMPDIR", self.root.join("tmp-nothing"))
            .env_remove("PYTHONDONTWRITEBYTECODE")
            .env("PYTHONPYCACHEPREFIX", self.bytecode())
            .env("ROUNDS", "1")
            .env("CLOSES", "1")
            .env("WINID", self.bin().join("winid"))
            .env("SCREENCAPTURE", self.bin().join("screencapture"))
            .env("CLOSE_CAPTURES_BIN", self.bin().join("arrakis"))
            .env("FAKE_REPORT", self.root.join("report.json"))
            .env("FAKE_PNG", self.root.join("window.png"))
            .env_remove("ARRAKIS_METAL_CLOSE")
            .env_remove("FAKE_EXIT")
            .env_remove("FAKE_WAIT_FOR")
            .env_remove("FAKE_HANG")
            .env_remove("FAKE_HANG_ROUNDS")
            .env_remove("FAKE_HANG_LATE")
            .env_remove("FAKE_CAPTURE_TAKES")
            .envs(env.iter().copied());
        command
    }

    /// Starts the script and leaves it running, its output in files of the sandbox, for a test that must
    /// end it or interrupt it. A shell ignores for good a signal that was ignored when it started, and
    /// a process that a non-interactive shell put in the background starts with INT ignored: a test run
    /// from such a shell would find the script deaf to the interrupt it sends, and hang, or pass for
    /// the wrong reason. So INT and TERM are put back to their defaults between the fork and the exec.
    fn start_script(&self, args: &[&Path], env: &[(&str, &str)]) -> Child {
        let mut command = self.script_command(args, env);
        command
            .stdin(Stdio::null())
            .stdout(File::create(self.root.join("script.out")).expect("a file"))
            .stderr(File::create(self.root.join("script.err")).expect("a file"));
        // SAFETY: `signal` is async-signal-safe, and the closure touches nothing else of this process:
        // it runs in the child, between the fork and the exec.
        unsafe {
            command.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGTERM, libc::SIG_DFL);
                Ok(())
            });
        }
        command.spawn().expect("sh runs")
    }

    /// What a script started with `start_script` came to, once it has ended. If it has not within
    /// `within`, it is killed and the test fails: a script that hangs must not hang the test.
    fn end_of(&self, mut child: Child, within: Duration) -> Script {
        let begin = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().expect("a wait") {
                break status;
            }
            if begin.elapsed() > within {
                let _ = child.kill();
                let _ = child.wait();
                let said = self.said_so_far();
                panic!("the script was still running after {within:?}: {said}");
            }
            thread::sleep(Duration::from_millis(50));
        };
        self.script_ended(status)
    }

    /// What a script started with `start_script` printed.
    fn said_so_far(&self) -> String {
        let read = |file: &str| fs::read_to_string(self.root.join(file)).unwrap_or_default();
        format!("{}\n{}", read("script.out"), read("script.err"))
    }

    fn script_ended(&self, status: ExitStatus) -> Script {
        let read = |file: &str| fs::read(self.root.join(file)).unwrap_or_default();
        Script(Output {
            status,
            stdout: read("script.out"),
            stderr: read("script.err"),
        })
    }

    /// The pids a stand-in noted, one to a line, each once.
    fn pids(&self, file: &str) -> Vec<i32> {
        let mut pids: Vec<i32> = self
            .noted(file)
            .iter()
            .filter_map(|line| line.trim().parse().ok())
            .collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// Whether a pid noted by a stand-in is still one of this sandbox's processes: a drive, or a capture
    /// loop, which is a copy of the script. A pid that was freed and given to another process is not.
    fn is_one_of_ours(&self, pid: i32) -> bool {
        let Ok(output) = Command::new("ps")
            .args(["-o", "command=", "-p", &pid.to_string()])
            .output()
        else {
            return false;
        };
        let command = String::from_utf8_lossy(&output.stdout);
        command.contains("close-captures.sh")
            || command.trim() == "sleep 60"
            || command.contains(&*self.bin().to_string_lossy())
    }

    /// The stand-ins of a run that captures: a window that is found, a screen that is recorded with a
    /// window's own picture, and a drive that holds, and waits for the first capture.
    fn capturing(&self, closes: usize) {
        self.stand_in("arrakis", APP);
        self.stand_in("winid", WINID);
        self.stand_in("screencapture", SCREENCAPTURE);
        self.stand_in("pgrep", NOTHING_RUNNING);
        fs::write(self.root.join("window.png"), window_picture()).expect("a picture");
        fs::write(self.root.join("report.json"), report(closes).to_string()).expect("a report");
    }

    /// The scan's own script, run on `args`.
    fn scan(&self, args: &[&str]) -> Script {
        let output = Command::new("python3")
            .arg(self.root.join("scripts/close-scan.py"))
            .args(args)
            .env_remove("PYTHONDONTWRITEBYTECODE")
            .env("PYTHONPYCACHEPREFIX", self.bytecode())
            .output()
            .expect("python3 runs");
        Script(output)
    }

    /// The scan on the folder of runs, for runs of `closes` closes.
    fn scan_runs(&self, closes: usize) -> Script {
        self.scan(&[
            self.pictures().to_str().expect("a path"),
            &closes.to_string(),
        ])
    }

    /// A run folder that holds: `closes` windows, three captures of the window's own picture each, an
    /// exit status of 0 and a report that holds what the scan wants.
    fn good_run(&self, name: &str, closes: usize) -> PathBuf {
        let folder = self.pictures().join(name);
        fs::create_dir_all(&folder).expect("a run folder");
        for window in 1..=closes {
            for n in 1..=3 {
                fs::write(
                    folder.join(format!("w{window}-0.60-{n:04}.png")),
                    window_picture(),
                )
                .expect("a capture");
            }
        }
        fs::write(folder.join("exit"), "0\n").expect("an exit status");
        fs::write(folder.join("report.json"), report(closes).to_string()).expect("a report");
        folder
    }

    /// The summary the scan wrote.
    fn summary(&self) -> Value {
        let text = fs::read_to_string(self.pictures().join("summary.json")).expect("a summary");
        serde_json::from_str(&text).expect("JSON")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // A test that failed may have left a drive that sleeps for a minute, or capture loops that go on
        // while it lives: end what this sandbox started, and only that.
        for pid in self
            .pids("app.pids")
            .into_iter()
            .chain(self.pids("loops.pids"))
        {
            if self.is_one_of_ours(pid) {
                // SAFETY: `kill` takes a pid and a signal, and touches nothing of this process.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Script(Output);

impl Script {
    fn code(&self) -> Option<i32> {
        self.0.status.code()
    }

    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.0.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.0.stderr).into_owned()
    }

    /// Everything the run printed, for an assert's message.
    fn say(&self) -> String {
        format!("{}\n{}", self.stdout(), self.stderr())
    }

    /// The lines of stdout that begin with `prefix`.
    fn lines(&self, prefix: &str) -> Vec<String> {
        self.stdout()
            .lines()
            .filter(|line| line.starts_with(prefix))
            .map(str::to_owned)
            .collect()
    }

    /// The one line of stdout that begins with `prefix`.
    fn line(&self, prefix: &str) -> String {
        match &self.lines(prefix)[..] {
            [line] => line.clone(),
            lines => panic!(
                "{} lines begin with {prefix:?}: {}",
                lines.len(),
                self.say()
            ),
        }
    }

    /// The first line of stdout, which is the scan's line for the runs.
    fn runs_line(&self) -> String {
        self.stdout().lines().next().unwrap_or_default().to_owned()
    }
}

// ---- the pictures ----

const SIDE: usize = 40;
/// The grey of a real window, as read off the bare captures.
const GREY: [u8; 4] = [35, 33, 33, 255];

/// An 8-bit PNG, as `screencapture -o` writes, with the encoder's real zlib stream.
fn png(width: usize, height: usize, color: png::ColorType, pixels: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width as u32, height as u32);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("a header");
        writer.write_image_data(pixels).expect("the pixels");
        writer.finish().expect("the end");
    }
    bytes
}

/// Opaque pixels that no two rows share and a zlib stream cannot shrink: what a window's interior is to
/// the scan, and what makes its file far larger than a flat picture's.
fn noise(width: usize, height: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    let mut pixels = Vec::with_capacity(width * height * 4);
    for _ in 0..width * height {
        for _ in 0..3 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            pixels.push((state >> 24) as u8);
        }
        pixels.push(255);
    }
    pixels
}

/// A window's own capture: opaque, and not flat.
fn window_picture() -> Vec<u8> {
    png(SIDE, SIDE, png::ColorType::Rgba, &noise(SIDE, SIDE, 1))
}

/// One RGBA color from edge to edge.
fn flat_picture(color: [u8; 4]) -> Vec<u8> {
    png(SIDE, SIDE, png::ColorType::Rgba, &color.repeat(SIDE * SIDE))
}

/// Every pixel at alpha 0: a window that had gone when the capture was taken.
fn transparent_picture() -> Vec<u8> {
    flat_picture([0, 0, 0, 0])
}

/// A window's rounded corners: transparent there, the window's grey everywhere else, so the interior
/// is flat and opaque.
fn bare_window_picture() -> Vec<u8> {
    let mut pixels = GREY.repeat(SIDE * SIDE);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let near_x = !(3..SIDE - 3).contains(&x);
            let near_y = !(3..SIDE - 3).contains(&y);
            if near_x && near_y {
                pixels[(y * SIDE + x) * 4..][..4].copy_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    png(SIDE, SIDE, png::ColorType::Rgba, &pixels)
}

/// The same grey with no alpha channel at all.
fn rgb_picture() -> Vec<u8> {
    png(
        SIDE,
        SIDE,
        png::ColorType::Rgb,
        &GREY[..3].repeat(SIDE * SIDE),
    )
}

/// Not flat, and a size of its own.
fn taller_picture() -> Vec<u8> {
    png(
        SIDE,
        SIDE + 20,
        png::ColorType::Rgba,
        &noise(SIDE, SIDE + 20, 2),
    )
}

/// A window's capture cut in half: a file that stops inside its data.
fn damaged_picture() -> Vec<u8> {
    let mut bytes = window_picture();
    bytes.truncate(bytes.len() / 2);
    bytes
}

/// What a passing drive reports for `closes` closes: finished in a little over their time, nothing
/// covered, a Metal window for each close.
fn report(closes: usize) -> Value {
    json!({
        "finished": true,
        "elapsed_s": closes as f64 * 3.0 + 1.0,
        "unseen_frames": 0,
        "opens": (0..closes).map(|_| json!({ "renderer": "metal" })).collect::<Vec<_>>(),
        "hold": { "spare_drops": 6 },
    })
}

#[test]
fn the_pictures_are_the_sizes_the_scan_sorts_by() {
    // A flat picture compresses far below rows of different colors, which is what makes it stray from the
    // median: the scan opens only the captures whose size strays by more than 2%.
    let window = window_picture().len();
    for (name, size) in [
        ("grey", flat_picture(GREY).len()),
        ("transparent", transparent_picture().len()),
        ("bare window", bare_window_picture().len()),
        ("rgb", rgb_picture().len()),
        ("damaged", damaged_picture().len()),
        ("taller", taller_picture().len()),
    ] {
        assert!(
            size.abs_diff(window) * 10 > window,
            "{name}: {size} against {window}"
        );
    }
}

// ---- the script ----

#[test]
fn the_script_is_valid_shell() {
    let status = Command::new("sh")
        .args(["-n", CAPTURES])
        .status()
        .expect("sh runs");
    assert!(status.success());
}

#[test]
fn no_arguments_or_one_says_the_usage_and_exits_non_zero_before_anything_runs() {
    let sandbox = Sandbox::new("usage");
    sandbox.capturing(1);
    let root = sandbox.work();
    for args in [&[][..], &[Path::new("label")][..]] {
        let run = sandbox.run_script(args, &[]);
        assert_eq!(run.code(), Some(2), "{args:?}: {}", run.say());
        assert!(
            run.stderr()
                .contains("usage: close-captures.sh <label> <root>"),
            "{}",
            run.stderr()
        );
    }
    assert!(!sandbox.anything_ran());
    assert!(!root.join("arrakis-measure").exists(), "nothing was made");
}

#[test]
fn a_missing_winid_says_so_and_nothing_is_captured_or_driven() {
    let sandbox = Sandbox::new("no-winid");
    sandbox.capturing(1);
    let run = sandbox.run_script(
        &[Path::new("label"), &sandbox.work()],
        &[(
            "WINID",
            sandbox.bin().join("missing").to_str().expect("a path"),
        )],
    );
    assert_eq!(run.code(), Some(2), "{}", run.say());
    assert!(
        run.stderr().contains("build winid into"),
        "{}",
        run.stderr()
    );
    assert!(!sandbox.anything_ran());
}

#[test]
fn a_missing_binary_says_so_and_nothing_is_captured_or_driven() {
    let sandbox = Sandbox::new("no-binary");
    sandbox.capturing(1);
    let missing = sandbox.bin().join("missing");
    let run = sandbox.run_script(
        &[Path::new("label"), &sandbox.work()],
        &[("CLOSE_CAPTURES_BIN", missing.to_str().expect("a path"))],
    );
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.stderr()
            .contains(&format!("no {}; build it first", missing.display())),
        "{}",
        run.stderr()
    );
    assert!(!sandbox.anything_ran());
}

#[test]
fn a_running_copy_refuses_the_start_and_no_drive_runs() {
    let sandbox = Sandbox::new("running");
    sandbox.capturing(1);
    sandbox.stand_in("pgrep", ARRAKIS_RUNNING);
    let run = sandbox.run_script(&[Path::new("label"), &sandbox.work()], &[]);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.stderr()
            .contains("another arrakis is running (pid 4242); quit it first"),
        "{}",
        run.stderr()
    );
    assert_eq!(sandbox.noted("pgrep.args"), ["-i -x arrakis"]);
    assert!(!sandbox.anything_ran());
}

#[test]
fn every_round_drives_open_close_in_its_own_folder_with_its_own_home_and_no_close_shape() {
    let sandbox = Sandbox::new("rounds");
    sandbox.capturing(1);
    let flag = sandbox.root.join("first-capture");
    let work = sandbox.work();
    let run = sandbox.run_script(
        &[Path::new("label"), &work],
        &[
            ("ROUNDS", "2"),
            ("FAKE_WAIT_FOR", flag.to_str().expect("a path")),
        ],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());

    // One drive for each round, with its own HOME and TMPDIR beside the work folder, the close's
    // seconds for the closes asked for, and no close shape in its environment.
    let measure = work.join("arrakis-measure");
    let calls = sandbox.noted("calls");
    assert_eq!(calls.len(), 2, "{calls:?}");
    let out = sandbox.out("label");
    for (round, call) in (1..).zip(&calls) {
        let want = format!(
            "{home}|{tmp}|unset|--demo-drive open-close --demo-seconds 3 --demo-report {report}",
            home = measure.join("home").display(),
            tmp = measure.join("tmp").display(),
            report = out.join(format!("run-{round}/report.json")).display(),
        );
        assert_eq!(*call, want);
    }
    let script = fs::read_to_string(CAPTURES).expect("the script");
    assert!(
        !script.contains("ARRAKIS_METAL_CLOSE"),
        "the script sets none"
    );

    // Each run's folder holds the drive's report, its exit status and the pictures taken of its window.
    for round in 1..=2 {
        let folder = out.join(format!("run-{round}"));
        assert!(folder.join("report.json").is_file(), "run-{round}");
        assert_eq!(
            fs::read_to_string(folder.join("exit"))
                .expect("an exit")
                .trim(),
            "0"
        );
        let pictures: Vec<String> = fs::read_dir(&folder)
            .expect("the folder")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.ends_with(".png"))
            .collect();
        assert!(!pictures.is_empty(), "run-{round} has no picture");
        assert!(
            pictures.iter().all(|name| name.starts_with("w7-")),
            "named by window: {pictures:?}"
        );
    }

    // Every call of the stand-in screen recorder asks for the one window, with no shadow and no sound.
    let captures = sandbox.noted("captures");
    assert!(!captures.is_empty());
    for call in &captures {
        assert!(call.starts_with("-x -o -l7 "), "{call}");
    }

    // The scan's line, and where the pictures are, last.
    assert!(
        run.runs_line().starts_with("runs=2 of 2 closes_seen=2 "),
        "{}",
        run.say()
    );
    assert_eq!(
        run.stdout().lines().last(),
        Some(format!("pictures: {}", out.display()).as_str())
    );
}

#[test]
fn a_drive_that_exits_non_zero_spoils_its_run_and_the_script_exits_one() {
    let sandbox = Sandbox::new("drive-fails");
    sandbox.capturing(1);
    let flag = sandbox.root.join("first-capture");
    let run = sandbox.run_script(
        &[Path::new("label"), &sandbox.work()],
        &[
            ("FAKE_EXIT", "3"),
            ("FAKE_WAIT_FOR", flag.to_str().expect("a path")),
        ],
    );
    assert_eq!(run.code(), Some(1), "{}", run.say());
    let exit = fs::read_to_string(sandbox.out("label").join("run-1/exit")).expect("an exit");
    assert_eq!(exit.trim(), "3");
    assert_eq!(run.line("spoiled: "), "spoiled: run-1: exit 3");
}

#[test]
fn with_no_window_to_find_nothing_is_captured_and_the_script_exits_two() {
    let sandbox = Sandbox::new("no-window");
    sandbox.capturing(1);
    sandbox.stand_in("winid", NO_WINDOW);
    let run = sandbox.run_script(&[Path::new("label"), &sandbox.work()], &[]);
    assert_eq!(run.code(), Some(2), "{}", run.say());
    assert!(sandbox.noted("captures").is_empty());
    assert_eq!(sandbox.noted("calls").len(), 1, "the drive ran once");
    assert!(
        run.stderr().contains("a run holds no capture"),
        "{}",
        run.stderr()
    );
    assert_eq!(
        run.stdout().lines().last(),
        Some(format!("pictures: {}", sandbox.out("label").display()).as_str())
    );
}

// ---- the script: an interrupt, the label and the deadline ----

/// Waits until `done` holds, and fails if it does not within `within`.
fn wait_until(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let begin = Instant::now();
    while !done() {
        assert!(begin.elapsed() < within, "{what}: not within {within:?}");
        thread::sleep(Duration::from_millis(25));
    }
}

/// Whether a process by this pid exists.
fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Sends `signal` to the script, which must be there to take it.
fn send(script: &Child, signal: i32) {
    // SAFETY: `kill` takes a pid and a signal, and touches nothing of this process.
    let sent = unsafe { libc::kill(script.id() as i32, signal) };
    assert_eq!(sent, 0, "the script was there to take signal {signal}");
}

/// Interrupts the script with `signal` in its first of two rounds, when the drive sleeps and each of the
/// three capture loops is inside a call of the screen recorder, and reads what is left: the exit status
/// `code`, no process of the run, no picture after the interrupt, no second round and no scan.
fn an_interrupt_ends_the_whole_script(name: &str, signal: i32, code: i32) {
    let sandbox = Sandbox::new(name);
    sandbox.capturing(1);
    sandbox.stand_in("arrakis", HANGING_APP);
    sandbox.stand_in("screencapture", TAKING_SCREENCAPTURE);
    let work = sandbox.work();
    let script = sandbox.start_script(
        &[Path::new("label"), &work],
        &[("ROUNDS", "2"), ("FAKE_CAPTURE_TAKES", "3")],
    );
    wait_until(
        "each of the three capture loops took a picture",
        Duration::from_secs(20),
        || sandbox.pids("loops.pids").len() == 3,
    );
    let loops = sandbox.pids("loops.pids");
    let apps = sandbox.pids("app.pids");
    assert_eq!(apps.len(), 1, "one drive so far");

    send(&script, signal);
    let run = sandbox.end_of(script, Duration::from_secs(20));
    assert_eq!(run.code(), Some(code), "{}", run.say());

    // The drive and every capture loop went with it, not the drive alone. A loop left to itself would
    // end too, once its call of the recorder (3 s, begun about a second ago) returned and found the
    // drive gone, so the grace is shorter than that.
    for pid in apps.iter().chain(&loops) {
        wait_until(
            &format!("process {pid} to be gone"),
            Duration::from_secs(1),
            || !alive(*pid),
        );
    }
    // And no picture is taken later.
    let taken = sandbox.noted("captures").len();
    thread::sleep(Duration::from_secs(1));
    assert_eq!(
        sandbox.noted("captures").len(),
        taken,
        "a capture loop went on after the interrupt"
    );
    assert_eq!(sandbox.noted("calls").len(), 1, "a second round started");
    let out = sandbox.out("label");
    assert!(!out.join("run-2").exists(), "a second round's folder");
    assert!(!out.join("summary.json").exists(), "the scan ran");
    assert!(run.lines("runs=").is_empty(), "{}", run.say());
    assert!(run.lines("pictures: ").is_empty(), "{}", run.say());
}

#[test]
fn an_interrupt_ends_the_drive_the_capture_loops_and_the_script_and_scans_nothing() {
    an_interrupt_ends_the_whole_script("interrupt", libc::SIGINT, 130);
}

#[test]
fn a_term_ends_the_drive_the_capture_loops_and_the_script_and_scans_nothing() {
    an_interrupt_ends_the_whole_script("term", libc::SIGTERM, 143);
}

#[test]
fn a_label_that_names_more_than_one_folder_exits_two_before_anything_runs_or_goes() {
    let sandbox = Sandbox::new("labels");
    sandbox.capturing(1);
    let work = sandbox.work();
    let measure = work.join("arrakis-measure");
    // What an `rm -rf` of `$M/close/<label>` would take for each of the labels below, one file in each.
    let canaries = [
        measure.join("canary"),
        measure.join("close/canary"),
        measure.join("close/a/b/canary"),
        measure.join("x/canary"),
    ];
    for canary in &canaries {
        fs::create_dir_all(canary.parent().expect("a folder")).expect("a folder");
        fs::write(canary, "alive").expect("a canary");
    }
    for label in ["", ".", "..", "a/b", "../x", "/"] {
        let run = sandbox.run_script(&[Path::new(label), &work], &[]);
        assert_eq!(run.code(), Some(2), "{label:?}: {}", run.say());
        assert!(
            run.stderr()
                .contains(&format!("a label names one folder, and '{label}' does not")),
            "{label:?}: {}",
            run.stderr()
        );
        // Nothing ran, not even the check for a copy of the app that runs.
        assert!(!sandbox.anything_ran(), "{label:?}");
        assert!(sandbox.noted("pgrep.args").is_empty(), "{label:?}");
        for canary in &canaries {
            assert!(canary.is_file(), "{label:?} removed {}", canary.display());
        }
        for made in ["home", "tmp"] {
            assert!(!measure.join(made).exists(), "{label:?} made {made}");
        }
    }

    // A label that only holds dots is one folder, and runs; it removes its own folder and no other.
    let flag = sandbox.root.join("first-capture");
    let run = sandbox.run_script(
        &[Path::new("v1.2"), &work],
        &[("FAKE_WAIT_FOR", flag.to_str().expect("a path"))],
    );
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert_eq!(sandbox.noted("calls").len(), 1);
    assert!(sandbox.out("v1.2").join("run-1/report.json").is_file());
    for canary in &canaries {
        assert!(
            canary.is_file(),
            "a good label removed {}",
            canary.display()
        );
    }
}

/// The script with a limit of two seconds on the app, whose drive hangs in its first round and takes the
/// next one as `APP` does, for `rounds` rounds, ended by the test if it will not end. The limit counts from
/// before the drive starts, and with the tests running side by side the drive's shell may take longer than
/// the limit to reach its `trap`: so `FROZEN_DATE` holds the deadline back until the drive has written
/// `ready` (see `HANGING_APP`), and the limit runs from the drive's readiness, never from a race to it.
/// `late` is the seconds the drive waits before its `trap`, or empty.
fn a_hung_app_is_ended(sandbox: &Sandbox, rounds: &str, hang: &str, late: &str) -> Script {
    let flag = sandbox.root.join("first-capture");
    let work = sandbox.work();
    sandbox.stand_in("date", FROZEN_DATE);
    let script = sandbox.start_script(
        &[Path::new("label"), &work],
        &[
            ("ROUNDS", rounds),
            ("CLOSE_CAPTURES_LIMIT", "2"),
            ("FAKE_HANG", hang),
            ("FAKE_HANG_LATE", late),
            ("FAKE_WAIT_FOR", flag.to_str().expect("a path")),
        ],
    );
    // Once the drive is ready, a limit of 2 s passes within 3.5 s, a TERM is given 5 s, and a round takes
    // about 1.3 s more: two rounds take about 12 s.
    sandbox.end_of(script, Duration::from_secs(30))
}

#[test]
fn an_app_past_its_limit_is_ended_its_run_is_spoiled_and_the_next_round_still_runs() {
    let sandbox = Sandbox::new("limit-term");
    sandbox.capturing(1);
    sandbox.stand_in("arrakis", HANGING_APP);
    sandbox.stand_in("screencapture", TAKING_SCREENCAPTURE);
    let run = a_hung_app_is_ended(&sandbox, "2", "sleep", "");

    // The drive was ready for the TERM before the limit ran: its mark is there.
    assert!(sandbox.bin().join("ready").is_file(), "{}", run.say());
    // One round was past its limit, and said so; the other was not.
    assert_eq!(
        run.stderr().matches("the app ran past 2 s; killed").count(),
        1,
        "{}",
        run.say()
    );
    let out = sandbox.out("label");
    let exit = |round: u32| {
        fs::read_to_string(out.join(format!("run-{round}/exit")))
            .expect("an exit")
            .trim()
            .to_owned()
    };
    // A TERM ended the first drive, so its status is 128 and 15, and the second ran to its end.
    assert_eq!(exit(1), "143");
    assert_eq!(exit(2), "0");
    assert_eq!(sandbox.noted("calls").len(), 2, "both rounds ran");
    for pid in sandbox.pids("app.pids") {
        assert!(!alive(pid), "the drive {pid} is still there");
    }
    // The scan reads the first run as spoiled, and the second as held.
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(run.runs_line().starts_with("runs=1 of 2 "), "{}", run.say());
    let spoiled = run.lines("spoiled: ");
    for reason in ["exit 143", "no report"] {
        assert!(
            spoiled.contains(&format!("spoiled: run-1: {reason}")),
            "{reason}: {}",
            run.say()
        );
    }
    assert!(
        spoiled
            .iter()
            .all(|line| line.starts_with("spoiled: run-1: ")),
        "the second run held: {}",
        run.say()
    );
}

#[test]
fn an_app_that_takes_no_notice_of_term_is_killed_and_its_run_is_spoiled() {
    let sandbox = Sandbox::new("limit-kill");
    sandbox.capturing(1);
    sandbox.stand_in("arrakis", HANGING_APP);
    sandbox.stand_in("screencapture", TAKING_SCREENCAPTURE);
    let begin = Instant::now();
    let run = a_hung_app_is_ended(&sandbox, "1", "deaf", "");

    assert!(sandbox.bin().join("ready").is_file(), "{}", run.say());
    assert!(
        run.stderr().contains("the app ran past 2 s; killed"),
        "{}",
        run.say()
    );
    // The drive took no notice of the TERM, and a KILL ended it: status 128 and 9. It was given the
    // seconds the script leaves a TERM to work in first.
    assert!(
        begin.elapsed() >= Duration::from_secs(5),
        "{:?}",
        begin.elapsed()
    );
    let exit = fs::read_to_string(sandbox.out("label").join("run-1/exit")).expect("an exit");
    assert_eq!(exit.trim(), "137");
    for pid in sandbox.pids("app.pids") {
        assert!(!alive(pid), "the drive {pid} is still there");
    }
    assert_eq!(run.code(), Some(1), "{}", run.say());
    let spoiled = run.lines("spoiled: ");
    for reason in ["exit 137", "no report"] {
        assert!(
            spoiled.contains(&format!("spoiled: run-1: {reason}")),
            "{reason}: {}",
            run.say()
        );
    }
}

#[test]
fn a_drive_slow_to_reach_its_trap_is_not_ended_before_it_has_the_limit_runs_from_its_readiness() {
    // The drive waits 4 s before its `trap '' TERM`, longer than the limit of 2 s and the 3 s at most that
    // the script's whole-second clock can take to pass it. Without the held-back deadline, the script's
    // TERM would reach a shell that has no trap yet, and its status would be 143: a loaded machine's slow
    // start, made certain.
    let sandbox = Sandbox::new("limit-late");
    sandbox.capturing(1);
    sandbox.stand_in("arrakis", HANGING_APP);
    sandbox.stand_in("screencapture", TAKING_SCREENCAPTURE);
    let begin = Instant::now();
    let run = a_hung_app_is_ended(&sandbox, "1", "deaf", "4");

    assert!(sandbox.bin().join("ready").is_file(), "{}", run.say());
    // The clock the script read was the one held at its first call, until the mark.
    assert!(sandbox.bin().join("date.frozen").is_file(), "{}", run.say());
    // 4 s to the mark, then the 5 s that a TERM is left to work in.
    assert!(
        begin.elapsed() >= Duration::from_secs(9),
        "{:?}",
        begin.elapsed()
    );
    let exit = fs::read_to_string(sandbox.out("label").join("run-1/exit")).expect("an exit");
    assert_eq!(exit.trim(), "137", "{}", run.say());
    for pid in sandbox.pids("app.pids") {
        assert!(!alive(pid), "the drive {pid} is still there");
    }
}

// ---- the scan ----

#[test]
fn an_all_good_scan_exits_zero_and_says_its_runs_in_one_line_with_no_memory_in_it() {
    let sandbox = Sandbox::new("scan-good");
    sandbox.good_run("run-1", 2);
    sandbox.good_run("run-2", 2);
    let run = sandbox.scan_runs(2);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert_eq!(run.stderr(), "");
    let lines: Vec<String> = run.stdout().lines().map(str::to_owned).collect();
    assert_eq!(
        lines,
        [
            "runs=2 of 2 closes_seen=4 captures=12 bare=0 bare_closes=0 gone=0 other=0 spare_drops=12"
                .to_owned(),
            format!("pictures: {}", sandbox.pictures().display()),
        ]
    );
    // The summary is written, a run to an entry, and holds no figure of memory.
    let summary = sandbox.summary();
    assert_eq!(summary.as_array().expect("a list").len(), 2);
    assert_eq!(summary[0]["run"], "run-1");
    assert_eq!(summary[0]["spare_drops"], 6);
    assert_eq!(summary[0]["spoiled"], json!([]));
    let text = fs::read_to_string(sandbox.pictures().join("summary.json")).expect("a summary");
    assert!(!text.contains("first_close_median_mb"), "{text}");
    assert!(!run.stdout().contains("first_close_median_mb"));
}

#[test]
fn a_flat_opaque_capture_far_from_the_median_is_bare_and_bare_closes_counts_windows() {
    let sandbox = Sandbox::new("scan-bare");
    let folder = sandbox.good_run("run-1", 2);
    // Two bare captures of window 1 and one of window 2.
    let strays = ["w1-0.65-0007.png", "w1-0.70-0008.png", "w2-0.65-0009.png"];
    for name in strays {
        fs::write(folder.join(name), flat_picture(GREY)).expect("a capture");
    }
    let run = sandbox.scan_runs(2);
    assert_eq!(
        run.runs_line(),
        "runs=1 of 1 closes_seen=2 captures=9 bare=3 bare_closes=2 gone=0 other=0 spare_drops=6",
        "{}",
        run.say()
    );
    let named = run.lines("bare: ");
    assert_eq!(named.len(), 3, "{}", run.say());
    for (line, name) in named.iter().zip(strays) {
        assert_eq!(*line, format!("bare: {}", folder.join(name).display()));
    }
    assert!(run.lines("other: ").is_empty());
    assert_eq!(sandbox.summary()[0]["bare_windows"], 2);
}

#[test]
fn a_capture_of_every_pixel_at_alpha_zero_is_gone_and_neither_bare_nor_other() {
    // An earlier scan called three of these bare, when a window that had already gone is
    // not a window shown bare: this is why the scan sorts them apart.
    let sandbox = Sandbox::new("scan-gone");
    let folder = sandbox.good_run("run-1", 2);
    let strays = ["w1-0.65-0007.png", "w1-0.70-0008.png", "w2-0.65-0009.png"];
    for name in strays {
        fs::write(folder.join(name), transparent_picture()).expect("a capture");
    }
    let run = sandbox.scan_runs(2);
    assert_eq!(run.code(), Some(0), "a run of those holds: {}", run.say());
    assert_eq!(
        run.runs_line(),
        "runs=1 of 1 closes_seen=2 captures=9 bare=0 bare_closes=0 gone=3 other=0 spare_drops=6",
        "{}",
        run.say()
    );
    assert!(run.lines("bare: ").is_empty() && run.lines("other: ").is_empty());
    let summary = sandbox.summary();
    let listed: Vec<String> = strays
        .iter()
        .map(|name| folder.join(name).display().to_string())
        .collect();
    assert_eq!(summary[0]["gone"], json!(listed));
    assert_eq!(summary[0]["bare"], json!([]));
    assert_eq!(summary[0]["other"], json!([]));
}

#[test]
fn a_picture_transparent_at_its_corners_and_opaque_inside_is_not_gone_and_a_flat_interior_is_bare()
{
    let sandbox = Sandbox::new("scan-corners");
    let folder = sandbox.good_run("run-1", 2);
    fs::write(folder.join("w2-0.65-0007.png"), bare_window_picture()).expect("a capture");
    let run = sandbox.scan_runs(2);
    assert_eq!(
        run.runs_line(),
        "runs=1 of 1 closes_seen=2 captures=7 bare=1 bare_closes=1 gone=0 other=0 spare_drops=6",
        "{}",
        run.say()
    );
}

#[test]
fn a_flat_capture_at_half_alpha_is_other() {
    let sandbox = Sandbox::new("scan-half");
    let folder = sandbox.good_run("run-1", 2);
    let name = "w2-0.65-0007.png";
    fs::write(
        folder.join(name),
        flat_picture([GREY[0], GREY[1], GREY[2], 128]),
    )
    .expect("a capture");
    let run = sandbox.scan_runs(2);
    assert_eq!(
        run.runs_line(),
        "runs=1 of 1 closes_seen=2 captures=7 bare=0 bare_closes=0 gone=0 other=1 spare_drops=6",
        "{}",
        run.say()
    );
    assert_eq!(
        run.line("other: "),
        format!("other: {}", folder.join(name).display())
    );
}

#[test]
fn a_capture_far_from_the_median_that_is_not_flat_is_other_and_named() {
    let sandbox = Sandbox::new("scan-not-flat");
    let folder = sandbox.good_run("run-1", 2);
    let name = "w2-0.65-0007.png";
    fs::write(folder.join(name), taller_picture()).expect("a capture");
    let run = sandbox.scan_runs(2);
    assert_eq!(
        run.runs_line(),
        "runs=1 of 1 closes_seen=2 captures=7 bare=0 bare_closes=0 gone=0 other=1 spare_drops=6",
        "{}",
        run.say()
    );
    assert_eq!(
        run.line("other: "),
        format!("other: {}", folder.join(name).display())
    );
}

#[test]
fn a_damaged_picture_is_other_and_the_scan_does_not_crash() {
    let sandbox = Sandbox::new("scan-damaged");
    let folder = sandbox.good_run("run-1", 2);
    fs::write(folder.join("w2-0.65-0007.png"), damaged_picture()).expect("a capture");
    fs::write(folder.join("w2-0.70-0008.png"), b"").expect("an empty capture");
    let run = sandbox.scan_runs(2);
    assert_eq!(
        run.runs_line(),
        "runs=1 of 1 closes_seen=2 captures=8 bare=0 bare_closes=0 gone=0 other=2 spare_drops=6",
        "{}",
        run.say()
    );
    assert!(!run.stderr().contains("Traceback"), "{}", run.stderr());
    assert_eq!(run.lines("other: ").len(), 2);
}

#[test]
fn an_8_bit_rgb_picture_with_no_alpha_that_is_flat_is_bare_and_never_gone() {
    let sandbox = Sandbox::new("scan-rgb");
    let folder = sandbox.good_run("run-1", 2);
    fs::write(folder.join("w2-0.65-0007.png"), rgb_picture()).expect("a capture");
    let run = sandbox.scan_runs(2);
    assert_eq!(
        run.runs_line(),
        "runs=1 of 1 closes_seen=2 captures=7 bare=1 bare_closes=1 gone=0 other=0 spare_drops=6",
        "{}",
        run.say()
    );
}

#[test]
fn each_spoil_by_itself_exits_one_names_its_reason_and_does_not_count_the_run() {
    // What is wrong with the run's folder, and the reason the scan gives.
    type Case = (&'static str, fn(&Path), &'static str);
    let reasons: [Case; 11] = [
        (
            "an exit status above 0",
            |run| {
                fs::write(run.join("exit"), "1\n").unwrap();
            },
            "exit 1",
        ),
        (
            "no exit file",
            |run| {
                fs::remove_file(run.join("exit")).unwrap();
            },
            "exit None",
        ),
        (
            "no report",
            |run| {
                fs::remove_file(run.join("report.json")).unwrap();
            },
            "no report",
        ),
        (
            "a drive that did not finish",
            |run| {
                change_report(run, |report| report["finished"] = json!(false));
            },
            "finished=False",
        ),
        (
            "too short",
            |run| {
                change_report(run, |report| report["elapsed_s"] = json!(5.5));
            },
            "elapsed_s=5.5 against 2 closes",
        ),
        (
            "too long",
            |run| {
                change_report(run, |report| report["elapsed_s"] = json!(10.75));
            },
            "elapsed_s=10.75 against 2 closes",
        ),
        (
            "frames unseen",
            |run| {
                change_report(run, |report| report["unseen_frames"] = json!(3));
            },
            "unseen_frames=3: the window was covered",
        ),
        (
            "another number of windows",
            |run| {
                change_report(run, |held| held["opens"] = report(3)["opens"].clone());
            },
            "3 windows, asked for 2",
        ),
        (
            "a painter other than metal",
            |run| {
                change_report(run, |report| report["opens"][1]["renderer"] = json!("glow"));
            },
            "painters ['glow', 'metal']",
        ),
        (
            "the window's own captures flat",
            |run| {
                for entry in fs::read_dir(run).unwrap() {
                    let path = entry.unwrap().path();
                    if path.extension().is_some_and(|extension| extension == "png") {
                        fs::write(path, flat_picture(GREY)).unwrap();
                    }
                }
            },
            "the window's own captures are flat or transparent",
        ),
        (
            "the window's own captures transparent",
            |run| {
                for entry in fs::read_dir(run).unwrap() {
                    let path = entry.unwrap().path();
                    if path.extension().is_some_and(|extension| extension == "png") {
                        fs::write(path, transparent_picture()).unwrap();
                    }
                }
            },
            "the window's own captures are flat or transparent",
        ),
    ];
    for (what, spoil, reason) in reasons {
        let sandbox = Sandbox::new("scan-spoiled");
        sandbox.good_run("run-1", 2);
        let spoiled = sandbox.good_run("run-2", 2);
        spoil(&spoiled);
        let run = sandbox.scan_runs(2);
        assert_eq!(run.code(), Some(1), "{what}: {}", run.say());
        // Only the held run counts toward the windows seen.
        assert!(
            run.runs_line()
                .starts_with("runs=1 of 2 closes_seen=2 captures=6 "),
            "{what}: {}",
            run.say()
        );
        assert_eq!(
            run.lines("spoiled: "),
            [format!("spoiled: run-2: {reason}")],
            "{what}"
        );
    }
}

/// Changes the report of a run folder.
fn change_report(run: &Path, change: impl FnOnce(&mut Value)) {
    let file = run.join("report.json");
    let mut report: Value =
        serde_json::from_str(&fs::read_to_string(&file).expect("a report")).expect("JSON");
    change(&mut report);
    fs::write(&file, report.to_string()).expect("a report");
}

#[test]
fn captures_of_fewer_windows_than_closes_spoil_a_run_and_say_both_numbers() {
    let sandbox = Sandbox::new("scan-windows");
    sandbox.good_run("run-1", 2);
    let short = sandbox.good_run("run-2", 2);
    // Every capture of the second window is lost: one window of two closes.
    for n in 1..=3 {
        fs::remove_file(short.join(format!("w2-0.60-{n:04}.png"))).expect("a capture");
    }
    let run = sandbox.scan_runs(2);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert_eq!(
        run.lines("spoiled: "),
        ["spoiled: run-2: captures saw 1 windows of 2 closes"]
    );
    assert!(
        run.runs_line().starts_with("runs=1 of 2 closes_seen=2 "),
        "{}",
        run.say()
    );
}

#[test]
fn a_capture_of_a_window_that_had_gone_still_counts_the_window_as_seen() {
    let sandbox = Sandbox::new("scan-gone-seen");
    let folder = sandbox.good_run("run-1", 2);
    // The second window's one capture came after it left: every pixel transparent. No flash, and seen.
    for n in 2..=3 {
        fs::remove_file(folder.join(format!("w2-0.60-{n:04}.png"))).expect("a capture");
    }
    fs::write(folder.join("w2-0.60-0001.png"), transparent_picture()).expect("a capture");
    let run = sandbox.scan_runs(2);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert_eq!(
        run.runs_line(),
        "runs=1 of 1 closes_seen=2 captures=4 bare=0 bare_closes=0 gone=1 other=0 spare_drops=6"
    );
}

#[test]
fn captures_of_more_windows_than_closes_do_not_spoil_a_run_for_that() {
    let sandbox = Sandbox::new("scan-more-windows");
    let folder = sandbox.good_run("run-1", 3);
    change_report(&folder, |held| *held = report(2));
    let run = sandbox.scan_runs(2);
    assert_eq!(run.code(), Some(0), "{}", run.say());
    assert!(
        run.runs_line().starts_with("runs=1 of 1 closes_seen=3 "),
        "{}",
        run.say()
    );
}

#[test]
fn a_run_without_a_count_of_the_pauses_blocks_is_spoiled_in_every_way_it_can_lack_one() {
    // The report holds the count at `hold.spare_drops`, and nowhere else: a glow run's reads 0.
    type Change = fn(&mut Value);
    let cases: [(&str, Change, &str); 9] = [
        ("zero", |held| held["hold"]["spare_drops"] = json!(0), "0"),
        (
            "null",
            |held| held["hold"]["spare_drops"] = json!(null),
            "None",
        ),
        (
            "absent from the hold",
            |held| {
                held["hold"]
                    .as_object_mut()
                    .expect("a hold")
                    .remove("spare_drops");
            },
            "None",
        ),
        (
            "no hold",
            |held| {
                held.as_object_mut().expect("a report").remove("hold");
            },
            "None",
        ),
        (
            "a hold that is not a dict",
            |held| held["hold"] = json!([6]),
            "None",
        ),
        (
            "at the top level only",
            |held| {
                held.as_object_mut().expect("a report").remove("hold");
                held["spare_drops"] = json!(6);
            },
            "None",
        ),
        (
            "a string",
            |held| held["hold"]["spare_drops"] = json!("6"),
            "6",
        ),
        (
            "not a whole number",
            |held| held["hold"]["spare_drops"] = json!(6.5),
            "6.5",
        ),
        (
            "negative",
            |held| held["hold"]["spare_drops"] = json!(-1),
            "-1",
        ),
    ];
    for (what, change, shown) in cases {
        let sandbox = Sandbox::new("scan-spare-drops");
        sandbox.good_run("run-1", 2);
        let lacking = sandbox.good_run("run-2", 2);
        change_report(&lacking, change);
        let run = sandbox.scan_runs(2);
        assert_eq!(run.code(), Some(1), "{what}: {}", run.say());
        assert_eq!(
            run.lines("spoiled: "),
            [format!(
                "spoiled: run-2: spare_drops={shown}: no pause's block is seen to have come"
            )],
            "{what}"
        );
        assert!(
            run.runs_line().starts_with("runs=1 of 2 "),
            "{what}: {}",
            run.say()
        );
    }
}

#[test]
fn the_runs_line_adds_the_pauses_blocks_of_the_runs_that_held_and_not_of_a_spoiled_one() {
    let sandbox = Sandbox::new("scan-spare-sum");
    sandbox.good_run("run-1", 2);
    let second = sandbox.good_run("run-2", 2);
    change_report(&second, |held| held["hold"]["spare_drops"] = json!(9));
    let spoiled = sandbox.good_run("run-3", 2);
    change_report(&spoiled, |held| held["hold"]["spare_drops"] = json!(4));
    fs::write(spoiled.join("exit"), "1\n").expect("an exit");
    let run = sandbox.scan_runs(2);
    assert_eq!(run.code(), Some(1), "{}", run.say());
    assert!(
        run.runs_line().ends_with(" spare_drops=15"),
        "{}",
        run.say()
    );
    // The summary keeps each run's own count, and none where the report had none.
    let counts: Vec<Value> = sandbox
        .summary()
        .as_array()
        .expect("runs")
        .iter()
        .map(|run| run["spare_drops"].clone())
        .collect();
    assert_eq!(counts, [json!(6), json!(9), json!(4)]);
}

#[test]
fn a_healthy_run_of_many_closes_is_not_too_long_and_one_that_takes_longer_a_close_is() {
    for closes in [40_u32, 64] {
        let sandbox = Sandbox::new("scan-long");
        let n = closes as usize;
        // Cycles of 3.125 s, as the drives read: not a fixed slack past a run's own cycles.
        let held = sandbox.good_run("run-1", n);
        let healthy = f64::from(closes) * 3.125;
        change_report(&held, |held| held["elapsed_s"] = json!(healthy));
        // Cycles of 3.4 s are slower than any a drive has taken.
        let slow = sandbox.good_run("run-2", n);
        let slower = f64::from(closes) * 3.4;
        change_report(&slow, |held| held["elapsed_s"] = json!(slower));
        let run = sandbox.scan_runs(n);
        assert_eq!(run.code(), Some(1), "{closes}: {}", run.say());
        assert_eq!(
            run.lines("spoiled: "),
            [format!(
                "spoiled: run-2: elapsed_s={slower:?} against {closes} closes"
            )],
            "{closes}"
        );
    }
}

#[test]
fn a_damaged_report_or_exit_reads_spoiled_with_a_reason_and_the_runs_beside_it_still_print() {
    type Damage = fn(&Path);
    const REPORT: &str = "report.json is damaged";
    const EXIT: &str = "exit unreadable";
    let cases: [(&str, Damage, &str); 10] = [
        (
            "a report that is not JSON",
            |run| fs::write(run.join("report.json"), "{\"finished\": tr").expect("a report"),
            REPORT,
        ),
        (
            "a report that is not UTF-8",
            |run| fs::write(run.join("report.json"), [0xff, 0xfe, 0x7b]).expect("a report"),
            REPORT,
        ),
        (
            "a report that is a list",
            |run| fs::write(run.join("report.json"), "[1, 2]").expect("a report"),
            REPORT,
        ),
        (
            "a report that is a folder",
            |run| {
                fs::remove_file(run.join("report.json")).expect("a report");
                fs::create_dir(run.join("report.json")).expect("a folder");
            },
            REPORT,
        ),
        (
            "opens that is a number",
            |run| change_report(run, |held| held["opens"] = json!(2)),
            REPORT,
        ),
        (
            "an open with no renderer",
            |run| change_report(run, |held| held["opens"][1] = json!({})),
            REPORT,
        ),
        (
            "a renderer that cannot be hashed",
            |run| change_report(run, |held| held["opens"][1]["renderer"] = json!(["metal"])),
            REPORT,
        ),
        (
            "an exit that is not a number",
            |run| fs::write(run.join("exit"), "done\n").expect("an exit"),
            EXIT,
        ),
        (
            "an exit that is not UTF-8",
            |run| fs::write(run.join("exit"), [0xff, 0xfe]).expect("an exit"),
            EXIT,
        ),
        (
            "an exit that is a folder",
            |run| {
                fs::remove_file(run.join("exit")).expect("an exit");
                fs::create_dir(run.join("exit")).expect("a folder");
            },
            EXIT,
        ),
    ];
    for (what, damage, reason) in cases {
        let sandbox = Sandbox::new("scan-damaged-run");
        sandbox.good_run("run-1", 2);
        let damaged = sandbox.good_run("run-2", 2);
        sandbox.good_run("run-3", 2);
        damage(&damaged);
        let run = sandbox.scan_runs(2);
        // Exit as for a spoiled run, and no traceback.
        assert_eq!(run.code(), Some(1), "{what}: {}", run.say());
        assert_eq!(run.stderr(), "", "{what}");
        assert_eq!(
            run.lines("spoiled: "),
            [format!("spoiled: run-2: {reason}")],
            "{what}"
        );
        // The runs on either side of it are read and printed, and so is where the pictures are.
        assert!(
            run.runs_line()
                .starts_with("runs=2 of 3 closes_seen=4 captures=12 "),
            "{what}: {}",
            run.say()
        );
        assert_eq!(run.lines("pictures: ").len(), 1, "{what}");
        let names: Vec<Value> = sandbox
            .summary()
            .as_array()
            .expect("runs")
            .iter()
            .map(|run| run["run"].clone())
            .collect();
        assert_eq!(names, ["run-1", "run-2", "run-3"], "{what}");
    }
}

#[test]
fn a_run_with_no_capture_exits_two_and_so_does_an_empty_folder() {
    let sandbox = Sandbox::new("scan-no-capture");
    let folder = sandbox.good_run("run-1", 2);
    for entry in fs::read_dir(&folder).expect("the run") {
        let path = entry.expect("an entry").path();
        if path.extension().is_some_and(|extension| extension == "png") {
            fs::remove_file(path).expect("removed");
        }
    }
    let run = sandbox.scan_runs(2);
    assert_eq!(run.code(), Some(2), "{}", run.say());
    assert!(
        run.stderr().contains("a run holds no capture"),
        "{}",
        run.stderr()
    );
    assert_eq!(
        run.line("runs="),
        "runs=0 of 1 closes_seen=0 captures=0 bare=0 bare_closes=0 gone=0 other=0 spare_drops=0"
    );

    // One run with pictures beside it does not make up for it.
    sandbox.good_run("run-2", 2);
    assert_eq!(sandbox.scan_runs(2).code(), Some(2));

    let empty = Sandbox::new("scan-empty");
    let run = empty.scan_runs(2);
    assert_eq!(run.code(), Some(2), "{}", run.say());
    assert_eq!(
        run.runs_line(),
        "runs=0 of 0 closes_seen=0 captures=0 bare=0 bare_closes=0 gone=0 other=0 spare_drops=0"
    );
}

#[test]
fn the_scan_with_the_wrong_number_of_arguments_says_its_usage() {
    let sandbox = Sandbox::new("scan-usage");
    for args in [&[][..], &["one"][..], &["one", "2", "three"][..]] {
        let run = sandbox.scan(args);
        assert_eq!(run.code(), Some(1), "{args:?}");
        assert!(
            run.stderr()
                .starts_with("Sorts the captures of a run of close-captures.sh"),
            "{}",
            run.stderr()
        );
        assert!(
            run.stderr()
                .contains("usage: close-scan.py <folder> <closes a run>")
        );
    }
}

#[test]
fn neither_the_scan_nor_the_script_that_runs_it_writes_bytecode() {
    let sandbox = Sandbox::new("bytecode");
    // The scan on its own copy, in a sandbox.
    sandbox.good_run("run-1", 2);
    let scan = sandbox.scan_runs(2);
    assert_eq!(scan.code(), Some(0), "{}", scan.say());
    sandbox.assert_no_bytecode_of_the_scripts();

    // And the repository's own, through the shell script, which runs it last. No window is found, so
    // this takes no capture; the scan still imports `tear-scan.py` before it finds there are none.
    sandbox.capturing(1);
    sandbox.stand_in("winid", NO_WINDOW);
    let run = sandbox.run_script(&[Path::new("label"), &sandbox.work()], &[]);
    assert_eq!(run.code(), Some(2), "{}", run.say());
    assert!(
        run.stderr()
            .contains("close-scan.py: a run holds no capture"),
        "the scan ran: {}",
        run.say()
    );
    sandbox.assert_no_bytecode_of_the_scripts();
}

// ---- tear-scan.py, whose dispatch moved into `main()` ----

#[test]
fn tear_scan_run_as_a_script_still_says_its_usage_and_exits_one() {
    for args in [&[][..], &["--sum"][..], &["-x"][..], &["a", "b"][..]] {
        let output = Command::new("python3")
            .arg(TEAR)
            .args(args)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .expect("python3 runs");
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let said = String::from_utf8_lossy(&output.stderr);
        assert!(
            said.starts_with(
                "Counts the tears in the window captures of one run of tear-captures.sh"
            ),
            "{args:?}: {said}"
        );
        assert!(said.contains("usage: tear-scan.py <folder>"), "{args:?}");
    }
}

#[test]
fn importing_tear_scan_runs_nothing() {
    // `close-scan.py` loads it the same way. A dispatch at the top of the file would read this
    // process's arguments, find none it knows and exit with the usage.
    let code = r#"
import importlib.util
import sys

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("tear_scan", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
print("imported", callable(module.main))
"#;
    let output = Command::new("python3")
        .args(["-I", "-c", code, TEAR, "--sum"])
        .output()
        .expect("python3 runs");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "imported True",
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success());
}

/// A run folder of `tear-captures.sh` that scans clean: three flat captures of each of two greys, an exit
/// of 0 and a 20 s report that finished with nothing covered.
fn tear_run(sandbox: &Sandbox, name: &str) -> PathBuf {
    let folder = sandbox.pictures().join(name);
    fs::create_dir_all(&folder).expect("a run folder");
    for n in 1..=6u8 {
        let grey = if n % 2 == 0 { 100 } else { 96 };
        fs::write(
            folder.join(format!("{n:04}.png")),
            flat_picture([grey, grey, grey, 255]),
        )
        .expect("a capture");
    }
    fs::write(folder.join("exit"), "0\n").expect("an exit status");
    let report = json!({ "finished": true, "elapsed_s": 20.7, "unseen_frames": 0 });
    fs::write(folder.join("report.json"), report.to_string()).expect("a report");
    folder
}

/// `tear-scan.py` on its own copy in the sandbox, run on `args`.
fn tear_scan(sandbox: &Sandbox, args: &[&Path]) -> Script {
    let output = Command::new("python3")
        .arg(sandbox.root.join("scripts/tear-scan.py"))
        .args(args)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3 runs");
    Script(output)
}

#[test]
fn a_damaged_report_or_exit_spoils_its_tear_run_and_the_runs_beside_it_still_scan_and_sum() {
    type Damage = fn(&Path);
    const REPORT: &str = "report.json is damaged";
    const EXIT: &str = "exit unreadable";
    let cases: [(&str, Damage, &str); 7] = [
        (
            "a report that is not JSON",
            |run| fs::write(run.join("report.json"), "{\"finished\": tr").expect("a report"),
            REPORT,
        ),
        (
            "a report that is not UTF-8",
            |run| fs::write(run.join("report.json"), [0xff, 0xfe, 0x7b]).expect("a report"),
            REPORT,
        ),
        (
            "a report that is a list",
            |run| fs::write(run.join("report.json"), "[1, 2]").expect("a report"),
            REPORT,
        ),
        (
            "a report that is a folder",
            |run| {
                fs::remove_file(run.join("report.json")).expect("a report");
                fs::create_dir(run.join("report.json")).expect("a folder");
            },
            REPORT,
        ),
        (
            "an exit that is not a number",
            |run| fs::write(run.join("exit"), "done\n").expect("an exit"),
            EXIT,
        ),
        (
            "an exit that is not UTF-8",
            |run| fs::write(run.join("exit"), [0xff, 0xfe]).expect("an exit"),
            EXIT,
        ),
        (
            "an exit that is a folder",
            |run| {
                fs::remove_file(run.join("exit")).expect("an exit");
                fs::create_dir(run.join("exit")).expect("a folder");
            },
            EXIT,
        ),
    ];
    for (what, damage, reason) in cases {
        let sandbox = Sandbox::new("tear-damaged-run");
        let runs: Vec<PathBuf> = ["run-1", "run-2", "run-3"]
            .iter()
            .map(|name| tear_run(&sandbox, name))
            .collect();
        damage(&runs[1]);

        // Each run is scanned on its own, as `tear-captures.sh` does: the damaged one too, with no traceback.
        for (index, run) in runs.iter().enumerate() {
            let scan = tear_scan(&sandbox, &[run]);
            assert_eq!(
                scan.code(),
                Some(0),
                "{what}, run {}: {}",
                index + 1,
                scan.say()
            );
            assert_eq!(scan.stderr(), "", "{what}, run {}", index + 1);
            let line = scan.stdout();
            assert!(
                line.starts_with(&format!("run-{} tears=0 settled=6 captures=6 ", index + 1)),
                "{what}: {line}"
            );
            if index == 1 {
                assert!(
                    line.contains(&format!("SPOILED ({reason}): left out of the total")),
                    "{what}: {line}"
                );
            } else {
                assert!(!line.contains("SPOILED"), "{what}: {line}");
            }
        }

        // The sum prints the three and totals the two that held.
        let mut args = vec![Path::new("--sum")];
        args.extend(runs.iter().map(PathBuf::as_path));
        let sum = tear_scan(&sandbox, &args);
        assert_eq!(sum.code(), Some(0), "{what}: {}", sum.say());
        assert_eq!(sum.stderr(), "", "{what}");
        assert_eq!(sum.stdout().lines().count(), 4, "{what}: {}", sum.say());
        assert_eq!(
            sum.line("TOTAL "),
            "TOTAL tears=0 settled=12 runs=2 of 3",
            "{what}"
        );
    }
}
