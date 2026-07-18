# Superoptimization Tools & Papers for x86-64

Research findings for building a custom x86-64 compiler with superoptimization.

---

## 1. STOKE — Stochastic Superoptimization

**Paper**: "Stochastic Superoptimization"  
**Authors**: Eric Schkufza, Rahul Sharma, Alex Aiken  
**Venue**: ASPLOS 2013  
**Citations**: 382 (landmark paper)  
**arXiv**: 1211.0557  
**GitHub**: https://github.com/StanfordPL/stoke (870 stars, 84 forks)

### What It Does
- Formulates x86-64 superoptimization as a **stochastic search problem** (MCMC sampling)
- Works on **loop-free binary x86-64** programs
- Uses random transformations to explore the space of all possible programs
- Encodes correctness (via test cases + formal verification) and performance improvement as terms in a cost function

### Optimizations Found in Practice
- Starting from `llvm -O0` binaries, STOKE produces code that **matches or outperforms** gcc `-O3`, icc `-O3`, and in some cases **expert hand-written assembly**
- Key example in the paper: **population count** — STOKE found a 17-instruction sequence that matches the hardware `popcnt` instruction
- Found optimizations involving **bit manipulation tricks** (multiply-based shifts, complex LEA chains)
- Discovered novel **instruction scheduling** patterns that compilers miss
- Found **strength reductions** across arithmetic, logic, and shift operations

### Key Techniques
- **MCMC (Markov Chain Monte Carlo)** random walk through program space
- **SMT verification** (Z3/CVC4) for formal correctness guarantees
- **Test case generation** for fast filtering before formal verification
- **Cost function** balancing program length, instruction latency, and correctness
- Supports AVX2 (Haswell+) and Sandy Bridge (partial)

### Related STOKE Papers
1. **"Data-Driven Equivalence Checking"** (OOPSLA 2013) — equivalence checking infrastructure
2. **"Stochastic Optimization of Floating-Point Programs with Tunable Precision"** (PLDI 2014) — trades accuracy for performance
3. **"Conditionally Correct Superoptimization"** (OOPSLA 2015) — relaxes correctness constraints
4. **"Stochastic Program Optimization"** (CACM 2016) — survey article
5. **"Stratified Synthesis: Automatically Learning the x86-64 Instruction Set"** (PLDI 2016) — learns instruction set semantics
6. **"Sound Loop Superoptimization for Google Native Client"** (ASPLOS 2017) — loop optimization for sandboxed code
7. **"A Complete Formal Semantics of x86-64 User-Level Instruction Set Architecture"** (PLDI 2019) — formal x86-64 semantics

### Limitations
- Only handles **loop-free** (straight-line) code
- No longer actively maintained (last commit ~6 years ago)
- Works at binary level, so limited to instruction subset supported by x86asm library

---

## 2. Souper — Synthesizing Superoptimizer for LLVM IR

**Paper**: "Souper: A Synthesizing Superoptimizer"  
**Authors**: Raimondas Sasnauskas, Yang Chen, Peter Collingbourne, Jeroen Ketema, Gratian Lup, Jubi Taneja, John Regehr  
**Venue**: PLDI 2017  
**Citations**: 89  
**arXiv**: 1711.04422  
**GitHub**: https://github.com/google/souper

### What It Does
- Superoptimizer for **LLVM IR** (not x86 directly)
- Uses **enumerative search** + **SMT solver (Z3)** to find missed peephole optimizations
- Extracts candidate optimizations from LLVM bitcode and validates them with an SMT solver
- Can be used as an automated LLVM optimization pass

### Optimizations Found in Practice
- **Shipped optimizations in both LLVM and Microsoft Visual C++** compilers (manually implemented by compiler teams)
- When used as automated pass: produces a **Clang binary ~3 MB (4.4%) smaller** than LLVM-compiled version
- Found missing peephole optimizations in:
  - Arithmetic strength reductions
  - Bitwise operation simplifications
  - Dead code elimination patterns
  - Redundant computation elimination
  - Conditional branch simplifications
- Optimizations are expressed as **LLVM IR rewrite rules** (source → target patterns)

### Key Techniques
- **Enumerative synthesis**: systematically generates candidate programs
- **SMT-based verification**: uses Z3 solver to verify equivalence
- **Souper IR**: custom intermediate representation similar to LLVM SSA
- **External caching** via Redis for large compilations
- Can be used as a drop-in compiler replacement (sclang/sclang++)

### Related Souper Papers
1. **"PrediPrune: Reducing Verification Overhead in Souper with Machine Learning Driven Pruning"** (2025, arXiv:2509.16497) — ML-guided pruning reduces compilation time by 51% vs baseline, 12% vs state-of-the-art Dataflow
2. **"Alive-Infer: Data-Driven Precondition Inference for Peephole Optimizations in LLVM"** (PLDI 2017) — generalizes 54 Souper optimization patterns with inferred preconditions

### Limitations
- Works at **LLVM IR level**, not directly on x86 assembly
- Enumeration is expensive for large instruction sequences
- SMT solver can be slow for complex equivalences

---

## 3. Minotaur — SIMD-Oriented Superoptimizer

**Paper**: "Minotaur: A SIMD-Oriented Synthesizing Superoptimizer"  
**Authors**: Zhengyang Liu, Stefan Mada, John Regehr  
**Venue**: PLDI 2024  
**Citations**: 18  
**arXiv**: 2306.00229

### What It Does
- Superoptimizer for **LLVM** focusing on **integer and floating-point SIMD code**
- Uses program synthesis to improve LLVM code generation
- Every optimization is **formally verified**

### Performance Results (MEASURABLE SPEEDUPS)
- **GMP benchmark suite** (GNU Multiple Precision):
  - Average speedup: **7.3%** over LLVM
  - Maximum speedup: **13%**
- **SPEC CPU 2017**:
  - Average speedup: **1.5%**
  - Maximum speedup: **4.5%** on 638.imagick

### Key Optimizations Found
- SIMD instruction selection improvements
- Floating-point SIMD optimizations missed by LLVM
- Several optimizations **implemented in LLVM upstream** as a result of this work

---

## 4. SuperCoder — LLM-Based Superoptimization

**Paper**: "SuperCoder: Assembly Program Superoptimization with Large Language Models"  
**Authors**: Anjiang Wei, Tarun Suresh, Huanmi Tan, Yinglun Xu, Gagandeep Singh, Ke Wang, Alex Aiken  
**Venue**: 2025 (arXiv: 2505.11480)  
**Citations**: 9

### What It Does
- Uses **LLMs as superoptimizers** for assembly programs
- First large-scale benchmark: **8,072 assembly programs** averaging 130 lines
- Uses **reinforcement learning** fine-tuning for optimization

### Performance Results
- **Claude-opus-4**: 51.5% test-passing rate, **1.43x average speedup** over gcc `-O3`
- **SuperCoder (fine-tuned)**: 95.0% correctness, **1.46x average speedup** over gcc `-O3`
- With Best-of-N sampling and iterative refinement: further improvements

### Key Insight
- LLMs can find optimizations that compilers miss, especially in:
  - Instruction scheduling
  - Register allocation patterns
  - Peephole optimizations in real-world code

---

## 5. SILO — Learning to Superoptimize Real-World Programs

**Paper**: "Learning to Superoptimize Real-world Programs"  
**Authors**: Alex Shypula, Pengcheng Yin, Jeremy Lacomis, Claire Le Goues, Edward Schwartz, Graham Neubig  
**Venue**: 2021 (arXiv: 2109.13498)  
**Citations**: 10

### What It Does
- Neural sequence-to-sequence models for **x86-64 assembly superoptimization**
- Dataset: **25,000+ real-world x86-64 assembly functions** mined from open-source projects
- Self Imitation Learning for Optimization (SILO)

### Results
- Superoptimizes **5.9%** of test set vs gcc 10.3 `-O3`
- 5x higher success rate than standard policy gradient approach
- First to demonstrate superoptimization on real-world assembly at scale

---

## 6. Equality Saturation & E-Graphs (egg)

**Paper**: "egg: Fast and Extensible Equality Saturation"  
**Authors**: Max Willsey, Chandrakana Nandi, Yisu Remy Wang, Oliver Flatt, Zachary Tatlock, Pavel Panchekha  
**Venue**: OOPSLA 2020  
**arXiv**: 2004.03082

### What It Does
- Open-source library for **equality saturation** (e-graphs)
- Represents many equivalent program forms simultaneously
- Enables rewrite-driven compiler optimizations without premature commitment

### Key Techniques
- **Rebuilding**: amortized invariant restoration with asymptotic speedups
- **E-class analyses**: integrates domain-specific analyses into e-graphs
- Used in many downstream superoptimizers and optimizers

### Relevance to x86-64
- Can express and explore many equivalent instruction sequences simultaneously
- Foundation for modern superoptimization tools
- Used in MLIR, TVM, and other compiler frameworks

---

## 7. Prism — Symbolic Superoptimization of Tensor Programs

**Paper**: "Prism: Symbolic Superoptimization of Tensor Programs"  
**Authors**: Mengdi Wu, Xiaoyu Jiang, Oded Padon, Zhihao Jia  
**Venue**: 2026 (arXiv: 2604.15272)

### Performance Results
- **2.2x speedup** over best superoptimizers
- **4.9x speedup** over best compiler-based approaches
- **3.4x reduction** in end-to-end optimization time

---

## 8. Alive — Verified Peephole Optimizations

**Paper**: "Provably Correct Peephole Optimizations with Alive"  
**Authors**: Juneyoung Lee, Nuno P. Lopes, Chung-Kil Hur, John Regehr, Nuno Lopes  
**Venue**: PLDI 2015  
**Citations**: 162

### What It Does
- Tool for **verifying LLVM peephole optimizations**
- Has helped find **dozens of bugs** in LLVM
- Expresses optimizations as LLVM IR rewrite rules with preconditions

### Related Papers
1. **"Alive-Infer"** (PLDI 2017, 24 citations) — data-driven precondition inference
2. **"Alive-FP"** (SAS 2016, 25 citations) — floating-point peephole verification
3. **"AliveInLean"** (CAV 2019, 8 citations) — verified implementation in Lean

---

## 9. Original Superoptimizer (Massalin, 1987)

**Paper**: "Superoptimizer — a Look at the Smallest Program"  
**Author**: Henry Massalin  
**Venue**: ASPLOS 1987

### Historical Significance
- First superoptimizer — brute-force enumeration of all programs
- Found optimal short sequences for small operations
- Limited to ~6 instructions due to exponential search space
- Motivated all subsequent work in the field

---

## Summary of What Superoptimizers Actually Find

### High-Value Optimizations for x86-64

1. **Arithmetic strength reductions**:
   - Multiply-by-constant → shifts + adds
   - Division by power-of-2 → shift right
   - Modulo operations → bitwise AND

2. **Bit manipulation tricks** (Hacker's Delight patterns):
   - Population count via multiply + magic constants
   - Bit counting, finding highest/lowest set bit
   - Bit reversal, byte swap via instruction sequences

3. **Instruction scheduling**:
   - Reordering independent instructions for ILP
   - Avoiding pipeline stalls
   - Leveraging micro-op fusion

4. **SIMD opportunities**:
   - Scalar → vectorization of small fixed-size loops
   - Intrinsic selection (SSE → AVX → AVX2)
   - SIMD instruction combining

5. **Dead code elimination**:
   - Unreachable branches after constant folding
   - Redundant loads/stores
   - Unused flag computations

6. **Conditional simplification**:
   - Branchless conditionals (CMOV, SETcc)
   - Predicated execution
   - Branch prediction hints

7. **Register allocation patterns**:
   - LEA for arithmetic (avoids flag side effects)
   - Stack → register moves
   - Zero-extension vs sign-extension choices

---

## Performance Results Summary

| Tool | Benchmark | Speedup |
|------|-----------|---------|
| STOKE | vs gcc -O3 | Matches or beats |
| STOKE | vs hand-written assembly | Sometimes beats |
| Souper | LLVM binary size | 4.4% smaller |
| Minotaur | GMP benchmark | 7.3% avg, 13% max |
| Minotaur | SPEC CPU 2017 | 1.5% avg, 4.5% max |
| SuperCoder | vs gcc -O3 | 1.46x avg |
| SILO | vs gcc -O3 | 5.9% of functions |
| Prism | vs best superoptimizers | 2.2x |

---

## Practical Takeaways for a Custom x86-64 Compiler

### What Actually Gives Measurable Speedups

1. **Peephole optimizations** are the lowest-hanging fruit — they're well-understood and give 5-15% improvements on specific code patterns
2. **SIMD instruction selection** gives the biggest single-function speedups (2-5x for compute-bound code)
3. **Instruction scheduling** for modern out-of-order CPUs (Haswell, Zen, Skylake) — 3-10% for latency-bound code
4. **Strength reductions** (multiply → shift/add, div → shift) — 10-30% for affected operations

### Recommended Approach for Your Compiler

1. **Start with a peephole optimizer** (like Alive/Souper approach) — highest ROI
2. **Add e-graph equality saturation** (egg) — enables discovering non-obvious equivalences
3. **Target specific instruction sequences** — focus on hot loops first
4. **Use STOKE-style MCMC** for small, critical code sequences (function inlining boundaries)
5. **Consider LLM-assisted optimization** (SuperCoder approach) for discovering novel patterns

### Key Resources
- STOKE GitHub: https://github.com/StanfordPL/stoke
- Souper GitHub: https://github.com/google/souper
- egg library: https://github.com/egraphs-good/egg
- Alive tool: https://alive2.llvm.org/
- Minotaur: https://github.com/zhengyangl/minotaur
