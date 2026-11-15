#!/usr/bin/env python3
"""Medians (and ranges) of the real-app runs and drives of the memory night, 2026-10-30 to 31.

usage: chain-summary.py <directory>
  The directory holds the logs and reports of the runs, none of it in git:
    night-real.log, pr34-real.log, chain-b.log
        the result lines of real-cycles.sh ("<arm> HH:MM:SS never=... open5=... windows_left=0"),
        any other lines are skipped; a run's arm is its first word
    drives/<arm>-<scenario>-<n>.json
        the --demo-report of a drive, scenario one of start, open-close, drag or open-idle
  A file that is missing is skipped. A file whose name does not fit, or that does not parse, and a
  value that a run lacks (a null field, an empty `opens`) are left out with a line on stderr, not a
  traceback. Every median is printed with its real n: the runs that had the value.
  What it prints is each state's footprint in MiB, a drive's times and CPU, and the counts that say
  whether a run did what it was meant to (windows left, reopens, finished, unseen frames: those of a
  covered or minimized window, which a good run has none of). It has no POWER, wakeups
  or peak: those are measure.sh's. It reads and prints; it writes nothing and runs nothing.
"""
import json
import re
import statistics as st
import sys
from pathlib import Path

if len(sys.argv) != 2:
    sys.exit(__doc__)
HERE = Path(sys.argv[1])
FIELDS = ["never", "open5", "open30", "open90", "close1_2s", "close1_60s", "close7_2s", "close7_60s"]
SCENARIOS = ["start", "open-close", "drag", "open-idle"]
# The night's arms first, in the order its tables use; any other arm follows, so none is dropped.
ARM_ORDER = ["cap", "release", "both", "metal", "direct"]


def note(source, why):
    print(f"{source}: {why}", file=sys.stderr)


def med(values):
    return st.median(values) if values else float("nan")


def cell(values, digits=2):
    """Median (min-max, n=k) of the values: k is how many runs had one."""
    if not values:
        return "- (n=0)"
    return f"{med(values):.{digits}f} ({min(values):.{digits}f}-{max(values):.{digits}f}, n={len(values)})"


def counts(values):
    """The distinct values of a count, one for each run: "6" if all agree, else "5,6"."""
    return ",".join(sorted({str(v) for v in values})) or "?"


runs = {}
for name in ["night-real.log", "pr34-real.log", "chain-b.log"]:
    path = HERE / name
    if not path.exists():
        continue
    for number, line in enumerate(path.read_text().splitlines(), 1):
        match = re.match(r"(\S+) \d\d:\d\d:\d\d never=", line)
        if not match:
            continue
        values = {}
        for key, text in re.findall(r"(\w+)=([\d.]+)", line):
            try:
                values[key] = float(text)
            except ValueError:
                note(f"{name}:{number}", f"{key}={text} is not a number; left out")
        runs.setdefault(match.group(1), []).append(values)

print("## Real app, MiB, median (range, n)")
print("arm n windows_left reopens windows_after_close1 " + " ".join(FIELDS))
for arm, rows in runs.items():
    left = counts(int(row["windows_left"]) for row in rows if "windows_left" in row)
    reopens = counts(int(row["reopens"]) for row in rows if "reopens" in row)
    # real-cycles.sh prints it since the first close was checked; the night's runs did not.
    took = counts(int(row["windows_after_close1"]) for row in rows if "windows_after_close1" in row)
    cells = [cell([row[f] for row in rows if f in row]) for f in FIELDS]
    print(f"{arm} n={len(rows)} left={left} reopens={reopens} after_close1={took} " + " | ".join(cells))

print()
print("## Drives")
mb = 1 << 20
drives = {}
for path in sorted((HERE / "drives").glob("*.json")):
    match = re.fullmatch(r"(.+?)-(start|open-close|drag|open-idle)-(\d+)\.json", path.name)
    if not match:
        note(path.name, "the name is not <arm>-<scenario>-<n>.json; skipped")
        continue
    try:
        report = json.loads(path.read_text())
    except (OSError, ValueError) as err:
        note(path.name, f"cannot be read ({err}); skipped")
        continue
    if not isinstance(report, dict):
        note(path.name, "is not a report; skipped")
        continue
    drives.setdefault((match.group(1), match.group(2)), []).append((path.name, report))


def column(rows, *keys, scale=1):
    """The value at `keys` of each report that has one, divided by `scale`; a report that lacks it
    (a missing or null field, an empty list) is named on stderr and left out."""
    out = []
    for source, report in rows:
        value = report
        for key in keys:
            try:
                value = value[key]
            except (KeyError, IndexError, TypeError):
                value = None
            if value is None:
                break
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            note(source, f"no {'.'.join(map(str, keys))}; left out of its median")
        else:
            out.append(value / scale)
    return out


def reopen_column(rows, key):
    """Each report's median of `key` over its opens after the first; a report with none is named."""
    out = []
    for source, report in rows:
        opens = report.get("opens") or []
        values = [o[key] for o in opens[1:] if isinstance(o.get(key), (int, float))]
        if values:
            out.append(med(values))
        else:
            note(source, f"no reopen {key}; left out of its median")
    return out


arms = {arm for arm, _ in drives}
arms = [a for a in ARM_ORDER if a in arms] + sorted(arms - set(ARM_ORDER))
for scenario in SCENARIOS:
    print(f"### {scenario}")
    for arm in arms:
        rows = drives.get((arm, scenario))
        if not rows:
            continue
        finished = [r.get("finished") for _, r in rows]
        out = [f"{arm} n={len(rows)} finished={all(f is True for f in finished)}"]
        out.append(f"elapsed {cell(column(rows, 'elapsed_s'), 1)}")
        # Frames eframe did not paint, the window being covered: "?" for a report from before the key.
        out.append("unseen frames " + counts(r.get("unseen_frames", "?") for _, r in rows))
        out.append(f"end MiB {cell(column(rows, 'footprint_at_end', scale=mb))}")
        out.append(f"first window ms {cell(column(rows, 'opens', 0, 'to_first_frame_ms'), 1)}")
        out.append(f"first frame cpu {cell(column(rows, 'opens', 0, 'first_frame_cpu_ms'), 1)}")
        if scenario in ("start", "drag"):
            out.append(f"main to first ms {cell(column(rows, 'main_to_first_frame_ms'), 1)}")
        if scenario == "open-close":
            # Opens after the first: the reopens. A run that closed fewer windows than it opened has fewer.
            out.append("reopens " + counts(max(len(r.get("opens") or []) - 1, 0) for _, r in rows))
            out.append(f"first close {cell(column(rows, 'footprint_after_first_close', scale=mb))}")
            out.append(f"last close {cell(column(rows, 'footprint_after_last_close', scale=mb))}")
            out.append(f"reopen cpu by run {cell(reopen_column(rows, 'first_frame_cpu_ms'))}")
            out.append(f"reopen to-first by run {cell(reopen_column(rows, 'to_first_frame_ms'), 1)}")
        if scenario == "drag":
            for q in ["p50", "p90", "p99"]:
                out.append(f"{q} {cell(column(rows, 'frame_cpu_ms', q), 3)}")
            out.append(f"f/s {cell(column(rows, 'frames_per_second'), 1)}")
        print(" ; ".join(out))
