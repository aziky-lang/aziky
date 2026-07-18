# x86-64 Peephole Optimization Patterns for Real Speedups

Research compiled for aziky project. All patterns target modern x86-64 (Skylake/Zen3+). References to Agner Fog's optimization manuals, uops.info measurements, and Intel/AMD optimization references.

---

## Table of Contents
1. [Immediate Folding & Instruction Encoding](#1-immediate-folding--instruction-encoding)
2. [Branch-Free Idioms](#2-branch-free-idioms)
3. [Bit Manipulation Tricks](#3-bit-manipulation-tricks)
4. [Strength Reduction](#4-strength-reduction)
5. [Instruction Fusion (Macro-Ops)](#5-instruction-fusion-macro-ops)
6. [Memory Access Patterns](#6-memory-access-patterns)
7. [Division & Multiplication](#7-division--multiplication)
8. [Sign Extension & Zero Extension](#8-sign-extension--zero-extension)
9. [Conditional Move & CMOV Patterns](#9-conditional-move--cmov-patterns)
10. [Function Call Conventions & ABI Tricks](#10-function-call-conventions--abi-tricks)
11. [Port Pressure & Throughput Optimization](#11-port-pressure--throughput-optimization)
12. [Miscellaneous High-Value Patterns](#12-miscellaneous-high-value-patterns)

---

## 1. Immediate Folding & Instruction Encoding

### 1a. Zero Extension Implicit in 32-bit Operations
**Pattern:** Writing to a 32-bit register automatically zero-extends to 64-bit.

```asm
; SLOW (5+ bytes, dependency on old RAX):
movzx eax, word [ptr]    ; 4+ bytes

; FASTER (when you only need 32-bit result):
mov    eax, [ptr]        ; REX-free when source is 32-bit memory
                         ; But use movzx for 16/8-bit loads

; KEY INSIGHT: mov eax, [mem] is 1 byte shorter than mov rax, [mem]
; and on modern CPUs, the 32-bit form avoids false dependencies on RAX.
; 32-bit operations zero-extend to 64-bit in the rename stage.
```
**Skylake latency:** MOVZX r32,r/m8: 1 µop, 1c latency. MOV r32,r/m32: 1 µop, 1c latency.
**Reference:** Agner Fog, "Optimizing subroutines in assembly language", §4.1.

### 1b. LEA as Arithmetic Shorthand
**Pattern:** LEA can perform multiply-by-constant + addition in a single instruction.

```asm
; SLOW: multiply and add
imul rax, rcx, 3     ; 3 µops, 3c latency (Skylake)
add  rax, rdx        ; 1 µop, 1c
; Total: 4 µops, 4c latency

; FASTER: LEA fold
lea  rax, [rdx + rcx*2]  ; 1 µop, 1c latency (LEA form)
; Then:
lea  rax, [rax + rcx]    ; rdx + rcx*3 = (rdx + rcx*2) + rcx
; Total: 2 µops, 2c latency

; Even better, one LEA:
lea  rax, [rdx + rcx*4]  ; multiply + add, 1 µop, 1c
```

**Valid LEA scale factors:** 1, 2, 4, 8 (powers of 2 only).
**Skylake:** LEA (complex form with scale): 1 µop on port 1, 1c latency. LEA (simple, no scale): 1 µop on port 5, 1c.
**IMPORTANT:** LEA has no flags side effect — it does NOT set CF/OF like ADD does.

### 1c. Immediates vs. Register Materialization
**Pattern:** Prefer small immediates that encode in the instruction.

```asm
; Use immediate for constants that fit in sign-extended 8-bit:
test rax, 0xFF       ; 3 bytes (sign-extended imm8)
and  rax, 0xFF       ; 3 bytes
; vs.
mov  r8, 0xFF        ; 7 bytes (64-bit immediate)
and  rax, r8         ; 3 bytes

; For XOR with self (zeroing idiom):
xor  eax, eax        ; 2 bytes, dependency-breaking on modern CPUs
; vs.
mov  rax, 0          ; 7 bytes, FALSE DEPENDENCY on old RAX value
```

---

## 2. Branch-Free Idioms

### 2a. Conditional Set Without Branch (CMOV)
```asm
; Branchy:
cmp  eax, ebx
jl   .is_less
mov  eax, ebx
jmp  .done
.is_less:
; eax already < ebx
.done:

; Branch-free (CMOVcc):
cmp  eax, ebx
cmovl eax, ebx    ; 1 µop, 1c latency (Skylake), port 0+1+5
; Total: 2 µops, 2c (vs. branch that costs 10-20c on mispredict)
```
**Skylake:** CMOVs: 1 µop, 1c latency on ports {0,1,5}. The CMP + CMOVL pair is 3 µops total, typically fused as CMP (µops on 0/1/5) + CMOV (µops on 0/1/5).

### 2b. Absolute Value Without Branch
```asm
; Branch-free absolute value:
mov  ecx, eax       ; save original
sar  eax, 31        ; sign extend: all 1s if negative, all 0s if positive
xor  eax, ecx       ; flip bits if negative
sub  eax, ecx       ; +1 if negative (two's complement negation)
; Total: 4 µops, ~4c

; Alternative (BMI1 - faster on Zen3+):
mov  ecx, eax
neg  eax             ; sets CF if eax != 0
sbb  eax, eax        ; -1 if negative, 0 otherwise
xor  eax, ecx        ; flip
and  eax, ecx        ; select
; Total: 5 µops but higher throughput on some µarchs
```

### 2c. Min/Max Without Branch (and without CMOV)
```asm
; For 32-bit integers (branch-free min):
cmp   edi, esi
cmovg edi, esi      ; edi = min(edi, esi)
; or
cmp   edi, esi
cmovl esi, edi      ; esi = min(edi, esi)
```

### 2d. Conditional Negate Without Branch
```asm
; Negate only if condition is true (e.g., sign is negative):
; Given: eax = value, edx = mask (all-ones if negate, all-zeros otherwise)
xor  eax, edx       ; flip bits if mask is all-ones
sub  eax, edx       ; -1 if mask is all-ones (completes two's complement negate)
```

### 2e. Boolean Select (ternary without branch)
```asm
; result = cond ? a : b
mov  eax, a
mov  ecx, b
test cond, cond
cmovz eax, ecx     ; if cond==0, take b; else keep a
```

---

## 3. Bit Manipulation Tricks

### 3a. BMI1/BMI2 Instructions (Haswell+, fast on Zen3+)
These replace multi-instruction bit manipulation sequences with single instructions.

```asm
; BLSR: Reset lowest set bit
; Equivalent to: x & (x - 1)
blsr  eax, ecx     ; 1 µop, 1c latency, 1/0.33c throughput (Skylake)

; BLSMSK: Get mask up to lowest set bit
; Equivalent to: x ^ (x - 1)
blsmsk eax, ecx    ; 1 µop, 1c latency

; BLSI: Extract lowest isolated bit
; Equivalent to: x & (-x)
blsi   eax, ecx    ; 1 µop, 1c latency

; ANDN: Logical AND NOT
; Equivalent to: ~a & b
andn   eax, ecx, edx  ; 1 µop, 1c latency

; BZHI: Zero high bits above bit index
bzhi   eax, ecx, edx  ; 1 µop, 3c latency (Skylake), 1/1c throughput

; PDEP: Parallel deposit bits
pdep   eax, ecx, edx  ; 1 µop, 3c latency (Skylake), 1/1c throughput

; PEXT: Parallel extract bits  
pext   eax, ecx, edx  ; 1 µop, 3c latency (Skylake), 1/1c throughput

; SHRX/SHLX/SARX (BMI1):
shrx   eax, ecx, edx  ; variable shift right, 1 µop, 1c (no flag effects!)
```
**Note:** BMI1/BMI2 do NOT modify flags. This is useful when you need to preserve flags across operations.

### 3b. Population Count (POPCNT)
```asm
; Popcount (single instruction on SSE4.2+, fast):
popcnt eax, ecx     ; 1 µop, 3c latency (Skylake), 1/1c throughput
; Replaces 10+ instruction bit-parallel algorithm
```

### 3c. Bit Scan Forward/Reverse
```asm
; BSF/BSR: Bit scan (legacy, works but has partial flag stall on some µarchs):
bsf  eax, ecx      ; 1 µop, 3c latency, 1c throughput (Skylake)

; TZCNT/LZCNT (BMI1, LZCNT needs ABM):
tzcnt eax, ecx      ; 1 µop, 3c latency, 1c throughput; returns 32 if ecx==0
lzcnt eax, ecx      ; 1 µop, 3c latency, 1c throughput; returns 32 if ecx==0
; vs BSF/BSR which are undefined on zero input
```

### 3d. Round to Nearest Power of 2
```asm
; DeBruijn sequence multiplication:
; Round up to next power of 2:
bsr   ecx, eax       ; find highest set bit
lea   eax, [1 << ecx] ; set that bit
; Or for ROUND UP:
; See Hacker's Delight / Sean Anderson's bithacks
```

---

## 4. Strength Reduction

### 4a. Division by Constant → Multiply + Shift
This is the single most impactful peephole optimization for arithmetic.

```asm
; Division by compile-time constant 10:
; SLOW: idiv (20-90 cycles on Skylake!)
mov  eax, dividend
xor  edx, edx
mov  ecx, 10
cdq                   ; sign extend EAX into EDX:EAX
idiv ecx              ; ~23c latency, 8-20c throughput (Skylake)

; FAST: multiply-high + shift
; For unsigned division by 10:
mov  eax, dividend
mov  ecx, 0xCCCCCCCD  ; magic number for /10
imul ecx              ; unsigned multiply high: edx = dividend * magic >> 32
shr  edx, 3           ; result in edx
; Total: 3 µops, ~4c latency

; For signed division by 10:
mov   eax, dividend
mov   ecx, 0xCCCCCCCD
imul  ecx              ; edx = unsigned multiply high
shr   edx, 3
; Then adjust for sign (one extra instruction for negative dividends)
```

**Magic numbers reference:** "Division by Invariant Integers using Multiplication" by Granlund & Montgomery (1994). LLVM implements this via `buildUDIVWithConstant` / `buildSDIVWithConstant`.

### 4b. Modulo by Power of 2 → AND mask
```asm
; SLOW:
mov  eax, value
xor  edx, edx
mov  ecx, 8
cdq
idiv ecx              ; 23c latency

; FAST:
mov  eax, value
and  eax, 7           ; value % 8 → 1 µop, 1c
```

### 4c. Division by 2 → Shift Right
```asm
; Signed division by 2 (rounds toward -∞, not toward zero!):
; C/C++ idiv rounds toward zero, arithmetic shift rounds toward -∞
mov  eax, value
shr  eax, 31          ; get sign bit
add  eax, value       ; add 1 if negative
sar  eax, 1           ; arithmetic shift right by 1
; OR for positive-only values:
mov  eax, value
sar  eax, 1           ; 1 µop, 1c
```

### 4d. Multiplication by Power of 2 → Shift Left
```asm
; SLOW:
imul eax, ecx, 8     ; 3 µops, 3c latency (Skylake)

; FAST:
lea  eax, [rcx*8]    ; 1 µop, 1c latency (if LEA port available)
; or
shl  eax, 3          ; 1 µop, 1c latency (but sets flags, overwrites source)
```

### 4e. Integer Multiplication by Constant
```asm
; imul r64,r64,imm: 3 µops, 3c latency on Skylake
; But for small constants, LEA or shifts are faster:

; multiply by 3:
lea rax, [rcx + rcx*2]  ; 1 µop, 1c (vs imul: 3 µops, 3c)

; multiply by 5:
lea rax, [rcx + rcx*4]  ; 1 µop, 1c

; multiply by 7:
lea rax, [rcx + rcx*8]  ; but we need rcx*7
; Better: rax = rcx*8 - rcx = rcx*8 + (-rcx)
lea rax, [rcx*8]
sub rax, rcx            ; 2 µops, 2c (vs imul: 3 µops, 3c)

; multiply by 6:
lea rax, [rcx + rcx*2]  ; rax = 3*rcx
shl rax, 1              ; 2 µops, 2c (vs imul: 3 µops, 3c)

; multiply by 10:
lea rax, [rcx + rcx*4]  ; 5*rcx
shl rax, 1              ; 10*rcx
; 2 µops, 2c
```

---

## 5. Instruction Fusion (Macro-Ops)

Modern x86 CPUs fuse pairs of instructions into single macro-ops during decode.

### 5a. Compare + Branch Fusion
```asm
; On Skylake/Zen3+, these pairs are fused into 1 macro-op:
cmp eax, ebx     ; } fused into 1 µop at decode
je  .target      ; }
; This is FREE — the fusion happens in hardware. Your job is to ENSURE
; you emit cmp+jcc back-to-back with no intervening instruction.

; FUSION IS BROKEN by:
; - Any instruction between cmp and jcc
; - The jcc target label being in the middle of a fused pair
; - Using LEA between cmp and jcc
```
**Skyclake:** Fusion of CMP+Jcc = 1 µop. Without fusion = 2 µops. This is 50% decode bandwidth savings.
**Zen3:** Same fusion support.

### 5b. TEST + Branch Fusion
```asm
; TEST/Jcc fuses on Skylake+:
test eax, eax     ; } fused into 1 µop
jz   .null_ptr    ; }
; TEST sets ZF; Jcc consumes it. Fuse pair.

; Anti-pattern that BREAKS fusion:
test eax, eax
nop               ; ← destroys fusion opportunity
jz   .null_ptr
```

### 5c. TEST + CMOV Does NOT Fuse
```asm
; CMOV does not fuse with its flag producer:
test eax, eax     ; 1 µop (on port 0/1/6)
cmovz eax, ebx    ; 1 µop (on port 0/1/5) — separate decode!
; Total: 2 µops (no fusion benefit)
```

### 5d. Boolean Patterns That Enable Fusion
```asm
; Test if bit N is set, branch:
test eax, (1 << 5)  ; } fused: 1 macro-op
jnz  .bit5_set       ; }

; Test if value is in range [0, N):
cmp  eax, N          ; } fused: 1 macro-op
jb   .in_range        ; }

; Test if two values equal:
cmp  eax, ebx        ; } fused: 1 macro-op
je   .equal           ; }

; Test sign bit:
test eax, eax        ; } fused: 1 macro-op
js   .negative        ; }
```

---

## 6. Memory Access Patterns

### 6a. Load-Use Penalty Avoidance
```asm
; SLOW: Load-then-use (load-use penalty ~4-5 cycles on Skylake):
mov  eax, [ptr]    ; load — latency depends on L1 hit (4c) or L2 (12c) etc.
add  eax, ebx      ; USE of eax — must wait for load to complete
; If add is the FIRST use of eax after load, there's a 4-5c penalty on top

; FASTER: Interleave unrelated work between load and use:
mov  eax, [ptr]     ; start load
add  ecx, edx       ; independent work (fills load latency)
add  eax, ebx       ; use eax — load likely done by now
; This HIDES the load latency
```

### 6b. Load Folding (Memory Operand in ALU Instructions)
```asm
; LLVM folds loads into ALU operands when beneficial:
; Instead of:
mov  eax, [ptr]     ; separate load
add  eax, ebx       ; use
; Fold to:
add  eax, [ptr]     ; load folded into ADD — saves 1 µop on some µarchs

; BUT: Only when there's no other use of the loaded value.
; If [ptr] is used elsewhere, keep the separate load.
```

### 6c. Store-Load Forwarding
```asm
; A store followed by a load to the same address gets forwarded
; through the store buffer (~5c latency on Skylake).
; If you store then immediately load, it's faster than keeping in register
; (only in specific patterns where register pressure is high).

; PATTERN: Use store forwarding to avoid register spills:
mov  [rsp], eax     ; store
mov  eax, [rsp]     ; load — forwarded through store buffer, ~5c
; vs. keeping value in a register: 0c but costs a register
```

### 6d. Non-Temporal Stores for Write-Once Patterns
```asm
; If you're writing to memory that won't be read soon:
movnti [ptr], eax   ; non-temporal store — bypasses cache
; Useful for streaming writes (copying large buffers, etc.)
; Skylake: MOVNTI is 1 µop, ~3c throughput (port 2/3)
```

---

## 7. Division & Multiplication

### 7a. Division Latency Reference (Skylake)
| Instruction | Latency | Throughput | Notes |
|------------|---------|------------|-------|
| DIV r32 | 23-26c | 8-20c | Very slow |
| DIV r64 | 35-88c | 21-35c | Extremely slow |
| IDIV r32 | 23-26c | 8-20c | Signed, even slower |
| IDIV r64 | 35-88c | 21-35c | Signed, worst case |
| MUL r32 | 3c | 1c | Fast |
| MUL r64 | 3c | 1c | Fast |

**Rule of thumb:** Avoid division entirely. Use multiply+shift for constant divisors. For variable divisors, consider: (a) reciprocal estimate + Newton-Raphson, (b) lookup table, (c) bit tricks.

### 7b. Multiplication High-Bits Trick
```asm
; Get high 64 bits of 64x64 multiply (useful for fast modulo):
mul  rcx           ; RDX:RAX = RAX * RCX (unsigned)
; RDX contains the high 64 bits
; This is useful for fast modular reduction by constants

; On Zen3+: IMUL r64,r64 is 3 µops, 3c latency, 1c throughput
```

### 7c. Division by 3 Without Division
```asm
; n / 3 for unsigned n:
mov  eax, n
mov  ecx, 0xAAAAAAAB  ; magic number
imul ecx              ; edx = high bits of n * magic
shr  edx, 1           ; result
; 3 µops, ~4c (vs idiv: 23c)
```

---

## 8. Sign Extension & Zero Extension

### 8a. MOVSX vs. MOVZX
```asm
; Sign-extend 8→32:
movsx eax, cl       ; 1 µop, 1c (Skylake)

; Zero-extend 8→32:
movzx eax, cl       ; 1 µop, 1c (Skylake)

; Sign-extend 8→64:
movsx rax, cl       ; 1 µop, 1c (Skylake)
; This is shorter than: movsx eax, cl + cdqe

; Zero-extend 8→64:
movzx eax, cl       ; 1 µop, 1c — the 32-bit form zero-extends to 64-bit!
; This is equivalent to: movzx rax, cl but shorter encoding
```

### 8b. Implicit Extension in 32-bit Operations
```asm
; Any 32-bit operation implicitly zero-extends the result to 64-bit:
mov  eax, ecx       ; zero-extends to RAX
add  eax, edx       ; zero-extends to RAX
; Use this to avoid explicit zero-extension instructions
```

---

## 9. Conditional Move & CMOV Patterns

### 9a. CMOVcc Timing
```asm
; On Skylake: CMOVcc is 1 µop, 1c latency, port 0/1/5
; The flag-producing instruction (CMP/TEST) and CMOVcc do NOT fuse
; So CMP+CMOV = 2 µops total

; CMOVcc latency: 1c (Skylake), 2c (Zen3)
; CMOVcc throughput: 0.5c (Skylake: port 0+1), 0.5c (Zen3)
```

### 9b. CMOV vs. Branch for Predictability
```asm
; CMOV is faster when:
; - Branch is unpredictable (50/50 taken/not-taken)
; - Both paths are short (don't need complex work)
; - Value is computed inline

; BRANCH is faster when:
; - Branch is highly predictable (>95% taken/not-taken)
; - One path is very short, other is complex
; - Mispredict penalty (~15-20c Skylake, ~15c Zen3) >> CMOV cost
```

### 9c. CMOV Chain Pattern
```asm
; Ternary chain: a ? b : (c ? d : e)
cmp  eax, 0
mov  ecx, b
mov  edx, d
cmovz ecx, edx     ; if eax==0, use d instead of b
cmp  eax, 0
cmovz ecx, e       ; nested: if eax==0 and some other condition...
; Watch out: CMOVcc uses the flags from the most recent flag-setting instruction
```

---

## 10. Function Call Conventions & ABI Tricks

### 10a. SysV AMD64 ABI (Linux)
```asm
; Arguments: RDI, RSI, RDX, RCX, R8, R9 (integer/pointer)
; Return: RAX (and RDX for 128-bit)
; Callee-saved: RBX, RBP, R12-R15
; Caller-saved: RAX, RCX, RDX, RSI, RDI, R8-R11

; Stack alignment: 16-byte aligned before CALL instruction
; Red zone: 128 bytes below RSP available without allocating
```

### 10b. Windows x64 ABI
```asm
; Arguments: RCX, RDX, R8, R9 (integer/pointer)
; Return: RAX
; Callee-saved: RBX, RBP, RDI, RSI, R12-R15
; Shadow space: 32 bytes above return address (caller must allocate)
; No red zone
```

### 10c. Leaf Function Optimization
```asm
; Leaf functions (no calls): skip frame setup entirely
my_func:
    lea  rax, [rdi + rsi]   ; do work
    ret                      ; no push/pop/rbp needed

; With red zone (Linux SysV):
my_leaf:
    mov  [rsp - 8], rbx     ; use red zone for callee-saved reg
    ; ... work ...
    mov  rbx, [rsp - 8]     ; restore
    ret
; Saves: push rbp / mov rbp,rsp / pop rbp (6+ bytes)
```

---

## 11. Port Pressure & Throughput Optimization

### 11a. Skylake Port Map (Simplified)
| Port | Operations |
|------|-----------|
| 0 | ALU (complex), multiply, divide, vector FP multiply, vector INT multiply |
| 1 | ALU (simple), vector ALU, CMOV, vector shuffle |
| 2 | Load |
| 3 | Load |
| 4 | Store address |
| 5 | ALU (simple, flag-setting), vector shuffle, LEA (simple) |
| 6 | ALU (flag-only: INC, DEC, TEST, CMP, etc.) |

### 11b. Avoiding Port 5 Bottleneck
```asm
; Port 5 is the bottleneck for LEA (simple form) and some shifts.
; Don't chain too many LEA instructions:

; BOTTLENECK:
lea eax, [rbx + rcx]   ; port 5
lea eax, [rax + rdx]   ; port 5
lea eax, [rax + r8]    ; port 5
; All three compete for port 5

; ALTERNATIVE: Mix port usage
lea eax, [rbx + rcx]   ; port 5
add eax, edx            ; port 0/1/5 (simple ALU)
add eax, r8d            ; port 0/1/5
```

### 11c. Port 6 Specialization
```asm
; INC/DEC only go to port 6 on Skylake (1 µop each)
; ADD/SUB/AND/OR/XOR can also go to port 0/1/5
; So if you have tight loops with lots of INC/DEC, port 6 can bottleneck

; Workaround: replace INC r with ADD r, 1 (which can use port 0/1/5)
inc  eax      ; 1 µop, port 6 ONLY
add  eax, 1   ; 1 µop, port 0/1/5 (but sets more flags — OF, etc.)
; NOTE: INC/DEC preserve CF; ADD/SUB modify CF
; If you need CF preserved, you must use INC/DEC

; BMI1/2: BLSR, BLSMSK, BLSI go to port 0 only
; PDEP/PEXT go to port 1 only on Skylake
```

### 11d. Throughput Optimization via Unrolling
```asm
; Single iteration (throughput-bound):
.loop:
    mov  eax, [rsi]      ; port 2 or 3
    add  eax, ecx         ; port 0/1/5
    mov  [rdi], eax       ; port 4
    add  rsi, 4
    add  rdi, 4
    dec  edx
    jnz  .loop
; This loop: ~7c per iteration (port 2: 1c, port 0/1/5: 1c, port 4: 1c, dec+jnz fused: 1c)

; Unrolled 2x (hide latencies):
.loop:
    mov  eax, [rsi]
    mov  r8d, [rsi+4]
    add  eax, ecx
    add  r8d, ecx
    mov  [rdi], eax
    mov  [rdi+4], r8d
    add  rsi, 8
    add  rdi, 8
    sub  edx, 2
    jnz  .loop
; Better: 2 loads can overlap on ports 2/3, better port utilization
```

---

## 12. Miscellaneous High-Value Patterns

### 12a. XOR Zeroing (Dependency Breaking)
```asm
; SLOW: mov eax, 0 — false dependency on old RAX value
mov  eax, 0       ; 7 bytes, 1 µop, but pipeline stall for dependency

; FAST: xor eax, eax — breaks dependency chain, recognized by µarch
xor  eax, eax     ; 2 bytes, 1 µop, 0c effective latency
; The CPU recognizes xor reg,reg as a zeroing idiom and removes the dependency

; Same for:
xor  ecx, ecx
xor  edx, edx
; etc.
```

### 12b. LEA vs. ADD for Flag-Preserving Add
```asm
; If you need to add without modifying flags:
add  eax, ecx      ; modifies OF, SF, ZF, AF, PF, CF
lea  eax, [rax+rcx] ; does NOT modify flags
; This is a common pattern after TEST/CMP when you need both comparison result AND addition
```

### 12c. BSWAP for Byte Reversal
```asm
; Byte-swap 32-bit register:
bswap eax          ; 1 µop, 1c latency (Skylake)
; Converts little-endian ↔ big-endian
; Only works on 32-bit and 64-bit registers

; Byte-swap 16-bit: no single instruction on x86-64
; Use: rol ax, 8  (1 µop, 1c)
; Or: movzx ecx, ax
;     bswap ecx
;     shr  ecx, 16
```

### 12d. CMOVcc Idioms for Flag-Free Computation
```asm
; Set eax to 1 if ecx > 0, else 0, WITHOUT using flags:
xor  eax, eax
test ecx, ecx
setg al             ; 1 µop, 2c (Skylake: port 5 only!)
; Alternative without flags:
shr  ecx, 31        ; 1 µop, 1c — gets sign bit
; Then negate or adjust

; SETcc latency: 2c on Skylake (port 5 only!)
; This is slow for chains — avoid heavy use of SETcc
```

### 12e. Bit Test and Modify (BT/BTS/BTR/BTC)
```asm
; Test bit N and branch:
bt   eax, ecx       ; test bit ecx of eax
jc   .bit_set       ; branch if bit was 1
; 1+1 µops, 1c latency (Skylake: BT goes to port 0)

; BTS: Test and set bit (atomic with LOCK prefix):
lock bts [mem], ecx  ; test-and-set bit — useful for spinlocks
```

### 12f. SIMD for Simple Operations
```asm
; Copy 16 bytes (one XMM register):
movdqu xmm0, [rsi]     ; unaligned load
movdqu [rdi], xmm0     ; unaligned store
; 2 µops, 2c (Skylake: ports 2/3 for load, port 4 for store)
; vs 4x MOV r64,[mem] + 4x MOV [mem],r64 = 8 µops

; Compare 4 integers in parallel:
movdqu xmm0, [rsi]
pcmpeqd xmm0, [rdi]    ; compare 4 × 32-bit at once
pmovmskb eax, xmm0      ; extract comparison results
; 4 µops for what would take 4 scalar CMPs
```

### 12g. TEST for Checking Specific Bits
```asm
; Check if low bit is set (odd number):
test eax, 1            ; 3 bytes (sign-extended imm8)
jnz  .odd

; Check multiple bits:
test eax, 0xF          ; check if low nibble is nonzero
jnz  .has_low_nibble

; Check if bit 3 is CLEAR:
test eax, (1 << 3)
jz   .bit3_clear
```

### 12h. CMOVcc for Absolute Value (Scalar, 32-bit)
```asm
; Branchless abs using CMOV:
mov  ecx, eax        ; save
neg  eax             ; negate (sets flags)
cmovs eax, ecx      ; if negative, restore original (which was positive)
; 3 µops, 4c (neg: 1c + cmovs: 1c + dependency)
; But: NEG only goes to port 0/1/5, CMOV to port 0/1/5
; Potential port pressure if combined with other port 5 ops
```

### 12i. Integer Multiply-Accumulate Without MUL
```asm
; For small constant multiplies in accumulation:
; Instead of: result += x * 3
imul eax, ecx, 3    ; 3 µops, 3c latency
add  edx, eax       ; 1 µop, 1c

; LEA + ADD:
lea  eax, [rcx + rcx*2]  ; 3*rcx, 1 µop, 1c
add  edx, eax             ; 1 µop, 1c
; Total: 2 µops, 2c
```

---

## Quick Reference: Instruction Latency/Throughput (Skylake)

| Instruction | Latency | Throughput | µops | Port |
|------------|---------|------------|------|------|
| MOV reg,reg | 0 (move eliminated) | — | 0 | — |
| MOV reg,imm | 1 | 0.25 | 1 | 5 |
| ADD/SUB/AND/OR/XOR reg,reg | 1 | 0.25 | 1 | 0/1/5 |
| LEA (complex) | 1 | 0.5 | 1 | 1 |
| LEA (simple) | 1 | 0.5 | 1 | 5 |
| INC/DEC reg | 1 | 0.25 | 1 | 6 |
| IMUL r,r | 3 | 1 | 3 | 1 |
| MUL r | 3 | 1 | 3 | 1 |
| IMUL r,r,imm | 3 | 1 | 3 | 1 |
| SHL/SHR/SAR reg,cl | 1 | 0.5 | 1 | 0/1/5 |
| SHL/SHR/SAR reg,1 | 1 | 0.25 | 1 | 0/1/5 |
| CMOVcc | 1 | 0.5 | 1 | 0/1/5 |
| BSF/TZCNT | 3 | 1 | 1 | 0/1 |
| BSR/LZCNT | 3 | 1 | 1 | 0/1 |
| POPCNT | 3 | 1 | 1 | 1 |
| BSWAP | 1 | 1 | 1 | 1 |
| BLSR/BLSMSK/BLSI | 1 | 1 | 1 | 0 |
| PDEP/PEXT | 3 | 1 | 1 | 1 |
| BZHI | 3 | 1 | 1 | 1 |
| BLSI | 1 | 1 | 1 | 0 |
| ANDN | 1 | 1 | 1 | 1 |
| SETcc | 2 | 0.5 | 1 | 5 |
| TEST r,r | 1 | 0.25 | 1 | 0/1/6 |
| CMP r,r | 1 | 0.25 | 1 | 0/1/6 |
| DIV r32 | 23-26 | 8-20 | 1 | 0 |
| IDIV r32 | 23-26 | 8-20 | 1 | 0 |
| MOVZX r32,r/m8 | 1 | 0.25 | 1 | 1/5 |
| MOVSX r32,r/m8 | 1 | 0.25 | 1 | 1/5 |
| MOV r32,r/m32 | 1 | 0.25 | 1 | 2/3 (load) |
| MOVNTI m32,r32 | ~3 | ~3 | 1 | 2/3 |

---

## Zen3+ Specific Notes

AMD Zen3/Zen4 have different port allocation than Intel Skylake:
- Zen3 has 6 ALU ports (ports 0-5), but not all instructions go to all ports
- INT multiply: 3 µops, port 0/1/2 (Zen3) vs port 1 (Skylake)
- INT divide: lower latency than Skylake for 32-bit (18c vs 23c)
- BMI1/BMI2: well-supported, generally 1 µop
- POPCNT: 1 µop, 1c latency (faster than Skylake's 3c!)
- CMOV: 2c latency on Zen3 vs 1c on Skylake
- BT/BTS/BTR/BTC: go to port 0 only on Zen3

---

## Priority Patterns for Compiler Emission

Sorted by impact (highest first):

1. **XOR zeroing** instead of MOV reg, 0 — 3 bytes saved + dependency breaking
2. **CMP+Jcc placement** — ensure fusion by keeping them adjacent
3. **LEA for multiply+add** — replaces 3 µop IMUL+ADD with 1 µop LEA
4. **Division by constant** — replace IDIV (23c) with IMUL+SHR (4c)
5. **32-bit operations** — implicit zero-extend, shorter encoding
6. **Load-use interleaving** — hide memory latency with independent work
7. **BMI1 instructions** (BLSI, BLSMSK, BLSR, ANDN) — replaces multi-instruction sequences
8. **CMOVcc for short conditionals** — avoids branch mispredict penalty
9. **MOD by power of 2** — AND mask instead of IDIV
10. **SIMD for bulk operations** — 16-byte copies, parallel comparisons

---

## References

1. Agner Fog, "Optimizing subroutines in assembly language" (optimizing_assembly.pdf)
2. Agner Fog, "The microarchitecture of Intel, AMD and VIA CPUs" (microarchitecture.pdf)
3. Agner Fog, "Instruction tables" (instruction_tables.pdf)
4. uops.info — Measured instruction latencies/throughputs for modern CPUs
5. Intel 64 and IA-32 Architectures Optimization Reference Manual
6. AMD Software Optimization Guide for AMD Family 19h and 21h (Zen3/Zen4)
7. LLVM X86 backend: X86InstrInfo.cpp, X86TargetTransformInfo.cpp
8. Sean Anderson, "Bit Twiddling Hacks" (graphics.stanford.edu/~seander/bithacks.html)
9. Granlund & Montgomery, "Division by Invariant Integers using Multiplication" (1994)
10. Hacker's Delight (Henry S. Warren Jr.) — bit manipulation patterns
