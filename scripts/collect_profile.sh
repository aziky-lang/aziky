#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
    echo "usage: scripts/collect_profile.sh <input.azk> <output-prefix> [compiler flags ...]" >&2
    exit 1
fi

input=$1
prefix=$2
shift 2

compiler=target/debug/aziky
training_bin="${prefix}.training"
template="${prefix}.template"
raw="${prefix}.raw"
profile="${prefix}.profile"
optimized_bin="${prefix}.pgo"

cargo build -q --locked --offline
"$compiler" compile "$input" -o "$training_bin" \
    --profile-generate "$template" --profile-instrument "$@"

set +e
"$training_bin" >/dev/null 2>"$raw"
training_exit=$?
set -e
if [[ "$training_exit" -gt 127 ]]; then
    echo "error: instrumented training binary failed with status $training_exit" >&2
    exit 1
fi

"$compiler" profile-merge "$template" "$raw" -o "$profile"
"$compiler" compile "$input" -o "$optimized_bin" --profile-use "$profile" "$@"

printf 'training_exit=%s\nprofile=%s\noptimized_binary=%s\n' \
    "$training_exit" "$profile" "$optimized_bin"
