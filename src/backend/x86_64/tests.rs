use super::*;
use crate::frontend::semantics::{
    RuntimeBinOp, RuntimeCmpOp, RuntimeFloatBinOp, RuntimeInstr, RuntimeOperand, RuntimeProgram,
};

#[test]
fn exit_only_contains_sys_exit() {
    let code = emit_linux_program(ProgramKind::ExitOnly);
    assert!(code.windows(2).any(|w| w == [0x0F, 0x05]));
}

#[test]
fn write_and_exit_embeds_message() {
    let msg = b"hello\n";
    let code = emit_linux_program(ProgramKind::WriteAndExit { message: msg });
    assert!(code.ends_with(msg));
}

#[test]
fn builder_patches_multiple_messages() {
    let mut program = X86Program::new();
    program.emit_write(b"a");
    program.emit_write(b"b");
    program.emit_exit(0);
    let code = program.finalize();
    assert!(code.ends_with(b"ab"));
}

#[test]
fn builder_interns_duplicate_messages() {
    let mut program = X86Program::new();
    program.emit_write(b"dup\n");
    program.emit_write(b"dup\n");
    program.emit_exit(0);
    let code = program.finalize();
    let count = code.windows(4).filter(|w| *w == b"dup\n").count();
    assert_eq!(count, 1);
}

#[test]
fn affine_pow_64_steps_matches_scalar_recurrence() {
    let mul = 1_664_525u64;
    let add = 1_013_904_223u64;
    let (chunk_mul, chunk_add) = X86Program::affine_pow_u64(mul, add, 64);
    for initial in [0, 1, 123_456_789, u64::MAX, 0xDEAD_BEEF_CAFE_BABE] {
        let mut scalar = initial;
        for _ in 0..64 {
            scalar = scalar.wrapping_mul(mul).wrapping_add(add);
        }
        assert_eq!(
            initial.wrapping_mul(chunk_mul).wrapping_add(chunk_add),
            scalar
        );
    }
}

#[test]
fn affine_composition_policy_is_bounded_and_tail_exact() {
    assert_eq!(X86Program::affine_composition_chunk(8_191), 1);
    assert_eq!(X86Program::affine_composition_chunk(8_192), 32);
    assert_eq!(X86Program::affine_composition_chunk(32_768), 64);
    assert_eq!(X86Program::affine_composition_chunk(131_072), 128);

    let mul = 1_664_525_u64;
    let add = 1_013_904_223_u64;
    for iterations in [1_u64, 31, 8_193, 32_771, 131_077] {
        let chunk = X86Program::affine_composition_chunk(iterations);
        let (chunk_mul, chunk_add) = X86Program::affine_pow_u64(mul, add, chunk);
        let mut transformed = 123_456_789_u64;
        for _ in 0..iterations / chunk {
            transformed = transformed.wrapping_mul(chunk_mul).wrapping_add(chunk_add);
        }
        for _ in 0..iterations % chunk {
            transformed = transformed.wrapping_mul(mul).wrapping_add(add);
        }
        let mut scalar = 123_456_789_u64;
        for _ in 0..iterations {
            scalar = scalar.wrapping_mul(mul).wrapping_add(add);
        }
        assert_eq!(transformed, scalar, "iterations={iterations}");
    }
}

#[test]
fn reversible_u32_affine_pair_recovers_every_demanded_predecessor_bit() {
    const MUL: u32 = 1_664_525;
    const ADD: u32 = 1_013_904_223;
    const INVERSE_MOD_1024: u32 = 197;
    let mul2 = MUL.wrapping_mul(MUL);
    let add2 = ADD.wrapping_mul(MUL.wrapping_add(1));

    for initial in [
        0_u32,
        1,
        123_456_789,
        u16::MAX as u32,
        u32::MAX - 1,
        u32::MAX,
    ] {
        let hi = initial.wrapping_mul(MUL).wrapping_add(ADD);
        let lo = hi.wrapping_mul(MUL).wrapping_add(ADD);
        let composed = initial.wrapping_mul(mul2).wrapping_add(add2);
        let recovered = lo.wrapping_sub(ADD).wrapping_mul(INVERSE_MOD_1024);
        assert_eq!(composed, lo);
        assert_eq!(recovered & 0x3ff, hi & 0x3ff);
        assert_eq!((recovered >> 4) & 63, (hi >> 4) & 63);
    }
}

fn contains_jne(code: &[u8]) -> bool {
    code.windows(2).any(|w| w == [0x0F, 0x85] || w[0] == 0x75)
}

#[test]
fn runtime_generic_program_emits_control_flow() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::Cmp {
                dst: 1,
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(10),
            },
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(10),
                target: 5,
            },
            RuntimeInstr::BinOpInPlace {
                dst: 0,
                op: RuntimeBinOp::Add,
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::Jump { target: 1 },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.windows(2).any(|w| w == [0x0F, 0x83])); // jae rel32 (false for <)
    assert!(code.windows(2).any(|w| w == [0x0F, 0x05])); // syscall
}

#[test]
fn runtime_generic_relaxes_small_backward_loop_jump_to_rel8() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 1,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(3),
            },
            RuntimeInstr::BinOpInPlace {
                dst: 0,
                op: RuntimeBinOp::Sub,
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::JumpIfZero {
                cond_slot: 0,
                target: 4,
            },
            RuntimeInstr::Jump { target: 1 },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(
        code.windows(2).any(|window| window[0] == 0xEB),
        "small backward loop should use a signed rel8 jump"
    );
}

#[test]
fn runtime_generic_allocations_share_one_slab_allocator_and_terminal_teardown() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 3,
        instrs: vec![
            RuntimeInstr::Alloc {
                dst: 0,
                size: RuntimeOperand::Imm(256),
            },
            RuntimeInstr::Alloc {
                dst: 1,
                size: RuntimeOperand::Imm(512),
            },
            RuntimeInstr::PrintInt {
                value: RuntimeOperand::Slot(0),
                signed: false,
                bits: 64,
            },
            RuntimeInstr::PrintInt {
                value: RuntimeOperand::Slot(1),
                signed: false,
                bits: 64,
            },
            RuntimeInstr::Free {
                ptr: RuntimeOperand::Slot(0),
                size: RuntimeOperand::Imm(256),
            },
            RuntimeInstr::Free {
                ptr: RuntimeOperand::Slot(1),
                size: RuntimeOperand::Imm(512),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert_eq!(
        code.windows(5)
            .filter(|w| *w == [0xB8, 0x09, 0x00, 0x00, 0x00])
            .count(),
        1,
        "all allocations must share one emitted mmap-backed slab routine"
    );
    assert_eq!(
        code.windows(5)
            .filter(|w| *w == [0xB8, 0x0B, 0x00, 0x00, 0x00])
            .count(),
        1,
        "all exits must share one emitted slab-chain teardown routine"
    );
    assert!(
        code.windows(7)
            .any(|window| window == [0x49, 0x81, 0xE0, 0xF0, 0xFF, 0xFF, 0xFF]),
        "shared allocator must round payload storage to 16-byte alignment"
    );
}

#[test]
fn runtime_generic_heap_integer_access_uses_requested_element_width() {
    let mut x = X86Program::new();
    let mut instrs = vec![RuntimeInstr::Mov {
        dst: 0,
        src: RuntimeOperand::Imm(0),
    }];
    for bytes in [1u8, 2, 4, 8] {
        instrs.push(RuntimeInstr::HeapStoreInt {
            ptr: RuntimeOperand::Slot(0),
            index: RuntimeOperand::Imm(3),
            src: RuntimeOperand::Imm(7),
            bytes,
        });
        instrs.push(RuntimeInstr::HeapLoadInt {
            dst: 1,
            ptr: RuntimeOperand::Slot(0),
            index: RuntimeOperand::Imm(3),
            bytes,
        });
    }
    instrs.push(RuntimeInstr::Exit {
        code: RuntimeOperand::Imm(0),
    });
    x.emit_runtime_generic_program(&RuntimeProgram { slots: 2, instrs });
    let code = x.finalize();

    for opcode in [
        &[0x43, 0x88, 0x04, 0x1A][..],
        &[0x66, 0x43, 0x89, 0x04, 0x5A],
        &[0x43, 0x89, 0x04, 0x9A],
        &[0x4B, 0x89, 0x04, 0xDA],
        &[0x43, 0x0F, 0xB6, 0x04, 0x1A],
        &[0x43, 0x0F, 0xB7, 0x04, 0x5A],
        &[0x43, 0x8B, 0x04, 0x9A],
        &[0x4B, 0x8B, 0x04, 0xDA],
    ] {
        assert!(
            code.windows(opcode.len()).any(|window| window == opcode),
            "missing packed heap access opcode {opcode:02x?}"
        );
    }
}

#[test]
fn runtime_generic_promotes_noescape_allocation_to_shared_stack_frame() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 1,
        instrs: vec![
            RuntimeInstr::Alloc {
                dst: 0,
                size: RuntimeOperand::Imm(256),
            },
            RuntimeInstr::Free {
                ptr: RuntimeOperand::Slot(0),
                size: RuntimeOperand::Imm(256),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(
        code.windows(3).any(|window| window == [0x48, 0x8D, 0x85]),
        "stack promotion should materialize an rbp-relative address"
    );
    assert!(
        !code
            .windows(5)
            .any(|window| window == [0xB8, 0x09, 0x00, 0x00, 0x00])
    );
    assert!(
        !code
            .windows(5)
            .any(|window| window == [0xB8, 0x0B, 0x00, 0x00, 0x00])
    );
}

#[test]
fn segmented_allocator_coalesces_dead_source_copy() {
    let program = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(7),
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Slot(0),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(1),
            },
        ],
    };
    let lir = MachineLIRProgram::lower(&program, None).expect("valid lir");
    let slots = RuntimeSlotMap::from_lir(&lir);
    assert_eq!(slots.reg(0), slots.reg(1));
    let repeated = RuntimeSlotMap::from_lir(&lir);
    assert_eq!(slots.reg_by_slot, repeated.reg_by_slot);
    assert_eq!(slots.stack_index_by_slot, repeated.stack_index_by_slot);
}

#[test]
fn stack_slot_coloring_reuses_disjoint_spill_storage() {
    let mut instrs = Vec::new();
    for slot in 0..12 {
        instrs.push(RuntimeInstr::Mov {
            dst: slot,
            src: RuntimeOperand::Imm(slot as u64),
        });
    }
    for slot in 0..12 {
        instrs.push(RuntimeInstr::PrintInt {
            value: RuntimeOperand::Slot(slot),
            signed: false,
            bits: 64,
        });
    }
    for slot in 12..24 {
        instrs.push(RuntimeInstr::Mov {
            dst: slot,
            src: RuntimeOperand::Imm(slot as u64),
        });
    }
    for slot in 12..24 {
        instrs.push(RuntimeInstr::PrintInt {
            value: RuntimeOperand::Slot(slot),
            signed: false,
            bits: 64,
        });
    }
    instrs.push(RuntimeInstr::Exit {
        code: RuntimeOperand::Imm(0),
    });
    let program = RuntimeProgram { slots: 24, instrs };
    let lir = MachineLIRProgram::lower(&program, None).expect("valid lir");
    let slots = RuntimeSlotMap::from_lir(&lir);
    assert_eq!(slots.stack_slots(), 1);
}

#[test]
fn runtime_generic_elides_frame_when_all_values_fit_registers() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 1,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(7),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert_ne!(code.first().copied(), Some(0x55));
}

#[test]
fn runtime_generic_program_emits_call_and_return() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 1,
        instrs: vec![
            RuntimeInstr::Call { target: 2 },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(0),
            },
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(7),
            },
            RuntimeInstr::Return,
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.contains(&0xE8)); // call rel32
    assert!(code.contains(&0xC3)); // ret
    assert!(code.windows(2).any(|w| w == [0x0F, 0x05])); // syscall
}

#[test]
fn runtime_generic_tail_call_elides_call_instruction() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 1,
        instrs: vec![
            RuntimeInstr::Call { target: 3 },
            RuntimeInstr::Return,
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(0),
            },
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(7),
            },
            RuntimeInstr::Return,
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.contains(&0xE9)); // jmp rel32 (tail call)
    assert!(!code.contains(&0xE8)); // no call rel32 in this program
    assert!(code.contains(&0xC3)); // callee return remains
}

#[test]
fn runtime_generic_float_binop_emits_sse_scalar_opcodes() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(1.25f64.to_bits()),
            },
            RuntimeInstr::FloatBinOp {
                dst: 0,
                bits: 64,
                op: RuntimeFloatBinOp::Add,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(2.0f64.to_bits()),
            },
            RuntimeInstr::FloatBinOp {
                dst: 1,
                bits: 32,
                op: RuntimeFloatBinOp::Mul,
                lhs: RuntimeOperand::Imm(u64::from((3.0f32).to_bits())),
                rhs: RuntimeOperand::Imm(u64::from((4.0f32).to_bits())),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.windows(3).any(|w| w == [0xF2, 0x0F, 0x58])); // addsd
    assert!(code.windows(3).any(|w| w == [0xF3, 0x0F, 0x59])); // mulss
    assert!(code.windows(2).any(|w| w == [0x0F, 0x05])); // syscall
}

#[test]
fn runtime_generic_fuses_cmp_then_jumpifzero() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::Cmp {
                dst: 1,
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(10),
            },
            RuntimeInstr::JumpIfZero {
                cond_slot: 1,
                target: 4,
            },
            RuntimeInstr::BinOpInPlace {
                dst: 0,
                op: RuntimeBinOp::Add,
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.windows(2).any(|w| w == [0x0F, 0x83])); // jae rel32 (false for <)
    assert!(!code.windows(3).any(|w| w == [0x0F, 0x92, 0xC0])); // setb al removed by fusion
}

#[test]
fn runtime_generic_compare_swap_unsigned_emits_cmova() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(9),
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Imm(3),
            },
            RuntimeInstr::CompareSwap {
                left: 0,
                right: 1,
                signed: false,
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.windows(2).any(|w| w == [0x0F, 0x47])); // cmova r64, r/m64
}

#[test]
fn runtime_generic_compare_swap_signed_emits_cmovg() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(u64::MAX - 2), // -3
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Imm(7),
            },
            RuntimeInstr::CompareSwap {
                left: 0,
                right: 1,
                signed: true,
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.windows(2).any(|w| w == [0x0F, 0x4F])); // cmovg r64, r/m64
}

#[test]
fn runtime_generic_radix_sort_kernel_emits_cpu_dispatch_and_xgetbv() {
    let mut x = X86Program::new();
    let mut slots = Vec::new();
    for i in 0..32usize {
        slots.push(i);
    }
    let program = RuntimeProgram {
        slots: 32,
        instrs: vec![
            RuntimeInstr::RadixSortFixedInt {
                slots,
                bits: 64,
                signed: false,
                stable: false,
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.windows(2).any(|w| w == [0x0F, 0xA2])); // cpuid
    assert!(code.windows(3).any(|w| w == [0x0F, 0x01, 0xD0])); // xgetbv
}

#[test]
fn runtime_generic_radix_sort_kernel_8lane_uses_small_network_without_cpu_dispatch() {
    let mut x = X86Program::new();
    let slots: Vec<usize> = (0..8usize).collect();
    let program = RuntimeProgram {
        slots: 8,
        instrs: vec![
            RuntimeInstr::RadixSortFixedInt {
                slots,
                bits: 64,
                signed: false,
                stable: false,
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(!code.windows(2).any(|w| w == [0x0F, 0xA2])); // cpuid
    assert!(!code.windows(3).any(|w| w == [0x0F, 0x01, 0xD0])); // xgetbv
}

#[test]
fn runtime_generic_radix_sort_respects_disabled_vector_features() {
    let mut x = X86Program::with_options(X86BackendOptions {
        target_features: TargetFeatureSet {
            avx2: false,
            avx512f: false,
            bmi2: true,
            popcnt: true,
        },
        ..X86BackendOptions::default()
    });
    let slots: Vec<usize> = (0..32usize).collect();
    let program = RuntimeProgram {
        slots: 32,
        instrs: vec![
            RuntimeInstr::RadixSortFixedInt {
                slots,
                bits: 64,
                signed: false,
                stable: false,
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(!code.windows(2).any(|w| w == [0x0F, 0xA2])); // cpuid
    assert!(!code.windows(3).any(|w| w == [0x0F, 0x01, 0xD0])); // xgetbv
}

#[test]
fn runtime_generic_large_zero_run_uses_compact_rep_stosb() {
    let mut x = X86Program::new();
    let base_slots: Vec<usize> = (0..64usize).collect();
    let mut instrs = Vec::new();
    for i in 0..64usize {
        instrs.push(RuntimeInstr::Mov {
            dst: i,
            src: RuntimeOperand::Imm(0),
        });
    }
    instrs.push(RuntimeInstr::LoadSeed {
        dst: 64,
        kind: RuntimeLoadKind::EntropySeed,
        input: None,
    });
    instrs.push(RuntimeInstr::LoadIndex {
        dst: 65,
        base_slots,
        index: RuntimeOperand::Slot(64),
    });
    instrs.push(RuntimeInstr::Exit {
        code: RuntimeOperand::Slot(65),
    });

    let program = RuntimeProgram { slots: 66, instrs };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    assert!(code.windows(2).any(|w| w == [0xF3, 0xAA])); // rep stosb
    assert!(code.windows(5).any(|w| w == [0xB9, 0x40, 0x00, 0x00, 0x00])); // mov ecx, 64
}

#[test]
fn runtime_generic_medium_zero_run_uses_vector_stores() {
    let mut x = X86Program::new();
    let base_slots: Vec<usize> = (0..24usize).collect();
    let mut instrs = Vec::new();
    for i in 0..24usize {
        instrs.push(RuntimeInstr::Mov {
            dst: i,
            src: RuntimeOperand::Imm(0),
        });
    }
    instrs.push(RuntimeInstr::Mov {
        dst: 24,
        src: RuntimeOperand::Imm(0),
    });
    instrs.push(RuntimeInstr::LoadIndex {
        dst: 25,
        base_slots,
        index: RuntimeOperand::Slot(24),
    });
    instrs.push(RuntimeInstr::Exit {
        code: RuntimeOperand::Slot(25),
    });

    let program = RuntimeProgram { slots: 26, instrs };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    assert!(!code.windows(3).any(|w| w == [0xF3, 0x48, 0xAB])); // no rep stosq
    assert!(code.windows(4).any(|w| w == [0x66, 0x0F, 0xEF, 0xC0])); // pxor xmm0, xmm0
    assert!(code.windows(2).any(|w| w == [0x0F, 0x11])); // movups [..], xmm0
}

#[test]
fn runtime_generic_small_zero_run_keeps_scalar_stores() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 4,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(0),
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Imm(0),
            },
            RuntimeInstr::Mov {
                dst: 2,
                src: RuntimeOperand::Imm(0),
            },
            RuntimeInstr::Mov {
                dst: 3,
                src: RuntimeOperand::Imm(0),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(!code.windows(3).any(|w| w == [0xF3, 0x48, 0xAB]));
}

#[test]
fn runtime_generic_indexed_array_uses_contiguous_stack_fast_path() {
    let mut x = X86Program::new();
    let base_slots: Vec<usize> = (0..64usize).collect();
    let mut instrs = Vec::new();
    for i in 0..64usize {
        instrs.push(RuntimeInstr::Mov {
            dst: i,
            src: RuntimeOperand::Imm(i as u64),
        });
    }
    instrs.push(RuntimeInstr::Mov {
        dst: 64,
        src: RuntimeOperand::Imm(7),
    });
    instrs.push(RuntimeInstr::LoadIndex {
        dst: 65,
        base_slots: base_slots.clone(),
        index: RuntimeOperand::Slot(64),
    });
    instrs.push(RuntimeInstr::BinOpInPlace {
        dst: 65,
        op: RuntimeBinOp::BitXor,
        rhs: RuntimeOperand::Imm(0xDEAD_BEEF),
    });
    instrs.push(RuntimeInstr::StoreIndex {
        base_slots,
        index: RuntimeOperand::Slot(64),
        src: RuntimeOperand::Slot(65),
    });
    instrs.push(RuntimeInstr::Exit {
        code: RuntimeOperand::Slot(65),
    });

    let program = RuntimeProgram { slots: 66, instrs };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    // Fast contiguous stack indexing should emit one bounds-check cmp per access,
    // not an O(N) cmp ladder per element.
    let cmp_count = code.windows(3).filter(|w| *w == [0x48, 0x81, 0xF9]).count();
    assert!(
        cmp_count <= 4,
        "expected indexed fast path, cmp_count={cmp_count}"
    );
}

#[test]
fn runtime_generic_constant_index_access_avoids_index_dispatch() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 4,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(11),
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Imm(13),
            },
            RuntimeInstr::LoadIndex {
                dst: 2,
                base_slots: vec![0, 1],
                index: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::StoreIndex {
                base_slots: vec![0, 1],
                index: RuntimeOperand::Imm(0),
                src: RuntimeOperand::Slot(2),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    let cmp_count = code.windows(3).filter(|w| *w == [0x48, 0x81, 0xF9]).count();
    assert_eq!(
        cmp_count, 0,
        "constant index path should not emit cmp rcx, imm"
    );
    assert!(
        !code.windows(4).any(|w| *w == [0x48, 0x63, 0x0C, 0x8A]),
        "constant index path should not emit jump-table dispatch"
    );
}

#[test]
fn runtime_generic_binop_preserves_register_rhs_when_it_is_the_destination() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 1,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(3),
            },
            RuntimeInstr::BinOp {
                dst: 0,
                op: RuntimeBinOp::Shl,
                lhs: RuntimeOperand::Imm(1),
                rhs: RuntimeOperand::Slot(0),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    let capture_rhs = code
        .windows(3)
        .position(|window| window == [0x4C, 0x89, 0xE1])
        .expect("expected mov rcx, r12 before overwriting destination");
    let load_lhs = code
        .windows(5)
        .position(|window| window == [0xB8, 1, 0, 0, 0])
        .expect("expected mov eax, 1");
    assert!(capture_rhs < load_lhs);
}

#[test]
fn runtime_generic_masked_u32_affine_chain_uses_native_32_bit_arithmetic() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 4,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(123_456_789),
            },
            RuntimeInstr::BinOp {
                dst: 1,
                op: RuntimeBinOp::Mul,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(1_664_525),
            },
            RuntimeInstr::BinOp {
                dst: 2,
                op: RuntimeBinOp::Add,
                lhs: RuntimeOperand::Slot(1),
                rhs: RuntimeOperand::Imm(1_013_904_223),
            },
            RuntimeInstr::BinOp {
                dst: 3,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(2),
                rhs: RuntimeOperand::Imm(u64::from(u32::MAX)),
            },
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Slot(3),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.contains(&0x69));
    assert!(
        !code
            .windows(2)
            .any(|window| window[1] == 0x69 && (window[0] & 0xF8) == 0x48)
    );
    assert!(
        code.windows(4)
            .any(|window| window == 1_013_904_223u32.to_le_bytes())
    );
}

#[test]
fn runtime_u32_affine_fusion_rejects_loop_carried_intermediate_read() {
    let program = RuntimeProgram {
        slots: 4,
        instrs: vec![
            RuntimeInstr::BinOp {
                dst: 1,
                op: RuntimeBinOp::Mul,
                lhs: RuntimeOperand::Slot(1),
                rhs: RuntimeOperand::Imm(1_664_525),
            },
            RuntimeInstr::BinOp {
                dst: 2,
                op: RuntimeBinOp::Add,
                lhs: RuntimeOperand::Slot(1),
                rhs: RuntimeOperand::Imm(1_013_904_223),
            },
            RuntimeInstr::BinOp {
                dst: 3,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(2),
                rhs: RuntimeOperand::Imm(u64::from(u32::MAX)),
            },
            RuntimeInstr::Jump { target: 0 },
        ],
    };
    let mut incoming = vec![false; program.instrs.len()];
    incoming[0] = true;
    assert!(runtime_u32_affine_fusion_candidate(&program, 0, &incoming).is_none());
}

#[test]
fn full_width_exit_observation_requires_mask_and_exit_in_one_block() {
    let direct = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::LoadSeed {
                dst: 0,
                kind: RuntimeLoadKind::EntropySeed,
                input: None,
            },
            RuntimeInstr::BinOp {
                dst: 1,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(127),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(1),
            },
        ],
    };
    assert!(matches!(
        full_width_exit_operand(&direct, 2),
        Some(RuntimeOperand::Slot(0))
    ));

    let bypassed = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::Jump { target: 2 },
            RuntimeInstr::BinOp {
                dst: 1,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(127),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(1),
            },
        ],
    };
    assert!(full_width_exit_operand(&bypassed, 2).is_none());
}

#[test]
fn runtime_generic_shift_or_reuses_dead_input_register() {
    let program = RuntimeProgram {
        slots: 4,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(0x1234),
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Imm(0x5678),
            },
            RuntimeInstr::BinOp {
                dst: 2,
                op: RuntimeBinOp::Shl,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(32),
            },
            RuntimeInstr::BinOp {
                dst: 3,
                op: RuntimeBinOp::BitOr,
                lhs: RuntimeOperand::Slot(2),
                rhs: RuntimeOperand::Slot(1),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(3),
            },
        ],
    };
    let incoming = vec![false; program.instrs.len()];
    let fusion = runtime_shift_or_fusion_candidate(&program, 2, &incoming)
        .expect("dead shifted input permits destructive composition");
    assert_eq!(fusion.dst, 3);
    assert_eq!(fusion.shift, 32);
}

#[test]
fn runtime_generic_unchecked_indexed_array_skips_bounds_cmp() {
    let mut x = X86Program::new();
    let base_slots: Vec<usize> = (11..75usize).collect();
    let mut instrs = Vec::new();
    // Keep r12..r15/rbx/r8..r11/rsi/rdi occupied by non-base slots so base slots
    // stay contiguous on stack and hit the unchecked fast path.
    for i in 0..11usize {
        instrs.push(RuntimeInstr::Mov {
            dst: i,
            src: RuntimeOperand::Imm(i as u64),
        });
        instrs.push(RuntimeInstr::BinOpInPlace {
            dst: i,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Imm(1),
        });
    }
    instrs.push(RuntimeInstr::Mov {
        dst: 75,
        src: RuntimeOperand::Imm(7),
    });
    instrs.push(RuntimeInstr::LoadIndexUnchecked {
        dst: 76,
        base_slots: base_slots.clone(),
        index: RuntimeOperand::Slot(75),
    });
    instrs.push(RuntimeInstr::StoreIndexUnchecked {
        base_slots,
        index: RuntimeOperand::Slot(75),
        src: RuntimeOperand::Slot(76),
    });
    instrs.push(RuntimeInstr::Exit {
        code: RuntimeOperand::Slot(76),
    });

    let program = RuntimeProgram { slots: 77, instrs };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    let cmp_count = code.windows(3).filter(|w| *w == [0x48, 0x81, 0xF9]).count();
    assert_eq!(cmp_count, 0, "unexpected bounds cmp in unchecked path");
    assert!(
        !code.windows(4).any(|w| *w == [0x48, 0x63, 0x0C, 0x8A]),
        "unchecked contiguous path should avoid jump-table dispatch"
    );
}

#[test]
fn runtime_generic_unchecked_indexed_jump_table_skips_bounds_cmp() {
    let mut x = X86Program::new();
    // Deliberately shuffled order forces jump-table fallback instead of
    // contiguous stack addressing.
    let base_slots = vec![0usize, 2, 1, 3];
    let program = RuntimeProgram {
        slots: 6,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(11),
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Imm(13),
            },
            RuntimeInstr::Mov {
                dst: 2,
                src: RuntimeOperand::Imm(17),
            },
            RuntimeInstr::Mov {
                dst: 3,
                src: RuntimeOperand::Imm(19),
            },
            RuntimeInstr::LoadSeed {
                dst: 4,
                kind: RuntimeLoadKind::EntropySeed,
                input: None,
            },
            RuntimeInstr::LoadIndexUnchecked {
                dst: 5,
                base_slots: base_slots.clone(),
                index: RuntimeOperand::Slot(4),
            },
            RuntimeInstr::StoreIndexUnchecked {
                base_slots,
                index: RuntimeOperand::Slot(4),
                src: RuntimeOperand::Slot(5),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(5),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    let cmp_count = code.windows(3).filter(|w| *w == [0x48, 0x81, 0xF9]).count();
    assert_eq!(cmp_count, 0, "unexpected bounds cmp in unchecked fallback");
    assert!(code.windows(4).any(|w| w == [0x48, 0x63, 0x0C, 0x8A]));
}

#[test]
fn runtime_generic_bit_test_fusion_emits_bt() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 4,
        instrs: vec![
            RuntimeInstr::LoadSeed {
                dst: 0,
                kind: RuntimeLoadKind::EntropySeed,
                input: None,
            },
            RuntimeInstr::LoadSeed {
                dst: 1,
                kind: RuntimeLoadKind::EntropySeed,
                input: None,
            },
            RuntimeInstr::BinOp {
                dst: 2,
                op: RuntimeBinOp::ShrUnsigned,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Slot(1),
            },
            RuntimeInstr::BinOp {
                dst: 3,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(2),
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(3),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    assert!(
        code.windows(4).any(|w| *w == [0x48, 0x0F, 0xA3, 0xC8]),
        "expected bt fusion sequence"
    );
}

#[test]
fn runtime_generic_bitset_store_fusion_emits_bts() {
    let mut x = X86Program::new();
    let base_slots = vec![0usize, 1, 2, 3];
    let program = RuntimeProgram {
        slots: 9,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(11),
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Imm(13),
            },
            RuntimeInstr::Mov {
                dst: 2,
                src: RuntimeOperand::Imm(17),
            },
            RuntimeInstr::Mov {
                dst: 3,
                src: RuntimeOperand::Imm(19),
            },
            RuntimeInstr::Mov {
                dst: 4,
                src: RuntimeOperand::Imm(2),
            },
            RuntimeInstr::Mov {
                dst: 7,
                src: RuntimeOperand::Imm(5),
            },
            RuntimeInstr::LoadIndexUnchecked {
                dst: 5,
                base_slots: base_slots.clone(),
                index: RuntimeOperand::Slot(4),
            },
            RuntimeInstr::BinOp {
                dst: 6,
                op: RuntimeBinOp::Shl,
                lhs: RuntimeOperand::Imm(1),
                rhs: RuntimeOperand::Slot(7),
            },
            RuntimeInstr::BinOp {
                dst: 8,
                op: RuntimeBinOp::BitOr,
                lhs: RuntimeOperand::Slot(5),
                rhs: RuntimeOperand::Slot(6),
            },
            RuntimeInstr::StoreIndexUnchecked {
                base_slots,
                index: RuntimeOperand::Slot(4),
                src: RuntimeOperand::Slot(8),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(8),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    assert!(
        code.windows(4).any(|w| *w == [0x48, 0x0F, 0xAB, 0xC8]),
        "expected bts fusion sequence"
    );
}

#[test]
fn runtime_generic_indexed_bit_test_fusion_emits_bt_mem() {
    let mut x = X86Program::new();
    let base_slots = vec![0usize, 1, 2, 3];
    let program = RuntimeProgram {
        slots: 9,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(11),
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Imm(13),
            },
            RuntimeInstr::Mov {
                dst: 2,
                src: RuntimeOperand::Imm(17),
            },
            RuntimeInstr::Mov {
                dst: 3,
                src: RuntimeOperand::Imm(19),
            },
            RuntimeInstr::Mov {
                dst: 4,
                src: RuntimeOperand::Imm(2),
            },
            RuntimeInstr::LoadIndexUnchecked {
                dst: 5,
                base_slots,
                index: RuntimeOperand::Slot(4),
            },
            RuntimeInstr::BinOp {
                dst: 6,
                op: RuntimeBinOp::ShrUnsigned,
                lhs: RuntimeOperand::Slot(5),
                rhs: RuntimeOperand::Imm(5),
            },
            RuntimeInstr::BinOp {
                dst: 8,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(6),
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(8),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    assert!(
        code.windows(3).any(|w| *w == [0x48, 0x0F, 0xA3]),
        "expected bt r/m64, r64 indexed fusion"
    );
}

#[test]
fn indexed_bit_test_accumulation_is_local_and_liveness_proven() {
    let mut program = RuntimeProgram {
        slots: 8,
        instrs: vec![
            RuntimeInstr::LoadIndexUnchecked {
                dst: 4,
                base_slots: vec![0, 1, 2, 3],
                index: RuntimeOperand::Slot(5),
            },
            RuntimeInstr::BinOp {
                dst: 6,
                op: RuntimeBinOp::ShrUnsigned,
                lhs: RuntimeOperand::Slot(4),
                rhs: RuntimeOperand::Slot(7),
            },
            RuntimeInstr::BinOp {
                dst: 6,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(6),
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::BinOpInPlace {
                dst: 5,
                op: RuntimeBinOp::BitAnd,
                rhs: RuntimeOperand::Slot(6),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(5),
            },
        ],
    };
    let incoming = vec![false; program.instrs.len()];
    let fusion = runtime_bit_test_accumulate_fusion_candidate(&program, 0, &incoming)
        .expect("generic indexed bit-test reduction should fuse");
    assert_eq!(fusion.dst, 5);
    assert!(matches!(fusion.accumulator, RuntimeOperand::Slot(5)));

    // Observing the extracted boolean after the reduction invalidates the
    // dead-temporary proof and must retain the scalar sequence.
    program.instrs.insert(
        4,
        RuntimeInstr::PrintInt {
            value: RuntimeOperand::Slot(6),
            signed: false,
            bits: 64,
        },
    );
    let incoming = vec![false; program.instrs.len()];
    assert!(runtime_bit_test_accumulate_fusion_candidate(&program, 0, &incoming).is_none());
}

#[test]
fn indexed_bit_accumulation_coalesces_a_later_unobserved_copy_back() {
    let program = RuntimeProgram {
        slots: 12,
        instrs: vec![
            RuntimeInstr::LoadIndexUnchecked {
                dst: 4,
                base_slots: vec![0, 1, 2, 3],
                index: RuntimeOperand::Slot(5),
            },
            RuntimeInstr::BinOp {
                dst: 6,
                op: RuntimeBinOp::ShrUnsigned,
                lhs: RuntimeOperand::Slot(4),
                rhs: RuntimeOperand::Slot(7),
            },
            RuntimeInstr::BinOp {
                dst: 6,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(6),
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::BinOp {
                dst: 8,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(9),
                rhs: RuntimeOperand::Slot(6),
            },
            RuntimeInstr::BinOp {
                dst: 10,
                op: RuntimeBinOp::Add,
                lhs: RuntimeOperand::Slot(11),
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::Mov {
                dst: 9,
                src: RuntimeOperand::Slot(8),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(9),
            },
        ],
    };
    let incoming = vec![false; program.instrs.len()];
    let fusion = runtime_bit_test_accumulate_fusion_candidate(&program, 0, &incoming)
        .expect("copy-back form should fuse");
    assert_eq!(fusion.dst, 9);
    assert_eq!(fusion.suppressed_copy, Some(5));
}

#[test]
fn indexed_bit_accumulation_folds_general_power_of_two_address_expressions() {
    let base_slots: Vec<usize> = (0..32).collect();
    let program = RuntimeProgram {
        slots: 44,
        instrs: vec![
            RuntimeInstr::BinOp {
                dst: 33,
                op: RuntimeBinOp::ShrUnsigned,
                lhs: RuntimeOperand::Slot(32),
                rhs: RuntimeOperand::Imm(17),
            },
            RuntimeInstr::BinOp {
                dst: 34,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(33),
                rhs: RuntimeOperand::Imm(31),
            },
            RuntimeInstr::Mov {
                dst: 42,
                src: RuntimeOperand::Slot(34),
            },
            RuntimeInstr::BinOp {
                dst: 35,
                op: RuntimeBinOp::ShrUnsigned,
                lhs: RuntimeOperand::Slot(32),
                rhs: RuntimeOperand::Imm(41),
            },
            RuntimeInstr::BinOp {
                dst: 36,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Imm(63),
                rhs: RuntimeOperand::Slot(35),
            },
            RuntimeInstr::Mov {
                dst: 43,
                src: RuntimeOperand::Slot(36),
            },
            RuntimeInstr::Mov {
                dst: 40,
                src: RuntimeOperand::Slot(34),
            },
            RuntimeInstr::Mov {
                dst: 41,
                src: RuntimeOperand::Slot(36),
            },
            RuntimeInstr::LoadIndexUnchecked {
                dst: 37,
                base_slots,
                index: RuntimeOperand::Slot(40),
            },
            RuntimeInstr::BinOp {
                dst: 38,
                op: RuntimeBinOp::ShrUnsigned,
                lhs: RuntimeOperand::Slot(37),
                rhs: RuntimeOperand::Slot(41),
            },
            RuntimeInstr::BinOp {
                dst: 38,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(38),
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::BinOp {
                dst: 39,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Imm(1),
                rhs: RuntimeOperand::Slot(38),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(39),
            },
        ],
    };
    let incoming = vec![false; program.instrs.len()];
    let fusion = runtime_bit_test_accumulate_fusion_candidate(&program, 8, &incoming)
        .expect("the local indexed bit accumulation should fuse");

    let index = fusion
        .index_expression
        .expect("a proven 32-element address expression should fold");
    assert_eq!(index.shift, 17);
    assert_eq!(index.mask, 31);
    assert_eq!(index.suppressed_instrs, [0, 1, 2, 6]);
    let bit = fusion
        .bit_expression
        .expect("the u64 shift-count expression should fold");
    assert_eq!(bit.shift, 41);
    assert_eq!(bit.mask, 63);
    assert_eq!(bit.suppressed_instrs, [3, 4, 5, 7]);
}

#[test]
fn indexed_bit_address_fold_rejects_wrong_mask_and_observed_temporaries() {
    let base_slots: Vec<usize> = (0..32).collect();
    let mut program = RuntimeProgram {
        slots: 41,
        instrs: vec![
            RuntimeInstr::BinOp {
                dst: 33,
                op: RuntimeBinOp::ShrUnsigned,
                lhs: RuntimeOperand::Slot(32),
                rhs: RuntimeOperand::Imm(9),
            },
            RuntimeInstr::BinOp {
                dst: 34,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(33),
                rhs: RuntimeOperand::Imm(30),
            },
            RuntimeInstr::Mov {
                dst: 35,
                src: RuntimeOperand::Slot(32),
            },
            RuntimeInstr::BinOp {
                dst: 36,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(35),
                rhs: RuntimeOperand::Imm(63),
            },
            RuntimeInstr::LoadIndexUnchecked {
                dst: 37,
                base_slots,
                index: RuntimeOperand::Slot(34),
            },
            RuntimeInstr::BinOp {
                dst: 38,
                op: RuntimeBinOp::ShrUnsigned,
                lhs: RuntimeOperand::Slot(37),
                rhs: RuntimeOperand::Slot(36),
            },
            RuntimeInstr::BinOp {
                dst: 38,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(38),
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::BinOp {
                dst: 39,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Imm(1),
                rhs: RuntimeOperand::Slot(38),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(39),
            },
        ],
    };
    let incoming = vec![false; program.instrs.len()];
    let fusion = runtime_bit_test_accumulate_fusion_candidate(&program, 4, &incoming)
        .expect("the containing local bit accumulation remains legal");
    assert!(
        fusion.index_expression.is_none(),
        "mask 30 does not prove an in-bounds index for a 32-element array"
    );
    assert!(fusion.bit_expression.is_some());

    program.instrs[1] = RuntimeInstr::BinOp {
        dst: 34,
        op: RuntimeBinOp::BitAnd,
        lhs: RuntimeOperand::Slot(33),
        rhs: RuntimeOperand::Imm(31),
    };
    program.instrs.insert(
        2,
        RuntimeInstr::PrintInt {
            value: RuntimeOperand::Slot(33),
            signed: false,
            bits: 64,
        },
    );
    let incoming = vec![false; program.instrs.len()];
    let fusion = runtime_bit_test_accumulate_fusion_candidate(&program, 5, &incoming)
        .expect("observing the address temporary need not disable outer bit fusion");
    assert!(
        fusion.index_expression.is_none(),
        "an observable producer may never be suppressed"
    );
}

fn indexed_bit_accumulation_program() -> RuntimeProgram {
    let base_slots: Vec<usize> = (0..32).collect();
    let mut instrs = (0..32)
        .map(|dst| RuntimeInstr::Mov {
            dst,
            src: RuntimeOperand::Imm(0),
        })
        .collect::<Vec<_>>();
    instrs.extend([
        RuntimeInstr::LoadSeed {
            dst: 32,
            kind: RuntimeLoadKind::EntropySeed,
            input: None,
        },
        RuntimeInstr::Mov {
            dst: 33,
            src: RuntimeOperand::Imm(7),
        },
        RuntimeInstr::Mov {
            dst: 34,
            src: RuntimeOperand::Imm(1),
        },
        RuntimeInstr::Mov {
            dst: 38,
            src: RuntimeOperand::Imm(0x1_0000_0000),
        },
        RuntimeInstr::StoreIndexUnchecked {
            base_slots: base_slots.clone(),
            index: RuntimeOperand::Slot(32),
            src: RuntimeOperand::Slot(38),
        },
        RuntimeInstr::LoadIndexUnchecked {
            dst: 35,
            base_slots,
            index: RuntimeOperand::Slot(32),
        },
        RuntimeInstr::BinOp {
            dst: 36,
            op: RuntimeBinOp::ShrUnsigned,
            lhs: RuntimeOperand::Slot(35),
            rhs: RuntimeOperand::Slot(33),
        },
        RuntimeInstr::BinOp {
            dst: 36,
            op: RuntimeBinOp::BitAnd,
            lhs: RuntimeOperand::Slot(36),
            rhs: RuntimeOperand::Imm(1),
        },
        RuntimeInstr::BinOp {
            dst: 37,
            op: RuntimeBinOp::BitAnd,
            lhs: RuntimeOperand::Slot(34),
            rhs: RuntimeOperand::Slot(36),
        },
        RuntimeInstr::Exit {
            code: RuntimeOperand::Slot(37),
        },
    ]);
    RuntimeProgram { slots: 39, instrs }
}

#[test]
fn indexed_bit_accumulation_selects_best_legal_bmi2_memory_shift() {
    let mut x = X86Program::new();
    x.emit_runtime_generic_program(&indexed_bit_accumulation_program());
    let code = x.finalize();
    assert!(
        code.windows(5)
            .any(|window| window[0] == 0xC4 && window[3] == 0xF7),
        "expected automatic SHRX memory selection for the configured target"
    );
}

#[test]
fn unsigned_low_bit_masks_select_canonical_zero_extensions() {
    let mut low_byte = X86Program::new();
    low_byte.emit_mask_unsigned_low_bits(1, 0xFF);
    assert_eq!(low_byte.code, [0x0F, 0xB6, 0xC9]); // movzx ecx, cl

    let mut extended_byte = X86Program::new();
    extended_byte.emit_mask_unsigned_low_bits(8, 0xFF);
    assert_eq!(extended_byte.code, [0x45, 0x0F, 0xB6, 0xC0]); // movzx r8d, r8b

    let mut low_word = X86Program::new();
    low_word.emit_mask_unsigned_low_bits(2, 0xFFFF);
    assert_eq!(low_word.code, [0x0F, 0xB7, 0xD2]); // movzx edx, dx

    let mut low_dword = X86Program::new();
    low_dword.emit_mask_unsigned_low_bits(1, u32::MAX);
    assert_eq!(low_dword.code, [0x89, 0xC9]); // mov ecx, ecx

    let mut direct_byte = X86Program::new();
    direct_byte.emit_mask_unsigned_low_bits_from_reg(1, 3, 0xFF);
    assert_eq!(direct_byte.code, [0x0F, 0xB6, 0xCB]); // movzx ecx, bl
}

#[test]
fn non_destructive_rotate_extract_encodes_all_register_banks() {
    let cases = [
        (0, 1, 8, vec![0xC4, 0xE3, 0xFB, 0xF0, 0xC1, 0x08]),
        (11, 15, 17, vec![0xC4, 0x43, 0xFB, 0xF0, 0xDF, 0x11]),
    ];
    for (dst, src, amount, expected) in cases {
        let mut program = X86Program::new();
        program.emit_rorx_reg_reg_imm8(dst, src, amount);
        assert_eq!(program.code, expected, "dst={dst} src={src}");
    }
}

#[test]
fn two_register_imul_maps_extended_register_bits_by_modrm_direction() {
    let cases = [
        (3, 15, vec![0x49, 0x0F, 0xAF, 0xDF]),  // imul rbx, r15
        (11, 7, vec![0x4C, 0x0F, 0xAF, 0xDF]),  // imul r11, rdi
        (11, 15, vec![0x4D, 0x0F, 0xAF, 0xDF]), // imul r11, r15
    ];
    for (dst, src, expected) in cases {
        let mut program = X86Program::new();
        program.emit_binop_reg_reg_in_place(RuntimeBinOp::Mul, dst, src);
        assert_eq!(program.code, expected, "dst={dst} src={src}");
    }
}

#[test]
fn indexed_bit_accumulation_preserves_cpu_safety_without_bmi2() {
    let mut x = X86Program::with_options(X86BackendOptions {
        target_features: TargetFeatureSet {
            bmi2: false,
            ..TargetFeatureSet::default()
        },
        ..X86BackendOptions::default()
    });
    x.emit_runtime_generic_program(&indexed_bit_accumulation_program());
    let code = x.finalize();
    assert!(
        !code
            .windows(5)
            .any(|window| window[0] == 0xC4 && window[3] == 0xF7)
    );
    assert!(
        code.windows(3).any(|window| window == [0x48, 0x0F, 0xA3]),
        "expected scalar BT fallback"
    );
}

#[test]
fn boolean_accumulator_proof_follows_only_local_dominating_definitions() {
    let program = RuntimeProgram {
        slots: 4,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::BinOp {
                dst: 1,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Slot(2),
            },
            RuntimeInstr::Mov {
                dst: 3,
                src: RuntimeOperand::Slot(1),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(3),
            },
        ],
    };
    let incoming = vec![false; program.instrs.len()];
    assert!(runtime_operand_is_boolean_before(
        &program,
        3,
        RuntimeOperand::Slot(3),
        &incoming,
        0,
    ));

    let mut merged = incoming;
    merged[2] = true;
    assert!(!runtime_operand_is_boolean_before(
        &program,
        3,
        RuntimeOperand::Slot(3),
        &merged,
        0,
    ));
}

#[test]
fn runtime_generic_dynamic_indexed_bit_test_preserves_modulo_bit_index() {
    let mut x = X86Program::new();
    let base_slots = vec![0usize, 1, 2, 3];
    let program = RuntimeProgram {
        slots: 10,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(11),
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Imm(13),
            },
            RuntimeInstr::Mov {
                dst: 2,
                src: RuntimeOperand::Imm(17),
            },
            RuntimeInstr::Mov {
                dst: 3,
                src: RuntimeOperand::Imm(19),
            },
            RuntimeInstr::Mov {
                dst: 4,
                src: RuntimeOperand::Imm(2),
            },
            RuntimeInstr::LoadSeed {
                dst: 5,
                kind: RuntimeLoadKind::EntropySeed,
                input: None,
            },
            RuntimeInstr::LoadIndexUnchecked {
                dst: 6,
                base_slots: base_slots.clone(),
                index: RuntimeOperand::Slot(4),
            },
            RuntimeInstr::BinOp {
                dst: 7,
                op: RuntimeBinOp::ShrUnsigned,
                lhs: RuntimeOperand::Slot(6),
                rhs: RuntimeOperand::Slot(5),
            },
            RuntimeInstr::BinOp {
                dst: 8,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(7),
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(8),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    assert!(
        code.windows(3).any(|w| *w == [0x48, 0x0F, 0xA3]),
        "expected bt r/m64, r64 indexed fusion"
    );
    // Register BT applies the same low-six-bit modulo rule as a variable
    // 64-bit shift, so no separate mask instruction is required.
}

#[test]
fn runtime_generic_indexed_bitset_store_fusion_can_emit_bts_mem() {
    let mut x = X86Program::new();
    let base_slots = vec![0usize, 1, 2, 3];
    let program = RuntimeProgram {
        slots: 9,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(11),
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Imm(13),
            },
            RuntimeInstr::Mov {
                dst: 2,
                src: RuntimeOperand::Imm(17),
            },
            RuntimeInstr::Mov {
                dst: 3,
                src: RuntimeOperand::Imm(19),
            },
            RuntimeInstr::Mov {
                dst: 4,
                src: RuntimeOperand::Imm(2),
            },
            RuntimeInstr::LoadIndexUnchecked {
                dst: 5,
                base_slots: base_slots.clone(),
                index: RuntimeOperand::Slot(4),
            },
            RuntimeInstr::BinOp {
                dst: 6,
                op: RuntimeBinOp::Shl,
                lhs: RuntimeOperand::Imm(1),
                rhs: RuntimeOperand::Imm(5),
            },
            RuntimeInstr::BinOp {
                dst: 8,
                op: RuntimeBinOp::BitOr,
                lhs: RuntimeOperand::Slot(5),
                rhs: RuntimeOperand::Slot(6),
            },
            RuntimeInstr::StoreIndexUnchecked {
                base_slots,
                index: RuntimeOperand::Slot(4),
                src: RuntimeOperand::Slot(8),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    assert!(
        code.windows(3).any(|w| *w == [0x48, 0x0F, 0xAB]),
        "expected bts r/m64, r64 indexed fusion"
    );
}

#[test]
fn runtime_generic_load_index_cmp_jump_fusion_avoids_spilling_loaded_value() {
    let mut x = X86Program::new();
    let instrs = vec![
        RuntimeInstr::Mov {
            dst: 17,
            src: RuntimeOperand::Imm(7),
        },
        RuntimeInstr::LoadIndexUnchecked {
            dst: 0,
            base_slots: (0..16usize).collect(),
            index: RuntimeOperand::Slot(17),
        },
        RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Eq,
            lhs: RuntimeOperand::Slot(0),
            rhs: RuntimeOperand::Imm(0),
            target: 4,
        },
        RuntimeInstr::Exit {
            code: RuntimeOperand::Imm(1),
        },
        RuntimeInstr::Exit {
            code: RuntimeOperand::Imm(0),
        },
    ];

    let program = RuntimeProgram { slots: 18, instrs };
    let slot_map = RuntimeSlotMap::build(&program);
    let stack_disp = stack_slot_disp(slot_map.stack_index(0).expect("slot 0 must spill"));

    x.emit_runtime_generic_program(&program);
    let code = x.finalize();

    let has_store_to_slot0 = if i8::try_from(stack_disp).is_ok() {
        code.windows(4)
            .any(|w| w == [0x48, 0x89, 0x45, stack_disp as i8 as u8])
    } else {
        let disp = stack_disp.to_le_bytes();
        code.windows(7)
            .any(|w| w[0] == 0x48 && w[1] == 0x89 && w[2] == 0x85 && w[3..7] == disp)
    };
    assert!(
        !has_store_to_slot0,
        "fused load-index+cmp+jump should avoid spilling transient loaded slot"
    );
}

#[test]
fn runtime_generic_mul_pow2_in_place_prefers_shift() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 1,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(3),
            },
            RuntimeInstr::BinOpInPlace {
                dst: 0,
                op: RuntimeBinOp::Mul,
                rhs: RuntimeOperand::Imm(4),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.windows(4).any(|w| w == [0x49, 0xC1, 0xE4, 0x02])); // shl r12, 2
}

#[test]
fn runtime_generic_signed_cmp_emits_signed_false_jump() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 1,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(u64::MAX - 4),
            },
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtSigned,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(0),
                target: 3,
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(0),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(1),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.windows(2).any(|w| w == [0x0F, 0x8D])); // jge rel32
}

#[test]
fn runtime_generic_normalize_int32_signed_emits_shift_pair() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 1,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(0xFFFF_FFFF),
            },
            RuntimeInstr::NormalizeInt {
                dst: 0,
                signed: true,
                bits: 32,
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(0),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.windows(4).any(|w| w == [0x49, 0xC1, 0xE4, 0x20])); // shl r12, 32
    assert!(code.windows(4).any(|w| w == [0x49, 0xC1, 0xFC, 0x20])); // sar r12, 32
}

#[test]
fn runtime_generic_div_unsigned_pow2_uses_shift() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::LoadSeed {
                dst: 0,
                kind: RuntimeLoadKind::EntropySeed,
                input: None,
            },
            RuntimeInstr::BinOp {
                dst: 1,
                op: RuntimeBinOp::DivUnsigned,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(8),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(1),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    // The register allocator may assign the slot to any register (rax, r12, etc).
    // What matters is that a shift-right-by-3 was emitted (opcode C1 /5 imm8=3),
    // NOT a div instruction (0xF7). Check that no div appears and a shift by 3 does.
    assert!(
        !code.windows(2).any(|w| w == [0xF7, 0xF1]) // no `div rcx`
            && code.windows(2).any(|w| w == [0xC1, 0xE8]   // shr rax, ...
                || w == [0xC1, 0xEC]                         // shr r12, ...
                || w == [0xC1, 0xED]                         // shr r13, ...
                || w == [0xC1, 0xEE]                         // shr r14, ...
                || w == [0xC1, 0xEB]                         // shr rbx, ...
            ),
        "expected a shift-right instruction, not a div"
    );
    assert!(
        code.windows(1).any(|w| w == [0x03]),
        "expected shift amount of 3"
    );
}

#[test]
fn runtime_generic_div_signed_emits_idiv() {
    let mut x = X86Program::new();
    let program = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::LoadSeed {
                dst: 0,
                kind: RuntimeLoadKind::EntropySeed,
                input: None,
            },
            RuntimeInstr::BinOp {
                dst: 1,
                op: RuntimeBinOp::DivSigned,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(2),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(1),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(code.windows(2).any(|w| w == [0x48, 0x99])); // cqo
    assert!(code.windows(3).any(|w| w == [0x48, 0xF7, 0xF9])); // idiv rcx
}

#[test]
fn runtime_generic_indexed_increment_fuses_to_memory_rmw() {
    let mut x = X86Program::new();
    // A runtime array large enough to be assigned a contiguous stack range.
    let base_slots: Vec<usize> = (0..64).collect();
    let program = RuntimeProgram {
        slots: 68,
        instrs: vec![
            RuntimeInstr::LoadSeed {
                dst: 64,
                kind: RuntimeLoadKind::EntropySeed,
                input: None,
            },
            RuntimeInstr::BinOp {
                dst: 65,
                op: RuntimeBinOp::BitAnd,
                lhs: RuntimeOperand::Slot(64),
                rhs: RuntimeOperand::Imm(63),
            },
            RuntimeInstr::LoadIndexUnchecked {
                dst: 66,
                base_slots: base_slots.clone(),
                index: RuntimeOperand::Slot(65),
            },
            RuntimeInstr::BinOp {
                dst: 67,
                op: RuntimeBinOp::Add,
                lhs: RuntimeOperand::Slot(66),
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::StoreIndexUnchecked {
                base_slots,
                index: RuntimeOperand::Slot(65),
                src: RuntimeOperand::Slot(67),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(64),
            },
        ],
    };
    x.emit_runtime_generic_program(&program);
    let code = x.finalize();
    assert!(
        code.windows(3).any(|bytes| {
            matches!(bytes[0], 0x48 | 0x4a) && bytes[1] == 0xff && matches!(bytes[2], 0x44 | 0x84)
        }),
        "expected a fused indexed incq instruction"
    );
}

#[test]
fn exact_unroll_emission_plan_preserves_ir_blocks_and_collapses_machine_guards() {
    let program = RuntimeProgram {
        slots: 1,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(0),
            },
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(8),
                target: 6,
            },
            RuntimeInstr::BinOpInPlace {
                dst: 0,
                op: RuntimeBinOp::Add,
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(8),
                target: 6,
            },
            RuntimeInstr::BinOpInPlace {
                dst: 0,
                op: RuntimeBinOp::Add,
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::Jump { target: 1 },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(0),
            },
        ],
    };
    let plan = runtime_exact_unroll_emission_plan(&program);
    assert!(!plan.suppress_guard[1]);
    assert!(plan.suppress_guard[3]);
    assert_eq!(plan.induction_increment[2], Some(0));
    assert_eq!(plan.induction_increment[4], Some(2));
    assert_eq!(program.instrs.len(), 7, "the IR and its CFG stay unchanged");
}

#[test]
fn counted_loop_emission_plan_uses_only_a_dead_control_induction() {
    let program = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(2),
            },
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(12),
                target: 6,
            },
            RuntimeInstr::BinOpInPlace {
                dst: 1,
                op: RuntimeBinOp::Add,
                rhs: RuntimeOperand::Imm(7),
            },
            RuntimeInstr::Jump { target: 4 },
            RuntimeInstr::BinOpInPlace {
                dst: 0,
                op: RuntimeBinOp::Add,
                rhs: RuntimeOperand::Imm(2),
            },
            RuntimeInstr::Jump { target: 1 },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(1),
            },
        ],
    };
    let plan = runtime_counted_loop_emission_plan(&program);
    assert_eq!(plan.initializer_count[0], Some(5));
    assert!(plan.suppress_instr[1]);
    assert!(!plan.suppress_instr[3]);
    assert!(plan.suppress_instr[4]);
    assert_eq!(plan.latch[5], Some((0, 1)));

    let mut machine = X86Program::new();
    machine.emit_runtime_generic_program(&program);
    assert!(
        machine.code.windows(2).any(|bytes| bytes == [0x0F, 0x85]),
        "the latch must use the flags produced by a decrement"
    );
}

#[test]
fn counted_loop_emission_plan_rejects_observed_induction_and_side_entries() {
    let mut observed = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(0),
            },
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(8),
                target: 5,
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Slot(0),
            },
            RuntimeInstr::BinOpInPlace {
                dst: 0,
                op: RuntimeBinOp::Add,
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::Jump { target: 1 },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(1),
            },
        ],
    };
    let plan = runtime_counted_loop_emission_plan(&observed);
    assert!(plan.initializer_count.iter().all(Option::is_none));

    observed.instrs[2] = RuntimeInstr::Mov {
        dst: 1,
        src: RuntimeOperand::Imm(0),
    };
    observed.instrs.insert(
        0,
        RuntimeInstr::JumpIfZero {
            cond_slot: 1,
            target: 2,
        },
    );
    let RuntimeInstr::JumpIfCmpFalse { target, .. } = &mut observed.instrs[2] else {
        unreachable!();
    };
    *target = 6;
    observed.instrs[5] = RuntimeInstr::Jump { target: 2 };
    let plan = runtime_counted_loop_emission_plan(&observed);
    assert!(plan.initializer_count.iter().all(Option::is_none));
}

#[test]
fn counted_loop_emission_plan_rejects_wrapping_induction() {
    let program = RuntimeProgram {
        slots: 2,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(u64::MAX - 3),
            },
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Slot(0),
                rhs: RuntimeOperand::Imm(u64::MAX),
                target: 5,
            },
            RuntimeInstr::BinOpInPlace {
                dst: 1,
                op: RuntimeBinOp::Add,
                rhs: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::BinOpInPlace {
                dst: 0,
                op: RuntimeBinOp::Add,
                rhs: RuntimeOperand::Imm(4),
            },
            RuntimeInstr::Jump { target: 1 },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(1),
            },
        ],
    };
    let plan = runtime_counted_loop_emission_plan(&program);
    assert!(plan.initializer_count.iter().all(Option::is_none));
}

#[test]
fn affine_select_fusion_requires_dead_proof_temporaries() {
    let canonical = vec![
        RuntimeInstr::Cmp {
            dst: 1,
            op: RuntimeCmpOp::LtUnsigned,
            lhs: RuntimeOperand::Slot(0),
            rhs: RuntimeOperand::Imm(128),
        },
        RuntimeInstr::BinOp {
            dst: 2,
            op: RuntimeBinOp::Sub,
            lhs: RuntimeOperand::Imm(0),
            rhs: RuntimeOperand::Slot(1),
        },
        RuntimeInstr::BinOp {
            dst: 3,
            op: RuntimeBinOp::BitAnd,
            lhs: RuntimeOperand::Slot(2),
            rhs: RuntimeOperand::Imm(5 ^ 9),
        },
        RuntimeInstr::BinOpInPlace {
            dst: 3,
            op: RuntimeBinOp::BitXor,
            rhs: RuntimeOperand::Imm(9),
        },
        RuntimeInstr::BinOpInPlace {
            dst: 0,
            op: RuntimeBinOp::Mul,
            rhs: RuntimeOperand::Slot(3),
        },
        RuntimeInstr::BinOp {
            dst: 3,
            op: RuntimeBinOp::BitAnd,
            lhs: RuntimeOperand::Slot(2),
            rhs: RuntimeOperand::Imm(7 ^ 11),
        },
        RuntimeInstr::BinOpInPlace {
            dst: 3,
            op: RuntimeBinOp::BitXor,
            rhs: RuntimeOperand::Imm(11),
        },
        RuntimeInstr::BinOpInPlace {
            dst: 0,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Slot(3),
        },
        RuntimeInstr::BinOpInPlace {
            dst: 0,
            op: RuntimeBinOp::BitAnd,
            rhs: RuntimeOperand::Imm(255),
        },
        RuntimeInstr::Exit {
            code: RuntimeOperand::Slot(0),
        },
    ];
    let program = RuntimeProgram {
        slots: 4,
        instrs: canonical.clone(),
    };
    let fusion =
        runtime_affine_select_fusion_candidate(&program, 0, &vec![false; program.instrs.len()])
            .expect("canonical target-neutral affine selection");
    assert_eq!((fusion.then_mul, fusion.then_add), (5, 7));
    assert_eq!(
        (fusion.else_mul, fusion.else_add, fusion.mask),
        (9, 11, 255)
    );

    let mut observed = canonical;
    observed.insert(
        9,
        RuntimeInstr::PrintInt {
            value: RuntimeOperand::Slot(3),
            signed: false,
            bits: 64,
        },
    );
    let observed = RuntimeProgram {
        slots: 4,
        instrs: observed,
    };
    assert!(
        runtime_affine_select_fusion_candidate(&observed, 0, &vec![false; observed.instrs.len()],)
            .is_none()
    );
}
