#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
    echo "usage: scripts/tune_variants.sh <input.azk> <output-dir> [--runs N] [--warmup N] [--cpu-set LIST] [--profile PATH]" >&2
    exit 1
fi

input=$1
output_dir=$2
shift 2
runs=20
warmup=3
cpu_set="${AZIKY_BENCH_CPU_SET:-}"
profile=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --runs) runs=$2; shift 2 ;;
        --warmup) warmup=$2; shift 2 ;;
        --cpu-set) cpu_set=$2; shift 2 ;;
        --profile) profile=$2; shift 2 ;;
        *) echo "error: unknown argument '$1'" >&2; exit 1 ;;
    esac
done

cargo build -q --locked --offline
mkdir -p "$output_dir"
compiler=target/debug/aziky
variants=(native no_bmi2 scalar)

compile_variant() {
    local variant=$1
    local output=$2
    local verify=$3
    local flags=()
    case "$variant" in
        native) ;;
        no_bmi2) flags+=(--target-features avx2,popcnt) ;;
        scalar) flags+=(--target-features none) ;;
    esac
    if [[ -n "$profile" ]]; then
        flags+=(--profile-use "$profile")
    fi
    if [[ "$verify" -eq 1 ]]; then
        flags+=(--emit-full-checksum)
    else
        flags+=(--preserve-full-checksum)
    fi
    "$compiler" compile "$input" -o "$output" "${flags[@]}" >/dev/null
}

reference_checksum=""
best_variant=""
best_ns=""
printf '%-12s %12s %18s\n' variant median_ms checksum_le
for variant in "${variants[@]}"; do
    binary="$output_dir/$variant"
    verify="$output_dir/$variant.verify"
    raw="$output_dir/$variant.checksum"
    compile_variant "$variant" "$binary" 0
    compile_variant "$variant" "$verify" 1
    set +e
    "$verify" >"$raw" 2>/dev/null
    verify_exit=$?
    set -e
    if [[ "$verify_exit" -gt 127 || $(wc -c <"$raw") -ne 8 ]]; then
        echo "error: verification failed for variant '$variant'" >&2
        exit 1
    fi
    checksum=$(od -An -tx1 -v "$raw" | tr -d '[:space:]')
    if [[ -z "$reference_checksum" ]]; then
        reference_checksum=$checksum
    elif [[ "$checksum" != "$reference_checksum" ]]; then
        echo "error: full-width checksum mismatch for variant '$variant'" >&2
        exit 1
    fi

    timer=(--runs "$runs" --warmup "$warmup" --score-stat median --label "$variant")
    if [[ -n "$cpu_set" ]]; then
        timer+=(--cpu-set "$cpu_set")
    fi
    report=$(scripts/time_binary.sh "$binary" "${timer[@]}")
    median_ns=$(awk '{for(i=1;i<=NF;i++) if($i ~ /^median_ns=/){split($i,a,"="); print a[2]; exit}}' <<<"$report")
    median_ms=$(awk -v ns="$median_ns" 'BEGIN { printf "%.6f", ns / 1000000.0 }')
    printf '%-12s %12s %18s\n' "$variant" "$median_ms" "$checksum"
    if [[ -z "$best_ns" || "$median_ns" -lt "$best_ns" ]]; then
        best_ns=$median_ns
        best_variant=$variant
    fi
done

printf 'winner=%s median_ns=%s binary=%s\n' \
    "$best_variant" "$best_ns" "$output_dir/$best_variant"
