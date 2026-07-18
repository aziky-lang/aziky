# x86-64 Store-to-Load Forwarding Requirements

## Source
Agner Fog's "The microarchitecture of Intel, AMD and VIA CPUs" (2026-05-23 update), supplemented by LLVM's `X86AvoidStoreForwardingBlocks.cpp`.

---

## 1. Minimum Store Size That Forwards

**The minimum store size is 1 byte (8 bits).**

On all modern Intel processors (Skylake+) and AMD Zen processors, a 1-byte store **can** be forwarded to a 1-byte load at the same address. The rule from Agner Fog's manual for Haswell/Broadwell/Skylake:

> "When a write of 64 bits or less is followed by a read of the same size and the same address, regardless of alignment."

This covers 8-bit, 16-bit, 32-bit, and 64-bit stores. All forward successfully when the load matches the store's size and starting address.

**Key rule**: The load must be **the same size or smaller** than the store, and must start at the **same address**.

---

## 2. Alignment Requirements for Forwarding

### Intel Skylake (and all Core processors since Nehalem)
- **64-bit or less store → same-size load**: Works **regardless of alignment**
- **64-bit or less store → smaller load (same address)**: Works **regardless of alignment**
- **128-bit store → same-size load**: Requires **full 16-byte alignment**
- **256-bit store → same-size load**: Requires **full 32-byte alignment** (Haswell/Broadwell); misaligned adds 3 cycle penalty on Skylake
- **Cache line crossing**: No extra penalty for ≤64-bit operands; 4-5 cycle penalty for larger operands when crossing a 64-byte cache line boundary

### AMD Zen 1/2/3
- **All sizes**: Works **regardless of alignment** with "little or no penalty" for unaligned accesses
- Exception: crossing a **memory page boundary** (4KB) may prevent forwarding
- Zen 3 partial overlap penalty: ~10 clock cycles

### Intel Ice Lake / Tiger Lake
- Best-case forwarding latency: **5 cycles** for ≤128-bit operands
- Cache line crossing penalty: **2 cycles**
- Misalignment penalty for 128+ bit: **2 cycles**

---

## 3. What Causes Store-to-Load Forwarding Failures

### Universal Failure Cases (all modern Intel + AMD)

| Failure Condition | Example |
|---|---|
| **Load larger than store** | `mov [esi], eax` then `movq xmm0, [esi]` — reads 8 bytes from 4-byte store |
| **Partial overlap** | `mov [esi], eax` then `mov [esi+2], cx` — load overlaps but doesn't match start |
| **Two non-overlapping stores merged** | `mov [esi], eax` + `mov [esi+4], edx` then `movq xmm0, [esi]` — read spans both |

### Intel-Specific Failure Cases

| Condition | Impact |
|---|---|
| 128-bit write → smaller read **crossing 64-bit half boundary** | Forward fails |
| 256-bit write → 128-bit read **crossing 128-bit half boundary** | Forward fails |
| 256-bit write → 64-bit read **crossing any 64-bit quarter boundary** | Forward fails |
| Cache line boundary crossing (64 bytes) | Forward stalls: 4-5 cycle penalty on Skylake; 2 cycles on Ice Lake |
| 128+ bit misaligned access | Forward stalls: up to 3 extra cycles on Skylake |

### AMD-Specific
- Page boundary crossing can cause issues
- Partial overlap penalty: 6-7 cycles (Zen 1/2), ~10 cycles (Zen 3)
- Otherwise, AMD Zen is very tolerant of misalignment

---

## 4. Latency Penalty: Forwarding Succeeds vs. Fails

### Intel Skylake (Skylake, Kaby Lake, Coffee Lake)
| Scenario | Latency (cycles) |
|---|---|
| **Successful forwarding, 32/64-bit** | **4** (best case) |
| **Successful forwarding, other sizes** | **5** |
| **Failed forwarding (standard)** | **~15** (4-5 base + 11 penalty) |
| **Failed forwarding (128+ bit misaligned ≥16B)** | **~50+** |
| **Cache line crossing, any size** | **+4-5 cycles** on top of base |
| **Read > write or partial overlap** | **~11 cycles extra** |

### Intel Ice Lake / Tiger Lake
| Scenario | Latency (cycles) |
|---|---|
| **Successful forwarding, ≤128-bit** | **5** |
| **Successful forwarding, 256/512-bit** | **7+** |
| **Failed forwarding** | **19-20** |
| **Fast forwarding (zero-latency)** | **0** (special cases: 8/32/64-bit, aligned, no cache line crossing, no rip-relative) |

### Intel Haswell / Broadwell
| Scenario | Latency (cycles) |
|---|---|
| **Successful forwarding, 64-bit or less** | **5** (same size, any alignment) |
| **Failed forwarding (standard)** | **~12 cycles extra** |
| **Failed forwarding (128/256-bit unaligned <16B)** | **~50 cycles** |
| **Failed forwarding (256-bit unaligned)** | **~210 cycles** (Ivy Bridge) |

### AMD Zen 1/2
| Scenario | Latency (cycles) |
|---|---|
| **Successful forwarding, any size** | **~4-5** |
| **Partial overlap (failed)** | **6-7 cycles extra** |
| **Page boundary crossing** | Potentially blocked |

### AMD Zen 3
| Scenario | Latency (cycles) |
|---|---|
| **Successful forwarding, any size** | **~4-5** |
| **Partial overlap (failed)** | **~10 cycles extra** |

### AMD Zen 2 Special: Memory Operand Mirroring
Zen 2 can achieve **zero-latency** store-to-load forwarding in certain patterns where the same memory address is stored then loaded with the same operand size. This is similar to (but different from) Intel Ice Lake's fast forwarding.

---

## 5. Summary Table for Compiler Writers

### Safe Forwarding Patterns (always works, no penalty)

| Store Size | Load Size | Alignment Requirement | Latency |
|---|---|---|---|
| 1 byte | 1 byte | None | 4-5 cycles |
| 2 bytes | 2 bytes | None | 4-5 cycles |
| 4 bytes | 4 bytes | None | 4-5 cycles |
| 8 bytes | 8 bytes | None | 4-5 cycles |
| 8 bytes | ≤8 bytes (same address) | None | 4-5 cycles |
| 16 bytes | 16 bytes | 16-byte aligned | 4-5 cycles |
| 16 bytes | 8 bytes (half) | 16-byte aligned, no 8B boundary cross | 4-5 cycles |
| 32 bytes | 32 bytes | 32-byte aligned | 4-5 cycles |

### Patterns That Cause Forwarding Failures

| Pattern | Penalty |
|---|---|
| Store 4B, load 8B at same address | +11-12 cycles |
| Two 4B stores, then 8B load spanning both | +11-12 cycles |
| Store 4B, load 4B at address+1 | +11-12 cycles |
| Store 16B unaligned, load 16B unaligned (<16B alignment) | +50+ cycles |
| Store 32B unaligned, load 32B unaligned | +210 cycles (Ivy Bridge) |

### Key Compiler Emit Guidelines

1. **Always match store/load sizes** when possible — same-size forwarding has the best success rate
2. **Avoid partial reads of stores** — never load a different size than what was stored at a different offset
3. **For struct copies**, emit same-size copies rather than merging multiple small stores into a large load
4. **Cache line alignment** matters: ensure ≥64B alignment for large (128/256-bit) SIMD stores
5. **Avoid unaligned 128/256-bit stores** if the same data will be read back — the penalty is catastrophic (50-210 cycles on older Intel)
6. **AMD Zen is more forgiving** of misalignment, but page boundary crossings can still block forwarding
7. **Ice Lake+ zero-latency forwarding** works for 8/32/64-bit aligned operands — useful for hot store-load pairs
