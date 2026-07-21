use super::*;
use crate::frontend::parse_program;
use crate::frontend::semantics::{LoweredStmt, RuntimeLoadKind};
use std::fs;

#[test]
fn optimize_pipeline_merges_and_prunes() {
    let input = vec![
        LoweredStmt::Print("x".into()),
        LoweredStmt::Print("x".into()),
        LoweredStmt::Print("".into()),
        LoweredStmt::Print("y".into()),
        LoweredStmt::Exit(0),
        LoweredStmt::Print("z".into()),
    ];
    let out = optimize_semantics_ir(input);
    assert_eq!(out.len(), 2);
    match &out[0] {
        LoweredStmt::Print(text) => assert_eq!(text, "xxy"),
        _ => panic!("expected first stmt to be print"),
    }
    assert!(matches!(out[1], LoweredStmt::Exit(0)));
}

#[test]
fn peephole_does_not_leak_exit_path_constants_into_a_branch_target() {
    let instrs = vec![
        RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Eq,
            lhs: RuntimeOperand::Slot(1),
            rhs: RuntimeOperand::Imm(1),
            target: 3,
        },
        RuntimeInstr::Mov {
            dst: 0,
            src: RuntimeOperand::Imm(0),
        },
        RuntimeInstr::Exit {
            code: RuntimeOperand::Imm(101),
        },
        RuntimeInstr::HeapCopy {
            dst_ptr: RuntimeOperand::Slot(2),
            src_ptr: RuntimeOperand::Slot(0),
            bytes: RuntimeOperand::Slot(3),
        },
    ];

    let optimized = peephole_optimize_runtime_instrs(4, &instrs);
    assert!(matches!(
        optimized.last(),
        Some(RuntimeInstr::HeapCopy {
            src_ptr: RuntimeOperand::Slot(0),
            ..
        })
    ));
}

#[test]
fn peephole_does_not_leak_constants_across_forward_merge_points() {
    let instrs = vec![
        RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Eq,
            lhs: RuntimeOperand::Slot(2),
            rhs: RuntimeOperand::Imm(0),
            target: 3,
        },
        RuntimeInstr::Mov {
            dst: 0,
            src: RuntimeOperand::Imm(13),
        },
        RuntimeInstr::Jump { target: 4 },
        RuntimeInstr::Mov {
            dst: 0,
            src: RuntimeOperand::Imm(99),
        },
        RuntimeInstr::Mov {
            dst: 1,
            src: RuntimeOperand::Slot(0),
        },
        RuntimeInstr::Exit {
            code: RuntimeOperand::Slot(1),
        },
    ];

    let optimized = peephole_optimize_runtime_instrs(3, &instrs);
    assert!(matches!(
        optimized.last(),
        Some(RuntimeInstr::Exit {
            code: RuntimeOperand::Slot(1),
        })
    ));
}

#[test]
fn resource_receiver_optimization_keeps_all_control_flow_targets_in_bounds() {
    let source = "struct Bag { count: u64; values: list<u16>; } impl Bag { fn add(self: &mut Self, value: u16) { self.values.push(value); self.count = self.count + 1u64; } } fn main() { let seed: u64 = runtime_seed(); let mut bag: Bag = Bag { count: 1u64, values: [3u16] }; bag.add(5u16); bag.add(8u16); exit(seed & 0u64); }";
    let parsed = parse_program(source).expect("parse resource receiver program");
    let mut lowered = crate::frontend::semantics::lower_program(&parsed)
        .expect("lower resource receiver program");
    let passes: &[(&str, fn(Vec<LoweredStmt>) -> Vec<LoweredStmt>)] = &[
        ("hoist_loop_invariants", hoist_loop_invariants),
        ("const_fold", const_fold),
        ("fold_runtime_kernels", fold_runtime_kernels),
        ("inline_leaf_calls", inline_runtime_generic_leaf_calls),
        ("tail_calls", tail_call_optimization),
        ("simplify_cfg_1", simplify_runtime_generic_control_flow),
        ("copy_propagation", copy_propagate_runtime_generic),
        (
            "invariant_constants",
            specialize_runtime_generic_invariant_constants,
        ),
        ("simplify_cfg_2", simplify_runtime_generic_control_flow),
        ("bounds_checks", eliminate_runtime_loop_bounds_checks),
        ("small_loop_unroll", unroll_runtime_small_counted_loops),
        ("lcg_lookahead", lcg_lookahead_optimize_runtime_generic),
        ("peephole", peephole_optimize_runtime_generic),
        ("overwritten_moves", eliminate_overwritten_runtime_moves),
        ("slot_compaction", compact_runtime_generic_slots),
        ("dead_prints", dead_print_elimination),
    ];
    for (name, pass) in passes {
        lowered = pass(lowered);
        let Some(LoweredStmt::RuntimeGeneric { program }) = lowered.first() else {
            panic!("{name} discarded runtime generic lowering");
        };
        for instr in &program.instrs {
            let target = match instr {
                RuntimeInstr::Jump { target }
                | RuntimeInstr::JumpIfZero { target, .. }
                | RuntimeInstr::JumpIfCmpFalse { target, .. }
                | RuntimeInstr::Call { target } => Some(*target),
                _ => None,
            };
            assert!(
                target.is_none_or(|target| target < program.instrs.len()),
                "{name} produced out-of-bounds target {target:?} for {} instructions",
                program.instrs.len()
            );
        }
    }
}

#[test]
fn optimizer_preserves_owned_list_memory_operations_after_guard_cleanup_paths() {
    let parsed = parse_program(
        "fn main() { let mut v: list<u64> = []; v.push(1u64); v[0u64] = 9u64; exit(v[0u64]); }",
    )
    .unwrap();
    let lowered = crate::frontend::semantics::lower_program(&parsed).unwrap();
    let optimized = optimize_semantics_ir(lowered);
    let program = optimized
        .iter()
        .find_map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { program } => Some(program),
            _ => None,
        })
        .expect("runtime program");
    let load_ptr = program
        .instrs
        .iter()
        .find_map(|instr| match instr {
            RuntimeInstr::HeapLoadInt { ptr, .. } => Some(*ptr),
            _ => None,
        })
        .expect("heap load");
    let assigned_ptr = program
        .instrs
        .iter()
        .find_map(|instr| match instr {
            RuntimeInstr::HeapStoreInt {
                ptr,
                src: RuntimeOperand::Imm(9),
                ..
            } => Some(*ptr),
            _ => None,
        })
        .expect("indexed heap store");
    assert!(matches!(
        (assigned_ptr, load_ptr),
        (RuntimeOperand::Slot(left), RuntimeOperand::Slot(right)) if left == right
    ));
}

#[test]
fn overwritten_move_elimination_stops_at_reads_and_control_flow() {
    let program = RuntimeProgram {
        slots: 3,
        instrs: vec![
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(1),
            },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Slot(0),
            },
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(2),
            },
            RuntimeInstr::Mov {
                dst: 2,
                src: RuntimeOperand::Imm(3),
            },
            RuntimeInstr::Mov {
                dst: 2,
                src: RuntimeOperand::Imm(4),
            },
            RuntimeInstr::Jump { target: 7 },
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Imm(9),
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(1),
            },
        ],
    };
    let out = eliminate_overwritten_runtime_moves(vec![LoweredStmt::RuntimeGeneric { program }]);
    let LoweredStmt::RuntimeGeneric { program } = &out[0] else {
        panic!("expected runtime generic program");
    };

    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        RuntimeInstr::Mov {
            dst: 0,
            src: RuntimeOperand::Imm(1)
        }
    )));
    assert!(!program.instrs.iter().any(|instr| matches!(
        instr,
        RuntimeInstr::Mov {
            dst: 2,
            src: RuntimeOperand::Imm(3)
        }
    )));
    assert!(matches!(
        program
            .instrs
            .iter()
            .find(|instr| matches!(instr, RuntimeInstr::Jump { .. })),
        Some(RuntimeInstr::Jump { target: 6 })
    ));
}

#[cfg(any())]
#[test]
fn optimize_keeps_runtime_lcg_for_runtime_measurement() {
    let input = vec![LoweredStmt::RuntimeLcgLoop {
        iterations: 32,
        state_init: 1,
        mul: 1_664_525,
        add: 1_013_904_223,
        exit_with_state: true,
        exit_mask: Some(u64::MAX),
    }];
    let out = optimize_semantics_ir(input);
    assert_eq!(out.len(), 1);
    assert!(matches!(
        out.first(),
        Some(LoweredStmt::RuntimeLcgLoop {
            iterations: 32,
            state_init: 1,
            ..
        })
    ));
}

#[cfg(any())]
#[test]
fn optimize_folds_seeded_runtime_lcg_to_closed_form() {
    let input = vec![LoweredStmt::RuntimeSeededLcgLoop {
        iterations: 32,
        mul: 1_664_525,
        add: 1_013_904_223,
        exit_with_state: true,
        exit_mask: Some(u64::MAX),
    }];
    let out = optimize_semantics_ir(input);
    assert!(matches!(
        out.first(),
        Some(LoweredStmt::RuntimeSeededAffineClosedForm { .. })
    ));
}

#[cfg(any())]
#[test]
fn optimize_keeps_seeded_alloc_runtime_lcg() {
    let input = vec![LoweredStmt::RuntimeSeededLcgAllocLoop {
        iterations: 32,
        mul: 1_664_525,
        add: 1_013_904_223,
        alloc_bytes: 65_536,
        exit_with_state: true,
    }];
    let out = optimize_semantics_ir(input);
    assert!(matches!(
        out.first(),
        Some(LoweredStmt::RuntimeSeededLcgAllocLoop { alloc_bytes, .. }) if *alloc_bytes == 65_536
    ));
}

#[cfg(any())]
#[test]
fn optimize_keeps_seeded_branch_runtime_lcg() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeSeededPredictableBranchLcgLoop {
        iterations: 64,
        then_iterations: 16,
        then_mul: 1_664_525,
        then_add: 1_013_904_223,
        else_mul: 22_695_477,
        else_add: 1,
        exit_with_state: true,
        exit_mask: Some(u64::MAX),
    }]);
    assert!(matches!(
        out.first(),
        Some(LoweredStmt::RuntimeSeededPredictableBranchLcgLoop { iterations, .. }) if *iterations == 64
    ));

    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeSeededUnpredictableBranchLcgLoop {
        iterations: 64,
        threshold: 1 << 63,
        then_mul: 1_664_525,
        then_add: 1_013_904_223,
        else_mul: 22_695_477,
        else_add: 1,
        exit_with_state: true,
        exit_mask: Some(u64::MAX),
    }]);
    assert!(matches!(
        out.first(),
        Some(LoweredStmt::RuntimeSeededUnpredictableBranchLcgLoop { threshold, .. }) if *threshold == (1 << 63)
    ));
}

#[test]
fn optimize_keeps_runtime_generic() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 1,
            instrs: vec![crate::frontend::semantics::RuntimeInstr::Exit {
                code: crate::frontend::semantics::RuntimeOperand::Imm(0),
            }],
        },
    }]);
    assert!(matches!(
        out.first(),
        Some(LoweredStmt::RuntimeGeneric { .. })
    ));
}

#[cfg(any())]
#[test]
fn optimize_folds_seeded_affine_index_to_closed_form() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeSeededAffineIndexLoop {
        iterations: 4,
        index_init: 0,
        state_mul: 2,
        index_mul: 1,
        add: 0,
        state_mask: u64::MAX,
        exit_with_state: true,
        exit_mask: Some(u64::MAX),
    }]);
    assert!(matches!(
        out.first(),
        Some(LoweredStmt::RuntimeSeededAffineClosedForm {
            state_mul,
            add,
            exit_with_state
        }) if *state_mul == 16 && *add == 11 && *exit_with_state
    ));
}

#[cfg(any())]
#[test]
fn optimize_folds_zero_mul_affine_closed_form_to_exit() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeSeededAffineClosedForm {
        state_mul: 0,
        add: 0xDEAD_BEEF,
        exit_with_state: true,
    }]);
    assert!(matches!(
        out.first(),
        Some(LoweredStmt::Exit(code)) if *code == 0xDEAD_BEEF
    ));
}

#[cfg(any())]
#[test]
fn optimize_folds_zero_mul_seeded_lcg_to_exit() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeSeededLcgLoop {
        iterations: 128,
        mul: 2,
        add: 1,
        exit_with_state: true,
        exit_mask: Some(u64::MAX),
    }]);
    assert!(matches!(out.first(), Some(LoweredStmt::Exit(_))));
}

#[cfg(any())]
#[test]
fn optimize_folds_zero_mul_seeded_affine_index_to_exit() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeSeededAffineIndexLoop {
        iterations: 128,
        index_init: 0,
        state_mul: 2,
        index_mul: 1,
        add: 0,
        state_mask: u64::MAX,
        exit_with_state: true,
        exit_mask: Some(u64::MAX),
    }]);
    assert!(matches!(out.first(), Some(LoweredStmt::Exit(_))));
}

#[cfg(any())]
#[test]
fn optimize_keeps_runtime_affine_index_loop() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeAffineIndexLoop {
        iterations: 4,
        state_init: 3,
        index_init: 0,
        state_mul: 2,
        index_mul: 1,
        add: 0,
        state_mask: 63,
        exit_with_state: true,
        exit_mask: Some(63),
    }]);
    assert!(matches!(
        out.first(),
        Some(LoweredStmt::RuntimeAffineIndexLoop {
            iterations: 4,
            state_init: 3,
            ..
        })
    ));
}

#[cfg(any())]
#[test]
fn optimize_preserves_runtime_ring_write_workload() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeRingWriteLoop {
        iterations: 130,
        state_init: 123_456_789,
        index_init: 0,
        mul: 1_664_525,
        add: 1_013_904_223,
        state_mask: 0xFFFF_FFFF,
        ring_mask: 63,
        value_shift: 32,
        exit_mask: 127,
    }]);
    assert!(matches!(
        out.first(),
        Some(LoweredStmt::RuntimeRingWriteLoop {
            iterations: 130,
            ring_mask: 63,
            ..
        })
    ));
}

#[cfg(any())]
#[test]
fn optimize_keeps_dual_state_branch_kernel() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeSeededDualStateBranchLoop {
        iterations: 64,
        index_init: 0,
        adaptive: true,
        branchless: true,
        exit_with_sum: true,
    }]);
    assert!(matches!(
        out.first(),
        Some(LoweredStmt::RuntimeSeededDualStateBranchLoop {
            iterations,
            branchless,
            ..
        }) if *iterations == 64 && *branchless
    ));
}

#[test]
fn optimize_inlines_runtime_generic_leaf_call() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 1,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Call { target: 2 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 0,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(7),
                },
                crate::frontend::semantics::RuntimeInstr::Return,
            ],
        },
    }]);

    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(
        !program
            .instrs
            .iter()
            .any(|instr| matches!(instr, crate::frontend::semantics::RuntimeInstr::Call { .. }))
    );
    assert!(matches!(
        program.instrs.first(),
        Some(crate::frontend::semantics::RuntimeInstr::Mov { .. })
    ));
}

#[test]
fn optimize_elides_runtime_generic_empty_leaf_call() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 0,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Call { target: 2 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::Return,
            ],
        },
    }]);

    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(
        !program
            .instrs
            .iter()
            .any(|instr| matches!(instr, crate::frontend::semantics::RuntimeInstr::Call { .. }))
    );
}

#[test]
fn optimize_runtime_generic_peephole_folds_consts_and_branches() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 3,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 0,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(4),
                },
                crate::frontend::semantics::RuntimeInstr::BinOp {
                    dst: 1,
                    op: crate::frontend::semantics::RuntimeBinOp::Mul,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(0),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(3),
                },
                crate::frontend::semantics::RuntimeInstr::Cmp {
                    dst: 2,
                    op: crate::frontend::semantics::RuntimeCmpOp::Eq,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(1),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(12),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfZero {
                    cond_slot: 2,
                    target: 5,
                },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::Mov {
            src: crate::frontend::semantics::RuntimeOperand::Imm(12),
            ..
        }
    )));
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::JumpIfZero { .. }
    )));
}

#[test]
fn optimize_runtime_generic_elides_index_checks_in_canonical_counted_loop() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 6,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 4,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(4),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(4),
                    target: 6,
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 5,
                    base_slots: vec![0, 1, 2, 3],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(4),
                },
                crate::frontend::semantics::RuntimeInstr::StoreIndex {
                    base_slots: vec![0, 1, 2, 3],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(4),
                    src: crate::frontend::semantics::RuntimeOperand::Slot(5),
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 4,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::Jump { target: 1 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(
        !program.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::LoadIndex { .. }
        )),
        "instrs={:?}",
        program.instrs
    );
    assert!(
        !program.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::StoreIndex { .. }
        )),
        "instrs={:?}",
        program.instrs
    );
}

#[test]
fn optimize_runtime_generic_keeps_checked_index_when_loop_body_has_control_flow() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 8,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 4,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::LoadSeed {
                    dst: 6,
                    kind: RuntimeLoadKind::EntropySeed,
                    input: None,
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(4),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(4),
                    target: 9,
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfZero {
                    cond_slot: 6,
                    target: 5,
                },
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 7,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 5,
                    base_slots: vec![0, 1, 2, 3],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(4),
                },
                crate::frontend::semantics::RuntimeInstr::StoreIndex {
                    base_slots: vec![0, 1, 2, 3],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(4),
                    src: crate::frontend::semantics::RuntimeOperand::Slot(5),
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 4,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::Jump { target: 2 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::LoadIndex { .. }
    )));
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::StoreIndexUnchecked { .. }
    )));
}

#[test]
fn optimize_runtime_generic_hoists_repeated_identical_index_checks_in_block() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 5,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::LoadSeed {
                    dst: 0,
                    kind: RuntimeLoadKind::EntropySeed,
                    input: None,
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 1,
                    base_slots: vec![2, 3, 4],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(0),
                },
                crate::frontend::semantics::RuntimeInstr::StoreIndex {
                    base_slots: vec![2, 3, 4],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(0),
                    src: crate::frontend::semantics::RuntimeOperand::Slot(1),
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 1,
                    base_slots: vec![2, 3, 4],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(0),
                },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Slot(1),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };

    let checked_count = program
        .instrs
        .iter()
        .filter(|instr| {
            matches!(
                instr,
                crate::frontend::semantics::RuntimeInstr::LoadIndex { .. }
                    | crate::frontend::semantics::RuntimeInstr::StoreIndex { .. }
            )
        })
        .count();
    let unchecked_count = program
        .instrs
        .iter()
        .filter(|instr| {
            matches!(
                instr,
                crate::frontend::semantics::RuntimeInstr::LoadIndexUnchecked { .. }
                    | crate::frontend::semantics::RuntimeInstr::StoreIndexUnchecked { .. }
            )
        })
        .count();
    assert_eq!(checked_count, 1, "instrs={:?}", program.instrs);
    assert_eq!(unchecked_count, 2, "instrs={:?}", program.instrs);
}

#[test]
fn optimize_runtime_generic_does_not_hoist_after_index_slot_write() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 5,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::LoadSeed {
                    dst: 0,
                    kind: RuntimeLoadKind::EntropySeed,
                    input: None,
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 1,
                    base_slots: vec![2, 3, 4],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(0),
                },
                crate::frontend::semantics::RuntimeInstr::LoadSeed {
                    dst: 0,
                    kind: RuntimeLoadKind::EntropySeed,
                    input: None,
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 1,
                    base_slots: vec![2, 3, 4],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(0),
                },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Slot(1),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };

    let checked_count = program
        .instrs
        .iter()
        .filter(|instr| {
            matches!(
                instr,
                crate::frontend::semantics::RuntimeInstr::LoadIndex { .. }
                    | crate::frontend::semantics::RuntimeInstr::StoreIndex { .. }
            )
        })
        .count();
    assert_eq!(checked_count, 2, "instrs={:?}", program.instrs);
}

#[test]
fn optimize_runtime_generic_does_not_hoist_across_branch_block_boundary() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 6,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::LoadSeed {
                    dst: 0,
                    kind: RuntimeLoadKind::EntropySeed,
                    input: None,
                },
                crate::frontend::semantics::RuntimeInstr::LoadSeed {
                    dst: 1,
                    kind: RuntimeLoadKind::EntropySeed,
                    input: None,
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfZero {
                    cond_slot: 1,
                    target: 4,
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 2,
                    base_slots: vec![3, 4, 5],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(0),
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 2,
                    base_slots: vec![3, 4, 5],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(0),
                },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Slot(2),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };

    let checked_count = program
        .instrs
        .iter()
        .filter(|instr| {
            matches!(
                instr,
                crate::frontend::semantics::RuntimeInstr::LoadIndex { .. }
                    | crate::frontend::semantics::RuntimeInstr::StoreIndex { .. }
            )
        })
        .count();
    assert_eq!(checked_count, 2, "instrs={:?}", program.instrs);
}

#[test]
fn optimize_runtime_generic_fully_unrolls_small_constant_counted_loop() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 3,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::LoadSeed {
                    dst: 0,
                    kind: RuntimeLoadKind::EntropySeed,
                    input: None,
                },
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 1,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(1),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(4),
                    target: 6,
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: crate::frontend::semantics::RuntimeBinOp::BitXor,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 1,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::Jump { target: 2 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Slot(0),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };

    assert!(
        !program.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                ..
            }
        )),
        "instrs={:?}",
        program.instrs
    );
    assert!(
        !program
            .instrs
            .iter()
            .enumerate()
            .any(|(idx, instr)| matches!(instr, crate::frontend::semantics::RuntimeInstr::Jump { target } if *target <= idx)),
        "instrs={:?}",
        program.instrs
    );
    let xor_count = program
        .instrs
        .iter()
        .filter(|instr| {
            matches!(
                instr,
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: crate::frontend::semantics::RuntimeBinOp::BitXor,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                }
            )
        })
        .count();
    assert_eq!(xor_count, 4, "instrs={:?}", program.instrs);
}

#[test]
fn optimize_runtime_generic_aggressively_unrolls_tiny_32_trip_loop() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 2,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 0,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 1,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(1),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(32),
                    target: 6,
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: crate::frontend::semantics::RuntimeBinOp::BitXor,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 1,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::Jump { target: 2 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Slot(0),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };

    assert!(
        !program.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                ..
            }
        )),
        "instrs={:?}",
        program.instrs
    );
    let ind_slot_moves = program
        .instrs
        .iter()
        .filter(|instr| {
            matches!(
                instr,
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 1,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(_),
                }
            )
        })
        .count();
    assert_eq!(
        ind_slot_moves, 1,
        "overwritten unrolled index states should be eliminated, instrs={:?}",
        program.instrs
    );
    assert!(
        program.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::Mov {
                dst: 1,
                src: crate::frontend::semantics::RuntimeOperand::Imm(32),
            }
        )),
        "missing final unrolled index state, instrs={:?}",
        program.instrs
    );
}

#[test]
fn optimize_runtime_generic_auto_selects_grouped_hash_probe_loop() {
    let ctrl_slots: Vec<usize> = (8..24usize).collect();
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 24,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 0,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 3,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(4),
                },
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 5,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(7),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(0),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(16),
                    target: 10,
                },
                crate::frontend::semantics::RuntimeInstr::BinOp {
                    dst: 1,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(3),
                    rhs: crate::frontend::semantics::RuntimeOperand::Slot(0),
                },
                crate::frontend::semantics::RuntimeInstr::BinOp {
                    dst: 2,
                    op: crate::frontend::semantics::RuntimeBinOp::BitAnd,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(1),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(15),
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 4,
                    base_slots: ctrl_slots,
                    index: crate::frontend::semantics::RuntimeOperand::Slot(2),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::Eq,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(4),
                    rhs: crate::frontend::semantics::RuntimeOperand::Slot(5),
                    target: 8,
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::Jump { target: 3 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(
        program.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::HashCtrlGroupProbe { .. }
        )),
        "instrs={:?}",
        program.instrs
    );
}

#[test]
fn optimize_runtime_generic_auto_selects_grouped_hash_probe_with_zero_check() {
    let ctrl_slots: Vec<usize> = (8..24usize).collect();
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 24,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 0,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 3,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(4),
                },
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 5,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(7),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(0),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(16),
                    target: 12,
                },
                crate::frontend::semantics::RuntimeInstr::BinOp {
                    dst: 1,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(3),
                    rhs: crate::frontend::semantics::RuntimeOperand::Slot(0),
                },
                crate::frontend::semantics::RuntimeInstr::BinOp {
                    dst: 2,
                    op: crate::frontend::semantics::RuntimeBinOp::BitAnd,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(1),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(15),
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 4,
                    base_slots: ctrl_slots,
                    index: crate::frontend::semantics::RuntimeOperand::Slot(2),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::Eq,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(4),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(0),
                    target: 9,
                },
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 6,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::Eq,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(4),
                    rhs: crate::frontend::semantics::RuntimeOperand::Slot(5),
                    target: 10,
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::Jump { target: 3 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::HashCtrlGroupProbe { .. }
    )));
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
            op: crate::frontend::semantics::RuntimeCmpOp::Eq,
            lhs: crate::frontend::semantics::RuntimeOperand::Slot(4),
            rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
            ..
        }
    )));
}

#[test]
fn optimize_runtime_generic_elides_index_checks_for_ring_mask_indices() {
    let base_slots: Vec<usize> = (0..64usize).collect();
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 67,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 64,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(64),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(5_000_000),
                    target: 7,
                },
                crate::frontend::semantics::RuntimeInstr::BinOp {
                    dst: 65,
                    op: crate::frontend::semantics::RuntimeBinOp::BitAnd,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(64),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(63),
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 66,
                    base_slots: base_slots.clone(),
                    index: crate::frontend::semantics::RuntimeOperand::Slot(65),
                },
                crate::frontend::semantics::RuntimeInstr::StoreIndex {
                    base_slots,
                    index: crate::frontend::semantics::RuntimeOperand::Slot(65),
                    src: crate::frontend::semantics::RuntimeOperand::Slot(66),
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 64,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::Jump { target: 1 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(
        !program.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::LoadIndex { .. }
        )),
        "instrs={:?}",
        program.instrs
    );
    assert!(
        !program.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::StoreIndex { .. }
        )),
        "instrs={:?}",
        program.instrs
    );
}

#[test]
fn optimize_runtime_generic_elides_index_checks_for_induction_plus_constant() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 8,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 4,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(4),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(4),
                    target: 7,
                },
                crate::frontend::semantics::RuntimeInstr::BinOp {
                    dst: 5,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(4),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 6,
                    base_slots: vec![0, 1, 2, 3, 7],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(5),
                },
                crate::frontend::semantics::RuntimeInstr::StoreIndex {
                    base_slots: vec![0, 1, 2, 3, 7],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(5),
                    src: crate::frontend::semantics::RuntimeOperand::Slot(6),
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 4,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::Jump { target: 1 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(
        !program.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::LoadIndex { .. }
        )),
        "instrs={:?}",
        program.instrs
    );
    assert!(
        !program.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::StoreIndex { .. }
        )),
        "instrs={:?}",
        program.instrs
    );
}

#[test]
fn optimize_runtime_generic_elides_ring_mask_indices_from_source() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut arr: [u64; 8] = [0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64]; let mut i: u64 = 0u64; while i < 1000u64 { state = state * 1664525u64 + 1013904223u64; let idx: u64 = i & 7u64; arr[idx] = state; i = i + 1u64; } exit(state ^ arr[0u8]); }";
    let program = parse_program(src).expect("parse failed");
    let lowered = crate::frontend::semantics::lower_program(&program).expect("lower failed");
    let optimized = optimize_semantics_ir(lowered);
    let runtime = optimized
        .iter()
        .find_map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { program } => Some(program),
            _ => None,
        })
        .expect("expected runtime generic stmt");
    assert!(
        runtime.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::StoreIndexUnchecked { .. }
        )),
        "instrs={:?}",
        runtime.instrs
    );
}

#[test]
fn optimize_runtime_generic_elides_ring_mask_indices_from_invariant_slot() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut arr: [u64; 64] = [0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64]; let mask: u64 = 63u64; let mut i: u64 = 0u64; while i < 1000u64 { state = state * 1664525u64 + 1013904223u64; let idx: u64 = i & mask; arr[idx] = state; i = i + 1u64; } exit(state ^ arr[0u8]); }";
    let program = parse_program(src).expect("parse failed");
    let lowered = crate::frontend::semantics::lower_program(&program).expect("lower failed");
    let optimized = optimize_semantics_ir(lowered);
    let runtime = optimized
        .iter()
        .find_map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { program } => Some(program),
            _ => None,
        })
        .expect("expected runtime generic stmt");
    assert!(
        runtime.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::StoreIndexUnchecked { .. }
        )),
        "instrs={:?}",
        runtime.instrs
    );
}

#[test]
fn optimize_runtime_generic_elides_masked_dynamic_index_not_from_induction_slot() {
    let src = "fn main() { let mut state: u64 = runtime_seed(); let mut arr: [u64; 8] = [0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64]; let mut i: u64 = 0u64; while i < 100u64 { state = state * 1664525u64 + 1013904223u64; let idx: u64 = state & 7u64; arr[idx] = state; i = i + 1u64; } exit(state); }";
    let program = parse_program(src).expect("parse failed");
    let lowered = crate::frontend::semantics::lower_program(&program).expect("lower failed");
    let optimized = optimize_semantics_ir(lowered);
    let runtime = optimized
        .iter()
        .find_map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { program } => Some(program),
            _ => None,
        })
        .expect("expected runtime generic stmt");
    assert!(
        runtime.instrs.iter().any(|instr| matches!(
            instr,
            crate::frontend::semantics::RuntimeInstr::StoreIndexUnchecked { .. }
        )),
        "instrs={:?}",
        runtime.instrs
    );
}

#[test]
fn optimize_runtime_generic_elides_bitor_composed_bounded_index() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 266,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::LoadSeed {
                    dst: 260,
                    kind: RuntimeLoadKind::EntropySeed,
                    input: None,
                },
                crate::frontend::semantics::RuntimeInstr::BinOp {
                    dst: 261,
                    op: crate::frontend::semantics::RuntimeBinOp::ShrUnsigned,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(260),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(62),
                },
                crate::frontend::semantics::RuntimeInstr::BinOp {
                    dst: 262,
                    op: crate::frontend::semantics::RuntimeBinOp::BitAnd,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(260),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(63),
                },
                crate::frontend::semantics::RuntimeInstr::BinOp {
                    dst: 263,
                    op: crate::frontend::semantics::RuntimeBinOp::Shl,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(261),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(6),
                },
                crate::frontend::semantics::RuntimeInstr::BinOp {
                    dst: 264,
                    op: crate::frontend::semantics::RuntimeBinOp::BitOr,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(263),
                    rhs: crate::frontend::semantics::RuntimeOperand::Slot(262),
                },
                crate::frontend::semantics::RuntimeInstr::StoreIndex {
                    base_slots: (0..256).collect(),
                    index: crate::frontend::semantics::RuntimeOperand::Slot(264),
                    src: crate::frontend::semantics::RuntimeOperand::Slot(260),
                },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::StoreIndexUnchecked { .. }
    )));
}

#[test]
fn optimize_runtime_generic_elides_masked_recurrence_index_across_backedge() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 8,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 0,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 1,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(0),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(100),
                    target: 8,
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 2,
                    base_slots: vec![3, 4, 5, 6],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(1),
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 1,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 1,
                    op: crate::frontend::semantics::RuntimeBinOp::BitAnd,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(3),
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::Jump { target: 2 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Slot(2),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::LoadIndexUnchecked { .. }
    )));
}

#[test]
fn copy_propagate_rewrites_slot_operands() {
    let instrs = vec![
        crate::frontend::semantics::RuntimeInstr::Mov {
            dst: 0,
            src: crate::frontend::semantics::RuntimeOperand::Imm(7),
        },
        crate::frontend::semantics::RuntimeInstr::Mov {
            dst: 1,
            src: crate::frontend::semantics::RuntimeOperand::Slot(0),
        },
        crate::frontend::semantics::RuntimeInstr::BinOp {
            dst: 2,
            op: crate::frontend::semantics::RuntimeBinOp::Add,
            lhs: crate::frontend::semantics::RuntimeOperand::Slot(1),
            rhs: crate::frontend::semantics::RuntimeOperand::Imm(3),
        },
        crate::frontend::semantics::RuntimeInstr::Exit {
            code: crate::frontend::semantics::RuntimeOperand::Slot(2),
        },
    ];
    let out = copy_propagate_runtime_instrs(3, &instrs);
    assert!(matches!(
        out.get(2),
        Some(crate::frontend::semantics::RuntimeInstr::BinOp {
            lhs: crate::frontend::semantics::RuntimeOperand::Slot(0),
            ..
        })
    ));
}

#[test]
fn optimize_runtime_generic_versions_slot_bounded_loop_accesses() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 7,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 5,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(4),
                },
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 4,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(4),
                    rhs: crate::frontend::semantics::RuntimeOperand::Slot(5),
                    target: 6,
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 6,
                    base_slots: vec![0, 1, 2, 3],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(4),
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 4,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::Jump { target: 2 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Slot(6),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::LoadIndexUnchecked { .. }
    )));
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
            op: crate::frontend::semantics::RuntimeCmpOp::LeUnsigned,
            lhs: crate::frontend::semantics::RuntimeOperand::Slot(5),
            rhs: crate::frontend::semantics::RuntimeOperand::Imm(4),
            ..
        }
    )));
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::Exit {
            code: crate::frontend::semantics::RuntimeOperand::Imm(255)
        }
    )));
}

#[test]
fn optimize_runtime_generic_versions_slot_bounded_offset_accesses() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: crate::frontend::semantics::RuntimeProgram {
            slots: 9,
            instrs: vec![
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 5,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(4),
                },
                crate::frontend::semantics::RuntimeInstr::Mov {
                    dst: 4,
                    src: crate::frontend::semantics::RuntimeOperand::Imm(0),
                },
                crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::LtUnsigned,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(4),
                    rhs: crate::frontend::semantics::RuntimeOperand::Slot(5),
                    target: 8,
                },
                crate::frontend::semantics::RuntimeInstr::BinOp {
                    dst: 6,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    lhs: crate::frontend::semantics::RuntimeOperand::Slot(4),
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::LoadIndex {
                    dst: 7,
                    base_slots: vec![0, 1, 2, 3, 8],
                    index: crate::frontend::semantics::RuntimeOperand::Slot(6),
                },
                crate::frontend::semantics::RuntimeInstr::BinOpInPlace {
                    dst: 4,
                    op: crate::frontend::semantics::RuntimeBinOp::Add,
                    rhs: crate::frontend::semantics::RuntimeOperand::Imm(1),
                },
                crate::frontend::semantics::RuntimeInstr::Jump { target: 2 },
                crate::frontend::semantics::RuntimeInstr::Exit {
                    code: crate::frontend::semantics::RuntimeOperand::Slot(7),
                },
            ],
        },
    }]);
    let Some(LoweredStmt::RuntimeGeneric { program }) = out.first() else {
        panic!("expected runtime generic program");
    };
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::LoadIndexUnchecked { .. }
    )));
    assert!(program.instrs.iter().any(|instr| matches!(
        instr,
        crate::frontend::semantics::RuntimeInstr::JumpIfCmpFalse {
            op: crate::frontend::semantics::RuntimeCmpOp::LeUnsigned,
            lhs: crate::frontend::semantics::RuntimeOperand::Slot(5),
            rhs: crate::frontend::semantics::RuntimeOperand::Imm(4),
            ..
        }
    )));
}

#[test]
fn compact_runtime_program_reduces_slot_count() {
    let mut program = crate::frontend::semantics::RuntimeProgram {
        slots: 6,
        instrs: vec![
            crate::frontend::semantics::RuntimeInstr::Mov {
                dst: 2,
                src: crate::frontend::semantics::RuntimeOperand::Imm(9),
            },
            crate::frontend::semantics::RuntimeInstr::Mov {
                dst: 5,
                src: crate::frontend::semantics::RuntimeOperand::Slot(2),
            },
            crate::frontend::semantics::RuntimeInstr::Exit {
                code: crate::frontend::semantics::RuntimeOperand::Slot(5),
            },
        ],
    };
    compact_runtime_program_slots(&mut program);
    assert_eq!(program.slots, 2);
    assert!(matches!(
        program.instrs[0],
        crate::frontend::semantics::RuntimeInstr::Mov {
            dst: 0,
            src: crate::frontend::semantics::RuntimeOperand::Imm(9)
        }
    ));
    assert!(matches!(
        program.instrs[1],
        crate::frontend::semantics::RuntimeInstr::Mov {
            dst: 1,
            src: crate::frontend::semantics::RuntimeOperand::Slot(0)
        }
    ));
    assert!(matches!(
        program.instrs[2],
        crate::frontend::semantics::RuntimeInstr::Exit {
            code: crate::frontend::semantics::RuntimeOperand::Slot(1)
        }
    ));
}

fn runtime_index_access_counts(
    program: &crate::frontend::semantics::RuntimeProgram,
) -> (usize, usize, usize, usize) {
    let mut load_checked = 0usize;
    let mut load_unchecked = 0usize;
    let mut store_checked = 0usize;
    let mut store_unchecked = 0usize;
    for instr in &program.instrs {
        match instr {
            crate::frontend::semantics::RuntimeInstr::LoadIndex { .. } => load_checked += 1,
            crate::frontend::semantics::RuntimeInstr::LoadIndexUnchecked { .. } => {
                load_unchecked += 1;
            }
            crate::frontend::semantics::RuntimeInstr::StoreIndex { .. } => store_checked += 1,
            crate::frontend::semantics::RuntimeInstr::StoreIndexUnchecked { .. } => {
                store_unchecked += 1;
            }
            _ => {}
        }
    }
    (load_checked, load_unchecked, store_checked, store_unchecked)
}

fn checked_index_instrs(program: &crate::frontend::semantics::RuntimeProgram) -> Vec<String> {
    let mut out = Vec::new();
    for (idx, instr) in program.instrs.iter().enumerate() {
        match instr {
            crate::frontend::semantics::RuntimeInstr::LoadIndex {
                base_slots, index, ..
            } => out.push(format!(
                "idx={idx} LoadIndex len={} index={index:?}",
                base_slots.len()
            )),
            crate::frontend::semantics::RuntimeInstr::StoreIndex {
                base_slots, index, ..
            } => out.push(format!(
                "idx={idx} StoreIndex len={} index={index:?}",
                base_slots.len()
            )),
            _ => {}
        }
    }
    out
}

fn optimized_runtime_program_from_file(path: &str) -> crate::frontend::semantics::RuntimeProgram {
    let src = fs::read_to_string(path).expect("read benchmark source");
    let parsed = parse_program(&src).expect("parse benchmark source");
    let lowered =
        crate::frontend::semantics::lower_program(&parsed).expect("lower benchmark source");
    let optimized = optimize_semantics_ir(lowered);
    optimized
        .into_iter()
        .find_map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { program } => Some(program),
            _ => None,
        })
        .expect("expected runtime-generic lowering")
}

#[cfg(any())]
#[test]
fn optimize_preserves_native_bloom_workload() {
    let src = fs::read_to_string("bench/bloom_filter.azk").expect("read benchmark source");
    let parsed = parse_program(&src).expect("parse benchmark source");
    let lowered =
        crate::frontend::semantics::lower_program(&parsed).expect("lower benchmark source");
    let optimized = optimize_semantics_ir(lowered);
    assert!(matches!(
        optimized.first(),
        Some(LoweredStmt::RuntimeBloomFilterLoop {
            build_iterations: 10_000,
            query_iterations: 1_000_000,
            ..
        })
    ));
}

#[test]
fn optimize_runtime_generic_hash_kernel_minimizes_checked_indexing() {
    let program = optimized_runtime_program_from_file("bench/hash_join.azk");
    let (load_checked, load_unchecked, store_checked, store_unchecked) =
        runtime_index_access_counts(&program);
    assert!(
        load_checked + store_checked <= 2,
        "hash kernel has too many checked index ops: load_checked={load_checked} load_unchecked={load_unchecked} store_checked={store_checked} store_unchecked={store_unchecked}"
    );
}

#[test]
fn optimize_runtime_generic_binary_search_proves_midpoint_in_bounds() {
    let program = optimized_runtime_program_from_file("bench/binary_search.azk");
    let (load_checked, load_unchecked, store_checked, store_unchecked) =
        runtime_index_access_counts(&program);
    assert_eq!(
        load_checked, 0,
        "canonical midpoint load should be unchecked"
    );
    assert_eq!(store_checked, 0);
    assert_eq!(load_unchecked, 2);
    assert_eq!(store_unchecked, 0);
    assert!(
        program.instrs.windows(2).any(|pair| {
            matches!(
                (&pair[0], &pair[1]),
                (
                    RuntimeInstr::BinOp {
                        dst: rounded,
                        op: RuntimeBinOp::Add,
                        rhs: RuntimeOperand::Imm(1),
                        ..
                    },
                    RuntimeInstr::BinOp {
                        op: RuntimeBinOp::ShrUnsigned,
                        lhs: RuntimeOperand::Slot(source),
                        rhs: RuntimeOperand::Imm(1),
                        ..
                    }
                ) if rounded == source
            )
        }),
        "affine lower-bound search should use its closed form"
    );
}

#[test]
fn optimize_runtime_generic_folds_constant_join_selection() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: RuntimeProgram {
            slots: 1,
            instrs: vec![
                RuntimeInstr::JoinSelectAdaptive {
                    dst: 0,
                    build_rows: RuntimeOperand::Imm(160),
                    probe_rows: RuntimeOperand::Imm(500_000),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(0),
                },
            ],
        },
    }]);
    let program = match &out[0] {
        LoweredStmt::RuntimeGeneric { program } => program,
        other => panic!("expected runtime generic program, got {other:?}"),
    };
    assert!(matches!(
        program.instrs.as_slice(),
        [
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(1)
            },
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(1)
            }
        ]
    ));
}

#[test]
fn optimize_runtime_generic_unswitches_immutable_strategy_branch() {
    let out = optimize_semantics_ir(vec![LoweredStmt::RuntimeGeneric {
        program: RuntimeProgram {
            slots: 2,
            instrs: vec![
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(1),
                },
                RuntimeInstr::JumpIfCmpFalse {
                    op: crate::frontend::semantics::RuntimeCmpOp::Eq,
                    lhs: RuntimeOperand::Slot(0),
                    rhs: RuntimeOperand::Imm(1),
                    target: 4,
                },
                RuntimeInstr::Mov {
                    dst: 1,
                    src: RuntimeOperand::Imm(7),
                },
                RuntimeInstr::Jump { target: 5 },
                RuntimeInstr::Mov {
                    dst: 1,
                    src: RuntimeOperand::Imm(9),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(1),
                },
            ],
        },
    }]);
    let program = match &out[0] {
        LoweredStmt::RuntimeGeneric { program } => program,
        other => panic!("expected runtime generic program, got {other:?}"),
    };
    assert!(
        !program
            .instrs
            .iter()
            .any(|instr| matches!(instr, RuntimeInstr::JumpIfCmpFalse { .. })),
        "unexpected conditional left after unswitching: {:?}",
        program.instrs
    );
    assert!(
        !program.instrs.iter().any(|instr| matches!(
            instr,
            RuntimeInstr::Mov {
                dst: 1,
                src: RuntimeOperand::Imm(9)
            }
        )),
        "cold branch should be gone after unswitching: {:?}",
        program.instrs
    );
}
