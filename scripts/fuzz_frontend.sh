#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<USAGE
Usage:
  scripts/fuzz_frontend.sh [--cases N]

Runs a deterministic frontend fuzz smoke pass and fails on compiler panics.
USAGE
}

cases=200
while [[ $# -gt 0 ]]; do
    case "$1" in
        --cases)
            cases="$2"
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

if ! [[ "$cases" =~ ^[0-9]+$ ]] || [[ "$cases" -lt 1 ]]; then
    echo "error: --cases must be an integer >= 1" >&2
    exit 1
fi

cargo build -q --locked --offline

tmp_dir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

snippets=(
    "fn main() { exit(0); }"
    "fn main() { let x: i32 = 1i32 + 2i32; print(x.to_str()); exit(0); }"
    "fn main() { let mut a: [u8; 2] = [1u8, 2u8]; a[1u8] = 3u8; exit(0); }"
    "fn main() { let mut d: dict<string, i32> = {\"k\": 1i32}; d[\"k\"] = 2i32; exit(0); }"
    "fn main() { if true { print(\"x\"); } else { print(\"y\"); } exit(0); }"
    "fn main() { while false { } exit(0); }"
)

for ((i = 0; i < cases; i++)); do
    idx=$((i % ${#snippets[@]}))
    src="${snippets[$idx]}"
    # deterministic mutation pattern
    if (( i % 7 == 0 )); then
        src="${src//;/}"
    elif (( i % 11 == 0 )); then
        src="${src/\{/\{}"
        src+=" }"
    elif (( i % 13 == 0 )); then
        src+=" ???"
    fi
    in_file="$tmp_dir/case_$i.azk"
    out_file="$tmp_dir/out_$i.bin"
    printf "%s\n" "$src" > "$in_file"
    set +e
    output="$(target/debug/aziky compile "$in_file" -o "$out_file" 2>&1)"
    status=$?
    set -e
    if grep -qi "panicked at" <<<"$output"; then
        echo "frontend_fuzz=FAILED case=$i status=$status" >&2
        echo "$output" >&2
        exit 2
    fi
done

echo "frontend_fuzz=PASS cases=$cases"
