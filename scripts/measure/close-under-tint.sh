#!/bin/sh
# Does a close move the tint? The real app, tinted at 6490 K (invisible, but not the identity table),
# one close while tabletool reads the main display's table for 12 s (about 3,600 reads).
# It prints the table before the app, while it is tinted (a line for each change, then the read,
# change and failed-read counts), and after its quit, and whether a window was left.
#
# THE REAL APP PAINTS THE REAL DISPLAYS, with a tint too small to see: run this only when you mean to.
# It needs the terminal to be allowed to control the computer (Accessibility), for System Events.
#
# usage: close-under-tint.sh <arm> <root holding arrakis-measure/real/release/arrakis>
#   root  as for real-cycles.sh
# It finds tabletool in $TMPDIR (scripts/measure/tabletool.c says how to build it).
set -u
fail() {
    echo "close-under-tint.sh: $*" >&2
    exit 1
}
. "$(dirname "$0")/common.sh"
TABLE="${TMPDIR%/}/tabletool"
[ -x "$TABLE" ] || { echo "close-under-tint.sh: build tabletool into $TABLE first" >&2; exit 2; }
# The app below runs under its own TMPDIR, which hides a copy the owner has running from it.
refuse_if_running
arm=$1 root=$2
M="$root/arrakis-measure"
S="$M/home"
T="$M/tmp"
C="$S/Library/Application Support/dev.Arrakis.Arrakis"
mkdir -p "$C" "$T"
# The window began two hours ago, so the level is full from the start.
printf '{"schedule_on":true,"start":"%s","end":"%s","fade_minutes":20,"kelvin":6490,"brightness":100}\n' \
    "$(date -v-2H +%H:%M)" "$(date -v+6H +%H:%M)" >"$C/settings.json"
before=$("$TABLE" 0.01 | head -n 1)
HOME="$S" TMPDIR="$T" "$M/real/release/arrakis" >/dev/null 2>&1 &
pid=$!
sleep 8
"$TABLE" 12 >"$T/table.txt" 2>&1 &
reader=$!
sleep 4
perl -e 'alarm 15; exec @ARGV' osascript -e "tell application \"System Events\" to tell (first process whose unix id is $pid) to click (first button of window 1 whose subrole is \"AXCloseButton\")" >/dev/null 2>&1
wait "$reader"
left=$(perl -e 'alarm 15; exec @ARGV' osascript -e "tell application \"System Events\" to tell (first process whose unix id is $pid) to get count of windows" 2>&1)
echo "$arm windows left: $left"
kill -TERM "$pid"
wait "$pid" 2>/dev/null
sleep 1
after=$("$TABLE" 0.01 | head -n 1)
echo "$arm no app: $before"
sed "s/^/$arm tinted: /" "$T/table.txt"
echo "$arm after quit: $after"
