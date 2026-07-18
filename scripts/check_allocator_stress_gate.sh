#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<USAGE
Usage:
  scripts/check_allocator_stress_gate.sh <csv_report>

Checks allocator stress CSV rows and enforces minimum remote-free/batching behavior.

Environment:
  ALLOC_GATE_MIN_REMOTE_FREE_RATIO (default: 0.10)
  ALLOC_GATE_MIN_AVG_REMOTE_BATCH  (default: 2.0)
USAGE
}

if [[ $# -ne 1 ]]; then
    usage
    exit 1
fi

csv_report=$1
if [[ ! -f "$csv_report" ]]; then
    echo "error: missing csv report: $csv_report" >&2
    exit 1
fi

min_remote_ratio=${ALLOC_GATE_MIN_REMOTE_FREE_RATIO:-0.10}
min_avg_batch=${ALLOC_GATE_MIN_AVG_REMOTE_BATCH:-2.0}

is_positive() {
    local value=$1
    awk -v v="$value" 'BEGIN {
        if (v == "" || v == "inf" || v == "nan" || v == "NaN") exit 1;
        exit !((v + 0.0) > 0.0);
    }'
}

at_least() {
    local value=$1
    local min=$2
    awk -v v="$value" -v m="$min" 'BEGIN {
        if (v == "" || v == "inf" || v == "nan" || v == "NaN") exit 1;
        exit !((v + 0.0) >= (m + 0.0));
    }'
}

row_count=0
while IFS=, read -r threads shards batch iters size drain_every runs ops_per_sec remote_frees remote_flushes avg_remote_batch remote_free_ratio fresh_allocs local_reuses remote_reuses; do
    if [[ "$threads" == "threads" ]]; then
        continue
    fi
    row_count=$((row_count + 1))

    if ! is_positive "$ops_per_sec"; then
        echo "allocator_stress_gate=FAIL threads=$threads batch=$batch invalid_ops_per_sec=$ops_per_sec" >&2
        exit 1
    fi
    if ! is_positive "$remote_frees"; then
        echo "allocator_stress_gate=FAIL threads=$threads batch=$batch remote_frees=$remote_frees" >&2
        exit 1
    fi
    if ! is_positive "$remote_flushes"; then
        echo "allocator_stress_gate=FAIL threads=$threads batch=$batch remote_flushes=$remote_flushes" >&2
        exit 1
    fi
    if ! at_least "$avg_remote_batch" "$min_avg_batch"; then
        echo "allocator_stress_gate=FAIL threads=$threads batch=$batch avg_remote_batch=$avg_remote_batch threshold=$min_avg_batch" >&2
        exit 1
    fi
    if ! at_least "$remote_free_ratio" "$min_remote_ratio"; then
        echo "allocator_stress_gate=FAIL threads=$threads batch=$batch remote_free_ratio=$remote_free_ratio threshold=$min_remote_ratio" >&2
        exit 1
    fi
done <"$csv_report"

if [[ "$row_count" -eq 0 ]]; then
    echo "allocator_stress_gate=FAIL empty_csv=$csv_report" >&2
    exit 1
fi

echo "allocator_stress_gate=PASS"
