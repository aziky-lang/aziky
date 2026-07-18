# Aziky Objects, Libraries, Symbols, and Debug Metadata

Aziky emits its object and library containers directly. The compiler does not
invoke an assembler, archiver, linker, platform SDK, or debug-information tool.
External tools are used by release gates only to independently validate the
generated standards.

## Command surface

```text
aziky compile <entry.azk> -o <output> \
    --emit executable|object|static-library|shared-library \
    --format elf64|macho64
```

`executable` remains the default. Artifact bytes depend only on source/package
inputs and explicit compiler options, never output names, absolute project or
cache paths, timestamps, process identity, ownership, or host directory order.

## Capability matrix

| Artifact | ELF64 | Mach-O64 |
|---|---:|---:|
| Direct executable container | Yes, native Linux x86-64 | Structural scaffold |
| Relocatable object | Yes | Yes |
| Deterministic static archive | Yes | Yes |
| Shared library | Yes, native/loadable | Not yet accepted |

Requesting a Mach-O shared library fails before writing output. A Darwin dylib
requires an accepted Darwin runtime, load-command, linking, and native-test
contract; wrapping Linux runtime code in a dylib would violate Aziky's standing
portability requirement.

## Whole-program image ABI

The current frontend/backend lowers a complete application rooted at `main`
into one self-contained, position-independent code image. Internal calls and
embedded data are resolved before container emission, so relocatable objects do
not need external relocation records.

ELF objects export `_start` and `aziky_program_entry` at the beginning of the
image. Mach-O objects export `_start` and `_aziky_program_entry`. ELF static
archives carry a deterministic GNU/System V archive index for both entry
symbols, allowing an ordinary linker to extract and link the member by `_start`.

The ELF shared object exports `aziky_program_entry` and has a valid dynamic
symbol/string table, System V hash, dynamic table, SONAME, load segment,
`PT_DYNAMIC`, and non-executable stack declaration. It is loadable without an
Aziky runtime dependency.

`aziky_program_entry` is a process-entry ABI, not a C-callable function: it
expects the accepted Linux process-entry stack and terminates through Aziky's
native process semantics. First-class callable public-library functions require
a future exported-function ABI and must not be faked by assigning declaration
names to unknown addresses.

## Deterministic symbol contract

Every ELF object has:

- a local `.text` section symbol;
- local `aziky.block.N` function symbols for machine blocks whose exact emitted
  byte ranges are known;
- global `_start` and `aziky_program_entry` symbols with exact image size.

Symbols are ordered null, local section, increasing block number, then stable
global entries. `.symtab` correctly identifies the first global index and links
to `.strtab`. Mach-O emits its two exact external section symbols through
`LC_SYMTAB`. No declaration receives a fabricated machine address.

## Source and debug metadata

ELF relocatables and libraries contain standard DWARF v4 `.debug_abbrev`,
`.debug_info`, `.debug_line`, and `.debug_str` sections. Mach-O relocatables
carry their equivalents under `__DWARF`. The compile unit records:

- the Aziky producer;
- a vendor language identifier;
- the logical root source;
- every logical source file in the line table;
- a text range ending at the emitted code size.

This is the initial line-table baseline. Until user functions retain exact
machine ranges through optimization, the line program associates source files
with the whole-program image rather than claiming instruction-precise lines.

The ELF `.note.aziky` note and Mach-O `__DATA,__aziky` section add Aziky's
lossless declaration provenance contract. Version 1 records target, logical
source index/path, qualified declaration name and kind, visibility, line, and
column. Functions, methods, types, traits, modules, and imports are included.

Package paths use `name@exact-version/relative/path`; root paths use the root
package identity. Embedded modules retain `<aziky:...>` names. Legacy projects
use paths relative to their entry directory. Absolute source and cache paths
are never written, so moving an identical project produces identical objects.

Direct executable containers remain stripped in this baseline. Debuggable
linking starts from `--emit object` or a library artifact, preserving the
existing compact executable layout and deterministic executable gates.

## Archive contract

Static libraries use the portable `!<arch>\n` format with:

- zero timestamps, user IDs, and group IDs;
- fixed `100644` member mode;
- stable member name `aziky.o`;
- a sorted big-endian symbol index;
- deterministic even-byte padding.

The member is an ELF or Mach-O relocatable selected by `--format`.
