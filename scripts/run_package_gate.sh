#!/usr/bin/env bash
set -euo pipefail

tmp_dir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

echo "[1/7] build compiler without network dependencies"
cargo build -q --locked --offline
compiler="target/debug/aziky"

echo "[2/7] verify checked-in lock and deterministic regeneration"
cp -a examples/package_app "$tmp_dir/app"
"$compiler" package verify "$tmp_dir/app" >"$tmp_dir/verify.txt"
cp "$tmp_dir/app/Aziky.lock" "$tmp_dir/original.lock"
"$compiler" package lock "$tmp_dir/app" >/dev/null
cmp -s "$tmp_dir/original.lock" "$tmp_dir/app/Aziky.lock"
"$compiler" package lock "$tmp_dir/app" >/dev/null
cmp -s "$tmp_dir/original.lock" "$tmp_dir/app/Aziky.lock"

echo "[3/7] require byte-identical multi-package artifacts"
"$compiler" compile "$tmp_dir/app/src/main.azk" -o "$tmp_dir/first.bin" >/dev/null
"$compiler" compile "$tmp_dir/app/src/main.azk" -o "$tmp_dir/second.bin" >/dev/null
cmp -s "$tmp_dir/first.bin" "$tmp_dir/second.bin"
"$compiler" compile examples/package_app/src/main.azk -o "$tmp_dir/original-root.bin" >/dev/null
cmp -s "$tmp_dir/first.bin" "$tmp_dir/original-root.bin"
"$compiler" compile "$tmp_dir/app/src/main.azk" -o "$tmp_dir/first.macho" --format macho64 >/dev/null
"$compiler" compile "$tmp_dir/app/src/main.azk" -o "$tmp_dir/second.macho" --format macho64 >/dev/null
cmp -s "$tmp_dir/first.macho" "$tmp_dir/second.macho"

echo "[4/7] execute the resolved transitive/default-feature graph"
mkdir "$tmp_dir/run"
set +e
(
    cd "$tmp_dir/run"
    "$tmp_dir/first.bin"
)
status=$?
set -e
if [[ "$status" -ne 82 ]]; then
    echo "package_gate=FAILED reason=package_execution actual=$status expected=82"
    exit 2
fi

echo "[5/7] verify feature selection and offline cache failure"
for run in first second; do
    if "$compiler" check "$tmp_dir/app/src/main.azk" --no-default-features >"$tmp_dir/feature_$run.txt" 2>&1; then
        echo "package_gate=FAILED reason=disabled_optional_dependency_accepted"
        exit 2
    fi
done
cmp -s "$tmp_dir/feature_first.txt" "$tmp_dir/feature_second.txt"
rg -q "lockfile .* is stale" "$tmp_dir/feature_first.txt"
mkdir "$tmp_dir/empty-cache"
if "$compiler" package lock "$tmp_dir/app" --package-cache "$tmp_dir/empty-cache" >"$tmp_dir/offline.txt" 2>&1; then
    echo "package_gate=FAILED reason=missing_cache_accepted"
    exit 2
fi
rg -q "never fetches dependencies implicitly" "$tmp_dir/offline.txt"

echo "[6/7] verify content tampering and stable structural diagnostics"
cp -a examples/package_app "$tmp_dir/tampered"
printf '\n' >>"$tmp_dir/tampered/.aziky/cache/base/1.0.0/src/lib.azk"
if "$compiler" package verify "$tmp_dir/tampered" >"$tmp_dir/tampered.txt" 2>&1; then
    echo "package_gate=FAILED reason=tampered_cache_accepted"
    exit 2
fi
rg -q "checksum mismatch for cached package 'base@1.0.0'" "$tmp_dir/tampered.txt"
for fixture in conflict cycle; do
    for run in first second; do
        if "$compiler" package lock "examples/package_app/invalid/$fixture" >"$tmp_dir/${fixture}_$run.txt" 2>&1; then
            echo "package_gate=FAILED reason=${fixture}_accepted"
            exit 2
        fi
    done
    cmp -s "$tmp_dir/${fixture}_first.txt" "$tmp_dir/${fixture}_second.txt"
done
rg -q "package version conflict" "$tmp_dir/conflict_first.txt"
rg -q "package dependency cycle detected" "$tmp_dir/cycle_first.txt"

echo "[7/7] require full compiler tests"
cargo test -q --locked --offline

echo "package_gate=PASS"
