#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<USAGE
Usage:
  scripts/check_sort_bench_gate.sh <csv_report>

Checks sort benchmark scenarios in a bench CSV and enforces max Rust/C over Aziky ratios.

Environment:
  SORT_GATE_MAX_RUST_OVER_AZIKY (default: 2.0)
  SORT_GATE_MAX_C_OVER_AZIKY    (default: 2.0)
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

max_rust=${SORT_GATE_MAX_RUST_OVER_AZIKY:-2.0}
max_c=${SORT_GATE_MAX_C_OVER_AZIKY:-2.0}
scenarios=(
    sort_window
)

ratio_ok() {
    local ratio=$1
    local max=$2
    awk -v r="$ratio" -v m="$max" 'BEGIN {
        if (r == "" || r == "inf" || r == "nan" || r == "NaN") {
            exit 1
        }
        exit !((r + 0.0) <= (m + 0.0))
    }'
}

for scenario in "${scenarios[@]}"; do
    line=$(awk -F, -v s="$scenario" '$1 == s { print; exit }' "$csv_report")
    if [[ -z "$line" ]]; then
        echo "sort_gate=FAIL missing scenario '$scenario' in $csv_report" >&2
        exit 1
    fi

    rust_ratio=$(cut -d, -f5 <<<"$line")
    c_ratio=$(cut -d, -f6 <<<"$line")

    if ! ratio_ok "$rust_ratio" "$max_rust"; then
        echo "sort_gate=FAIL scenario=$scenario rust_over_aziky=$rust_ratio threshold=$max_rust" >&2
        exit 1
    fi
    if ! ratio_ok "$c_ratio" "$max_c"; then
        echo "sort_gate=FAIL scenario=$scenario c_over_aziky=$c_ratio threshold=$max_c" >&2
        exit 1
    fi

done

echo "sort_gate=PASS"
