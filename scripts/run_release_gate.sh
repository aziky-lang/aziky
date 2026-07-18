#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "Usage: scripts/run_release_gate.sh [--allow-dirty]"
}

allow_dirty=false
if [[ $# -gt 1 ]]; then
    usage
    exit 1
fi
if [[ $# -eq 1 ]]; then
    if [[ "$1" != "--allow-dirty" ]]; then
        usage
        exit 1
    fi
    allow_dirty=true
fi

if [[ "$allow_dirty" == false ]] && git status --porcelain --untracked-files=normal | rg -q .; then
    echo "release_gate=FAILED reason=worktree_not_clean"
    git status --short
    exit 2
fi

tmp_dir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

echo "[1/10] build the compiler offline"
cargo build -q --locked --offline
compiler="target/debug/aziky"
cp -a examples/package_app "$tmp_dir/package_app"

echo "[2/10] verify the offline package graph and runtime-native composition"
"$compiler" package verify "$tmp_dir/package_app" >"$tmp_dir/package-verify.txt"
for source in \
    "$tmp_dir/package_app/src/main.azk" \
    examples/host_services_app/main.azk \
    examples/platform_app/main.azk \
    examples/thread_channel_app/main.azk; do
    AZIKY_RUNTIME_FALLBACK_REPORT=1 "$compiler" check "$source" \
        >"$tmp_dir/native.stdout" 2>"$tmp_dir/native.stderr"
    if rg -q 'runtime_generic_fallback' "$tmp_dir/native.stderr"; then
        echo "release_gate=FAILED reason=semantic_fallback source=$source"
        exit 2
    fi
done

echo "[3/10] require formatter, linter, and deterministic package tests"
"$compiler" fmt "$tmp_dir/package_app" --check >"$tmp_dir/fmt.txt"
"$compiler" lint "$tmp_dir/package_app" --check >"$tmp_dir/lint.txt"
"$compiler" test "$tmp_dir/package_app" >"$tmp_dir/tests-first.txt"
"$compiler" test "$tmp_dir/package_app" >"$tmp_dir/tests-second.txt"
cmp -s "$tmp_dir/tests-first.txt" "$tmp_dir/tests-second.txt"
rg -q '^test result: ok\. 2 passed; 0 failed$' "$tmp_dir/tests-first.txt"

echo "[4/10] require cross-root byte-identical executables and libraries"
for kind in executable object static-library shared-library; do
    "$compiler" compile examples/package_app/src/main.azk \
        --emit "$kind" -o "$tmp_dir/original-$kind" >/dev/null
    "$compiler" compile "$tmp_dir/package_app/src/main.azk" \
        --emit "$kind" -o "$tmp_dir/copied-$kind" >/dev/null
    "$compiler" compile "$tmp_dir/package_app/src/main.azk" \
        --emit "$kind" -o "$tmp_dir/repeated-$kind" >/dev/null
    cmp -s "$tmp_dir/original-$kind" "$tmp_dir/copied-$kind"
    cmp -s "$tmp_dir/copied-$kind" "$tmp_dir/repeated-$kind"
done
for kind in executable object static-library; do
    "$compiler" compile examples/package_app/src/main.azk --format macho64 \
        --emit "$kind" -o "$tmp_dir/original-macho-$kind" >/dev/null
    "$compiler" compile "$tmp_dir/package_app/src/main.azk" --format macho64 \
        --emit "$kind" -o "$tmp_dir/copied-macho-$kind" >/dev/null
    cmp -s "$tmp_dir/original-macho-$kind" "$tmp_dir/copied-macho-$kind"
done

echo "[5/10] execute package, host-service, and filesystem applications"
mkdir "$tmp_dir/package-run" "$tmp_dir/host-run" "$tmp_dir/platform-run"
set +e
(
    cd "$tmp_dir/package-run"
    "$tmp_dir/original-executable"
)
package_status=$?
set -e
if [[ "$package_status" -ne 82 ]]; then
    echo "release_gate=FAILED reason=package_status actual=$package_status expected=82"
    exit 2
fi
"$compiler" compile examples/host_services_app/main.azk \
    -o "$tmp_dir/host-services" >/dev/null
(
    cd "$tmp_dir/host-run"
    "$tmp_dir/host-services" release
)
"$compiler" compile examples/platform_app/main.azk \
    -o "$tmp_dir/platform-app" >/dev/null
set +e
(
    cd "$tmp_dir/platform-run"
    "$tmp_dir/platform-app"
)
platform_status=$?
set -e
if [[ "$platform_status" -ne 28 ]]; then
    echo "release_gate=FAILED reason=platform_status actual=$platform_status expected=28"
    exit 2
fi
rg -q '^Aziky platform$' "$tmp_dir/platform-run/platform-output.txt"

echo "[6/10] link and validate emitted library artifacts"
ld -o "$tmp_dir/object-linked" "$tmp_dir/original-object" -e _start
ld -o "$tmp_dir/static-linked" -e _start "$tmp_dir/original-static-library"
for linked in object-linked static-linked; do
    set +e
    "$tmp_dir/$linked"
    linked_status=$?
    set -e
    if [[ "$linked_status" -ne 82 ]]; then
        echo "release_gate=FAILED reason=${linked}_status actual=$linked_status expected=82"
        exit 2
    fi
done
readelf -h -S -s "$tmp_dir/original-object" >"$tmp_dir/object.txt"
rg -q 'Type:[[:space:]]+REL' "$tmp_dir/object.txt"
rg -q 'aziky_program_entry' "$tmp_dir/object.txt"
nm -s "$tmp_dir/original-static-library" >"$tmp_dir/archive.txt"
rg -q '_start in aziky\.o' "$tmp_dir/archive.txt"
readelf -h -d --dyn-syms "$tmp_dir/original-shared-library" >"$tmp_dir/shared.txt"
rg -q 'Type:[[:space:]]+DYN' "$tmp_dir/shared.txt"
rg -q 'aziky_program_entry' "$tmp_dir/shared.txt"
python3 -c 'import ctypes,sys; assert ctypes.CDLL(sys.argv[1]).aziky_program_entry' \
    "$tmp_dir/original-shared-library"
llvm-readobj --file-headers --sections --symbols "$tmp_dir/original-macho-object" \
    >"$tmp_dir/macho.txt"
rg -q 'FileType: Relocatable' "$tmp_dir/macho.txt"

echo "[7/10] run the native concurrency integration gate"
scripts/run_concurrency_gate.sh >"$tmp_dir/concurrency.txt"
rg -q '^concurrency_gate=PASS$' "$tmp_dir/concurrency.txt"

echo "[8/10] require stable negative and offline diagnostics"
mkdir "$tmp_dir/empty-cache"
for run in first second; do
    if "$compiler" package lock "$tmp_dir/package_app" \
        --package-cache "$tmp_dir/empty-cache" >"$tmp_dir/offline-$run.txt" 2>&1; then
        echo "release_gate=FAILED reason=offline_cache_failure_accepted"
        exit 2
    fi
    if "$compiler" check examples/foundation_app/invalid/main.azk \
        >"$tmp_dir/semantic-$run.txt" 2>&1; then
        echo "release_gate=FAILED reason=semantic_error_accepted"
        exit 2
    fi
    if "$compiler" compile examples/package_app/src/main.azk --format macho64 \
        --emit shared-library -o "$tmp_dir/rejected-$run.dylib" \
        >"$tmp_dir/target-$run.txt" 2>&1; then
        echo "release_gate=FAILED reason=unsupported_target_accepted"
        exit 2
    fi
done
cmp -s "$tmp_dir/offline-first.txt" "$tmp_dir/offline-second.txt"
cmp -s "$tmp_dir/semantic-first.txt" "$tmp_dir/semantic-second.txt"
cmp -s "$tmp_dir/target-first.txt" "$tmp_dir/target-second.txt"
rg -q 'never fetches dependencies implicitly' "$tmp_dir/offline-first.txt"
rg -q 'type mismatch: expected u64, got bool' "$tmp_dir/semantic-first.txt"
rg -q 'not supported for macho64' "$tmp_dir/target-first.txt"
test ! -e "$tmp_dir/rejected-first.dylib"
test ! -e "$tmp_dir/rejected-second.dylib"

echo "[9/10] require the complete compiler suite offline"
cargo test -q --locked --offline

echo "[10/10] require documented limitations and clean diffs"
for limitation in \
    'Linux x86-64' \
    'Mach-O64' \
    'PE/COFF' \
    'AArch64' \
    'Child-process spawning' \
    'Benchmark discovery'; do
    rg -q "$limitation" docs/RELEASE_STATUS.md
done
git diff --check

echo "release_gate=PASS"
