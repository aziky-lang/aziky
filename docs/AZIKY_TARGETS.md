# Aziky Targets and Platform Runtime Boundary

Last updated: 2026-07-16

## Target model

Aziky target selection is explicit and deterministic:

```text
aziky target list
aziky target show <triple>
aziky compile app.azk -o app --target x86_64-pc-windows-msvc
```

A target specification owns the architecture, operating system, ABI, object
format, pointer width, endianness, process entry, kernel/API call convention,
native service constants, and independently reported runtime capabilities.
Aliases are accepted only as input; artifact metadata always uses the canonical
triple.

| Target | Format | Codegen | Runtime level |
|---|---|---|---|
| `x86_64-unknown-linux-gnu` | ELF64 | accepted | application baseline |
| `x86_64-pc-windows-msvc` | PE32+/COFF | accepted | application baseline |
| `x86_64-apple-darwin` | Mach-O64 | accepted | core: startup, allocation, files, process |
| `aarch64-unknown-linux-gnu` | ELF64 | planned | none |

Codegen acceptance and individual runtime capabilities are deliberately
separate. A source program requiring an unavailable capability is rejected
before machine-code emission and before its output path is created. This lets a
real, useful target grow in audited slices without claiming that unimplemented
services work.

## Shared instruction lowering

The x86-64 instruction selector is shared. It emits calls through a target-owned
runtime boundary instead of embedding anonymous Linux assumptions:

- Linux uses the x86-64 syscall ABI and Linux negative-error convention.
- Darwin uses the x86-64 BSD syscall class, converts carry-set errno results to
  Aziky's negative-error convention, and translates mmap/open flags.
- Windows uses stable internal service identifiers. A self-contained native
  dispatcher converts the compiler's service ABI into the Microsoft x64 ABI
  and imported `KERNEL32.dll` operations.

Linux output from before this refactor remains byte-for-byte identical.

## Windows x86-64

Windows executables are deterministic PE32+ console images with a fixed,
non-ASLR base and an explicit import table. No CRT or separately installed Aziky
runtime is required. The native dispatcher currently covers:

- process startup, stdout/stderr writes, and `ExitProcess`;
- `VirtualAlloc`/`VirtualFree` allocation;
- `CreateFileA`, complete `ReadFile`/`WriteFile` loops, and `CloseHandle`;
- wall and monotonic clocks plus process identity; and
- `CreateThread`, join/cleanup, and channel synchronization.

The synchronization baseline uses a one-millisecond kernel sleep before
rechecking the channel sequence. This remains blocking and correct on both
Windows and Wine. `WaitOnAddress` is reserved as a future performance
optimization because the available Wine runtime exposes but aborts that API.

Relocatable COFF objects and deterministic indexed static libraries are also
supported. Windows DLL output remains explicitly rejected until the export and
loader contract is designed.

## macOS x86-64

macOS executables use a direct `LC_UNIXTHREAD` entry and the kernel-provided
initial stack. The image has no dyld or libSystem dependency. Native Darwin
startup, output, mmap allocation, process identity, and file operations are
lowered today. The syscall identifiers are checked against Apple's
[`syscalls.master`](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/syscalls.master),
including mmap `197`, openat `463`, and the Darwin file/syscall conventions.

Clocks, native threads, and channel synchronization remain capability-gated.
Darwin does not expose Linux's clone/futex model: its native thread path requires
the bsdthread registration/create/terminate contract (XNU calls `360`, `361`,
and `366`) and its clock path requires a stable Mach timebase conversion. Aziky
will not substitute Linux constants or claim those services until they execute
on native macOS hardware.

## Formats and acceptance

With `--target`, the target's format is automatic and an incompatible explicit
`--format` is rejected. Format-only Mach-O output remains for writer regression
coverage, but explicit targets are the supported application path.

`scripts/run_portability_gate.sh` proves deterministic containers,
target/format validation, capability rejection, Linux execution, object
recognition, and the full Rust suite. `--with-wine` additionally executes the
Windows minimal, platform-services, and threading/channel applications with
the same expected statuses as Linux.

Native macOS validation remains mandatory before expanding the Darwin
capability set. Linux CI validates deterministic Mach-O generation and format
structure, while the `macos-15-intel` public CI job runs
`scripts/run_macos_x86_64_gate.sh` natively. The gate executes deterministic
startup/output, process identity, allocation, and file round trips before any
additional Darwin capability can be checked off. The same script can be run by
contributors on Intel macOS or Apple silicon with Rosetta 2.
