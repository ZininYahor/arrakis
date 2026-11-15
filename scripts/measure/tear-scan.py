#!/usr/bin/env python3
"""Counts the tears in the window captures of one run of tear-captures.sh.

The demo build's Metal painter with ARRAKIS_METAL_TEAR=1 draws no UI: each frame is one flat grey, 96
or 100 by turns, so a capture of the window holds exactly one of the two
greys unless the window server showed parts of two frames at once. This scans the PNGs with the
standard library only, on the window's interior (no corners, no traffic lights), and sorts each one:

  a, b      every interior row is the same single color, one of the two greys: no tear
  both      rows of both greys, or a row that holds more than one color: a tear
  other     one flat color that is neither grey, or a mix with a third color: a tear ("neither")

The two greys are not assumed: they are the two most common flat colors of the run, because a capture
passes through the display's profile. The window's fade-in and its exit are not counted: a run's
settled captures are those from its first grey capture to its last. A run needs both greys at least
3 times, or this fails loudly.

usage: tear-scan.py <folder>             scan <folder>/*.png and write <folder>/summary.json
       tear-scan.py --sum <folder>...    print each folder's line and their total
  <folder>/exit and <folder>/report.json (the drive's --demo-report), when present, spoil a run that
  exited non-zero, did not finish, took a drive time other than its 20 s plus up to 2 s, or had its
  window covered or minimized (`unseen_frames` above 0: eframe paints none of those frames). A report
  without that key, from a build before it, cannot be checked and is kept. A file that is damaged (an
  exit that is no number, a report that is no JSON object) spoils its run too ("exit unreadable",
  "report.json is damaged"): the scan still writes the run's summary.json, and --sum prints the runs
  beside it.
A zero bounds the rate of tears and proves nothing alone: a capture may not read as the compositor
does.
"""
import json
import struct
import sys
import zlib
from collections import Counter
from pathlib import Path

DRIVE_SECONDS = 20  # tear-captures.sh's default drive length
# A test holds SLACK equal to the driver's `TIMED_SLACK_S` (src/drive.rs), the bound of a timed drive.
SLACK = 2.0  # how far past DRIVE_SECONDS a healthy drive's elapsed_s may be
# The interior, as fractions of the capture: clear of the rounded corners and the traffic lights.
X0, X1, Y0, Y1 = 0.10, 0.90, 0.15, 0.92
MIN_GREY = 3  # captures of each grey a run needs before it counts
CLOSE = 16  # the greys are "a few levels apart"; farther than this and the scan doubts them


def fail(message):
    sys.exit(f"tear-scan.py: {message}")


def add_bytes(a, b):
    """Byte-wise (a + b) mod 256 of two equal-length byte strings, with no carry between bytes."""
    n = len(a)
    m7 = int.from_bytes(b"\x7f" * n, "little")
    m8 = int.from_bytes(b"\x80" * n, "little")
    x, y = int.from_bytes(a, "little"), int.from_bytes(b, "little")
    return (((x & m7) + (y & m7)) ^ ((x ^ y) & m8)).to_bytes(n, "little")


def unfilter(kind, line, prev, bpp):
    n = len(line)
    if kind == 0:
        return line
    if kind == 2:
        return add_bytes(line, prev)
    if kind == 1:
        # The running sum along the row, a pixel apart: a doubling scan of byte-wise additions.
        m = (1 << (8 * n)) - 1
        m7 = int.from_bytes(b"\x7f" * n, "little")
        m8 = int.from_bytes(b"\x80" * n, "little")
        y = int.from_bytes(line, "little")
        step = bpp
        while step < n:
            s = (y << (8 * step)) & m
            y = ((y & m7) + (s & m7)) ^ ((y ^ s) & m8)
            step *= 2
        return y.to_bytes(n, "little")
    out = bytearray(line)
    if kind == 3:
        for i in range(n):
            left = out[i - bpp] if i >= bpp else 0
            out[i] = (out[i] + ((left + prev[i]) >> 1)) & 255
        return bytes(out)
    if kind == 4:
        for i in range(n):
            a = out[i - bpp] if i >= bpp else 0
            b = prev[i]
            c = prev[i - bpp] if i >= bpp else 0
            p = a + b - c
            pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
            guess = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
            out[i] = (out[i] + guess) & 255
        return bytes(out)
    raise ValueError(f"PNG filter {kind}")


def read_png(path):
    """(width, height, bytes a pixel, the rows) of an 8-bit RGB or RGBA PNG without interlacing."""
    data = Path(path).read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG")
    pos, idat, header = 8, [], None
    while pos + 8 <= len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind, body = data[pos + 4 : pos + 8], data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat.append(body)
        elif kind == b"IEND":
            break
    if header is None or not idat:
        raise ValueError("no IHDR or IDAT")
    width, height, depth, ctype, _, _, interlace = header
    if depth != 8 or ctype not in (2, 6) or interlace != 0:
        raise ValueError(f"bit depth {depth}, color type {ctype}, interlace {interlace}: not 8-bit RGB(A)")
    bpp = 3 if ctype == 2 else 4
    raw = zlib.decompress(b"".join(idat))
    stride = width * bpp
    if len(raw) != height * (stride + 1):
        raise ValueError("the pixel data has the wrong length")
    rows, prev = [], bytes(stride)
    for y in range(height):
        base = y * (stride + 1)
        prev = unfilter(raw[base], raw[base + 1 : base + 1 + stride], prev, bpp)
        rows.append(prev)
    return width, height, bpp, rows


def interior(path):
    """(Counter of the flat colors of the interior's rows, how many of its rows are not flat)."""
    width, height, bpp, rows = read_png(path)
    x0, x1 = int(width * X0), int(width * X1)
    colors, ragged = Counter(), 0
    for y in range(int(height * Y0), int(height * Y1)):
        line = rows[y][x0 * bpp : x1 * bpp]
        pixel = line[:bpp]
        if line == pixel * (x1 - x0):
            colors[pixel] += 1
        else:
            ragged += 1
    return colors, ragged


def hexcolor(pixel):
    return "#" + pixel.hex()


def read_status(path):
    """The run's exit status, None with no file, and "unreadable" when the file is damaged: that spoils
    the run and nothing else, as a traceback here would end the scan of the runs beside it. close-scan.py
    reads its runs' exit files with this too."""
    try:
        return int(path.read_text())
    except FileNotFoundError:
        return None
    except (OSError, ValueError):
        return "unreadable"


def read_report(path):
    """The drive's report, None with no file, and False when the file is damaged (not JSON, not UTF-8,
    not an object, or not a file). close-scan.py reads its runs' reports with this too, and takes either
    for a report that says nothing."""
    try:
        report = json.loads(path.read_text())
    except FileNotFoundError:
        return None
    except (OSError, ValueError):
        return False
    return report if isinstance(report, dict) else False


def scan(folder):
    folder = Path(folder)
    files = sorted(folder.glob("*.png"))
    if not files:
        fail(f"no PNG in {folder}")
    seen = []
    for f in files:
        try:
            seen.append(interior(f))
        except (ValueError, zlib.error, struct.error) as err:
            fail(f"{f}: {err}")
    flat = Counter(next(iter(c)) for c, ragged in seen if len(c) == 1 and ragged == 0)
    common = flat.most_common(2)
    if len(common) < 2 or common[1][1] < MIN_GREY:
        fail(
            f"{folder}: fewer than two flat colors, {MIN_GREY} captures each (flat colors seen: "
            f"{', '.join(f'{hexcolor(p)} x{n}' for p, n in flat.most_common(4)) or 'none'}); "
            "was a demo build started with ARRAKIS_METAL_TEAR=1?"
        )
    (a, na), (b, nb) = common
    far = max(abs(x - y) for x, y in zip(a[:3], b[:3]))
    greys = {a, b}
    kinds = []
    for colors, ragged in seen:
        if len(colors) == 1 and ragged == 0:
            kinds.append("a" if next(iter(colors)) == a else "b" if next(iter(colors)) == b else "other")
        elif set(colors) <= greys and (len(colors) == 2 or ragged):
            kinds.append("both")
        else:
            kinds.append("other")
    settled_at = [i for i, k in enumerate(kinds) if k in ("a", "b")]
    first, last = settled_at[0], settled_at[-1]
    counts = Counter(kinds[first : last + 1])
    status = read_status(folder / "exit")
    report = read_report(folder / "report.json")
    spoiled = []
    if status not in (None, 0):
        spoiled.append(f"exit {status}")
    if report is False:
        spoiled.append("report.json is damaged")
    elif report is not None:
        elapsed = report.get("elapsed_s")
        if report.get("finished") is not True:
            spoiled.append(f"finished={report.get('finished')}")
        if not isinstance(elapsed, (int, float)) or not DRIVE_SECONDS <= elapsed <= DRIVE_SECONDS + SLACK:
            spoiled.append(f"elapsed_s={elapsed} against a {DRIVE_SECONDS} s drive")
        unseen = report.get("unseen_frames")
        if isinstance(unseen, int) and unseen > 0:
            spoiled.append(f"the window was covered or minimized for {unseen} frames")
    summary = {
        "run": folder.name,
        "captures": len(files),
        "before": first,
        "after": len(files) - 1 - last,
        "settled": last - first + 1,
        "a": counts["a"],
        "b": counts["b"],
        "both": counts["both"],
        "other": counts["other"],
        "tears": counts["both"] + counts["other"],
        "a_color": hexcolor(a),
        "b_color": hexcolor(b),
        "greys_far_apart": far > CLOSE,
        "elapsed_s": report.get("elapsed_s") if isinstance(report, dict) else None,
        "spoiled": "; ".join(spoiled) or None,
    }
    (folder / "summary.json").write_text(json.dumps(summary, indent=1))
    return summary


def line(s):
    flag = f" SPOILED ({s['spoiled']}): left out of the total" if s["spoiled"] else ""
    far = " WARNING: the two greys are more than 16 levels apart; is this ARRAKIS_METAL_TEAR?" if s["greys_far_apart"] else ""
    return (
        f"{s['run']} tears={s['tears']} settled={s['settled']} captures={s['captures']} "
        f"before={s['before']} after={s['after']} a={s['a']}({s['a_color']}) b={s['b']}({s['b_color']}) "
        f"both={s['both']} other={s['other']} elapsed_s={s['elapsed_s']}{flag}{far}"
    )


def main():
    if len(sys.argv) >= 2 and sys.argv[1] == "--sum":
        if len(sys.argv) < 3:
            sys.exit(__doc__)
        runs = []
        for folder in sys.argv[2:]:
            summary_file = Path(folder) / "summary.json"
            if not summary_file.exists():
                fail(f"{folder} has no summary.json; run tear-captures.sh first")
            runs.append(json.loads(summary_file.read_text()))
        for s in runs:
            print(line(s))
        good = [s for s in runs if not s["spoiled"]]
        tears, settled = sum(s["tears"] for s in good), sum(s["settled"] for s in good)
        print(f"TOTAL tears={tears} settled={settled} runs={len(good)} of {len(runs)}")
    elif len(sys.argv) == 2 and not sys.argv[1].startswith("-"):
        print(line(scan(sys.argv[1])))
    else:
        sys.exit(__doc__)


# close-scan.py reads this file's PNG reader, and its `read_status` and `read_report`.
if __name__ == "__main__":
    main()
