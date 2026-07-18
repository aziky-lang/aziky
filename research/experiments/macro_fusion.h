// macro_fusion.h — Machine-readable macro-fusion rules for x86-64 compiler
// Source: Agner Fog "The microarchitecture of Intel, AMD and VIA CPUs" (2024)
// All information verified against uops.info measurements.

#ifndef MACRO_FUSION_H
#define MACRO_FUSION_H

#include <stdint.h>

// Architecture enumeration
typedef enum {
    FUSION_SKYLAKE,        // Intel Skylake (also Kaby/Coffee/Cascade Lake)
    FUSION_ICE_LAKE,       // Intel Ice Lake (also Tiger Lake, Rocket Lake)
    FUSION_ALDER_LAKE_P,   // Intel Alder Lake P-core (Golden Cove)
    FUSION_ZEN1,           // AMD Zen 1 / Zen+
    FUSION_ZEN2,           // AMD Zen 2
    FUSION_ZEN3,           // AMD Zen 3
    FUSION_ZEN4,           // AMD Zen 4
    FUSION_ZEN5,           // AMD Zen 5
    FUSION_ARCH_COUNT
} fusion_arch_t;

// ALU instruction opcodes (the first instruction in a potential pair)
typedef enum {
    FUSION_OP_CMP,
    FUSION_OP_TEST,
    FUSION_OP_ADD,
    FUSION_OP_SUB,
    FUSION_OP_INC,
    FUSION_OP_DEC,
    FUSION_OP_AND,
    FUSION_OP_OR,
    FUSION_OP_XOR,
    FUSION_OP_ADC,
    FUSION_OP_SBB,
    FUSION_OP_NEG,
    FUSION_OP_NOT,
    FUSION_OP_SHIFT,   // SHL, SHR, SAR, ROL, ROR, etc.
    FUSION_OP_MOV,     // MOV never fuses
    FUSION_OP_NOP,     // NOP (Zen 4 only)
    FUSION_OP_COUNT
} fusion_alu_op_t;

// Branch condition types
typedef enum {
    FUSION_CC_E,    // JE/JZ     (ZF=1)
    FUSION_CC_NE,   // JNE/JNZ   (ZF=0)
    FUSION_CC_B,    // JB/JC     (CF=1)
    FUSION_CC_AE,   // JAE/JNB   (CF=0)
    FUSION_CC_A,    // JA        (CF=0 & ZF=0)
    FUSION_CC_BE,   // JBE       (CF=1 | ZF=1)
    FUSION_CC_L,    // JL        (SF!=OF)
    FUSION_CC_GE,   // JGE       (SF=OF)
    FUSION_CC_G,    // JG        (ZF=0 & SF=OF)
    FUSION_CC_LE,   // JLE       (ZF=1 | SF!=OF)
    FUSION_CC_O,    // JO        (OF=1)
    FUSION_CC_NO,   // JNO       (OF=0)
    FUSION_CC_S,    // JS        (SF=1)
    FUSION_CC_NS,   // JNS       (SF=0)
    FUSION_CC_P,    // JP        (PF=1)
    FUSION_CC_NP,   // JNP       (PF=0)
    FUSION_CC_COUNT
} fusion_cc_t;

// Branch taken/not-taken prediction hint
typedef enum {
    FUSION_NT,   // Not taken
    FUSION_T,    // Taken
} fusion_pred_t;

// Operand type for the ALU instruction's second operand
typedef enum {
    FUSION_OPERAND_REG,          // register-register
    FUSION_OPERAND_IMM,          // register-immediate
    FUSION_OPERAND_MEM,          // register-memory (or memory-register)
    FUSION_OPERAND_MEM_IMM,      // memory with both displacement and immediate
    FUSION_OPERAND_RIP_REL,      // RIP-relative memory addressing
} fusion_operand_t;

// Check if an ALU op + branch condition can macro-fuse on a given architecture.
// Returns 1 if the pair can fuse, 0 otherwise.
int fusion_can_pair(fusion_arch_t arch, fusion_alu_op_t alu, fusion_cc_t cc);

// Check if an ALU op with a given operand type can participate in fusion.
// Returns 1 if the operand encoding is allowed, 0 otherwise.
int fusion_valid_operand(fusion_arch_t arch, fusion_alu_op_t alu, fusion_operand_t operand);

// Get the maximum number of fused pairs that can be decoded per cycle.
int fusion_max_pairs_per_cycle(fusion_arch_t arch);

// Get the throughput of fused branches (taken vs not-taken).
// Returns max fused branches per cycle.
int fusion_throughput(fusion_arch_t arch, fusion_pred_t pred);

// Check if JECXZ or LOOP can fuse (always returns 0).
int fusion_is_branch_fuseable(uint8_t opcode);

#endif // MACRO_FUSION_H
