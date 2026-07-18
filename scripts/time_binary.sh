#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<USAGE
Usage:
  scripts/time_binary.sh <binary> [--runs N] [--warmup N] [--label NAME] [--keep-output]
                         [--cpu-set LIST] [--rt-prio N]
                         [--perf-counters] [--perf-runs N]
                         [--perf-events comma,separated,list]
                         [--score-stat mean|median|trimmed] [--trim-pct N]
                         [-- arg1 arg2 ...]

Notes:
  - By default, stdout/stderr are redirected to /dev/null during timed runs.
  - Use --keep-output to benchmark with output enabled.
  - Use --cpu-set/--rt-prio to reduce scheduler noise when supported.
USAGE
}

if [[ $# -lt 1 ]]; then
    usage
    exit 1
fi

binary=$1
shift

runs=30
warmup=5
label="$(basename "$binary")"
redirect_output=1
cpu_set="${AZIKY_BENCH_CPU_SET:-}"
rt_prio="${AZIKY_BENCH_RT_PRIO:-}"
perf_counters="${AZIKY_BENCH_PERF_COUNTERS:-0}"
perf_runs="${AZIKY_BENCH_PERF_RUNS:-3}"
perf_events="${AZIKY_BENCH_PERF_EVENTS:-cycles,instructions,branches,branch-misses,cache-misses}"
score_stat="${AZIKY_BENCH_SCORE_STAT:-median}"
trim_pct="${AZIKY_BENCH_TRIM_PCT:-10}"
args=()

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
        --label)
            label=$2
            shift 2
            ;;
        --keep-output)
            redirect_output=0
            shift
            ;;
        --cpu-set)
            cpu_set=$2
            shift 2
            ;;
        --rt-prio)
            rt_prio=$2
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
        --score-stat)
            score_stat=$2
            shift 2
            ;;
        --trim-pct)
            trim_pct=$2
            shift 2
            ;;
        --)
            shift
            args=("$@")
            break
            ;;
        *)
            args+=("$1")
            shift
            ;;
    esac
done

if [[ ! -x "$binary" ]]; then
    echo "error: binary is not executable: $binary" >&2
    exit 1
fi

if ! [[ "$runs" =~ ^[0-9]+$ ]] || ! [[ "$warmup" =~ ^[0-9]+$ ]]; then
    echo "error: --runs and --warmup must be non-negative integers" >&2
    exit 1
fi
if ! [[ "$trim_pct" =~ ^[0-9]+$ ]]; then
    echo "error: --trim-pct must be a non-negative integer" >&2
    exit 1
fi
if [[ "$trim_pct" -gt 45 ]]; then
    echo "error: --trim-pct must be <= 45" >&2
    exit 1
fi
if [[ -n "$rt_prio" ]] && ! [[ "$rt_prio" =~ ^[0-9]+$ ]]; then
    echo "error: --rt-prio must be a non-negative integer" >&2
    exit 1
fi
if ! [[ "$perf_runs" =~ ^[0-9]+$ ]] || [[ "$perf_runs" -lt 0 ]]; then
    echo "error: --perf-runs must be a non-negative integer" >&2
    exit 1
fi
case "$score_stat" in
    mean|median|trimmed) ;;
    *)
        echo "error: --score-stat must be one of mean|median|trimmed" >&2
        exit 1
        ;;
esac

runner_prefix=()
if [[ -n "$cpu_set" ]]; then
    if ! command -v taskset >/dev/null 2>&1; then
        echo "error: taskset not available but --cpu-set was provided" >&2
        exit 1
    fi
    runner_prefix+=(taskset -c "$cpu_set")
fi
if [[ -n "$rt_prio" ]]; then
    if ! command -v chrt >/dev/null 2>&1; then
        echo "error: chrt not available but --rt-prio was provided" >&2
        exit 1
    fi
    runner_prefix+=(chrt -f "$rt_prio")
fi
if [[ "$perf_counters" -eq 1 ]] && ! command -v perf >/dev/null 2>&1; then
    echo "error: perf not available but --perf-counters was requested" >&2
    exit 1
fi

now_ns() {
    date +%s%N
}

run_cmd_once() {
    if [[ "$redirect_output" -eq 1 ]]; then
        "${runner_prefix[@]}" "$binary" "${args[@]}" >/dev/null 2>&1
    else
        "${runner_prefix[@]}" "$binary" "${args[@]}"
    fi
}

run_once() {
    local start end rc
    start=$(now_ns)
    set +e
    run_cmd_once
    rc=$?
    set -e
    end=$(now_ns)
    run_rc=$rc
    echo $((end - start))
}

perf_stat_once() {
    local rc stats
    set +e
    stats=$({
        if [[ "$redirect_output" -eq 1 ]]; then
            perf stat -x, -e "$perf_events" "${runner_prefix[@]}" "$binary" "${args[@]}" >/dev/null
        else
            perf stat -x, -e "$perf_events" "${runner_prefix[@]}" "$binary" "${args[@]}"
        fi
    } 2>&1)
    rc=$?
    set -e
    if [[ "$rc" -ne 0 ]]; then
        echo "error: perf stat failed (label=$label exit_code=$rc binary=$binary)" >&2
        echo "$stats" >&2
        exit 1
    fi
    printf '%s\n' "$stats"
}

parse_perf_value() {
    local event=$1
    local stats=$2
    awk -F, -v event="$event" '
        $3 == event {
            gsub(/[[:space:]]/, "", $1)
            if ($1 ~ /^</ || $1 == "") {
                print 0
            } else {
                gsub(/,/, "", $1)
                print $1
            }
            found = 1
            exit
        }
        END {
            if (!found) print 0
        }
    ' <<<"$stats"
}

probe_crash_once() {
    local rc diag
    set +e
    if [[ "$redirect_output" -eq 1 ]]; then
        diag=$(bash -c '"$@" >/dev/null 2>&1' _ "${runner_prefix[@]}" "$binary" "${args[@]}" 2>&1)
    else
        diag=$(bash -c '"$@"' _ "${runner_prefix[@]}" "$binary" "${args[@]}" 2>&1)
    fi
    rc=$?
    set -e
    case "$diag" in
        *"Segmentation fault"*|*"Bus error"*|*"Illegal instruction"*|*"Floating point exception"*|*"Aborted"*|*"Trace/breakpoint trap"*|*"Killed"*)
            echo "error: benchmark run crashed during preflight (label=$label exit_code=$rc binary=$binary)" >&2
            echo "error: signal_detail=$diag" >&2
            exit 1
            ;;
    esac
}

run_rc=0
first_rc=""

maybe_probe_signal_exit() {
    case "$1" in
        132|133|134|135|136|137|139)
            probe_crash_once
            ;;
    esac
}

probe_crash_once

for ((i = 0; i < warmup; i++)); do
    run_once >/dev/null
    maybe_probe_signal_exit "$run_rc"
    if [[ -z "$first_rc" ]]; then
        first_rc=$run_rc
    fi
done

sum=0
min=0
max=0
samples=()

for ((i = 0; i < runs; i++)); do
    elapsed_ns=$(run_once)
    maybe_probe_signal_exit "$run_rc"
    if [[ -z "$first_rc" ]]; then
        first_rc=$run_rc
    fi
    sum=$((sum + elapsed_ns))
    if [[ $i -eq 0 || elapsed_ns -lt min ]]; then
        min=$elapsed_ns
    fi
    if [[ $i -eq 0 || elapsed_ns -gt max ]]; then
        max=$elapsed_ns
    fi
    samples+=("$elapsed_ns")
done

median_from_sorted() {
    local n=$1
    awk -v n="$n" '
        { a[NR]=$1 }
        END {
            if (n == 0) {
                print 0
            } else if (n % 2 == 1) {
                print a[(n + 1) / 2]
            } else {
                i = n / 2
                print int((a[i] + a[i + 1]) / 2)
            }
        }
    '
}

if [[ "$runs" -eq 0 ]]; then
    mean=0
    median=0
    p90=0
    trimmed_mean=0
    stddev=0
    mad=0
else
    mean=$((sum / runs))
    sorted=$(printf '%s\n' "${samples[@]}" | sort -n)
    median=$(median_from_sorted "$runs" <<<"$sorted")
    p90_index=$(( (runs * 9 + 9) / 10 ))
    p90=$(awk -v i="$p90_index" 'NR == i { print $1; exit }' <<<"$sorted")

    trim_count=$(( runs * trim_pct / 100 ))
    if (( trim_count * 2 >= runs )); then
        trim_count=0
    fi
    trimmed_mean=$(awk -v n="$runs" -v t="$trim_count" '
        { a[NR]=$1 }
        END {
            if (n == 0) {
                print 0
                exit
            }
            s = t + 1
            e = n - t
            if (s > e) {
                print 0
                exit
            }
            sum = 0
            c = 0
            for (i = s; i <= e; i++) {
                sum += a[i]
                c += 1
            }
            if (c == 0) {
                print 0
            } else {
                print int(sum / c)
            }
        }
    ' <<<"$sorted")

    stddev=$(awk -v n="$runs" '
        { s += $1; ss += ($1 * $1) }
        END {
            if (n == 0) {
                print 0
                exit
            }
            m = s / n
            v = (ss / n) - (m * m)
            if (v < 0) v = 0
            print int(sqrt(v))
        }
    ' <<<"$sorted")

    abs_dev_sorted=$(awk -v med="$median" '
        {
            d = $1 - med
            if (d < 0) d = -d
            print d
        }
    ' <<<"$sorted" | sort -n)
    mad=$(median_from_sorted "$runs" <<<"$abs_dev_sorted")
fi

perf_cycles=0
perf_instructions=0
perf_branches=0
perf_branch_misses=0
perf_cache_misses=0
perf_ipc=0
perf_branch_miss_pct=0
perf_cache_misses_per_kinst=0
if [[ "$perf_counters" -eq 1 && "$perf_runs" -gt 0 ]]; then
    sum_perf_cycles=0
    sum_perf_instructions=0
    sum_perf_branches=0
    sum_perf_branch_misses=0
    sum_perf_cache_misses=0
    for ((i = 0; i < perf_runs; i++)); do
        perf_stats=$(perf_stat_once)
        sum_perf_cycles=$((sum_perf_cycles + $(parse_perf_value "cycles" "$perf_stats")))
        sum_perf_instructions=$((sum_perf_instructions + $(parse_perf_value "instructions" "$perf_stats")))
        sum_perf_branches=$((sum_perf_branches + $(parse_perf_value "branches" "$perf_stats")))
        sum_perf_branch_misses=$((sum_perf_branch_misses + $(parse_perf_value "branch-misses" "$perf_stats")))
        sum_perf_cache_misses=$((sum_perf_cache_misses + $(parse_perf_value "cache-misses" "$perf_stats")))
    done
    perf_cycles=$((sum_perf_cycles / perf_runs))
    perf_instructions=$((sum_perf_instructions / perf_runs))
    perf_branches=$((sum_perf_branches / perf_runs))
    perf_branch_misses=$((sum_perf_branch_misses / perf_runs))
    perf_cache_misses=$((sum_perf_cache_misses / perf_runs))
    perf_ipc=$(awk -v instructions="$perf_instructions" -v cycles="$perf_cycles" \
        'BEGIN { if (cycles == 0) print "0.000000"; else printf "%.6f", instructions / cycles }')
    perf_branch_miss_pct=$(awk -v misses="$perf_branch_misses" -v branches="$perf_branches" \
        'BEGIN { if (branches == 0) print "0.000000"; else printf "%.6f", 100.0 * misses / branches }')
    perf_cache_misses_per_kinst=$(awk -v misses="$perf_cache_misses" -v instructions="$perf_instructions" \
        'BEGIN { if (instructions == 0) print "0.000000"; else printf "%.6f", 1000.0 * misses / instructions }')
fi

case "$score_stat" in
    mean) score_ns=$mean ;;
    median) score_ns=$median ;;
    trimmed) score_ns=$trimmed_mean ;;
    *) score_ns=$median ;;
esac

fmt_ms() {
    awk -v ns="$1" 'BEGIN { printf "%.6f", ns / 1000000.0 }'
}

printf 'benchmark=%s\n' "$label"
printf 'binary=%s\n' "$binary"
printf 'exit_code=%s\n' "$first_rc"
printf 'runs=%s warmup=%s\n' "$runs" "$warmup"
printf 'cpu_set=%s\n' "${cpu_set:-none}"
printf 'rt_prio=%s\n' "${rt_prio:-none}"
printf 'score_stat=%s\n' "$score_stat"
printf 'score_ns=%s score_ms=%s\n' "$score_ns" "$(fmt_ms "$score_ns")"
printf 'min_ns=%s min_ms=%s\n' "$min" "$(fmt_ms "$min")"
printf 'mean_ns=%s mean_ms=%s\n' "$mean" "$(fmt_ms "$mean")"
printf 'trimmed_mean_ns=%s trimmed_mean_ms=%s\n' "$trimmed_mean" "$(fmt_ms "$trimmed_mean")"
printf 'median_ns=%s median_ms=%s\n' "$median" "$(fmt_ms "$median")"
printf 'mad_ns=%s mad_ms=%s\n' "$mad" "$(fmt_ms "$mad")"
printf 'stddev_ns=%s stddev_ms=%s\n' "$stddev" "$(fmt_ms "$stddev")"
printf 'p90_ns=%s p90_ms=%s\n' "$p90" "$(fmt_ms "$p90")"
printf 'max_ns=%s max_ms=%s\n' "$max" "$(fmt_ms "$max")"
printf 'perf_counters=%s perf_runs=%s\n' "$perf_counters" "$perf_runs"
printf 'perf_events=%s\n' "$perf_events"
printf 'perf_cycles=%s\n' "$perf_cycles"
printf 'perf_instructions=%s\n' "$perf_instructions"
printf 'perf_branches=%s\n' "$perf_branches"
printf 'perf_branch_misses=%s\n' "$perf_branch_misses"
printf 'perf_cache_misses=%s\n' "$perf_cache_misses"
printf 'perf_ipc=%s\n' "$perf_ipc"
printf 'perf_branch_miss_pct=%s\n' "$perf_branch_miss_pct"
printf 'perf_cache_misses_per_kinst=%s\n' "$perf_cache_misses_per_kinst"
