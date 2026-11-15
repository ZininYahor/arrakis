#!/bin/sh
# Does a drag tear? One count: a drive of the demo build, on NullScreen, while the window as the window
# server composes it is captured (`screencapture -o -l<id>`, about 6 a second), then scanned by
# tear-scan.py. The demo build's Metal painter with
# ARRAKIS_METAL_TEAR=1 draws each frame as one flat grey, 96 or 100 by turns, and a capture that holds both
# greys, or neither, inside the window is a tear. A zero bounds the rate and proves nothing alone; the
# count is kept to compare later builds and other machines.
#
# THE WINDOW OPENS ON THE OWNER'S SCREEN and takes the focus for 20 s, as every drive with a window does:
# run this only when you mean to. It paints no tint (NullScreen). It needs the terminal to be allowed to
# record the screen, for screencapture. About 30 s a run.
#
# usage: tear-captures.sh <label> <root> [VAR=VALUE ...]
#   label  names the run's folder, <root>/arrakis-measure/tear/<label>: its PNGs, the drive's report,
#          its exit status and summary.json. A label already used is emptied first.
#   root   as for real-cycles.sh: a TMPDIR for which `CARGO_TARGET_DIR=<root>/arrakis-measure/drive
#          cargo build --release --features demo` was run (scripts/measure.sh describes the layout);
#          home/ and tmp/ for the run go beside it
#   VAR=VALUE  environment for the app only, after ARRAKIS_METAL_TEAR=1, which is always set
# TEAR_ARGS replaces the app's arguments (default: --demo-drive drag --demo-seconds 20).
# It finds winid in $TMPDIR (scripts/measure/winid.c says how to build it). It prints one line:
# tears=, settled=, the captures and the greys; tear-scan.py --sum <folder>... adds runs up.
set -u
fail() {
    echo "tear-captures.sh: $*" >&2
    exit 1
}
. "$(dirname "$0")/common.sh"
[ $# -ge 2 ] || {
    echo "usage: tear-captures.sh <label> <root> [VAR=VALUE ...]  (see the header)" >&2
    exit 2
}
WINID="${TMPDIR%/}/winid"
[ -x "$WINID" ] || { echo "tear-captures.sh: build winid into $WINID first" >&2; exit 2; }
# The app below runs under its own TMPDIR, which hides a copy the owner has running from it.
refuse_if_running
label=$1 root=$2
shift 2
M="$root/arrakis-measure"
BIN="$M/drive/release/arrakis"
[ -x "$BIN" ] || fail "no $BIN; build it first: CARGO_TARGET_DIR=$M/drive cargo build --release --features demo"
OUT="$M/tear/$label"
mkdir -p "$M/home" "$M/tmp" "$OUT"
rm -f "$OUT"/*.png "$OUT"/exit "$OUT"/report.json "$OUT"/summary.json "$OUT"/capture.log
ARGS="${TEAR_ARGS:---demo-drive drag --demo-seconds 20}"
case $ARGS in
    *--demo-drive*) ARGS="$ARGS --demo-report $OUT/report.json" ;;
esac
# $ARGS is split on purpose: it holds options, none with a space.
# shellcheck disable=SC2086
env ARRAKIS_METAL_TEAR=1 "$@" HOME="$M/home" TMPDIR="$M/tmp" "$BIN" $ARGS >"$OUT/app.log" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null' INT TERM
deadline=$(($(date +%s) + 80))
n=0
while kill -0 "$pid" 2>/dev/null; do
    if [ "$(date +%s)" -gt "$deadline" ]; then
        kill "$pid" 2>/dev/null
        echo "tear-captures.sh: the app ran past 80 s; killed" >&2
        break
    fi
    id=$("$WINID" "$pid" 2>/dev/null) || { sleep 0.05; continue; }
    n=$((n + 1))
    /usr/sbin/screencapture -x -o -l"$id" "$OUT/$(printf %04d "$n").png" 2>>"$OUT/capture.log"
done
wait "$pid"
echo $? >"$OUT/exit"
[ "$n" -gt 0 ] || fail "no window was found: see $OUT/app.log"
python3 "$(dirname "$0")/tear-scan.py" "$OUT"
