// macro_fusion.c — Implementation of macro-fusion decision tables
// Source: Agner Fog "The microarchitecture of Intel, AMD and VIA CPUs" (2024)

#include "macro_fusion.h"

// ============================================================================
// FUSION PAIR TABLE
// 1 = can fuse, 0 = cannot fuse
// Indexed as: can_fuse[arch][alu_op][cc_group]
//
// cc_group 0 = ZF-based (JE/JNE)
// cc_group 1 = CF-based (JB/JAE/JA/JBE)
// cc_group 2 = SF/OF-based (JL/JGE/JG/JLE)
// cc_group 3 = OF/SF/PF-only (JO/JNO/JS/JNS/JP/JNP)
//
// This grouping captures the key constraint: on Intel, CMP/ADD/SUB/INC/DEC
// cannot fuse with cc_group 3 because those branches test flags that CMP
// doesn't always produce, or that ADD/SUB/INC/DEC may clobber differently.
// ============================================================================

#define ARCH_COUNT FUSION_ARCH_COUNT

// cc_groups: 0=ZF, 1=CF, 2=SF/OF, 3=OF/SF/PF-only
#define CCGRP_COUNT 4

static const uint8_t fusion_table[ARCH_COUNT][FUSION_OP_COUNT][CCGRP_COUNT] = {
    // ---- Skylake (also Kaby, Coffee, Cascade Lake) ----
    [FUSION_SKYLAKE] = {
        [FUSION_OP_CMP]   = {1, 1, 1, 0},  // ZF/CF/SF-OF OK, OF/SF/PF NOT
        [FUSION_OP_TEST]  = {1, 1, 1, 1},  // ALL branches
        [FUSION_OP_ADD]   = {1, 1, 1, 0},
        [FUSION_OP_SUB]   = {1, 1, 1, 0},
        [FUSION_OP_INC]   = {1, 0, 1, 0},  // ZF and SF/OF only (no CF branches)
        [FUSION_OP_DEC]   = {1, 0, 1, 0},
        [FUSION_OP_AND]   = {1, 1, 1, 1},  // ALL branches
        [FUSION_OP_OR]    = {0, 0, 0, 0},
        [FUSION_OP_XOR]   = {0, 0, 0, 0},
        [FUSION_OP_ADC]   = {0, 0, 0, 0},
        [FUSION_OP_SBB]   = {0, 0, 0, 0},
        [FUSION_OP_NEG]   = {0, 0, 0, 0},
        [FUSION_OP_NOT]   = {0, 0, 0, 0},
        [FUSION_OP_SHIFT] = {0, 0, 0, 0},
        [FUSION_OP_MOV]   = {0, 0, 0, 0},
        [FUSION_OP_NOP]   = {0, 0, 0, 0},
    },
    // ---- Ice Lake / Tiger Lake / Rocket Lake ----
    [FUSION_ICE_LAKE] = {
        [FUSION_OP_CMP]   = {1, 1, 1, 0},
        [FUSION_OP_TEST]  = {1, 1, 1, 1},
        [FUSION_OP_ADD]   = {1, 1, 1, 0},
        [FUSION_OP_SUB]   = {1, 1, 1, 0},
        [FUSION_OP_INC]   = {1, 0, 1, 0},
        [FUSION_OP_DEC]   = {1, 0, 1, 0},
        [FUSION_OP_AND]   = {1, 1, 1, 1},
        [FUSION_OP_OR]    = {0, 0, 0, 0},
        [FUSION_OP_XOR]   = {0, 0, 0, 0},
        [FUSION_OP_ADC]   = {0, 0, 0, 0},
        [FUSION_OP_SBB]   = {0, 0, 0, 0},
        [FUSION_OP_NEG]   = {0, 0, 0, 0},
        [FUSION_OP_NOT]   = {0, 0, 0, 0},
        [FUSION_OP_SHIFT] = {0, 0, 0, 0},
        [FUSION_OP_MOV]   = {0, 0, 0, 0},
        [FUSION_OP_NOP]   = {0, 0, 0, 0},
    },
    // ---- Alder Lake P-core (Golden Cove) ----
    // Same branch conditions as Skylake, but memory operands re-enabled
    [FUSION_ALDER_LAKE_P] = {
        [FUSION_OP_CMP]   = {1, 1, 1, 0},
        [FUSION_OP_TEST]  = {1, 1, 1, 1},
        [FUSION_OP_ADD]   = {1, 1, 1, 0},
        [FUSION_OP_SUB]   = {1, 1, 1, 0},
        [FUSION_OP_INC]   = {1, 0, 1, 0},
        [FUSION_OP_DEC]   = {1, 0, 1, 0},
        [FUSION_OP_AND]   = {1, 1, 1, 1},
        [FUSION_OP_OR]    = {0, 0, 0, 0},
        [FUSION_OP_XOR]   = {0, 0, 0, 0},
        [FUSION_OP_ADC]   = {0, 0, 0, 0},
        [FUSION_OP_SBB]   = {0, 0, 0, 0},
        [FUSION_OP_NEG]   = {0, 0, 0, 0},
        [FUSION_OP_NOT]   = {0, 0, 0, 0},
        [FUSION_OP_SHIFT] = {0, 0, 0, 0},
        [FUSION_OP_MOV]   = {0, 0, 0, 0},
        [FUSION_OP_NOP]   = {0, 0, 0, 0},
    },
    // ---- AMD Zen 1 / Zen+ ----
    [FUSION_ZEN1] = {
        [FUSION_OP_CMP]   = {1, 1, 1, 1},  // ALL branches (no restrictions)
        [FUSION_OP_TEST]  = {1, 1, 1, 1},
        [FUSION_OP_ADD]   = {0, 0, 0, 0},
        [FUSION_OP_SUB]   = {0, 0, 0, 0},
        [FUSION_OP_INC]   = {0, 0, 0, 0},
        [FUSION_OP_DEC]   = {0, 0, 0, 0},
        [FUSION_OP_AND]   = {0, 0, 0, 0},
        [FUSION_OP_OR]    = {0, 0, 0, 0},
        [FUSION_OP_XOR]   = {0, 0, 0, 0},
        [FUSION_OP_ADC]   = {0, 0, 0, 0},
        [FUSION_OP_SBB]   = {0, 0, 0, 0},
        [FUSION_OP_NEG]   = {0, 0, 0, 0},
        [FUSION_OP_NOT]   = {0, 0, 0, 0},
        [FUSION_OP_SHIFT] = {0, 0, 0, 0},
        [FUSION_OP_MOV]   = {0, 0, 0, 0},
        [FUSION_OP_NOP]   = {0, 0, 0, 0},
    },
    // ---- AMD Zen 2 ----
    [FUSION_ZEN2] = {
        [FUSION_OP_CMP]   = {1, 1, 1, 1},
        [FUSION_OP_TEST]  = {1, 1, 1, 1},
        [FUSION_OP_ADD]   = {0, 0, 0, 0},
        [FUSION_OP_SUB]   = {0, 0, 0, 0},
        [FUSION_OP_INC]   = {0, 0, 0, 0},
        [FUSION_OP_DEC]   = {0, 0, 0, 0},
        [FUSION_OP_AND]   = {0, 0, 0, 0},
        [FUSION_OP_OR]    = {0, 0, 0, 0},
        [FUSION_OP_XOR]   = {0, 0, 0, 0},
        [FUSION_OP_ADC]   = {0, 0, 0, 0},
        [FUSION_OP_SBB]   = {0, 0, 0, 0},
        [FUSION_OP_NEG]   = {0, 0, 0, 0},
        [FUSION_OP_NOT]   = {0, 0, 0, 0},
        [FUSION_OP_SHIFT] = {0, 0, 0, 0},
        [FUSION_OP_MOV]   = {0, 0, 0, 0},
        [FUSION_OP_NOP]   = {0, 0, 0, 0},
    },
    // ---- AMD Zen 3 ----
    [FUSION_ZEN3] = {
        [FUSION_OP_CMP]   = {1, 1, 1, 1},
        [FUSION_OP_TEST]  = {1, 1, 1, 1},
        [FUSION_OP_ADD]   = {1, 1, 1, 1},  // ALL branches, ALL ALU ops
        [FUSION_OP_SUB]   = {1, 1, 1, 1},
        [FUSION_OP_INC]   = {1, 1, 1, 1},
        [FUSION_OP_DEC]   = {1, 1, 1, 1},
        [FUSION_OP_AND]   = {1, 1, 1, 1},
        [FUSION_OP_OR]    = {1, 1, 1, 1},
        [FUSION_OP_XOR]   = {1, 1, 1, 1},
        [FUSION_OP_ADC]   = {0, 0, 0, 0},
        [FUSION_OP_SBB]   = {0, 0, 0, 0},
        [FUSION_OP_NEG]   = {0, 0, 0, 0},
        [FUSION_OP_NOT]   = {0, 0, 0, 0},
        [FUSION_OP_SHIFT] = {0, 0, 0, 0},
        [FUSION_OP_MOV]   = {0, 0, 0, 0},
        [FUSION_OP_NOP]   = {0, 0, 0, 0},
    },
    // ---- AMD Zen 4 ----
    [FUSION_ZEN4] = {
        [FUSION_OP_CMP]   = {1, 1, 1, 1},
        [FUSION_OP_TEST]  = {1, 1, 1, 1},
        [FUSION_OP_ADD]   = {1, 1, 1, 1},
        [FUSION_OP_SUB]   = {1, 1, 1, 1},
        [FUSION_OP_INC]   = {1, 1, 1, 1},
        [FUSION_OP_DEC]   = {1, 1, 1, 1},
        [FUSION_OP_AND]   = {1, 1, 1, 1},
        [FUSION_OP_OR]    = {1, 1, 1, 1},
        [FUSION_OP_XOR]   = {1, 1, 1, 1},
        [FUSION_OP_ADC]   = {0, 0, 0, 0},
        [FUSION_OP_SBB]   = {0, 0, 0, 0},
        [FUSION_OP_NEG]   = {0, 0, 0, 0},
        [FUSION_OP_NOT]   = {0, 0, 0, 0},
        [FUSION_OP_SHIFT] = {0, 0, 0, 0},
        [FUSION_OP_MOV]   = {0, 0, 0, 0},
        [FUSION_OP_NOP]   = {1, 1, 1, 1},  // Zen 4 can fuse NOP with preceding instruction
    },
    // ---- AMD Zen 5 ----
    [FUSION_ZEN5] = {
        [FUSION_OP_CMP]   = {1, 1, 1, 1},
        [FUSION_OP_TEST]  = {1, 1, 1, 1},
        [FUSION_OP_ADD]   = {1, 1, 1, 1},
        [FUSION_OP_SUB]   = {1, 1, 1, 1},
        [FUSION_OP_INC]   = {1, 1, 1, 1},
        [FUSION_OP_DEC]   = {1, 1, 1, 1},
        [FUSION_OP_AND]   = {1, 1, 1, 1},
        [FUSION_OP_OR]    = {1, 1, 1, 1},
        [FUSION_OP_XOR]   = {1, 1, 1, 1},
        [FUSION_OP_ADC]   = {0, 0, 0, 0},
        [FUSION_OP_SBB]   = {0, 0, 0, 0},
        [FUSION_OP_NEG]   = {0, 0, 0, 0},
        [FUSION_OP_NOT]   = {0, 0, 0, 0},
        [FUSION_OP_SHIFT] = {0, 0, 0, 0},
        [FUSION_OP_MOV]   = {0, 0, 0, 0},
        [FUSION_OP_NOP]   = {0, 0, 0, 0},  // Zen 5 cannot fuse NOPs
    },
};

// Map each individual branch condition to its cc_group
static int cc_to_group(fusion_cc_t cc) {
    switch (cc) {
        case FUSION_CC_E:  case FUSION_CC_NE: return 0;  // ZF
        case FUSION_CC_B:  case FUSION_CC_AE:
        case FUSION_CC_A:  case FUSION_CC_BE: return 1;  // CF
        case FUSION_CC_L:  case FUSION_CC_GE:
        case FUSION_CC_G:  case FUSION_CC_LE: return 2;  // SF/OF
        case FUSION_CC_O:  case FUSION_CC_NO:
        case FUSION_CC_S:  case FUSION_CC_NS:
        case FUSION_CC_P:  case FUSION_CC_NP: return 3;  // OF/SF/PF only
        default: return -1;
    }
}

int fusion_can_pair(fusion_arch_t arch, fusion_alu_op_t alu, fusion_cc_t cc) {
    if (arch >= ARCH_COUNT || alu >= FUSION_OP_COUNT || cc >= FUSION_CC_COUNT)
        return 0;
    int grp = cc_to_group(cc);
    if (grp < 0) return 0;
    return fusion_table[arch][alu][grp];
}

int fusion_valid_operand(fusion_arch_t arch, fusion_alu_op_t alu, fusion_operand_t operand) {
    (void)alu; // All ALU ops have the same operand restrictions

    switch (arch) {
        case FUSION_SKYLAKE:
        case FUSION_ALDER_LAKE_P:
            // Skylake: reg+reg, reg+imm, reg+[mem] OK
            // NOT: [mem]+imm, RIP-relative, memory dest
            switch (operand) {
                case FUSION_OPERAND_REG:     return 1;
                case FUSION_OPERAND_IMM:     return 1;
                case FUSION_OPERAND_MEM:     return 1;
                case FUSION_OPERAND_MEM_IMM: return 0;
                case FUSION_OPERAND_RIP_REL: return 0;
            }
            break;

        case FUSION_ICE_LAKE:
            // Ice Lake: memory operands NOT allowed at all
            switch (operand) {
                case FUSION_OPERAND_REG:     return 1;
                case FUSION_OPERAND_IMM:     return 1;
                case FUSION_OPERAND_MEM:     return 0;
                case FUSION_OPERAND_MEM_IMM: return 0;
                case FUSION_OPERAND_RIP_REL: return 0;
            }
            break;

        case FUSION_ZEN1:
        case FUSION_ZEN2:
        case FUSION_ZEN3:
        case FUSION_ZEN4:
        case FUSION_ZEN5:
            // AMD: reg+reg, reg+imm, reg+[mem] OK
            // NOT: [mem]+imm, RIP-relative
            switch (operand) {
                case FUSION_OPERAND_REG:     return 1;
                case FUSION_OPERAND_IMM:     return 1;
                case FUSION_OPERAND_MEM:     return 1;
                case FUSION_OPERAND_MEM_IMM: return 0;
                case FUSION_OPERAND_RIP_REL: return 0;
            }
            break;

        default:
            break;
    }
    return 0;
}

int fusion_max_pairs_per_cycle(fusion_arch_t arch) {
    switch (arch) {
        case FUSION_SKYLAKE:
        case FUSION_ICE_LAKE:
        case FUSION_ALDER_LAKE_P:
            return 2;  // Intel: up to 2 fused pairs per decode cycle
        default:
            return 1;  // AMD: only 1 fused pair per decode cycle
    }
}

int fusion_throughput(fusion_arch_t arch, fusion_pred_t pred) {
    switch (arch) {
        case FUSION_SKYLAKE:
        case FUSION_ICE_LAKE:
        case FUSION_ALDER_LAKE_P:
            return pred == FUSION_NT ? 2 : 1;  // 2/cycle NT, 1/cycle T
        case FUSION_ZEN1:
        case FUSION_ZEN2:
            return pred == FUSION_NT ? 2 : 1;  // 2/cycle NT, 1/2 cycle T
        case FUSION_ZEN3:
        case FUSION_ZEN4:
            return pred == FUSION_NT ? 2 : 1;  // 2/cycle NT, 1/cycle T
        case FUSION_ZEN5:
            return pred == FUSION_NT ? 3 : 2;  // 3/cycle NT, 2/cycle T
        default:
            return 0;
    }
}

int fusion_is_branch_fuseable(uint8_t opcode) {
    // JECXZ = 0xE3, LOOP = 0xE2, LOOPE = 0xE1, LOOPNE = 0xE0
    // These are NEVER fuseable on any architecture
    if (opcode == 0xE3 || opcode == 0xE2 || opcode == 0xE1 || opcode == 0xE0)
        return 0;
    // All other 0x7x and 0x0F 0x8x conditional jumps are fuseable
    return 1;
}
