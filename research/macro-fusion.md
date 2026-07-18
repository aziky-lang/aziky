# x86-64 Macro-Fusion Reference for Aziky

> Source: Agner Fog "The microarchitecture of Intel, AMD and VIA CPUs" (2024),  
> verified against uops.info measurements and Intel/AMD optimization manuals.

---

## 1. ALU+Branch Fusion Tables (Primary Compiler Target)

### 1.1 What Fuses: Instruction Pair Compatibility

| ALU Instruction | Skylake | Ice Lake | Alder Lake P (GC) | Zen 1/2 | Zen 3 | Zen 4 | Zen 5 |
|---|---|---|---|---|---|---|---|
| **CMP** | JE/JNE/JB/JA/JL/JG (+inv) | JE/JNE/JB/JA/JL/JG (+inv) | Same as Ice Lake | All Jcc | All Jcc | All Jcc | All Jcc |
| **TEST** | All Jcc | All Jcc | All Jcc | All Jcc | All Jcc | All Jcc | All Jcc |
| **ADD, SUB** | JE/JNE/JB/JA/JL/JG (+inv) | JE/JNE/JB/JA/JL/JG (+inv) | Same as Ice Lake | ✗ | All Jcc | All Jcc | All Jcc |
| **INC, DEC** | JE/JNE/JL/JG (+inv) | JE/JNE/JL/JG (+inv) | Same as Ice Lake | ✗ | All Jcc | All Jcc | All Jcc |
| **AND** | All Jcc | All Jcc | Same as Ice Lake | ✗ | All Jcc | All Jcc | All Jcc |
| **OR, XOR** | ✗ | ✗ | ✗ | ✗ | All Jcc | All Jcc | All Jcc |
| **ADC, SBB** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| **NEG, NOT** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| **SHIFT, ROTATE** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| **MOV** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| **NOP** | ✗ | ✗ | ✗ | ✗ | ✗ | ✓¹ | ✗ |

**Key**: `+inv` = inverse condition also works (e.g., JE↔JNE, JB↔JAE).  
**✗** = Does NOT fuse.  
**All Jcc** = All conditional jumps (JE, JNE, JB, JAE, JA, JBE, JL, JGE, JG, JLE, JO, JNO, JS, JNS, JP, JNP).  
¹ Zen 4 only: NOP fuses with preceding instruction. Zen 5 cannot fuse NOPs.

### 1.2 Branch Conditions That DO NOT Fuse (Intel Skylake/Ice Lake/Alder Lake)

On **Skylake through Alder Lake P**, CMP/ADD/SUB can only fuse with branches that test **ZF and/or CF**:

| Branch | Tests | CMP fuses? | ADD/SUB fuses? | INC/DEC fuses? | TEST/AND fuses? |
|---|---|---|---|---|---|
| JE / JZ | ZF=1 | ✓ | ✓ | ✓ | ✓ |
| JNE / JNZ | ZF=0 | ✓ | ✓ | ✓ | ✓ |
| JB / JC / JNAE | CF=1 | ✓ | ✓ | ✗ | ✓ |
| JNB / JNC / JAE | CF=0 | ✓ | ✓ | ✗ | ✓ |
| JA / JNBE | CF=0 & ZF=0 | ✓ | ✓ | ✗ | ✓ |
| JBE / JNA | CF=1 \| ZF=1 | ✓ | ✓ | ✗ | ✓ |
| JL / JNGE | SF≠OF | ✓ | ✓ | ✓ | ✓ |
| JNL / JGE | SF=OF | ✓ | ✓ | ✓ | ✓ |
| JG / JNLE | ZF=0 & SF=OF | ✓ | ✓ | ✓ | ✓ |
| JLE / JNG | ZF=1 \| SF≠OF | ✓ | ✓ | ✓ | ✓ |
| **JO** | OF=1 | **✗** | **✗** | **✗** | ✓ |
| **JNO** | OF=0 | **✗** | **✗** | **✗** | ✓ |
| **JS** | SF=1 | **✗** | **✗** | **✗** | ✓ |
| **JNS** | SF=0 | **✗** | **✗** | **✗** | ✓ |
| **JP / JPE** | PF=1 | **✗** | **✗** | **✗** | ✓ |
| **JNP / JPO** | PF=0 | **✗** | **✗** | **✗** | ✓ |

> **Compiler takeaway**: On Intel, CMP/ADD/SUB/INC/DEC **cannot** fuse with JO, JNO, JS, JNS, JP, JNP.  
> Only TEST and AND can fuse with ALL branch types.  
> On **AMD Zen 3+**, all ALU instructions fuse with ALL conditional jumps.

---

## 2. Operand Encoding Constraints

### 2.1 Memory Operand Rules

| Constraint | Skylake | Ice Lake | Alder Lake P (GC) | Zen 1/2 | Zen 3 | Zen 4 | Zen 5 |
|---|---|---|---|---|---|---|---|
| **Reg + Reg** | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| **Reg + Imm** | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| **Reg + [mem]** | ✓ | **✗** | **✓**² | ✓ | ✓ | ✓ | ✓ |
| **[mem] + Imm** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| **[mem] (no imm)** | ✓ | **✗** | **✓**² | ✓ | ✓ | ✓ | ✓ |
| **Memory dest** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |

² **Golden Cove (Alder Lake P)** re-enabled memory-operand fusion that Ice Lake disabled. This is a significant change — Ice Lake removed memory fusion entirely, but Golden Cove brought it back.

### 2.2 Addressing Mode Constraints

| Constraint | Skylake | Ice Lake | Alder Lake P | Zen 1–5 |
|---|---|---|---|---|
| **RIP-relative addressing** | ✗ (32-bit mode only) | N/A (mem already banned) | ✗ | ✗ |
| **Displacement + Immediate** | ✗ | N/A | ✗ | ✗ |
| **Scaled index (e.g. [rsi+rbx*4])** | ✓ | N/A | ✓ | ✓ |
| **32-bit absolute + index** | ✓ | N/A | ✓ | ✓ |

---

## 3. Branch Encoding & Alignment Constraints

| Constraint | Skylake | Ice Lake | Alder Lake P | Zen 1–5 |
|---|---|---|---|---|
| **JECXZ** | ✗ | ✗ | ✗ | ✗ |
| **LOOP / LOOPE / LOOPNE** | ✗ | ✗ | ✗ | ✗ |
| **16-byte boundary crossing** | ✗ (SB uncertain, IVB+ ✓) | ✓ (no penalty) | ✓ | ✓ |
| **Cache line boundary** | ✗ | ✓ | ✓ | ✓ |
| **Instruction between pair** | ✗ | ✗ | ✗ | ✗ |
| **Branch hint prefixes** | ✓ (ignored) | ✓ (ignored) | ✓ (ignored) | ✓ (ignored) |
| **SIMD/VEX prefix on ALU** | N/A | N/A | N/A | ✗ |

---

## 4. Throughput & Port Constraints

| Property | Skylake | Ice Lake | Alder Lake P | Zen 1/2 | Zen 3 | Zen 4 | Zen 5 |
|---|---|---|---|---|---|---|---|
| **Fused branch execution port** | p0 or p6 | p0 or p6 | p0 or p6 | INT pipes | INT pipes | INT pipes | INT pipes |
| **Max fused pairs/decode cycle** | 2 | 2 | 2 | 1 | 1 | 1 | 1 |
| **Max fused branches throughput (NT)** | 2/cycle | 2/cycle | 2/cycle | 2/cycle | 2/cycle | 2/cycle | 3/cycle |
| **Max fused branches throughput (T)** | 1/cycle | 1/cycle | 1/cycle | 1/2 cycle | 1/cycle | 1/cycle | 2/cycle |
| **Fusion done by** | Decoders | After decode | After decode | Decoders | Decoders | Decoders | Decoders |
| **Total decoders** | 4 | 4 | 6 (P-core) | 4 | 4 | 4 | 4+4 (2-way) |
| **µop cache delivery** | 6/cycle | 6/cycle | 6/cycle | 5-6/cycle | 6/cycle | 6-9/cycle | 6/cycle×2 |

---

## 5. Practical Compiler Decision Rules

### 5.1 Universal Safe Fusions (all targets)

These pairs will macro-fuse on ALL listed architectures:

```asm
; SAFE ON EVERYTHING — always emit as adjacent pair
CMP  reg, reg      ; or CMP reg, imm
Jcc  label         ; any conditional jump except JECXZ/LOOP

TEST reg, reg      ; or TEST reg, imm
Jcc  label         ; any conditional jump

CMP  reg, [mem]    ; only if mem operand is supported (see table)
Jcc  label
```

### 5.2 Intel-Only Fusions (won't work on Zen 1/2)

```asm
; These fuse on Skylake+ but NOT on Zen 1 or Zen 2
ADD  reg, reg
Jcc  label         ; JE/JNE/JB/JA/JL/JG (not JS/JO/JP)

SUB  reg, reg
Jcc  label

INC  reg
Jcc  label         ; JE/JNE/JL/JG only (not JB/JA/JS/JO/JP)

AND  reg, reg
Jcc  label         ; all Jcc
```

### 5.3 AMD Zen 3+ Exclusive Fusions (wider than Intel)

```asm
; These ONLY fuse on Zen 3/4/5, not on any Intel or Zen 1/2
OR   reg, reg
Jcc  label         ; all Jcc

XOR  reg, reg      ; note: XOR reg,reg is also recognized as zeroing idiom
Jcc  label         ; all Jcc
```

### 5.4 Memory Operand Strategy

```asm
; OPTIMAL: Load first, then compare (fuses on ALL targets)
MOV   reg, [mem]
CMP   reg, imm     ; or CMP reg, reg
Jcc   label

; WORKS on Skylake but NOT on Ice Lake (or if targeting Ice Lake)
CMP   reg, [mem]
Jcc   label

; NEVER FUSES (on any target)
CMP   [mem], imm
Jcc   label         ; two issues: mem dest + mem+imm
```

---

## 6. µop Fusion (Separate from Macro-Fusion)

µop fusion is a DIFFERENT mechanism from macro-fusion. It combines two µops from a single instruction into one slot in the pipeline.

| Instruction | µop fusion? | Architecture |
|---|---|---|
| `CMP reg, [mem]` + `Jcc` (macro-fused) | Triple micro-macro-fusion (read+cmp+branch) | Core2, Nehalem, Sandy Bridge–Skylake |
| `LOCK CMPXCHG` | 1 µop (fused read-modify-write) | All |
| `XADD [mem], reg` | 1 µop (fused read-modify-write) | All |
| `CMP [mem], imm` | 2 µops: load + compare | All |
| `TEST [mem], imm` | 2 µops: load + test | All |

> **Note on Ice Lake**: Memory operands are excluded from macro-fusion entirely.  
> `CMP reg, [mem]` + `Jcc` produces TWO µops (no fusion).  
> On Golden Cove (Alder Lake P), memory fusion was re-enabled.

---

## 7. Summary Table: Compiler-Grade Decision Matrix

For a compiler emitting raw x86-64 bytes, here's the decision matrix:

| Emit Pattern | Skylake | Ice Lake | Alder Lake P | Zen 1/2 | Zen 3 | Zen 4 | Zen 5 | Recommendation |
|---|---|---|---|---|---|---|---|---|
| `CMP r,r` + `Jcc` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | **Always fuse** |
| `CMP r,imm` + `Jcc` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | **Always fuse** |
| `CMP r,[mem]` + `Jcc` | ✓ | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ | Fuse (except Ice Lake) |
| `TEST r,r` + `Jcc` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | **Always fuse** |
| `TEST r,imm` + `Jcc` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | **Always fuse** |
| `ADD r,r` + `Jcc` | ✓* | ✓* | ✓* | ✗ | ✓ | ✓ | ✓ | Only Zen 3+ safe |
| `SUB r,r` + `Jcc` | ✓* | ✓* | ✓* | ✗ | ✓ | ✓ | ✓ | Only Zen 3+ safe |
| `INC r` + `Jcc` | ✓* | ✓* | ✓* | ✗ | ✓ | ✓ | ✓ | Only Zen 3+ safe |
| `DEC r` + `Jcc` | ✓* | ✓* | ✓* | ✗ | ✓ | ✓ | ✓ | Only Zen 3+ safe |
| `AND r,r` + `Jcc` | ✓ | ✓ | ✓ | ✗ | ✓ | ✓ | ✓ | Intel + Zen 3+ safe |
| `OR r,r` + `Jcc` | ✗ | ✗ | ✗ | ✗ | ✓ | ✓ | ✓ | Zen 3+ only |
| `XOR r,r` + `Jcc` | ✗ | ✗ | ✗ | ✗ | ✓ | ✓ | ✓ | Zen 3+ only |
| `NOP` + preceding | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ | ✗ | Zen 4 only |

✓* = Only works with certain branch conditions (see §1.2). JE/JNE/JB/JA/JL/JG only, NOT JS/JO/JP.

---

## 8. Important Caveats for Raw Byte Emission

1. **16-byte alignment**: The branch instruction should NOT start at a 16-byte boundary or cross one (Skylake). Ice Lake+ removed this restriction.

2. **No instructions between**: The ALU and Jcc must be immediately adjacent with no instructions between them. Branch hint prefixes (2-byte hints like `0x3E`) are allowed but ignored.

3. **Decoder restriction on Intel**: On Skylake, if a fuseable pair lands in decoder 4 (the last), it gets delayed to the next cycle to check if the next instruction is fuseable. This means fuseable ALU instructions decode at a LOWER rate than non-fuseable ones, even in code with no branches.

4. **µop cache bypass**: When code runs from the µop cache (DSB), macro-fusion still occurs. When running from the legacy decoders, the decoder restriction applies.

5. **JECXZ never fuses**: This instruction is explicitly excluded from macro-fusion on ALL architectures.

6. **LOOP/LOOPE/LOOPNE never fuse**: Excluded on ALL architectures.

7. **AMD Zen 1/2 is most restrictive**: Only CMP and TEST can fuse. No ADD, SUB, INC, DEC, AND, OR, XOR.

8. **AMD Zen 3+ is most permissive**: All ALU+Jcc combinations fuse. No branch condition restrictions.

9. **Intel CMP cannot fuse with OF/PF/SF branches**: CMP does not produce OF, PF, or SF for all cases. Only TEST can set all flags. This is why CMP cannot fuse with JO/JNO/JS/JNS/JP/JNP.
