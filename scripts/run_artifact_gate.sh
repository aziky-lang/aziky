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
cp -a examples/package_app "$tmp_dir/package_app"

echo "[2/7] require byte-identical artifacts across builds and absolute roots"
for kind in object static-library shared-library; do
    "$compiler" compile examples/package_app/src/main.azk \
        -o "$tmp_dir/original-$kind" --emit "$kind" >/dev/null
    "$compiler" compile "$tmp_dir/package_app/src/main.azk" \
        -o "$tmp_dir/copied-$kind" --emit "$kind" >/dev/null
    "$compiler" compile "$tmp_dir/package_app/src/main.azk" \
        -o "$tmp_dir/repeated-$kind" --emit "$kind" >/dev/null
    cmp -s "$tmp_dir/original-$kind" "$tmp_dir/copied-$kind"
    cmp -s "$tmp_dir/copied-$kind" "$tmp_dir/repeated-$kind"
done
for kind in object static-library; do
    "$compiler" compile examples/package_app/src/main.azk \
        -o "$tmp_dir/original-macho-$kind" --emit "$kind" --format macho64 >/dev/null
    "$compiler" compile "$tmp_dir/package_app/src/main.azk" \
        -o "$tmp_dir/copied-macho-$kind" --emit "$kind" --format macho64 >/dev/null
    cmp -s "$tmp_dir/original-macho-$kind" "$tmp_dir/copied-macho-$kind"
done

echo "[3/7] validate ELF relocatable symbols, DWARF, provenance, and linking"
readelf -h -S -s "$tmp_dir/original-object" >"$tmp_dir/elf-object.txt"
rg -q 'Type:[[:space:]]+REL' "$tmp_dir/elf-object.txt"
for section in .text .note.aziky .debug_abbrev .debug_info .debug_line .debug_str .symtab .strtab; do
    rg -q "$section" "$tmp_dir/elf-object.txt"
done
rg -q 'aziky_program_entry' "$tmp_dir/elf-object.txt"
rg -q '_start' "$tmp_dir/elf-object.txt"
rg -q 'aziky.block.0' "$tmp_dir/elf-object.txt"
readelf --debug-dump=info --debug-dump=decodedline "$tmp_dir/original-object" \
    >"$tmp_dir/dwarf.txt"
rg -q 'Aziky compiler' "$tmp_dir/dwarf.txt"
rg -q 'package_app@0.1.0/src/main.azk' "$tmp_dir/dwarf.txt"
rg -q 'math@1.0.0/src/lib.azk' "$tmp_dir/dwarf.txt"
strings "$tmp_dir/original-object" >"$tmp_dir/object-strings.txt"
rg -q '^aziky-metadata=1$' "$tmp_dir/object-strings.txt"
rg -q 'declaration.*function.*main' "$tmp_dir/object-strings.txt"
if rg -q '/home/|/tmp/' "$tmp_dir/object-strings.txt"; then
    echo "artifact_gate=FAILED reason=absolute_path_leaked"
    exit 2
fi
ld -o "$tmp_dir/object-linked" "$tmp_dir/original-object" -e _start
set +e
"$tmp_dir/object-linked"
object_status=$?
set -e
if [[ "$object_status" -ne 82 ]]; then
    echo "artifact_gate=FAILED reason=object_link_status actual=$object_status expected=82"
    exit 2
fi

echo "[4/7] validate deterministic indexed static libraries"
ar t "$tmp_dir/original-static-library" >"$tmp_dir/archive.txt"
rg -q '^aziky\.o$' "$tmp_dir/archive.txt"
nm -s "$tmp_dir/original-static-library" >"$tmp_dir/archive-symbols.txt"
rg -q '_start in aziky\.o' "$tmp_dir/archive-symbols.txt"
rg -q 'aziky_program_entry in aziky\.o' "$tmp_dir/archive-symbols.txt"
ld -o "$tmp_dir/static-linked" -e _start "$tmp_dir/original-static-library"
set +e
"$tmp_dir/static-linked"
static_status=$?
set -e
if [[ "$static_status" -ne 82 ]]; then
    echo "artifact_gate=FAILED reason=static_link_status actual=$static_status expected=82"
    exit 2
fi
ar p "$tmp_dir/original-macho-static-library" aziky.o >"$tmp_dir/archive-macho.o"
cmp -s "$tmp_dir/original-macho-object" "$tmp_dir/archive-macho.o"

echo "[5/7] validate loadable ELF shared image and non-executable stack"
readelf -h -l -d --dyn-syms "$tmp_dir/original-shared-library" \
    >"$tmp_dir/shared.txt"
rg -q 'Type:[[:space:]]+DYN' "$tmp_dir/shared.txt"
rg -q 'SONAME.*libaziky\.so' "$tmp_dir/shared.txt"
rg -q 'aziky_program_entry' "$tmp_dir/shared.txt"
rg -q 'GNU_STACK' "$tmp_dir/shared.txt"
rg -q 'RW[[:space:]]+0x10' "$tmp_dir/shared.txt"
python3 -c 'import ctypes,sys; lib=ctypes.CDLL(sys.argv[1]); assert lib.aziky_program_entry' \
    "$tmp_dir/original-shared-library"

echo "[6/7] validate Mach-O relocatable metadata and capability diagnostics"
llvm-readobj --file-headers --sections --symbols "$tmp_dir/original-macho-object" \
    >"$tmp_dir/macho.txt"
rg -q 'FileType: Relocatable' "$tmp_dir/macho.txt"
for section in __text __aziky __debug_abbrev __debug_info __debug_line __debug_str; do
    rg -q "Name: $section" "$tmp_dir/macho.txt"
done
rg -q 'Name: _aziky_program_entry' "$tmp_dir/macho.txt"
for run in first second; do
    if "$compiler" compile examples/package_app/src/main.azk \
        -o "$tmp_dir/rejected-$run.dylib" --emit shared-library --format macho64 \
        >"$tmp_dir/rejected-$run.txt" 2>&1; then
        echo "artifact_gate=FAILED reason=macho_shared_accepted"
        exit 2
    fi
    test ! -e "$tmp_dir/rejected-$run.dylib"
done
cmp -s "$tmp_dir/rejected-first.txt" "$tmp_dir/rejected-second.txt"
rg -q 'not supported for macho64 until the Darwin runtime/loader target is accepted' \
    "$tmp_dir/rejected-first.txt"

echo "[7/7] require full suite and clean diffs"
cargo test -q --locked --offline
git diff --check

echo "artifact_gate=PASS"
