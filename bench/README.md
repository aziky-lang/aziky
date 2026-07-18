# Benchmark Suite

This suite is intentionally small and strict.

Active scenarios:
- `stream_lcg`: bounded-state scalar update loop.
- `packet_classifier`: bounded-state unpredictable branch loop.
- `ring_write`: fixed-size ring-buffer write pressure.
- `affine_mix`: arithmetic transform loop with bounded integer state.
- `sort_window`: refill + hand-written 8-lane bubble sort.
- `bloom_filter`: masked bitset insert/probe kernel.
- `hash_join`: grouped-probe hash table + bloom prefilter kernel.
- `histogram`: data-dependent indexed counter updates over 64 bins.
- `binary_search`: lower-bound search over a sorted 64-element array.
- `prefix_scan`: repeated 16-element refill and inclusive prefix scan.

Methodology:
- Every benchmark has an `*.azk`, `*.rs`, and `*.c` implementation.
- The three sources implement the same source-level algorithm, constants, data sizes, integer wrapping behavior, initialization, loop bounds, and checksum reduction.
- Each triplet starts with an identical `benchmark-contract` declaration. `scripts/check_benchmark_contracts.py` rejects missing sources, parameter drift, duplicate scenarios, and Aziky/C scenarios omitted from the timed suite.
- `scripts/run_bench_suite.sh` rejects a triplet before timing unless all three optimized binaries produce the same process result and the same full 64-bit checksum.
- After semantic parity is established, each compiler may legally optimize its own implementation. The suite therefore compares optimized programs, including loop unrolling, inlining, strength reduction, and closed-form transformations where a compiler can prove them valid.
- Backend regression tests separately ensure Aziky preserves required loops, memory effects, branches, and result lanes when those effects cannot legally be removed.
- Rust is built with `opt-level=3`, `target-cpu=native`, `codegen-units=1`, `panic=abort`, `lto=fat`, and `overflow-checks=off`.
- C is built with `-O3 -march=native -flto` when available, along with stripped-down unwind settings.

Notes:
- The process exit status exposes only a compact checksum, so it is never used
  alone as proof of equivalence. Verification builds emit the full checksum as
  eight little-endian bytes before any timed run is accepted.
- The workload contract is a regression guard, not a substitute for review.
  Changes to benchmark logic must be applied to and reviewed across all three
  source files.
- `scripts/check_sort_bench_gate.sh` gates only `sort_window`, which is the sort scenario that remains practical in Aziky’s current pipeline.
- `scripts/run_allocator_stress.sh` remains separate for allocator-specific stress testing.
