# SIMD Vectorization Strategies for Aziky Benchmarks

## 1. LCG Unrolling into N Independent SIMD Streams

### Mathematical Foundation

The LCG recurrence is: `s_{n+1} = (a * s_n + c) mod m`

After k steps, the state is: `s_{n+k} = A_k * s_n + C_k mod m`

Where:
- `A_k = a^k mod m`
- `C_k = c * (a^k - 1) / (a - 1) mod m` (geometric series)

**Key insight**: We can precompute `A_k` and `C_k` for a chosen unroll width `k`, then run `k` independent streams simultaneously in SIMD lanes.

### Practical Implementation (AVX2 - 4x unroll)

For the benchmark's LCG: `a = 1664525, c = 1013904223, m = 2^32`

```c
#include <immintrin.h>
#include <stdint.h>

// Precompute jump-ahead constants for 4-stream unroll
// s_{n+4} = A4 * s_n + C4 mod 2^32
static const uint64_t A4 = 1664525ULL * 1664525ULL * 1664525ULL * 1664525ULL; // a^4 mod 2^32
// C4 = c * (a^3 + a^2 + a + 1) mod 2^32
static const uint64_t C4 = 1013904223ULL * (1664525ULL*1664525ULL*1664525ULL + 
                           1664525ULL*1664525ULL + 1664525ULL + 1); // mod 2^32

// AVX2 vectorized 4-stream LCG
// Each __m256i holds 4 x 32-bit states (one per stream)
static inline __m256i lcg_step_avx2(__m256i* states) {
    __m256i a_vec = _mm256_set1_epi32((int32_t)1664525);
    __m256i c_vec = _mm256_set1_epi32((int32_t)1013904223);
    __m256i mask  = _mm256_set1_epi32((int32_t)0xFFFFFFFF);
    
    // states = (states * a + c) & 0xFFFFFFFF
    *states = _mm256_and_si256(
        _mm256_add_epi32(
            _mm256_mullo_epi32(*states, a_vec),
            c_vec
        ),
        mask
    );
    return *states;
}

// Alternative: precomputed 4-step jump (avoids dependency chain)
static inline __m256i lcg_jump4_avx2(__m256i* states) {
    __m256i a4_vec = _mm256_set1_epi32((int32_t)A4);
    __m256i c4_vec = _mm256_set1_epi32((int32_t)C4);
    __m256i mask   = _mm256_set1_epi32((int32_t)0xFFFFFFFF);
    
    *states = _mm256_and_si256(
        _mm256_add_epi32(
            _mm256_mullo_epi32(*states, a4_vec),
            c4_vec
        ),
        mask
    );
    return *states;
}
```

### AVX-512 Version (8x unroll)

```c
static inline __m512i lcg_step_avx512(__m512i* states) {
    __m512i a_vec = _mm512_set1_epi32((int32_t)1664525);
    __m512i c_vec = _mm512_set1_epi32((int32_t)1013904223);
    __m512i mask  = _mm512_set1_epi32((int32_t)0xFFFFFFFF);
    
    *states = _mm512_and_epi32(
        _mm512_add_epi32(
            _mm512_mullo_epi32(*states, a_vec),
            c_vec
        ),
        mask
    );
    return *states;
}
```

### Mathematical Constraints

1. **Period preservation**: Each stream must produce a different subsequence of the full period. Using `A_k = a^k mod m` ensures stream `i` starts at `s_{i*k}` in the original sequence.

2. **Correlation avoidance**: For `m = 2^32` (power of 2), the LCG has well-known poor high-bit correlation. The benchmark masks to 32 bits (`& 0xffffffff`), so only the low 32 bits matter.

3. **Jump-ahead computation**: For `m = 2^32`, modular exponentiation is straightforward since we only need 32-bit arithmetic. For prime moduli, use binary exponentiation.

4. **Stream independence**: The `k` streams are mathematically independent subsequences. They never overlap because they're spaced `k` apart in the original sequence.

### Performance Notes

- **AVX2 `_mm256_mullo_epi32`**: 3-cycle latency, 1-cycle throughput on Haswell+
- **Dependency chain**: The LCG has a true dependency on `state`. Jump-ahead breaks this by computing `A_k * s + C_k` which only depends on `s` from `k` steps ago.
- **Expected speedup**: ~3.5-4x on AVX2, ~7-8x on AVX-512 for the LCG portion.

---

## 2. Bloom Filter: Scatter/Broadcast OR with Bitmasks

### The Pattern in bloom_filter.c

The bloom filter uses a "split block" design with 256 uint64 words (16384 bits total). Each hash produces 4 bit positions to set/check:

```c
// Current scalar: set bit at arbitrary position
for (uint64_t lane = 0; lane < 4; ++lane) {
    uint64_t word = (hash >> (lane * 8)) & 255;
    uint64_t bit  = (hash >> (lane * 11 + 3)) & 63;
    bloom[word] |= 1ULL << bit;
}
```

### AVX2 Approach: Process Multiple Hashes in Parallel

The key insight: process 4 hashes simultaneously, each setting 4 bits = 16 bit-set operations per iteration.

```c
#include <immintrin.h>

// Process 4 hashes at once, setting bits in the bloom filter
static inline void bloom_add_4x_avx2(uint64_t bloom[256], 
                                      const uint64_t hashes[4]) {
    // Extract word indices for all 4 hashes × 4 lanes = 16 word indices
    // hash[i] -> word[i][lane] = (hashes[i] >> (lane*8)) & 255
    
    for (uint64_t lane = 0; lane < 4; ++lane) {
        // Extract word indices for this lane from all 4 hashes
        uint64_t w0 = (hashes[0] >> (lane * 8)) & 255;
        uint64_t w1 = (hashes[1] >> (lane * 8)) & 255;
        uint64_t w2 = (hashes[2] >> (lane * 8)) & 255;
        uint64_t w3 = (hashes[3] >> (lane * 8)) & 255;
        
        // Extract bit indices for this lane from all 4 hashes
        uint64_t b0 = 1ULL << ((hashes[0] >> (lane * 11 + 3)) & 63);
        uint64_t b1 = 1ULL << ((hashes[1] >> (lane * 11 + 3)) & 63);
        uint64_t b2 = 1ULL << ((hashes[2] >> (lane * 11 + 3)) & 63);
        uint64_t b3 = 1ULL << ((hashes[3] >> (lane * 11 + 3)) & 63);
        
        // Problem: words w0,w1,w2,w3 are arbitrary indices in [0,255]
        // We can't use SIMD scatter for OR (no scatter-OR instruction)
        // Must use scalar OR for each word
        bloom[w0] |= b0;
        bloom[w1] |= b1;
        bloom[w2] |= b2;
        bloom[w3] |= b3;
    }
}
```

### Better Approach: Reorganize for SIMD-Friendly Access

Since the bloom filter has 256 words and we're accessing arbitrary positions, the real SIMD opportunity is in the **query** path (checking membership), not insertion:

```c
// Bloom query: check if ALL 4 bits are set (short-circuit on first miss)
static inline uint64_t bloom_maybe_4x_avx2(const uint64_t bloom[256], 
                                             uint64_t hash) {
    // Load all 256 words into SIMD registers (4 × __m256i = 8 words each)
    // Then use gather to check specific positions
    
    // But gather is slow! Better approach: batch multiple queries
    
    // For a single query with 4 bit positions:
    uint64_t hit = 1;
    for (uint64_t lane = 0; lane < 4; ++lane) {
        uint64_t word = (hash >> (lane * 8)) & 255;
        uint64_t bit  = (hash >> (lane * 11 + 3)) & 63;
        hit &= (bloom[word] >> bit) & 1ULL;
        if (!hit) return 0;  // short-circuit
    }
    return hit;
}
```

### AVX-512 Approach: Scatter for Insertion

AVX-512 introduces `_mm512_i32scatter_epi64` which can write to arbitrary positions:

```c
#include <immintrin.h>

// AVX-512 scatter-based bloom insert for 4 hashes
static inline void bloom_add_4x_avx512(uint64_t bloom[256], 
                                        const uint64_t hashes[4]) {
    // For each lane (0-3), we want to set bits at positions:
    // word0 = (hashes[0] >> (lane*8)) & 255, bit0 = ...
    // word1 = (hashes[1] >> (lane*8)) & 255, bit1 = ...
    // word2 = (hashes[2] >> (lane*8)) & 255, bit2 = ...
    // word3 = (hashes[3] >> (lane*8)) & 255, bit3 = ...
    
    // Problem: scatter writes full values, we need OR (read-modify-write)
    // AVX-512 doesn't have scatter-OR!
    
    // Solution: Use gather-load → OR → scatter-store
    for (uint64_t lane = 0; lane < 4; ++lane) {
        __m256i words = _mm256_set_epi64x(
            (hashes[3] >> (lane * 8)) & 255,
            (hashes[2] >> (lane * 8)) & 255,
            (hashes[1] >> (lane * 8)) & 255,
            (hashes[0] >> (lane * 8)) & 255
        );
        __m256i bits = _mm256_set_epi64x(
            1ULL << ((hashes[3] >> (lane * 11 + 3)) & 63),
            1ULL << ((hashes[2] >> (lane * 11 + 3)) & 63),
            1ULL << ((hashes[1] >> (lane * 11 + 3)) & 63),
            1ULL << ((hashes[0] >> (lane * 11 + 3)) & 63)
        );
        
        // Gather existing values
        __m256i existing = _mm256_i32gather_epi64(
            (const long long*)bloom, words, 8);
        
        // OR with new bits
        __m256i updated = _mm256_or_si256(existing, bits);
        
        // Scatter back (AVX-512 only)
        _mm256_i32scatter_epi64((long long*)bloom, words, updated, 8);
    }
}
```

### Practical Reality

For the bloom filter pattern:
- **Insertion**: Scalar is often faster than SIMD because the 4 bit positions per hash are likely in different cache lines, making scatter/gather expensive.
- **Query**: SIMD can help by batching multiple queries and using the bloom filter's structure for early termination.
- **Best approach**: Process multiple hashes' bloom operations together, using the fact that the 256-word array fits in L1 cache (2KB).

---

## 3. Ring Buffer Scatter: AVX2 Gather vs AVX-512 Scatter

### The Pattern in ring_write.c

```c
// Ring buffer: write to position (i & 63) in a 64-element array
buf[i & mask] = (state << 32) | state;
```

### AVX2: `_mm256_i32gather_epi64` (No native scatter)

AVX2 has **gather** but **no scatter**. For scatter, you must use scalar stores or emulate with shuffle+store.

```c
#include <immintrin.h>

// AVX2 "scatter" emulation for ring buffer writes
// Write 8 values to positions given by indices[8]
static inline void scatter_8x_avx2(uint64_t* buf, 
                                    const uint64_t values[8],
                                    const uint32_t indices[8]) {
    // Option 1: Scalar stores (usually fastest for small counts)
    for (int i = 0; i < 8; i++) {
        buf[indices[i]] = values[i];
    }
    
    // Option 2: If indices are contiguous or have a pattern, use masked stores
    // (e.g., _mm256_maskstore_epi64)
}

// AVX2 gather for ring buffer reads
static inline __m256i gather_8x_avx2(const uint64_t* buf, 
                                      const uint32_t indices[8]) {
    __m256i idx = _mm256_loadu_si256((const __m256i*)indices);
    return _mm256_i32gather_epi64((const long long*)buf, idx, 8);
}
```

### AVX-512: `_mm512_i32scatter_epi64` (Native scatter)

AVX-512 provides native scatter instructions:

```c
#include <immintrin.h>

// AVX-512 native scatter for ring buffer writes
static inline void scatter_8x_avx512(uint64_t* buf,
                                       const uint64_t values[8],
                                       const uint32_t indices[8]) {
    __m256i idx = _mm256_loadu_si256((const __m256i*)indices);
    __m512i vals = _mm512_loadu_si512((const __m512i*)values);
    
    // Write values to buf[indices[i]] for i = 0..7
    _mm512_i32scatter_epi64((long long*)buf, idx, vals, 8);
}

// AVX-512 native gather for ring buffer reads
static inline __m512i gather_8x_avx512(const uint64_t* buf,
                                         const uint32_t indices[8]) {
    __m256i idx = _mm256_loadu_si256((const __m256i*)indices);
    return _mm512_i32gather_epi64((const long long*)buf, idx, 8);
}
```

### Performance Characteristics

| Instruction | Latency | Throughput | Notes |
|------------|---------|------------|-------|
| AVX2 `_mm256_i32gather_epi64` | ~12-20 cycles | 1/5 | Depends on cache hits |
| AVX-512 `_mm512_i32gather_epi64` | ~12-20 cycles | 1/5 | Similar to AVX2 |
| AVX-512 `_mm512_i32scatter_epi64` | ~12-20 cycles | 1/8 | Writeback, cache line splits |
| Scalar store | 1 cycle | 1/cycle | For L1-hitting addresses |

### Key Findings

1. **AVX2 has no scatter**: Must use scalar stores or `_mm256_maskstore_epi64` (which requires a mask, not indices).

2. **Gather/Scatter are slow**: ~12-20 cycles latency. For a 64-element ring buffer, all indices hit L1 cache, so the bottleneck is instruction latency, not memory.

3. **Scalar is often faster for small buffers**: For a 64-element ring buffer, scalar stores to L1 are ~1 cycle each. Scatter is ~12-20x slower per element.

4. **Best use case**: Gather/scatter shine when:
   - Buffer is large (spans multiple cache lines)
   - You can pipeline multiple gather/scatter operations
   - The index pattern is regular enough for the hardware to optimize

5. **For ring_write.c**: The ring buffer is only 64 × 8 = 512 bytes (fits in L1). Scalar writes are likely faster than SIMD scatter. The SIMD opportunity is in **batching multiple LCG steps** and doing the ring writes scalar.

### Alternative: Blocked Ring Buffer Writes

```c
// Instead of writing one element per LCG step, batch 8 writes:
static inline void ring_write_batched(uint64_t buf[64], uint64_t* state) {
    uint64_t states[8];
    __m256i vstates = lcg_step_avx2((__m256i*)states);  // 8 LCG steps
    
    // Extract and write (scalar for small ring buffer)
    for (int i = 0; i < 8; i++) {
        uint64_t val = (states[i] << 32) | states[i];
        buf[(i + *state) & 63] = val;  // offset by iteration count
    }
    *state = states[7];  // advance state
}
```

---

## 4. Hash Table Lookups: SwissTable SIMD Patterns

### The Pattern in hash_join.c

The benchmark uses a SwissTable-like design with:
- `ctrl[256]`: Control bytes (h2 hash fragments + empty/deleted markers)
- `keys[256]`: Key storage
- `vals[256]`: Value storage
- Group size: 16 elements (GROUP = 16)

### SwissTable Core SIMD: Ctrl Group Matching

From the cwisstable implementation (the actual Google SwissTable code):

```c
// SSE2-based group matching (16 control bytes at once)
typedef __m128i CWISS_Group;
#define CWISS_Group_kWidth 16

// Load 16 control bytes
static inline CWISS_Group CWISS_Group_new(const int8_t* pos) {
    return _mm_loadu_si128((const CWISS_Group*)pos);
}

// Match: find all slots where ctrl matches h2
// Uses: load 16 bytes → broadcast h2 → compare → movemask → bitmask
static inline uint64_t CWISS_Group_Match(const CWISS_Group* self, uint8_t hash) {
    return _mm_movemask_epi8(
        _mm_cmpeq_epi8(
            _mm_set1_epi8((char)hash),   // broadcast h2 to all 16 lanes
            *self                         // load 16 ctrl bytes
        )
    );
    // Returns 16-bit mask: bit i set if ctrl[i] == hash
}

// Match empty slots (ctrl byte == 0x80)
static inline uint64_t CWISS_Group_MatchEmpty(const CWISS_Group* self) {
    // Uses SSSE3 _mm_sign_epi8 trick: kEmpty = -128 = 0x80
    return _mm_movemask_epi8(_mm_sign_epi8(*self, *self));
}

// Match empty OR deleted slots
static inline uint64_t CWISS_Group_MatchEmptyOrDeleted(const CWISS_Group* self) {
    __m128i sentinel = _mm_set1_epi8((char)0xFF);  // kSentinel = -1
    // sentinel > *self means empty or deleted (both < -1)
    return _mm_movemask_epi8(_mm_cmpgt_epi8(sentinel, *self));
}
```

### The SwissTable Lookup Algorithm

```c
// Core probe loop (from cwisstable)
CWISS_Group g = CWISS_Group_new(ctrl + seq.offset);
CWISS_BitMask match = CWISS_Group_Match(&g, CWISS_H2(hash));

uint32_t i;
while (CWISS_BitMask_next(&match, &i)) {
    // Potential match at position i in group
    size_t idx = probe_seq_offset(&seq, i);
    if (keys[idx] == probe_key) {
        *out_val = vals[idx];
        return 1;  // found
    }
}
if (CWISS_Group_MatchEmpty(&g).mask) break;  // empty slot = not found
// advance to next group in probe sequence
```

### AVX-512 Enhancement: 32-byte Groups

```c
// AVX-512: 32-byte control groups
typedef __m256i CWISS_Group_AVX512;
#define CWISS_Group_kWidth_AVX512 32

static inline uint32_t CWISS_Group_Match_AVX512(const CWISS_Group_AVX512* self, 
                                                   uint8_t hash) {
    __m256i broadcast = _mm256_set1_epi8((char)hash);
    return _mm256_movemask_epi8(_mm256_cmpeq_epi8(broadcast, *self));
    // Returns 32-bit mask
}

static inline uint32_t CWISS_Group_MatchEmpty_AVX512(const CWISS_Group_AVX512* self) {
    __m256i broadcast = _mm256_set1_epi8((char)0x80);  // kEmpty
    return _mm256_movemask_epi8(_mm256_cmpeq_epi8(broadcast, *self));
}
```

### Hash Join SIMD Opportunities

The hash_join.c benchmark has these SIMD opportunities:

1. **Bloom filter query batching**: Process multiple probe keys against the bloom filter simultaneously
2. **Ctrl group matching**: Use SwissTable SIMD for the grouped_probe inner loop
3. **Key comparison**: After ctrl match, compare actual keys with SIMD
4. **Compaction**: The compact[] accumulation can use SIMD XOR reduction

```c
// SIMD-accelerated grouped probe
static inline int grouped_probe_simd(const uint64_t ctrl[256], 
                                      const uint64_t keys[256],
                                      const uint64_t vals[256],
                                      uint64_t probe_key, bool partitioned) {
    uint8_t fp = (uint8_t)(fp7(probe_key));
    
    // Load 16 ctrl bytes at once
    __m128i ctrl_vec = _mm_loadu_si128((const __m128i*)(ctrl + group_offset));
    
    // Match: find all slots with matching h2
    __m128i fp_vec = _mm_set1_epi8((char)fp);
    __m128i match_mask = _mm_cmpeq_epi8(ctrl_vec, fp_vec);
    uint32_t matches = _mm_movemask_epi8(match_mask);
    
    // Check each matching slot
    while (matches) {
        int i = __builtin_ctz(matches);
        matches &= matches - 1;  // clear lowest set bit
        
        size_t idx = group_offset + i;
        if (keys[idx] == probe_key) {
            return 1;  // found
        }
    }
    
    // Check for empty slot (early termination)
    __m128i empty_vec = _mm_set1_epi8((char)0x80);
    if (_mm_movemask_epi8(_mm_cmpeq_epi8(ctrl_vec, empty_vec))) {
        return 0;  // empty slot = not found
    }
    
    return -1;  // continue probing
}
```

### Performance: SwissTable SIMD Impact

From the cwisstable benchmarks and Abseil documentation:

- **SIMD ctrl matching**: Processes 16 bytes (16 slots) in ~3 cycles (load + cmp + movemask)
- **Without SIMD**: Process 16 slots in ~16 × (load + compare + branch) = ~48+ cycles
- **Speedup**: ~10-15x for the ctrl matching phase
- **Overall lookup speedup**: ~3-5x (ctrl matching is ~30-50% of lookup time)

---

## 5. Sort Window (bubble_sort8): SIMD Sorting Networks

### The Pattern in sort_window.c

```c
static inline void bubble_sort8(uint64_t arr[8]) {
    for (unsigned pass = 0; pass < 8; ++pass) {
        for (unsigned j = 0; j + 1 < 8 - pass; ++j) {
            if (arr[j] > arr[j + 1]) {
                uint64_t tmp = arr[j];
                arr[j] = arr[j + 1];
                arr[j + 1] = tmp;
            }
        }
    }
}
```

### AVX2 Sorting Network for 8 Elements

```c
#include <immintrin.h>

// Compare-and-swap for two __m256i vectors (4 elements each)
static inline void cmpwap_4x(__m256i* a, __m256i* b) {
    __m256i lo = _mm256_min_epu64(*a, *b);  // smaller values
    __m256i hi = _mm256_max_epu64(*a, *b);  // larger values
    *a = lo;
    *b = hi;
}

// Bitonic sort network for 8 uint64 values
static inline void sort8_avx2(uint64_t arr[8]) {
    __m256i a = _mm256_loadu_si256((const __m256i*)(arr));     // arr[0..3]
    __m256i b = _mm256_loadu_si256((const __m256i*)(arr + 4)); // arr[4..7]
    
    // Stage 1: Sort each half independently
    // (requires scalar or more complex SIMD for within-half sorting)
    
    // For a proper 8-element sort, use a sorting network:
    // Compare-exchange at specific pairs determined by the network
    
    // After sorting, store back
    _mm256_storeu_si256((__m256i*)(arr), a);
    _mm256_storeu_si256((__m256i*)(arr + 4), b);
}
```

### Practical Note

For 8-element sorting, the SIMD benefit is modest because:
- The data fits in 64 bytes (one cache line)
- Bubble sort's O(n²) is only 28 comparisons for n=8
- The real bottleneck is the LCG step, not the sort

---

## Summary: Recommended SIMD Strategy per Benchmark

| Benchmark | Current Bottleneck | SIMD Strategy | Expected Speedup |
|-----------|-------------------|---------------|-----------------|
| stream_lcg.c | LCG dependency chain | 4x/8x LCG unroll with jump-ahead | 3.5-8x |
| ring_write.c | LCG + ring scatter | LCG unroll + scalar ring writes | 3-4x |
| bloom_filter.c | LCG + bloom insert/query | LCG unroll + batch bloom ops | 3-5x |
| hash_join.c | LCG + bloom + hash probe | LCG unroll + SwissTable SIMD ctrl | 3-6x |
| sort_window.c | LCG + bubble sort | LCG unroll + sorting network | 3-4x |

### Priority Order for Implementation

1. **LCG unrolling** (all benchmarks): Highest impact, simplest to implement
2. **SwissTable SIMD ctrl matching** (hash_join.c): Second highest impact
3. **Bloom filter batching** (bloom_filter.c): Moderate impact
4. **Ring buffer**: Low impact (64-element buffer fits in L1)
5. **Sorting network**: Low impact for 8 elements
