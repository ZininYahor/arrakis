#!/bin/sh
# Arrakis 0.1.0 against f.lux 42.2, each alone, launched by `open`, for the README's "Next to f.lux"
# table. It reads the counters of rusagetool.c (CPU time, the CPU's energy, interrupt wake-ups,
# phys_footprint) and the gamma table of tabledump.c. This is the tool for reading the comparison again,
# not the file that ran on 2026-11-16, 11:25 to 12:05: this one refuses to start while an app runs,
# spoils a turn that goes wrong and reads f.lux by day, which that file did not.
#
# THIS RUNS THE REAL APPS ON THE REAL DISPLAYS. It tints every display for about 45 minutes, quits and
# starts Arrakis (kill -TERM) and f.lux (a quit event), writes f.lux's `dayColorTemp` (1200), deletes it
# and runs `defaults import` of the snapshot's prefs into it for the day turn, and clicks through System
# Events to close each app's window (and OK on f.lux's sheet). Run it only on a Mac that you can leave
# alone for about 45 minutes. It does not put your apps back: after it, check f.lux's prefs against the
# snapshot (the key `dayColorTemp` must be absent: `defaults delete org.herf.Flux dayColorTemp`), start
# your f.lux and your Arrakis again, and compare the table with the snapshot.
#
# Before it: both apps quit (the script refuses to start while arrakis or Flux runs), the table at its
# baseline (the script skips a turn whose table is not), the terminal allowed to control the computer
# (Accessibility: System Events reads the window count and presses the close button), and the machine
# quiet, with `caffeinate -dims -t <seconds>` running.
#
# One display. The baseline and the end-of-turn checks read every display (tabledump), but the wait for a
# still table (settle) watches the main display only (tabletool). On a Mac with a second display, that
# one is checked at the start and the end of a turn and is not watched for a change in between.
#
# usage: APP_A=<Arrakis.app> APP_F=<Flux.app> FLUX_DIR=<folder> flux-compare.sh
#   APP_A      no default. The Arrakis bundle to start, a full path, for example /Applications/Arrakis.app.
#   APP_F      no default. The f.lux bundle to start, a full path, for example ~/Applications/Flux.app.
#   FLUX_DIR   no default. A folder that holds snapshot/ with two files, made by hand with both apps quit:
#                snapshot/table-baseline.txt   "$TMPDIR/tabledump" > table-baseline.txt
#                snapshot/flux-prefs.plist     defaults export org.herf.Flux flux-prefs.plist
#              The run makes arrakis-home/ (Arrakis's own HOME and settings.json) and run-HHMM/ (log.txt
#              and before-<turn>.txt, the process list before each turn) in it, and prints the run's folder.
#   TMPDIR     where the tools are: build rusagetool, tabledump and tabletool (scripts/measure/*.c, each file
#              gives its clang line) into it.
#
# The turns: F1b F tint, A2 A tint, F2 F tint, A3 A tint, AR A red, DA A day, DF F day. Each: `open` the
# app (both open their window at launch), read it at 30 s, close the window, wait for a still table,
# read the counters from 90 s after launch and again 300 s later, quit, check that the table is back at
# its baseline. A tint is 1200 K for the whole run (Arrakis: a window from two hours before the start to
# three hours after it, brightness 100, so that no fade and no midnight falls inside the run; f.lux:
# `dayColorTemp` 1200), red is Arrakis at 850 K (its table only), and day is Arrakis with the schedule
# off, and f.lux with the snapshot's prefs and no `dayColorTemp` of its own.
#
# A turn whose window is still up after the close, or whose table does not settle, is logged
# `SPOILED <turn>: <why>`, quit as usual, and left without its rest reads: the log says it, and the
# turn is not used. The script logs each reading of rusagetool as it is and takes no difference: the
# rest figures are worked out from the log.
S="${FLUX_DIR:-}"
[ -n "$S" ] || { echo "flux-compare.sh: set FLUX_DIR to the folder that holds snapshot/ (see the header)" >&2; exit 2; }
APP_A="${APP_A:-}"
[ -n "$APP_A" ] || { echo "flux-compare.sh: set APP_A to the Arrakis bundle, for example /Applications/Arrakis.app (see the header)" >&2; exit 2; }
APP_F="${APP_F:-}"
[ -n "$APP_F" ] || { echo "flux-compare.sh: set APP_F to the f.lux bundle, for example ~/Applications/Flux.app (see the header)" >&2; exit 2; }
T="${TMPDIR%/}"
for tool in rusagetool tabledump tabletool; do
    [ -x "$T/$tool" ] || { echo "flux-compare.sh: build $tool into $T first (scripts/measure/$tool.c says how)" >&2; exit 2; }
done
O=$S/snapshot
for f in table-baseline.txt flux-prefs.plist; do
    [ -f "$O/$f" ] || { echo "flux-compare.sh: $O/$f is missing (see the header)" >&2; exit 2; }
done
for proc in arrakis Flux; do
    running=$(pgrep -x "$proc" 2>/dev/null || true)
    [ -z "$running" ] || { echo "flux-compare.sh: $proc is running (pid $(echo $running)); quit it first" >&2; exit 2; }
done
R=$S/run-$(date +%H%M)
mkdir -p "$R"
LOG=$R/log.txt
H=$S/arrakis-home
C="$H/Library/Application Support/dev.Arrakis.Arrakis"
mkdir -p "$C"
REST=300
# Arrakis's window, from two hours ago to three hours on: the run takes about 45 minutes, so no fade (20
# minutes at either end) falls inside it, whatever time it starts, and the window may cross midnight, as
# a schedule's may, so a midnight inside the run changes nothing.
WIN_START=$(date -v-2H +%H:%M)
WIN_END=$(date -v+3H +%H:%M)
BASE=$(sed 's/.*hash=\([0-9a-f]*\).*/\1/' "$O/table-baseline.txt")

log() { echo "$(date '+%H:%M:%S') $*" >>"$LOG"; }
osa() { perl -e 'alarm 15; exec @ARGV' osascript -e "tell application \"System Events\" to tell (first process whose unix id is $1) to $2" 2>&1; }
hash_now() { "$T/tabledump" | sed 's/.*hash=\([0-9a-f]*\).*/\1/'; }
# The processes that began after before-<name>.txt and whose path matches: their pid, then the whole rest of
# the ps line (a path may hold a space).
helpers() {
    ps -A -o pid=,comm= | awk -v before="$R/before-$1.txt" '
        BEGIN { while ((getline l < before) > 0) { split(l, a, " "); seen[a[1]] = 1 } }
        { pid = $1; sub(/^ *[0-9]+ +/, ""); comm = $0 }
        !(pid in seen) && comm ~ /WebKit|Flux|arrakis|AutoFill|ViewService|Sparkle|Autoupdate|Updater/ { print pid, comm }'
}
# The five busiest processes, each as its %cpu and the last part of its path.
top5() { ps -A -o %cpu=,comm= -r | head -5 | awk '{ c = $1; sub(/^ *[0-9.]+ +/, ""); n = split($0, p, "/"); printf "%s %s; ", c, p[n] }'; }
read_all() { # pid name tag
    log "$3 main $1 $("$T/rusagetool" "$1")"
    helpers "$2" | while read -r hp hc; do
        [ "$hp" = "$1" ] && continue
        log "$3 helper $hp ${hc##*/} $("$T/rusagetool" "$hp")"
    done
}
settle() {
    for k in 1 2 3 4 5 6; do
        c=$("$T/tabletool" 20 | tail -1)
        log "settle $k $c"
        case "$c" in *changes=0*) return 0 ;; esac
    done
    return 1
}
windows() { osa "$1" 'get count of windows'; }
close_window() { osa "$1" 'click (first button of window 1 whose subrole is "AXCloseButton")' >/dev/null; }
quit_app() { # kind pid
    if [ "$1" = A ]; then kill -TERM "$2"; else perl -e 'alarm 20; exec @ARGV' osascript -e 'tell application id "org.herf.Flux" to quit' >/dev/null 2>&1; fi
    for i in $(seq 1 20); do kill -0 "$2" 2>/dev/null || break; sleep 1; done
    if kill -0 "$2" 2>/dev/null; then
        log "STILL RUNNING $2: TERM"
        kill -TERM "$2"
        sleep 5
    fi
    sleep 3
    h=$(hash_now)
    if [ "$h" = "$BASE" ]; then log "quit: table back to baseline"; else log "quit: TABLE NOT BACK: $("$T/tabledump")"; fi
}
# Arrakis's settings.json: the schedule on or off, the window around the present, the kelvin.
settings() { # schedule_on kelvin
    printf '{"schedule_on":%s,"start":"%s","end":"%s","fade_minutes":20,"kelvin":%s,"brightness":100}\n' \
        "$1" "$WIN_START" "$WIN_END" "$2" >"$C/settings.json"
}
# A turn that cannot be used: say so, quit the app as usual, and take no rest reads.
spoil() { # name kind pid why
    log "SPOILED $1: $4"
    quit_app "$2" "$3"
}
arm() { # name kind mode
    name=$1 kind=$2 mode=$3
    ps -A -o pid=,comm= >"$R/before-$name.txt"
    log "== $name $kind $mode load=$(sysctl -n vm.loadavg | tr -d '{}') top=$(top5)"
    if [ "$(hash_now)" != "$BASE" ]; then log "SKIP: the table is not the baseline before $name"; return; fi
    if [ "$kind" = A ]; then
        case "$mode" in
        tint) settings true 1200 ;;
        red) settings true 850 ;;
        day) settings false 1200 ;;
        esac
        open --env HOME="$H" "$APP_A"
        proc=arrakis
    else
        case "$mode" in
        tint) defaults write org.herf.Flux dayColorTemp -int 1200 ;;
        day)
            # `defaults import` merges and does not replace: without this, the key stays at 1200 from the tint turns.
            defaults delete org.herf.Flux dayColorTemp 2>/dev/null || true
            defaults import org.herf.Flux "$O/flux-prefs.plist"
            ;;
        esac
        open "$APP_F"
        proc=Flux
    fi
    t0=$(date +%s)
    pid=
    for i in $(seq 1 20); do pid=$(pgrep -x "$proc" | head -1); [ -n "$pid" ] && break; sleep 0.5; done
    [ -z "$pid" ] && { log "NO PROCESS for $name"; return; }
    log "launched pid=$pid"
    # Both apps open their window at launch. It stays open 30 s, is read, then closes.
    sleep 30
    log "windows at 30 s: $(windows "$pid")"
    read_all "$pid" "$name" open30
    close_window "$pid"
    sleep 1
    # f.lux puts up a sheet when its window is closed, and the window stays until it is answered: OK.
    if [ "$kind" = F ]; then osa "$pid" 'click button "OK" of sheet 1 of window 1' >/dev/null 2>&1; sleep 1; fi
    n=$(windows "$pid")
    log "windows after close: $n"
    [ "$n" = 0 ] || { spoil "$name" "$kind" "$pid" "$n windows after the close"; return; }
    settle || { spoil "$name" "$kind" "$pid" "the table did not settle"; return; }
    log "table $("$T/tabledump")"
    if [ "$mode" = red ]; then quit_app "$kind" "$pid"; return; fi
    now=$(date +%s)
    [ $((now - t0)) -lt 90 ] && sleep $((90 - (now - t0)))
    log "rest from $(($(date +%s) - t0)) s after launch"
    read_all "$pid" "$name" rest0
    sleep "$REST"
    read_all "$pid" "$name" rest1
    log "table at end $("$T/tabledump")"
    quit_app "$kind" "$pid"
}

log "start; baseline $BASE"
arm F1b F tint
arm A2 A tint
arm F2 F tint
arm A3 A tint
arm AR A red
arm DA A day
arm DF F day
log "prefs now: $(defaults read org.herf.Flux 2>&1 | tr -s ' \n' ' ')"
log "end"
echo "$R"
