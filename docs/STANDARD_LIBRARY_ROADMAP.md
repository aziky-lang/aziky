# Aziky Standard Library Roadmap

Last updated: 2026-07-18

## Goal

Turn Aziky's small platform facade into a coherent, portable standard library
for command-line tools, long-running services, and general applications. Every
accepted API must work through normal Aziky source compilation, preserve owned
resource cleanup, build offline, produce deterministic results where the host
permits it, and expose host-dependent behavior explicitly.

This roadmap tracks large implementation milestones. A checked chunk means
its language dependencies, runtime lowering, public modules, positive and
negative tests, native execution coverage, examples, and reference
documentation are all complete. A module stub or semantic-only fallback does
not complete a chunk.

## Standard-library boundary

The standard library owns universally useful language and operating-system
facilities:

- core values, conversions, comparison, hashing, numeric operations, and math;
- owned and borrowed text, formatting, collections, iterators, and algorithms;
- I/O, paths, filesystems, arguments, environment, processes, time, randomness,
  concurrency, and networking;
- deterministic testing/debug support needed to use those facilities safely.

Application protocols and product layers remain official offline packages:
JSON and other serialization formats, HTTP, TLS, cryptography, databases, GUI,
web frameworks, compression, image/audio codecs, and domain-specific clients.
They may graduate into `std` only when they are required by nearly every Aziky
program and can honor the same portability and compatibility guarantees.

## Non-negotiable Aziky rules

- [ ] No hidden garbage collector, interpreter, separately installed runtime,
  network fetch, locale dependency, randomized hash seed, or implicit global
  executor.
- [ ] Owned resources are move-only and cleaned exactly once on every normal,
  early-return, error-propagation, break, and panic edge.
- [ ] Recoverable failures use typed `Result`; absence uses `Option`; panics are
  reserved for violated invariants and explicitly named unchecked operations.
- [ ] Time, randomness, environment mutation, filesystem access, process
  creation, networking, and concurrency remain explicit capabilities.
- [ ] Safe indexing and conversions stay checked; lossy and wrapping operations
  are visible in their names.
- [ ] Debug output, hashing, iteration promises, formatting, tests, and
  diagnostics have documented deterministic behavior.
- [ ] Platform modules share portable public contracts and isolate Linux,
  Windows, and macOS ABI details behind target runtime dispatchers. Unsupported
  capabilities fail at compile time rather than being falsely advertised.

## Chunk 1 — Core contracts and fallibility

- [x] Version the compiler/standard-library intrinsic ABI and diagnose a
  mismatched embedded library deterministically.
- [ ] Finish general native `Option<T>` and `Result<T, E>` layouts, moves,
  returns, matching, combinators, and cleanup for scalar and owned payloads.
- [ ] Add `core.option`, `core.result`, `core.cmp`, `core.convert`, `core.hash`,
  and reusable typed error foundations.
- [ ] Add `usize`/`isize`, unit and never types, tuples, destructuring, ranges,
  slices, and borrowed `str` where required by public APIs.
- [ ] Centralize equality, ordering, hashing, formatting, copying, conversion,
  and cleanup capabilities in the type system.
- [ ] Complete checked, wrapping, saturating, and overflowing integer
  arithmetic; integer constants, bit operations, parsing, and conversions.
- [ ] Complete float classification, rounding, finite/total-order wrappers,
  checked conversions, and portable mathematical functions.
- [ ] Gate the chunk with offline compiler tests and a native core-contract
  application covering success, absence, recoverable error, and cleanup paths.

Progress delivered in the first Chunk 1 slice:

- ABI version `1` is stored in one embedded manifest, validated before every
  compilation, exposed by `stdlib_abi_version()`, and covered by malformed,
  mismatch, and native-execution tests.
- The initial `cmp` module provides `Ordering`, signed/unsigned 64-bit compare,
  and predicates. The initial `convert` module provides checked character,
  boolean, every fixed-width integer, and `f32`/`f64` parsing facades.
- Native enum-returning calls now capture ABI scratch slots into distinct local
  slots. Multiple calls can no longer mutate earlier `Ordering`, `Option`,
  `Result`, or user-enum bindings by aliasing one return area.
- Resource-enum layouts now carry owned UTF-8 strings, owned lists, and typed
  scalar maps. Repeated native `Option<list<T>>`, `Option<map<K, V>>`, mixed
  `Result<string, list<T>>`, and `Result<map<K, V>, string>` calls, branch
  returns, moves, matches, and cleanup are protected by the checked-in
  `examples/core_contract_app` execution gate.
- Nested non-resource enums now flatten into the native tagged ABI. Typed error
  enums can cross `Result<T, E>` calls and returns and be destructured by nested
  matches. The public conversion facade now exposes `ParseError::InvalidInput`
  instead of leaking intrinsic-owned diagnostic strings as its error type.
- Native `parse_i8` through `parse_i64`, `parse_u8` through `parse_u64`, and
  `parse_bool` use deterministic byte parsing with explicit range checks and
  owned `Result` errors. Native scalar `Result.is_ok`, `is_err`, and
  `unwrap_or` now share the tagged-enum ABI. Floating parsing and the remaining
  typed-facade call-path integration are still open in this chunk.

## Chunk 2 — UTF-8 text and deterministic formatting

- [ ] Establish `String`, borrowed `str`, Unicode-scalar `char`, byte slices,
  validated UTF-8 decoding, and explicit byte-versus-character indexing.
- [ ] Implement searching, prefix/suffix checks, splitting, lines, trimming,
  replacement, insertion/removal, case conversion, escaping, and joining.
- [ ] Add builders for allocation-efficient text construction and deterministic
  capacity behavior.
- [ ] Implement display/debug formatting traits, format-spec parsing, integer
  bases, float formatting, padding/alignment, escaping, and collection/struct
  rendering without addresses or locale dependence.
- [ ] Provide `format`, `print`, `println`, `eprint`, and `eprintln` on the same
  formatting engine with explicit stdout/stderr failure behavior.
- [ ] Gate the chunk with Unicode, malformed UTF-8, allocation-failure,
  deterministic-output, and cross-target container tests.

## Chunk 3 — Collections, iterators, and algorithms

- [ ] Stabilize generic `List`, `Map`, `Set`, `Deque`, slices, entries, and
  borrowed/mutable iterators with exact ownership and invalidation rules.
- [ ] Implement deterministic hashing and collision handling without hidden
  process-global seeds; provide explicit seeded hashers where adversarial input
  requires them.
- [ ] Implement iterator adapters: map, filter, filter_map, flat_map, fold,
  reduce, enumerate, zip, chain, take, skip, chunks, windows, and collect.
- [ ] Implement stable/unstable sorting, selection, binary search, partition,
  deduplication, reverse/rotate, min/max, and comparator/key variants.
- [ ] Provide collection conversion, cloning, equality, ordering where valid,
  deterministic debug rendering, and fallible reserve APIs.
- [ ] Gate the chunk with empty/boundary/large inputs, deliberate hash
  collisions, iterator cleanup, allocation failures, and deterministic runs.

## Chunk 4 — I/O, paths, and filesystem

- [ ] Define typed `IoError`, error kinds, owned handles, reader/writer traits,
  exact/partial operations, buffering, seeking, and explicit flushing.
- [ ] Add stdin/stdout/stderr handles plus byte/text readers and writers without
  conflating UTF-8 decoding with raw I/O.
- [ ] Complete lexical `Path`/`PathBuf`: components, roots/prefixes, joins,
  parent/name/extension, normalization without filesystem access, and native
  byte/wide-path conversion.
- [ ] Complete files and directories: open options, create/remove/rename/copy,
  metadata, permissions, directory iteration, canonicalization, links, and
  atomic temporary-file replacement where supported.
- [ ] Preserve platform-specific information in typed extensions while keeping
  portable behavior consistent across Linux, Windows, and macOS.
- [ ] Gate the chunk with binary data, malformed text, partial I/O, large files,
  cleanup on errors, path edge cases, and platform capability tests.

## Chunk 5 — Application host services

- [ ] Replace raw argument/environment entry APIs with owned snapshots, checked
  Unicode access, raw OS access, lookup, mutation, and deterministic iteration.
- [ ] Add distinct `Duration`, monotonic `Instant`, and wall-clock `SystemTime`,
  including checked arithmetic, sleeping, deadlines, and platform timebase
  conversion.
- [ ] Add explicit seeded deterministic PRNGs, entropy-backed construction as a
  fallible host capability, range/distribution APIs, and unbiased shuffling.
- [ ] Add process command construction, arguments, environment, working
  directory, stdio inheritance/pipes, spawn, status, wait, output capture,
  termination, and move-only child handles.
- [ ] Add portable system queries only where contracts are stable; keep host
  identity, locale, terminal, and platform extensions explicit.
- [ ] Gate the chunk with deterministic seeded runs, environment isolation,
  timeout/pipe behavior, child cleanup, and Linux/Windows execution tests.

## Chunk 6 — Concurrency and synchronization

- [ ] Finish portable native thread creation/joining and scoped threads that
  cannot outlive borrowed values.
- [ ] Add atomics and explicit memory orderings, mutexes, read/write locks,
  condition variables, once initialization, barriers, and thread parking.
- [ ] Stabilize bounded/unbounded channels with blocking/non-blocking operations,
  typed disconnect/full/empty results, selection, and deterministic close rules.
- [ ] Add thread-safe ownership traits/capabilities and reject invalid sharing
  statically.
- [ ] Add an explicit scheduler/executor only after cancellation, wakeups,
  resource cleanup, and deterministic testing are specified; never create one
  implicitly.
- [ ] Gate the chunk with contention, shutdown, panic, disconnect, cancellation,
  and sanitizer/stress tests on every fully accepted platform.

## Chunk 7 — Networking foundation

- [ ] Add typed IP addresses, socket addresses, parsing/formatting, and portable
  DNS resolution with explicit ordering and errors.
- [ ] Add move-only TCP listeners/streams and UDP sockets with connect, bind,
  accept, read/write, timeouts, shutdown, and socket option baselines.
- [ ] Integrate sockets with `std.io` and the concurrency/event model without
  hiding threads or blocking behavior.
- [ ] Isolate platform socket ABI and error translation; do not expose Unix file
  descriptors as the portable handle model.
- [ ] Gate the chunk with loopback integration, partial reads/writes, IPv4/IPv6,
  timeout, disconnect, cleanup, and deterministic local tests.

## Chunk 8 — Standard-library usability and release gate

- [ ] Give every public item reference documentation, ownership/error notes,
  target availability, checked examples, and consistent naming.
- [ ] Add per-module unit tests, compile-fail tests, integration applications,
  fuzz/property cases for parsers and collections, and deterministic snapshots.
- [ ] Add standard prelude policy, stable module paths, API compatibility rules,
  deprecation support, and semantic versioning tied to the intrinsic ABI.
- [ ] Add lint coverage for ignored `Result`, leaked resources, accidental
  nondeterminism, blocking calls in restricted contexts, and portability traps.
- [ ] Require locked/offline clean builds, byte-identical artifacts, no semantic
  fallback, no leaks/double cleanup, and full capability matrices before the
  application standard library is declared complete.

## Current baseline

The checked-in `stdlib` currently provides thin `core`, `alloc`, `args`, `env`,
`path`, `fs`, `process`, and `time` modules plus the `std` facade. It has useful
owned `File`, collection, string, `Option`, and `Result` compiler foundations,
but does not yet satisfy any large chunk above. Work begins with Chunk 1 because
all later public APIs depend on its type, fallibility, and cleanup contracts.
