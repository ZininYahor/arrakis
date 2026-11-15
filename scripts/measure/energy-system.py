#!/usr/bin/env python3
"""The whole machine's power in a drag: CPU, GPU, and the window server's and Arrakis's rows.

Why: energy-drag.py counts the app's CPU only (proc_pid_rusage). The Metal painter moves work to the
GPU and the window server, which only `sudo powermetrics` reads. The owner's decision of 2026-10-31:
stop if CPU and GPU power together, in a drag, read above glow's with the 3 runs' ranges apart.

THE FIELD NAMES OF THE FILE WERE CONFIRMED on 2026-11-10, against the owner's real one (macOS 25G83,
MacBookPro18,3; `sudo powermetrics --samplers cpu_power,gpu_power,tasks --show-process-gpu
--show-process-energy -f plist`): a stream of plist documents; per sample `timestamp`, `elapsed_ns`,
`processor` with `cpu_power`, `gpu_power` and `combined_power` in mW and `cpu_energy` and `gpu_energy`
in mJ (the ANE reads 0), and a `tasks` array with `name`, `cputime_ms_per_s` and `energy_impact`.
FIELDS below reads it as written. The one difference from what this file first expected: the task
rows hold no `gputime_ms_per_s` on that macOS, though `--show-process-gpu` is given, so `read` prints
"-" for it. The rule does not use it, and the file does not say whose GPU time it was. A field that is
missing, or a unit that looks wrong, stops `read` with the name of what is missing and the keys that
are there: nothing is guessed. Run `keys` on a 3-second probe first, and keep the file: a fix to
FIELDS below can read the same file again.

usage:
  energy-system.py run <runs.tsv> [turns [arm,arm,...]]
      Runs, in turns (3 by default), the arms named (`cap,metal,metal-2,idle` by default): `cap` + flag,
      `metal` + flag, `metal-2` (metal + flag with SOFT_SURFACES=2), `final` (the final build + flag:
      name it, as in `run runs.tsv 3 cap,final,idle`) and `idle`, 30 s with no Arrakis. Each drive
      is `--demo-drive drag --demo-seconds 30` on NullScreen, from <root>/arrakis-measure/drive/release/arrakis
      (scripts/measure.sh describes the layout). The roots are $TMPDIR/t1/cap, $TMPDIR/t1/metal (which
      `metal-2` shares) and $TMPDIR/arms/final. ENERGY_SYSTEM_ROOT_<NAME>=<root> points one at another root
      (an absolute path, such as an arm that build-arm.sh made): <NAME> is the root's name in capitals, so
      ENERGY_SYSTEM_ROOT_FINAL=/x/arm-abc1234 sets "final". A variable that is not an absolute path ends the
      run before it starts when its root is one of the arms named, and is ignored when it is not.
      <runs.tsv> gets one line after each run: label, turn, start and end (Unix
      seconds), elapsed_s, nominal seconds and why a run is spoiled; a file that exists gets more
      turns after its last. Start `powermetrics` first (the
      command above, with `-i 1000 -n 600 -f plist -o <power.plist>`), and keep the machine quiet: this
      takes the focus, as every drive with a window does. A run takes about 37 s (the drive, its start and
      quit, 5 s of rest): 7.5 minutes for 3 turns of 4 arms, which a 600-sample powermetrics (10 minutes)
      outlasts if it is started first.
  energy-system.py read <power.plist> <runs.tsv>
      Cuts the samples from 5 s to 25 s of each run and prints each run's mean CPU, GPU and combined
      power, the two processes' rows, the arms' medians and ranges, and the rule's verdict for each of
      `metal`, `metal-2` and `final` that ran, against `cap`. It needs `cap`, `idle` and at least one of them.
  energy-system.py keys <power.plist>
      Prints the last sample's keys and the first tasks' names, to check the fields above.
"""
import calendar
import datetime
import json
import os
import plistlib
import re
import statistics as st
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# A drive's length and the rest between runs. The environment changes them for a test of this script's
# plumbing only; a measurement keeps 30 and 5.
SECONDS = int(os.environ.get("ENERGY_SYSTEM_SECONDS", "30"))
FROM = 5  # seconds into a run when the cut begins; it ends FROM seconds before the run does
GAP = int(os.environ.get("ENERGY_SYSTEM_GAP", "5"))
# A test holds SLACK equal to the driver's `TIMED_SLACK_S` (src/drive.rs), the bound of a timed drive.
SLACK = 2.0  # how far past SECONDS a healthy drive's elapsed_s may be
ROOT_PREFIX = "ENERGY_SYSTEM_ROOT_"
FLAG = {"MallocSpaceEfficient": "1"}
# (label, root, the app's extra environment). `idle` runs no Arrakis.
ARMS = [
    ("cap", "cap", FLAG),
    ("metal", "metal", FLAG),
    ("metal-2", "metal", dict(FLAG, SOFT_SURFACES="2")),
    ("final", "final", FLAG),
    ("idle", None, None),
]
# What `run` runs when no arm is named: the arms of the prototype's first reading. The final build is
# named when it is read.
DEFAULT_ARMS = ("cap", "metal", "metal-2", "idle")
# The arms `read` sets against `cap`, with what each is.
COMPARED = (
    ("metal", "the shipped shape on one surface"),
    ("metal-2", "two surfaces"),
    ("final", "the final build"),
)
# The fields to read, in the order tried: (path in a sample, unit). mJ is a sample's energy, divided by
# its elapsed_ns; mW is a power, taken as it is.
FIELDS = {
    "cpu": [(("processor", "cpu_power"), "mW"), (("processor", "cpu_energy"), "mJ")],
    "gpu": [
        (("processor", "gpu_power"), "mW"),
        (("processor", "gpu_energy"), "mJ"),
        (("gpu", "gpu_power"), "mW"),
        (("gpu", "gpu_energy"), "mJ"),
    ],
}
OPTIONAL = {
    "ane": [(("processor", "ane_power"), "mW"), (("processor", "ane_energy"), "mJ")],
    "combined": [(("processor", "combined_power"), "mW"), (("processor", "package_energy"), "mJ")],
}
PROCESSES = {"WindowServer": "windowserver", "arrakis": "arrakis"}
ROW_FIELDS = ("cputime_ms_per_s", "gputime_ms_per_s", "energy_impact")
MAX_MW = 200_000  # more than this is a unit that is not what the table says


def fail(message):
    sys.exit(f"energy-system.py: {message}")


# ---- run ---------------------------------------------------------------------------------------------


def pgrep_arrakis():
    out = subprocess.run(["pgrep", "-i", "-x", "arrakis"], capture_output=True, text=True).stdout.split()
    return out


def arms_named(names):
    """The ARMS named in `names` (comma-separated, in that order, a name once for each run of it), or
    DEFAULT_ARMS when `names` is None."""
    by_label = {arm[0]: arm for arm in ARMS}
    labels = DEFAULT_ARMS if names is None else names.split(",")
    for label in labels:
        if label not in by_label:
            fail(f"no arm {label!r}; the arms are {', '.join(by_label)}")
    return [by_label[label] for label in labels]


def arm_roots(environ, used):
    """The roots of the builds: `cap` and `metal` (the prototype's first reading) and `final`, then the ones
    the environment sets or adds (ENERGY_SYSTEM_ROOT_<NAME>). `used` is the names of the roots the night's
    arms run. A variable whose value is not an absolute path ends the run when its root is in `used`; for a
    root that is not, it is left out and nothing is said, as that root is not read (a leftover variable must
    not stop a night)."""
    tmp = Path(environ.get("TMPDIR", "/tmp"))
    roots = {"cap": tmp / "t1/cap", "metal": tmp / "t1/metal", "final": tmp / "arms/final"}
    for var, value in environ.items():
        if var.startswith(ROOT_PREFIX) and var != ROOT_PREFIX:
            root = var.removeprefix(ROOT_PREFIX).lower().replace("_", "-")
            if not value.startswith("/"):
                if root in used:
                    fail(f"{var} must be an absolute path, and {value!r} is not")
                continue
            roots[root] = Path(value)
    return roots


def one_run(label, root, extra):
    """One run: its TSV fields, with the reason it is spoiled (or "") last. `root` is the build's root, or
    None for the run with no Arrakis."""
    start = time.time()
    reasons = []
    elapsed = ""
    if root is None:
        time.sleep(SECONDS)
    else:
        m = root / "arrakis-measure"
        (m / "home").mkdir(parents=True, exist_ok=True)
        (m / "tmp").mkdir(parents=True, exist_ok=True)
        env = dict(os.environ, HOME=str(m / "home"), TMPDIR=str(m / "tmp"), **extra)
        with tempfile.NamedTemporaryFile(suffix=".json") as report:
            cmd = [str(m / "drive/release/arrakis"), "--demo-drive", "drag", "--demo-seconds", str(SECONDS)]
            try:
                code = subprocess.run(
                    cmd + ["--demo-report", report.name],
                    env=env,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=SECONDS + 30,
                ).returncode
            except subprocess.TimeoutExpired:
                code = "timeout"
            try:
                data = json.loads(Path(report.name).read_text())
            except (OSError, ValueError) as err:
                data = {}
                reasons.append(f"no report ({err})")
        elapsed = data.get("elapsed_s")
        if code != 0:
            reasons.append(f"exit {code}")
        if data and data.get("finished") is not True:
            reasons.append(f"finished={data.get('finished')}")
        if data and (not isinstance(elapsed, (int, float)) or not SECONDS <= elapsed <= SECONDS + SLACK):
            reasons.append(f"elapsed_s={elapsed} against a {SECONDS} s drive")
        # eframe paints no frame of a covered or minimized window, so its energy reads too good. A
        # report from an arm built before the key has no count: it cannot be checked, and is kept.
        unseen = data.get("unseen_frames")
        if isinstance(unseen, int) and unseen > 0:
            reasons.append(f"the window was covered or minimized for {unseen} frames")
    end = time.time()
    if not SECONDS <= end - start <= SECONDS + 10:
        reasons.append(f"the run took {end - start:.1f} s by the wall clock (a sleep?)")
    return [label, "", f"{start:.3f}", f"{end:.3f}", str(elapsed), str(SECONDS), "; ".join(reasons)]


def cmd_run(path, turns, names=None):
    arms = arms_named(names)
    roots = arm_roots(os.environ, {root for _, root, _ in arms if root is not None})
    if pgrep_arrakis():
        fail(f"another arrakis is running (pid {' '.join(pgrep_arrakis())}); quit it first")
    for _, root, _ in arms:
        if root is not None and not (roots[root] / "arrakis-measure/drive/release/arrakis").is_file():
            fail(f"no drive build of {root} in {roots[root]}; see the run page for the layout")
    out = Path(path)
    done = 0
    if out.exists():  # a second call adds turns after the first call's, for a spoiled run's sake
        done = max(r["turn"] for r in read_runs(path))
    else:
        out.write_text("# label\tturn\tstart\tend\telapsed_s\tseconds\tspoiled\n")
    for turn in range(done + 1, done + turns + 1):
        for label, root, extra in arms:
            row = one_run(label, None if root is None else roots[root], extra)
            row[1] = str(turn)
            with out.open("a") as f:
                f.write("\t".join(row) + "\n")
            print(" ".join(f"{k}={v}" for k, v in zip(("label", "turn", "start", "end", "elapsed_s"), row)),
                  "SPOILED: " + row[6] if row[6] else "", flush=True)
            time.sleep(GAP)


# ---- the file ----------------------------------------------------------------------------------------


def load_samples(path):
    data = Path(path).read_bytes()
    starts = [m.start() for m in re.finditer(rb"<\?xml", data)]
    if not starts:
        fail(f"{path} holds no plist document (no '<?xml'); is it from powermetrics -f plist?")
    samples = []
    for i, begin in enumerate(starts):
        end = starts[i + 1] if i + 1 < len(starts) else len(data)
        try:
            samples.append(plistlib.loads(data[begin:end].rstrip(b"\0 \n\t")))
        except Exception as err:  # the last document may be cut short if powermetrics was stopped
            if i + 1 == len(starts):
                print(f"energy-system.py: the last document is cut short and is left out ({err})", file=sys.stderr)
                break
            fail(f"document {i} of {path} does not parse: {err}")
    if not samples:
        fail(f"{path} holds no sample that parses")
    return samples


def keys_of(node):
    return ", ".join(sorted(node)) if isinstance(node, dict) else f"<{type(node).__name__}>"


def at(sample, path):
    node = sample
    for key in path:
        if not isinstance(node, dict) or key not in node:
            return None
        node = node[key]
    return node if isinstance(node, (int, float)) else None


def power(sample, tried, name, required):
    """mW of the first field present in `tried`, or None (when not `required`)."""
    for path, unit in tried:
        value = at(sample, path)
        if value is None:
            continue
        if unit == "mJ":
            ns = sample.get("elapsed_ns")
            if not isinstance(ns, (int, float)) or ns <= 0:
                fail(f"{'.'.join(path)} is in mJ but the sample has no usable elapsed_ns ({ns!r}); keys: {keys_of(sample)}")
            value = value / (ns / 1e9)
        if not 0 <= value <= MAX_MW:
            fail(f"{name} power reads {value:.4g} mW from {'.'.join(path)} ({unit}): the unit is probably not what this table says")
        return value
    if required:
        names = ", ".join(f"{'.'.join(p)} ({u})" for p, u in tried)
        sections = sorted({p[0] for p, _ in tried if p[0] in sample})
        places = "".join(f"; under '{s}': {keys_of(sample[s])}" for s in sections)
        fail(f"a sample has no {name} power; tried {names}. Keys at the top: {keys_of(sample)}{places}")
    return None


def sample_time(sample):
    stamp = sample.get("timestamp")
    if not isinstance(stamp, datetime.datetime):
        fail(f"a sample has no timestamp (found {stamp!r}); keys: {keys_of(sample)}")
    return calendar.timegm(stamp.utctimetuple()) + stamp.microsecond / 1e6


def process_row(sample, name):
    """({field: value}, True) for the process's row in a sample; ({}, False) when it has none."""
    for task in sample.get("tasks") or []:
        if isinstance(task, dict) and str(task.get("name", "")).lower() == name.lower():
            row = {f: task[f] for f in ROW_FIELDS if isinstance(task.get(f), (int, float))}
            if not row:
                fail(f"the row of {name} has none of {', '.join(ROW_FIELDS)}; its keys: {keys_of(task)}")
            return row, True
    return {}, False


def mean(values):
    return sum(values) / len(values)


def read_runs(tsv):
    runs = []
    for number, line in enumerate(Path(tsv).read_text().splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        f = line.split("\t")
        if len(f) < 6:
            fail(f"{tsv} line {number}: expected 7 tab-separated fields, found {len(f)}")
        runs.append(
            {"label": f[0], "turn": int(f[1]), "start": float(f[2]), "seconds": float(f[5]), "spoiled": f[6] if len(f) > 6 else ""}
        )
    if not runs:
        fail(f"{tsv} holds no run")
    return runs


def measure(samples, run):
    lo, hi = run["start"] + FROM, run["start"] + run["seconds"] - FROM
    chosen = [s for s in samples if lo <= sample_time(s) <= hi]
    need = int((hi - lo) * 0.75)
    if len(chosen) < need:
        times = sorted(sample_time(s) for s in samples)
        fail(
            f"{run['label']} turn {run['turn']}: {len(chosen)} samples between {lo:.0f} and {hi:.0f}, need {need}; "
            f"the file runs from {times[0]:.0f} to {times[-1]:.0f}. Did powermetrics start before the runs, and run long enough?"
        )
    out = {"n": len(chosen)}
    columns = {name: [power(s, tried, name, True) for s in chosen] for name, tried in FIELDS.items()}
    for name, tried in OPTIONAL.items():
        found = [power(s, tried, name, False) for s in chosen]
        columns[name] = found if all(v is not None for v in found) else None
    for name, values in columns.items():
        out[name] = None if values is None else mean(values)
    out["cpu+gpu"] = mean([c + g for c, g in zip(columns["cpu"], columns["gpu"])])
    ane = columns["ane"] or [0.0] * len(chosen)
    out["all"] = mean([c + g + a for c, g, a in zip(columns["cpu"], columns["gpu"], ane)])
    for key, name in PROCESSES.items():
        rows = [process_row(s, key) for s in chosen]
        out[f"{name}_seen"] = sum(1 for _, seen in rows if seen)
        for field in ROW_FIELDS:
            # A sample without the process's row counts as 0: powermetrics lists only what ran.
            out[f"{name}.{field}"] = mean([r.get(field, 0.0) for r, _ in rows]) if any(field in r for r, _ in rows) else None
    return out


def verdict(metal, cap):
    if min(metal) > max(cap):
        return "STOP: above glow's, the ranges apart"
    if max(metal) < min(cap):
        return "go: below glow's, the ranges apart"
    return "go: the ranges overlap, so no measurable loss"


def fmt(value, digits=0):
    return "-" if value is None else f"{value:.{digits}f}"


def cmd_read(plist, tsv):
    samples = load_samples(plist)
    runs = read_runs(tsv)
    for run in runs:
        run["m"] = measure(samples, run)
    print(f"{len(samples)} samples; power in mW, means of the samples from {FROM} s to {SECONDS - FROM} s of each run\n")
    head = "arm      turn   n   cpu    gpu  cpu+gpu c+g+ane  combined | WindowServer cpu gpu ms/s, impact | arrakis cpu gpu ms/s, impact"
    print(head)
    for run in runs:
        m = run["m"]
        ws = " ".join(fmt(m[f"windowserver.{f}"], 1) for f in ROW_FIELDS)
        ar = " ".join(fmt(m[f"arrakis.{f}"], 1) for f in ROW_FIELDS)
        print(
            f"{run['label']:<8} {run['turn']:>3} {m['n']:>3} {fmt(m['cpu']):>5} {fmt(m['gpu']):>6} {fmt(m['cpu+gpu']):>8} "
            f"{fmt(m['all']):>7} {fmt(m['combined']):>9} | {ws:<28} | {ar}"
            + (f"  SPOILED: {run['spoiled']}" if run["spoiled"] else "")
        )
    good = [r for r in runs if not r["spoiled"]]
    by_arm = {}
    for r in good:
        by_arm.setdefault(r["label"], []).append(r["m"])
    compared = [(arm, note) for arm, note in COMPARED if arm in by_arm]
    for needed in ("cap", "idle"):
        if needed not in by_arm:
            fail(f"no good run of {needed}; the runs file has {', '.join(sorted({r['label'] for r in runs}))}")
    if not compared:
        fail(
            f"no good run of {', '.join(arm for arm, _ in COMPARED)} to set against cap; "
            f"the runs file has {', '.join(sorted({r['label'] for r in runs}))}"
        )
    # A process's row must exist in some sample of an arm that ran Arrakis, or the name is wrong.
    for arm, ms in by_arm.items():
        if arm == "idle":
            continue
        for key, name in PROCESSES.items():
            if sum(m[f"{name}_seen"] for m in ms) == 0:
                names = sorted({str(t.get("name")) for s in samples[:20] for t in (s.get("tasks") or []) if isinstance(t, dict)})
                fail(f"no row of {key} in any sample of the {arm} runs; the first samples' tasks are: {', '.join(names[:25])}")
    idle = st.median(m["cpu+gpu"] for m in by_arm["idle"])
    print(f"\nidle (no Arrakis): cpu+gpu {', '.join(fmt(m['cpu+gpu']) for m in by_arm['idle'])} mW, median {fmt(idle)}\n")
    print("per arm, cpu+gpu in mW: the runs, the range, the median, and the median less idle")
    for arm in ("cap", *(arm for arm, _ in COMPARED)):
        ms = by_arm.get(arm)
        if not ms:
            continue
        v = [m["cpu+gpu"] for m in ms]
        print(f"  {arm:<8} n={len(v)}  runs {', '.join(fmt(x) for x in v)}  range {fmt(min(v))}-{fmt(max(v))}  median {fmt(st.median(v))}  net {fmt(st.median(v) - idle)}")
    cap = [m["cpu+gpu"] for m in by_arm["cap"]]
    print("\nThe rule (the owner's, of 2026-10-31): stop if CPU and GPU power together, in a drag, read above glow's with the 3 runs' ranges apart.")
    for arm, note in compared:
        v = [m["cpu+gpu"] for m in by_arm[arm]]
        answer = verdict(v, cap)
        every = verdict([m["all"] for m in by_arm[arm]], [m["all"] for m in by_arm["cap"]])
        print(f"  {arm:<8} ({note}): {answer}")
        if answer.split(":")[0] != every.split(":")[0]:
            print(f"           NOTE: with the ANE added the verdict is '{every}'; the owner's words name CPU and GPU")
    counts = {arm: len(by_arm[arm]) for arm in ("cap", *(arm for arm, _ in compared))}
    if min(counts.values()) < 3:
        print(f"  NOTE: fewer than 3 good runs ({', '.join(f'{arm} {n}' for arm, n in counts.items())}): the ranges are narrower than the rule's")


def tree(node, depth, pad=""):
    if isinstance(node, dict):
        for k in sorted(node):
            v = node[k]
            if isinstance(v, dict):
                print(f"{pad}{k}: dict of {len(v)}")
                if depth > 1:
                    tree(v, depth - 1, pad + "    ")
            elif isinstance(v, list):
                print(f"{pad}{k}: list of {len(v)}")
            else:
                print(f"{pad}{k} = {v!r}"[:110])


def cmd_keys(plist):
    samples = load_samples(plist)
    print(f"{len(samples)} samples. The last one's keys (the first is often not a delta):")
    sample = samples[-1]
    tree(sample, 3)
    tasks = [t for t in sample.get("tasks") or [] if isinstance(t, dict)]
    if tasks:
        print(f"\ntasks: {len(tasks)}, the first one's keys: {keys_of(tasks[0])}")
        print("names:", ", ".join(str(t.get("name")) for t in tasks[:30]))
    print("\nread looks for: " + "; ".join(f"{n}: " + " | ".join('.'.join(p) + f' ({u})' for p, u in tried) for n, tried in {**FIELDS, **OPTIONAL}.items()))


def main(argv):
    if len(argv) >= 2 and argv[0] == "run" and len(argv) <= 4:
        cmd_run(argv[1], int(argv[2]) if len(argv) >= 3 else 3, argv[3] if len(argv) == 4 else None)
    elif len(argv) == 3 and argv[0] == "read":
        cmd_read(argv[1], argv[2])
    elif len(argv) == 2 and argv[0] == "keys":
        cmd_keys(argv[1])
    else:
        sys.exit(__doc__)


# The tests import this file, and nothing runs then.
if __name__ == "__main__":
    main(sys.argv[1:])
