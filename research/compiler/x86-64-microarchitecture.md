# x86-64 Microarchitecture Optimization Reference

> For aziky emitting raw x86-64 bytes for tight LCG loops (50M iterations).
> Sources: Agner Fog's microarchitecture manual (2026), Intel SDM, AMD PPR.

---

## 1. Macro-Fusion: What Fuses and What Doesn't

### Universal Safe Pairs (all Intel + all AMD)
```asm
CMP  reg, reg       ; or CMP reg, imm
Jcc  label           ; any conditional branch

TEST reg, reg        ; or TEST reg, imm
Jcc  label           ; any conditional branch
```

### Intel-Specific (Skylake, Ice Lake, Alder Lake P)
| ALU Instr | Fuses With | Does NOT Fuse With |
|-----------|------------|-------------------|
| ADD, SUB | JE/JNE/JB/JA/JL/JG (+inverses) | JO, JNO, JS, JNS, JP, JNP |
| INC, DEC | JE/JNE/JL/JG only | JB, JA, JS, JO, JP (no CF/OF/PF) |
| AND | All Jcc | — |
| OR, XOR | **NEVER** | — |
| CMP [mem] | ✓ (SKL, ALD) but **NOT Ice Lake** | — |

### AMD Zen 1/2 (most restrictive)
| ALU Instr | Fuses? |
|-----------|--------|
| CMP, TEST | ✓ with ALL Jcc |
| ADD, SUB, INC, DEC, AND, OR, XOR | **ALL DO NOT FUSE** |

### AMD Zen 3/4/5 (most permissive)
| ALU Instr | Fuses? |
|-----------|--------|
| ALL ALU (ADD, SUB, INC, DEC, AND, OR, XOR, CMP, TEST) | ✓ with ALL Jcc |

### Encoding Constraints (Universal)
- **No memory destination** on ALU instruction
- **No RIP-relative addressing** on memory operand
- **No both displacement AND immediate** on memory operand
- **No instructions between** the ALU and Jcc
- **JECXZ/LOOP/LOOPE/LOOPNE never fuse** on anything

### Fused Pair Limits
- Max **2 fused pairs per decode cycle** (all Intel + AMD)
- **Taken branch throughput**: 1/cycle (all), 2/cycle Zen 5
- Fused pairs count as **1 µop** for loop buffer sizing

---

## 2. Loop Buffer Requirements

### Intel LSD (Loop Stream Detector)
| Platform | Max µops | Throughput | Alignment | Fusion Count |
|----------|----------|------------|-----------|--------------|
| Skylake | **30 µops** | 4 µops/cycle | None | Fused = 1 µop |
| Ice Lake | **50 µops** | 5 µops/cycle | None | Fused = 2 µops |
| Alder Lake P | ~50 µops | 6 µops/cycle | None | — |

### Intel µop Cache (DSB)
- **Skylake**: 1,536 µops, **32 sets × 8 ways × 6 µops/line**
- **Indexed by 32-byte aligned blocks**
- Max **18 µops per 32B block** (3 lines × 6 µops)
- **Unconditional JMP always ends a µop cache line** (wastes remaining slots)
- **Microcode instructions (>4 µops)** use entire µop cache line
- **Throughput**: 4 µops/cycle (SKL), 5 (ICL), 6 (ALD P)

### AMD µop Cache (no separate LSD)
| Platform | µops (nominal) | µops (effective ST) | Line Size | Throughput |
|----------|----------------|---------------------|-----------|------------|
| Zen 2 | 4,096 | ~2,200 | 8 µops | 5 instr/cycle |
| Zen 3 | 4,096 | ~2,200 | 8 µops | 6 µops/cycle |
| Zen 4 | 6,912 | ~3,500 | 8 µops | 6 µops/cycle |
| Zen 5 | 6,144 | ~3,000 | 6 µops | 6 µops/cycle ×2 |

### Critical Failure Conditions
1. **>30 µops** → no LSD on Skylake (use µop cache instead)
2. **32-byte boundary crossing** → wastes µop cache slots
3. **Unconditional JMP inside loop** → ends µop cache line, wastes slots
4. **>4 µop instruction** → uses microcode ROM, consumes entire line
5. **AH/BH/CH/DH registers** → extra µops or loop buffer issues
6. **64-byte boundary crossing in tiny loops** → kills AMD 1-cycle fast path

### Optimal Loop Layout
```
; Align loop start to 32 bytes
.p2align 5
loop_body:
    ; Keep total ≤30 µops for Skylake LSD
    ; Keep within single 32B block for µop cache efficiency
    ; Avoid unconditional JMPs inside
    ; Avoid crossing 64B boundary on AMD
```

---

## 3. Execution Port Pressure

### Skylake Port Map
```
P0: ALU simple+complex, IMUL, AES, CRC32, FP mul/add, INT div
P1: ALU simple, complex (BMI), INT div, FP div, vector ALU
P2: Load + AGU
P3: Load + AGU
P4: Store data
P5: ALU simple, branch, shuffle, vector shuffle
P6: ALU simple, branch
P7: Store address (AGU)
```

### Zen 2 Port Map
```
P0: ALU simple, branch
P1: ALU simple, complex (IMUL, DIV, CRC32)
P2: Load AGU 0
P3: Load AGU 1
P4: ALU simple, branch
P5: ALU simple, complex (IMUL, DIV)
P6: Store address AGU 0
P7: Store address AGU 1
S0: Store data 0
S1: Store data 1
```

### Instruction → Port Mapping (Both Architectures)
| Instruction | Skylake Ports | Zen 2 Ports | Throughput | Latency |
|---|---|---|---|---|
| ADD/AND/OR/XOR/SHL r,r | P0,1,5,6 | P0,1,4,5 | **0.25 CPI** (4/cycle) | 1 cyc |
| ADD/AND/OR/XOR/SHL r,imm | P0,1,5,6 | P0,1,4,5 | 0.25 CPI | 1 cyc |
| MOV r,r | **eliminated** | **eliminated** | **0 CPI** | 0 cyc |
| MOV r,[mem] | P2,P3 | P2,P3 | 0.5 CPI | 4-5 cyc |
| MOV [mem],r | P4+P7 (2µops) | P6/7+S0/1 (2µops) | 0.5 CPI | — |
| **IMUL r,r** | **P0,P1** | **P1,P5** | **0.5 CPI** | **3 cyc** |
| IMUL r,r,imm | P0,P1 | P1,P5 | 0.5 CPI | 3 cyc |
| INC/DEC r | P0,1,5,6 | P0,1,4,5 | 0.25 CPI | 1 cyc |
| LEA (complex) | P0,P1,P5 | P0,1,4,5 | 0.33 CPI | 1 cyc |
| LEA (simple) | P0,1,5,P6 | P0,1,4,5 | 0.25 CPI | 1 cyc |

### Critical Bottleneck: IMUL
- **IMUL uses only 2 ports** vs 4 for simple ALU → **2× slower throughput**
- Single LCG stream: **~0.5 iter/cycle** (IMUL-limited)
- 2-stream ILP: **~1 iter/cycle** (fills both IMUL ports)
- 4-stream ILP: **~2 iters/cycle** (saturates all ports)

### Store Port Bottleneck
- Every store = 2 µops (address + data)
- **Skylake client**: 1 store-address port (P7) → **max 1 store/cycle**
- **Zen 2**: 2 store-address ports (P6, P7) → **max 2 stores/cycle**
- **Skylake server (SKX/CLX)**: adds P8 → 2 stores/cycle

### Data Dependency Chain Limits
```asm
; BAD: 5-cycle dependency chain (IMUL=3, ADD=1, AND=1)
imul rax, rax, 1664525     ; rax←f(rax)  [3 cycles]
add  rax, 1013904223       ; rax←f(rax)  [1 cycle, waits]
and  rax, mask             ; rax←f(rax)  [1 cycle]
; Throughput: 0.2 iter/cycle

; GOOD: 2 independent chains in parallel
imul r8, r8, 1664525
imul r9, r9, 1664525       ; independent!
add  r8, 1013904223
add  r9, 1013904223
and  r8, mask
and  r9, mask
; Throughput: 2 iters per 5 cycles = 0.4 iter/cycle (2× improvement)
```

---

## 4. Store-to-Load Forwarding

### Minimum Size
- **1 byte forwards successfully** (8-bit store → 8-bit load at same address)
- **Rule**: Store ≥ load size, same starting address → forwards
- No alignment requirement for ≤64-bit operands

### Latency Table
| Scenario | Skylake | Ice Lake | Zen 2/3 |
|----------|---------|----------|---------|
| Successful forwarding (≤64-bit) | **4-5 cyc** | **5 cyc** | **4-5 cyc** |
| Ice Lake zero-latency forwarding | — | **0 cyc** (8/32/64-bit aligned) | — |
| Failed forwarding (standard) | **~15 cyc** | **19-20 cyc** | **6-10 cyc** |
| Cache line crossing | +4-5 cyc | +2 cyc | small |

### Forwarding Failures (What to Avoid)
| Pattern | Penalty |
|---------|---------|
| Store 4B, load 8B at same address | +11-12 cycles |
| Two 4B stores → 8B load spanning both | +11-12 cycles |
| Store 4B, load 4B at address+1 (partial overlap) | +11-12 cycles |
| Store 16B unaligned, load 16B unaligned (<16B alignment) | **+50+ cycles** |
| 128+ bit crossing 64B cache line | +4-5 cycles (SKL), +2 (ICL) |

### Safe Patterns
```asm
; Always match store/load sizes
mov [rbx], rax        ; 8-byte store
mov rcx, [rbx]        ; 8-byte load at same address → forwards

; Same address, smaller read is OK
mov [rbx], rax        ; 8-byte store
mov cx, [rbx]         ; 2-byte load → forwards (same start addr)

; NEVER do this (store-then-merge-load)
mov [rbx], eax        ; 4-byte store
mov [rbx+4], edx      ; 4-byte store
movq xmm0, [rbx]      ; 8-byte load → FORWARDING FAILURE (+12 cyc)
```

### Compiler Guidelines
1. **Always match store/load sizes** when possible
2. **Never merge small stores into large loads** (memcpy pattern)
3. **Cache-line align** large (128/256-bit) SIMD stores
4. **Avoid unaligned 128/256-bit stores** if read back later (catastrophic 50-210 cyc penalty on older Intel)

---

## 5. Optimal Byte-Level Patterns for LCG Loops

### Single-Stream LCG (Minimal, for Small Output)
```asm
; 8 instructions, ~24 bytes
; state = state * 1664525 + 1013904223
imul rax, rax, 1664525       ; 48 69 C5 09            [3 cyc, P0/P1]
add  rax, 1013904223         ; 48 05 0F 26 96 3E      [1 cyc, any ALU]
and  rax, 0xFFFFFFFF         ; 48 25 FF FF FF 7F      [1 cyc, any ALU]
; Total: 5-cycle dependency chain → 0.2 iter/cycle
```

### 2-Stream LCG (Optimal for Skylake/Zen2)
```asm
; 16 instructions, ~48 bytes
; Two independent LCG streams saturating both IMUL ports
imul r8, r8, 1664525         ; P0 or P1
imul r9, r9, 1664525         ; P0 or P1 (parallel!)
add  r8, 1013904223
add  r9, 1013904223
and  r8, 0xFFFFFFFF
and  r9, 0xFFFFFFFF
; Each chain: 5 cyc, but interleaved: 2 iters per 5 cycles = 0.4/cycle
```

### 4-Stream LCG (Maximum Throughput)
```asm
; 32 instructions, ~96 bytes
; Saturates all 2 IMUL ports → ~2 iters/cycle
imul r8,  r8,  1664525
imul r9,  r9,  1664525
imul r10, r10, 1664525
imul r11, r11, 1664525       ; 4 IMULs but only 2 ports → 2 cycles
add  r8,  1013904223
add  r9,  1013904223
add  r10, 1013904223
add  r11, 1013904223
and  r8,  0xFFFFFFFF
and  r9,  0xFFFFFFFF
and  r10, 0xFFFFFFFF
and  r11, 0xFFFFFFFF
; 2 IMUL ports: 4 IMULs = 2 cycles of IMUL
; 4 ALU ports: 8 ADD+AND = 2 cycles of ALU
; Total: ~2 iters/cycle
```

### Store Pattern (Skylake-Optimized)
```asm
; Interleave store addr computation with ALU to hide store latency
imul rax, rax, 1664525
add  rax, 1013904223
and  rax, 0xFFFFFFFF
mov  [rbx + r13*8], rax      ; store: P4(data) + P7(addr)
; On Skylake: 1 store = 1 P7 slot → max 1 store/cycle
; On Zen 2: 2 store AGUs → max 2 stores/cycle
```

---

## 6. Summary: Optimal Emission Rules

| Rule | Why |
|------|-----|
| **32-byte align loop start** | µop cache filling on Intel + AMD |
| **Keep loop ≤30 µops** (Skylake) / **≤50** (Ice Lake) | LSD activation |
| **Use TEST/CMP + Jcc for branches** | Fuses on ALL architectures |
| **Avoid unconditional JMP inside loop** | Wastes µop cache slots |
| **Avoid AH/BH/CH/DH** | Extra µops, loop buffer issues |
| **Don't cross 64B boundary in tiny loops** | Kills AMD 1-cycle fast path |
| **IMUL is the bottleneck** | 2 ports vs 4 for simple ALU |
| **2+ independent streams** | Saturates IMUL ports (0.5→1 iter/cycle) |
| **Match store/load sizes** | Store forwarding success (4-5 cyc vs 15+ cyc) |
| **Never merge stores into larger load** | Forwarding failure (+12 cyc penalty) |
