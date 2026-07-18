# x86-64 Execution Port Pressure Reference

> Source: Agner Fog's Instruction Tables v2025-Sep-20, Intel SDM Vol. 4,
> AMD PPR for Zen 2, and uops.info measurement data.
> All throughput = cycles per instruction (CPI). Lower = better.

---

## 1. Intel Skylake+ (Client: SKL / KBL / CFL — Server: SKX / CLX)

### 1.1 Port Map

```
┌──────┬──────────────────────────────────────────────────────────────────┐
│ Port │ Execution Units                                                 │
├──────┼──────────────────────────────────────────────────────────────────┤
│ P0   │ ALU simple+complex, IMUL, AES, CRC32, FP mul/add, INT div      │
│ P1   │ ALU simple, complex (BMI), INT div, FP div, vector ALU         │
│ P2   │ Load + AGU (address generation)                                 │
│ P3   │ Load + AGU (address generation)                                 │
│ P4   │ Store data                                                      │
│ P5   │ ALU simple, branch, shuffle, vector shuffle                     │
│ P6   │ ALU simple, branch                                              │
│ P7   │ Store address (AGU)                                             │
└──────┴──────────────────────────────────────────────────────────────────┘
```

**Client SKL** = 8 ports (P0-P7).
**Server SKX/CLX** = 10 ports (adds P0b load port + P8 store address).

### 1.2 Instruction → Port Mapping (Skylake Client)

| Instruction | Form | μops | Latency | Ports | Throughput (CPI) |
|---|---|---|---|---|---|
| `ADD r64, r/m64` | reg,reg | 1 | 1 | P0,1,5,6 | 0.25 |
| `ADD r64, r/m64` | reg,[mem] | 1 | 1 | P0,1,5,6 | 0.25 |
| `ADD r64, imm32` | reg,imm | 1 | 1 | P0,1,5,6 | 0.25 |
| `AND r64, r/m64` | reg,reg | 1 | 1 | P0,1,5,6 | 0.25 |
| `AND r64, imm32` | reg,imm | 1 | 1 | P0,1,5,6 | 0.25 |
| `OR r64, r/m64` | reg,reg | 1 | 1 | P0,1,5,6 | 0.25 |
| `OR r64, imm32` | reg,imm | 1 | 1 | P0,1,5,6 | 0.25 |
| `XOR r64, r/m64` | reg,reg | 1 | 1 | P0,1,5,6 | 0.25 |
| `SHL r64, imm8` | reg,imm | 1 | 1 | P0,1,5,6 | 0.25 |
| `SHR r64, imm8` | reg,imm | 1 | 1 | P0,1,5,6 | 0.25 |
| `MOV r64, r64` | reg,reg | 0 (eliminated) | 0 | — (zero-latency, resolved in rename) | 0 |
| `MOV r64, [mem]` | reg,[mem] | 1 | 4-5 (L1D hit) | P2,P3 | 0.5 |
| `MOV [mem], r64` | [mem],reg | 2 | — | P4 (data) + P7 (addr) | 0.5 |
| `MOV [mem], imm` | [mem],imm | 2 | — | P4 (data) + P7 (addr) | 0.5 |
| `IMUL r64, r/m64` | 2-op | 1 | 3 | P0,P1 | 0.5 |
| `IMUL r64, r/m64, imm32` | 3-op | 1 | 3 | P0,P1 | 0.5 |
| `LEA r64, [addr]` | complex | 1 | 1 | P0,P1,P5 | 0.33 |
| `LEA r64, [reg+reg*1+off]` | simple | 1 | 1 | P0,P1,P5,P6 | 0.25 |
| `JMP/Jcc` | branch | 0 | 0 | resolved in rename if predicted | 0 |
| `INC/DEC r64` | reg | 1 | 1 | P0,1,5,6 | 0.25 |

### 1.3 Key Skylake Port Facts

- **Simple integer ALU**: 4-wide (P0, P1, P5, P6) — the workhorse of ALU-heavy loops
- **IMUL**: 2-wide (P0, P1) — **half the throughput** of simple ALU
- **MOV reg,reg**: eliminated in register rename — **zero cost, zero ports**
- **Loads**: 2-wide (P2, P3) — can sustain 2 loads/cycle
- **Stores**: 2 μops each (addr on P7, data on P4) — **max 0.5 stores/cycle** (1 store-address port)

---

## 2. AMD Zen 2 (Family 17h)

### 2.1 Port Map

```
┌──────┬──────────────────────────────────────────────────────────────────┐
│ Port │ Execution Units                                                 │
├──────┼──────────────────────────────────────────────────────────────────┤
│ P0   │ ALU simple, branch (ALU pipe 0)                                 │
│ P1   │ ALU simple, complex (IMUL, DIV, CRC32) (ALU pipe 1)            │
│ P2   │ Load AGU 0                                                      │
│ P3   │ Load AGU 1                                                      │
│ P4   │ ALU simple, branch (ALU pipe 2)                                 │
│ P5   │ ALU simple, complex (IMUL, DIV) (ALU pipe 3)                   │
│ P6   │ Store address AGU 0                                             │
│ P7   │ Store address AGU 1                                             │
│ S0   │ Store data 0                                                    │
│ S1   │ Store data 1                                                    │
└──────┴──────────────────────────────────────────────────────────────────┘
```

**Note**: AMD Zen 2 has a 6-wide dispatch but only 4 simple-ALU-capable integer pipes
(P0, P1, P4, P5), with 2 of those (P1, P5) also handling complex ops (IMUL, DIV).
Store ports are separate: 2 AGUs for store addresses (P6, P7) + dedicated store-data paths.

### 2.2 Instruction → Port Mapping (Zen 2)

| Instruction | Form | μops | Latency | Ports | Throughput (CPI) |
|---|---|---|---|---|---|
| `ADD r64, r/m64` | reg,reg | 1 | 1 | P0,1,4,5 | 0.25 |
| `ADD r64, r/m64` | reg,[mem] | 1+1(load) | 1+4 | P2/3 + P0,1,4,5 | 0.25* |
| `ADD r64, imm32` | reg,imm | 1 | 1 | P0,1,4,5 | 0.25 |
| `AND r64, r/m64` | reg,reg | 1 | 1 | P0,1,4,5 | 0.25 |
| `AND r64, imm32` | reg,imm | 1 | 1 | P0,1,4,5 | 0.25 |
| `OR r64, r/m64` | reg,reg | 1 | 1 | P0,1,4,5 | 0.25 |
| `OR r64, imm32` | reg,imm | 1 | 1 | P0,1,4,5 | 0.25 |
| `XOR r64, r/m64` | reg,reg | 1 | 1 | P0,1,4,5 | 0.25 |
| `SHL r64, imm8` | reg,imm | 1 | 1 | P0,1,4,5 | 0.25 |
| `SHR r64, imm8` | reg,imm | 1 | 1 | P0,1,4,5 | 0.25 |
| `MOV r64, r64` | reg,reg | 0 (eliminated) | 0 | — (zero-latency) | 0 |
| `MOV r64, [mem]` | reg,[mem] | 1 | 4 (L1D hit) | P2,P3 | 0.5 |
| `MOV [mem], r64` | [mem],reg | 2 | — | P6/P7 (addr) + S0/S1 (data) | 0.5 |
| `MOV [mem], imm` | [mem],imm | 2 | — | P6/P7 (addr) + S0/S1 (data) | 0.5 |
| `IMUL r64, r/m64` | 2-op | 1 | 3 | P1,P5 | 0.5 |
| `IMUL r64, r/m64, imm32` | 3-op | 1 | 3 | P1,P5 | 0.5 |
| `LEA r64, [addr]` | complex | 1 | 1 | P0,1,4,5 | 0.25 |
| `INC/DEC r64` | reg | 1 | 1 | P0,1,4,5 | 0.25 |

*\* reg,[mem] form: the load μop goes to P2 or P3, the ALU μop goes to P0,1,4,5. These can issue in parallel.*

### 2.3 Key Zen 2 Port Facts

- **Simple integer ALU**: 4-wide (P0, P1, P4, P5) — same as Skylake
- **IMUL**: 2-wide (P1, P5 only) — **half the throughput** of simple ALU (same as Skylake)
- **MOV reg,reg**: eliminated — zero cost
- **Loads**: 2-wide (P2, P3) — same as Skylake
- **Stores**: 2 address AGUs (P6, P7) + 2 store-data paths — **max 2 stores/cycle** (better than Skylake's 1 store-address port!)
- Zen 2 can do **2 store addresses per cycle** vs Skylake's **1**

---

## 3. Comparison: Skylake vs Zen 2

### 3.1 Simple Integer (ADD, AND, OR, SHL, MOV r,r)

| Property | Intel Skylake | AMD Zen 2 |
|---|---|---|
| ALU ports | P0, P1, P5, P6 (4 ports) | P0, P1, P4, P5 (4 ports) |
| Max throughput | 4 simple ALU ops/cycle | 4 simple ALU ops/cycle |
| Latency | 1 cycle | 1 cycle |

### 3.2 IMUL (16-bit × 16-bit → 32-bit and wider)

| Property | Intel Skylake | AMD Zen 2 |
|---|---|---|
| IMUL ports | P0, P1 (2 ports) | P1, P5 (2 ports) |
| Max throughput | 2 IMUL/cycle | 2 IMUL/cycle |
| Latency | 3 cycles | 3 cycles |

### 3.3 Memory Operations

| Property | Intel Skylake (client) | AMD Zen 2 |
|---|---|---|
| Load ports | P2, P3 (2 ports) | P2, P3 (2 ports) |
| Max load throughput | 2 loads/cycle | 2 loads/cycle |
| Store-address ports | P7 (1 port) | P6, P7 (2 ports!) |
| Store-data ports | P4 (1 port) | S0, S1 (2 paths) |
| Max store throughput | **1 store/cycle** | **2 stores/cycle** |

---

## 4. Store Address vs Store Data

### 4.1 Why Stores Cost 2 μops

A store like `MOV [rbx + rcx*8], rax` is split into:

```
μop 1: Store Address AGU — computes effective address [rbx + rcx*8]
        → dispatched to store-address port
μop 2: Store Data — writes rax's value into the store buffer
        → dispatched to store-data port
```

The store buffer then handles the actual cache-line write.

### 4.2 Port Assignments

```
┌──────────────┬─────────────────────────┬──────────────────────────────┐
│              │ Intel Skylake (client)  │ AMD Zen 2                    │
├──────────────┼─────────────────────────┼──────────────────────────────┤
│ Store Addr   │ P7 only                 │ P6 and P7                    │
│ Store Data   │ P4 only                 │ S0 and S1 (separate paths)   │
│ Max stores   │ 1 per cycle            │ 2 per cycle                  │
│ per cycle    │ (limited by 1 addr port)│ (2 addr + 2 data paths)      │
└──────────────┴─────────────────────────┴──────────────────────────────┘
```

**Intel Skylake-X/Skylake Server** adds P8 (second store-address port), bringing it to 2 stores/cycle as well.

### 4.3 Compiler Implication

For the aziky: **every store to memory costs 2 μops on the port scheduler**. A loop body with 2 stores consumes both store-address slots on Skylake, or 1 store-address slot on Zen 2.

---

## 5. Port Bottlenecks in ALU-Heavy Loops

### 5.1 The IMUL Bottleneck

The most common bottleneck in LCG (Linear Congruential Generator) loops:

```asm
; Typical LCG iteration: state = state * 1664525 + 1013904223
imul rax, rax, 1664525      ; 1 μop → P0 or P1 (Skylake), P1 or P5 (Zen2)
add  rax, 1013904223        ; 1 μop → P0,1,5,6 (Skylake), P0,1,4,5 (Zen2)
and  rax, 0xFFFFFFFF        ; 1 μop → P0,1,5,6 (Skylake), P0,1,4,5 (Zen2)
```

**Bottleneck**: The `imul` limits throughput to **2 iterations/cycle** even though there are 4 ALU ports. The simple ALU ops (add, and) have plenty of port capacity, but the IMUL is the bottleneck.

**Mitigation**: Run 2-4 independent LCG streams in parallel (register-level parallelism):

```asm
; 2-stream LCG: throughput = 2 IMUL/cycle = 1 iteration per half-cycle
imul r8,  r8,  1664525     ; P0,P1
imul r9,  r9,  1664525     ; P0,P1 (parallel!)
add  r8,  1013904223       ; P0,1,5,6
add  r9,  1013904223       ; P0,1,5,6
and  r8,  mask              ; P0,1,5,6
and  r9,  mask              ; P0,1,5,6
```

With 2 independent streams: 2 IMULs in one cycle → **1 iteration/cycle** instead of 0.5.
With 4 independent streams: **2 iterations/cycle** (fills all 2 IMUL ports).

### 5.2 Store Port Bottleneck (Skylake Client)

```asm
; Loop with 2 stores per iteration on Skylake:
mov [rbx + r13*8], rax      ; μops: 1 addr (P7) + 1 data (P4)
mov [rbx + r14*8], rdx      ; μops: 1 addr (P7) + 1 data (P4)  ← CONFLICT!
```

On Skylake client, **2 stores = 2 P7 μops but only 1 P7 slot/cycle** → bottleneck.
On Zen 2, **2 stores = 2 addr AGUs (P6, P7) → no conflict**, sustain 2 stores/cycle.

### 5.3 Load Port Bottleneck

```asm
; 3 loads per iteration:
mov rax, [rbx]               ; P2 or P3
mov rdx, [rbx + 8]           ; P2 or P3
mov rcx, [rbx + 16]          ; P2 or P3  ← 3rd load needs 3rd port
```

Only 2 load ports → **3 loads/cycle max on both architectures** (2 per cycle sustained).
With 4+ independent loads: 2/cycle maximum.

### 5.4 Common Bottleneck Patterns in ALU Loops

| Pattern | Bottleneck Port | Max Throughput | Fix |
|---|---|---|---|
| 1 IMUL + ALU per iter | P0/P1 (IMUL) | 0.5 iter/cycle (1 IMUL/cycle, 2 ports shared) | 2+ independent streams |
| 2 stores per iter (SKL) | P7 (store addr) | 0.5 iter/cycle | Reduce stores or use Zen2/Server |
| 3+ loads per iter | P2/P3 | 2 loads/cycle max | Use LEA for address calc, reuse values |
| Pure ALU (ADD/AND/OR/SHL) | None (4 ports) | 4 ops/cycle | Already optimal; interleave to avoid data deps |
| IMUL + 2 stores (SKL) | P0/P1 + P7 | 0.5 iter/cycle | Parallel streams, reduce stores |

### 5.5 Data Dependency Chains

Beyond port pressure, **data dependency chains** limit throughput:

```asm
; BAD: dependency chain (3-cycle latency per iteration)
imul rax, rax, 1664525      ; rax←f(rax)  [3 cycles]
add  rax, 1013904223        ; rax←f(rax)  [1 cycle, waits for imul]
and  rax, mask              ; rax←f(rax)  [1 cycle]
; Total: 5 cycles per iteration (dependency chain)
; Throughput: 0.2 iter/cycle

; GOOD: 2 independent streams (no cross-chain dependency)
imul r8,  r8, 1664525       ; r8←f(r8)
imul r9,  r9, 1664525       ; r9←f(r9) — independent!
add  r8,  1013904223
add  r9,  1013904223
and  r8,  mask
and  r9,  mask
; r8 chain: 3+1+1 = 5 cycles (but r9 runs in parallel!)
; Throughput: 2 iters per 5 cycles = 0.4 iter/cycle (2x improvement)
```

---

## 6. Quick Reference: Compiler Emission Strategy

### For the aziky's LCG loops:

1. **Default**: Single-stream LCG → 1 IMUL/cycle on P0 (or P1), ~0.5 iter/cycle
2. **Optimal**: 2-stream ILP → 2 IMULs/cycle on P0+P1, ~1 iter/cycle
3. **Best**: 4-stream ILP → saturates both IMUL ports, ~2 iters/cycle

### Port budget per cycle (Skylake):

```
Budget:  P0(1) + P1(1) + P2(1) + P3(1) + P4(1) + P5(1) + P6(1) + P7(1) = 8 ops/cycle

Per LCG iteration (single stream):
  imul  → 1 of P0/P1      (leaves P0 or P1 free)
  add   → 1 of P0,1,5,6   (fills remaining ALU)
  and   → 1 of P0,1,5,6   (needs another slot)
  store → P4+P7            (data + addr)

  Total: 3 ALU μops + 2 store μops = 5 μops
  Port pressure: 3 of 4 ALU ports used → OK
  But store pressure: 1 of 1 P7 → BOTTLENECK if >1 store
```

### Summary Table for Compiler Decision-Making

| Instruction | Skylake Ports | Zen 2 Ports | Throughput (both) | Latency (both) |
|---|---|---|---|---|
| ADD r,r | P0,P1,P5,P6 | P0,P1,P4,P5 | 0.25 CPI | 1 cyc |
| ADD r,imm | P0,P1,P5,P6 | P0,P1,P4,P5 | 0.25 CPI | 1 cyc |
| AND r,r | P0,P1,P5,P6 | P0,P1,P4,P5 | 0.25 CPI | 1 cyc |
| AND r,imm | P0,P1,P5,P6 | P0,P1,P4,P5 | 0.25 CPI | 1 cyc |
| OR r,r | P0,P1,P5,P6 | P0,P1,P4,P5 | 0.25 CPI | 1 cyc |
| OR r,imm | P0,P1,P5,P6 | P0,P1,P4,P5 | 0.25 CPI | 1 cyc |
| SHL r,imm | P0,P1,P5,P6 | P0,P1,P4,P5 | 0.25 CPI | 1 cyc |
| SHR r,imm | P0,P1,P5,P6 | P0,P1,P4,P5 | 0.25 CPI | 1 cyc |
| MOV r,r | eliminated | eliminated | 0 CPI | 0 cyc |
| MOV r,[mem] | P2,P3 | P2,P3 | 0.5 CPI | 4-5 cyc |
| MOV [mem],r | P4+P7 (2μops) | P6/7+S0/1 (2μops) | 0.5 CPI | — |
| IMUL r,r | P0,P1 | P1,P5 | 0.5 CPI | 3 cyc |
| IMUL r,r,imm | P0,P1 | P1,P5 | 0.5 CPI | 3 cyc |
| INC/DEC r | P0,P1,P5,P6 | P0,P1,P4,P5 | 0.25 CPI | 1 cyc |
| LEA (complex) | P0,P1,P5 | P0,P1,P4,P5 | 0.33 CPI | 1 cyc |
| LEA (simple) | P0,P1,P5,P6 | P0,P1,P4,P5 | 0.25 CPI | 1 cyc |
