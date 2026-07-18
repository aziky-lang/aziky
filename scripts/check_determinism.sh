#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<USAGE
Usage:
  scripts/check_determinism.sh <input.azk> [--runs N]

Builds the compiler (debug), compiles the same source N times, and verifies
all emitted binaries are byte-identical via SHA-256.
USAGE
}

if [[ $# -lt 1 ]]; then
    usage
    exit 1
fi

input="$1"
shift

runs=5
while [[ $# -gt 0 ]]; do
    case "$1" in
        --runs)
            runs="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage
            exit 1
            ;;
    esac
done

if [[ ! -f "$input" ]]; then
    echo "error: input not found: $input" >&2
    exit 1
fi

if ! [[ "$runs" =~ ^[0-9]+$ ]] || [[ "$runs" -lt 2 ]]; then
    echo "error: --runs must be an integer >= 2" >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    hash_cmd=(sha256sum)
elif command -v shasum >/dev/null 2>&1; then
    hash_cmd=(shasum -a 256)
else
    echo "error: sha256 tool not found (need sha256sum or shasum)" >&2
    exit 1
fi

echo "building compiler..."
cargo build -q --locked --offline

tmp_dir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

ref_hash=""
ref_path=""
for ((i = 1; i <= runs; i++)); do
    out="$tmp_dir/out_$i.bin"
    target/debug/aziky compile "$input" -o "$out" >/dev/null
    current_hash="$("${hash_cmd[@]}" "$out" | awk '{print $1}')"
    if [[ -z "$ref_hash" ]]; then
        ref_hash="$current_hash"
        ref_path="$out"
        continue
    fi
    if [[ "$current_hash" != "$ref_hash" ]]; then
        echo "determinism_check=FAILED"
        echo "input=$input"
        echo "runs=$runs"
        echo "expected_hash=$ref_hash"
        echo "actual_hash=$current_hash"
        echo "reference_output=$ref_path"
        echo "mismatch_output=$out"
        exit 2
    fi
done

echo "determinism_check=PASS"
echo "input=$input"
echo "runs=$runs"
echo "sha256=$ref_hash"
