#!/bin/sh
# Measures what Arrakis costs.
#
#   scripts/measure.sh real <s1|s2|s3|s4> [runs]
#       The release build on the real displays: S1 to S4. It tints them, so
#       run it only when you mean to.
#   scripts/measure.sh drive <scenario> [runs] [seconds]
#       The demo build's --demo-drive, which never touches the displays.
#       closed-day, closed-night and open-idle are about rest: each run first
#       settles for SETTLE seconds (30 unless the environment says), then is
#       sampled for [seconds]. The driver runs for both.
#
# Runs default to 3, a drive's seconds to 30. Raw files go under
# $TMPDIR/arrakis-measure/. Each run prints one line, and the runs their
# median.
#
# A drive's line carries the driver's own `unseen=N`: the frames that began with
# the window covered or minimized, which eframe paints none of, so a run with N
# above 0 is spoiled and its per-frame figures read too good. The median is
# over the runs that held: a line on stdout before it names each run left out
# and why, and with every run spoiled the script says so and prints no median.
# A run with no `unseen` (a real run, or a driver from before the key) cannot
# be checked, and is kept.
#
# It also carries the driver's `spoiled=`: `-` for a drive that held, and for one
# that did not, either `elapsed_s:<x>,bound_s:<from>..<to>` (the machine slowed or
# stopped it: its time is outside `Scenario::bound` in src/drive.rs) or
# `cycles:<done>/<planned>` (a cycle broke off, as a window did not open or close,
# whatever its time), whose per-frame figures do not compare. Such a run is left
# out of the median the same way, with a line that gives the pair. A line with no
# `spoiled` (a driver from before the key, as the arms of a comparison may be)
# counts as in its time.
#
# $TMPDIR is the root that
# scripts/measure/build-arm.sh lays an arm out in, so TMPDIR=<root> reads an arm.
#
# The app runs with the bundle's environment: the keys of LSEnvironment in
# scripts/Info.plist (MallocSpaceEfficient=1 at present), which LaunchServices
# gives a process it starts from the bundle and a bare run of the binary does
# not get. Otherwise every reading would be of a build that does not ship.
# NO_LSENVIRONMENT=1 leaves those keys out (and takes them out of the shell's
# environment), to compare without them. The line on stderr says which it did.
#
# Build first, from the repository's root:
#
#   CARGO_TARGET_DIR=$TMPDIR/arrakis-measure/real  cargo build --release
#   CARGO_TARGET_DIR=$TMPDIR/arrakis-measure/drive cargo build --release --features demo

set -eu

# How long a resting scenario of a drive runs before its first sample, in
# seconds (a multiple of 5): what a real run's first 30 s are for. Start-up
# is not the rest.
SETTLE="${SETTLE:-30}"
RESTING="closed-day closed-night open-idle"

M="${TMPDIR:-/tmp}/arrakis-measure"
PLIST="$(dirname "$0")/Info.plist"
S="$M/home"
T="$M/tmp"
C="$S/Library/Application Support/dev.Arrakis.Arrakis"
REAL="$M/real/release/arrakis"
DRIVE="$M/drive/release/arrakis"
STATS="pid,command,cpu,idlew,power,instrs,mem,th"
SCENARIOS="closed-day closed-night closed-fade open-idle open-fade drag open-close start"

usage() {
    sed -n '4,11p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

fail() {
    echo "measure.sh: $*" >&2
    exit 1
}

# refuse_if_running, which scripts/measure/*.sh use too.
. "$(dirname "$0")/measure/common.sh"

# The median of the numbers on stdin, one per line; "-" without any. An odd
# and an even count print the same way: %.15g keeps a whole number whole,
# 73150000 and not 7.31e+07, up to 15 digits.
median() {
    grep -E '^-?[0-9.]+$' | sort -n | awk '
        { v[NR] = $1 }
        END {
            if (NR == 0) { print "-"; exit }
            if (NR % 2) m = v[(NR + 1) / 2] + 0
            else m = (v[NR / 2] + v[NR / 2 + 1]) / 2
            printf "%.15g\n", m
        }'
}

# $1 as a positive whole number, or the default $2 when $1 is empty. 00 is
# zero, as 0 is; 010 is ten. $3, when given, names $1 in the refusal.
count() {
    case "${1:-}" in
        '') echo "$2" ;;
        *[!0-9]*) fail "not a positive whole number: ${3:+$3=}$1" ;;
        *[!0]*)
            n=$1
            while [ "${n#0}" != "$n" ]; do n=${n#0}; done
            echo "$n"
            ;;
        *) fail "not a positive whole number: ${3:+$3=}$1" ;;
    esac
}

# The CPU time `ps` reports for a process, [[hh:]mm:]ss.ss, in seconds.
cpu_seconds() {
    ps -o time= -p "$1" | awk '{
        n = split($1, part, ":"); s = 0
        for (i = 1; i <= n; i++) s = s * 60 + part[i]
        print s
    }'
}

# Writes the settings of real scenario $1 into the measuring home.
write_settings() {
    case "$1" in
        s1 | s4) start=$(date -v+6H +%H:%M) end=$(date -v+8H +%H:%M) ;;
        s2) start=$(date -v-2H +%H:%M) end=$(date -v+6H +%H:%M) ;;
        s3) start=$(date +%H:%M) end=$(date -v+8H +%H:%M) ;;
    esac
    mkdir -p "$C" "$T"
    printf '{"schedule_on":true,"start":"%s","end":"%s","fade_minutes":20,"kelvin":6490,"brightness":100}\n' \
        "$start" "$end" >"$C/settings.json"
}

# Reads a `top -l N -s 5 -pid P -stats $STATS` file: idle wakeups/s from
# the first sample to the last, the mean POWER and %CPU after the first
# sample, the last MEM in KB and the last thread count.
read_top() {
    awk -v pid="$2" -v interval=5 '
        function kb(text,   number, unit) {
            number = text + 0
            unit = text
            gsub(/[0-9.+-]/, "", unit)
            if (unit == "B") return number / 1024
            if (unit == "M") return number * 1024
            if (unit == "G") return number * 1048576
            return number
        }
        $1 == pid {
            n++
            if (n == 1) first = $4
            last = $4
            if (n > 1) { power += $5; cpu += $3 }
            mem = $7
            threads = $8
        }
        END {
            if (n < 2) { print "idlew_per_s=- power=- cpu_percent=- mem_kb=- threads=-"; exit }
            printf "idlew_per_s=%.3f power=%.3f cpu_percent=%.3f mem_kb=%d threads=%s\n",
                (last - first) / ((n - 1) * interval), power / (n - 1), cpu / (n - 1), kb(mem), threads
        }' "$1"
}

# The footprint in KB from a `footprint -p` file.
read_footprint() {
    awk '/Footprint:/ {
        for (i = 1; i < NF; i++) if ($i == "Footprint:") {
            number = $(i + 1); unit = $(i + 2)
            if (unit == "MB") number *= 1024
            if (unit == "GB") number *= 1048576
            if (unit == "B") number /= 1024
            printf "%d\n", number; exit
        }
    }' "$1"
}

# The value of key $2 in a line of key=value pairs, $1.
value() {
    echo "$1" | tr ' ' '\n' | sed -n "s/^$2=//p" | head -n 1
}

# The LSEnvironment dictionary of the plist $1 as NAME=VALUE lines, one for
# each key. The template is XML laid out by hand, a <key> and its <string> to
# a line; anything else inside the dictionary is an error, as LaunchServices
# takes strings only. No dictionary prints nothing.
lsenvironment() {
    awk '
        /<key>LSEnvironment<\/key>/ { state = 1; next }
        state == 1 && /<dict\/>/ { exit }
        state == 1 && /<dict>/ { state = 2; next }
        state == 2 && /<\/dict>/ { exit }
        state == 2 && /<key>.*<\/key>/ {
            if (wanted) bad = 1
            name = $0
            sub(/.*<key>/, "", name)
            sub(/<\/key>.*/, "", name)
            wanted = 1
            next
        }
        state == 2 && wanted && /<string>.*<\/string>/ {
            text = $0
            sub(/.*<string>/, "", text)
            sub(/<\/string>.*/, "", text)
            print name "=" text
            wanted = 0
            next
        }
        state == 2 && /[^ \t]/ { bad = 1 }
        END { if (bad || wanted) exit 1 }
    ' "$1"
}

# Gives the app the environment that LaunchServices gives the bundle's: the
# keys of LSEnvironment in $PLIST, exported to everything this script starts.
# With NO_LSENVIRONMENT they are taken out instead, even when the shell had
# them, so that the other arm of a comparison runs without them.
bundle_environment() {
    [ -r "$PLIST" ] || fail "no $PLIST to read the bundle's environment from"
    pairs=$(lsenvironment "$PLIST") ||
        fail "cannot read the LSEnvironment of $PLIST: only <key> and <string> lines are understood"
    if [ -z "$pairs" ]; then
        echo "measure.sh: no LSEnvironment in $PLIST: the app runs with the environment it is given" >&2
        return
    fi
    shown=
    while IFS='=' read -r var val; do
        case "$var" in
            '' | [!A-Za-z_]* | *[!A-Za-z0-9_]*) fail "not a name for an environment variable: '$var' (LSEnvironment of $PLIST)" ;;
        esac
        if [ -n "${NO_LSENVIRONMENT:-}" ]; then
            unset "$var"
        else
            export "$var=$val"
        fi
        shown="$shown $var=$val"
    done <<EOF
$pairs
EOF
    if [ -n "${NO_LSENVIRONMENT:-}" ]; then
        echo "measure.sh: NO_LSENVIRONMENT is set: the app runs without$shown, as the bare binary does" >&2
    else
        echo "measure.sh: the app runs with the bundle's LSEnvironment from $PLIST:$shown (NO_LSENVIRONMENT=1 leaves it out)" >&2
    fi
}

real_run() {
    scenario=$1 run=$2
    refuse_if_running
    write_settings "$scenario"
    if [ "$scenario" = s4 ]; then
        HOME="$S" TMPDIR="$T" "$REAL" >/dev/null 2>&1 &
    else
        HOME="$S" TMPDIR="$T" "$REAL" --hidden >/dev/null 2>&1 &
    fi
    pid=$!
    top -l 7 -s 5 -pid "$pid" -stats "$STATS" >/dev/null # 30 s to settle
    cpu_start=$(cpu_seconds "$pid")
    started=$(date +%s)
    top -l 37 -s 5 -pid "$pid" -stats "$STATS" >"$M/$scenario-top-$run.txt" # 3 min
    cpu_end=$(cpu_seconds "$pid")
    ended=$(date +%s)
    footprint -p "$pid" >"$M/$scenario-footprint-$run.txt" 2>&1 || true
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" || true
    cpu=$(awk -v a="$cpu_start" -v b="$cpu_end" -v s=$((ended - started)) \
        'BEGIN { printf "%.4f", (b - a) * 60 / (s > 0 ? s : 1) }')
    line="cpu_s_per_min=$cpu $(read_top "$M/$scenario-top-$run.txt" "$pid")"
    line="$line footprint_kb=$(read_footprint "$M/$scenario-footprint-$run.txt")"
    echo "$line" >>"$M/$scenario-runs.txt"
    echo "run $run: $line"
}

drive_run() {
    scenario=$1 run=$2 seconds=$3
    report="$M/$scenario-$run.json"
    if [ "$scenario" = start ]; then
        /usr/bin/time -l "$DRIVE" --demo-drive start --demo-report "$report" \
            >"$M/start-$run.out" 2>"$M/start-time-$run.txt"
        line="$(cat "$M/start-$run.out") $(awk '
            / real / { printf "wall_s=%s ", $1 }
            /instructions retired/ { printf "instructions=%s ", $1 }
            /peak memory footprint/ { printf "peak_footprint=%s", $1 }' "$M/start-time-$run.txt")"
    else
        # A resting scenario settles first, unsampled, as a real run does.
        settle=0
        case " $RESTING " in *" $scenario "*) settle=$SETTLE ;; esac
        "$DRIVE" --demo-drive "$scenario" --demo-seconds $((settle + seconds)) \
            --demo-report "$report" >"$M/$scenario-$run.out" 2>"$M/$scenario-$run.err" &
        pid=$!
        if [ "$settle" -gt 0 ]; then
            top -l $((settle / 5 + 1)) -s 5 -pid "$pid" -stats "$STATS" >/dev/null
        fi
        samples=$((seconds / 5 + 1))
        top -l "$samples" -s 5 -pid "$pid" -stats "$STATS" >"$M/$scenario-top-$run.txt"
        wait "$pid" || echo "run $run: the driver did not finish by itself" >&2
        line="$(cat "$M/$scenario-$run.out") $(read_top "$M/$scenario-top-$run.txt" "$pid")"
    fi
    echo "$line" >>"$M/$scenario-runs.txt"
    echo "run $run: $line"
}

# One line with the median of each numeric key over the runs in $1.
medians() {
    first=$(head -n 1 "$1")
    out="median:"
    for pair in $first; do
        key=${pair%%=*}
        case "$key" in scenario | finished | spoiled) continue ;; esac
        m=$(while read -r runline; do value "$runline" "$key"; done <"$1" | median)
        out="$out $key=$m"
    done
    echo "$out"
}

[ $# -ge 2 ] || usage
mode=$1 scenario=$2
mkdir -p "$M"
case "$mode" in
    real)
        case "$scenario" in s1 | s2 | s3 | s4) ;; *) usage ;; esac
        runs=$(count "${3:-}" 3)
        refuse_if_running
        [ -x "$REAL" ] || fail "no $REAL; build it first: CARGO_TARGET_DIR=$M/real cargo build --release"
        ;;
    drive)
        case " $SCENARIOS " in *" $scenario "*) ;; *) usage ;; esac
        runs=$(count "${3:-}" 3)
        seconds=$(count "${4:-}" 30)
        SETTLE=$(count "$SETTLE" 30 SETTLE)
        [ -x "$DRIVE" ] || fail "no $DRIVE; build it first: CARGO_TARGET_DIR=$M/drive cargo build --release --features demo"
        ;;
    *) usage ;;
esac
bundle_environment

: >"$M/$scenario-runs.txt"
run=1
while [ "$run" -le "$runs" ]; do
    if [ "$mode" = real ]; then
        real_run "$scenario" "$run"
    else
        drive_run "$scenario" "$run" "$seconds"
    fi
    run=$((run + 1))
done

# The runs whose window was covered or minimized (the driver's `unseen=` above 0) are left out of the median:
# eframe paints none of those frames, so their per-frame figures read too good. So are the runs that were out
# of their time or broke off a cycle (the driver's `spoiled=` is not `-`): the machine slowed or stopped them, or
# they hold fewer windows than asked, so their per-frame figures do not compare. A line with no `spoiled=`, from
# a driver built before it, is in its time. Their file
# keeps every run; the median reads the runs that held, from a file of their own.
held="$M/$scenario-held.txt"
: >"$held"
left_out=$(awk -v held="$held" '{
    covered = ""
    late = ""
    for (i = 1; i <= NF; i++) {
        if ($i ~ /^unseen=[0-9]+$/ && substr($i, 8) + 0 > 0) covered = $i
        if ($i ~ /^spoiled=/ && $i != "spoiled=-") late = $i
    }
    if (covered != "")
        printf "SPOILED run %d (%s: the window was covered or minimized, so eframe painted none of those frames " \
            "and its per-frame figures read too good), left out of the median\n", NR, covered
    if (late != "")
        printf "SPOILED run %d (%s: either the drive ran out of its time, so the machine slowed or stopped it, or a " \
            "cycle broke off, so it holds fewer windows than asked; its per-frame figures do not compare), left out " \
            "of the median\n", NR, late
    if (covered == "" && late == "")
        print > held
}' "$M/$scenario-runs.txt")
[ -z "$left_out" ] || echo "$left_out"
if [ -s "$held" ]; then
    medians "$held"
else
    echo "every run was spoiled, so there is no median"
fi
