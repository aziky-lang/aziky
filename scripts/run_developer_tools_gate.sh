#!/usr/bin/env bash
set -euo pipefail

tmp_dir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

echo "[1/7] build compiler"
cargo build -q --locked --offline
compiler="target/debug/aziky"

echo "[2/7] verify deterministic discovery, filtering, and package tests"
"$compiler" test examples/package_app --list >"$tmp_dir/list-first.txt"
"$compiler" test examples/package_app --list >"$tmp_dir/list-second.txt"
cmp -s "$tmp_dir/list-first.txt" "$tmp_dir/list-second.txt"
sed -n '1p' "$tmp_dir/list-first.txt" | rg -q '^01_math\.azk$'
sed -n '2p' "$tmp_dir/list-first.txt" | rg -q '^02_base\.azk$'
rg -q '^tests=2$' "$tmp_dir/list-first.txt"
"$compiler" test examples/package_app --filter math >"$tmp_dir/filter.txt"
rg -q '^test 01_math\.azk \.\.\. ok$' "$tmp_dir/filter.txt"
rg -q '^test result: ok\. 1 passed; 0 failed$' "$tmp_dir/filter.txt"
"$compiler" test examples/package_app >"$tmp_dir/package-tests.txt"
rg -q '^test result: ok\. 2 passed; 0 failed$' "$tmp_dir/package-tests.txt"

echo "[3/7] verify stable failure, compile-error, and timeout reporting"
for fixture in failing compile_error; do
    for run in first second; do
        if "$compiler" test "examples/developer_commands/invalid/$fixture.azk" \
            >"$tmp_dir/${fixture}_$run.txt" 2>&1; then
            echo "developer_tools_gate=FAILED reason=${fixture}_accepted"
            exit 2
        fi
    done
    cmp -s "$tmp_dir/${fixture}_first.txt" "$tmp_dir/${fixture}_second.txt"
done
rg -q '^test failing\.azk \.\.\. FAILED \(exit code 7\)$' "$tmp_dir/failing_first.txt"
rg -q '^test compile_error\.azk \.\.\. FAILED \(compile\)$' "$tmp_dir/compile_error_first.txt"
if "$compiler" test examples/developer_commands/invalid/timeout.azk --timeout-ms 25 \
    >"$tmp_dir/timeout.txt" 2>&1; then
    echo "developer_tools_gate=FAILED reason=timeout_accepted"
    exit 2
fi
rg -q 'FAILED \(timed out after 25 ms\)' "$tmp_dir/timeout.txt"

echo "[4/7] verify formatter check, write, stdout, and idempotence"
cp examples/developer_commands/invalid/unformatted.azk "$tmp_dir/unformatted.azk"
if "$compiler" fmt "$tmp_dir/unformatted.azk" --check >"$tmp_dir/fmt-check.txt" 2>&1; then
    echo "developer_tools_gate=FAILED reason=unformatted_source_accepted"
    exit 2
fi
rg -q '^would reformat:' "$tmp_dir/fmt-check.txt"
"$compiler" fmt "$tmp_dir/unformatted.azk" >/dev/null
cp "$tmp_dir/unformatted.azk" "$tmp_dir/formatted-once.azk"
"$compiler" fmt "$tmp_dir/unformatted.azk" >/dev/null
cmp -s "$tmp_dir/formatted-once.azk" "$tmp_dir/unformatted.azk"
"$compiler" fmt "$tmp_dir/unformatted.azk" --check >"$tmp_dir/fmt-pass.txt"
rg -q 'format=PASS files=1 changed=0 mode=check' "$tmp_dir/fmt-pass.txt"
"$compiler" fmt "$tmp_dir/unformatted.azk" --stdout >"$tmp_dir/stdout.azk"
cmp -s "$tmp_dir/unformatted.azk" "$tmp_dir/stdout.azk"

echo "[5/7] verify stable lint diagnostics and CI mode"
for run in first second; do
    if "$compiler" lint examples/developer_commands/invalid/lint_names.azk --check \
        >"$tmp_dir/lint-$run.txt" 2>&1; then
        echo "developer_tools_gate=FAILED reason=lint_warning_accepted"
        exit 2
    fi
done
cmp -s "$tmp_dir/lint-first.txt" "$tmp_dir/lint-second.txt"
rg -q 'warning\[AZK-L100\]' "$tmp_dir/lint-first.txt"
rg -q 'warning\[AZK-L101\]' "$tmp_dir/lint-first.txt"
"$compiler" lint examples/developer_commands/tests --check >"$tmp_dir/lint-pass.txt"
rg -q 'lint=PASS files=2 warnings=0' "$tmp_dir/lint-pass.txt"

echo "[6/7] require benchmark tooling to remain explicitly documented"
rg -q 'First-class benchmark discovery and execution are intentionally deferred' \
    docs/AZIKY_DEVELOPER_COMMANDS.md

echo "[7/7] require full compiler tests and clean diffs"
cargo test -q --locked --offline
git diff --check

echo "developer_tools_gate=PASS bench=DEFERRED"
