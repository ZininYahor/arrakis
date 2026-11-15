#!/usr/bin/env python3
"""A drag's CPU energy and core placement, by arm, from proc_pid_rusage, on NullScreen.

Why: the limits on frame time and top's POWER say one thing and the energy another. On a quiet machine the
renderers' frames retire fewer instructions but take 2.4 to 3 times the CPU time (p90), because the
scheduler keeps about four fifths of it on the efficiency cores. This script reads, for 20 s inside a 30 s
`--demo-drive drag`, the process's CPU time, the share of it on performance cores, instructions,
cycles and CPU energy (RUSAGE_INFO_V6). It counts the CPU only: no GPU and no window server.

THE WINDOW TAKES THE FOCUS for the 30 s of the drive, as every `--demo-drive` scenario with a window does
(`open-*`, `drag` and `start`: `src/drive.rs`, `src/app.rs`, `fastframe-tray`'s `activate()`). Run it only
when you mean to.

usage: energy-drag.py <out.json> <arm,arm,...>
  An arm is a name from `arm_roots` below, optionally with "+flag" (MallocSpaceEfficient=1). List an arm
  once for each run; the runs go in the order given, and the medians are printed at the end.
  $TMPDIR must hold energytool (scripts/measure/energytool.c says how to build it) and, for each
  arm, a root: a TMPDIR for which
      CARGO_TARGET_DIR=<root>/arrakis-measure/drive cargo build --release --features demo
  was run on that arm's code (the script makes home/ and tmp/ beside the build when they are not
  there, as scripts/measure.sh lays them out).
  `arm_roots` names the night's roots: "cap" is main's code in $TMPDIR/m-drive, the others are built in
  $TMPDIR/arms/<name>. Change them to the arms in hand. "t1-cap" and "t1-metal" are the builds of the
  Metal painter's first prototype reading, in $TMPDIR/t1/<cap|metal>. The app inherits the environment,
  so `SOFT_SURFACES=2 energy-drag.py ... t1-metal+flag,...` runs two surfaces. "t2-metal" is the painter
  on its own display link, in $TMPDIR/t2/metal: the limits' first reading on that link. "final" is the
  final build, in $TMPDIR/arms/final. ENERGY_DRAG_ROOT_<NAME>=<root>
  points an arm at another root (an absolute path, such as an arm that build-arm.sh made), or adds one:
  <NAME> is the arm's name in capitals with "_" for "-", so ENERGY_DRAG_ROOT_FINAL=/x/arm-abc1234 sets
  "final" and ENERGY_DRAG_ROOT_BISECT_MID=/x/arm-def5678 adds "bisect-mid". The arm's drive build
  is then <root>/arrakis-measure/drive/release/arrakis, as build-arm.sh lays it out.
  Each row also keeps the machine's state, taken before the app starts and again once the drive has
  ended (a reading above 257 µJ a frame is reported together with it): whether `pmset -g assertions` shows
  `audio-out`, the load averages, and the five processes using the most CPU by `top`'s second sample.
  The second state waits for the drive to end,
  because `top -l 2 -s 1` takes about two seconds and its own load would sit in the row's `fps`,
  `p50` and `p90` if it ran while the drive still records; the counters' reads stay inside the drive.
  A call that fails leaves its part null and spoils nothing: neither a command that cannot run, nor
  one whose output is not UTF-8 (a cut name in `top`'s), is an error.
  An `ENERGY_DRAG_ROOT_*` that is not an absolute path ends the night before it starts when its arm is
  in the order, and is ignored when it is not.
  <out.json> is written after every run, so an interrupted night keeps what it measured.
  Each row keeps the report's `finished` and `elapsed_s` and the exit code. A run that did not finish,
  exited non-zero, took a drive time other than SECONDS plus up to 2 s (a stall, a sleep), lacks
  a figure, or saw its window covered or minimized (the report's `unseen_frames` above 0: eframe
  paints none of those frames, so they cost almost nothing and the per-frame figures read too good)
  is SPOILED: it is named on stderr, marked in the file and left out of the medians, which say how
  many runs they hold. Leave the drive's window uncovered. A report without `unseen_frames`, from an
  arm built before the key, is read as it is: it cannot be checked, its row says `unseen_frames:
  null`, and it is not spoiled for that.
Nothing else may be at work on the machine, and no load generator: the scheduler's choice of cores is
what it measures.
"""
import json
import os
import re
import statistics as st
import subprocess
import sys
import tempfile
import time
from pathlib import Path

SECONDS = 30  # the drive's length, `--demo-seconds`
READ_AFTER = 5  # seconds into the drive when the counters are first read
READ_FOR = 20  # seconds between the two reads
# A test holds SLACK equal to the driver's `TIMED_SLACK_S` (src/drive.rs), the bound of a timed drive.
SLACK = 2.0  # how far past SECONDS a healthy drive's elapsed_s may be (its start and its quit)
ROOT_PREFIX = "ENERGY_DRAG_ROOT_"
T = Path(os.environ["TMPDIR"])


def arm_roots(environ, order=()):
    """The arms' roots: the night's, then the ones the environment sets or adds (ENERGY_DRAG_ROOT_<NAME>).

    `order` is the night's arms, each with its "+flag" or without. A variable whose value is not an absolute
    path ends the run when its arm is in `order`; for an arm that is not, it is left out and nothing is said,
    as that arm is not used (a shell's leftover variable must not stop a night that never reads it)."""
    tmp = Path(environ["TMPDIR"])
    wanted = {arm.removesuffix("+flag") for arm in order}
    roots = {
        "cap": tmp / "m-drive",
        "metal": tmp / "arms/metal",
        "direct": tmp / "arms/direct",
        "release": tmp / "arms/release",
        "final": tmp / "arms/final",
        "t1-cap": tmp / "t1/cap",
        "t1-metal": tmp / "t1/metal",
        "t2-metal": tmp / "t2/metal",
    }
    for var, value in environ.items():
        if var.startswith(ROOT_PREFIX) and var != ROOT_PREFIX:
            arm = var.removeprefix(ROOT_PREFIX).lower().replace("_", "-")
            if not value.startswith("/"):
                if arm in wanted:
                    sys.exit(f"energy-drag.py: {var} must be an absolute path, and {value!r} is not")
                continue
            roots[arm] = Path(value)
    return roots


# Not a row's measure: what says whether the run was a good one.
NOT_MEASURES = ("arm", "exit", "finished", "elapsed_s", "unseen_frames", "spoiled", "machine")


def sh(*command):
    """The output of a command, or None when it cannot run, fails or takes over 30 s. Output that is not UTF-8
    (a name `top` cut in the middle of a character) is read with U+FFFD for the bad bytes, and is no error."""
    try:
        return subprocess.run(
            command, capture_output=True, text=True, errors="replace", timeout=30, check=True
        ).stdout
    except (OSError, ValueError, subprocess.SubprocessError):
        return None


def top_processes(text, count=5):
    """The first `count` rows of the last sample in `top -l 2 -n 6 -o cpu -stats pid,command,cpu`, as
    {pid, command, cpu}, without `top` itself (asked for one row more). top's first sample is the CPU time
    since each process started, so only the second counts. None without text."""
    if text is None:
        return None
    lines = text.splitlines()
    heads = [i for i, line in enumerate(lines) if line.split()[:1] == ["PID"]]
    rows = []
    for line in lines[heads[-1] + 1 :] if heads else []:
        parts = line.split()
        try:
            row = {"pid": int(parts[0]), "command": " ".join(parts[1:-1]), "cpu": float(parts[-1])}
        except (ValueError, IndexError):
            continue
        if row["command"] != "top":
            rows.append(row)
    return rows[:count]


def machine_state(runner=sh):
    """What else the machine is doing: `audio_out` (whether `pmset -g assertions` shows `audio-out`), `load`
    (the 1, 5 and 15 minute load averages) and `top` (the five processes using the most CPU). A part that
    cannot be read is None. Audio output moves the energy of a drag, and so does another state."""
    assertions = runner("pmset", "-g", "assertions")
    try:
        load = [round(x, 2) for x in os.getloadavg()]
    except OSError:
        load = None
    return {
        "audio_out": None if assertions is None else "audio-out" in assertions,
        "load": load,
        "top": top_processes(runner("top", "-l", "2", "-s", "1", "-n", "6", "-o", "cpu", "-stats", "pid,command,cpu")),
    }


def machine_line(when, state):
    """One line for a state: `machine before: audio-out, load 1.20 1.10 1.05, top Safari 12%, ...`."""
    audio = {True: "audio-out", False: "no audio-out", None: "audio unread"}[state["audio_out"]]
    load = "load " + " ".join(f"{x:.2f}" for x in state["load"]) if state["load"] else "load unread"
    top = (
        "top " + ", ".join(f"{p['command']} {p['cpu']:.0f}%" for p in state["top"])
        if state["top"] is not None
        else "top unread"
    )
    return f"  machine {when}: {audio}, {load}, {top}"


def unseen_check(data):
    """(count, why) for a report's `unseen_frames`: the frames eframe did not paint, the window being
    covered or minimized. `why` names the spoil, and is None when there are none.

    A report with no whole count (an arm built before the key) gives (None, None): nothing says the
    window was covered, and the row keeps null, so that the file shows which runs could be checked.
    """
    unseen = data.get("unseen_frames")
    if isinstance(unseen, bool) or not isinstance(unseen, int):
        return None, None
    return unseen, (f"the window was covered or minimized for {unseen} frames" if unseen > 0 else None)


def counters(pid):
    line = subprocess.check_output([str(T / "energytool"), str(pid)], text=True)
    return {k: float(v) for k, v in re.findall(r"(\w+)=(\d+)", line)}


def run(arm, roots):
    """One drive of `arm`, whose root is in `roots`: its row, with `spoiled` naming why it is not a good run,
    or None."""
    m = roots[arm.removesuffix("+flag")] / "arrakis-measure"
    (m / "home").mkdir(parents=True, exist_ok=True)
    (m / "tmp").mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, HOME=str(m / "home"), TMPDIR=str(m / "tmp"))
    if arm.endswith("+flag"):
        env["MallocSpaceEfficient"] = "1"
    row = {
        "arm": arm,
        "exit": None,
        "finished": None,
        "elapsed_s": None,
        "unseen_frames": None,
        "spoiled": None,
        "machine": {"before": machine_state(), "after": None},  # before the app starts: it is not in the list
    }
    with tempfile.NamedTemporaryFile(suffix=".json") as report:
        proc = subprocess.Popen(
            [str(m / "drive/release/arrakis"), "--demo-drive", "drag", "--demo-seconds", str(SECONDS), "--demo-report", report.name],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            time.sleep(READ_AFTER)
            a, ta, wa = counters(proc.pid), time.monotonic(), time.time()
            time.sleep(READ_FOR)
            b, tb, wb = counters(proc.pid), time.monotonic(), time.time()
        except Exception as err:  # the drive ended early, or energytool failed
            proc.kill()
            row["exit"] = proc.wait()
            row["spoiled"] = f"the counters could not be read ({err})"
            return row
        row["exit"] = proc.wait()
        # Only now: `top -l 2 -s 1` takes about two seconds and loads the machine, and the drive records until
        # its end, so a state taken before would put that load in the row's fps, p50 and p90.
        row["machine"]["after"] = machine_state()
        try:
            data = json.loads(Path(report.name).read_text())
        except (OSError, ValueError) as err:
            row["spoiled"] = f"no report ({err})"
            return row
    row["finished"] = data.get("finished")
    row["elapsed_s"] = data.get("elapsed_s")
    row["unseen_frames"], covered = unseen_check(data)
    seconds = tb - ta
    reasons = []
    if row["exit"] != 0:
        reasons.append(f"exit {row['exit']}")
    if row["finished"] is not True:
        reasons.append(f"finished={row['finished']}")
    if not isinstance(row["elapsed_s"], (int, float)) or not SECONDS <= row["elapsed_s"] <= SECONDS + SLACK:
        reasons.append(f"elapsed_s={row['elapsed_s']} against a {SECONDS} s drive")
    if abs((wb - wa) - seconds) > 1.0:
        reasons.append(f"the read took {wb - wa:.1f} s by the wall clock, {seconds:.1f} s by the monotonic one (a sleep?)")
    if covered:
        reasons.append(covered)
    fps = data.get("frames_per_second")
    spread = data.get("frame_cpu_ms") or {}
    if not isinstance(fps, (int, float)) or fps <= 0:
        reasons.append(f"frames_per_second={fps}")
    if spread.get("p50") is None or spread.get("p90") is None:
        reasons.append("no frame_cpu_ms")
    if reasons:
        row["spoiled"] = "; ".join(reasons)
        return row
    try:
        d = {k: b[k] - a[k] for k in a}
        frames = fps * seconds
        row.update(
            {
                "fps": fps,
                "cpu_percent": d["cpu_ns"] / seconds / 1e7,
                "p_share_cpu": d["p_cpu_ns"] / d["cpu_ns"],
                "p_share_instr": d["p_instr"] / d["instr"],
                "instr_per_frame_M": d["instr"] / frames / 1e6,
                "ghz": d["cycles"] / d["cpu_ns"],
                "ipc": d["instr"] / d["cycles"],
                "energy_mW": d["energy_nj"] / seconds / 1e6,
                "energy_uJ_per_frame": d["energy_nj"] / frames / 1e3,
                "p50": spread["p50"],
                "p90": spread["p90"],
            }
        )
    except (KeyError, ZeroDivisionError) as err:
        row["spoiled"] = f"the counters do not divide ({err!r})"
    return row


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    order = sys.argv[2].split(",")
    roots = arm_roots(os.environ, order)
    for arm in order:
        if arm.removesuffix("+flag") not in roots:
            sys.exit(f"energy-drag.py: no root for the arm {arm!r}; the roots are {', '.join(roots)} (each with +flag)")
    results = []
    for arm in order:
        row = run(arm, roots)
        results.append(row)
        Path(sys.argv[1]).write_text(json.dumps(results, indent=1))
        if row["spoiled"]:
            print(f"SPOILED {arm} (run {len(results)}): {row['spoiled']}; left out of the medians", file=sys.stderr, flush=True)
        shown = {
            k: round(v, 3) if isinstance(v, float) else v
            for k, v in row.items()
            if k not in ("arm", "machine") and v is not None
        }
        print(arm, shown, flush=True)
        for when in ("before", "after"):
            if row["machine"][when] is not None:
                print(machine_line(when, row["machine"][when]), flush=True)

    print()
    for arm in dict.fromkeys(order):
        rows = [r for r in results if r["arm"] == arm]
        good = [r for r in rows if not r["spoiled"]]
        if not good:
            print(f"{arm} n=0 of {len(rows)}: every run spoiled")
            continue
        keys = [k for k in good[0] if k not in NOT_MEASURES]
        left_out = f" ({len(rows) - len(good)} spoiled left out)" if len(good) != len(rows) else ""
        print(arm, f"n={len(good)} of {len(rows)}{left_out}", " ".join(f"{k}={st.median(r[k] for r in good):.3f}" for k in keys))


# The tests import this file, and nothing runs then.
if __name__ == "__main__":
    main()
