#!/bin/sh
# What the real app holds in each state, in MiB of phys_footprint: never opened (30 s), open (5, 30
# and 90 s), 2 s and about 60 s after the first close, then after six more opens of 5 s and closes
# (2 s and about 60 s after the last). The table of the real app of the memory night, 2026-10-30 to 31,
# one run of it. That night's runs read the last close at 5 s and about 65 s, not 2 s and 60 s: the loop
# below ended in a `sleep 5`. Their figures are what they are; this script now reads both closes
# alike.
#
# THE REAL APP, ON THE REAL DISPLAYS: run this only when you mean to. The schedule starts six hours off,
# so nothing tints, but the window opens and closes on the owner's screen, and a click on the menu-bar
# item moves the pointer.
# It needs the terminal to be allowed to control the computer (Accessibility), for System Events and
# for axtool. About 5 minutes a run.
#
# usage: real-cycles.sh <arm> <root holding arrakis-measure/real/release/arrakis> [VAR=VALUE ...]
#   arm   a label, printed first on the result line
#   root  a TMPDIR for which `CARGO_TARGET_DIR=<root>/arrakis-measure/real cargo build --release` was
#         run (scripts/measure.sh describes the layout); home/ and tmp/ for the run go beside it
#   VAR=VALUE  environment for the app only, such as MallocSpaceEfficient=1
# Q divides the waits between the reads (wait_s), for a quick try. It does not divide the 5 s a window
# stays open or closed in the loop, the pause after the menu-bar click, or the time of a read.
# Prints one line: the MiB, then windows_after_close1 (0 if the first close took), reopens (the reopens
# that showed a window, 6 at most) and windows_left (at the end).
# It finds axtool in $TMPDIR (scripts/measure/axtool.c says how to build it).
# After each of the eight reads, the first thing once phys_footprint is read, `footprint -p <pid> -f bytes`
# goes to a file of that read's name (never, open5, open30, open90, close1_2s, close1_60s, close7_2s,
# close7_60s: the result line's) in <root>/arrakis-measure/cycles/<arm>-<date>-<time>/, a folder of its own
# for each run, named on stderr. So a figure above its limit splits into the graphics rows ("IOSurface",
# "IOAccelerator (graphics)", "Owned physical footprint (unmapped) (graphics)"), MALLOC_SMALL and a whole
# surface. The call is quick (0.02 s on a shell) and comes after the read, so it moves none; the reads
# after it come that much later. `footprint --unmapped`, which names who holds the unmapped graphics
# memory, needs root and is left out. An arm names one folder, so one with a `/` exits 2.
set -u
fail() {
    echo "real-cycles.sh: $*" >&2
    exit 1
}
. "$(dirname "$0")/common.sh"
AX="${TMPDIR%/}/axtool"
[ -x "$AX" ] || { echo "real-cycles.sh: build axtool into $AX first" >&2; exit 2; }
# The app below runs under its own TMPDIR, which hides a copy the owner has running from it.
refuse_if_running
arm=$1 root=$2
shift 2
case $arm in
    '' | . | .. | */*)
        echo "real-cycles.sh: an arm names one folder, and '$arm' does not" >&2
        exit 2
        ;;
esac
Q="${Q:-1}"
M="$root/arrakis-measure"
S="$M/home"
T="$M/tmp"
C="$S/Library/Application Support/dev.Arrakis.Arrakis"
BIN="$M/real/release/arrakis"
RUN="$M/cycles/$arm-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$C" "$T" "$RUN"
echo "real-cycles.sh: the footprint files of this run are in $RUN" >&2
# The window starts six hours off, so nothing tints.
printf '{"schedule_on":true,"start":"%s","end":"%s","fade_minutes":20,"kelvin":6490,"brightness":100}\n' \
    "$(date -v+6H +%H:%M)" "$(date -v+8H +%H:%M)" >"$C/settings.json"

wait_s() { sleep "$(($1 / Q))"; }
# The categories of the read $1, after its phys_footprint. A failed call leaves its file as it ended.
cats() { footprint -p "$pid" -f bytes >"$RUN/$1.txt" 2>&1 || true; }
osa() { perl -e 'alarm 15; exec @ARGV' osascript -e "tell application \"System Events\" to tell (first process whose unix id is $pid) to $1" 2>&1; }
windows() { osa 'get count of windows'; }
close_window() { osa 'click (first button of window 1 whose subrole is "AXCloseButton")' >/dev/null; }
open_window() {
    box=$(osa 'get {position, size} of menu bar item 1 of menu bar 2' | tr -d ' ')
    cx=$(echo "$box" | awk -F, '{print $1 + $3 / 2}')
    cy=$(echo "$box" | awk -F, '{print $2 + $4 / 2}')
    "$AX" click "$cx" "$cy"
    sleep 0.6
    osa 'click menu item "Open Arrakis..." of menu 1 of menu bar item 1 of menu bar 2' >/dev/null
}

env "$@" HOME="$S" TMPDIR="$T" "$BIN" --hidden >/dev/null 2>&1 &
pid=$!
wait_s 30
never=$("$AX" fp "$pid")
cats never
kill -TERM "$pid"
wait "$pid" 2>/dev/null

env "$@" HOME="$S" TMPDIR="$T" "$BIN" >/dev/null 2>&1 &
pid=$!
wait_s 5
open5=$("$AX" fp "$pid")
cats open5
wait_s 25
open30=$("$AX" fp "$pid")
cats open30
wait_s 60
open90=$("$AX" fp "$pid")
cats open90
close_window
wait_s 2
close1_2=$("$AX" fp "$pid")
cats close1_2s
wait_s 58
close1=$("$AX" fp "$pid")
cats close1_60s
# Read after the 60 s footprint, so it moves neither read: a close that did not take leaves 1.
took=$(windows)
reopens=0
for i in 2 3 4 5 6 7; do
    open_window
    sleep 5
    [ "$(windows)" = 1 ] && reopens=$((reopens + 1))
    close_window
    # The last close is read as the first was, at 2 s and about 60 s; between closes the window rests 5 s.
    [ "$i" = 7 ] && break
    sleep 5
done
wait_s 2
close7_2=$("$AX" fp "$pid")
cats close7_2s
wait_s 58
close7=$("$AX" fp "$pid")
cats close7_60s
left=$(windows)
kill -TERM "$pid"
wait "$pid" 2>/dev/null
echo "$arm $(date +%H:%M:%S) never=$never open5=$open5 open30=$open30 open90=$open90 close1_2s=$close1_2 close1_60s=$close1 close7_2s=$close7_2 close7_60s=$close7 windows_after_close1=$took reopens=$reopens windows_left=$left"
