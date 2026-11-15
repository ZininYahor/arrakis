#!/bin/sh
# Rebuilds crates/egui-wgpu-metal/src/shader.metallib from shader.metal beside it, and writes its stamp,
# shader_stamp.rs: the source's SHA-256, the file's, its length, the compiler's version line and the
# deployment target. The one way either file changes. Run it after any change to shader.metal, then
# `cargo test --features demo`, whose tests compare the hashes and the frames:
#
#   scripts/build-metallib.sh
#
# It needs Xcode and Xcode's Metal Toolchain, which Xcode 26 and later download apart from Xcode, and it
# refuses by name without either. Only whoever changes the shader needs them: building and testing the app
# need only the Command Line Tools. It keeps a DEVELOPER_DIR the caller set, and sets Xcode's default place,
# /Applications/Xcode.app/Contents/Developer, otherwise: so it works whatever `xcode-select -p` says, and
# changes no selection.
#
# The compile is one step: `metal -target air64-apple-macos<target> -o shader.metallib shader.metal`
# compiles and links, with no `metallib` step. Read on 2026-11-07 with Xcode 27.2 (27B52), Metal Toolchain
# 27B47, `Apple metal version 32023.922 (metalfe-32023.922.4)`: `metal --help` lists `--target=<value>`,
# `-target` is clang's spelling of it, and `metal-objdump --private-headers` reads `Platform: MACOS`,
# `PlatformMajor: 14` in the file. The same source gives the same bytes from any folder, and another
# target gives other bytes. The target is MACOSX_DEPLOYMENT_TARGET from .cargo/config.toml, the app's
# minimum, which a test compares with the stamp.
#
# It writes only the two files, through a folder under $TMPDIR, and launches nothing.
set -eu

fail() {
    echo "build-metallib.sh: $*" >&2
    exit 1
}

[ "$(uname -s)" = Darwin ] || fail "macOS only"
ROOT=$(cd "$(dirname "$0")/.." && pwd)
SRC="$ROOT/crates/egui-wgpu-metal/src"
[ -f "$SRC/shader.metal" ] || fail "no $SRC/shader.metal"

DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}"
export DEVELOPER_DIR
[ -d "$DEVELOPER_DIR" ] ||
    fail "no Xcode in $DEVELOPER_DIR. Install Xcode from the App Store, accept its license and open it once, \
then download its Metal Toolchain: xcodebuild -downloadComponent MetalToolchain. For an Xcode elsewhere, set \
DEVELOPER_DIR to its Contents/Developer. Only a change of the shader needs Xcode."

WORK=$(mktemp -d "${TMPDIR:-/tmp}/build-metallib.XXXXXX") || fail "cannot make a folder under \$TMPDIR"
trap 'rm -rf "$WORK"' EXIT

xcrun -sdk macosx metal --version >"$WORK/version" 2>"$WORK/version.err" ||
    fail "Xcode's Metal compiler does not run in DEVELOPER_DIR=$DEVELOPER_DIR: $(cat "$WORK/version.err") \
If that is not an Xcode, unset DEVELOPER_DIR or set it to an Xcode's Contents/Developer. Xcode 26 and later \
download the compiler apart from Xcode, as the Metal Toolchain: xcodebuild -downloadComponent MetalToolchain"
compiler=$(sed -n 1p "$WORK/version")
case "$compiler" in
"" | *'"'* | *\\*) fail "an unexpected version line from metal: $compiler" ;;
esac

# The one form read, here and in `the_file_is_built_for_the_apps_minimum` (painter/tests.rs): a line that
# begins `MACOSX_DEPLOYMENT_TARGET = { value = "`, then digits and dots up to the next `"`, and the only
# such line. Any other spelling is refused, by the test too.
target=$(sed -n 's/^MACOSX_DEPLOYMENT_TARGET = { value = "\([0-9][0-9.]*\)".*/\1/p' "$ROOT/.cargo/config.toml")
case "$target" in
"" | *[!0-9.]*)
    fail "no MACOSX_DEPLOYMENT_TARGET = { value = \"<version>\" ... } line, or more than one, in \
$ROOT/.cargo/config.toml"
    ;;
esac

# From the source's own folder, by its bare name, as every rebuild does.
(cd "$SRC" && xcrun -sdk macosx metal -target "air64-apple-macos$target" -o "$WORK/shader.metallib" shader.metal) ||
    fail "metal did not compile $SRC/shader.metal"

sha256() {
    /usr/bin/shasum -a 256 "$1" | cut -d ' ' -f 1
}
source_sha=$(sha256 "$SRC/shader.metal")
file_sha=$(sha256 "$WORK/shader.metallib")
bytes=$(stat -f %z "$WORK/shader.metallib")

# Each constant that a build does not read says so, so that one more that nothing reads is a warning.
cat >"$WORK/stamp.rs" <<EOF
//! What \`shader.metallib\` was built from. \`scripts/build-metallib.sh\` writes this file with it, and
//! nobody edits it: \`painter.rs\` asserts the length, and its tests check the rest.

/// \`shader.metal\`'s SHA-256.
#[cfg_attr(not(test), expect(dead_code, reason = "only the tests read it"))]
pub const SOURCE: &str = "$source_sha";
/// \`shader.metallib\`'s SHA-256.
#[cfg_attr(not(test), expect(dead_code, reason = "only the tests read it"))]
pub const FILE: &str = "$file_sha";
/// \`shader.metallib\`'s length, in bytes.
pub const BYTES: usize = $bytes;
/// The first line of \`metal --version\`.
#[expect(dead_code, reason = "only a review reads the compiler's line")]
pub const COMPILER: &str = "$compiler";
/// The deployment target, from \`.cargo/config.toml\`: \`-target air64-apple-macos$target\`.
#[cfg_attr(not(test), expect(dead_code, reason = "only the tests read it"))]
pub const TARGET: &str = "$target";
EOF
# rustfmt in the repository, so that its toolchain is the one `cargo fmt --check` runs, and through stdin,
# as rustfmt looks for its config from the folder it runs in then and not from the file's, under $TMPDIR.
(cd "$ROOT" && rustfmt --edition 2024 --emit stdout <"$WORK/stamp.rs" >"$WORK/shader_stamp.rs") &&
    [ -s "$WORK/shader_stamp.rs" ] || fail "rustfmt did not take the stamp"

mv "$WORK/shader.metallib" "$SRC/shader.metallib"
mv "$WORK/shader_stamp.rs" "$SRC/shader_stamp.rs"
echo "wrote $SRC/shader.metallib ($bytes bytes) and $SRC/shader_stamp.rs"
echo "compiler: $compiler; target: macOS $target; $(xcodebuild -version | tr '\n' ' ')"
echo "now run: cargo test --features demo"
