# Audited Low-Level Optimization Research

Last updated: 2026-07-14

## Scope and target

This note records optimizations that are legal for Aziky's benchmark kernels and useful on the current Intel Kaby Lake target. The host supports AVX2, BMI1/BMI2, POPCNT, and scalar x86-64 integer instructions. It does not support AVX-512.

The governing rule is semantic preservation: an optimization must retain every workload-defining iteration, state transition, memory access, and result. A compact checksum is an observation mechanism, not permission to replace a workload with its checksum.

## Corrected ISA facts

- AVX2 has packed 64-bit equality and signed-greater-than compares (`VPCMPEQQ`, `VPCMPGTQ`). It lacks the full predicate model introduced by AVX-512.
- AVX2 provides gathers for 32-bit and 64-bit elements with supported index widths. It does not provide scatter.
- AVX2 has packed low 32-bit multiplication (`VPMULLD`) and unsigned even-lane 32-by-32-to-64 multiplication (`VPMULUDQ`). It has no packed low 64-bit multiply.
- AVX-512F provides 64-bit gather/scatter and qword compress/expand. Native packed low 64-bit multiply requires AVX-512DQ. Vector qword popcount requires AVX-512VPOPCNTDQ, not AVX-512DQ alone.
- There is no general `_mm512_mulhi_epu64` intrinsic corresponding to one native packed unsigned high-half qword multiply.
- Gather/OR/scatter is not a correct general Bloom insertion algorithm. If two lanes address the same word, both lanes can read the old word and the later scatter can discard another lane's bit. Conflicting lanes must first be detected and combined, or handled by a correct scalar/atomic fallback.
- Gather/scatter intrinsic scale operands are address scales (1, 2, 4, or 8), not cache hints.

Primary references:

- Intel 64 and IA-32 Architectures Software Developer's Manual: https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html
- Intel Intrinsics Guide: https://www.intel.com/content/www/us/en/docs/intrinsics-guide/
- uops.info instruction measurements: https://uops.info/
- Agner Fog optimization manuals: https://www.agner.org/optimize/

## Bloom-filter findings

The benchmark uses a dependent 32-bit LCG to produce a 64-bit hash, then performs four irregular bitset probes. On Kaby Lake, four independent packed LCG streams would change the source recurrence. Exact leap-ahead states can expose independent work, but the extra coefficient materialization, lane setup, and extraction must earn their cost.

Credible scalar lowering:

1. Use native 32-bit multiply/add for arithmetic modulo 2^32.
2. Keep the LCG state in a register.
3. Fold the filter load and variable shift with BMI2 `SHRX`.
4. AND the four selected low bits and add the exact boolean result.
5. Retain a `BT`/`SETB` fallback when BMI2 is disabled.

This was implemented and measured. Alternating focused medians improved from 6.44–6.66 ms to 5.45–5.64 ms while preserving exit-code parity and all four probes.

Rejected after measurement:

- Four-query exact leap-ahead batching was bit-for-bit correct but neutral to slower on this target.
- Naive two-query scalar unrolling varied from a 1.9% regression to a 1.0% improvement depending on ordering. It retained two dependent LCG steps per query and was removed.
- AVX2 vector address generation plus scalar irregular loads adds packing/extraction overhead without vector memory operations that match the workload.
- AVX-512 scatter is unavailable on the target and would require explicit conflict handling even where available.

New exact recurrence result:

- The multiplier is odd, so the 32-bit LCG is reversible modulo every `2^k`.
- Composing `f²` advances directly between low halves of consecutive 64-bit hashes.
- Lane 3 needs only predecessor bits 4 through 9. The predecessor can therefore be reconstructed modulo `2^10` with inverse `197`, rather than executing the missing full LCG step.
- Two queries can then be software-pipelined with one loop-carried multiply per query while preserving the exact sequence and an exact odd-query tail.
- A differential test checks every demanded predecessor bit. The final proof-tightened 30-run pinned suite measured Aziky at `6.553 ms` versus C at `6.965 ms`.

## Hash-join findings

The query-side Bloom reducer rejects nearly every random probe. Consequently, the common path is LCG generation plus the first one or two Bloom tests; grouped table probing is rare. Compact control bytes and SIMD fingerprint masks remain sound table designs, but changing the table representation cannot close a common-path gap that occurs before the table is reached.

The emitted code revealed four general lowering problems:

1. The four-lane Bloom instruction materialized a boolean and lane counter into result thunks, then a following instruction immediately branched on the boolean.
2. Source initializers overwritten by the fused Bloom instruction remained in the hot loop.
3. Exact modulo-2^32 affine chains always round-tripped through `RAX` instead of computing into the allocated destination register.
4. A dead `shift` temporary forced extra moves before an immediately following OR composition.

Implemented corrections:

- Fuse a fixed-group bit-test result with a following boolean branch only when the result has no later read and every memory and control-flow precondition is proven. A scalar backend may then branch directly on the failed bit test.
- Eliminate overwritten `Mov` instructions only along straight-line code, stopping at reads and every control-flow boundary, and remap all targets deterministically.
- Emit three-operand 32-bit `IMUL` and 32-bit `ADD` directly into a register-allocated affine destination, with the stack fallback retained.
- Reuse a dead input register for `shift-left` followed by OR only after proving the shifted temporary and original input are not read before redefinition.
- Remove redundant explicit `& 63` on bit indices because register-form `BT` masks the bit index modulo 64 by architectural definition.

The full pinned suite measured hash join at 5.216 ms for Aziky versus 5.415 ms for optimized C. A focused 100-run sequence measured 5.188–5.224 ms for Aziky versus 5.693 ms for C in the same thermal sequence. Timing variance is significant, so both the full-suite result and focused A/B evidence are retained.

Compact table work:

- Packing semantic `u64` control values into bytes is now implemented as a general `PackedBytes` representation. It is selected only for non-escaping, non-conservatively-aliased objects whose every store is proven byte-bounded. Loads zero-extend, so source-level `u64` semantics remain unchanged.
- Loop phis may conservatively erase the fingerprint range. The representation proof recovers it only from a unique static definition that dominates the store; this is a legality proof, not a hash-join pattern match.
- Grouped control probing has a contiguous 16-byte `PCMPEQB`/`PMOVMSKB` path and a wrapped scalar fallback. The benchmark's Bloom reducer still makes table probing rare, so this transform is architectural groundwork rather than the main observed speedup.
- Batched multi-probe hash lookup can expose memory-level parallelism, but it must preserve probe order, collision semantics, and compaction order. It belongs after a general lane/conflict model exists.

The final proof-tightened 30-run pinned suite measured hash join at `5.401 ms` for Aziky versus `6.165 ms` for C. Because full-suite position still affects frequency, an alternating 100-run A/B/A/B check was also run: Aziky measured `5.525/5.608 ms` and C `5.897/6.044 ms`, a repeatable local advantage of `6.7–7.8%`. This still must not be generalized without PMU and cross-machine evidence; no benchmark-only shortcut is warranted.

## Full-width integrity findings

The earlier seven-bit process-exit comparison was insufficient. Every benchmark peer now has a verification build that writes the complete final workload value as eight little-endian bytes, and the harness refuses to time a scenario until all three values match.

This gate found two real hidden-state discrepancies:

1. Affine-index composition preserved only the low bits admitted by the exit mask. The lowered kernel now carries the source state mask and applies it after every exact composed chunk and scalar tail.
2. Sort-window demanded-bit lowering truncated the final loop-carried state to 32 bits. Timed and verification Aziky binaries now use the same explicit full-checksum preservation contract, so workload state remains live even when the OS exit status is narrow.

These repairs slightly constrain optimization freedom, but they make benchmark comparisons substantially more credible.

## Measured PGO and candidate control

- `--profile-instrument` adds exact 64-bit basic-block counters only to training binaries.
- Training records carry an `AZKPGO1` header and block count and are emitted to stderr, keeping program stdout independent.
- `profile-merge` validates record size and CFG-template topology before producing a deterministic profile.
- A measured hash-join profile caused 111 block reorders and 9 branch inversions, but regressed the 30-run median from `6.173 ms` to `6.771 ms`; it was correctly rejected as the default.
- The target tournament compares native, no-BMI2, and scalar variants only after full-checksum equality. Bloom medians were `6.729`, `7.025`, and `7.637 ms`, respectively, so native remained the winner.
- The baseline target no longer advertises AVX-512 on the Kaby Lake host.

## General lowering principles established

- Prefer exact width-aware scalar instructions over SIMD when the recurrence is serial or the memory operation is irregular.
- Fuse a producer with its only branch consumer instead of materializing booleans, but only with explicit liveness proof.
- Treat architectural modulo behavior (`BT` bit-index masking and 32-bit zero-extension) as a legal strength reduction when it exactly matches the language operation.
- Direct instruction selection into allocated registers is often more valuable than adding wider vectors.
- Measure candidates in both execution orders, preserve a feature-disabled fallback, and remove transformations whose benefit is within noise.
- Never specialize on a benchmark filename, final checksum, or known answer. Match typed semantics and prove dead values or bounded indices.

## Verification state

- 284 unit tests pass.
- Formatting and whitespace checks pass.
- Full 64-bit Aziky/Rust/C results match for all seven benchmark scenarios before timing.
- Workload-preservation tests cover Bloom build/query loops, all four lanes, BMI2 fallback, classic-Bloom branch legality, ring initialization/stores, and the new dead-value fusions.
- The final proof-tightened 30-run pinned suite (10 warmups, CPU 2) records Rust/Aziky geomean `1.582x` and C/Aziky geomean `1.192x`; all seven scenarios beat both optimized peers in that local sample. Narrow margins remain subject to PMU, interleaved, and cross-machine validation.
