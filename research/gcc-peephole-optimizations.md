# GCC x86-64 Integer Peephole Optimizations & Strength Reduction Patterns

Concrete before/after instruction transformations extracted from GCC source
(i386.md, expmed.cc, fold-const.cc). Focus: integer domain only.

---

## 1. INTEGER MULTIPLY OPTIMIZATIONS

### 1.1 Power-of-2 Multiply → Shift
```
; BEFORE                          ; AFTER
imul  $8, %eax, %eax              shl   $3, %eax
imul  $4, %eax, %eax              shl   $2, %eax
imul  $2, %eax, %eax              shl   $1, %eax
imul  $32, %ecx, %edx             shl   $5, %ecx      ; or mov+shl
```
**Pattern**: When constant multiplier is exact power of 2 → left shift by log2.
**Source**: `synth_mult` in expmed.cc (alg_shift), i386.md split pattern.
**Cost**: shift_cost(m) vs imul_cost. Shifts are cheaper on all x86.

### 1.2 LEA-Based Multiply (3, 5, 9, etc.)
```
; BEFORE                          ; AFTER
imul  $3, %eax, %ecx              lea   (%eax,%eax,2), %ecx      ; eax*3
imul  $5, %eax, %ecx              lea   (%eax,%eax,4), %ecx      ; eax*5 (via eax,eax,4)
                                  ; actually: eax + eax*4 = eax*5
imul  $9, %eax, %ecx              lea   (%eax,%eax,8), %ecx
imul  $6, %eax, %ecx              lea   (%eax,%eax,2), %ecx      ; eax*3 first
                                  ; then  lea   (%ecx,%ecx,2), %ecx  ; *2 → eax*6
```
**Pattern**: `a * (2^n + 1)` → `lea (%a, %a, 2^n)`. On x86-64, LEA has scale factor 1,2,4,8.
**Source**: `synth_mult` (alg_add_t2_m), expmed.cc. GCC does: `q = t-1; m = ctz(q);`
**Examples**:
- `a * 3` → `a + a*2` → `lea (base, index, 2)` — 1 instruction
- `a * 5` → `a + a*4` → `lea (base, index, 4)` — 1 instruction  
- `a * 9` → `a + a*8` → `lea (base, index, 8)` — 1 instruction
- `a * 17` → `a + a*16` → `shl $4, %tmp; lea (%a, %tmp)` — 2 instructions

### 1.3 LEA-Based Multiply-Subtract
```
; BEFORE                          ; AFTER
imul  $7, %eax, %ecx              ; a * 7 = a * 8 - a
                                  lea   (%eax,%eax,8), %ecx
                                  sub   %eax, %ecx
imul  $15, %eax, %ecx             ; a * 15 = a * 16 - a
                                  lea   (%eax,%eax,8), %ecx
                                  lea   (%ecx,%ecx,8), %ecx
                                  sub   %eax, %ecx
```
**Pattern**: `a * (2^m - 1)` → `lea(a, a, 2^m) - a`. GCC's `alg_sub_t2_m`.
**Source**: synth_mult: `q = t+1; m = ctz(q); synth_mult(q >> m); append sub_t2_m(m)`

### 1.4 Shift-Add Factor Decomposition (General Multiply by Constant)
```
; For a * 21 (binary 10101):
;   21 = 5 * (4+1) = 5 * 5
;   Actually 21 = (4+1)*(4+1) — but simpler: 21 = 16+4+1

; BEFORE                          ; AFTER
imul  $21, %eax, %ecx             lea   (%eax,%eax,4), %ecx      ; ecx = 5*a
                                  lea   (%eax,%ecx,4), %ecx      ; ecx = a + 4*(5*a) = 21*a
```
**Pattern**: GCC decomposes into `t = q * (2^m + 1)` or `t = q * (2^m - 1)`.
**Source**: synth_mult `do_alg_addsub_factor` loop.

### 1.5 Negative Multiply via Negate
```
; BEFORE                          ; AFTER
imul  $-3, %eax, %ecx             ; -3 = negate(3)
                                  lea   (%eax,%eax,2), %ecx      ; 3*a
                                  neg   %ecx                      ; -(3*a)
imul  $-7, %eax, %ecx             ; -7 = negate(7)
                                  lea   (%eax,%eax,8), %ecx
                                  sub   %eax, %ecx              ; 7*a
                                  neg   %ecx                    ; -(7*a)
```
**Pattern**: `a * -C` → `neg(a * C)`. `choose_mult_variant` in expmed.cc.
**Source**: `negate_variant`: synth_mult for -val, append neg.

### 1.6 Multiply-by-(val-1) Plus Add
```
; BEFORE                          ; AFTER
imul  $22, %eax, %ecx             ; 22 = 21 + 1
                                  imul  $21, %eax, %ecx
                                  add   %eax, %ecx
```
**Pattern**: Sometimes `synth_mult(val-1)` + add is cheaper than direct.
**Source**: `add_variant` in choose_mult_variant.

### 1.7 Zero-Extend Multiply (APX ZU instruction)
```
; BEFORE                          ; AFTER
movzwl %ax, %eax                  ; clear upper 16 bits
imulw  $imm, %ax, %ax             ; multiply 16-bit
```
On AMD APX targets with ZU (zero-extend) support:
```
imulzuw $imm, %ax, %ax            ; zero-extend + multiply in one instruction
```

---

## 2. INTEGER DIVIDE/MODULO OPTIMIZATIONS

### 2.1 Unsigned Division by Constant (Magic Multiplier)
```
; x / 10 (unsigned) →
;   multiply by magic constant 0xCCCCCCCD, then shift right 35
mov    $0xCCCCCCCD, %ecx
imul   %ecx, %edx:eax        ; multiply edx:eax by magic (or use imul eax,ecx → edx)
shr    $3, %eax              ; shift right 3 (post-shift)
```
**Pattern**: `udiv x, const` → `mulhi(x, magic) >> post_shift`
**Source**: `choose_multiplier()` in expmed.cc computes magic multiplier.
**Algorithm**: Find m, k such that `floor(x/d) = floor(x*m / 2^(n+k))`.

### 2.2 Signed Division by Constant (Add-and-shift correction)
```
; x / 7 (signed) →
mov    %eax, %ecx
sar    $31, %ecx              ; sign extension
sub    %ecx, %eax              ; eax = |x|  (conditional negate)
mov    $0x24924925, %edx      ; magic constant for 7
imul   %edx, %eax
sar    $2, %eax               ; post-shift
add    %ecx, %eax              ; conditional add back sign
```
**Pattern**: `sdiv x, const` → `abs(x) * magic >> post_shift` with sign correction.
**Source**: expmed.cc signed division expansion.

### 2.3 Exact Division by Constant (Magic * Inverse)
```
; x / 12 (exact, no remainder) →
; 12 = 3 * 4, pre_shift = 2 (remove trailing zeros)
; ml = inverse_mod2n(3) mod 2^N = some value
mov    $0x55555556, %ecx      ; inverse of 3 mod 2^32
mov    %eax, %edx
shr    $2, %edx               ; pre-shift to remove trailing zeros (12 = 3<<2)
imul   %ecx, %edx             ; multiply by modular inverse
```
**Pattern**: `exact_div(x, d)` → `mult(x >> ctz(d), inverse_mod2n(d >> ctz(d)))`.
**Source**: `EXACT_DIV_EXPR` in expmed.cc.

### 2.4 Signed Division with Adjustment (Rounding)
```
; x / 7 with rounding correction →
; Compute x/7, then check if remainder has wrong sign
; If remainder != 0 and (x ^ d) < 0: quotient++, remainder -= d
```
**Source**: expmed.cc shows branch-based adjustment after division.

---

## 3. COMPARISON & BRANCH OPTIMIZATIONS

### 3.1 Compare-Zero → Test (most important!)
```
; BEFORE                          ; AFTER
cmp    $0, %eax                  test  %eax, %eax
cmp    $0, %ecx                  test  %ecx, %ecx
cmp    $0, (%rdi)                test  %eax, %eax    ; (load first)
```
**Pattern**: `cmp reg, 0` → `test reg, reg` (shorter encoding, same flags).
**Source**: i386.md `*cmp<mode>_ccz_1` and `*cmp<mode>_ccno_1`.
**Encoding**: `test %reg, %reg` is 2 bytes; `cmp $0, %reg` is 3 bytes (sign-extended imm8).

### 3.2 Test-Against-Zero for Bit Tests
```
; BEFORE                          ; AFTER
test   $1, %eax                  test  %al, %al      ; if testing lowest bit only
test   $0xFF, %eax               test  %eax, %eax    ; if just checking non-zero
```
**Pattern**: When checking if any bits are set, `test reg,reg` is shorter.
When checking specific bits, `test $imm, reg` is correct.
**Source**: i386.md test patterns.

### 3.3 High-Byte Comparison
```
; BEFORE                          ; AFTER  
cmp    $0, %ah                    test  %ah, %ah
cmp    $0, %bh                    test  %bh, %bh
```
**Source**: i386.md `*cmpqi_ext<mode>_2` pattern.

### 3.4 Compare-Minus-Is-Add-Sub (Fold subtract into compare)
```
; BEFORE                          ; AFTER
mov    %eax, %ecx
sub    %ebx, %ecx
cmp    $0, %ecx                  cmp   %ebx, %eax
```
**Pattern**: `(a - b) == 0` → `cmp b, a`.
**Source**: i386.md `*cmp<mode>_1` directly compares the two operands.

### 3.5 Add-Negative-In-Compare
```
; BEFORE                          ; AFTER
; (a + (-5)) == 0
; which is: cmp $0, (a + (-5))
; →       cmp $5, a
cmp    $0, (-5 + %eax)           cmp   $5, %eax
```
**Source**: i386.md `*cmp<mode>_plus_1`: negate the constant.

### 3.6 Inc/Dec with Flags
```
; BEFORE                          ; AFTER
add    $1, %eax                   inc   %eax       ; 1 byte shorter on 32-bit
sub    $1, %eax                   dec   %eax       ; 1 byte shorter on 32-bit
add    $-1, %eax                  dec   %eax
```
**Pattern**: When flags usage allows, `inc`/`dec` save encoding space.
**Caveat**: On some microarchitectures (since P4), inc/dec cause partial flag stalls.
**Source**: i386.md `*add<mode>_2` TYPE_INCDEC case.

### 3.7 Combined Compare-And-Branch (cmov/cmovcc)
```
; BEFORE                          ; AFTER
cmp    %ebx, %eax
jge    .Ltrue
mov    %ebx, %eax                 ; max(a,b)
jmp    .Lend
.Ltrue:
; eax already >= ebx
.Lend:
```
On modern x86 with cmov:
```
cmp    %ebx, %eax
cmovl  %ebx, %eax                 ; max(a,b) — branchless
```
**Source**: i386.md `*icmov` patterns.

---

## 4. LOAD/STORE OPTIMIZATIONS

### 4.1 LEA as Address Computation (Avoid Memory Loads)
```
; BEFORE                          ; AFTER
mov    (%rdi), %eax               ; load from array base
; ...                             lea   (%rdi,%rsi,4), %rax  ; addr = base + idx*4
                                  mov   (%rax), %eax          ; then load
```
**Pattern**: Use LEA to compute address in one step (base + index*scale + disp).
**Scale values**: 1, 2, 4, 8 only (hardware limitation).

### 4.2 LEA to Avoid Flags Dependency
```
; BEFORE (with flags dependency):  ; AFTER (no flags dependency):
mov    %eax, %ecx                  lea   (%ecx,%eax), %edx
add    %eax, %ecx                  ; LEA doesn't touch flags!
; flags now set by add              ; original %ecx still has old value
```
**Pattern**: `add %reg1, %reg2` → `lea (%reg1,%reg2), %reg2` when flags aren't needed.
**Source**: i386.md `Convert add to the lea pattern to avoid flags dependency`.

### 4.3 Combined Shift+Add → LEA
```
; BEFORE                          ; AFTER
shl    $2, %eax                   ; eax = eax * 4
add    4(%esp), %eax              ; eax += [esp+4]
```
Becomes:
```
mov    4(%esp), %edx              ; load the memory operand
lea    (%edx,%eax,4), %eax       ; eax = edx + eax*4
```
**Source**: i386.md peephole2 at line ~30210: shift + add → LEA.
**Constraint**: shift count must be 1, 2, or 3 (scale 2, 4, 8).

### 4.4 Consecutive Adds → LEA
```
; BEFORE                          ; AFTER
add    %ecx, %eax                 lea   (%eax,%ecx,1), %edx  ; wait, LEA needs scale
add    $16, %eax                  ; Actually this becomes:
                                  lea   16(%eax,%ecx), %eax  ; eax = eax + ecx + 16
```
**Pattern**: Two consecutive adds (reg+reg, then reg+imm) → single LEA.
**Source**: i386.md peephole2 at line 6521-6535.

### 4.5 Zero-Extend Load (movzwl vs movw)
```
; BEFORE                          ; AFTER
mov    (%rdi), %ax                movzwl (%rdi), %eax  ; zero-extends, avoids partial reg stall
; only sets lower 16 bits
```
**Pattern**: On P2+, `movzwl` is faster than `movw` due to partial register stalls.
**Source**: i386.md `*movhi_internal` TYPE_IMOVX case.

### 4.6 Combine movl + movb → Single Constant Load
```
; BEFORE                          ; AFTER
mov    $0x1234, %eax              mov   $0x12FF, %eax  ; merged into one constant
movb   $0xFF, %ah                 ; (set byte at position 8-15)
```
**Source**: i386.md peephole2 at line 3690-3704.

---

## 5. DEAD CODE ELIMINATION & FLAG PATTERNS

### 5.1 Dead Flags Register → Use Flag-Setting Instructions
```
; BEFORE                          ; AFTER
xor    %eax, %eax                 xor   %eax, %eax     ; (same, but now...)
; ... flags not used ...
; If flags dead:
add    $1, %eax                   inc   %eax           ; doesn't need test before
```
**Pattern**: When FLAGS_REG is dead (not used by subsequent branches/tests),
use flag-clobbering variants of instructions.

### 5.2 XCHG Dead → MOV
```
; BEFORE                          ; AFTER
xchg   %eax, %ecx                 mov   %eax, %ecx     ; if ecx is dead after xchg
; both registers modified          ; only ecx matters
```
**Source**: i386.md peephole2 lines 3436-3457.

### 5.3 Move Zero → XOR (Size Optimization)
```
; BEFORE                          ; AFTER
mov    $0, %eax                    xor   %eax, %eax    ; 2 bytes vs 5 bytes
mov    $0, %ecx                    xor   %ecx, %ecx    ; (xor also clears flags)
```
**Source**: i386.md `*xor<mode>_1` pattern.

### 5.4 mov $large → xor + mov small byte
```
; WITH -Oz (size optimization):
; BEFORE                          ; AFTER
mov    $200, %eax    ; 5 bytes     xor   %eax, %eax    ; 2 bytes
; flags dead                       movb  $200, %al      ; 2 bytes = 4 total
```
**Source**: i386.md peephole2 at line 5009-5021.

### 5.5 mov to High Byte
```
; WITH -Oz:
; BEFORE                          ; AFTER
mov    $512, %eax    ; 5 bytes     xor   %eax, %eax    ; 2 bytes
; (0x200 = 2 << 8)                movb  $2, %ah        ; 2 bytes = 4 total
```
**Source**: i386.md peephole2 at line 5025-5035.

### 5.6 mov $imm → push/pop (-Oz)
```
; WITH -Oz (very aggressive size):
; BEFORE                          ; AFTER
mov    $42, %eax     ; 5 bytes     push  $42             ; 2 bytes (sign-ext)
;                                  pop   %eax            ; 1 byte = 3 total
```
**Source**: i386.md peephole2 at line 2982-3001.

### 5.7 Zero + Sub-Low → Zero-Extend
```
; BEFORE                          ; AFTER
mov    $0, %eax                    movzwl (%rdi), %eax  ; zero-extend from memory
movb   (%rdi), %al
```
**Pattern**: `x = 0; x = (byte)x` → `x = zero_extend(byte_load)`.
**Source**: i386.md peephole2 at lines 4983-5005.

### 5.8 movl $large → movl + movb (Combine Constants)
```
; BEFORE                          ; AFTER
mov    $0x12340000, %eax           mov   $0x1234FF00, %eax  ; combine into one mov
movb   $0x00, %ah                 ; (merged)
```
**Source**: i386.md peephole2 at line 3690.

---

## 6. ADDITIONAL INTEGER STRENGTH REDUCTIONS

### 6.1 Negate-Extract-Sign-Bit
```
; BEFORE                          ; AFTER
; Extract LSB and negate
and    $1, %eax                    and   $1, %eax
neg    %eax                        neg   %eax
```
**Pattern**: `sign_extract bit 0` → `and $1; neg`.
**Source**: i386.md `*extv<mode>_1_0`.

### 6.2 Rotate Detection
```
; BEFORE                          ; AFTER
; (x << C1) | (x >> C2)           rol   $C1, %eax    ; if C1+C2 = bitwidth
; where C1 + C2 = 32              ; single rotate instruction
```
**Source**: fold-const.cc `bit_rotate` section.

### 6.3 XOR of Bit-Test → Equality Test
```
; BEFORE                          ; AFTER
xor    $1, %eax                    ; (x & 1) ^ 1  →
test   $1, %eax                    je   .Lzero      ; if lowest bit is 0
; OR:                               ; (equivalent to eq check)
; fold: (x & 1) ^ 1 == 0  →  (x & 1) == 0
```
**Source**: fold-const.cc: `(X & 1) ^ 1` → `(X & 1) == 0`.

### 6.4 Shift-of-Shift → Combined Shift
```
; BEFORE                          ; AFTER
shl    $3, %eax                    shl   $5, %eax    ; (3+2=5)
shl    $2, %eax
```
**Pattern**: Two consecutive shifts by constants → single combined shift.
**Source**: Common CSE/combine optimization.

### 6.5 Add-Sub Identity Elimination
```
; BEFORE                          ; AFTER
add    $5, %eax                    ; (nothing — eliminated)
sub    $5, %eax
```
**Pattern**: `a + C - C → a`.
**Source**: Standard algebraic simplification in fold-const.

### 6.6 Multiply-by-Negative-One → Negate
```
; BEFORE                          ; AFTER
imul   $-1, %eax, %eax            neg   %eax
```
**Source**: expmed.cc `expand_mult`: `op1 == CONSTM1_RTX → neg_optab`.

### 6.7 Multiply-by-Zero → Zero
```
; BEFORE                          ; AFTER
imul   $0, %eax, %ecx             xor   %ecx, %ecx
```
**Source**: expmed.cc: `op1 == CONST0_RTX → return op1`.

### 6.8 Multiply-by-One → Identity
```
; BEFORE                          ; AFTER
imul   $1, %eax, %ecx             mov   %eax, %ecx    ; (or just use eax)
```
**Source**: expmed.cc: `op1 == CONST1_RTX → return op0`.

---

## 7. TEST/AND COMBINATION PATTERNS

### 7.1 Test of Sign Bit
```
; BEFORE                          ; AFTER
cmp    $0, %eax                    ; (for signed comparison)
jl     .Lnegative                  
```
Better:
```
test   %eax, %eax                  ; sets SF flag
js     .Lnegative                  ; jump if sign bit set
```

### 7.2 Test-and-Branch on Specific Bit
```
; Test bit 3 of eax:
test   $8, %eax                    ; 2-byte test with imm8
jnz    .Lbit3set
```
On Pentium, `test imm, reg` is pairable only with eax, ax, al.

### 7.3 Test Result of Previous Operation (Elide Redundant Test)
```
; BEFORE                          ; AFTER
add    %ecx, %eax                  add   %ecx, %eax   ; add already sets flags!
test   %eax, %eax                  jz    .Lzero       ; use flags from add
jz     .Lzero
```
**Pattern**: If previous instruction already sets the needed flags, elide the test.

---

## 8. COMPARISON FOLDING PATTERNS

### 8.1 Fold `(a - b) op 0` → `a op b`
```
; BEFORE                          ; AFTER
mov    %eax, %ecx                  cmp   %ebx, %eax    ; cmp already does a-b
sub    %ebx, %ecx
cmp    $0, %ecx
je     .Ltarget                    je    .Ltarget
```

### 8.2 Fold `(a + (-C)) op 0` → `a op C`
```
; BEFORE                          ; AFTER
; if we have:  (eax - 5) == 0
lea    -5(%eax), %ecx             cmp   $5, %eax
cmp    $0, %ecx
```

### 8.3 Doubleword Compare Optimization
```
; Comparing 64-bit values on 32-bit:
; BEFORE                          ; AFTER
cmp    %edx, %ecx                  cmp   %edx, %ecx   ; compare high words first
jne    .Ldone                      jne   .Ldone
cmp    %eax, %ebx                  cmp   %eax, %ebx   ; only if high words equal
.Ldone:                            .Ldone:
```
**Special case**: If comparing against -1, `ior` high+low parts, then test.
**Source**: i386.md `*cmp<dwi>_doubleword`.

---

## 9. SIZE-OF-INSTRUCTION AWARE PATTERNS

### 9.1 mov → test+branch Shortening
```
; 32-bit: mov eax,0  (5 bytes)    xor eax,eax  (2 bytes)
; 64-bit: mov rax,0  (7 bytes)    xor eax,eax  (2 bytes) — zero-extends!
```

### 9.2 Short Immediate Forms
```
; add $1, %eax     →  inc %eax      (2 bytes vs 3 bytes on 32-bit)
; add $-1, %eax    →  dec %eax      (2 bytes vs 3 bytes on 32-bit)
; sub $1, %eax     →  dec %eax
; add $127, %eax   →  add $127,%eax (3 bytes, fits in imm8)
; add $256, %eax   →  add $256,%eax (6 bytes, needs imm32)
```

---

## 10. PEEPHOLE PATTERNS FROM i386.md (FLAG AWARENESS)

### 10.1 Add Without Flags
```
; BEFORE                          ; AFTER
add    %ecx, %eax                 add   %ecx, %eax   ; same instruction
; if flags are dead (no test/branch uses flags)
```
GCC uses `(clobber (reg:CC FLAGS_REG))` pattern to mark flag-irrelevant adds.
This allows LEA substitution when flags aren't needed.

### 10.2 Conditional Move Instead of Branch
```
; BEFORE                          ; AFTER
cmp    %ebx, %eax                  cmp   %ebx, %eax
jge    .Lskip                      cmovl %ebx, %eax   ; max(a,b) branchless
mov    %ebx, %eax
.Lskip:
```

---

## KEY TAKEAWAYS FOR YOUR COMPILER

### Must-Implement Patterns (High Impact):
1. **cmp reg,0 → test reg,reg** (saves 1 byte, universal)
2. **mul power-of-2 → shl** (you have this)
3. **mul constant → LEA** for 3,5,9 (you may be missing this)
4. **lea to avoid flags** (convert add→lea when flags dead)
5. **shift+add → combined LEA** (shift 1/2/3 + add → single lea)
6. **xor reg,reg → zero** (instead of mov reg,0)
7. **xchg dead → mov** (saves bytes)
8. **push/pop → mov** for small constants (-Oz)

### Medium-Impact Patterns:
9. **Unsigned div by constant** → magic multiply + shift
10. **Signed div by constant** → magic multiply + sign correction
11. **Consecutive adds → LEA** (reg+reg, then reg+imm → one lea)
12. **movl+movb → combined constant**
13. **Rotate detection** ((x<<C1)|(x>>(32-C1)) → rol)
14. **Doubleword compare optimization**

### Lower-Priority Patterns:
15. **Size-optimal mov variants** (xor+byte-mov for -Oz)
16. **push/pop for constant loads** (-Oz only)
17. **xchg ↔ mov** based on AX register presence
