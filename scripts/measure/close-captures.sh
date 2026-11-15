#!/bin/sh
# Does a close show the bare window? The captures: real closes of the shipped close, on NullScreen, with
# the window as the window server composes it captured across each close, as tear-captures.sh captures a
# drag (`screencapture -o -l<id>`), then sorted by close-scan.py.
#
# It runs `--demo-drive open-close` ROUNDS times, for CLOSES closes each: by default 3 x 10, so 30 real
# closes, 30 windows, about 1.7 min.
#
# THE WINDOW OPENS ON THE OWNER'S SCREEN AND TAKES THE FOCUS, about 1 s in every 3, for the whole run: run
# this only when you mean to. Nothing here paints a tint (NullScreen), or touches a display's tables. It
# needs the terminal to be allowed to record the screen, for screencapture.
#
# Three capture loops run at once, staggered, as one takes about 150 ms and a bare window, if it comes,
# lasts only until the window itself goes. Each starts a while after the window appears, as a close comes
# 1 s after the first frame, and goes on until the window has gone. So a zero bounds how often a bare window
# shows and proves nothing alone.
#
# usage: close-captures.sh <label> <root>
#   label  names the run's folder, <root>/arrakis-measure/close/<label>, with a folder per run in it,
#          run-<round>: its PNGs, its drive's report, its exit status and log. A label already used is
#          emptied first, so a label must name one folder: empty, `.`, `..` or one with a `/` exits 2.
#   root   as for tear-captures.sh: a TMPDIR for which `CARGO_TARGET_DIR=<root>/arrakis-measure/drive
#          cargo build --release --features demo` was run; home/ and tmp/ for the runs go beside it.
# ROUNDS and CLOSES change the counts above.
# CLOSE_CAPTURES_BIN names another demo build, and WINID and SCREENCAPTURE stand in for those tools.
# CLOSE_CAPTURES_LIMIT is how many seconds an app may run: by default a run's cycles at 4 s each, against
# the 3.1 s they take, and 60 s more. Past that it is killed, and its run is spoiled.
# It finds winid in $TMPDIR (scripts/measure/winid.c says how to build it).
# It prints one line for the runs and where the pictures are (close-scan.py says what they mean). Exits
# as close-scan.py does: 0 when every run held, 1 when one was spoiled, 2 when nothing could be captured.
# An interrupt ends all of it at once, the owner's way out of a window that takes the focus: the app and
# the capture loops go, no further round runs and nothing is scanned. It exits 130, or 143 on a TERM.
set -u
fail() {
    echo "close-captures.sh: $*" >&2
    exit 1
}
HERE=$(dirname "$0")
. "$HERE/common.sh"
[ $# -eq 2 ] || {
    echo "usage: close-captures.sh <label> <root>  (see the header)" >&2
    exit 2
}
label=$1 root=$2
case $label in
    '' | . | .. | */*)
        echo "close-captures.sh: a label names one folder, and '$label' does not" >&2
        exit 2
        ;;
esac
WINID="${WINID:-${TMPDIR%/}/winid}"
SCREENCAPTURE="${SCREENCAPTURE:-/usr/sbin/screencapture}"
[ -x "$WINID" ] || { echo "close-captures.sh: build winid into $WINID first" >&2; exit 2; }
# The app below runs under its own TMPDIR, which hides a copy the owner has running from it.
refuse_if_running
ROUNDS="${ROUNDS:-3}"
CLOSES="${CLOSES:-10}"
LIMIT="${CLOSE_CAPTURES_LIMIT:-$((CLOSES * 4 + 60))}"
M="$root/arrakis-measure"
BIN="${CLOSE_CAPTURES_BIN:-$M/drive/release/arrakis}"
[ -x "$BIN" ] || fail "no $BIN; build it first: CARGO_TARGET_DIR=$M/drive cargo build --release --features demo"
OUT="$M/close/$label"
rm -rf "$OUT"
mkdir -p "$M/home" "$M/tmp" "$OUT" || fail "cannot write to $OUT"

# Captures $pid's window into $1 until the process ends. $2 is how long after a new window this loop's
# first capture comes, which staggers the loops.
capture() {
    dir=$1 settle=$2 seen='' n=0
    while kill -0 "$pid" 2>/dev/null; do
        id=$("$WINID" "$pid" 2>/dev/null) || {
            seen=''
            sleep 0.02
            continue
        }
        if [ "$id" != "$seen" ]; then
            seen=$id
            sleep "$settle"
            continue
        fi
        n=$((n + 1))
        # The name holds the window, as each open makes a new one: close-scan.py counts closes by it.
        "$SCREENCAPTURE" -x -o -l"$id" "$dir/w$id-$settle-$(printf %04d "$n").png" 2>>"$dir/capture.log"
    done
}

# A background job here ignores INT, so an interrupt reaches these only through this. Each name is empty
# until its process starts, and goes away unquoted.
pid='' first='' second='' third=''
stop() {
    # shellcheck disable=SC2086
    kill $pid $first $second $third 2>/dev/null
    exit "$1"
}
trap 'stop 130' INT
trap 'stop 143' TERM

# Waits for $pid until the run's limit, and then ends it: a TERM, and a KILL for a hung app that takes no
# notice of one. The exit status that leaves spoils the run.
await_app() {
    deadline=$(($(date +%s) + LIMIT))
    while kill -0 "$pid" 2>/dev/null; do
        if [ "$(date +%s)" -gt "$deadline" ]; then
            echo "close-captures.sh: the app ran past $LIMIT s; killed" >&2
            kill "$pid" 2>/dev/null
            sleep 5
            kill -9 "$pid" 2>/dev/null
            break
        fi
        sleep 0.5
    done
}

run() {
    dir=$1
    mkdir -p "$dir" || fail "cannot write to $dir"
    env HOME="$M/home" TMPDIR="$M/tmp" "$BIN" --demo-drive open-close \
        --demo-seconds "$((CLOSES * 3))" --demo-report "$dir/report.json" >"$dir/app.log" 2>&1 &
    pid=$!
    capture "$dir" 0.60 &
    first=$!
    capture "$dir" 0.65 &
    second=$!
    capture "$dir" 0.70 &
    third=$!
    await_app
    wait "$pid"
    echo $? >"$dir/exit"
    wait "$first" "$second" "$third"
    pid='' first='' second='' third=''
}

round=1
while [ "$round" -le "$ROUNDS" ]; do
    run "$OUT/run-$round"
    round=$((round + 1))
done
python3 "$HERE/close-scan.py" "$OUT" "$CLOSES"
