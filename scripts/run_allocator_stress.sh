#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<USAGE
Usage:
  scripts/run_allocator_stress.sh [--runs N] [--threads LIST] [--batches LIST] [--shard-mults LIST] [--iters N] [--size N] [--drain-every N] [--csv PATH]

Runs the multi-thread remote-free-heavy allocator stress benchmark matrix.

Examples:
  scripts/run_allocator_stress.sh
  scripts/run_allocator_stress.sh --runs 5 --threads 2,4,8 --batches 8,32,128 --shard-mults 1,2
USAGE
}

runs=3
threads_csv="2,4,8"
batches_csv="8,32,128"
shard_mults_csv="1,2"
iters=250000
alloc_size=64
drain_every=64
csv_out="target/bench/allocator_stress_latest.csv"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --runs)
            runs=$2
            shift 2
            ;;
        --threads)
            threads_csv=$2
            shift 2
            ;;
        --batches)
            batches_csv=$2
            shift 2
            ;;
        --shard-mults)
            shard_mults_csv=$2
            shift 2
            ;;
        --iters)
            iters=$2
            shift 2
            ;;
        --size)
            alloc_size=$2
            shift 2
            ;;
        --drain-every)
            drain_every=$2
            shift 2
            ;;
        --csv)
            csv_out=$2
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

for v in "$runs" "$iters" "$alloc_size" "$drain_every"; do
    if ! [[ "$v" =~ ^[0-9]+$ ]] || [[ "$v" -eq 0 ]]; then
        echo "error: numeric options must be positive integers" >&2
        exit 1
    fi
done

readarray -t threads < <(tr ',' '\n' <<<"$threads_csv" | sed '/^$/d')
readarray -t batches < <(tr ',' '\n' <<<"$batches_csv" | sed '/^$/d')
readarray -t shard_mults < <(tr ',' '\n' <<<"$shard_mults_csv" | sed '/^$/d')

if [[ "${#threads[@]}" -eq 0 || "${#batches[@]}" -eq 0 || "${#shard_mults[@]}" -eq 0 ]]; then
    echo "error: --threads/--batches/--shard-mults must each include at least one value" >&2
    exit 1
fi

for t in "${threads[@]}"; do
    if ! [[ "$t" =~ ^[0-9]+$ ]] || [[ "$t" -eq 0 ]]; then
        echo "error: invalid --threads value: $t" >&2
        exit 1
    fi
done
for b in "${batches[@]}"; do
    if ! [[ "$b" =~ ^[0-9]+$ ]] || [[ "$b" -eq 0 ]]; then
        echo "error: invalid --batches value: $b" >&2
        exit 1
    fi
done
for m in "${shard_mults[@]}"; do
    if ! [[ "$m" =~ ^[0-9]+$ ]] || [[ "$m" -eq 0 ]]; then
        echo "error: invalid --shard-mults value: $m" >&2
        exit 1
    fi
done

build_dir="target/bench/build"
bin="$build_dir/allocator_stress"
mkdir -p "$build_dir"
mkdir -p "$(dirname "$csv_out")"

echo "building allocator stress benchmark..."
rustc -O -C target-cpu=native bench/allocator_stress.rs -o "$bin"

printf "threads,shards,batch,iters,size,drain_every,runs,ops_per_sec,remote_frees,remote_flushes,avg_remote_batch,remote_free_ratio,fresh_allocs,local_reuses,remote_reuses\n" >"$csv_out"

extract_metric() {
    local key=$1
    awk -v k="$key" '{
        for (i = 1; i <= NF; i++) {
            if ($i ~ ("^" k "=")) {
                split($i, a, "=");
                print a[2];
                exit;
            }
        }
    }'
}

printf "\n%-8s %-8s %-8s %-14s %-14s %-12s %-14s\n" "threads" "shards" "batch" "ops_per_sec" "remote_frees" "flushes" "avg_batch"
printf "%-8s %-8s %-8s %-14s %-14s %-12s %-14s\n" "--------" "--------" "--------" "--------------" "--------------" "------------" "--------------"

for t in "${threads[@]}"; do
    for mult in "${shard_mults[@]}"; do
        shards=$((t * mult))
        for b in "${batches[@]}"; do
            sum_ops="0"
            sum_remote_frees="0"
            sum_remote_flushes="0"
            sum_avg_batch="0"
            sum_remote_ratio="0"
            sum_fresh="0"
            sum_local_reuses="0"
            sum_remote_reuses="0"

            for _ in $(seq 1 "$runs"); do
                report=$("$bin" \
                    --threads "$t" \
                    --shards "$shards" \
                    --batch "$b" \
                    --iters "$iters" \
                    --size "$alloc_size" \
                    --drain-every "$drain_every")

                ops=$(extract_metric "ops_per_sec" <<<"$report")
                remote_frees=$(extract_metric "remote_frees" <<<"$report")
                remote_flushes=$(extract_metric "remote_flushes" <<<"$report")
                avg_batch=$(extract_metric "avg_remote_batch" <<<"$report")
                remote_ratio=$(extract_metric "remote_free_ratio" <<<"$report")
                fresh_allocs=$(extract_metric "fresh_allocs" <<<"$report")
                local_reuses=$(extract_metric "local_reuses" <<<"$report")
                remote_reuses=$(extract_metric "remote_reuses" <<<"$report")

                sum_ops=$(awk -v a="$sum_ops" -v b="$ops" 'BEGIN { printf "%.9f", a + b }')
                sum_remote_frees=$(awk -v a="$sum_remote_frees" -v b="$remote_frees" 'BEGIN { printf "%.9f", a + b }')
                sum_remote_flushes=$(awk -v a="$sum_remote_flushes" -v b="$remote_flushes" 'BEGIN { printf "%.9f", a + b }')
                sum_avg_batch=$(awk -v a="$sum_avg_batch" -v b="$avg_batch" 'BEGIN { printf "%.9f", a + b }')
                sum_remote_ratio=$(awk -v a="$sum_remote_ratio" -v b="$remote_ratio" 'BEGIN { printf "%.9f", a + b }')
                sum_fresh=$(awk -v a="$sum_fresh" -v b="$fresh_allocs" 'BEGIN { printf "%.9f", a + b }')
                sum_local_reuses=$(awk -v a="$sum_local_reuses" -v b="$local_reuses" 'BEGIN { printf "%.9f", a + b }')
                sum_remote_reuses=$(awk -v a="$sum_remote_reuses" -v b="$remote_reuses" 'BEGIN { printf "%.9f", a + b }')
            done

            avg_ops=$(awk -v s="$sum_ops" -v n="$runs" 'BEGIN { printf "%.3f", s / n }')
            avg_remote_frees=$(awk -v s="$sum_remote_frees" -v n="$runs" 'BEGIN { printf "%.0f", s / n }')
            avg_remote_flushes=$(awk -v s="$sum_remote_flushes" -v n="$runs" 'BEGIN { printf "%.0f", s / n }')
            avg_batch_val=$(awk -v s="$sum_avg_batch" -v n="$runs" 'BEGIN { printf "%.3f", s / n }')
            avg_remote_ratio=$(awk -v s="$sum_remote_ratio" -v n="$runs" 'BEGIN { printf "%.6f", s / n }')
            avg_fresh=$(awk -v s="$sum_fresh" -v n="$runs" 'BEGIN { printf "%.0f", s / n }')
            avg_local_reuses=$(awk -v s="$sum_local_reuses" -v n="$runs" 'BEGIN { printf "%.0f", s / n }')
            avg_remote_reuses=$(awk -v s="$sum_remote_reuses" -v n="$runs" 'BEGIN { printf "%.0f", s / n }')

            printf "%-8s %-8s %-8s %-14s %-14s %-12s %-14s\n" \
                "$t" "$shards" "$b" "$avg_ops" "$avg_remote_frees" "$avg_remote_flushes" "$avg_batch_val"

            printf "%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n" \
                "$t" "$shards" "$b" "$iters" "$alloc_size" "$drain_every" "$runs" \
                "$avg_ops" "$avg_remote_frees" "$avg_remote_flushes" "$avg_batch_val" \
                "$avg_remote_ratio" "$avg_fresh" "$avg_local_reuses" "$avg_remote_reuses" >>"$csv_out"
        done
    done
done

echo "csv_report=$csv_out"
