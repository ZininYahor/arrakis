#!/bin/sh
# Which merge of a block moved a figure? Lists the merges between a good build and a bad one, and builds
# the middle one as an arm (build-arm.sh), to be read. It reads and runs nothing else.
#
# usage: bisect-arms.sh <base> <tip> [<root> [<base-root>]]
#   base  a commit whose reading is good, such as the one before the merges in question
#   tip   a commit whose reading is bad
#   root  where to build the middle merge (build-arm.sh's <root>: one of its own, never another arm's).
#         Without it, the merges are listed and the middle is named, and nothing is built.
#   base-root  the root of the base's arm (build-arm.sh <base> <base-root>). Once the middle is built, the
#         script compares the two arms' `sdk` and `toolchain` files (build-arm.sh's header) and warns, with
#         the lines that differ, when they do: a reading of two arms from different toolchains compares the
#         toolchains. It warns too when either arm has no finished build to read them from.
# The candidates are the first-parent merge commits of <tip> since <base>, oldest first, <tip> included
# (as a candidate even when it is no merge) and <base> not: the branch takes a piece of work as one merge,
# and a commit made straight on it falls to the next merge. <tip> is known bad, so the middle is candidate
# k/2 of k. With one candidate left, that is the first bad merge, and nothing is built.
#
# How a reading decides good or bad. It is the recipe that read the final build and found the figure above
# its limit: the same script, runs, flag and seconds, and that limit with its rule, such as a median of 3
# or the ranges apart. A step is one reading of the middle arm IN TURNS with the base's arm
# (build-arm.sh <base> <root of its own>) in the same sitting: a miss that the base's arm shares is the
# sitting's, not the merge's. The middle is BAD when it misses the limit and the base's arm in those turns
# does not, and GOOD otherwise. Say which sdk each arm has (the `sdk` file): both
# arms of a pair come from one toolchain. Then run this again:
#   bad:  with <tip> = the middle merge          good: with <base> = the middle merge
# Each reading is yours to run: this script runs none and starts no app.
set -u
refuse() {
    echo "bisect-arms.sh: $*" >&2
    exit 2
}
[ $# -ge 2 ] && [ $# -le 4 ] || {
    echo "usage: bisect-arms.sh <base> <tip> [<root> [<base-root>]]  (see the header)" >&2
    exit 2
}
HERE=$(cd "$(dirname "$0")" && pwd -P)
REPO=$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null) || refuse "$HERE is not in a git repository"
base=$(git -C "$REPO" rev-parse --verify --quiet "$1^{commit}") || refuse "'$1' does not resolve to a commit"
tip=$(git -C "$REPO" rev-parse --verify --quiet "$2^{commit}") || refuse "'$2' does not resolve to a commit"
root=${3-} base_root=${4-}
[ $# -eq 2 ] || [ -n "$root" ] || refuse "the root is empty"
[ $# -lt 4 ] || [ -n "$base_root" ] || refuse "the base's root is empty"
[ "$base" != "$tip" ] || refuse "the base and the tip are one commit"
git -C "$REPO" merge-base --is-ancestor "$base" "$tip" || refuse "the base $base is no ancestor of the tip $tip"

candidates=$(git -C "$REPO" rev-list --first-parent --merges --reverse "$base..$tip")
last=$(echo "$candidates" | tail -n 1)
if [ "$last" != "$tip" ]; then
    candidates="$candidates
$tip"
fi
candidates=$(echo "$candidates" | sed '/^$/d')
k=$(echo "$candidates" | wc -l | tr -d ' ')

echo "bisect-arms.sh: $k candidate(s) after $base, up to $tip:"
n=0
for sha in $candidates; do
    n=$((n + 1))
    echo "  $n $(git -C "$REPO" log -1 --format='%h %s' "$sha")"
done
if [ "$k" -eq 1 ]; then
    echo "bisect-arms.sh: one candidate left: the first bad merge is $tip. Nothing is built."
    exit 0
fi
middle=$((k / 2))
sha=$(echo "$candidates" | sed -n "${middle}p")
echo "bisect-arms.sh: the middle is $middle of $k, $sha"
[ -n "$root" ] || exit 0
"$HERE/build-arm.sh" "$sha" "$root" || exit $?
# The base's arm and this step's, from one toolchain or the reading compares the toolchains.
if [ -n "$base_root" ]; then
    for file in sdk toolchain; do
        for arm in "$base_root" "$root"; do
            if [ ! -e "$arm/commit" ] || [ ! -f "$arm/$file" ]; then
                echo "bisect-arms.sh: WARNING: $arm holds no finished arm with a $file file, so its $file cannot be compared with the other arm's" >&2
                continue 2
            fi
        done
        if ! difference=$(diff "$base_root/$file" "$root/$file" 2>&1); then
            echo "bisect-arms.sh: WARNING: the base's arm ($base_root) and the middle's ($root) differ in their $file; a reading of the two compares the toolchains as well as the code (< the base's, > the middle's):" >&2
            echo "$difference" >&2
        fi
    done
fi
echo "bisect-arms.sh: read it in turns with the base's arm. Bad: run again with the tip $sha. Good: run again with the base $sha."
