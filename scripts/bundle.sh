#!/bin/sh
# Builds Arrakis.app, the bundle that gives the app its icon:
#
#   scripts/bundle.sh
#
# It builds the release binary, makes Arrakis.icns from the master
# assets/icon.png (sips for each size, then iconutil), writes the bundle
# (Contents/MacOS/arrakis, Contents/Resources/Arrakis.icns and an Info.plist
# filled in from scripts/Info.plist) and signs it ad hoc. Everything goes
# under target/bundle/ (git ignores target/); nothing is written into the
# repository, and nothing is launched. `target` is the folder cargo builds
# into, as `cargo metadata` names it: it follows cargo's config as well as
# CARGO_TARGET_DIR. On a Mac older than macOS 14, the app's minimum, it stops
# with a message before it builds or writes anything.
#
# Environment (a relative path is taken from where the script is run):
#   ARRAKIS_BIN  bundle this binary and do not build the release one.
#   BUNDLE_DIR   write to this directory instead of target/bundle.
#
# The master is drawn by the app's own code: `cargo run --features demo --
# --demo-icon assets/icon.png` writes it again.

set -eu

fail() {
    echo "bundle.sh: $*" >&2
    exit 1
}

[ "$(uname -s)" = Darwin ] || fail "macOS only"

# The app needs macOS 14 (.cargo/config.toml sets the same number for the
# linker). A bundle made on an older Mac would not open there, and the
# README's update steps would put it over a working app, so this stops first:
# before anything is built or written.
min_macos=14
macos=$(sw_vers -productVersion) || fail "sw_vers did not say which macOS this is"
case "${macos%%.*}" in
'' | *[!0-9]*) fail "no usable macOS version from sw_vers: '$macos'" ;;
esac
[ "${macos%%.*}" -ge "$min_macos" ] ||
    fail "this Mac runs macOS $macos, and Arrakis needs macOS $min_macos or later: nothing was built"

# A relative ARRAKIS_BIN or BUNDLE_DIR means the folder the script is run
# from: make them absolute before moving to the repository.
absolute() {
    case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$PWD" "$1" ;;
    esac
}
[ -z "${ARRAKIS_BIN:-}" ] || ARRAKIS_BIN=$(absolute "$ARRAKIS_BIN")
[ -z "${BUNDLE_DIR:-}" ] || BUNDLE_DIR=$(absolute "$BUNDLE_DIR")

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# The folder cargo builds into, as cargo says: not only CARGO_TARGET_DIR but
# the config's build.target-dir too. A guess would bundle a stale binary.
target_dir() {
    cargo metadata --format-version 1 --no-deps |
        plutil -extract target_directory raw -o - - ||
        fail "cargo metadata did not give the target directory"
}
# Only asked when it is needed: a binary and a folder given need no cargo.
target=
if [ -z "${ARRAKIS_BIN:-}" ] || [ -z "${BUNDLE_DIR:-}" ]; then
    target=$(target_dir)
fi
out="${BUNDLE_DIR:-$target/bundle}"
master=assets/icon.png
app="$out/Arrakis.app"
iconset="$out/Arrakis.iconset"
icns="$out/Arrakis.icns"

[ -f "$master" ] || fail "$master is missing"
[ "$(sips -g pixelWidth -g pixelHeight "$master" | awk '/pixel(Width|Height)/ { print $2 }' | tr '\n' ' ')" = "1024 1024 " ] ||
    fail "$master is not 1024 by 1024 pixels"

# The version of the package, from Cargo.toml: the first `version` under
# [package]. The bundle's two version keys are integers separated by periods,
# so a pre-release or build suffix (0.2.0-beta.1, 1.0.0+build.5) is cut from
# the first `-` or `+`.
version=$(awk -F'"' '/^\[/ { in_package = ($0 == "[package]") } in_package && /^version[ ]*=/ { print $2; exit }' Cargo.toml)
version=${version%%[-+]*}
case "$version" in
'' | *[!0-9.]*) fail "no usable version in Cargo.toml: '$version'" ;;
esac

if [ -n "${ARRAKIS_BIN:-}" ]; then
    bin=$ARRAKIS_BIN
else
    cargo build --release
    bin="$target/release/arrakis"
fi
[ -x "$bin" ] || fail "$bin is not an executable"

# The oldest macOS the binary was linked to run on.
min_os=$(otool -l "$bin" | awk '
    $1 == "cmd" { cmd = $2 }
    cmd == "LC_BUILD_VERSION" && $1 == "minos" { print $2; exit }
    cmd == "LC_VERSION_MIN_MACOSX" && $1 == "version" { print $2; exit }')
[ -n "$min_os" ] || fail "no minimum macOS version in $bin"

rm -rf "$app" "$iconset" "$icns"
mkdir -p "$iconset" "$app/Contents/MacOS" "$app/Contents/Resources"

# Each size at 1x and 2x, as iconutil names them. The largest, 512 at 2x, is
# the master itself, not a resampling of it: it is copied below.
for size in 16 32 128 256 512; do
    sips -s format png -z "$size" "$size" "$master" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    if [ "$size" -ne 512 ]; then
        double=$((size * 2))
        sips -s format png -z "$double" "$double" "$master" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
    fi
done
cp "$master" "$iconset/icon_512x512@2x.png"
iconutil -c icns "$iconset" -o "$icns"

cp "$bin" "$app/Contents/MacOS/arrakis"
cp "$icns" "$app/Contents/Resources/Arrakis.icns"
sed -e "s/@VERSION@/$version/g" -e "s/@MIN_OS@/$min_os/g" scripts/Info.plist >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null

# Ad hoc: no developer identity. The signature seals the Info.plist and the
# icon with the binary, and `codesign --verify` checks them.
codesign --force --sign - "$app"
codesign --verify --strict --verbose=2 "$app"

echo "$app"
