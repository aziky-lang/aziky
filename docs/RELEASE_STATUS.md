# Aziky Release Status

Last updated: 2026-07-18

## Accepted baseline

The release gate accepts a deterministic, offline, native
application-development baseline. The permanent package application combines a
checksum-locked direct/transitive dependency graph with a native standard
process service. Separate permanent applications cover the complete host
snapshot/clock/path facade, owned filesystem I/O, and native threads/channels.
The gate also requires package tests, formatter and linter CI modes, ELF64 and
Mach-O64 artifact reproducibility across absolute roots, object/static linking,
ELF shared loading, stable negative diagnostics, and the full Rust suite.

Run the final gate from a clean tree:

```text
scripts/run_release_gate.sh
```

`--allow-dirty` exists only for developing the gate. It does not qualify a
release tag. Cargo compilation and tests run locked and offline; Aziky package
resolution is always offline and checksum-verified.

## Explicit remaining platform and target limitations

- Linux x86-64 and Windows x86-64 carry the complete accepted runtime surface.
  Windows uses native PE/COFF (PE32+ and COFF) artifacts, Win32 services,
  threads, and channels and is
  execution-gated under Wine without a CRT or installed Aziky runtime.
- macOS x86-64 has direct-entry Mach-O codegen plus Darwin startup, allocation,
  file, and process services. Mach clocks, bsdthread-based threads/channels,
  shared libraries, and native macOS validation remain incomplete and
  capability-gated.
- AArch64 instruction selection and ABI lowering do not exist yet.
- Child-process spawning, owned child handles, pipes, and waiting are not
  implemented. Child-process spawning must land as a linear resource ABI, not
  a scalar syscall wrapper.
- Filesystem support is the fail-fast owned-file baseline. Directories,
  metadata, seeking, buffered/stream I/O, recoverable I/O errors, and async I/O
  remain future work.
- Arguments and environment are immutable startup snapshots. Environment
  lookup/mutation and raw non-UTF-8 platform representations are not complete.
  Monotonic and wall time are scalar nanoseconds rather than distinct duration
  and instant types.
- Channels are single-producer/single-consumer and carry `u64`. The unbounded
  channel reserves a sparse 8-GiB virtual region. General message types,
  multi-producer/multi-consumer channels, mutexes, read/write locks, condition
  variables, and a portable scheduler/runtime boundary remain future work.
- Relocatable and library artifacts wrap Aziky's self-contained whole-program
  image. `aziky_program_entry` is a process-entry ABI, not a C-callable exported
  function ABI. Direct executables do not yet carry the object debug baseline.
- Package resolution uses exact versions, a deterministic single-version
  graph, and a pre-populated local cache. There is no registry/network fetch,
  source publishing/signing workflow, or multi-version selection.
- Test execution is sequential and currently enabled only when the selected
  target can execute on the host; cross-target Windows execution is covered by
  the dedicated Wine portability gate.
  Formatter and linter behavior is intentionally a baseline, not a complete
  style/type lint suite. Benchmark discovery and first-class benchmark
  reporting remain deliberately deferred from the developer-command baseline.

These are capability boundaries, not silent fallbacks. Work that depends on an
unsupported target/runtime combination must add an explicit capability and a
native acceptance gate before it can be described as supported.
