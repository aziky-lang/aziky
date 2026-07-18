#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "macos_x86_64_gate=SKIPPED reason=requires_macos" >&2
    exit 2
fi

case "$(uname -m)" in
    x86_64)
        run_x86_64() { "$@"; }
        ;;
    arm64)
        if ! arch -x86_64 /usr/bin/true; then
            echo "macos_x86_64_gate=SKIPPED reason=requires_rosetta2" >&2
            exit 2
        fi
        run_x86_64() { arch -x86_64 "$@"; }
        ;;
    *)
        echo "macos_x86_64_gate=SKIPPED reason=unsupported_host_arch" >&2
        exit 2
        ;;
esac

tmp_dir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

echo "[1/5] build the compiler from the locked offline source tree"
cargo build -q --locked --offline
compiler="target/debug/aziky"

echo "[2/5] require deterministic direct-entry Mach-O output"
for run in first second; do
    "$compiler" compile examples/portable_targets/minimal.azk \
        --target x86_64-apple-darwin -o "$tmp_dir/minimal-$run" >/dev/null
done
cmp -s "$tmp_dir/minimal-first" "$tmp_dir/minimal-second"
file "$tmp_dir/minimal-first" | grep -q 'Mach-O 64-bit executable x86_64'
otool -l "$tmp_dir/minimal-first" >"$tmp_dir/load-commands.txt"
grep -q 'LC_UNIXTHREAD' "$tmp_dir/load-commands.txt"

echo "[3/5] execute startup, output, and process identity"
codesign --force --sign - "$tmp_dir/minimal-first"
set +e
run_x86_64 "$tmp_dir/minimal-first" >"$tmp_dir/minimal-output.txt"
minimal_status=$?
set -e
[[ "$minimal_status" -eq 82 ]]
grep -q '^Aziky portable target$' "$tmp_dir/minimal-output.txt"

"$compiler" compile examples/package_app/src/main.azk \
    --target x86_64-apple-darwin -o "$tmp_dir/package-app" >/dev/null
codesign --force --sign - "$tmp_dir/package-app"
set +e
run_x86_64 "$tmp_dir/package-app"
package_status=$?
set -e
[[ "$package_status" -eq 82 ]]

echo "[4/5] execute allocation and owned file services"
"$compiler" compile examples/platform_app/main.azk \
    --target x86_64-apple-darwin -o "$tmp_dir/platform-app" >/dev/null
codesign --force --sign - "$tmp_dir/platform-app"
mkdir "$tmp_dir/platform-run"
set +e
(cd "$tmp_dir/platform-run" && run_x86_64 "$tmp_dir/platform-app")
platform_status=$?
set -e
[[ "$platform_status" -eq 28 ]]
grep -q '^Aziky platform$' "$tmp_dir/platform-run/platform-output.txt"

echo "[5/5] keep incomplete clocks and threading capability-gated"
if "$compiler" compile examples/host_services_app/main.azk \
    --target x86_64-apple-darwin -o "$tmp_dir/host-services" \
    >"$tmp_dir/host-services.txt" 2>&1; then
    echo "macos_x86_64_gate=FAILED reason=unavailable_clocks_accepted" >&2
    exit 2
fi
if "$compiler" compile examples/thread_channel_app/main.azk \
    --target x86_64-apple-darwin -o "$tmp_dir/thread-channel" \
    >"$tmp_dir/thread-channel.txt" 2>&1; then
    echo "macos_x86_64_gate=FAILED reason=unavailable_threads_accepted" >&2
    exit 2
fi
grep -q 'required runtime capabilities: clocks' "$tmp_dir/host-services.txt"
grep -q 'required runtime capabilities: synchronization, threads' "$tmp_dir/thread-channel.txt"
test ! -e "$tmp_dir/host-services"
test ! -e "$tmp_dir/thread-channel"

echo "macos_x86_64_gate=PASS runtime=CORE clocks=GATED threads=GATED"
