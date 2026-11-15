#!/usr/bin/env python3
"""Sorts the captures of a run of close-captures.sh, with the standard library only.

A capture of a window at rest is one picture, again and again, so its file size stays. Only a capture
whose size strays from the run's median is opened, which keeps this quick and sorts each one as:

  window  the size of the window's own captures
  gone    every pixel fully transparent: the window had already left, so no flash
  bare    one opaque flat color fills the window's interior (no corners, no traffic lights): the bare window
  other   none of these, for the eye: it may be a window with its contents half gone, or a damaged file

Each capture's name says its window, and each window is one close. A run is spoiled if it exited
non-zero, did not finish, took another time than its closes ask, had its window covered or minimized
(`unseen_frames`), ran a painter other than Metal or another number of closes, or if the window's own
captures are flat or transparent. It is spoiled too if captures saw fewer windows than it has closes, as
what no capture saw says nothing, or if no pause's block dropped a spare (the report's `hold.spare_drops`
is 0 or absent), or if its report or exit file is damaged.

The lines it prints read: the shipped close shows no bare window. A close read under these captures also
frees differently, so no figure of memory comes from here.

usage: close-scan.py <folder> <closes a run>
  <folder>/run-<round>/ each hold the PNGs, report.json and exit. It writes <folder>/summary.json.
Exits 0 when every run held, 1 when one was spoiled, 2 when a run holds no capture.
"""
import importlib.util
import json
import statistics
import struct
import sys
import zlib
from pathlib import Path

# The bound of an open-close run is the driver's own (`Scenario::bound` in src/drive.rs), and a test holds
# these three equal to its constants: CYCLE to `CYCLE`, SLACK to `CYCLED_SLACK_S` and OVERRUN to `CYCLE_OVERRUN_S`.
CYCLE = 3  # the seconds of one open-close cycle: `CYCLE` in src/drive.rs
SLACK = 4.0  # what a run takes beyond its cycles: the start and the end
OVERRUN = 0.25  # what one cycle may take beyond `CYCLE`: twice the 0.11 to 0.125 s that drives read
SAME = 0.02  # how far a capture's size strays from the median and still is the window's own
FLAT = 0.99  # the share of the interior's rows of one color that makes a window bare
OPAQUE = 255

# The PNG reader, and what it calls the window's interior. No bytecode of it in the repository.
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("tear_scan", Path(__file__).with_name("tear-scan.py"))
tear_scan = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(tear_scan)

UNREADABLE = (ValueError, zlib.error, struct.error)


def flat(path):
    """Whether one opaque color fills the window's interior. An unreadable file is not flat."""
    try:
        colors, ragged = tear_scan.interior(path)
    except UNREADABLE:
        return False
    if not colors:
        return False
    color, rows = colors.most_common(1)[0]
    has_no_alpha = len(color) == 3
    return rows >= FLAT * (sum(colors.values()) + ragged) and (has_no_alpha or color[3] == OPAQUE)


def gone(path):
    """Whether every pixel is fully transparent. An unreadable file is not."""
    try:
        _, _, bytes_a_pixel, rows = tear_scan.read_png(path)
    except UNREADABLE:
        return False
    return bytes_a_pixel == 4 and not any(any(row[3::4]) for row in rows)


def sort_captures(files):
    """Each file's kind, and whether the window's own captures are themselves flat or transparent."""
    median = statistics.median(f.stat().st_size for f in files)
    kinds = {}
    for f in files:
        if abs(f.stat().st_size - median) <= SAME * median:
            kinds[f] = "window"
        elif gone(f):
            kinds[f] = "gone"
        else:
            kinds[f] = "bare" if flat(f) else "other"
    own = [f for f, kind in kinds.items() if kind == "window"]
    return kinds, not own or flat(own[0]) or gone(own[0])


def spare_drops(report):
    """What the drive's report counts in `hold.spare_drops`, None without it."""
    hold = report.get("hold")
    return hold.get("spare_drops") if isinstance(hold, dict) else None


def report_faults(report, closes):
    """What spoils a run in its report. A report of another shape raises what `read_run` catches."""
    faults = []
    elapsed = report.get("elapsed_s")
    longest = closes * (CYCLE + OVERRUN) + SLACK
    if report.get("finished") is not True:
        faults.append(f"finished={report.get('finished')}")
    if not isinstance(elapsed, (int, float)) or not closes * CYCLE <= elapsed <= longest:
        faults.append(f"elapsed_s={elapsed} against {closes} closes")
    if report.get("unseen_frames") != 0:
        faults.append(f"unseen_frames={report.get('unseen_frames')}: the window was covered")
    opens = report.get("opens", [])
    if len(opens) != closes:
        faults.append(f"{len(opens)} windows, asked for {closes}")
    painters = {open_["renderer"] for open_ in opens}
    if painters - {"metal"}:
        faults.append(f"painters {sorted(painters)}")
    drops = spare_drops(report)
    if not isinstance(drops, int) or drops <= 0:
        faults.append(f"spare_drops={drops}: no pause's block is seen to have come")
    return faults


def read_run(folder, closes):
    files = sorted(folder.glob("*.png"))
    spoiled = []
    status = tear_scan.read_status(folder / "exit")
    if status != 0:
        spoiled.append(f"exit {status}")
    report = tear_scan.read_report(folder / "report.json")  # None with no file, False when damaged
    if report is False:
        report = {}
        spoiled.append("report.json is damaged")
    elif not report:
        report = {}
        spoiled.append("no report")
    else:
        try:
            spoiled += report_faults(report, closes)
        except (KeyError, TypeError):
            spoiled.append("report.json is damaged")
    windows = len({f.name.split("-", 1)[0] for f in files})
    if windows < closes:
        spoiled.append(f"captures saw {windows} windows of {closes} closes")
    kinds, own_flat = sort_captures(files) if files else ({}, False)
    if own_flat:
        spoiled.append("the window's own captures are flat or transparent")
    named = lambda kind: sorted(str(f) for f, found in kinds.items() if found == kind)
    bare = named("bare")
    return {
        "run": folder.name,
        "captures": len(files),
        "windows": windows,
        "bare": bare,
        "bare_windows": len({Path(path).name.split("-", 1)[0] for path in bare}),
        "gone": named("gone"),
        "other": named("other"),
        "spare_drops": spare_drops(report),
        "spoiled": spoiled,
    }


def runs_line(runs):
    good = [run for run in runs if not run["spoiled"]]
    count = lambda key: sum(run[key] for run in good)
    listed = lambda key: sum(len(run[key]) for run in good)
    return (
        f"runs={len(good)} of {len(runs)} closes_seen={count('windows')} "
        f"captures={count('captures')} bare={listed('bare')} bare_closes={count('bare_windows')} "
        f"gone={listed('gone')} other={listed('other')} spare_drops={count('spare_drops')}"
    )


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    folder, closes = Path(sys.argv[1]), int(sys.argv[2])
    runs = [read_run(run, closes) for run in sorted(p for p in folder.iterdir() if p.is_dir())]
    (folder / "summary.json").write_text(json.dumps(runs, indent=1) + "\n")
    print(runs_line(runs))
    for run in runs:
        for why in run["spoiled"]:
            print(f"spoiled: {run['run']}: {why}")
        for path in run["bare"]:
            print(f"bare: {path}")
        for path in run["other"]:
            print(f"other: {path}")
    print(f"pictures: {folder}")
    if not runs or any(run["captures"] == 0 for run in runs):
        print("close-scan.py: a run holds no capture: see its capture.log", file=sys.stderr)
        sys.exit(2)
    sys.exit(1 if any(run["spoiled"] for run in runs) else 0)


if __name__ == "__main__":
    main()
