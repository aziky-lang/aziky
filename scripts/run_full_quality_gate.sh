#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<USAGE
Usage:
  scripts/run_full_quality_gate.sh [--runs N] [--warmup N] [--fuzz-cases N] [--stress-runs N] [--stress-iters N]
                              [--cpu-set LIST] [--rt-prio N]
                              [--score-stat mean|median|trimmed] [--trim-pct N]

Runs the complete local quality and performance gate:
  1) unit/integration tests
  2) frontend fuzz smoke
  3) differential encoder checks
  4) determinism check on representative azk programs
  5) independent clean-build artifact reproducibility
  6) benchmark suite (Aziky vs Rust vs C)
  7) sort benchmark ratio gate
  8) allocator stress benchmark + batching/remote-free gate
USAGE
}

runs=20
warmup=3
fuzz_cases=200
stress_runs=1
stress_iters=100000
cpu_set="${AZIKY_BENCH_CPU_SET:-}"
rt_prio="${AZIKY_BENCH_RT_PRIO:-}"
score_stat="${AZIKY_BENCH_SCORE_STAT:-median}"
trim_pct="${AZIKY_BENCH_TRIM_PCT:-10}"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --runs)
            runs="$2"
            shift 2
            ;;
        --warmup)
            warmup="$2"
            shift 2
            ;;
        --fuzz-cases)
            fuzz_cases="$2"
            shift 2
            ;;
        --stress-runs)
            stress_runs="$2"
            shift 2
            ;;
        --stress-iters)
            stress_iters="$2"
            shift 2
            ;;
        --cpu-set)
            cpu_set="$2"
            shift 2
            ;;
        --rt-prio)
            rt_prio="$2"
            shift 2
            ;;
        --score-stat)
            score_stat="$2"
            shift 2
            ;;
        --trim-pct)
            trim_pct="$2"
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

echo "[1/8] cargo test"
cargo test -q --locked --offline

echo "[2/8] frontend fuzz smoke"
bash scripts/fuzz_frontend.sh --cases "$fuzz_cases"

echo "[3/8] differential encoder checks"
bash scripts/differential_encoder_check.sh

echo "[4/8] determinism checks"
scripts/check_determinism.sh examples/hello.azk --runs 5
scripts/check_determinism.sh bench/stream_lcg.azk --runs 5
scripts/check_determinism.sh bench/packet_classifier.azk --runs 5

echo "[5/8] independent clean-build artifact reproducibility"
scripts/check_reproducible_build.sh examples/foundation_app/main.azk

echo "[6/8] benchmark suite"
csv_out="target/bench/full_quality_gate_latest.csv"
bench_args=(
    --runs "$runs"
    --warmup "$warmup"
    --csv "$csv_out"
    --score-stat "$score_stat"
    --trim-pct "$trim_pct"
)
if [[ -n "$cpu_set" ]]; then
    bench_args+=(--cpu-set "$cpu_set")
fi
if [[ -n "$rt_prio" ]]; then
    bench_args+=(--rt-prio "$rt_prio")
fi
scripts/run_bench_suite.sh "${bench_args[@]}"
echo "benchmark_csv=$csv_out"

echo "[7/8] sort benchmark gate"
scripts/check_sort_bench_gate.sh "$csv_out"

echo "[8/8] allocator stress gate"
allocator_stress_csv="target/bench/allocator_stress_gate.csv"
scripts/run_allocator_stress.sh \
    --runs "$stress_runs" \
    --threads 2,4 \
    --batches 16,64 \
    --shard-mults 1 \
    --iters "$stress_iters" \
    --size 64 \
    --drain-every 32 \
    --csv "$allocator_stress_csv"
scripts/check_allocator_stress_gate.sh "$allocator_stress_csv"
echo "allocator_stress_csv=$allocator_stress_csv"

echo "full_quality_gate=PASS"
