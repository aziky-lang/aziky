# Aziky Owned-Data Runtime ABI

Status: implementation contract, 2026-07-15.

This document fixes the representation and safety rules for the next
allocator-backed `string`, `list<T>`, and `map<K, V>` tranche. It exists to keep
frontend types, runtime IR, generated code, and `std` on one ABI.

## Principles

- Ownership is explicit and statically unique. Raw pointer integers are not an
  ownership model.
- Cleanup is deterministic on every lexical and terminal control-flow edge.
- Length and capacity are distinct and always measured in elements; allocation
  size is tracked in bytes.
- Integer overflow, allocation failure, invalid release, and out-of-bounds
  access fail deterministically. None may become undefined behavior.
- Empty values use a null pointer with zero length, capacity, and allocation
  bytes; freeing them is a no-op.
- Optimizations may scalarize or stack-promote a value only after proving the
  observable ownership and cleanup behavior equivalent.

## Implemented Linear Allocation Foundation

The runtime-generic lowering currently represents a named `heap_alloc(size)`
owner with two private slots:

1. `ptr`: the allocation address, cleared after release.
2. `allocation_bytes`: the size captured before allocation and never recomputed
   from a mutable source expression.

The owner cannot be copied, converted, reassigned, passed as an ordinary scalar,
or fabricated from an integer. `heap_free(owner, asserted_size)` checks that the
owner is live and the assertion matches `allocation_bytes`, then releases and
clears it. Allocation failure and invalid release terminate with safety status
101 after cleaning other live owners.

Cleanup is emitted for normal scope fallthrough, both sides of conditionals,
loop iteration completion, `break`, `continue`, `return`, explicit/implicit
exit, assertion failure, and panic. Repeated compiler-inserted cleanup is safe
because the pointer slot is cleared immediately after the first release.

## Owned Sequence Descriptor

Owned UTF-8 strings and `list<T>` use four machine words:

| Word | Meaning |
|---|---|
| `ptr` | Allocation address, or null for the canonical empty value |
| `len` | Initialized elements (`string`: initialized UTF-8 bytes) |
| `capacity` | Elements available without growth (`string`: bytes) |
| `allocation_bytes` | Exact byte count required by the allocator release API |

The invariant is `len <= capacity`. For element size `E`, a non-empty list also
requires `capacity * E <= allocation_bytes`, checked with overflow-aware
multiplication. Strings additionally maintain valid UTF-8; character indexing
does not pretend byte offsets are character offsets.

Growth uses `max(required, max(4, capacity + capacity / 2))`, rounded to an
allocator size class when the runtime allocator provides one. The compiler must
check both the element-count addition and byte multiplication before allocating.
Growth is allocate-copy-commit-release: failure leaves the original owner
unchanged. Trivially copyable elements use bulk copy; resource-bearing elements
move individually and clear their old ownership state.

## Owned Map Descriptor

`map<K, V>` uses a control-byte table plus parallel key/value storage owned by
one logical map value. Its public descriptor carries length, growth threshold,
group mask, and exact allocation metadata for every backing region. Initial
lowering will use 16-byte control groups, deterministic probing, and a fixed
hash seed unless an explicit randomized map type is introduced.

Rehash follows the same transaction rule as sequence growth: allocate all new
regions, move initialized entries, commit the descriptor, then release old
regions. A failure before commit cleans only new regions and preserves the old
map. Key replacement drops the previous value exactly once.

## Required Implementation Order

1. Add a runtime-IR owned descriptor and allocator API that can allocate,
   release, and grow without exposing raw ownership as `u64`.
2. Lower empty and capacity-qualified `list<T>` construction for scalar `T`.
3. Implement checked `push`, `pop`, indexing, iteration, and scope cleanup.
4. Reuse the descriptor for owned UTF-8 strings, adding byte/character APIs.
5. Add control-group maps for scalar/string keys and resource-aware values.
6. Route the ABI through embedded `alloc`/`core`/`std`, then add filesystem and
   process resources only after ownership transfer is supported.

Each step requires positive execution tests, allocation-failure tests,
overflow/bounds tests, early-exit cleanup tests, optimizer-equivalence checks,
and the full-quality gate before it becomes part of the completed surface.
