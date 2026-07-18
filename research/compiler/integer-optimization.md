# Cranelift & General Compiler Integer Optimization Patterns for x86-64

Research compiled from Cranelift ISLE rules (wasmtime repo), Hacker's Delight, and LLVM pass documentation.

---

## 1. LOOP OPTIMIZATION PASSES (Ranked by Impact for Integer Workloads)

### 1.1 Loop-Invariant Code Motion (LICM) — HIGHEST IMPACT

**What it does:** Hoists computations that produce the same value on every iteration out of the loop body.

**Before:**
```
loop:
  x = a * 3          // invariant! 'a' doesn't change
  y = x + i          // 'i' changes each iteration
  ...
  goto loop
```

**After:**
```
x = a * 3            // hoisted
loop:
  y = x + i
  ...
  goto loop
```

**Why it matters for integer workloads:** Integer multiply/divide are expensive. If any multiply-by-constant or divide-by-constant appears in a loop body but its operands are loop-invariant, hoisting saves the cost per iteration.

**Implementation notes:**
- Requires dominator tree analysis
- Must check that the computation doesn't trap (integer division CAN trap on /0, so be careful with division LICM)
- Memory operations need alias analysis
- The loop header must dominate all exits

### 1.2 Induction Variable Optimization — HIGH IMPACT

**What it does:** Recognizes variables that change by a constant amount each iteration and simplifies comparisons/arithmetic involving them.

**Canonical form:** `iv = base + step * trip_count`

**Key transformations:**
1. **Normalize induction variables** — pick a canonical form (start at 0, step by 1)
2. **Eliminate redundant IVs** — if two IVs are linearly related, express one in terms of the other
3. **Strength-reduce IV comparisons** — compare against loop bounds instead of computing the final value

**Before:**
```
i = start
loop:
  addr = arr + i * 4    // multiply in loop
  ... = mem[addr]
  i += 1
  if i < end: goto loop
```

**After (IV opt + strength reduction):**
```
i = start
ptr = arr + start * 4   // hoisted base
loop:
  ... = mem[ptr]
  ptr += 4               // replace multiply with add
  i += 1
  if i < end: goto loop
```

**Cranelift note:** Cranelift doesn't currently do loop-level optimizations. This is a gap compared to LLVM. For a custom compiler targeting integer workloads, this is a major win.

### 1.3 Loop Unrolling — HIGH IMPACT for Integer Workloads

**What it does:** Duplicates the loop body to reduce loop overhead and expose more instruction-level parallelism.

**Before (unroll factor 4):**
```
loop:
  a[i] = b[i] + c[i]
  i += 1
  if i < N: goto loop
```

**After:**
```
loop:
  a[i]   = b[i]   + c[i]
  a[i+1] = b[i+1] + c[i+1]
  a[i+2] = b[i+2] + c[i+2]
  a[i+3] = b[i+3] + c[i+3]
  i += 4
  if i < N: goto loop
```

**Integer-specific benefits:**
- Eliminates branch prediction misses (loop back-edge)
- Exposes instruction-level parallelism for independent integer ops
- Enables further constant propagation within unrolled bodies
- **Sweet spot for integer workloads: unroll factor 2-4** (code size vs speed)

**Heuristics:**
- Don't unroll if loop body > ~30 instructions (icache pressure)
- Don't unroll if trip count is known small (< unroll factor)
- Prefer unrolling inner loops of nested loops

### 1.4 Strength Reduction in Loops — HIGH IMPACT

**What it does:** Replace expensive operations with cheaper equivalents, especially when applied to loop-varying quantities.

**This is where the multiply/divide-by-constant patterns shine inside loops:**

**Before:**
```
loop:
  y = x * 12          // multiply by constant
  z = x / 7           // divide by constant
  x += 1
  if x < limit: goto loop
```

**After:**
```
loop:
  y = x * 12           // LEA + ADD chain or LEA + shift
  z = x / 7            // mul by magic + shift
  x += 1
  if x < limit: goto loop
```

**The multiplication patterns below (Section 3) become even more valuable inside loops.**

### 1.5 Loop Fusion / Fission — MEDIUM IMPACT

**Loop Fusion (merge two loops with same bounds):**
```
// Before
for i in 0..N: a[i] = b[i] + 1
for i in 0..N: c[i] = a[i] * 2

// After (fusion)
for i in 0..N:
  a[i] = b[i] + 1
  c[i] = a[i] * 2
```
**Benefits:** Better cache locality, eliminates redundant loop overhead, enables further instruction combining.

**Loop Fission (split a hot loop):**
- Useful when a loop body is too large for unrolling
- Can help when parts of the loop have different alias properties

### 1.6 Recommended Pass Order for Integer Workloads

```
1. LICM              — hoist invariant computations
2. Induction Variable Opt — canonicalize and simplify IVs
3. Strength Reduction — replace expensive ops with cheap ones
4. Loop Unrolling     — duplicate body for ILP
5. LICM again         — new opportunities after unrolling
6. Constant Folding   — simplify within unrolled bodies
7. Dead Code Elimination — clean up dead code from simplifications
```

---

## 2. CRANELIFT-SPECIFIC INTEGER PEEPHOLE PATTERNS

### 2.1 Arithmetic Simplifications (from arithmetic.isle)

#### Identity Elimination
```
x + 0       →  x
x - 0       →  x
x * 1       →  x
x * 0       →  0
x - x       →  0
x * (-1)    →  -x
```

#### Double Negation
```
-(-x)       →  x
neg(x) * neg(y)  →  x * y
```

#### Arithmetic Identities
```
x + (-y)    →  x - y
x - (-y)    →  x + y
!x + 1      →  -x          (where ! = bnot)
!(x - 1)    →  -x
~x - ~y     →  y - x
```

#### Add/Sub Cancellation
```
(x - y) + y  →  x
(x + y) - y  →  x
(x + y) + (-y) → x
x - (x + y)  → -y
```

#### Add/Sub Association
```
(x + z) - (y + z)  →  x - y
(x - z) - (y - z)  →  x - y
(x - y) - (x - z)  →  z - y
((x - y) + (y + z)) → x + z
```

#### Algebraic Relations Between Arithmetic & Bitwise
```
(x + y) - (x | y)  →  x & y
(x | y) - (x & y)  →  x ^ y
(x + y) - (x & y)  →  x | y
x & (x | y)         →  x
x | (x & y)         →  x
```

#### Abs Identities
```
abs(-x)      →  abs(x)
abs(abs(x))  →  abs(x)
```

#### Min/Max Identities
```
min(x, x)         →  x
max(x, x)         →  x
min(x, y) + max(x, y) → x + y
max(x, y) >= x    →  true
min(x, y) <= x    →  true
min(max(x, y), y) → y
max(min(x, y), y) → y
min(min(x, y), max(x, y)) → min(x, y)
```

#### Multiply/Comparison Simplifications
```
(x * C) == D  →  x == (D/C)    when C is odd and divides D
```

#### Multiply by -1 Through Subtraction
```
(-x) * C  →  x * (-C)
```

#### Power-of-2 Multiply Detection
```
x * (1 << y)  →  x << y
```

#### Mulhi Pattern Recognition
```
// Detect open-coded mulhi: (x as big * y as big) >> bits
(sextend x) * (sextend y) >> 32  →  smulhi(x, y)
(uextend x) * (uextend y) >> 32  →  umulhi(x, y)
```

### 2.2 Bitwise Optimizations (from bitops.isle)

#### Identity
```
x | 0       →  x
x | x       →  x
x ^ 0       →  x
x ^ x       →  0
x & x       →  x
x & -1      →  x
x & 0       →  0
x ^ -1      →  ~x
```

#### Complement
```
~(~x)        →  x
```

#### De Morgan's Laws
```
~x & ~y      →  ~(x | y)
~x | ~y      →  ~(x & y)
```

#### Boolean Algebra Extensive Pattern Set
```
(x & y) ^ (x ^ y)    →  x | y
(x | y) ^ (x ^ y)    →  x & y
(x & y) + (x ^ y)    →  x | y
(x | y) + (x & y)    →  x + y
(x & y) | x          →  x
(x | y) & x          →  x
(x ^ y) ^ y          →  x
(x ^ y) ^ x          →  y
x | (x ^ y)          →  x | y

(x & y) | ~(x ^ y)   →  ~(x ^ y)
(x | y) & ~(x ^ y)   →  x & y

(x | y) & (x | ~y)   →  x
(x & y) | (x & ~y)   →  x

(x ^ y) | ~x         →  ~(x & y)

(x & ~y) | (~x & y)  →  x ^ y
(x & y) ^ (~x & y)   →  ~x & y

(x | y) ^ (x | ~y)   →  ~x

(x ^ z) | (y | x)    →  (y | x) | z

~((~x) & y)          →  x | ~y
~((~x) | y)          →  x & ~y

((~x) + y) with bnot  →  x - y

(x < y) | (x > y)    →  x != y
(x <_u y) | (x >_u y) → x != y
(x < y) | (x == y)    →  x <= y
(x <_u y) | (x == y)  →  x <=_u y
```

#### Byte Swap Recognition
```
// 32-bit byte swap pattern
(bor (ishl x 24)
     (ishl (band x 0xff00) 8))
(bor (band (ushr x 8) 0xff00)
     (ushr x 24))
→ bswap(x)

// 64-bit similar pattern
→ bswap(x)
```

### 2.3 Shift Optimizations (from shifts.isle)

#### Identity
```
x >> 0      →  x
x << 0      →  x
x rotr 0    →  x
x rotl 0    →  x
```

#### Round-trip Elimination
```
(x >> k) << k  →  x & mask    // clear bottom k bits
(x << k) >> k  →  x & mask    // clear top k bits (unsigned)
(x << k) >> k  →  sextend(x)  // sign-extend from narrow type
```

#### Shift Chaining
```
(x << k1) << k2  →  x << (k1 + k2)   if k1+k2 < bits
(x >> k1) >> k2  →  x >> (k1 + k2)   if k1+k2 < bits
(x << k1) << k2  →  0                 if k1+k2 >= bits
```

#### Rotate Simplification
```
rotl(rotr(x, y), y)  →  x
rotr(rotl(x, y), y)  →  x
(bor (ishl x k1) (ushr x k2))  →  rotl x k1    when k2 == bits - k1
```

#### Rotate Chaining
```
rotr(rotr(x, y), z)  →  rotr(x, y+z)
rotl(rotl(x, y), z)  →  rotl(x, y+z)
rotr(rotl(x, y), z)  →  rotl(x, y-z)
rotl(rotr(x, y), z)  →  rotr(x, y-z)
```

#### Distributive
```
band(ishl(x, z), ishl(y, z))  →  ishl(band(x, y), z)
iadd(ishl(x, z), ishl(y, z))  →  ishl(iadd(x, y), z)
isub(ishl(x, z), ishl(y, z))  →  ishl(isub(x, y), z)
ushr(band(ishl(x, y), z), y)  →  band(x, ushr(z, y))
```

#### Normalize Shift Amount
```
(x << k)  →  (x << (k & shift_mask))    // clamp shift amount
```

#### Eliminate Extend from Shift Amount
```
(x << (ireduce y))  →  (x << y)
(x << (uextend y))  →  (x << y)
// same for ushr, sshr, rotl, rotr
```

### 2.4 ICMP Optimizations (from icmp.isle)

#### Reflexive
```
x == x      →  true       (1)
x != x      →  false      (0)
x >  x      →  false
x >= x      →  true
// etc. for all comparisons
```

#### Comparison of Comparison Results
```
ne(icmp(cc, x, y), 0)  →  icmp(cc, x, y)
eq(icmp(cc, x, y), 0)  →  icmp(!cc, x, y)    // complement
eq(icmp(cc, x, y), 1)  →  icmp(cc, x, y)
```

#### Comparison Against Boundary Values
```
ult(x, 0)       →  false
uge(x, 0)       →  true
ule(x, 0)       →  (x == 0)
ugt(x, 0)       →  (x != 0)
ult(x, UMAX)    →  (x != UMAX)
ule(x, UMAX)    →  true
uge(x, UMAX)    →  (x == UMAX)
slt(x, SMIN)    →  false
sge(x, SMIN)    →  true
sgt(x, SMAX)    →  false
sle(x, SMAX)    →  true
```

#### Add/Sub in Comparisons
```
eq(a + k, b + k)  →  eq(a, b)
ne(a + k, b + k)  →  ne(a, b)
(a - b) == (c - d) → (a + d) == (c + b)
```

#### Comparison Folding with AND/OR
```
// When two comparisons share the same operands:
band(icmp(cc1, x, y), icmp(cc2, x, y))  →  compose_icmp(cc1 & cc2, x, y)
bor(icmp(cc1, x, y), icmp(cc2, x, y))   →  compose_icmp(cc1 | cc2, x, y)
```

#### Prefer Comparing Against Zero
```
uge(x, 1)    →  ne(x, 0)
ult(x, 1)    →  eq(x, 0)
sge(x, 1)    →  sgt(x, 0)
sgt(x, -1)   →  sge(x, 0)
```

### 2.5 Extend/Reduce Optimizations (from extends.isle)

#### Chained Extends
```
uextend(uextend(x))  →  uextend(x)
sextend(sextend(x))  →  sextend(x)
sextend(uextend(x))  →  uextend(x)
sextend(icmp(x))     →  uextend(icmp(x))    // icmp is 0 or 1
```

#### Reduce of Extend (identity)
```
ireduce(x, sextend(x))  →  x     // same type
ireduce(x, uextend(x))  →  x
```

#### Reduce of Extend (cross-type)
```
ireduce(tiny, sextend(large, x))  →  ireduce(tiny, x)   // if tiny < large
ireduce(tiny, uextend(large, x))  →  ireduce(tiny, x)   // if tiny < large
ireduce(med, sextend(tiny, x))    →  sextend(med, x)    // if med > tiny
ireduce(med, uextend(tiny, x))    →  uextend(med, x)    // if med > tiny
```

#### Push Bitwise Into Extends
```
band(uextend(x), uextend(y))  →  uextend(band(x, y))
bor(uextend(x), uextend(y))   →  uextend(bor(x, y))
bxor(uextend(x), uextend(y))  →  uextend(bxor(x, y))
```

#### Reduce Push-Down
```
ireduce(iadd(x, y))   →  iadd(ireduce(x), ireduce(y))
ireduce(isub(x, y))   →  isub(ireduce(x), ireduce(y))
ireduce(imul(x, y))   →  imul(ireduce(x), ireduce(y))
ireduce(bor(x, y))    →  bor(ireduce(x), ireduce(y))
ireduce(bxor(x, y))   →  bxor(ireduce(x), ireduce(y))
ireduce(band(x, y))   →  band(ireduce(x), ireduce(y))
ireduce(ineg(x))      →  ineg(ireduce(x))
ireduce(bnot(x))      →  bnot(ireduce(x))
```

### 2.6 Constant Propagation (from cprop.isle)

```
// All operations between constants are folded:
iadd(k1, k2)  →  k1 + k2
isub(k1, k2)  →  k1 - k2
imul(k1, k2)  →  k1 * k2
sdiv(k1, k2)  →  k1 / k2
udiv(k1, k2)  →  k1 / k2
bor(k1, k2)   →  k1 | k2
band(k1, k2)  →  k1 & k2
bxor(k1, k2)  →  k1 ^ k2
ishl(k1, k2)  →  k1 << k2
ushr(k1, k2)  →  k1 >> k2
sshr(k1, k2)  →  k1 >> k2
// etc.

// Canonicalize: push constants to the right
iadd(k, x)  →  iadd(x, k)
imul(k, x)  →  imul(x, k)
bor(k, x)   →  bor(x, k)
band(k, x)  →  band(x, k)
bxor(k, x)  →  bxor(x, k)

// Reassociate constants together
iadd(iadd(x, k1), k2)  →  iadd(x, k1+k2)
```

---

## 3. STRENGTH REDUCTION PATTERNS

### 3.1 Multiply by Constant → Shift+Add (WHAT YOU ALREADY HAVE + EXTENSIONS)

**Power-of-2 multiply:**
```
x * 2   →  x << 1          // or x + x
x * 4   →  x << 2
x * 8   →  x << 3
x * 16  →  x << 4
```

**What you might be MISSING — multiply by non-power-of-2 using LEA:**

```
x * 3   →  LEA(x, x, 1)           // x + x*2 = x*3   (1 instruction!)
x * 5   →  LEA(x, x, 2)           // x + x*4 = x*5
x * 9   →  LEA(x, x, 3)           // x + x*8 = x*9
x * 6   →  LEA(x*2, x*2, 1)       // x*2 + x*4 = x*6
x * 10  →  LEA(x, x, 2)           // x*5 * 2 = x*10  (LEA + shift)
x * 12  →  LEA(x, x, 1) then <<2  // x*3 * 4
```

**General algorithm for multiply by constant M (x86-64):**

1. Factor M into M = a * 2^k + b * 2^j where a, b ∈ {1, 2, 3, 5, 9}
2. Use LEA for each term (LEA can do base + index*scale where scale ∈ {2, 4, 8})
3. Chain with ADD/SHIFT

**More complex patterns:**
```
x * 7   →  LEA(x, x, 1) → LEA(_, x, 1)   // (x + x*2)*2 + x = x*7 (2 LEA)
x * 11  →  LEA(x, x, 2) → LEA(_, x, 0) + ADD  // x*5 + x = x*11 (2 LEA + ADD)
x * 15  →  LEA(x, x, 2) → LEA(_, x, 0) + SUB  // x*16 - x = x*15
```

**Cranelift approach (from arithmetic.isle):**
```
x * 2           →  x + x
x * (2^k)       →  x << k
x * (-1)        →  -x
x * (-C)        →  x * (-C)     // negation passed through
```

**For LEA-based multiply, the pattern to implement in your compiler:**
```
multiply_by_constant(x, C):
  if C == 0: return 0
  if C == 1: return x
  if C == -1: return -x
  if is_power_of_2(C): return x << log2(C)
  
  // Try LEA patterns (for C <= ~60 or so):
  // LEA supports: base + index * {2, 4, 8}
  // Try: find a, s such that C = a * s + b where s ∈ {2,4,8}
  // Then: LEA(x, x, s-1) gives x*s, then add b*x
  
  // Fallback: use multiply instruction
  return x * C
```

### 3.2 Division by Constant → Magic Number (WHAT YOU ALREADY HAVE + COMPLETE REFERENCE)

**From Cranelift's `div_const.rs` — Hacker's Delight algorithm:**

#### Unsigned Division by Constant

**Algorithm:** For unsigned x / d where d is not a power of 2:

Compute magic number `m = magic_u(d)` which gives:
- `mul_by`: the magic multiplier
- `do_add`: whether to use the "add" variant
- `shift_by`: the final shift amount

**x86-64 emission for unsigned x / d (no add variant):**
```asm
mov    rax, MAGIC          ; load magic number
mul    rdi                  ; unsigned multiply, result in RDX:RAX
shr    rdx, SHIFT           ; shift high half
; result in rdx
```

**x86-64 emission for unsigned x / d (add variant):**
```asm
mov    rax, MAGIC
mul    rdi
mov    rax, rdi
sub    rax, rdx
shr    rax, 1
add    rax, rdx
shr    rax, (SHIFT - 1)
; result in rax
```

**Example magic numbers (from Cranelift tests):**
```
d=3:  magic = 0xAAAAAAAB, do_add=false, shift=1
d=5:  magic = 0xCCCCCCCD, do_add=false, shift=2
d=6:  magic = 0xAAAAAAAB, do_add=false, shift=2
d=7:  magic = 0x24924925, do_add=true,  shift=3
d=10: magic = 0xCCCCCCCD, do_add=false, shift=3
d=100: magic = (complex),  shift=6
d=1000: magic = (complex), shift=9
```

#### Signed Division by Constant

**Algorithm:** For signed x / d where d is not ±1, 0, or a power of 2:

Compute `magic_s(d)` which gives:
- `mul_by`: the magic multiplier (may be negative)
- `shift_by`: the final shift amount

**x86-64 emission for signed x / d:**
```asm
mov    rax, MAGIC
imul   rdi                  ; signed multiply, result in RDX:RAX
; if d > 0 and magic < 0: add rdx, rdi
; if d < 0 and magic > 0: sub rdx, rdi
sar    rdx, SHIFT
; fixup: shr rdx, 63; add rdx, result  (rounds toward zero)
; result in rdx
```

**Example signed magic numbers (from Cranelift tests):**
```
d=3:   magic = 0x55555556, shift=0
d=5:   magic = 0x66666667, shift=1
d=7:   magic = 0x92492493, shift=2
d=10:  magic = 0x66666667, shift=2
d=-5:  magic = 0x99999999, shift=1
d=-3:  magic = 0x55555555, shift=1
```

#### Signed Division by Negative Power-of-2

```
x / (-8)  →  (x >> 2) → (x >> 30) → add → sar 3 → neg
// Cranelift: (sshr (iadd x (ushr (sshr x 2) 30)) 3) then negate
```

**Key implementation detail from Cranelift:**
```
// For signed power-of-2 division:
k = trailing_zeros(d)
t1 = sshr(x, k-1)                    // arithmetic right shift by k-1
t2 = ushr(t1, bits - k)              // unsigned right shift (get 0 or 1)
t3 = iadd(x, t2)                     // add correction
t4 = sshr(t3, k)                     // final shift
// For negative power of 2, also negate t4
```

### 3.3 Modulo by Constant → Division Remainder (EXTEND YOUR EXISTING)

**What you already have:**
```
x % (2^k)  →  x & ((2^k) - 1)     // unsigned only
```

**What you should add — general modulo by constant:**
```
x % d  →  x - (x / d) * d         // after division is strength-reduced
```

**x86-64 for unsigned x % d (no add variant):**
```asm
mov    rax, MAGIC
mul    rdi
shr    rdx, SHIFT           ; this is x / d
imul   rdx, DIVISOR         ; (x/d) * d
sub    rdi, rdx             ; x - (x/d)*d = x % d
; result in rdi
```

**Cranelift's approach:** Same magic number approach as division, but compute quotient then `x - quotient * d`.

### 3.4 Subtraction Patterns

```
x - x       →  0
x - 0       →  0 - x    (→ neg(x))
x - (-y)    →  x + y
```

### 3.5 Comparison Strength Reduction

```
x * C == D  →  x == (D/C)      when C is odd and D%C == 0
x * C != D  →  x != (D/C)      same conditions
```

**From Cranelift (with careful guards):**
```
// Only fires when C is odd (avoids overflow issues)
ne(imul(x, C), D) → ne(x, D/C)    if D%C==0 && C%2==1
eq(imul(x, C), D) → eq(x, D/C)    if D%C==0 && C%2==1
```

### 3.6 Bitwise ↔ Arithmetic Conversions

```
or(x, C) + (-C)    →  and(x, ~C)
(x + y) - (x | y)  →  x & y
(x | y) - (x & y)  →  x ^ y
(x | y) - (x ^ y)  →  x & y
(x & y) + (x ^ y)  →  x | y
(x | y) + (x & y)  →  x + y
```

### 3.7 Overflow Detection Patterns (for checked arithmetic)

```
// Detect unsigned overflow: (x + y) < x
// Detect signed overflow: ((x ^ s) & (x ^ result)) < 0 where s = sum
```

### 3.8 Division/Modulo by Special Values

```
x / 1       →  x
x % 1       →  0
x / -1      →  -x              (signed only, but note INT_MIN / -1 = INT_MIN)
x % -1      →  0               (signed)
x / INT_MIN →  (x == INT_MIN) ? 1 : 0   (signed edge case)
```

---

## 4. PATTERNS YOU'RE LIKELY MISSING

Based on comparing what you described having vs. what Cranelift implements:

### 4.1 Signed Division by Power of 2
```
// You have: udiv by power-of-2 → shift
// You're likely MISSING: sdiv by power-of-2 (more complex)

x / 4 (signed)  →  add correction then shift:
  t = x >> (bits-1)           // all 1s if negative, all 0s if positive
  x = x + (t >> (bits - k))   // add (1 << (bits-k)) to negative numbers
  result = x >> k              // arithmetic shift
```

### 4.2 Signed Modulo by Power of 2
```
x % 4 (signed)  →  more complex than unsigned version
  Uses the same correction as signed division, then AND + subtract
```

### 4.3 General Constant Division (Magic Number)
If you don't have the `magic_u32/u64/s32/s64` implementation from Hacker's Delight, this is a MAJOR gap. It replaces `div` instructions (20-90 cycles) with `mul` + `shr` (3-4 cycles).

### 4.4 Boolean Algebra → Arithmetic Conversions
```
(x & 1) + (y & 1)  →  (x + y) & 1    // for bit booleans
~x + ~y + 1         →  -(x + y) + 1   // De Morgan for comparisons
```

### 4.5 BSWAP Pattern Recognition
Recognize byte-swap patterns in input code and replace with `bswap` instruction:
```
(x << 24) | ((x & 0xff00) << 8) | ((x >> 8) & 0xff00) | (x >> 24)
→  bswap(x)
```

### 4.6 POPCNT Pattern Recognition
```
// If user computes popcount manually:
// Could recognize x & (x-1) patterns for bit manipulation
```

### 4.7 ABS Pattern
```
// x86-64: abs(x) = xor(sub(x, sar(x, 31)), sar(x, 31))   for 32-bit
// x86-64: abs(x) = cqo; xor(rax, rdx); sub(rax, rdx)      for 64-bit
```

### 4.8 Overflow-Safe Arithmetic
```
// Signed negation check: (x == INT_MIN) can't be negated
// Signed add overflow: add; into; seto
// Signed multiply overflow: imul; seto
```

---

## 5. CRANELIFT'S MAGIC NUMBER IMPLEMENTATION REFERENCE

The complete Cranelift magic number computation is in `div_const.rs`. Key functions:

```
magic_u32(d: u32) -> MU32 { mul_by: u32, do_add: bool, shift_by: i32 }
magic_u64(d: u64) -> MU64 { mul_by: u64, do_add: bool, shift_by: i32 }
magic_s32(d: i32) -> MS32 { mul_by: i32, shift_by: i32 }
magic_s64(d: i64) -> MS64 { mul_by: i64, shift_by: i32 }
```

The algorithm is O(bits) — it iterates at most 2*bits times. It's from "Hacker's Delight" by Henry Warren.

**Evaluation of magic numbers:**

Unsigned division (no add):
```
q = mulhi(n, magic)    // high half of unsigned n * magic
q >>= shift
return q
```

Unsigned division (add variant):
```
q = mulhi(n, magic)
t = (n - q) >> 1
q = (t + q) >> (shift - 1)
return q
```

Signed division:
```
q = mulhi(n, magic)    // high half of signed n * magic
if d > 0 and magic < 0: q += n
if d < 0 and magic > 0: q -= n
q >>= shift
// round toward zero:
t = q >> 31 (or 63)
q += t
return q
```

---

## 6. x86-64 INSTRUCTION MAPPING CHEAT SHEET

| Operation | x86-64 Instruction(s) | Latency (approx) |
|-----------|----------------------|-------------------|
| x + y | `add` | 1 cycle |
| x - y | `sub` | 1 cycle |
| x * 2^k | `shl` | 1 cycle |
| x * 3,5,9 | `lea rax,[rdi+rdi*N]` | 1 cycle |
| x * general | `imul` | 3 cycles |
| x / 2^k (unsigned) | `shr` | 1 cycle |
| x / 2^k (signed) | `sar` + correction | 3 cycles |
| x / C (unsigned) | `mov rax,M; mul rdi; shr rdx,S` | 4 cycles |
| x / C (signed) | `mov rax,M; imul rdi; sar rdx,S` | 5 cycles |
| x % 2^k (unsigned) | `and` | 1 cycle |
| x % C (unsigned) | div-like pattern | 6 cycles |
| x & y | `and` | 1 cycle |
| x \| y | `or` | 1 cycle |
| x ^ y | `xor` | 1 cycle |
| ~x | `not` | 1 cycle |
| x << k | `shl` | 1 cycle |
| x >> k (unsigned) | `shr` | 1 cycle |
| x >> k (signed) | `sar` | 1 cycle |
| -x | `neg` | 1 cycle |
| abs(x) | `cdq; xor; sub` | 3 cycles |
| clz(x) | `bsr` + fixup | 3 cycles |
| ctz(x) | `bsf` | 3 cycles |
| popcnt(x) | `popcnt` (SSE4.2+) | 3 cycles |
| bswap(x) | `bswap` | 1 cycle |
