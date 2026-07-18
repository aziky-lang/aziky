#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<USAGE
Usage:
  scripts/compare_aziky_rust_bench.sh <aziky_bin> <rust_bin> [--label-aziky NAME] [--label-rust NAME] [--keep-output]

Runs each binary separately (sequentially) and reports wall-clock duration.
USAGE
}

if [[ $# -lt 2 ]]; then
    usage
    exit 1
fi

aziky_bin=$1
rust_bin=$2
shift 2

label_aziky="aziky"
label_rust="rust"
redirect_output=1

while [[ $# -gt 0 ]]; do
    case "$1" in
        --label-aziky)
            label_aziky=$2
            shift 2
            ;;
        --label-rust)
            label_rust=$2
            shift 2
            ;;
        --keep-output)
            redirect_output=0
            shift
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

for bin in "$aziky_bin" "$rust_bin"; do
    if [[ ! -x "$bin" ]]; then
        echo "error: binary is not executable: $bin" >&2
        exit 1
    fi
done

now_ns() {
    date +%s%N
}

run_once() {
    local label=$1
    local bin=$2
    local start end elapsed rc

    start=$(now_ns)
    set +e
    if [[ "$redirect_output" -eq 1 ]]; then
        "$bin" >/dev/null 2>&1
    else
        "$bin"
    fi
    rc=$?
    set -e
    end=$(now_ns)
    elapsed=$((end - start))

    awk -v name="$label" -v ns="$elapsed" -v rc="$rc" 'BEGIN { printf "%s_exit_code=%s\n%s_duration_ns=%s\n%s_duration_ms=%.6f\n", name, rc, name, ns, name, ns / 1000000.0 }'
}

run_once "$label_aziky" "$aziky_bin"
run_once "$label_rust" "$rust_bin"
