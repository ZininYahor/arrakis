#!/bin/sh
# Does a failed Metal painter leave the app alive, its menu-bar item up, and its next window on glow?
# This reads it on a real window, which no `cargo test` opens. It takes the demo build, makes each failure
# with ARRAKIS_METAL_FAIL in one drive, and reads the drive's report:
#
#   ARRAKIS_METAL_FAIL  drive            the report must hold
#   device              open-close, 9 s  no fallback; 3 windows
#   pipeline, surface,  open-close, 9 s  one fallback; 3 windows; the window that replaced the failed one
#   stall                                showed within 1000 ms of the failure
#   metallib            open-close, 9 s  no fallback; 3 windows, every one metal's, its shader from the
#                                        source; and in the log, one warning that the file does not serve
#   frame               open-night, 9 s  one fallback; 1 window, shown within 1000 ms of the failure; the
#                                        engine checked the tinted displays after it
#   rest                open-idle, 5 s   one fallback; 1 window, shown within 1000 ms of the failure
#
# And in every drive: it finished by itself and in its time (the report's `spoiled` is null: no cycle
# broke off, and its `elapsed_s` is in the driver's bound, `Scenario::bound` in src/drive.rs), every window is glow's
# (metal's under `metallib`, the one failure that stays on Metal), each fallback's `why` names the
# failure the run made, the app held its
# menu-bar item at each window's first frame, and the engine's thread, told to stop after the fallback,
# gave the displays back (`screen.restores`). Only `frame` runs tinted, so only it can read
# `screen.checks`: the others run by day, and the engine checks a tinted screen alone.
#
# The word `plate` is no drive and has no report: it is `--demo-shot` of the Scale plate under
# ARRAKIS_METAL_FAIL=frame. A plate has no `App` to reopen on glow, so it must end by itself, within
# PAINTER_FALLBACK_PLATE_LIMIT seconds (20 by default), with a nonzero exit, a message that says why, and
# no picture.
#
# What no row can read: a preview the engine held and let go, as the report only counts what it paints,
# and no ARRAKIS_METAL_FAIL fails a frame in a drag. `src/app.rs`'s tests read that, on the engine's channel.
#
# A drive runs on NullScreen, so the real displays and the settings stay as they are, but THE WINDOW OPENS
# ON THE OWNER'S SCREEN and takes the focus: 7 drives and a plate, about 60 s. On a Mac with no menu bar,
# such as a session with no login, `wire` logs "no menu-bar item", and the `tray` check says so here.
#
# usage: painter-fallback.sh [folder]
#   folder  takes each run's report and log, in a subfolder per failure; a new one under $TMPDIR by
#           default. The runs' HOME and TMPDIR are under $TMPDIR/arrakis-painter-fallback.
# PAINTER_FALLBACK_BIN names a demo build to use; by default it runs `cargo build --features demo`.
# PAINTER_FALLBACK_ONLY names the failures to run, separated by spaces; by default all eight.
# Prints each run's result, its `elapsed_s` and, for a drive, its `unseen_frames`, and exits 0 when every run
# held and 1 otherwise. A drive whose `unseen_frames` is above 0 had its window covered: the line says so.
# A covered Metal window does not reopen, so its run also says that window is metal's: run it again uncovered.
set -u

fail() {
    echo "painter-fallback.sh: $*" >&2
    exit 1
}

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK="${TMPDIR:-/tmp}/arrakis-painter-fallback"
OUT="${1:-$WORK/$(date +%Y%m%d-%H%M%S)}"
FAILURES="${PAINTER_FALLBACK_ONLY:-device pipeline surface frame stall metallib rest plate}"
PLATE_LIMIT="${PAINTER_FALLBACK_PLATE_LIMIT:-20}"

BIN="${PAINTER_FALLBACK_BIN:-}"
if [ -z "$BIN" ]; then
    (cd "$ROOT" && cargo build --features demo) || fail "the demo build failed"
    # Where cargo says, as in scripts/bundle.sh: a config file can move it too.
    target=$(cd "$ROOT" && cargo metadata --format-version 1 --no-deps |
        plutil -extract target_directory raw -o - -) ||
        fail "cargo metadata did not give the target directory"
    BIN="$target/debug/arrakis"
fi
[ -x "$BIN" ] || fail "no $BIN"
mkdir -p "$OUT" "$WORK/home" "$WORK/tmp" || fail "cannot write to $OUT or $WORK"

# Reads the report $1 of the failure $2, and its log $3. Prints one line, and exits 1 at anything the table
# above denies. The warning's words are the painter's own (`METALLIB_REFUSED`, painter.rs).
read_report() {
    python3 - "$1" "$2" "$3" <<'PY'
import json
import sys

path, failure, log_path = sys.argv[1:4]
with open(path, encoding="utf-8") as file:
    report = json.load(file)

opens = report["opens"]
fallbacks = report["fallbacks"]
wrong = []
on_metal = failure == "metallib"

if not report["finished"]:
    wrong.append("it did not finish by itself")
# The driver flags a drive out of its time, or one whose cycle broke off (`Report::spoiled`), with its reason as a
# sentence. A report from a driver built before the key has none, and counts as in its time.
spoiled = report.get("spoiled")
if spoiled is not None:
    wrong.append(f"spoiled: {spoiled}")
for index, window in enumerate(opens):
    if window["renderer"] != ("metal" if on_metal else "glow"):
        wrong.append(f"window {index} is {window['renderer']}'s")
    if on_metal and window.get("shader") != "source":
        wrong.append(f"window {index}'s shader is from {window.get('shader')!r}")
    if not window["tray"]:
        wrong.append(f"window {index} has no menu-bar item")

warned = ""
if on_metal:
    with open(log_path, encoding="utf-8", errors="replace") as log:
        warnings = sum("shader.metallib does not serve" in line for line in log)
    if warnings != 1:
        wrong.append(f"{warnings} warnings that the file does not serve, not 1")
    warned = f" warnings={warnings}"

wanted_opens = 1 if failure in ("rest", "frame") else 3
wanted_fallbacks = 0 if failure in ("device", "metallib") else 1
if len(opens) != wanted_opens:
    wrong.append(f"{len(opens)} windows, not {wanted_opens}")
if len(fallbacks) != wanted_fallbacks:
    wrong.append(f"{len(fallbacks)} fallbacks, not {wanted_fallbacks}")
if report["screen"]["restores"] <= 0:
    wrong.append("the engine did not give the displays back")

first_frame = ""
for fallback in fallbacks:
    if fallback["why"] != f"ARRAKIS_METAL_FAIL={failure}":
        wrong.append(f"the fallback's why is {fallback['why']!r}")
    if failure == "frame" and report["screen"]["checks"] <= fallback["screen_checks"]:
        wrong.append("the engine checked nothing after the fallback")
    at_open = fallback["at_open"]
    shown = opens[at_open]["to_first_frame_ms"] if at_open < len(opens) else None
    if shown is None or shown > 1000:
        wrong.append(f"the window after the failure showed at {shown} ms")
    else:
        first_frame = f" shown_after_failure_ms={shown:.0f} why={fallback['why']!r}"

unseen = report["unseen_frames"]
covered = " COVERED" if unseen > 0 else ""
verdict = "; ".join(wrong) if wrong else "held"
print(
    f"{failure}: {verdict}: elapsed_s={report['elapsed_s']:.2f} unseen_frames={unseen}{covered} "
    f"windows={len(opens)} fallbacks={len(fallbacks)}{warned}{first_frame}"
)
sys.exit(1 if wrong else 0)
PY
}

# Runs a plate on a failed painter in the folder $1. Prints one line, and exits 1 at anything the table
# above denies. Python holds the limit, as `timeout` is not on a stock Mac. A run past it is killed, and
# as a shot touches no display, that leaves nothing to give back.
run_plate() {
    python3 - "$BIN" "$1" "$WORK" "$PLATE_LIMIT" <<'PY'
import os
import subprocess
import sys
import time

binary, folder, work, limit = sys.argv[1:5]
picture = os.path.join(folder, "plate.png")
log_path = os.path.join(folder, "app.log")
env = dict(
    os.environ,
    ARRAKIS_METAL_FAIL="frame",
    HOME=os.path.join(work, "home"),
    TMPDIR=os.path.join(work, "tmp"),
)
wrong = []
started = time.monotonic()
with open(log_path, "wb") as log:
    try:
        code = subprocess.run(
            [binary, "--demo-shot", picture, "--demo-scene", "scale"],
            stdout=log,
            stderr=log,
            env=env,
            timeout=float(limit),
        ).returncode
    except subprocess.TimeoutExpired:
        code = None
        wrong.append(f"it did not end within {limit} s")
elapsed = time.monotonic() - started
if code == 0:
    wrong.append("it exited 0")
with open(log_path, encoding="utf-8", errors="replace") as log:
    said = log.read()
if code is not None and "this plate has no picture: ARRAKIS_METAL_FAIL=frame" not in said:
    wrong.append(f"it did not say why: see {log_path}")
if os.path.exists(picture):
    wrong.append("it left a picture")
verdict = "; ".join(wrong) if wrong else "held"
print(f"plate: {verdict}: elapsed_s={elapsed:.2f} exit={code}")
sys.exit(1 if wrong else 0)
PY
}

misses=0
for failure in $FAILURES; do
    case "$failure" in
    device | pipeline | surface | stall | metallib)
        scenario=open-close seconds=9
        ;;
    frame)
        scenario=open-night seconds=9
        ;;
    rest)
        scenario=open-idle seconds=5
        ;;
    plate)
        dir="$OUT/plate"
        mkdir -p "$dir" || fail "cannot write to $dir"
        rm -f "$dir/plate.png" "$dir/app.log"
        run_plate "$dir" || misses=$((misses + 1))
        continue
        ;;
    *)
        fail "no such failure: $failure"
        ;;
    esac
    dir="$OUT/$failure"
    mkdir -p "$dir" || fail "cannot write to $dir"
    rm -f "$dir/report.json" "$dir/app.log"
    # The app's exit status says little here: a run that the driver had to end also writes its report,
    # with `finished` false, and one that wrote none fails below.
    ARRAKIS_METAL_FAIL="$failure" HOME="$WORK/home" TMPDIR="$WORK/tmp" "$BIN" \
        --demo-drive "$scenario" --demo-seconds "$seconds" --demo-report "$dir/report.json" \
        >"$dir/app.log" 2>&1
    if [ ! -s "$dir/report.json" ]; then
        echo "$failure: no report: see $dir/app.log"
        misses=$((misses + 1))
    elif ! read_report "$dir/report.json" "$failure" "$dir/app.log"; then
        misses=$((misses + 1))
    fi
done

if [ "$misses" -gt 0 ]; then
    echo "painter-fallback.sh: $misses runs did not hold: see $OUT" >&2
    exit 1
fi
echo "painter-fallback.sh: every run held: $OUT"
