#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<USAGE
Usage:
  scripts/check_reproducible_build.sh <input.azk>

Builds the compiler independently in two clean Cargo target directories, then
verifies that repeated ELF64 and Mach-O64 compilations are byte-identical both
within and across the two compiler builds.
USAGE
}

if [[ $# -ne 1 ]]; then
    usage
    exit 1
fi

input="$1"
if [[ ! -f "$input" ]]; then
    echo "error: input not found: $input" >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    hash_cmd=(sha256sum)
elif command -v shasum >/dev/null 2>&1; then
    hash_cmd=(shasum -a 256)
else
    echo "error: sha256 tool not found (need sha256sum or shasum)" >&2
    exit 1
fi

tmp_dir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

declare -a artifacts=()
for build in a b; do
    target_dir="$tmp_dir/target_$build"
    CARGO_INCREMENTAL=0 RUSTFLAGS="${RUSTFLAGS:-} -Awarnings" \
        cargo build -q --locked --offline --target-dir "$target_dir"
    compiler="$target_dir/debug/aziky"
    for format in elf64 macho64; do
        for run in 1 2; do
            output="$tmp_dir/${build}_${format}_${run}.bin"
            "$compiler" compile "$input" --format "$format" -o "$output" >/dev/null
            artifacts+=("$output")
        done
    done
done

for artifact in "${artifacts[@]}"; do
    format="elf64"
    if [[ "$artifact" == *macho64* ]]; then
        format="macho64"
    fi
    format_reference="$tmp_dir/a_${format}_1.bin"
    format_hash="$("${hash_cmd[@]}" "$format_reference" | awk '{print $1}')"
    current_hash="$("${hash_cmd[@]}" "$artifact" | awk '{print $1}')"
    if [[ "$current_hash" != "$format_hash" ]]; then
        echo "reproducible_build_check=FAILED"
        echo "input=$input"
        echo "format=$format"
        echo "expected_hash=$format_hash"
        echo "actual_hash=$current_hash"
        echo "reference_output=$format_reference"
        echo "mismatch_output=$artifact"
        exit 2
    fi
done

elf_hash="$("${hash_cmd[@]}" "$tmp_dir/a_elf64_1.bin" | awk '{print $1}')"
macho_hash="$("${hash_cmd[@]}" "$tmp_dir/a_macho64_1.bin" | awk '{print $1}')"
echo "reproducible_build_check=PASS"
echo "input=$input"
echo "compiler_builds=2"
echo "compilations_per_format=4"
echo "elf64_sha256=$elf_hash"
echo "macho64_sha256=$macho_hash"
