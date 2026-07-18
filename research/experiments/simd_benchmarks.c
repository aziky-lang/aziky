// simd_benchmarks.c - Test SIMD patterns for aziky benchmarks
// Compile: gcc -O3 -mavx2 -march=native simd_benchmarks.c -o simd_benchmarks
//          gcc -O3 -mavx512f -mavx512bw -march=native simd_benchmarks.c -o simd_benchmarks

#include <immintrin.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

// ============================================================================
// 1. LCG VECTORIZATION: 4-stream jump-ahead
// ============================================================================

#define LCG_A  1664525u
#define LCG_C  1013904223u

// Compute a^k mod 2^32
static inline uint32_t lcg_a_pow_k(uint32_t a, int k) {
    uint32_t result = 1;
    for (int i = 0; i < k; i++) result *= a;
    return result;
}

// Compute c * (a^(k-1) + ... + 1) mod 2^32
static inline uint32_t lcg_c_k(uint32_t a, uint32_t c, int k) {
    uint32_t sum = 0, ak = 1;
    for (int i = 0; i < k; i++) { sum += ak; ak *= a; }
    return c * sum;
}

// 4-stream LCG state
typedef struct {
    __m256i state;
    uint32_t a_k;
    uint32_t c_k;
} LCG4;

static inline LCG4 lcg4_init(uint32_t s0, uint32_t s1, uint32_t s2, uint32_t s3, int k) {
    LCG4 l;
    l.state = _mm256_setr_epi32(s0, s1, s2, s3, 0, 0, 0, 0);
    l.a_k = lcg_a_pow_k(LCG_A, k);
    l.c_k = lcg_c_k(LCG_A, LCG_C, k);
    return l;
}

// Advance 4 streams by k steps
static inline __m256i lcg4_step(LCG4* l) {
    __m256i a = _mm256_set1_epi32((int)l->a_k);
    __m256i c = _mm256_set1_epi32((int)l->c_k);
    __m256i mask = _mm256_set1_epi32((int)0xFFFFFFFF);
    l->state = _mm256_and_si256(
        _mm256_add_epi32(_mm256_mullo_epi32(l->state, a), c), mask);
    return l->state;
}

// Scalar LCG for comparison
static inline uint32_t lcg_scalar_step(uint32_t* state) {
    *state = (*state * LCG_A + LCG_C) & 0xFFFFFFFF;
    return *state;
}

// ============================================================================
// 2. SWISSTABLE: SIMD ctrl matching
// ============================================================================

#define CTRL_EMPTY    ((int8_t)0x80)
#define CTRL_DELETED  ((int8_t)0xFE)
#define CTRL_SENTINEL ((int8_t)0xFF)

// SSE2: Match h2 against 16 control bytes
static inline uint16_t ctrl_match_sse(const int8_t ctrl[16], uint8_t h2) {
    __m128i cv = _mm_loadu_si128((const __m128i*)ctrl);
    __m128i hv = _mm_set1_epi8((char)h2);
    return (uint16_t)_mm_movemask_epi8(_mm_cmpeq_epi8(cv, hv));
}

// SSE2: Match empty slots
static inline uint16_t ctrl_match_empty_sse(const int8_t ctrl[16]) {
    __m128i cv = _mm_loadu_si128((const __m128i*)ctrl);
    __m128i ev = _mm_set1_epi8(CTRL_EMPTY);
    return (uint16_t)_mm_movemask_epi8(_mm_cmpeq_epi8(cv, ev));
}

// AVX-512BW: Match h2 against 32 control bytes
#ifdef __AVX512BW__
static inline uint32_t ctrl_match_avx512(const int8_t ctrl[32], uint8_t h2) {
    __m256i cv = _mm256_loadu_si256((const __m256i*)ctrl);
    __m256i hv = _mm256_set1_epi8((char)h2);
    return (uint32_t)_mm256_movemask_epi8(_mm256_cmpeq_epi8(cv, hv));
}

static inline uint32_t ctrl_match_empty_avx512(const int8_t ctrl[32]) {
    __m256i cv = _mm256_loadu_si256((const __m256i*)ctrl);
    __m256i ev = _mm256_set1_epi8(CTRL_EMPTY);
    return (uint32_t)_mm256_movemask_epi8(_mm256_cmpeq_epi8(cv, ev));
}
#endif

// SwissTable probe using SIMD ctrl matching
static inline int swiss_probe_simd(const int8_t ctrl[256], 
                                    const uint64_t keys[256],
                                    const uint64_t vals[256],
                                    uint64_t probe_key, uint64_t* out_val) {
    uint64_t hash = probe_key * 0x9E3779B97F4A7C15ULL;
    uint8_t h2 = (uint8_t)(hash & 0x7F);
    
    for (uint64_t offset = 0; offset < 256; offset += 16) {
        uint16_t matches = ctrl_match_sse(ctrl + offset, h2);
        while (matches) {
            int i = __builtin_ctz(matches);
            matches &= matches - 1;
            size_t idx = offset + i;
            if (keys[idx] == probe_key) {
                *out_val = vals[idx];
                return 1;
            }
        }
        if (ctrl_match_empty_sse(ctrl + offset)) return 0;
    }
    return 0;
}

// Scalar probe for comparison
static inline int swiss_probe_scalar(const int8_t ctrl[256],
                                      const uint64_t keys[256],
                                      const uint64_t vals[256],
                                      uint64_t probe_key, uint64_t* out_val) {
    uint64_t hash = probe_key * 0x9E3779B97F4A7C15ULL;
    uint8_t h2 = (uint8_t)(hash & 0x7F);
    
    for (uint64_t offset = 0; offset < 256; offset += 16) {
        for (int i = 0; i < 16; i++) {
            size_t idx = offset + i;
            if (ctrl[idx] == CTRL_EMPTY) return 0;
            if (ctrl[idx] == (int8_t)h2 && keys[idx] == probe_key) {
                *out_val = vals[idx];
                return 1;
            }
        }
    }
    return 0;
}

// ============================================================================
// 3. BLOOM FILTER: Batched operations
// ============================================================================

static inline void bloom_insert_scalar(uint64_t bloom[256], uint64_t hash) {
    for (uint64_t lane = 0; lane < 4; ++lane) {
        uint64_t word = (hash >> (lane * 8)) & 255;
        uint64_t bit  = (hash >> (lane * 11 + 3)) & 63;
        bloom[word] |= 1ULL << bit;
    }
}

static inline int bloom_query_scalar(const uint64_t bloom[256], uint64_t hash) {
    for (uint64_t lane = 0; lane < 4; ++lane) {
        uint64_t word = (hash >> (lane * 8)) & 255;
        uint64_t bit  = (hash >> (lane * 11 + 3)) & 63;
        if (((bloom[word] >> bit) & 1) == 0) return 0;
    }
    return 1;
}

// ============================================================================
// 4. SORTING NETWORK: 8-element bitonic sort
// ============================================================================

static inline void cmpswap(uint64_t* a, uint64_t* b) {
    if (*a > *b) { uint64_t t = *a; *a = *b; *b = t; }
}

// Optimal 19-comparison sorting network for 8 elements
static inline void sort8_network(uint64_t arr[8]) {
    cmpswap(&arr[0], &arr[1]); cmpswap(&arr[2], &arr[3]);
    cmpswap(&arr[4], &arr[5]); cmpswap(&arr[6], &arr[7]);
    cmpswap(&arr[0], &arr[2]); cmpswap(&arr[1], &arr[3]);
    cmpswap(&arr[4], &arr[6]); cmpswap(&arr[5], &arr[7]);
    cmpswap(&arr[0], &arr[1]); cmpswap(&arr[2], &arr[3]);
    cmpswap(&arr[4], &arr[5]); cmpswap(&arr[6], &arr[7]);
    cmpswap(&arr[1], &arr[2]); cmpswap(&arr[5], &arr[6]);
    cmpswap(&arr[0], &arr[4]); cmpswap(&arr[1], &arr[5]);
    cmpswap(&arr[2], &arr[6]); cmpswap(&arr[3], &arr[7]);
    cmpswap(&arr[2], &arr[4]); cmpswap(&arr[3], &arr[5]);
    cmpswap(&arr[1], &arr[2]); cmpswap(&arr[3], &arr[4]);
    cmpswap(&arr[5], &arr[6]);
}

// Bubble sort for comparison (from benchmark)
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

// ============================================================================
// 5. RING BUFFER: Batched writes
// ============================================================================

static inline void ring_write_batch(uint64_t buf[64], uint64_t base_idx,
                                     const uint32_t states[8]) {
    for (int i = 0; i < 8; i++) {
        uint64_t val = ((uint64_t)states[i] << 32) | states[i];
        buf[(base_idx + i) & 63] = val;
    }
}

// ============================================================================
// BENCHMARKS
// ============================================================================

static uint64_t xorshift(uint64_t* state) {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    return *state;
}

int main(void) {
    const uint64_t ITERATIONS = 50000000ULL;
    
    // --- Benchmark 1: LCG scalar vs SIMD ---
    {
        uint32_t state_scalar = 123456789;
        uint32_t state_simd[4] = {123456789, 987654321, 42424242, 111111111};
        
        LCG4 lcg = lcg4_init(state_simd[0], state_simd[1], state_simd[2], state_simd[3], 1);
        
        uint64_t sum_scalar = 0, sum_simd = 0;
        
        for (uint64_t i = 0; i < ITERATIONS / 4; i++) {
            // Scalar: 4 steps
            for (int j = 0; j < 4; j++) {
                sum_scalar += lcg_scalar_step(&state_scalar);
            }
            // SIMD: 4 steps (one SIMD operation)
            __m256i s = lcg4_step(&lcg);
            uint32_t tmp[8];
            _mm256_storeu_si256((__m256i*)tmp, s);
            for (int j = 0; j < 4; j++) sum_simd += tmp[j];
        }
        
        printf("LCG: scalar=%lu, simd=%lu\n", 
               (unsigned long)sum_scalar, (unsigned long)sum_simd);
    }
    
    // --- Benchmark 2: SwissTable probe ---
    {
        int8_t ctrl[256];
        uint64_t keys[256], vals[256];
        memset(ctrl, CTRL_EMPTY, 256);
        
        // Insert some elements
        uint64_t state = 123456789;
        for (int i = 0; i < 100; i++) {
            state = (state * LCG_A + LCG_C) & 0xFFFFFFFF;
            uint64_t key = (state << 32) | state;
            uint64_t hash = key * 0x9E3779B97F4A7C15ULL;
            uint8_t h2 = (uint8_t)(hash & 0x7F);
            uint64_t h1 = (hash >> 7) % 256;
            
            // Find empty slot
            for (uint64_t j = 0; j < 256; j++) {
                uint64_t idx = (h1 + j) % 256;
                if (ctrl[idx] == CTRL_EMPTY) {
                    ctrl[idx] = (int8_t)h2;
                    keys[idx] = key;
                    vals[idx] = i;
                    break;
                }
            }
        }
        
        // Probe
        uint64_t found_scalar = 0, found_simd = 0;
        state = 987654321;
        
        for (uint64_t i = 0; i < 100000; i++) {
            state = (state * LCG_A + LCG_C) & 0xFFFFFFFF;
            uint64_t key = (state << 32) | state;
            uint64_t out;
            
            if (swiss_probe_scalar(ctrl, keys, vals, key, &out)) found_scalar++;
            if (swiss_probe_simd(ctrl, keys, vals, key, &out)) found_simd++;
        }
        
        printf("SwissTable probe: scalar=%lu, simd=%lu\n",
               (unsigned long)found_scalar, (unsigned long)found_simd);
    }
    
    // --- Benchmark 3: Sort ---
    {
        uint64_t arr1[8], arr2[8];
        uint64_t state = 42;
        
        for (int i = 0; i < 8; i++) {
            state = xorshift(&state);
            arr1[i] = arr2[i] = state & 0xFFFFFFFF;
        }
        
        bubble_sort8(arr1);
        sort8_network(arr2);
        
        int match = 1;
        for (int i = 0; i < 8; i++) {
            if (arr1[i] != arr2[i]) { match = 0; break; }
        }
        printf("Sort: %s\n", match ? "PASS" : "FAIL");
    }
    
    printf("All SIMD pattern tests completed.\n");
    return 0;
}
