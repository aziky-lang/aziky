#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<USAGE
Usage:
  scripts/run_bench_suite.sh [--runs N] [--warmup N] [--csv PATH]
                             [--cpu-set LIST] [--rt-prio N]
                             [--score-stat mean|median|trimmed] [--trim-pct N]
                             [--perf-counters] [--perf-runs N] [--perf-events LIST]

Compiles benchmark triplets in bench/*.azk, bench/*.rs, bench/*.c,
runs each binary separately, and prints Aziky vs Rust vs optimized C timings.
USAGE
}

runs=20
warmup=3
csv_out=""
cpu_set="${AZIKY_BENCH_CPU_SET:-}"
rt_prio="${AZIKY_BENCH_RT_PRIO:-}"
score_stat="${AZIKY_BENCH_SCORE_STAT:-median}"
trim_pct="${AZIKY_BENCH_TRIM_PCT:-10}"
perf_counters="${AZIKY_BENCH_PERF_COUNTERS:-0}"
perf_runs="${AZIKY_BENCH_PERF_RUNS:-3}"
perf_events="${AZIKY_BENCH_PERF_EVENTS:-cycles,instructions,branches,branch-misses,cache-misses}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --runs)
            runs=$2
            shift 2
            ;;
        --warmup)
            warmup=$2
            shift 2
            ;;
        --csv)
            csv_out=$2
            shift 2
            ;;
        --cpu-set)
            cpu_set=$2
            shift 2
            ;;
        --rt-prio)
            rt_prio=$2
            shift 2
            ;;
        --score-stat)
            score_stat=$2
            shift 2
            ;;
        --trim-pct)
            trim_pct=$2
            shift 2
            ;;
        --perf-counters)
            perf_counters=1
            shift
            ;;
        --perf-runs)
            perf_runs=$2
            shift 2
            ;;
        --perf-events)
            perf_events=$2
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

if ! [[ "$runs" =~ ^[0-9]+$ ]] || ! [[ "$warmup" =~ ^[0-9]+$ ]]; then
    echo "error: --runs and --warmup must be non-negative integers" >&2
    exit 1
fi
if ! [[ "$trim_pct" =~ ^[0-9]+$ ]]; then
    echo "error: --trim-pct must be a non-negative integer" >&2
    exit 1
fi
case "$score_stat" in
    mean|median|trimmed) ;;
    *)
        echo "error: --score-stat must be one of mean|median|trimmed" >&2
        exit 1
        ;;
esac

bench_dir="bench"
build_dir="target/bench"
scenarios=(
    stream_lcg
    packet_classifier
    ring_write
    affine_mix
    sort_window
    bloom_filter
    hash_join
    histogram
    binary_search
    prefix_scan
)

python3 scripts/check_benchmark_contracts.py "${scenarios[@]}"

if command -v clang >/dev/null 2>&1; then
    c_compiler="clang"
elif command -v gcc >/dev/null 2>&1; then
    c_compiler="gcc"
elif command -v cc >/dev/null 2>&1; then
    c_compiler="cc"
else
    echo "error: no C compiler found (clang/gcc/cc)" >&2
    exit 1
fi

c_flags=(
    -std=c17
    -O3
    -march=native
    -fomit-frame-pointer
    -fno-asynchronous-unwind-tables
    -fno-unwind-tables
    -DNDEBUG
)

rust_flags=(
    --edition=2021
    -C opt-level=3
    -C target-cpu=native
    -C codegen-units=1
    -C panic=abort
    -C lto=fat
    -C overflow-checks=off
)

if "$c_compiler" "${c_flags[@]}" -flto -x c - -o /tmp/aziky_lto_probe_bin >/dev/null 2>&1 <<'LTOPROBE'
int main(void) { return 0; }
LTOPROBE
then
    c_flags+=(-flto)
fi
rm -f /tmp/aziky_lto_probe_bin

echo "building aziky..."
cargo build -q --locked --offline
mkdir -p "$build_dir"

echo "using C compiler: $c_compiler"
printf "using C flags:"
printf " %s" "${c_flags[@]}"
printf "\n"
printf "using Rust flags:"
printf " %s" "${rust_flags[@]}"
printf "\n"
printf "timing score_stat=%s trim_pct=%s cpu_set=%s rt_prio=%s\n" \
    "$score_stat" "$trim_pct" "${cpu_set:-none}" "${rt_prio:-none}"

if [[ -n "$csv_out" ]]; then
    mkdir -p "$(dirname "$csv_out")"
    printf "scenario,aziky_ms,rust_ms,c_ms,rust_over_aziky,c_over_aziky,runs,warmup,c_compiler,score_stat,trim_pct,cpu_set,rt_prio,exit_code,checksum_le,aziky_cycles,aziky_instructions,aziky_ipc,aziky_branch_miss_pct,aziky_cache_misses_per_kinst,rust_cycles,rust_instructions,rust_ipc,rust_branch_miss_pct,rust_cache_misses_per_kinst,c_cycles,c_instructions,c_ipc,c_branch_miss_pct,c_cache_misses_per_kinst\n" >"$csv_out"
fi

printf "\n%-20s %11s %11s %11s %11s %11s\n" "scenario" "aziky_ms" "rust_ms" "c_ms" "rust/azk" "c/azk"
printf "%-20s %11s %11s %11s %11s %11s\n" "--------------------" "-----------" "-----------" "-----------" "-----------" "-----------"

sum_rust_speedup=0
sum_c_speedup=0
product_rust_speedup=1
product_c_speedup=1
count=0

timer_args=(
    --runs "$runs"
    --warmup "$warmup"
    --score-stat "$score_stat"
    --trim-pct "$trim_pct"
)
if [[ "$perf_counters" -eq 1 ]]; then
    timer_args+=(--perf-counters --perf-runs "$perf_runs" --perf-events "$perf_events")
fi
if [[ -n "$cpu_set" ]]; then
    timer_args+=(--cpu-set "$cpu_set")
fi
if [[ -n "$rt_prio" ]]; then
    timer_args+=(--rt-prio "$rt_prio")
fi

extract_score_ms() {
    awk '{for(i=1;i<=NF;i++){if($i ~ /^score_ms=/){split($i,a,"="); print a[2]; exit}}}' <<<"$1"
}

extract_field() {
    local field=$1
    local report=$2
    awk -v field="$field" '{for(i=1;i<=NF;i++){if($i ~ ("^" field "=")){split($i,a,"="); print a[2]; exit}}}' <<<"$report"
}

capture_exit_code() {
    local label=$1
    local binary=$2
    local rc

    set +e
    "$binary" >/dev/null 2>&1
    rc=$?
    set -e

    case "$rc" in
        132|133|134|135|136|137|139)
            echo "error: benchmark preflight crashed (label=$label exit_code=$rc binary=$binary)" >&2
            exit 1
            ;;
    esac

    if [[ "$rc" -gt 127 ]]; then
        echo "error: benchmark preflight failed (label=$label exit_code=$rc binary=$binary)" >&2
        exit 1
    fi

    printf "%s" "$rc"
}

capture_full_checksum() {
    local label=$1
    local binary=$2
    local output="$build_dir/.${label}.checksum"
    local rc bytes checksum

    set +e
    "$binary" >"$output" 2>/dev/null
    rc=$?
    set -e
    if [[ "$rc" -gt 127 ]]; then
        echo "error: checksum verification binary failed (label=$label exit_code=$rc)" >&2
        exit 1
    fi
    bytes=$(wc -c <"$output")
    if [[ "$bytes" -ne 8 ]]; then
        echo "error: checksum verification expected 8 bytes (label=$label bytes=$bytes)" >&2
        exit 1
    fi
    checksum=$(od -An -tx1 -v "$output" | tr -d '[:space:]')
    rm -f "$output"
    printf "%s" "$checksum"
}

for scenario in "${scenarios[@]}"; do
    azk_src="$bench_dir/$scenario.azk"
    rust_src="$bench_dir/$scenario.rs"
    c_src="$bench_dir/$scenario.c"

    azk_bin="$build_dir/aziky_$scenario"
    rust_bin="$build_dir/rust_$scenario"
    c_bin="$build_dir/c_$scenario"
    azk_verify_bin="$build_dir/verify_aziky_$scenario"
    rust_verify_bin="$build_dir/verify_rust_$scenario"
    c_verify_bin="$build_dir/verify_c_$scenario"

    if [[ ! -f "$azk_src" || ! -f "$rust_src" || ! -f "$c_src" ]]; then
        echo "skipping $scenario (missing azk/rs/c source triple)"
        continue
    fi

    rm -f "$azk_bin" "$rust_bin" "$c_bin" \
        "$azk_verify_bin" "$rust_verify_bin" "$c_verify_bin"
    target/debug/aziky compile "$azk_src" -o "$azk_bin" \
        --preserve-full-checksum >/dev/null
    rustc "${rust_flags[@]}" "$rust_src" -o "$rust_bin"
    "$c_compiler" "${c_flags[@]}" "$c_src" -o "$c_bin"

    target/debug/aziky compile "$azk_src" -o "$azk_verify_bin" \
        --emit-full-checksum >/dev/null
    rustc "${rust_flags[@]}" --cfg aziky_verify "$rust_src" -o "$rust_verify_bin"
    "$c_compiler" "${c_flags[@]}" -DAZIKY_VERIFY "$c_src" -o "$c_verify_bin"

    azk_exit=$(capture_exit_code "aziky_$scenario" "$azk_bin")
    rust_exit=$(capture_exit_code "rust_$scenario" "$rust_bin")
    c_exit=$(capture_exit_code "c_$scenario" "$c_bin")
    if [[ "$azk_exit" != "$rust_exit" || "$azk_exit" != "$c_exit" ]]; then
        echo "error: benchmark exit-code mismatch for scenario '$scenario'" >&2
        echo "aziky=$azk_exit" >&2
        echo "rust=$rust_exit" >&2
        echo "c=$c_exit" >&2
        exit 1
    fi

    azk_checksum=$(capture_full_checksum "aziky_$scenario" "$azk_verify_bin")
    rust_checksum=$(capture_full_checksum "rust_$scenario" "$rust_verify_bin")
    c_checksum=$(capture_full_checksum "c_$scenario" "$c_verify_bin")
    if [[ "$azk_checksum" != "$rust_checksum" || "$azk_checksum" != "$c_checksum" ]]; then
        echo "error: full-width checksum mismatch for scenario '$scenario'" >&2
        echo "aziky=$azk_checksum" >&2
        echo "rust=$rust_checksum" >&2
        echo "c=$c_checksum" >&2
        exit 1
    fi

    # Rotate the execution order to distribute thermal/frequency bias instead
    # of always favoring or penalizing the same implementation.
    case $((count % 3)) in
        0)
            azk_report=$(scripts/time_binary.sh "$azk_bin" "${timer_args[@]}" --label "aziky_$scenario")
            rust_report=$(scripts/time_binary.sh "$rust_bin" "${timer_args[@]}" --label "rust_$scenario")
            c_report=$(scripts/time_binary.sh "$c_bin" "${timer_args[@]}" --label "c_$scenario")
            ;;
        1)
            rust_report=$(scripts/time_binary.sh "$rust_bin" "${timer_args[@]}" --label "rust_$scenario")
            c_report=$(scripts/time_binary.sh "$c_bin" "${timer_args[@]}" --label "c_$scenario")
            azk_report=$(scripts/time_binary.sh "$azk_bin" "${timer_args[@]}" --label "aziky_$scenario")
            ;;
        2)
            c_report=$(scripts/time_binary.sh "$c_bin" "${timer_args[@]}" --label "c_$scenario")
            azk_report=$(scripts/time_binary.sh "$azk_bin" "${timer_args[@]}" --label "aziky_$scenario")
            rust_report=$(scripts/time_binary.sh "$rust_bin" "${timer_args[@]}" --label "rust_$scenario")
            ;;
    esac

    azk_ms=$(extract_score_ms "$azk_report")
    rust_ms=$(extract_score_ms "$rust_report")
    c_ms=$(extract_score_ms "$c_report")
    azk_cycles=$(extract_field perf_cycles "$azk_report")
    azk_instructions=$(extract_field perf_instructions "$azk_report")
    azk_ipc=$(extract_field perf_ipc "$azk_report")
    azk_branch_miss_pct=$(extract_field perf_branch_miss_pct "$azk_report")
    azk_cache_misses_per_kinst=$(extract_field perf_cache_misses_per_kinst "$azk_report")
    rust_cycles=$(extract_field perf_cycles "$rust_report")
    rust_instructions=$(extract_field perf_instructions "$rust_report")
    rust_ipc=$(extract_field perf_ipc "$rust_report")
    rust_branch_miss_pct=$(extract_field perf_branch_miss_pct "$rust_report")
    rust_cache_misses_per_kinst=$(extract_field perf_cache_misses_per_kinst "$rust_report")
    c_cycles=$(extract_field perf_cycles "$c_report")
    c_instructions=$(extract_field perf_instructions "$c_report")
    c_ipc=$(extract_field perf_ipc "$c_report")
    c_branch_miss_pct=$(extract_field perf_branch_miss_pct "$c_report")
    c_cache_misses_per_kinst=$(extract_field perf_cache_misses_per_kinst "$c_report")

    rust_speedup=$(awk -v a="$azk_ms" -v r="$rust_ms" 'BEGIN { if (a == 0) print "inf"; else printf "%.3f", r / a }')
    c_speedup=$(awk -v a="$azk_ms" -v c="$c_ms" 'BEGIN { if (a == 0) print "inf"; else printf "%.3f", c / a }')

    printf "%-20s %11s %11s %11s %11sx %11sx\n" "$scenario" "$azk_ms" "$rust_ms" "$c_ms" "$rust_speedup" "$c_speedup"
    if [[ -n "$csv_out" ]]; then
        printf "%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n" \
            "$scenario" "$azk_ms" "$rust_ms" "$c_ms" "$rust_speedup" "$c_speedup" \
            "$runs" "$warmup" "$c_compiler" "$score_stat" "$trim_pct" "${cpu_set:-none}" "${rt_prio:-none}" "$azk_exit" "$azk_checksum" \
            "$azk_cycles" "$azk_instructions" "$azk_ipc" "$azk_branch_miss_pct" "$azk_cache_misses_per_kinst" \
            "$rust_cycles" "$rust_instructions" "$rust_ipc" "$rust_branch_miss_pct" "$rust_cache_misses_per_kinst" \
            "$c_cycles" "$c_instructions" "$c_ipc" "$c_branch_miss_pct" "$c_cache_misses_per_kinst" >>"$csv_out"
    fi

    sum_rust_speedup=$(awk -v s="$sum_rust_speedup" -v x="$rust_speedup" 'BEGIN { printf "%.9f", s + x }')
    sum_c_speedup=$(awk -v s="$sum_c_speedup" -v x="$c_speedup" 'BEGIN { printf "%.9f", s + x }')
    product_rust_speedup=$(awk -v p="$product_rust_speedup" -v x="$rust_speedup" 'BEGIN { printf "%.12g", p * x }')
    product_c_speedup=$(awk -v p="$product_c_speedup" -v x="$c_speedup" 'BEGIN { printf "%.12g", p * x }')
    count=$((count + 1))
done

if [[ "$count" -gt 0 ]]; then
    avg_rust_speedup=$(awk -v s="$sum_rust_speedup" -v n="$count" 'BEGIN { printf "%.3f", s / n }')
    avg_c_speedup=$(awk -v s="$sum_c_speedup" -v n="$count" 'BEGIN { printf "%.3f", s / n }')
    geomean_rust_speedup=$(awk -v p="$product_rust_speedup" -v n="$count" 'BEGIN { printf "%.3f", exp(log(p) / n) }')
    geomean_c_speedup=$(awk -v p="$product_c_speedup" -v n="$count" 'BEGIN { printf "%.3f", exp(log(p) / n) }')
    printf "\navg_rust_over_aziky=%sx (%s scenarios)\n" "$avg_rust_speedup" "$count"
    printf "avg_c_over_aziky=%sx (%s scenarios)\n" "$avg_c_speedup" "$count"
    printf "geomean_rust_over_aziky=%sx (%s scenarios)\n" "$geomean_rust_speedup" "$count"
    printf "geomean_c_over_aziky=%sx (%s scenarios)\n" "$geomean_c_speedup" "$count"
    if [[ -n "$csv_out" ]]; then
        printf "%s,,,,%s,%s,%s,%s,%s,%s,%s,%s,%s,,,,,,,,,,,,,,,,,\n" \
            "AVERAGE" "$avg_rust_speedup" "$avg_c_speedup" "$runs" "$warmup" "$c_compiler" \
            "$score_stat" "$trim_pct" "${cpu_set:-none}" "${rt_prio:-none}" >>"$csv_out"
        printf "%s,,,,%s,%s,%s,%s,%s,%s,%s,%s,%s,,,,,,,,,,,,,,,,,\n" \
            "GEOMEAN" "$geomean_rust_speedup" "$geomean_c_speedup" "$runs" "$warmup" "$c_compiler" \
            "$score_stat" "$trim_pct" "${cpu_set:-none}" "${rt_prio:-none}" >>"$csv_out"
        echo "csv_report=$csv_out"
    fi
fi
