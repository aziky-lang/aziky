#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<USAGE
Usage:
  scripts/differential_encoder_check.sh

Checks backend encoder consistency by comparing:
  1) `emit` mode output bytes
  2) `compile` output bytes for equivalent Aziky source
for canonical write/exit and exit-only programs.
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi

cargo build -q --locked --offline

tmp_dir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

extract_code() {
    local elf="$1"
    # ELF code starts at fixed 0x1000 offset in this compiler.
    tail -c +4097 "$elf"
}

# Case 1: exit-only
target/debug/aziky emit "$tmp_dir/emit_exit.bin" --exit-only >/dev/null
cat > "$tmp_dir/exit_only.azk" <<'EOF'
fn main() {
    let s = "0";
    let n: i32 = s.to_i32();
    exit(n);
}
EOF
target/debug/aziky compile "$tmp_dir/exit_only.azk" -o "$tmp_dir/compile_exit.bin" >/dev/null

if ! cmp -s <(extract_code "$tmp_dir/emit_exit.bin") <(extract_code "$tmp_dir/compile_exit.bin"); then
    echo "differential_encoder=FAILED case=exit_only" >&2
    exit 2
fi

# Case 2: write + exit
message="Hello from differential check\n"
target/debug/aziky emit "$tmp_dir/emit_write.bin" --message "$message" >/dev/null
cat > "$tmp_dir/write_exit.azk" <<'EOF'
fn main() {
    let msg = "Hello from differential check\n";
    print(msg);
    exit(0);
}
EOF
target/debug/aziky compile "$tmp_dir/write_exit.azk" -o "$tmp_dir/compile_write.bin" >/dev/null

if ! cmp -s <(extract_code "$tmp_dir/emit_write.bin") <(extract_code "$tmp_dir/compile_write.bin"); then
    echo "differential_encoder=FAILED case=write_exit" >&2
    exit 3
fi

echo "differential_encoder=PASS"
