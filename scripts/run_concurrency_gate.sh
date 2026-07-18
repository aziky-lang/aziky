#!/usr/bin/env bash
set -euo pipefail

tmp_dir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

echo "[1/5] build compiler"
cargo build -q --locked --offline
compiler="target/debug/aziky"

echo "[2/5] require native concurrency lowering"
AZIKY_RUNTIME_FALLBACK_REPORT=1 "$compiler" check examples/thread_channel_app/main.azk \
    >"$tmp_dir/check.stdout" 2>"$tmp_dir/check.stderr"
if rg -q "runtime_generic_fallback" "$tmp_dir/check.stderr"; then
    echo "concurrency_gate=FAILED reason=semantic_fallback"
    exit 2
fi
rg -q "check=PASS" "$tmp_dir/check.stdout"

echo "[3/5] require byte-identical native output and FIFO stress result"
"$compiler" compile examples/thread_channel_app/main.azk -o "$tmp_dir/first.bin" >/dev/null
"$compiler" compile examples/thread_channel_app/main.azk -o "$tmp_dir/second.bin" >/dev/null
cmp -s "$tmp_dir/first.bin" "$tmp_dir/second.bin"
set +e
"$tmp_dir/first.bin"
status=$?
set -e
if [[ "$status" -ne 186 ]]; then
    echo "concurrency_gate=FAILED reason=fifo_status actual=$status expected=186"
    exit 2
fi

echo "[4/5] verify unbounded and worker-failure contracts"
for fixture in unbounded failure closed_peer shutdown; do
    "$compiler" compile "examples/thread_channel_app/$fixture.azk" \
        -o "$tmp_dir/$fixture.bin" >/dev/null
done
set +e
"$tmp_dir/unbounded.bin"
unbounded_status=$?
"$tmp_dir/failure.bin"
failure_status=$?
"$tmp_dir/closed_peer.bin"
closed_peer_status=$?
timeout 5s "$tmp_dir/shutdown.bin"
shutdown_status=$?
set -e
if [[ "$unbounded_status" -ne 29 || "$failure_status" -ne 101 || "$closed_peer_status" -ne 111 || "$shutdown_status" -ne 0 ]]; then
    echo "concurrency_gate=FAILED reason=contract_status unbounded=$unbounded_status failure=$failure_status closed_peer=$closed_peer_status shutdown=$shutdown_status"
    exit 2
fi

echo "[5/5] verify stable linearity diagnostic"
for run in first second; do
    if "$compiler" check examples/thread_channel_app/invalid_duplicate_sender.azk \
        >"$tmp_dir/invalid_$run.txt" 2>&1; then
        echo "concurrency_gate=FAILED reason=invalid_fixture_accepted"
        exit 2
    fi
done
cmp -s "$tmp_dir/invalid_first.txt" "$tmp_dir/invalid_second.txt"
rg -q "each channel endpoint can be extracted exactly once" "$tmp_dir/invalid_first.txt"

echo "concurrency_gate=PASS"
