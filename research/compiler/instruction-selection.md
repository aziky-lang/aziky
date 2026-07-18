# Instruction Selection Research: Modern Approaches for x86-64 Compilers

## 1. The Spectrum of ISel Approaches (Best to Worst for Quality)

| Approach | Quality | Compile Speed | Complexity | Who Uses It |
|----------|---------|---------------|------------|-------------|
| **Graph/PBQP (whole-function)** | Best | Slow | Very High | LLVM GlobalISel (experimental) |
| **DAG tiling + peephole** | Very Good | Medium | High | LLVM SelectionDAG (primary) |
| **Greedy tree tiling (backward walk)** | Good | Fast | Medium | Cranelift ISLE, GCC RTL |
| **Macro expansion (per-instr)** | OK | Fastest | Low | Your current dedicated emitters |

**Key insight for aziky**: Your current dedicated emitters (LCG loops, ring-write, branch-LCG) are already **macro expansion** for hot patterns. The generic LIR path does a linear walk. The gap is: **no cross-instruction optimization in the generic path** — each `RuntimeInstr` is emitted independently with no attempt to fuse or match across instruction boundaries.

---

## 2. What Works Better Than Naive Pattern Matching

### 2.1 Tree Pattern Matching (Aho-Corasick for IR)

The classic approach from Aho & Johnson (1976): represent each statement as a **data flow tree**, then use **dynamic programming** to find a cost-optimal tiling.

- **burg/iburg**: Tools that compile tree grammars into C tree pattern matchers
  - `burg`: Precomputes states at codegen time (constant costs, fast)
  - `iburg`: Computes states at ISel time (dynamic costs, slightly slower)
- **Key limitation**: Only matches within a single statement. Cannot fuse across statements.

**Your compiler already has the building blocks**: Your `RuntimeInstr` enum already represents operations as flat SSA-like instructions. Each `BinOp`, `Cmp`, `LoadIndex` etc. is essentially a node in a tree. A tree matcher could walk each `RuntimeInstr` and decide how to emit it based on the operand patterns (imm/reg/stack).

### 2.2 DAG Tiling (Beyond Single Statements)

When you have multiple uses of the same value (e.g., `x = a + b; y = x * 2; z = x >> 1`), the IR forms a DAG, not a tree. DAG tiling can:

- **Recognize addressing modes**: `x = load(base, index); y = x + 1` → `lea rax, [rbx + rcx*8 + 1]`
- **Sink loads**: `x = load(addr); y = x + a` can become `add reg, [addr]` if x is used once
- **Fuse compare+branch**: Already implemented in your `runtime_cmp_jumpifzero_fusion_candidate`

**Key paper**: "Near-Optimal Instruction Selection on DAGs" (Ebner et al., 2008)
- Uses dynamic programming on DAGs
- Achieves near-optimal code quality
- Handles x86 addressing modes and RMW patterns

### 2.3 SSA-Graph Instruction Selection (Whole-Function)

The most powerful approach: treat the entire function's SSA form as a graph and find an optimal cover.

**Key paper**: "Generalized Instruction Selection using SSA-Graphs" (Ebner, Brandner, Scholz, Krall — LCTES 2008)
- Maps the ISel problem to **PBQP (Partitioned Boolean Quadratic Problem)**
- NP-complete in theory, but solved efficiently with heuristic solvers
- 99.83% of instances solved optimally
- 57% speedups on DSP kernels, 10% on SPECINT 2000
- Handles complex patterns: div-mod coalescing, pre/post increment addressing, SIMD

**What you can borrow**: The SSA-graph approach is overkill for your compiler (which doesn't have SSA form in the traditional sense), but the **concept of multi-instruction pattern matching** is directly applicable. Your fusion candidates (cmp+jump, bit-test, bitset-store) are already doing this manually.

---

## 3. How Modern Compilers Do ISel

### 3.1 LLVM SelectionDAG (Primary Approach)

LLVM's main ISel pipeline:

```
LLVM IR → SelectionDAG → LegalizeTypes → Legalize → DAGCombiner → Select → MachineSched
```

**Key design decisions**:
1. **One DAG per basic block** (not whole function) — this is a pragmatic compromise
2. **Legalization phases** before selection — types and operations are normalized to what the target supports
3. **DAGCombiner** performs peephole optimizations on the DAG (strength reduction, constant folding)
4. **Pattern matching** via TableGen-generated match tables (precomputed itemsets)
5. **Two-address instruction fixup** (x86 `add eax, ebx` pattern) happens during scheduling

**What you can borrow**:
- The **LegalizeTypes** concept: before ISel, ensure all types are supported. Your compiler already handles this implicitly, but making it explicit would catch edge cases.
- **DAGCombiner-style rewrites**: Combine `x * 2` → `x << 1`, `x & 0xFF` → `movzx`, etc. as a separate pass before ISel.
- The **match table** approach: Precompute which patterns match which opcodes for fast lookup.

### 3.2 LLVM GlobalISel (Next-Generation)

LLVM's replacement for SelectionDAG, addressing three problems:
1. **Compile time**: SelectionDAG introduces a separate IR; GlobalISel operates on MIR directly
2. **Granularity**: Works on whole functions, not just basic blocks
3. **Modularity**: Shared pipeline between fast and optimized selectors

**Pipeline**:
```
IRTranslator → Legalizer → RegBankSelect → InstructionSelect
```

- **IRTranslator**: Converts LLVM IR to Generic MIR
- **Legalizer**: Converts unsupported types/operations to supported equivalents
- **RegBankSelect**: Assigns virtual registers to register banks (GPR, FPR, etc.)
- **InstructionSelect**: Greedy pattern matching with cost model

**What you can borrow**:
- The **four-phase pipeline** is clean and composable
- **RegBankSelect** concept: Before ISel, decide which "class" of register each value needs (integer vs. floating-point). Your compiler's `RuntimeSlotMap` already does this implicitly.
- **Greedy with backtracking**: GlobalISel tries patterns greedily but can fall back

### 3.3 Cranelift ISLE (Most Relevant for Your Compiler)

Cranelift uses **ISLE (Instruction Selection/Lowering Expressions)**, a term-rewriting DSL compiled into Rust match expressions.

**Key design**:
1. **Backward walk**: The compiler walks backward through CLIF instructions
2. **Greedy subtree matching**: Each instruction is matched along with its operands, greedily consuming subtrees
3. **Rules compiled to Rust match trees**: ISLE compiles to efficient `match` expressions
4. **External extractors**: Custom Rust code can check preconditions (e.g., "is this value used only once?")
5. **Extenders**: Allow combining multiple input instructions into one output

**Example ISLE rule**:
```lisp
(rule (lower (Add a b))
      (Add (RegReg (put_in_reg a) (put_in_reg b))))
```

**What you can borrow**:
- The **term-rewriting paradigm** is perfect for your compiler
- You already have a similar pattern: match on `RuntimeInstr` variants and emit code
- ISLE's **extractors** would let you express your fusion candidates declaratively
- The **compiled decision tree** approach is fast and deterministic

### 3.4 GCC RTL/GIMPLE

GCC uses a multi-level approach:
- **GIMPLE** → **RTL** conversion (similar to ISel)
- **Machine description** files (`.md`) define patterns in a Lisp-like language
- **Recog** function: pattern matching on RTL expressions
- **Split** patterns: Break complex operations into simpler ones
- **Peephole2**: Post-ISel peephole optimization

**What you can borrow**:
- The **split** concept: If a pattern doesn't match directly, split it into simpler pieces that do
- **Peephole2** pass: After register allocation, find and optimize instruction sequences

---

## 4. Practical Approaches for Aziky

### 4.1 Recommended Architecture: Three-Layer ISel

Given your constraint of zero external crates and hand-written x86-64 bytes, here's the architecture that would give you the best quality with manageable complexity:

```
Layer 1: Peephole Fusion (existing, extend)
  ├─ cmp + jumpIfZero → fused cmovcc/jcc
  ├─ load + cmp + jump → fused compare-and-branch
  ├─ shift + and → bt instruction
  ├─ load + or + store → bts instruction
  └─ NEW: add + load → lea with addressing mode
  
Layer 2: Per-Instruction Selection (generic LIR path, add DAG awareness)
  ├─ BinOp with one imm operand → immediate form (add rax, imm32)
  ├─ BinOp with one stack operand → mem form (add rax, [rsp+disp])
  ├─ Load from constant address → RIP-relative addressing
  ├─ Mov imm → optimal encoding (mov eax,imm32 vs mov rax,imm64)
  └─ Cmp with immediate → test/cmp with imm

Layer 3: Structural Peephole (NEW, post-ISel)
  ├─ xor reg, reg → zero idiom (already have for zero-regs)
  ├─ mov reg, reg → eliminated
  ├─ lea with no offset → mov (if single-use)
  └─ Combine consecutive stores/loads where possible
```

### 4.2 Concrete Implementation: Greedy Backward Walk with Tiling

This is closest to what Cranelift does and is the most practical approach for your compiler:

```rust
/// Instruction selector that walks the LIR backward and tiles
/// each instruction with its operand patterns.
fn select_instruction(
    instr: &RuntimeInstr,
    program: &RuntimeProgram,
    slot_map: &RuntimeSlotMap,
    code: &mut Vec<u8>,
) {
    match instr {
        // Layer 1: Multi-instruction fusions (check 2-4 instruction windows)
        RuntimeInstr::BinOp { dst, op: RuntimeBinOp::Add, lhs, rhs } => {
            // Check if next instruction loads from [dst + offset]
            // → fold into LEA
            // Check if next instruction is Cmp + JumpIfZero
            // → fold into add + jcc
        }
        
        // Layer 2: Per-instruction selection with operand pattern matching
        RuntimeInstr::BinOp { dst, op, lhs, rhs } => {
            match (op, classify_operand(lhs, slot_map), classify_operand(rhs, slot_map)) {
                (Add, Reg(r), Imm(i)) => emit_add_reg_imm(code, r, i),
                (Add, Reg(r1), Reg(r2)) => emit_add_reg_reg(code, r1, r2),
                (Add, Reg(r), Mem(disp)) => emit_add_reg_mem(code, r, disp),
                (Mul, Reg(r), Imm(i)) => {
                    if i.is_power_of_two() {
                        emit_shl_reg_imm(code, r, i.trailing_zeros());
                    } else {
                        emit_imul_reg_reg_imm(code, r, r, i);
                    }
                }
                // ... more patterns
            }
        }
    }
}

enum OperandClass {
    Reg(u8),
    Imm(i64),
    Mem(i32),        // [rsp + disp]
    RegMem(u8, i32), // [reg + disp]
}

fn classify_operand(op: &RuntimeOperand, slot_map: &RuntimeSlotMap) -> OperandClass {
    match op {
        RuntimeOperand::Imm(v) => OperandClass::Imm(*v as i64),
        RuntimeOperand::Slot(s) => {
            if let Some(reg) = slot_map.reg(*s) {
                OperandClass::Reg(reg)
            } else if let Some(idx) = slot_map.stack_index(*s) {
                OperandClass::Mem(stack_slot_disp(idx))
            } else {
                OperandClass::Reg(0) // fallback
            }
        }
    }
}
```

### 4.3 Key Fusion Patterns to Add (x86-64 Specific)

These are the most impactful fusions for x86-64 that you don't currently have:

1. **LEA sink** (saves a register and an instruction):
   ```
   x = load(base, index)
   y = x + offset
   → LEA y, [base + index*scale + offset]
   ```

2. **MOV-CMOV chain** (eliminates branch for small conditionals):
   ```
   Cmp a, b
   JumpIfCmpFalse(target)
   Mov dst, true_val
   ...
   Mov dst, false_val
   → CMOVcc dst, true_val, a, b
   ```

3. **INC/DEC fusion** (shorter encoding than add reg, 1):
   ```
   BinOpInPlace{Add, dst, Imm(1)} → INC reg (1 byte vs 4 bytes)
   ```

4. **TEST + Jcc** (eliminates explicit Cmp):
   ```
   Cmp(x, 0) + JumpIfZero → TEST x, x + JZ
   ```

5. **Memory operand sinking** (x86 CISC advantage):
   ```
   y = load(slot_a)
   x = BinOp(Add, y, slot_b)
   → ADD reg, [rsp+disp_b] (if y has single use)
   ```

### 4.4 Cost Model for x86-64

For your compiler's purposes, approximate instruction costs:

| Instruction | Latency | Throughput | Size |
|------------|---------|------------|------|
| `mov reg, reg` | 0.25 | 3/cycle | 3 bytes |
| `mov reg, imm32` | 0.25 | 1/cycle | 5-7 bytes |
| `add reg, imm32` | 0.25 | 1/cycle | 4-7 bytes |
| `add reg, reg` | 0.25 | 1/cycle | 3 bytes |
| `imul reg, imm32` | 3 | 1/cycle | 7 bytes |
| `shl reg, imm` | 1 | 1/cycle | 3-4 bytes |
| `lea reg, [addr]` | 1 | 1/cycle | 4-8 bytes |
| `test reg, reg` | 0.25 | 1/cycle | 3 bytes |
| `jcc` | 0.5-1.5 | 1/cycle | 2-6 bytes |
| `cmov reg, reg` | 1 | 1/cycle | 4 bytes |
| `bt reg, reg` | 1 | 2/cycle | 4 bytes |
| `bts [mem], reg` | 6 | 1/cycle | 5-8 bytes |

**Key principle**: Prefer smaller encodings for hot code (instruction cache), prefer fewer instructions for cold code (code size).

---

## 5. Papers and Resources

### Must-Read Papers

1. **"Generalized Instruction Selection using SSA-Graphs"** (Ebner et al., LCTES 2008)
   - PBQP-based whole-function ISel
   - Handles complex patterns (div-mod, pre/post increment, SIMD)
   - 57% speedup on DSP kernels
   - https://llvm.org/pubs/2008-06-LCTES-ISelUsingSSAGraphs.pdf

2. **"Near-Optimal Instruction Selection on DAGs"** (Ebner et al., LCTES 2008)
   - Dynamic programming on DAGs for near-optimal ISel
   - Handles x86 addressing modes
   - https://llvm.org/pubs/2008-12-LCTES-ISelOnDAGs.pdf

3. **"Complete and Practical Universal Instruction Selection"** (Blindell, KTH PhD Thesis, 2016)
   - Comprehensive survey of all ISel approaches
   - The definitive reference
   - https://www.diva-portal.org/smash/get/diva2:1139368/FULLTEXT01.pdf

4. **"Instruction Selection: An Extensive and Modern Literature Review"** (Blindell, 2013)
   - Survey covering tree matching, DAG matching, PBQP, ILP approaches
   - https://www.diva-portal.org/smash/get/diva2:1080985/FULLTEXT01.pdf

### Practical References

5. **Cranelift ISLE Documentation**
   - Term-rewriting DSL for instruction selection
   - Compiles to efficient Rust match trees
   - https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/isle/README.md

6. **LLVM GlobalISel Documentation**
   - Four-phase pipeline: IRTranslator → Legalizer → RegBankSelect → InstructionSelect
   - https://llvm.org/docs/GlobalISel/

7. **LLVM SelectionDAG Documentation**
   - DAG-based ISel with legalization and combining
   - https://llvm.org/docs/CodeGenerator.html

8. **"TPDE: A Fast Adaptable Compiler Back-End Framework"** (2024)
   - Modern take on fast ISel for JIT compilation
   - https://arxiv.org/pdf/2401.xxxxx (search for TPDE CGO 2024)

### Classic References

9. **burg** (Fraser et al., 1992): Code generator generator using tree pattern matching
10. **iburg** (Fraser et al., 1994): Dynamic cost version of burg
11. **"A Transformational Approach to Compiler Construction"** (Tjiang, PhD Thesis, 1992)

---

## 6. Summary: What to Do Next

For the aziky specifically, the recommended path is:

1. **Immediate**: Extend your existing fusion system with the patterns from Section 4.3
2. **Short-term**: Add `OperandClass` classification to the generic LIR path so each instruction can select its encoding based on operand types (reg/imm/mem)
3. **Medium-term**: Implement a cost-model-driven peephole pass that runs after ISel to catch suboptimal encodings
4. **Long-term**: If you want optimal code, implement DAG tiling on your `RuntimeInstr` SSA form (but this is likely overkill for the benchmarks you're targeting)

The most impactful single change would be **operand classification + pattern matching in the generic LIR path** — this would give you CISC-style memory operand folding for free on x86-64, which is exactly what LLVM's SelectionDAG does with its addressing mode matching.
