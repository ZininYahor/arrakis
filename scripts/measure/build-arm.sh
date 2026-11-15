#!/bin/sh
# An arm for any commit: its source, a release build and a demo build, in the layout that the scripts of
# this folder and scripts/measure.sh read, so that a block of merges can be bisected (bisect-arms.sh).
#
# usage: build-arm.sh <commit> <root>
#   commit  anything `git rev-parse` resolves to a commit, of the repository this script sits in
#   root    an absolute path, a TMPDIR for the readings (scripts/measure.sh describes the layout)
# It makes, all under <root>, with two exceptions that are not: `git worktree add` records the worktree in
# the repository's own `.git/worktrees/`, and cargo's cache (CARGO_HOME) takes any crate that a locked build
# fetches. A root deleted by hand leaves the worktree's record in the repository, which `git worktree prune`
# clears; `git worktree remove` (the command printed at the end) clears it with the folder.
#   src/                       a detached worktree of the commit (`git worktree add --detach`), built with
#                              `cargo build --locked`
#   toolchain                  what shapes a build beside the sdk, taken in src/ before the builds: the lines of
#                              `rustc -Vv` and `cargo -V` (the compiler that src/'s rust-toolchain.toml picks),
#                              and `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_PROFILE_RELEASE_*`,
#                              `RUSTUP_TOOLCHAIN`, `SDKROOT` and `DEVELOPER_DIR` for each one that is set, as
#                              `rustc: ...`, `cargo: ...` and `env: NAME=value`. bisect-arms.sh warns when the
#                              base's arm and a step's differ in it or in `sdk`.
#   arrakis-measure/real/      `cargo build --release`, its own CARGO_TARGET_DIR; the binary is
#                              arrakis-measure/real/release/arrakis (real-cycles.sh, close-under-tint.sh,
#                              delivery.sh, scripts/measure.sh real)
#   arrakis-measure/drive/     `cargo build --release --features demo`, its own CARGO_TARGET_DIR; the
#                              binary is arrakis-measure/drive/release/arrakis (energy-drag.py,
#                              energy-system.py, close-captures.sh, tear-captures.sh, scripts/measure.sh
#                              drive)
#   sdk                        one line for each binary, `real sdk=27.2 minos=14.0`: the `sdk` and `minos` of
#                              its LC_BUILD_VERSION in `otool -l`. A figure names its sdk, and two
#                              arms to compare are built under one toolchain.
#   commit                     the full SHA, written last: a root with it holds a finished arm. A run removes
#                              `commit`, `sdk` and `toolchain` before it builds, so a run that fails part-way
#                              leaves no mark of a finished arm.
# How each script finds an arm:
#   real-cycles.sh <arm> <root>, close-under-tint.sh <arm> <root>, delivery.sh <root> <repo>,
#   close-captures.sh <label> <root>, tear-captures.sh <label> <root>
#                              the arm's root, as the script's <root>
#   scripts/measure.sh         TMPDIR=<root>
#   energy-drag.py             ENERGY_DRAG_ROOT_<ARM>=<root>, <ARM> the arm's name in capitals with "_" for "-"
#                              (FINAL, RELEASE, or a new one such as BISECT_MID)
#   energy-system.py           ENERGY_SYSTEM_ROOT_<NAME>=<root>, <NAME> one of CAP, METAL and FINAL
# None reads src/, commit, sdk or toolchain.
# Exits 2, before anything runs, for a root that is empty, `/` or relative, a commit that does not
# resolve, a root that holds another commit's arm (its `commit`, its `src`, or builds that this script did
# not make), a `src` with changes (`git status --porcelain` shows any, untracked files too: a build of a
# changed tree is no build of the commit), and a missing cargo, rustc or otool. Exits 1 when a build or the
# reading of a binary fails. Run again with the same commit and root, it reuses `src` and the target folders,
# and builds what changed.
# It runs nothing that it builds. A build takes minutes, and the first of a root compiles every dependency.
# It prints how to remove the worktree once, on stderr and last, which git keeps a record of in the
# repository until it is removed (after a failure too, when the worktree is there).
set -u
# How to remove the worktree: the one line this script prints about it, at its end.
removal_hint() {
    echo "build-arm.sh: to remove the worktree: git -C $REPO worktree remove --force $root/src" >&2
}
fail() {
    echo "build-arm.sh: $*" >&2
    [ -e "${root:-}/src/.git" ] && removal_hint
    exit 1
}
refuse() {
    echo "build-arm.sh: $*" >&2
    exit 2
}
[ $# -eq 2 ] || {
    echo "usage: build-arm.sh <commit> <root>  (see the header)" >&2
    exit 2
}
commit=$1 root=$2

case $root in
    /*) ;;
    '') refuse "the root is empty" ;;
    *) refuse "the root must be an absolute path, and '$root' is not" ;;
esac
while [ "${root%/}" != "$root" ]; do root=${root%/}; done
[ -n "$root" ] || refuse "the root is /"

HERE=$(cd "$(dirname "$0")" && pwd -P)
REPO=$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null) || refuse "$HERE is not in a git repository"
sha=$(git -C "$REPO" rev-parse --verify --quiet "$commit^{commit}") || refuse "'$commit' does not resolve to a commit of $REPO"
for tool in cargo rustc otool; do
    command -v "$tool" >/dev/null 2>&1 || refuse "no $tool on the PATH"
done

# Another commit's arm: the commit it was finished for, the source it was started from, or builds that were
# not made here (arms built by hand) and that a build would overwrite.
if [ -e "$root/commit" ]; then
    held=$(cat "$root/commit" 2>/dev/null)
    [ "$held" = "$sha" ] || refuse "$root holds the arm of ${held:-an unreadable commit}, not of $sha; use another root"
fi
if [ -e "$root/src" ]; then
    held=$(git -C "$root/src" rev-parse --verify --quiet HEAD 2>/dev/null) || held=
    if [ ! -e "$root/src/.git" ] || [ "$held" != "$sha" ]; then
        refuse "$root/src is not a worktree of $sha (it holds ${held:-no worktree}); use another root"
    fi
    # Its HEAD is the commit, but a build of an edited or added file is no build of the commit. `git status`
    # lists untracked files too (the ignored ones are not part of the commit and not listed).
    changed=$(git -C "$root/src" status --porcelain 2>&1) || refuse "cannot read the status of $root/src: $changed"
    if [ -n "$changed" ]; then
        refuse "$root/src has changes against $sha, so it is no build of that commit (git status --porcelain: $(echo "$changed" | head -n 3 | tr '\n' ' ')); remove it or use another root"
    fi
elif [ -e "$root/arrakis-measure" ] && [ ! -e "$root/commit" ]; then
    refuse "$root holds builds that this script did not make; use another root"
fi

# The binaries sit under $root/arrakis-measure/<real|drive>/release/arrakis, as every script of this folder
# reads them. A cargo told to build for another target would put them elsewhere: that is no arm.
unset CARGO_BUILD_TARGET CARGO_BUILD_TARGET_DIR
mkdir -p "$root" || fail "cannot write to $root"
if [ ! -e "$root/src/.git" ]; then
    git -C "$REPO" worktree add --detach "$root/src" "$sha" >&2 || fail "git worktree add failed for $root/src"
fi
# From here on the root holds no finished arm until `commit`, written last, says so. A rerun that fails part-way
# must not keep the mark, or the sdk and the toolchain, of the arm it started from.
rm -f "$root/commit" "$root/sdk" "$root/toolchain" || fail "cannot clear the marks of a finished arm in $root"

# What shapes a build beside the sdk, as it is in src/ (a rust-toolchain.toml there picks the compiler), written
# before the builds so that a toolchain that cannot run ends the script early.
rustc_v=$(cd "$root/src" && rustc -Vv) || fail "rustc -Vv failed in $root/src"
cargo_v=$(cd "$root/src" && cargo -V) || fail "cargo -V failed in $root/src"
{
    printf '%s\n' "$rustc_v" | sed 's/^/rustc: /'
    printf '%s\n' "$cargo_v" | sed 's/^/cargo: /'
    env | grep -E '^(RUSTFLAGS|CARGO_ENCODED_RUSTFLAGS|CARGO_PROFILE_RELEASE_[A-Za-z0-9_]*|RUSTUP_TOOLCHAIN|SDKROOT|DEVELOPER_DIR)=' |
        LC_ALL=C sort | sed 's/^/env: /'
    true
} >"$root/toolchain" || fail "cannot write $root/toolchain"

build() {
    name=$1
    shift
    echo "build-arm.sh: cargo build --release --locked $* into $root/arrakis-measure/$name" >&2
    (cd "$root/src" && CARGO_TARGET_DIR="$root/arrakis-measure/$name" cargo build --release --locked "$@") ||
        fail "the $name build failed"
    [ -x "$root/arrakis-measure/$name/release/arrakis" ] ||
        fail "the $name build left no $root/arrakis-measure/$name/release/arrakis"
}

# "sdk=26.5 minos=14.0" from the LC_BUILD_VERSION of the binary $1, and a failure without one.
platform() {
    otool -l "$1" | awk '
        /^Load command/ { inside = 0 }
        $1 == "cmd" && $2 == "LC_BUILD_VERSION" { inside = 1; found = 1 }
        inside && $1 == "minos" { minos = $2 }
        inside && $1 == "sdk" { sdk = $2 }
        END { if (found && sdk != "" && minos != "") printf "sdk=%s minos=%s\n", sdk, minos; else exit 1 }'
}

build real
build drive --features demo
lines=
for name in real drive; do
    read_sdk=$(platform "$root/arrakis-measure/$name/release/arrakis") ||
        fail "no LC_BUILD_VERSION with an sdk and a minos in the $name binary"
    lines="$lines$name $read_sdk
"
done
printf %s "$lines" >"$root/sdk" || fail "cannot write $root/sdk"
echo "$sha" >"$root/commit" || fail "cannot write $root/commit"
echo "build-arm.sh: $sha is built in $root"
cat "$root/sdk"
removal_hint
