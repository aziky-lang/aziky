# LLVM Integer Peephole Optimizations & Strength Reduction Patterns (x86-64)

Sourced from LLVM trunk source code: `X86InstrInfo.cpp`, `X86ISelDAGToDAG.cpp`, `X86ISelLowering.cpp`, `InstCombineAddSub.cpp`, `InstCombineCompares.cpp`, `InstCombineShifts.cpp`, `InstCombineMulDivRem.cpp`, `InstCombineAndOrXor.cpp`.

---

## 1. INTEGER ARITHMETIC SIMPLIFICATIONS

### 1.1 Constant arithmetic folding (constant propagation at IR level)
```
add X, 0           →  X
sub X, 0           →  X
mul X, 0           →  0
mul X, 1           →  X
shl X, 0           →  X
shr X, 0           →  X
and X, -1          →  X
or X, 0            →  X
xor X, 0           →  X
sub X, X           →  0
and X, X           →  X
or X, X            →  X
```

### 1.2 Division by power-of-2 → shift
```
udiv X, 8          →  shr X, 3
sdiv X, 8          →  sar X, 3  (with adjustment for negative X)
```
**x86 machine level:**
```
mov rax, rdi
cqo
mov rcx, 8
idiv rcx           →  mov rax, rdi
                     sar rax, 63      ; sign extend for rounding
                     shr rax, 61      ; >> 64-3 = shift
                     add rax, rdi     ; add 1 if negative
                     sar rax, 3
```

### 1.3 Modulo by power-of-2 → AND mask
```
urem X, 8          →  and X, 7
srem X, 8          →  and X, 7  (with adjustment)
```

### 1.4 Negate patterns
```
sub 0, X           →  neg X
add X, -1          →  sub X, 1
xor X, -1          →  not X
```

### 1.5 Double negate elimination
```
neg (neg X)        →  X
not (not X)        →  X
sub 0, (sub 0, X)  →  X
```

### 1.6 Add/Sub fold-through logic
```
add (sub X, Y), -1      →  add (not Y), X
sub 0, (shl B, C)       →  sub 0, (shl B, C)  [no fold, but:]
add A, (shl (neg B), C)  →  sub A, (shl B, C)
```

### 1.7 Algebraic simplifications
```
(a - b) + (c - a)    →  c - b
(a - b) + (a - c)    →  2*a - b - c
add (sub X, Y), Y    →  X
sub (add X, Y), Y    →  X
sub (add X, Y), X    →  Y
add (mul X, C1), C2  →  mul X, C1 + C2  (when add is (or disjoint, C))
```

### 1.8 Mul constant strength reduction (at IR level)
```
mul X, 2            →  shl X, 1
mul X, 3            →  add (shl X, 1), X
mul X, 4            →  shl X, 2
mul X, 5            →  add (shl X, 2), X
mul X, 7            →  sub (shl X, 3), X
mul X, 8            →  shl X, 3
mul X, 9            →  add (shl X, 3), X
mul X, -1           →  neg X
mul X, (2^n - 1)    →  sub (shl X, n), X
mul X, (2^n + 1)    →  add (shl X, n), X
```

### 1.9 Div-ceil folding
```
add (udiv X, Y), (zext (icmp ne (urem X, Y), 0))
  →  udiv (add nuw X, Y-1), Y
```

### 1.10 Square identity
```
a*a + 2*a*b + b*b    →  (a+b)*(a+b)
```

---

## 2. COMPARISON STRENGTH REDUCTIONS

### 2.1 TEST-based comparison elimination
```
cmp X, 0            →  test X, X    (when only flags are needed)
```

### 2.2 AND+CMP → TEST elimination (DAG level, `X86ISelDAGToDAG.cpp`)
```
; If AND feeds TEST:
and rax, rdi
test rax, rax        →  test rdi, rdi   (fused, if AND result only used in TEST)
```
Specific patterns from `findRedundantFlagInstr()`:
```
SUBREG_TO_REG + TEST64rr  →  eliminated if AND was the only use
```

### 2.3 AND+CMP→TEST with shift for multi-byte masks
When `(AND X, mask) == 0` where mask is a shifted-bytes pattern:
```
test rax, 0x00FF0000     →  shr rax, 16
                              test al, 0xFF     ; test8rr
```
From `X86ISelDAGToDAG.cpp` lines 6276-6360:
```
; If mask has leading zeros and saves bytes:
; 64-bit mask that fits in 32-bit: shr + test32rr
; 16-bit popcount: shr + test16rr
; 8-bit popcount: shr + test8rr
```

### 2.4 ICMP of (SHL X, C), 0 → ICMP of X, 0
```
icmp eq (shl nuw X, Y), 0  →  icmp eq X, 0
icmp eq (shl nsw X, Y), 0  →  icmp eq X, 0
icmp slt (shl nsw X, Y), 0 →  icmp slt X, 0
icmp sgt (shl nsw X, Y), -1 → icmp sgt X, -1
icmp slt (shl nsw X, Y), 1 →  icmp slt X, 1
icmp sle (shl nuw&nsw X, Y), Csle0 → icmp sle X, Csle0
```

### 2.5 ICMP of (SHL AP2, A), AP1 → direct comparison on A
```
icmp eq (shl 2, A), 8    →  icmp eq A, 3    (because 2^A = 8 → A = 3)
icmp ne (shl 4, A), 16   →  icmp ne A, 2
```

### 2.6 ICMP of AND with power-of-2 → bit test
```
icmp eq (and X, 1), 0    →  trunc X to i1
icmp eq (and X, (1<<n)), 0  →  icmp eq (and X, (1<<n)), 0  [no further]
```

### 2.7 ICMP of AND+shift → bit test without shift
```
icmp (and (sh X, Y), C2), C1  →  icmp (and X, (C2 << Y)), (C1 << C1)
; i.e., move the shift into the mask/compare, eliminating the shift
```

### 2.8 ICMP of OR/XOR/ADD/SUB chains → equality comparisons
```
((X1 ^ X2) | (X3 ^ X4)) == 0  →  (X1 == X2) && (X3 == X4)
((X1 - X2) | (X3 - X4)) == 0  →  (X1 == X2) && (X3 == X4)
```

### 2.9 ICMP of ADD constant → simplified comparison
```
icmp ult (add X, 1), X     →  icmp eq X, MAXUINT  (unsigned overflow check)
icmp ult (add X, 2), X     →  icmp ugt X, MAXUINT-2
icmp ult (add X, MAXUINT), X → icmp ne X, 0
```

### 2.10 ICMP of AND+add → simplified
```
icmp eq/ne (and (add A, addend), mask), C
  →  icmp eq/ne (and A, mask), (C - addend) & mask
```

### 2.11 ICMP of (SHL nuw X, Y), C → simplified
```
; For equality with nuw shift:
icmp eq (shl nuw 2, Y), 8  →  icmp eq Y, 3
```

### 2.12 ICMP remainder by power of 2 → bit test
```
icmp eq (irem X, 8), 0    →  icmp eq (and X, 7), 0
```

### 2.13 TEST+AND with zero comparison → SHR+TEST
```
; When mask covers only MSB:
; TEST AND → SHR + TESTrr (saves bytes for large constants)
; When mask covers only LSB:
; TEST AND → SHL + TESTrr
```

---

## 3. BIT MANIPULATION OPTIMIZATIONS

### 3.1 BEXTR patterns (BMI) — `matchBitExtract()`
Extracts `x & ((1 << nbits) - 1)` or equivalently `x >> shift & mask`:

**BMI1 BEXTR:**
```
; Pattern: x & ((1 << n) - 1)
; BEXTR control = nbits << 8 | 0
mov ecx, nbits << 8       ; control word
bextr rax, rdi, rcx       ; extract low nbits
```

**BMI2 BZHI (for dynamic nbits):**
```
; Pattern: x & ((1 << n) - 1) where n is variable
movzx ecx, sil            ; zero-extend nbits to 32-bit
bzhi rax, rdi, rcx        ; zero-high bits
```

**Patterns matched:**
```
a) x &  ((1 << n) - 1)
b) x &  ~(-1 << n)
c) x &  (-1 >> (32 - y))
d) x << (32 - y) >> (32 - y)
e) (1 << n) - 1
```

### 3.2 BEXTR from shift+mask (TBM/BMI) — `matchBEXTRFromAndImm()`
```
; Pattern: (x >> SHIFT) & MASK  where MASK is contiguous
; TBM: BEXTRI with immediate control
mov ecx, shift | (popcount(mask) << 8)
bextr rax, rdi, rcx

; BMI2 BZHI fallback (if BEXTR not fast):
; First BZHI to mask, then SHR to shift
bzhi rax, rdi, rcx   ; rcx = shift + masksize
shr rax, shift
```

### 3.3 AND mask shrinking — `shrinkAndImmediate()`
```
; AND with large positive mask that can be shrunk to negative:
and rax, 0x0000FFFF0000  →  and rax, 0xFFFF0000FFFF0000  (negative form)
; Or mask that becomes -1 (full mask):
and rax, 0xFFFFFFFFFFFFFFFF  →  (remove AND entirely)
```

### 3.4 SHL+Logic imm shrinking — `tryShrinkShlLogicImm()`
```
; Pattern: (x << C1) OR/AND/XOR C2
; Transform: (x OR/AND/XOR (C2 >> C1)) << C1
; This uses a smaller immediate encoding for C2 >> C1
```

### 3.5 XOR with sign bit → arithmetic shift pair (sign extension without sext)
```
add (xor X, 0x80), 0xF..F80   →  (X << ShAmtC) >>s ShAmtC
add (xor X, 0xF..F80), 0x80   →  (X << ShAmtC) >>s ShAmtC
; Only when high bits of X are known zero
```

### 3.6 CTTZ → TZCNT, CTLZ → LZCNT, CTPOP → POPCNT (BMI/POPCNT)
```
llvm.ctlz(X)     →  lzcnt rax, rdi    ; if X known non-zero or lzcnt+cmov
llvm.cttz(X)     →  tzcnt rax, rdi    ; if X known non-zero or tzcnt+cmov
llvm.ctpop(X)    →  popcnt rax, rdi
```

### 3.7 BSWAP (byte swap)
```
llvm.bswap(X)    →  bswap rax, rdi
```

### 3.8 Bit rotation → shift pair
```
rotr X, C        →  mov ecx, C
                     shrd rax, rax, rcx  (on x86-64)
; Or two shifts for scalar:
;  ror X, C  →  (X >> C) | (X << (64-C))
```

### 3.9 AND with all-ones mask → remove AND
```
and rax, -1      →  (remove, just mov rax, src)
```

### 3.10 AND with subset of known bits → simplify
```
; If upper bits of X are known zero:
and X, 0x0000FFFF  →  movzx rax, word ptr [addr]  ; use smaller load
```

### 3.11 Disjoint OR → ADD
```
; If (X | Y) has non-overlapping bits:
or X, Y           →  add X, Y     ; when disjoint bits known
; LLVM marks these as "disjoint" flag on OR
```

### 3.12 OR with constant → AND + smaller constant
```
; LLVM x86 backend: AND immediate shrinking:
or rax, 0xFFFF0000  →  and rax, 0x0000FFFF   ; (negate and swap op)
; When upper bits are all 1s, OR with them = keep lower bits = AND complement
```

### 3.13 Masked shift to scale factor (addressing mode)
```
; (x >> SHIFT) & MASK → folded into LEA scale
; where mask is shifted continuous bits:
and rax, 0x00FF0000  →  use SHR + scale in addressing mode
; or: shr rdi, 16 → and with 0xFF → scale by something
```

---

## 4. LEA-BASED OPTIMIZATIONS (beyond multiply)

### 4.1 INC/DEC → LEA (flag-free increment/decrement)
```
inc rax            →  lea rax, [rax + 1]    ; if flags not needed
dec rax            →  lea rax, [rax - 1]    ; if flags not needed
```

### 4.2 ADD reg, reg → LEA with scale (2x, 3x, 4x)
```
; x86-64: LEA can do base + index*scale + disp where scale = 1,2,4,8
mov rax, rdi
add rax, rdi        →  lea rax, [rdi + rdi]     ; = 2*x

mov rax, rdi
add rax, rdi
add rax, rdi        →  lea rax, [rdi + rdi*2]   ; = 3*x

mov rax, rdi
shl rax, 2          →  lea rax, [rdi*4]          ; = 4*x

mov rax, rdi
shl rax, 3          →  lea rax, [rdi*8]          ; = 8*x
```

### 4.3 ADD reg, imm → LEA (avoid flags)
```
add rdi, 5          →  lea rdi, [rdi + 5]        ; if flags not needed
add rax, -3         →  lea rax, [rax - 3]        ; (neg imm in LEA)
```

### 4.4 SUB reg, imm → LEA with negated immediate
```
sub rdi, 42         →  lea rdi, [rdi + (-42)]    ; i.e., lea rdi, [rdi - 42]
```

### 4.5 ADD reg, reg + imm → LEA (two-source add with constant)
```
add rax, rdi
add rax, 10         →  lea rax, [rax + rdi + 10]  ; three-operand form
```

### 4.6 ADD/INC/DEC with LEA scale (for 2x, 3x)
From `convertToThreeAddress()` and `convertToThreeAddressWithLEA()`:
```
; ADD8rr / ADD16rr where Src1 == Src2:
add di, di           →  lea di, [rdi + rdi]        ; = 2*x, zero-extends

; SHL by 1/2/3:
shl rdi, 1           →  lea rdi, [rdi + rdi]
shl rdi, 2           →  lea rdi, [rdi*4]
shl rdi, 3           →  lea rdi, [rdi*8]
```

### 4.7 LEA for ADD without flag clobber
Key insight from LLVM source: **LEA does not set EFLAGS**. This makes it valuable when the ADD result is needed but flags are not (or would be overwritten). LLVM uses a heuristic: when both operands of ADD are results of flag-setting operations (SUB, ADC, SBB, SMUL, UMUL), prefer LEA to avoid duplicating flag-producing instructions.

```
; If both inputs set flags, prefer LEA:
sub rax, rcx        ; sets flags
add rax, rdx        ; clobbers flags, but we need flags from SUB
→
sub rax, rcx        ; sets flags
lea rax, [rax + rdx] ; does NOT clobber flags!
```

### 4.8 LEA for RIP-relative addressing
```
add rax, [rip + symbol]  →  lea rax, [rip + symbol]  ; on x86-64, always
```

### 4.9 LEA as ADD with zero-extending semantics
```
; LEA32r zero-extends result to 64 bits (clears upper 32 bits)
lea eax, [rdi + rsi]  ; result zero-extends to rax
; vs
add eax, esi          ; also zero-extends, but sets flags
```

### 4.10 NEG → LEA with immediate 0
```
; Not typically done - NEG is already single-instruction
; But for LEA context:
neg rax              →  lea rax, [0 - rax]  ; not done, neg is preferred
```

---

## 5. ADDITIONAL x86-SPECIFIC PATTERNS

### 5.1 Zero-extension via 32-bit operation
```
movsxd rax, edi      →  mov eax, edi   ; 32-bit mov implicitly zero-extends
```

### 5.2 Sign-extension pattern recognition
```
; movsx can be folded into load or LEA:
movzx eax, byte [addr]  →  mov al, [addr]     ; if movzxb available
; or for load+extend:
movsx eax, byte [addr]  →  movsx eax, byte [addr]  ; fold load into extend
```

### 5.3 TEST + CMOV → TEST (when only zero flag matters)
```
; If TEST result is only used for zero flag:
test rax, rax
cmovne rcx, rdx     ; depends on ZF from test
; LLVM knows TEST sets all flags, can sometimes eliminate redundant TEST
```

### 5.4 SUB with zero → CMP pattern
```
sub rax, 0           →  cmp rax, 0  (if result not used, only flags)
```

### 5.5 AND+TEST fusion
```
and rax, rdi
test rax, rax        →  test rdi, rdi  (if AND only used in TEST)
```

### 5.6 LEA as multiply + add
```
imul rax, rdi, 3
add rax, 5           →  lea rax, [rdi + rdi*2 + 5]  ; = rdi*3 + 5
```

### 5.7 MOVABS → LEA for some immediates
```
mov rax, symbol      →  lea rax, [rip + symbol]  ; PIC-friendly
```

---

## 6. x86-64 LEA SCALE ENCODING REFERENCE

LEA supports these addressing modes:
```
[base + index * scale + disp]
; where scale ∈ {1, 2, 4, 8}
; base and index can be any GPR (except RSP for index)
; disp is 32-bit sign-extended
```

Effective arithmetic operations:
```
lea rax, [rdi]                    = rdi              (mov)
lea rax, [rdi + rsi]             = rdi + rsi        (add two regs)
lea rax, [rdi + rsi*2]           = rdi + 2*rsi      (scaled add)
lea rax, [rdi + rsi*4]           = rdi + 4*rsi      (scaled add)
lea rax, [rdi + rsi*8]           = rdi + 8*rsi      (scaled add)
lea rax, [rdi + rsi*2 + rcx*4]  = rdi + 2*rsi + 4*rcx  (complex)
lea rax, [rdi + rsi*2 + 100]     = rdi + 2*rsi + 100
lea rax, [rdi*4 + 100]           = 4*rdi + 100      (scaled base)
lea rax, [rdi*8]                 = 8*rdi            (scaled only)
```

---

## 7. QUICK REFERENCE TABLE

| Pattern | Optimized form | Condition |
|---------|---------------|-----------|
| `add rax, rdi` (same) | `lea rax, [rdi+rdi]` | = 2x, no flags needed |
| `add rax, 5` | `lea rax, [rax+5]` | No flags needed |
| `sub rax, 3` | `lea rax, [rax-3]` | No flags needed |
| `inc rax` | `lea rax, [rax+1]` | No flags needed |
| `dec rax` | `lea rax, [rax-1]` | No flags needed |
| `shl rdi, 1` | `lea rdi, [rdi+rdi]` | No flags needed |
| `shl rdi, 2` | `lea rdi, [rdi*4]` | No flags needed |
| `shl rdi, 3` | `lea rdi, [rdi*8]` | No flags needed |
| `test rax, rax` | (keep, or fold AND into TEST) | When AND+TEST |
| `and X, mask; cmp 0` | `shr X, n; test low, small_mask` | When mask saves bytes |
| `mul X, 2^n` | `shl X, n` | Always |
| `udiv X, 2^n` | `shr X, n` | Always |
| `sdiv X, 2^n` | `sar X, n` + adjustments | Always |
| `urem X, 2^n` | `and X, (2^n - 1)` | Always |
| `x & ((1<<n)-1)` | `bextr rax, rdi, rcx` (BMI1) or `bzhi` (BMI2) | With BMI |
| `neg (neg X)` | `X` | Always |
| `not (not X)` | `X` | Always |
| `sub 0, X` | `neg X` | Always |
| `xor X, -1` | `not X` | Always |
| `x * 3` | `lea rax, [rdi+rdi*2]` | Always |
| `x * 5` | `lea rax, [rdi*4+rdi]` | Always |
| `x * 9` | `lea rax, [rdi*8+rdi]` | Always |
| `x * 3 + C` | `lea rax, [rdi+rdi*2+C]` | Always |
| `add rax, rdx` (flags clobber) | `lea rax, [rax+rdx]` | When flags from prior op needed |

---

*Compiled from LLVM trunk source analysis, July 2026.*
