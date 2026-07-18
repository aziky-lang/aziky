#!/usr/bin/env bash
set -euo pipefail

run_wine=false
if [[ "${1:-}" == "--with-wine" ]]; then
    run_wine=true
elif [[ $# -ne 0 ]]; then
    echo "usage: $0 [--with-wine]" >&2
    exit 2
fi

tmp_dir="$(mktemp -d)"
wine_prefix="$PWD/target/portability-wine-prefix"
cleanup() {
    rm -rf "$tmp_dir" "$wine_prefix"
}
trap cleanup EXIT

echo "[1/7] build offline and inspect deterministic target capabilities"
cargo build -q --locked --offline
compiler="target/debug/aziky"
"$compiler" target list >"$tmp_dir/targets-first.txt"
"$compiler" target list >"$tmp_dir/targets-second.txt"
cmp -s "$tmp_dir/targets-first.txt" "$tmp_dir/targets-second.txt"
rg -q 'target=x86_64-unknown-linux-gnu.*codegen=accepted.*runtime=startup,allocation,files,clocks,process,threads,synchronization' "$tmp_dir/targets-first.txt"
rg -q 'target=x86_64-pc-windows-msvc.*codegen=accepted.*runtime=startup,allocation,files,clocks,process,threads,synchronization' "$tmp_dir/targets-first.txt"
rg -q 'target=x86_64-apple-darwin.*codegen=accepted.*runtime=startup,allocation,files,process' "$tmp_dir/targets-first.txt"
rg -q 'target=aarch64-unknown-linux-gnu.*codegen=planned.*runtime=none' "$tmp_dir/targets-first.txt"

echo "[2/7] preserve byte-identical default and explicit Linux output"
for run in default explicit repeated; do
    target_args=()
    if [[ "$run" != "default" ]]; then
        target_args=(--target x86_64-unknown-linux-gnu)
    fi
    "$compiler" compile examples/package_app/src/main.azk -o "$tmp_dir/$run" "${target_args[@]}" >/dev/null
done
cmp -s "$tmp_dir/default" "$tmp_dir/explicit"
cmp -s "$tmp_dir/explicit" "$tmp_dir/repeated"
set +e
"$tmp_dir/explicit"
linux_status=$?
set -e
[[ "$linux_status" -eq 82 ]]

echo "[3/7] emit deterministic native containers for all x86-64 targets"
for run in first second; do
    "$compiler" compile examples/portable_targets/minimal.azk --target macos-x86_64 -o "$tmp_dir/macos-$run" >/dev/null
    "$compiler" compile examples/portable_targets/minimal.azk --target windows-x86_64 -o "$tmp_dir/windows-$run.exe" >/dev/null
    "$compiler" compile examples/portable_targets/minimal.azk --target windows-x86_64 --emit object -o "$tmp_dir/windows-$run.obj" >/dev/null
    "$compiler" compile examples/portable_targets/minimal.azk --target windows-x86_64 --emit static-library -o "$tmp_dir/windows-$run.lib" >/dev/null
done
cmp -s "$tmp_dir/macos-first" "$tmp_dir/macos-second"
cmp -s "$tmp_dir/windows-first.exe" "$tmp_dir/windows-second.exe"
cmp -s "$tmp_dir/windows-first.obj" "$tmp_dir/windows-second.obj"
cmp -s "$tmp_dir/windows-first.lib" "$tmp_dir/windows-second.lib"
llvm-readobj --file-headers "$tmp_dir/macos-first" | rg -q 'FileType: Executable'
llvm-objdump --macho --private-headers "$tmp_dir/macos-first" | rg -q 'LC_UNIXTHREAD'
llvm-readobj --file-headers --coff-imports "$tmp_dir/windows-first.exe" >"$tmp_dir/pe.txt"
rg -q 'IMAGE_FILE_EXECUTABLE_IMAGE' "$tmp_dir/pe.txt"
for import in ExitProcess WriteFile VirtualAlloc CreateFileA CreateThread Sleep; do
    rg -q "Symbol: $import" "$tmp_dir/pe.txt"
done
llvm-readobj --file-headers --sections --symbols "$tmp_dir/windows-first.obj" >"$tmp_dir/coff.txt"
rg -q 'Format: COFF-x86-64' "$tmp_dir/coff.txt"
rg -q 'Name: .text' "$tmp_dir/coff.txt"
rg -q 'Name: aziky_program_entry' "$tmp_dir/coff.txt"

echo "[4/7] reject invalid formats, unavailable architectures, and missing capabilities"
if "$compiler" compile examples/portable_targets/minimal.azk --target linux-x86_64 --format macho64 -o "$tmp_dir/mismatch" >"$tmp_dir/mismatch.txt" 2>&1; then
    exit 2
fi
if "$compiler" compile examples/portable_targets/minimal.azk --target linux-aarch64 -o "$tmp_dir/aarch64" >"$tmp_dir/aarch64.txt" 2>&1; then
    exit 2
fi
if "$compiler" compile examples/thread_channel_app/main.azk --target macos-x86_64 -o "$tmp_dir/macos-thread" >"$tmp_dir/macos-thread.txt" 2>&1; then
    exit 2
fi
rg -q "requires format 'elf64'.*'macho64' is incompatible" "$tmp_dir/mismatch.txt"
rg -q "target 'aarch64-unknown-linux-gnu' is recognized but native code generation/runtime support is not accepted yet" "$tmp_dir/aarch64.txt"
rg -q "does not provide required runtime capabilities: synchronization, threads" "$tmp_dir/macos-thread.txt"
test ! -e "$tmp_dir/mismatch"
test ! -e "$tmp_dir/aarch64"
test ! -e "$tmp_dir/macos-thread"

echo "[5/7] execute the complete Linux runtime surface"
"$compiler" compile examples/platform_app/main.azk --target linux-x86_64 -o "$tmp_dir/linux-platform" >/dev/null
mkdir "$tmp_dir/linux-platform-run"
set +e
(cd "$tmp_dir/linux-platform-run" && "$tmp_dir/linux-platform")
linux_platform_status=$?
set -e
[[ "$linux_platform_status" -eq 28 ]]
rg -q '^Aziky platform$' "$tmp_dir/linux-platform-run/platform-output.txt"
"$compiler" compile examples/thread_channel_app/main.azk --target linux-x86_64 -o "$tmp_dir/linux-thread" >/dev/null
set +e
"$tmp_dir/linux-thread"
linux_thread_status=$?
set -e
[[ "$linux_thread_status" -eq 186 ]]

echo "[6/7] execute the complete Windows runtime surface when requested"
if $run_wine; then
    command -v wine >/dev/null
    mkdir -p "$wine_prefix" "$tmp_dir/windows-platform-run"
    "$compiler" compile examples/platform_app/main.azk --target windows-x86_64 -o "$tmp_dir/windows-platform.exe" >/dev/null
    "$compiler" compile examples/thread_channel_app/main.azk --target windows-x86_64 -o "$tmp_dir/windows-thread.exe" >/dev/null
    set +e
    WINEPREFIX="$wine_prefix" WINEDEBUG=-all wine "$tmp_dir/windows-first.exe"
    windows_minimal_status=$?
    (cd "$tmp_dir/windows-platform-run" && WINEPREFIX="$wine_prefix" WINEDEBUG=-all wine "$tmp_dir/windows-platform.exe")
    windows_platform_status=$?
    WINEPREFIX="$wine_prefix" WINEDEBUG=-all wine "$tmp_dir/windows-thread.exe"
    windows_thread_status=$?
    set -e
    [[ "$windows_minimal_status" -eq 82 ]]
    [[ "$windows_platform_status" -eq 28 ]]
    [[ "$windows_thread_status" -eq 186 ]]
    rg -q '^Aziky platform$' "$tmp_dir/windows-platform-run/platform-output.txt"
else
    echo "wine_runtime=SKIPPED hint=rerun_with_--with-wine"
fi

echo "[7/7] require full offline suite and clean diffs"
cargo test -q --locked --offline
git diff --check

echo "portability_gate=PASS linux=FULL windows=FULL macos=CORE aarch64=PLANNED"
