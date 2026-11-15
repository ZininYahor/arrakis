#!/bin/sh
# Are the Metal painter's pixels glow's? Glow is linked as the live reference, so that no full-window
# golden is stored.
#
# Four scenes through both painters (--demo-renderer), which give seven pictures with the three tinted
# twins, compared by --demo-diff --tolerance 0. Then the real window of a night shot through each, as the
# window server composes it: the one picture here that goes through the layer tree, so it covers the
# upright image, the corners, the traffic lights and the color path, P3 tag included. Exact holds on one
# machine only.
#
# A shot leaves the real displays and the settings alone, and each window opens on the screen and takes
# the focus for about 2.5 s: 10 windows, about 40 s. The window's captures need the terminal to be allowed
# to record the screen. Without that the scenes are still compared, and it exits 2.
#
# Each shot says which painter drew it, from what eframe gave the app, and a shot on the wrong painter stops
# the check: comparing a painter with itself would prove nothing.
#
# usage: painter-check.sh [folder]
#   folder  takes the pictures; a new one under $TMPDIR by default. Under $TMPDIR/arrakis-painter-check
#           it also takes winid's build, the shots' HOME and their TMPDIR. An earlier run's files in it go
#           first, and only those: the names this script writes, in its glow and metal subfolders.
# PAINTER_CHECK_BIN names a demo build to use; by default it runs `cargo build --features demo`.
# Exits 0 when all match, 1 at a difference, 2 when the window could not be captured.
set -u

fail() {
    echo "painter-check.sh: $*" >&2
    exit 1
}

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK="${TMPDIR:-/tmp}/arrakis-painter-check"
OUT="${1:-$WORK/$(date +%Y%m%d-%H%M%S)}"
SCENES="night day focused desert"
PICTURES="night night-tinted day focused focused-tinted desert desert-tinted"

BIN="${PAINTER_CHECK_BIN:-}"
if [ -z "$BIN" ]; then
    (cd "$ROOT" && cargo build --features demo) || fail "the demo build failed"
    # Where cargo says, as in scripts/bundle.sh: a config file can move it too.
    target=$(cd "$ROOT" && cargo metadata --format-version 1 --no-deps |
        plutil -extract target_directory raw -o - -) ||
        fail "cargo metadata did not give the target directory"
    BIN="$target/debug/arrakis"
fi
[ -x "$BIN" ] || fail "no $BIN"
mkdir -p "$OUT" "$WORK/home" "$WORK/tmp" || fail "cannot write to $OUT or $WORK"

# What an earlier run into this folder left: its captures would vote in `settled`, and a tinted twin that no
# shot wrote this time would be compared. The folder is the caller's, so only the names this script writes.
for painter in glow metal; do
    for scene in $SCENES; do
        rm -f "$OUT/$painter/$scene.png" "$OUT/$painter/$scene-tinted.png" "$OUT/$painter/$scene.log"
    done
    rm -f "$OUT/$painter/window"/[0-9][0-9][0-9][0-9].png "$OUT/$painter/window/shot.png" \
        "$OUT/$painter/window/shot-tinted.png" "$OUT/$painter/window/app.log" "$OUT/$painter/window/capture.log"
done

# The app, with a home and a temporary folder of its own.
app() {
    HOME="$WORK/home" TMPDIR="$WORK/tmp" "$BIN" "$@"
}

# Stops the check unless the log $1 of the shot named $2 says it was drawn by the painter $3, and by no
# other. A window says its painter once: a Metal window that failed opens again on glow, which says so too,
# and the picture is then glow's.
check_painter() {
    [ "$(grep '^painter: ' "$1")" = "painter: $3" ] ||
        fail "the $2 shot asked for $3 and another painter drew it: see $1"
}

for painter in glow metal; do
    mkdir -p "$OUT/$painter"
    for scene in $SCENES; do
        app --demo-shot "$OUT/$painter/$scene.png" --demo-scene "$scene" --demo-renderer "$painter" \
            >"$OUT/$painter/$scene.log" 2>&1 || fail "the $scene shot on $painter failed: see $OUT/$painter/$scene.log"
        check_painter "$OUT/$painter/$scene.log" "$scene" "$painter"
    done
done

differences=0
for picture in $PICTURES; do
    if app --demo-diff "$OUT/glow/$picture.png" "$OUT/metal/$picture.png" --tolerance 0 \
        >"$OUT/$picture.diff" 2>&1; then
        echo "same: $picture"
    else
        echo "DIFFERENT: $picture: $(cat "$OUT/$picture.diff")"
        differences=$((differences + 1))
    fi
done

# Captures one painter's night window for as long as it lives, into $OUT/$1/window.
capture_window() {
    folder="$OUT/$1/window"
    mkdir -p "$folder"
    # Not through `app`: a function in the background is a shell of its own, and $! would be that shell.
    HOME="$WORK/home" TMPDIR="$WORK/tmp" "$BIN" --demo-shot "$folder/shot.png" --demo-scene night \
        --demo-renderer "$1" >"$folder/app.log" 2>&1 &
    pid=$!
    n=0
    while kill -0 "$pid" 2>/dev/null; do
        id=$("$WINID" "$pid" 2>/dev/null) || {
            sleep 0.05
            continue
        }
        n=$((n + 1))
        /usr/sbin/screencapture -x -o -l"$id" "$folder/$(printf %04d "$n").png" 2>>"$folder/capture.log"
    done
    wait "$pid" || fail "the night shot on $1 failed: see $folder/app.log"
    check_painter "$folder/app.log" "night window" "$1"
}

# The most common capture of $1, which is the settled window: the first captures are the window's fade-in,
# and the last can be the bare window at its exit. Prints "<count> <file>", or nothing with no capture.
# shasum prints the hash, two characters, and the path, which may hold a space.
settled() {
    for capture in "$1"/[0-9][0-9][0-9][0-9].png; do
        [ -f "$capture" ] && shasum "$capture"
    done | awk '
        { count[$1]++; file[$1] = substr($0, length($1) + 3) }
        END { for (hash in count) if (count[hash] > best) { best = count[hash]; pick = file[hash] }
              if (best) print best, pick }'
}

WINID="$WORK/winid"
clang -O1 -framework CoreGraphics -framework CoreFoundation "$ROOT/scripts/measure/winid.c" -o "$WINID" ||
    fail "cannot build winid"
capture_window glow
capture_window metal
glow_window=$(settled "$OUT/glow/window")
metal_window=$(settled "$OUT/metal/window")
if [ -z "$glow_window" ] || [ -z "$metal_window" ]; then
    echo "UNREAD: real window: no capture. Allow the terminal to record the screen, and see $OUT/*/window/capture.log" >&2
    [ "$differences" -eq 0 ] && exit 2
    exit 1
fi
# A settled window holds still across captures. One capture alone could be the fade-in.
if [ "${glow_window%% *}" -lt 2 ] || [ "${metal_window%% *}" -lt 2 ]; then
    echo "DIFFERENT: real window: no settled capture (glow: $glow_window; metal: $metal_window)"
    differences=$((differences + 1))
elif cmp -s "${glow_window#* }" "${metal_window#* }"; then
    echo "same: real window, byte for byte (held for ${glow_window%% *} and ${metal_window%% *} captures)"
else
    echo "DIFFERENT: real window: ${glow_window#* } and ${metal_window#* }"
    differences=$((differences + 1))
fi

echo "$differences differ; the pictures are in $OUT"
[ "$differences" -eq 0 ]
