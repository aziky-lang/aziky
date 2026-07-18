# x86-64 Loop Buffer & uop Cache Requirements

Research compiled from Agner Fog's microarchitecture manual, Intel optimization manuals, and web sources.

---

## 1. Intel Loop Stream Detector (LSD) / Loopback Buffer

The LSD (also called "loopback buffer") is a hardware mechanism that replays decoded µops from the µop queue (IDQ), bypassing both the legacy decoders and the µop cache. It exists to help tiny loops that don't fit the µop cache well.

### Intel Skylake+ (and Kaby Lake, Coffee Lake, Comet Lake)

| Property | Value |
|---|---|
| **Max µops** | **30 µops** (sometimes up to 40, but 30 is reliable) |
| **Queue source** | µop queue (IDQ), 64 entries per thread |
| **Max throughput** | 4 µops/cycle (stable, regardless of instruction length) |
| **Alignment** | None required — works from any address |
| **Max bytes** | No byte limit — purely µop-count based |
| **Fusion counting** | Fused instruction pairs count as 1 µop |

**Source**: Agner Fog, p. 153: *"Small loops of up to 30 µops, or sometimes up to 40, will benefit from the loop buffer. The loop buffer gives a stable throughput of 4 µops per clock."*

### Intel Ice Lake / Tiger Lake

| Property | Value |
|---|---|
| **Max µops** | **50 µops** |
| **Queue size** | 50 or 70 entries (variously reported) |
| **Max throughput** | **5 µops/cycle** |
| **Alignment** | None required |
| **Fusion counting** | Fused pairs count as **2** (changed from Skylake!) |

**Source**: Agner Fog, p. 164: *"Loops with up to 50 µops can run from the loop buffer. The loop buffer gives a stable throughput of 5 µops per clock."*

### Intel Alder Lake (P-core, Golden Cove)

Similar to Ice Lake with up to 6 µops/cycle throughput for loops in µop cache, loop buffer supports similar sizes to Ice Lake.

### Legacy Intel (for reference)

| Architecture | Max µops | Max bytes | Throughput |
|---|---|---|---|
| Core 2 | 18 instructions | 64 bytes (4×16B aligned blocks) | 32 bytes/cycle |
| Nehalem | 28 µops | 256 bytes (8×32B blocks) | 4 µops/cycle |
| Sandy Bridge | 28 µops | (from µop queue) | 4 µops/cycle |
| Haswell/Broadwell | 30-40 µops (56-entry queue) | (from µop queue) | 4 µops/cycle |
| **Skylake+** | **30 µops (64-entry queue)** | **N/A (µop count only)** | **4 µops/cycle** |
| **Ice Lake+** | **50 µops** | **N/A (µop count only)** | **5 µops/cycle** |

---

## 2. Intel µop Cache (DSB - Decoded Stream Buffer)

The DSB caches decoded µops, bypassing the legacy decoders when a loop fits. It's organized as a set-associative cache indexed by 32-byte-aligned code address blocks.

### Organization (Sandy Bridge through Skylake, same structure)

| Property | Value |
|---|---|
| **Total capacity** | **1,536 µops** |
| **Organization** | **32 sets × 8 ways × 6 µops per way** |
| **Line size** | **6 µops per way** |
| **Alignment requirement** | **32-byte aligned blocks** |
| **Max allocation per 32B block** | **3 lines of 6 µops each = 18 µops max per 32B block** |
| **Max throughput** | **4 µops/cycle** or 32 bytes of code/cycle |
| **SMT sharing** | Dynamically shared between hyperthreading threads |

### Ice Lake / Tiger Lake µop Cache

| Property | Value |
|---|---|
| **Total capacity** | **~2,000+ instructions** (larger than Skylake) |
| **Max throughput** | **5 µops/cycle** or 32 bytes/cycle |

### Key Alignment Rules (Critical for Compiler Emission)

The DSB has strict rules about what fills a cache line:

1. **32-byte boundary splitting**: A new µop cache line is started every time a 32-byte boundary is crossed, even if the previous line is only partially filled.
2. **Multi-µop instructions can't split lines**: If an instruction generating multiple µops can't fit in the current line, the rest of that line is wasted and the instruction starts a new line.
3. **Microcode ROM instructions** (>4 µops) use an entire µop cache line.
4. **Unconditional jumps/calls always end a µop cache line**.
5. **Same code can have multiple entries** if it has multiple jump entry points.
6. **Instructions needing >32 bits of storage** may take 2 entries and an extra cycle.
7. **Only 1 µop cache line can be read per cycle** — if many instructions use 2 entries each, this becomes a bottleneck.

### Practical Consequence for Compiler Emission

**32-byte alignment of the loop start is strongly recommended.** The loop's code should ideally start at a 32-byte aligned address. For small loops, fitting entirely within 1-2 cache lines of 6 µops each means the DSB can serve them at full 4 µops/cycle throughput.

---

## 3. AMD Zen Microarchitecture µop Cache and Loop Buffer

### AMD Zen 1

| Property | Value |
|---|---|
| **µop cache size** | **2,048 µops** (nominal), **32 sets × 8 ways × 8 µops/line** |
| **Effective single-thread** | ~1,200 µops (slightly more than half of nominal) |
| **Throughput from µop cache** | **5 instructions/cycle** (6 µops/cycle with µop fusion) |
| **Loop buffer** | Not documented separately — µop cache serves this role |
| **Max taken jumps** | 1/clock in general; tiny loops ≤5 instructions with no 64B boundary = 1 clock/iteration |

### AMD Zen 2

| Property | Value |
|---|---|
| **µop cache size** | **4,096 µops** (nominal), **line size = 8 µops** |
| **Effective single-thread** | ~2,200+ µops (slightly more than half) |
| **Throughput from µop cache** | **5 instructions/cycle** |
| **µop queue** | Dynamically sized, feeds rename/scheduler |
| **Max µops in flight** | 224 (scheduler) |
| **Fetch rate (fallback)** | 16 bytes/cycle from L1i cache |
| **Loop detection** | Tiny loops ≤5 instructions, no 64B boundary crossing = 1 cycle/iteration |
| **No separate LSD** | Uses µop cache as loop buffer |

### AMD Zen 3

| Property | Value |
|---|---|
| **µop cache size** | **4,096 µops** |
| **µop cache throughput** | **8 µops/cycle** (but limited to 6 by pipeline) |
| **Pipeline throughput** | **6 µops/cycle max** |
| **Fetch rate (fallback)** | 16 bytes/cycle |

### AMD Zen 4

| Property | Value |
|---|---|
| **µop cache size** | **6,912 µops** |
| **µop cache throughput** | **9 µops/cycle** (limited to 6 by pipeline) |
| **Pipeline throughput** | **6 µops/cycle** |
| **Fetch rate (fallback)** | 16 bytes/cycle |
| **Note** | AMD disabled Zen 4's separate loop buffer (µop cache serves this role) |

### AMD Zen 5

| Property | Value |
|---|---|
| **µop cache size** | **6,144 µops (16 ways)**, line size = 6 µops |
| **µop cache throughput** | **6 µops/cycle per branch** |
| **Pipeline throughput** | **8 µops/cycle** (first time AMD beats Intel!) |
| **Fetch rate** | **32 bytes/cycle** (doubled from Zen 4) |
| **Decode rate** | **6 instructions/cycle** (doubled from 4) |

### Key Intel vs AMD Differences

| Feature | Intel Skylake+ | AMD Zen2+ |
|---|---|---|
| **Separate LSD?** | Yes (replays from IDQ) | No (µop cache serves as loop buffer) |
| **µop cache size** | 1,536 µops | 4,096 (Zen2) → 6,912 (Zen4) |
| **µop cache line** | 6 µops | 8 µops (Zen1-2), 6 µops (Zen5) |
| **µop cache throughput** | 4 µops/cycle | 5 instr/cycle (Zen2), 6 µops/cycle (Zen3+) |
| **Loop buffer max** | 30 µops (Skylake), 50 (Ice Lake) | N/A (uses µop cache) |
| **32B alignment needed?** | Yes (for DSB filling) | Yes (for µop cache filling) |
| **Fallback decode** | 16B/cycle, 4 instr/cycle | 16B/cycle, 4 instr/cycle (Zen4-:), 32B/cycle (Zen5) |
| **SMT µop cache** | Statically partitioned | Competitive sharing |

---

## 4. What Causes Loops to NOT Be Detected / NOT Use µop Cache

### µop Cache (DSB) Failure Conditions (Intel)

1. **32-byte boundary crossing wastes cache lines**: Each 32-byte aligned block gets at most 3 lines of 6 µops. If a loop spans many 32B boundaries with sparse uops per block, effective capacity drops dramatically.

2. **Multi-µop instructions can't cross line boundaries**: An instruction generating 2+ µops that doesn't fit in the current line wastes the rest of that line.

3. **Unconditional jumps always end a µop cache line**: A `JMP` instruction forces a new line, wasting any remaining slots.

4. **Microcode instructions (>4 µops)** use an entire µop cache line.

5. **AH/BH/CH/DH register usage** (older Intel, pre-Skylake): Using high-byte registers could disable the loop buffer. On Skylake+, this was fixed but may still cause extra µops.

6. **Self-modifying code or cross-modifying code** invalidates the µop cache.

7. **Branches crossing 32-byte boundaries** create additional entries.

8. **Code running out of µop cache capacity** falls back to legacy decoders at 16B/cycle.

### Loopback Buffer (LSD) Failure Conditions (Intel)

1. **Loop exceeds µop count limit**: >30 µops (Skylake) or >50 µops (Ice Lake) prevents LSD activation.

2. **Branch misprediction / loop exit**: The LSD only works for the loop body; exiting requires re-fetch.

3. **The loop must be a "backwards jump" pattern**: The detector looks for repeated execution of the same µop sequence.

4. **Multiple taken branches** (historically >8 per loop on older designs, less restrictive on modern Intel).

### AMD µop Cache Failure Conditions

1. **Effective size is ~50% of nominal** when running single-threaded: The 4,096 µop Zen 2 cache has ~2,200 effective entries per thread.

2. **32-byte alignment** similarly required for optimal filling.

3. **Loops > effective µop cache size** fall back to legacy decoders at 16B/cycle (or 32B on Zen5).

4. **64-byte boundary crossing in tiny loops**: AMD's "tiny loop" fast path (1 cycle/iteration) requires the loop body to NOT cross a 64-byte aligned boundary.

---

## 5. Practical Recommendations for Compiler Emitting Raw x86-64

For tight ALU loops of ~50M iterations:

### Loop Size Budget
- **Target**: Keep loop body ≤30 µops for Intel LSD (Skylake), ≤50 µops (Ice Lake+)
- **For AMD**: Keep loop body within µop cache (easily satisfied with <2000 instructions)
- **Byte budget**: ~30 instructions × ~3 bytes avg = ~90 bytes typical for tight ALU loops

### Alignment
- **32-byte align the loop start** for optimal µop cache filling on BOTH Intel and AMD
- For very small loops (≤49 bytes), alignment is less critical (will fit regardless)
- Use NOP padding or `.p2align 5` (32-byte) before the loop

### Avoid
- Using AH/BH/CH/DH registers (causes extra µops or historical loop buffer issues)
- Instructions generating >4 µops (uses microcode ROM, wastes a µop cache line)
- Unconditional JMP within the loop body (wastes µop cache slots)
- Crossing 32-byte boundaries with multi-µop instructions
- Loop bodies crossing 64-byte boundaries (kills AMD's 1-cycle fast path)

### Throughput Targets
| Platform | Loop Buffer | µop Cache | Legacy Decode |
|---|---|---|---|
| Intel Skylake | 4 µops/cycle | 4 µops/cycle | 4 instr/cycle |
| Intel Ice Lake | 5 µops/cycle | 5 µops/cycle | 4 instr/cycle |
| AMD Zen 2 | N/A | 5 instr/cycle | 4 instr/cycle |
| AMD Zen 4 | N/A | 6 µops/cycle | 4 instr/cycle |
| AMD Zen 5 | N/A | 6 µops/cycle | 6 instr/cycle |
