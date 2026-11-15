#!/bin/sh
# Two questions about the real app:
#   1. Does MallocSpaceEfficient=1 reach the app when it is set by an LSEnvironment key in Info.plist and
#      the app is started by `open`? It builds two bundles, with and without the key, signs them as
#      scripts/bundle.sh does, and reads each process's environment and footprints (never opened, open
#      30 s, 60 s after a close).
#   2. What does the AutoFill helper cost, and does NSAutoFillHeuristicsEnabled NO keep it from starting?
#      Two runs of the bare binary with the key and two without; it prints the helper's pids and sizes.
# The schedule starts six hours off, so nothing tints.
#
# THE REAL APP, ON THE REAL DISPLAYS: run this only when you mean to. It writes the `arrakis`
# defaults domain and deletes it again, and stops at once if that domain already exists. It needs the
# terminal to be allowed to control the computer (Accessibility), for System Events. About 7 minutes.
#
# usage: delivery.sh <root holding arrakis-measure/real/release/arrakis> <repo> [bundle]
#   root   as for real-cycles.sh
#   repo   the checkout whose scripts/Info.plist the bundles use
#   bundle only the two bundle runs, not the AutoFill ones
# It finds axtool in $TMPDIR (scripts/measure/axtool.c says how to build it).
# It stops, after killing what it started, when a bundle's process is not found; and it leaves no
# ~/Library/Preferences/arrakis.plist behind (`defaults delete` keeps an empty one).
set -u
fail() {
    echo "delivery.sh: $*" >&2
    exit 1
}
. "$(dirname "$0")/common.sh"
AX="${TMPDIR%/}/axtool"
[ -x "$AX" ] || { echo "delivery.sh: build axtool into $AX first" >&2; exit 2; }
# Everything below runs under its own TMPDIR, which hides a copy the owner has running from it.
refuse_if_running
# The real path: a process's own holds no /var or //, which TMPDIR's does.
root=$(cd "$1" && pwd -P) repo=$2
M="$root/arrakis-measure"
S="$M/home"
T="$M/tmp"
C="$S/Library/Application Support/dev.Arrakis.Arrakis"
BIN="$M/real/release/arrakis"
# The oldest macOS the binary was linked to run on, read as scripts/bundle.sh does and not copied: an arm
# built before .cargo/config.toml says 11.0, and its bundle's plist must say what its binary says.
min_os=$(otool -l "$BIN" | awk '
    $1 == "cmd" { cmd = $2 }
    cmd == "LC_BUILD_VERSION" && $1 == "minos" { print $2; exit }
    cmd == "LC_VERSION_MIN_MACOSX" && $1 == "version" { print $2; exit }')
[ -n "$min_os" ] || fail "no minimum macOS version in $BIN"
LS=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
B="$T/delivery"
mkdir -p "$C" "$B"
printf '{"schedule_on":true,"start":"%s","end":"%s","fade_minutes":20,"kelvin":6490,"brightness":100}\n' \
    "$(date -v+6H +%H:%M)" "$(date -v+8H +%H:%M)" >"$C/settings.json"

make_bundle() { # dir, with the key or not
    app="$B/$1/Arrakis.app"
    rm -rf "$B/$1"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$BIN" "$app/Contents/MacOS/arrakis"
    plist="$app/Contents/Info.plist"
    sed -e 's/@VERSION@/0.0.0/g' -e "s/@MIN_OS@/$min_os/g" "$repo/scripts/Info.plist" >"$plist"
    # A step that fails stops the script (it has no `set -e`: `fail` exits this function's subshell, and the
    # caller's `|| exit 1` ends the script). The lint comes first, so that a template that does not parse
    # fails as a bad plist and not as a missing key; an `echo` there would land in `$app`, this function's
    # output. The template holds the flag (LSEnvironment): the `key` bundle keeps it and the
    # `none` one leaves it out, as a pair of bundles that both carry the flag would answer question 1 with
    # the flag against the flag. A signature that fails stops it too, as a bundle not signed the way
    # scripts/bundle.sh signs it would answer another question. plutil's and codesign's messages stay
    # on stderr: plutil writes a lint failure there, and only its "OK" goes to the stdout that is dropped.
    plutil -lint "$plist" >/dev/null || fail "$plist is not a valid plist"
    if [ "$2" = none ]; then
        plutil -remove LSEnvironment "$plist" || fail "no LSEnvironment in $repo/scripts/Info.plist to leave out"
    else
        [ "$(plutil -extract LSEnvironment.MallocSpaceEfficient raw -o - "$plist")" = 1 ] ||
            fail "$repo/scripts/Info.plist does not set LSEnvironment.MallocSpaceEfficient to 1"
    fi
    codesign --force --sign - "$app" || fail "codesign could not sign $app"
    echo "$app"
}

# No `^`: the process's path starts with /private, and TMPDIR's does not. `pgrep -f` also takes the
# path as a regular expression, so a path with a `+` or a `(` may find nothing.
bundle_pid() { pgrep -f "$1/Contents/MacOS/arrakis" | head -n 1; }

# Stops the script when $1, a process id, is empty: the process was not found, so the figures that
# follow would be those of nothing. Kills every arrakis there is (refuse_if_running saw none at the
# start, so they are this script's) and says which.
need_pid() { # pid, what it was meant to be
    [ -n "$1" ] && return 0
    running=$(pgrep -il arrakis | tr '\n' ' ')
    echo "delivery.sh: no process found for $2; killing what is running: ${running:-nothing}" >&2
    pkill -TERM -i arrakis
    sleep 2
    left=$(pgrep -il arrakis | tr '\n' ' ')
    echo "delivery.sh: left after the kill: ${left:-nothing}" >&2
    exit 1
}
osa() { perl -e 'alarm 15; exec @ARGV' osascript -e "tell application \"System Events\" to tell (first process whose unix id is $1) to $2" 2>&1; }

bundle_run() { # name, key or none
    # `fail` inside the substitution only ends its subshell: the status stops this one.
    app=$(make_bundle "$1" "$2") || exit 1
    open -n --env HOME="$S" --env TMPDIR="$T" "$app" --args --hidden
    sleep 30
    pid=$(bundle_pid "$app")
    need_pid "$pid" "the hidden $1 bundle"
    malloc=$(ps eww "$pid" | tr ' ' '\n' | grep -c '^MallocSpaceEfficient=1$')
    home_ok=$(ps eww "$pid" | tr ' ' '\n' | grep -c "^HOME=$S\$")
    never=$("$AX" fp "$pid")
    kill -TERM "$pid"
    sleep 2
    open -n --env HOME="$S" --env TMPDIR="$T" "$app"
    sleep 30
    pid=$(bundle_pid "$app")
    need_pid "$pid" "the $1 bundle with a window"
    open30=$("$AX" fp "$pid")
    osa "$pid" 'click (first button of window 1 whose subrole is "AXCloseButton")' >/dev/null
    sleep 60
    close60=$("$AX" fp "$pid")
    left=$(osa "$pid" 'get count of windows')
    kill -TERM "$pid"
    sleep 2
    "$LS" -u "$app"
    echo "bundle $1 env_has_flag=$malloc home_ok=$home_ok never=$never open30=$open30 close60=$close60 windows_left=$left"
}

# The helper is `AutoFill (arrakis)` in Activity Monitor, com.apple.SafariPlatformSupport.Helper by path.
helper() { pgrep -f SafariPlatformSupport.Helper | tr '\n' ' '; }

# The bare binary's defaults domain is its name, which nothing else uses. The app takes no such argument.
autofill_run() { # control or off
    defaults delete arrakis >/dev/null 2>&1
    if [ "$1" = off ]; then
        defaults write arrakis NSAutoFillHeuristicsEnabled -bool NO
    fi
    before=$(helper)
    env HOME="$S" TMPDIR="$T" "$BIN" >/dev/null 2>&1 &
    pid=$!
    sleep 3
    early=$(helper)
    sleep 27
    app=$("$AX" fp "$pid")
    during=$(helper)
    sizes=""
    for h in $during; do
        sizes="$sizes $h=$("$AX" fp "$h")"
    done
    kill -TERM "$pid"
    wait "$pid" 2>/dev/null
    sleep 3
    defaults delete arrakis >/dev/null 2>&1
    echo "autofill $1 app=$app before=[$before] at3s=[$early] at30s=[$sizes ] after=[$(helper)]"
}

defaults read arrakis >/dev/null 2>&1 && echo "the arrakis domain already exists: stopping" && exit 1
bundle_run none none
bundle_run key key
if [ "${3:-all}" = bundle ]; then
    pgrep -il arrakis | wc -l
    exit 0
fi
autofill_run control
autofill_run off
autofill_run off
autofill_run control
if defaults read arrakis >/dev/null 2>&1; then
    echo "the arrakis domain is left over"
else
    # `defaults delete` leaves the domain's file, empty. The start refused a domain that held anything.
    rm -f "$HOME/Library/Preferences/arrakis.plist"
fi
pgrep -il arrakis | wc -l
