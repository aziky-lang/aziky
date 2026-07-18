# Aziky Technical Architecture

## Project Objective
Build a zero-dependency, deterministic, end-to-end compiler toolchain that translates aziky source code directly into x86_64 machine code without invoking an external assembler or linker.

Current support and limitations are recorded in `docs/RELEASE_STATUS.md`.

## Non-Negotiable Constraints
- Zero external crates for parser, codegen, allocator, or binary packaging.
- Deterministic output: same source + same target triple => bit-for-bit identical binary.
- Memory-safe compiler implementation (Rust).
- Fully offline build and compile path.
- No hidden dynamic dispatch in generated machine code for language-level polymorphism.

## System Architecture
The compiler is a monolithic binary with explicit, auditable pipeline stages.

1. `frontend` (source -> Typed AST)
1. `safety` (borrow + linear checks)
1. `middle` (lowering + monomorphization + layout)
1. `backend` (instruction selection + register allocation)
1. `object` (ELF64/Mach-O writer + relocations)
1. `driver` (CLI, diagnostics, deterministic build controls)

---

## 1) Frontend (Source to Typed AST)

### 1.1 Lexer
Hand-written scanner (SIMD optimization optional in v1; correctness first).

Responsibilities:
- Convert UTF-8 source to token stream with stable span metadata.
- Reject invalid UTF-8, non-canonical forms, and homoglyph-confusable identifiers.
- Emit newline/indent-sensitive tokens only if grammar requires it.

Acceptance:
- Tokenization result is deterministic and independent of host locale.
- Invalid byte sequences return structured diagnostics (no panic).

### 1.2 Parser
Recursive-descent parser producing AST and then Typed AST.

Required transforms in parse/lower stage:
- Desugar `obj.func(x)` to `func(ref obj, x)` for static dispatch pipeline.
- Flatten `embed` fields into concrete layout metadata.
- Preserve source spans through all desugar operations.

Acceptance:
- Parser never performs type-dependent speculative backtracking.
- All syntax errors include line, column, and nearest recovery point.

---

## 2) Safety Engine (Borrow Checker + Linear Types)

### 2.1 Region-Based Lifetime Model
Use scope regions instead of graph-heavy inference.

Region classes:
- `stack(scope_id)`
- `heap(arena_id)`
- `static`

On scope exit, backend emits deterministic cleanup sequence (`add rsp, imm` or region-pointer reset), not GC.

### 2.2 Permission Model
- `val`: owning value; move invalidates source binding.
- `ref`: shared read-only borrow (N allowed).
- `mut`: exclusive mutable borrow (exactly one).

### 2.3 Linear Types
Linear resources (e.g., key material, file descriptors) must be consumed exactly once.

Acceptance:
- Compile-time error for drop-without-consume and double-consume.
- Diagnostics reference origin point and missing consume path.

---

## 3) Language Model (Pseudo-OOP, Traits, and Layout)

### 3.1 Data Layout
`embed` causes physical field flattening.

```aziky
struct SensorData {
    id: u32,
    value: f64,
}

struct AerospaceModule {
    embed SensorData,
    unit_id: u16,
}
```

Layout guarantees:
- Field order deterministic and documented.
- Alignment and padding rules explicit per target ABI.

### 3.2 Behavior Extension
`extend` binds statically to concrete receiver types.

```aziky
extend AerospaceModule {
    fn calibrate(mut self) {
        self.value = 0.0;
    }
}
```

### 3.3 Traits (Static Dispatch Only)
Trait calls are monomorphized at compile time. No vtables.

Acceptance:
- Trait call lowers to direct symbol reference.
- All generated instances listed in compile artifact report.

---

## 4) Backend (x86_64 Encoder)

### 4.1 Instruction Encoder
Manual x86_64 encoder with tested primitives:
- REX prefix generation (`REX.W`, `REX.R`, `REX.X`, `REX.B`)
- ModR/M emission
- SIB emission
- immediate/displacement endianness

### 4.2 Register Allocation
Deterministic segmented allocator over explicit SSA/value and memory dataflow.

Strategy:
- Represent lifetime holes explicitly and allocate by segment interference.
- Prefer hot, call-spanning values in GPRs; discount rematerializable constants.
- Coalesce proven-compatible copies and color disjoint scalar spills onto shared slots.
- Keep dynamically indexed and packed objects contiguous and addressable.
- Use one whole-program internal frame only when required; eligible leaf paths are frameless.
- Keep allocation, tie-breaking, stack slots, and internal-call slot exchange deterministic.

### 4.3 ABI + Syscalls (Linux baseline)
Initial backend target: Linux x86_64 syscall ABI.

Acceptance:
- Can encode `write` and `exit` sequences.
- Generated code executes on Linux without external toolchain stages.

---

## 5) Internal Linker / Binary Packaging

### 5.1 Executable Format
Direct ELF64 output first; Mach-O as subsequent target.

ELF64 v1 scope:
- ELF header
- one `PT_LOAD` program header
- contiguous code segment
- entry point at emitted code start

### 5.2 Relocation / Back-Patching
When target offset unknown:
1. Emit placeholder immediate (`0x00000000`) for `call rel32` / jump.
1. Store relocation record `{patch_offset, target_label, kind}`.
1. Resolve after final code layout and patch with signed relative displacement.

### 5.3 Determinism Rules
- No wall-clock timestamps.
- No host path embedding.
- Stable symbol ordering.
- Stable map/dictionary iteration (avoid randomized hash iteration in output-sensitive paths).

Acceptance:
- Hash of output binary remains constant across repeated builds.

---

## 6) Planned Inline Assembly Boundary
`asm { ... }` is specified as a future constrained escape hatch; it is not yet part of the implemented parser/lowering surface.

Rules:
- Explicit clobber list required.
- Compiler verifies save/restore contract around block.
- Inline asm is forbidden from violating linear resource obligations.

---

## 7) Bootstrapping and Repository Layout

Language for compiler implementation: Rust (std only).

Proposed tree:

```text
aziky/
  docs/ARCHITECTURE.md
  Cargo.toml
  src/
    main.rs
    driver/
    frontend/
    safety/
    middle/
    backend/
      x86_64/
    object/
      elf64/
    diagnostics/
  tests/
    integration/
  examples/
    hello.azk
```

---

## 8) Milestones and Current State

`docs/RELEASE_STATUS.md` is the authoritative release boundary. This section
records only stable architectural milestones and does not duplicate transient
work queues or benchmark snapshots.

### Milestone A: Binary Zero — complete

- [x] Exact-byte ELF64 and program headers.
- [x] Direct Linux x86-64 `write` and `exit` syscall encoding.
- [x] Executable exit-only and deterministic write/exit programs.
- [x] Local repeat-build SHA-256 determinism gate.

CI enforcement remains tracked separately because a local gate is not the same as a configured CI job.

### Milestone B: Core Logic — baseline complete, expansion ongoing

- [x] Semantic IR, general runtime IR, explicit value SSA/MemorySSA analysis, and MachineLIR.
- [x] Deterministic segmented register allocation with weighted interference and spill-slot coloring for general runtime IR.
- [x] Trait validation and `Type__method` monomorphization.
- [x] Deterministic runtime slot cleanup on exit.

The allocator and runtime IR still need broader coverage as the runtime-native language surface expands.

### Milestone C: Safety — partial

- [x] Lexical-scope borrow tracking and alias diagnostics in the semantic/interpreter path.
- [x] Deterministic cleanup for current runtime stack slots and explicit heap operations.
- [ ] Complete move semantics and negative path coverage.
- [ ] Linear-resource consume-once enforcement on every runtime path.

### Milestone D: Language Surface — partial

- [x] Functions, locals, arithmetic, structs, enums, arrays, dictionaries, conditionals, and loops.
- [x] Payload/generic enums, exhaustive `match`, and built-in
  `Option<T>`/`Result<T, E>` with checked semantic lookup/parsing APIs.
- [x] Typed owned `list<T>`/`map<K, V>` semantic baseline plus runtime-native
  scalar-list descriptors, checked geometric growth, typed scalar heap
  indexing/mutation, iteration, membership, clearing, reserve/shrink operations,
  checked access and value-returning `pop` through typed two-slot scalar
  `Option<T>`, packed 1/2/4/8-byte element storage, IEEE-correct float
  membership, and deterministic cleanup; aggregate lists, general option
  ABI/match support, map layouts, and a shared allocator remain incomplete.
- [x] Recursive deterministic embedded-struct layout flattening.
- [x] Trait-method static dispatch in semantic lowering.
- [x] Recursive deterministic modules with isolated qualified symbols,
  private-by-default declarations, `pub` exports/re-exports, and selective
  aliased imports.
- [x] Embedded offline `std`/`core`/`alloc` Aziky-source foundation compiled
  through the ordinary user-language pipeline.
- [x] Runtime-generic linear heap-owner baseline with non-copyable bindings,
  checked early release, frozen allocation sizes, and cleanup across lexical and
  terminal control-flow edges.
- [ ] Runtime-native user-defined receiver-method dispatch without interpreter fallback.
- [ ] Package manifests, dependency resolution, and public module/glob surfaces.
- [ ] Runtime-native enum/match and generalized allocator-backed dynamic
  collection lowering (scalar-list core operations are the completed first family).

### Milestone E: Hardening — local gates complete, CI pending

- [x] Deterministic lexer/parser fuzz smoke.
- [x] Differential encoder checks.
- [x] Repeat-emission binary determinism checks.
- [x] Aziky/Rust/C benchmark parity and timing suite.
- [x] Full-width benchmark result transport and an integrity-liveness mode shared by timed and verification binaries.

### Performance integrity contract

Benchmark optimization is subordinate to semantic and workload preservation.
The official suite must reject a scenario before timing unless Aziky, Rust,
and C agree on the complete 64-bit workload result. Timed Aziky binaries use
the same full-state liveness contract as verification binaries; a narrow OS
exit status must never authorize deletion of iterations, state transitions, or
memory traffic that defines the workload. Candidate PGO layouts, ISA variants,
and recurrence transforms are accepted only after this gate and repeated
measurement. Benchmark names and known final answers are never optimization
inputs.
- [ ] CI jobs enforcing reproducibility and performance gates.

---

## 9) Verification Matrix

Every implementation phase must include the applicable checks:

- Unit tests for pure transformations and safety rules.
- Byte-level tests for machine encoding and object layout.
- Compile-and-execute integration tests.
- Semantic-equivalence tests for specialized kernels.
- Repeated-emission hash checks for determinism.
- Cross-language checksum parity before performance measurement.

The local full-quality gate composes tests, deterministic fuzzing, differential
encoding, repeated binary hashing, benchmark parity/timing, sort performance,
and allocator stress. Public CI runs the non-benchmark release gate, native
platform checks, repository hygiene, and editor-extension validation; noisy
performance measurements remain local by design.

---

## 10) Performance Program and Guardrails

The performance objective is broad real-workload wins against optimized Rust/C,
not isolated benchmark tricks. Reproducible methodology belongs in
`bench/README.md`; dated measurements belong in commit or release records rather
than an undated architecture document.

Non-negotiable rules:

- No optimization may weaken borrow, ownership, or linear-resource guarantees.
- No profile-guided transform may make output depend on timing, iteration order, or unstable maps.
- Specialized kernels require exact arithmetic reasoning and regression tests against scalar semantics.
- Benchmark programs must pass cross-language result parity before their timings count.
- Benchmark-only observability limits do not justify changing the source workload asymmetrically. Every implementation must retain the same workload contract; once parity is established, each compiler remains free to apply any semantics-preserving optimization.
- Target-specific vector paths must have deterministic feature selection and scalar/SSE fallbacks where promised.

---

## 11) Parallel Loop Strategy (Performance + Safety)

Current `parfor` model:
- Each iteration executes with an isolated environment snapshot to avoid data races by construction.
- Iteration outputs are merged by logical iteration index, not completion order, preserving deterministic binaries and runtime behavior.
- `break` and `exit` are rejected in `parfor` bodies to prevent nondeterministic control-flow collapse.
- Reduction mode runs deterministic parallel map+reduce with fixed merge ordering across chunks.

Next hardening targets:
- Add side-effect analysis to reject hidden shared-state writes and enforce parallel purity contracts.
- Add configurable scheduling policies (`static`, `dynamic`, `guided`) while preserving deterministic observable output.
- Add deterministic reduction identities for empty ranges (`sum=0`, `min/max` optional types) without sacrificing type soundness.

Microarchitecture optimization references to align backend roadmap:
- Intel Optimization Reference Manual (latest v50 family guidance).
- Agner Fog optimization manuals and instruction tables.
- Work-stealing runtime design principles from Cilk-5.
- LLVM Loop + SLP vectorizer design notes for cost-model and legality checks.
- OpenMP 6.0/5.2 spec sections on memory model, reductions, and loop parallel semantics.
- Top-down PMU methodology (`perf`) for bottleneck classification.

Primary references:
- https://www.intel.com/content/www/us/en/content-details/821612/intel-64-and-ia-32-architectures-optimization-reference-manual-volume-1.html
- https://www.agner.org/optimize/
- https://www.fftw.org/~athena/abstracts/abstract17.html
- https://llvm.org/docs/Vectorizers.html
- https://www.openmp.org/specifications/
- https://www.kernel.org/doc/html/latest/admin-guide/perf/index.html

---

## 12) Source-of-Truth Policy

- `docs/ARCHITECTURE.md`: stable objective, architecture, constraints, and acceptance principles.
- `docs/AZIKY_LANGUAGE_REFERENCE.md`: implemented language surface, with explicit notes where behavior still falls back or remains planned.
- `docs/RELEASE_STATUS.md`: accepted release surface and known limitations.
- `docs/STANDARD_LIBRARY_ROADMAP.md`: active application-library plan.

When these documents disagree, implementation and tests establish the facts;
correct the relevant reference and release-status document in the same change.
