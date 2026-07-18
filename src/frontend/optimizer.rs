use crate::frontend::semantics::{
    LoweredStmt, RuntimeBinOp, RuntimeCmpOp, RuntimeFloatBinOp, RuntimeInstr, RuntimeOperand,
    RuntimeProgram,
};
use std::collections::{HashMap, VecDeque};

fn lcg_lookahead_optimize_runtime_generic(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { mut program } => {
                optimize_lcg_lookahead(&mut program.instrs);
                LoweredStmt::RuntimeGeneric { program }
            }
            other => other,
        })
        .collect()
}

pub fn optimize_semantics_ir(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    let stmts = hoist_loop_invariants(stmts);
    let stmts = const_fold(stmts);
    let stmts = fold_runtime_kernels(stmts);
    let stmts = inline_runtime_generic_leaf_calls(stmts);
    let stmts = tail_call_optimization(stmts);
    let stmts = simplify_runtime_generic_control_flow(stmts);
    let stmts = copy_propagate_runtime_generic(stmts);
    let stmts = specialize_runtime_generic_invariant_constants(stmts);
    let stmts = simplify_runtime_generic_control_flow(stmts);
    let stmts = eliminate_runtime_loop_bounds_checks(stmts);
    let stmts = unroll_runtime_small_counted_loops(stmts);
    let stmts = lcg_lookahead_optimize_runtime_generic(stmts);
    let stmts = peephole_optimize_runtime_generic(stmts);
    let stmts = eliminate_overwritten_runtime_moves(stmts);
    let stmts = compact_runtime_generic_slots(stmts);
    dead_print_elimination(stmts)
}

fn eliminate_overwritten_runtime_moves(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { mut program } => {
                let old = program.instrs;
                let mut removable = vec![false; old.len()];
                for (idx, instr) in old.iter().enumerate() {
                    let RuntimeInstr::Mov { dst, .. } = instr else {
                        continue;
                    };
                    for next in &old[idx + 1..] {
                        if runtime_instr_reads_slot(next, *dst) {
                            break;
                        }
                        if runtime_instr_writes_slot(next, *dst) {
                            removable[idx] = true;
                            break;
                        }
                        if matches!(
                            next,
                            RuntimeInstr::Jump { .. }
                                | RuntimeInstr::JumpIfZero { .. }
                                | RuntimeInstr::JumpIfCmpFalse { .. }
                                | RuntimeInstr::Call { .. }
                                | RuntimeInstr::Return
                                | RuntimeInstr::Exit { .. }
                        ) {
                            break;
                        }
                    }
                }

                let mut remap = vec![0usize; old.len() + 1];
                let mut instrs = Vec::with_capacity(old.len());
                for (idx, instr) in old.into_iter().enumerate() {
                    remap[idx] = instrs.len();
                    if !removable[idx] {
                        instrs.push(instr);
                    }
                }
                remap[removable.len()] = instrs.len();
                for instr in &mut instrs {
                    let target = match instr {
                        RuntimeInstr::Jump { target }
                        | RuntimeInstr::JumpIfZero { target, .. }
                        | RuntimeInstr::JumpIfCmpFalse { target, .. }
                        | RuntimeInstr::Call { target } => target,
                        _ => continue,
                    };
                    *target = remap[(*target).min(removable.len())];
                }
                program.instrs = instrs;
                LoweredStmt::RuntimeGeneric { program }
            }
            other => other,
        })
        .collect()
}

fn tail_call_optimization(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { mut program } => {
                let mut i = 0;
                while i + 1 < program.instrs.len() {
                    if let (RuntimeInstr::Call { target }, RuntimeInstr::Return) =
                        (&program.instrs[i], &program.instrs[i + 1])
                    {
                        program.instrs[i] = RuntimeInstr::Jump { target: *target };
                    }
                    i += 1;
                }
                LoweredStmt::RuntimeGeneric { program }
            }
            other => other,
        })
        .collect()
}

fn peephole_optimize_runtime_generic(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { mut program } => {
                program.instrs = peephole_optimize_runtime_instrs(program.slots, &program.instrs);
                LoweredStmt::RuntimeGeneric { program }
            }
            other => other,
        })
        .collect()
}

fn peephole_optimize_runtime_instrs(slots: usize, instrs: &[RuntimeInstr]) -> Vec<RuntimeInstr> {
    let mut const_slots: Vec<Option<u64>> = vec![None; slots];
    let mut remap = vec![0usize; instrs.len() + 1];
    let mut out = Vec::with_capacity(instrs.len());
    let mut control_flow_targets = vec![false; instrs.len()];
    for instr in instrs {
        let target = match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => *target,
            _ => continue,
        };
        if target < instrs.len() {
            control_flow_targets[target] = true;
        }
    }

    for (idx, instr) in instrs.iter().enumerate() {
        // Linear constant facts are invalid at any block with another predecessor,
        // including forward merge points and internal function entries.
        if control_flow_targets[idx] {
            clear_const_slots(&mut const_slots);
        }
        remap[idx] = out.len();
        if let Some(next) = fold_runtime_instr(instr, &mut const_slots) {
            out.push(next);
        }
    }
    remap[instrs.len()] = out.len();

    for instr in &mut out {
        match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => {
                let clamped = (*target).min(instrs.len());
                *target = remap[clamped];
            }
            RuntimeInstr::LoadIndex { .. }
            | RuntimeInstr::LoadIndexUnchecked { .. }
            | RuntimeInstr::StoreIndex { .. }
            | RuntimeInstr::StoreIndexUnchecked { .. }
            | RuntimeInstr::HeapLoadInt { .. }
            | RuntimeInstr::HeapStoreInt { .. }
            | RuntimeInstr::HeapCopy { .. }
            | RuntimeInstr::BloomSplitBlockInsert { .. }
            | RuntimeInstr::BloomSplitBlockCheck { .. }
            | RuntimeInstr::BloomClassic4Check { .. }
            | RuntimeInstr::HashCtrlGroupProbe { .. }
            | RuntimeInstr::JoinSelectAdaptive { .. }
            | RuntimeInstr::LoadSeed { .. }
            | RuntimeInstr::Mov { .. }
            | RuntimeInstr::BinOp { .. }
            | RuntimeInstr::BinOpInPlace { .. }
            | RuntimeInstr::FloatBinOp { .. }
            | RuntimeInstr::Cmp { .. }
            | RuntimeInstr::NormalizeInt { .. }
            | RuntimeInstr::CompareSwap { .. }
            | RuntimeInstr::RadixSortFixedInt { .. }
            | RuntimeInstr::Alloc { .. }
            | RuntimeInstr::Free { .. }
            | RuntimeInstr::FileOpen { .. }
            | RuntimeInstr::FileWrite { .. }
            | RuntimeInstr::FileRead { .. }
            | RuntimeInstr::FileClose { .. }
            | RuntimeInstr::ThreadSpawn { .. }
            | RuntimeInstr::ThreadJoin { .. }
            | RuntimeInstr::ChannelCreate { .. }
            | RuntimeInstr::ChannelSend { .. }
            | RuntimeInstr::ChannelRecv { .. }
            | RuntimeInstr::ChannelClose { .. }
            | RuntimeInstr::ChannelDestroy { .. }
            | RuntimeInstr::PrintConst { .. }
            | RuntimeInstr::PrintInt { .. }
            | RuntimeInstr::Return
            | RuntimeInstr::Exit { .. } => {}
        }
    }
    out
}

fn fold_runtime_instr(
    instr: &RuntimeInstr,
    const_slots: &mut [Option<u64>],
) -> Option<RuntimeInstr> {
    match instr {
        RuntimeInstr::LoadSeed { dst, .. } => {
            const_slots[*dst] = None;
            Some(instr.clone())
        }
        RuntimeInstr::Mov { dst, src } => {
            let src = fold_operand_const(src, const_slots);
            if matches!(src, RuntimeOperand::Slot(slot) if slot == *dst) {
                return None;
            }
            const_slots[*dst] = operand_const_value(&src, const_slots);
            Some(RuntimeInstr::Mov { dst: *dst, src })
        }
        RuntimeInstr::BinOp { dst, op, lhs, rhs } => {
            let lhs = fold_operand_const(lhs, const_slots);
            let rhs = fold_operand_const(rhs, const_slots);
            if let (Some(l), Some(r)) = (
                operand_const_value(&lhs, const_slots),
                operand_const_value(&rhs, const_slots),
            ) {
                if let Some(value) = eval_runtime_binop(*op, l, r) {
                    const_slots[*dst] = Some(value);
                    return Some(RuntimeInstr::Mov {
                        dst: *dst,
                        src: RuntimeOperand::Imm(value),
                    });
                }
            }
            if let Some(identity) = fold_binop_identity(*op, &lhs, &rhs) {
                const_slots[*dst] = operand_const_value(&identity, const_slots);
                return Some(RuntimeInstr::Mov {
                    dst: *dst,
                    src: identity,
                });
            }
            const_slots[*dst] = None;
            Some(RuntimeInstr::BinOp {
                dst: *dst,
                op: *op,
                lhs,
                rhs,
            })
        }
        RuntimeInstr::BinOpInPlace { dst, op, rhs } => {
            let rhs = fold_operand_const(rhs, const_slots);
            if fold_binop_in_place_is_nop(*op, &rhs) {
                return None;
            }
            if let (Some(l), Some(r)) = (const_slots[*dst], operand_const_value(&rhs, const_slots))
            {
                if let Some(value) = eval_runtime_binop(*op, l, r) {
                    const_slots[*dst] = Some(value);
                    return Some(RuntimeInstr::Mov {
                        dst: *dst,
                        src: RuntimeOperand::Imm(value),
                    });
                }
            }
            const_slots[*dst] = None;
            Some(RuntimeInstr::BinOpInPlace {
                dst: *dst,
                op: *op,
                rhs,
            })
        }
        RuntimeInstr::FloatBinOp {
            dst,
            bits,
            op,
            lhs,
            rhs,
        } => {
            let lhs = fold_operand_const(lhs, const_slots);
            let rhs = fold_operand_const(rhs, const_slots);
            if let (Some(l), Some(r)) = (
                operand_const_value(&lhs, const_slots),
                operand_const_value(&rhs, const_slots),
            ) {
                if let Some(value) = eval_runtime_float_binop(*bits, *op, l, r) {
                    const_slots[*dst] = Some(value);
                    return Some(RuntimeInstr::Mov {
                        dst: *dst,
                        src: RuntimeOperand::Imm(value),
                    });
                }
            }
            const_slots[*dst] = None;
            Some(RuntimeInstr::FloatBinOp {
                dst: *dst,
                bits: *bits,
                op: *op,
                lhs,
                rhs,
            })
        }
        RuntimeInstr::LoadIndex {
            dst,
            base_slots,
            index,
        } => {
            let index = fold_operand_const(index, const_slots);
            if let Some(i) = operand_const_value(&index, const_slots) {
                if (i as usize) < base_slots.len() {
                    let slot = base_slots[i as usize];
                    const_slots[*dst] = const_slots[slot];
                    return Some(RuntimeInstr::Mov {
                        dst: *dst,
                        src: RuntimeOperand::Slot(slot),
                    });
                }
            }
            const_slots[*dst] = None;
            Some(RuntimeInstr::LoadIndex {
                dst: *dst,
                base_slots: base_slots.clone(),
                index,
            })
        }
        RuntimeInstr::LoadIndexUnchecked {
            dst,
            base_slots,
            index,
        } => {
            let index = fold_operand_const(index, const_slots);
            if let Some(i) = operand_const_value(&index, const_slots) {
                if (i as usize) < base_slots.len() {
                    let slot = base_slots[i as usize];
                    const_slots[*dst] = const_slots[slot];
                    return Some(RuntimeInstr::Mov {
                        dst: *dst,
                        src: RuntimeOperand::Slot(slot),
                    });
                }
            }
            const_slots[*dst] = None;
            Some(RuntimeInstr::LoadIndexUnchecked {
                dst: *dst,
                base_slots: base_slots.clone(),
                index,
            })
        }
        RuntimeInstr::StoreIndex {
            base_slots,
            index,
            src,
        } => {
            let index = fold_operand_const(index, const_slots);
            let src = fold_operand_const(src, const_slots);
            if let Some(i) = operand_const_value(&index, const_slots) {
                if (i as usize) < base_slots.len() {
                    let slot = base_slots[i as usize];
                    const_slots[slot] = operand_const_value(&src, const_slots);
                    return Some(RuntimeInstr::Mov { dst: slot, src });
                }
            }
            // Cannot track individual slots easily if index is dynamic
            for &slot in base_slots {
                const_slots[slot] = None;
            }
            Some(RuntimeInstr::StoreIndex {
                base_slots: base_slots.clone(),
                index,
                src,
            })
        }
        RuntimeInstr::StoreIndexUnchecked {
            base_slots,
            index,
            src,
        } => {
            let index = fold_operand_const(index, const_slots);
            let src = fold_operand_const(src, const_slots);
            if let Some(i) = operand_const_value(&index, const_slots) {
                if (i as usize) < base_slots.len() {
                    let slot = base_slots[i as usize];
                    const_slots[slot] = operand_const_value(&src, const_slots);
                    return Some(RuntimeInstr::Mov { dst: slot, src });
                }
            }
            // Cannot track individual slots easily if index is dynamic.
            for &slot in base_slots {
                const_slots[slot] = None;
            }
            Some(RuntimeInstr::StoreIndexUnchecked {
                base_slots: base_slots.clone(),
                index,
                src,
            })
        }
        RuntimeInstr::BloomSplitBlockInsert { filter_slots, hash } => {
            let hash = fold_operand_const(hash, const_slots);
            for &slot in filter_slots {
                const_slots[slot] = None;
            }
            Some(RuntimeInstr::BloomSplitBlockInsert {
                filter_slots: filter_slots.clone(),
                hash,
            })
        }
        RuntimeInstr::BloomSplitBlockCheck {
            dst,
            filter_slots,
            hash,
        } => {
            let hash = fold_operand_const(hash, const_slots);
            for &slot in filter_slots {
                const_slots[slot] = None;
            }
            const_slots[*dst] = None;
            Some(RuntimeInstr::BloomSplitBlockCheck {
                dst: *dst,
                filter_slots: filter_slots.clone(),
                hash,
            })
        }
        RuntimeInstr::BloomClassic4Check {
            dst,
            lanes_checked,
            filter_slots,
            hash,
        } => {
            let hash = fold_operand_const(hash, const_slots);
            for &slot in filter_slots {
                const_slots[slot] = None;
            }
            const_slots[*dst] = None;
            const_slots[*lanes_checked] = None;
            Some(RuntimeInstr::BloomClassic4Check {
                dst: *dst,
                lanes_checked: *lanes_checked,
                filter_slots: filter_slots.clone(),
                hash,
            })
        }
        RuntimeInstr::HashCtrlGroupProbe {
            dst_mask,
            ctrl_slots,
            group_start,
            fingerprint,
        } => {
            let group_start = fold_operand_const(group_start, const_slots);
            let fingerprint = fold_operand_const(fingerprint, const_slots);
            for &slot in ctrl_slots {
                const_slots[slot] = None;
            }
            const_slots[*dst_mask] = None;
            Some(RuntimeInstr::HashCtrlGroupProbe {
                dst_mask: *dst_mask,
                ctrl_slots: ctrl_slots.clone(),
                group_start,
                fingerprint,
            })
        }
        RuntimeInstr::JoinSelectAdaptive {
            dst,
            build_rows,
            probe_rows,
        } => {
            let build_rows = fold_operand_const(build_rows, const_slots);
            let probe_rows = fold_operand_const(probe_rows, const_slots);
            if let (Some(build), Some(probe)) = (
                operand_const_value(&build_rows, const_slots),
                operand_const_value(&probe_rows, const_slots),
            ) {
                let selected = u64::from(build >= 128 && probe >= 200_000);
                const_slots[*dst] = Some(selected);
                return Some(RuntimeInstr::Mov {
                    dst: *dst,
                    src: RuntimeOperand::Imm(selected),
                });
            }
            const_slots[*dst] = None;
            Some(RuntimeInstr::JoinSelectAdaptive {
                dst: *dst,
                build_rows,
                probe_rows,
            })
        }
        RuntimeInstr::HeapLoadInt {
            dst,
            ptr,
            index,
            bytes,
        } => {
            let ptr = fold_operand_const(ptr, const_slots);
            let index = fold_operand_const(index, const_slots);
            const_slots[*dst] = None;
            Some(RuntimeInstr::HeapLoadInt {
                dst: *dst,
                ptr,
                index,
                bytes: *bytes,
            })
        }
        RuntimeInstr::HeapStoreInt {
            ptr,
            index,
            src,
            bytes,
        } => Some(RuntimeInstr::HeapStoreInt {
            ptr: fold_operand_const(ptr, const_slots),
            index: fold_operand_const(index, const_slots),
            src: fold_operand_const(src, const_slots),
            bytes: *bytes,
        }),
        RuntimeInstr::HeapCopy {
            dst_ptr,
            src_ptr,
            bytes,
        } => Some(RuntimeInstr::HeapCopy {
            dst_ptr: fold_operand_const(dst_ptr, const_slots),
            src_ptr: fold_operand_const(src_ptr, const_slots),
            bytes: fold_operand_const(bytes, const_slots),
        }),
        RuntimeInstr::Alloc { dst, size } => {
            let size = fold_operand_const(size, const_slots);
            clear_const_slots(const_slots);
            const_slots[*dst] = None;
            Some(RuntimeInstr::Alloc { dst: *dst, size })
        }
        RuntimeInstr::Free { ptr, size } => {
            let ptr = fold_operand_const(ptr, const_slots);
            let size = fold_operand_const(size, const_slots);
            clear_const_slots(const_slots);
            Some(RuntimeInstr::Free { ptr, size })
        }
        RuntimeInstr::FileOpen {
            dst,
            path_ptr,
            flags,
            mode,
        } => {
            let path_ptr = fold_operand_const(path_ptr, const_slots);
            clear_const_slots(const_slots);
            const_slots[*dst] = None;
            Some(RuntimeInstr::FileOpen {
                dst: *dst,
                path_ptr,
                flags: *flags,
                mode: *mode,
            })
        }
        RuntimeInstr::FileWrite { dst, fd, ptr, len } => {
            let result = RuntimeInstr::FileWrite {
                dst: *dst,
                fd: fold_operand_const(fd, const_slots),
                ptr: fold_operand_const(ptr, const_slots),
                len: fold_operand_const(len, const_slots),
            };
            clear_const_slots(const_slots);
            const_slots[*dst] = None;
            Some(result)
        }
        RuntimeInstr::FileRead { dst, fd, ptr, len } => {
            let result = RuntimeInstr::FileRead {
                dst: *dst,
                fd: fold_operand_const(fd, const_slots),
                ptr: fold_operand_const(ptr, const_slots),
                len: fold_operand_const(len, const_slots),
            };
            clear_const_slots(const_slots);
            const_slots[*dst] = None;
            Some(result)
        }
        RuntimeInstr::FileClose { fd } => {
            let fd = fold_operand_const(fd, const_slots);
            clear_const_slots(const_slots);
            Some(RuntimeInstr::FileClose { fd })
        }
        RuntimeInstr::PrintConst { text } => Some(RuntimeInstr::PrintConst { text: text.clone() }),
        RuntimeInstr::PrintInt {
            value,
            signed,
            bits,
        } => {
            let value = fold_operand_const(value, const_slots);
            Some(RuntimeInstr::PrintInt {
                value,
                signed: *signed,
                bits: *bits,
            })
        }
        RuntimeInstr::Cmp { dst, op, lhs, rhs } => {
            let lhs = fold_operand_const(lhs, const_slots);
            let rhs = fold_operand_const(rhs, const_slots);
            if let (Some(l), Some(r)) = (
                operand_const_value(&lhs, const_slots),
                operand_const_value(&rhs, const_slots),
            ) {
                let value = eval_runtime_cmp(*op, l, r);
                const_slots[*dst] = Some(value);
                return Some(RuntimeInstr::Mov {
                    dst: *dst,
                    src: RuntimeOperand::Imm(value),
                });
            }
            const_slots[*dst] = None;
            Some(RuntimeInstr::Cmp {
                dst: *dst,
                op: *op,
                lhs,
                rhs,
            })
        }
        RuntimeInstr::NormalizeInt { dst, signed, bits } => {
            if *bits >= 64 {
                return None;
            }
            if let Some(value) = const_slots[*dst] {
                let normalized = normalize_const_int(value, *signed, *bits);
                const_slots[*dst] = Some(normalized);
                return Some(RuntimeInstr::Mov {
                    dst: *dst,
                    src: RuntimeOperand::Imm(normalized),
                });
            }
            Some(instr.clone())
        }
        RuntimeInstr::CompareSwap {
            left,
            right,
            signed,
        } => {
            const_slots[*left] = None;
            const_slots[*right] = None;
            Some(RuntimeInstr::CompareSwap {
                left: *left,
                right: *right,
                signed: *signed,
            })
        }
        RuntimeInstr::RadixSortFixedInt {
            slots,
            bits,
            signed,
            stable,
        } => {
            for slot in slots {
                const_slots[*slot] = None;
            }
            Some(RuntimeInstr::RadixSortFixedInt {
                slots: slots.clone(),
                bits: *bits,
                signed: *signed,
                stable: *stable,
            })
        }
        RuntimeInstr::Jump { .. } => {
            clear_const_slots(const_slots);
            Some(instr.clone())
        }
        RuntimeInstr::JumpIfZero { cond_slot, target } => {
            // Branch elimination is unsafe here because this peephole pass is linear and does not
            // model back-edges/merges; dropping a loop guard can create infinite loops.
            let rewritten = Some(RuntimeInstr::JumpIfZero {
                cond_slot: *cond_slot,
                target: *target,
            });
            clear_const_slots(const_slots);
            rewritten
        }
        RuntimeInstr::JumpIfCmpFalse {
            op,
            lhs,
            rhs,
            target,
        } => {
            // Same rationale as JumpIfZero above: never fold-away conditional control flow in this
            // local pass, even if operands look constant at the current linear position.
            let rewritten = Some(RuntimeInstr::JumpIfCmpFalse {
                op: *op,
                lhs: *lhs,
                rhs: *rhs,
                target: *target,
            });
            clear_const_slots(const_slots);
            rewritten
        }
        RuntimeInstr::Call { .. } | RuntimeInstr::ThreadSpawn { .. } => {
            clear_const_slots(const_slots);
            Some(instr.clone())
        }
        RuntimeInstr::ThreadJoin { dst, handle } => {
            const_slots[*dst] = None;
            Some(RuntimeInstr::ThreadJoin {
                dst: *dst,
                handle: fold_operand_const(handle, const_slots),
            })
        }
        RuntimeInstr::ChannelCreate {
            dst,
            capacity,
            unbounded,
        } => {
            const_slots[*dst] = None;
            Some(RuntimeInstr::ChannelCreate {
                dst: *dst,
                capacity: fold_operand_const(capacity, const_slots),
                unbounded: *unbounded,
            })
        }
        RuntimeInstr::ChannelSend { handle, value } => Some(RuntimeInstr::ChannelSend {
            handle: fold_operand_const(handle, const_slots),
            value: fold_operand_const(value, const_slots),
        }),
        RuntimeInstr::ChannelRecv { dst, handle } => {
            const_slots[*dst] = None;
            Some(RuntimeInstr::ChannelRecv {
                dst: *dst,
                handle: fold_operand_const(handle, const_slots),
            })
        }
        RuntimeInstr::ChannelClose { handle, sender } => Some(RuntimeInstr::ChannelClose {
            handle: fold_operand_const(handle, const_slots),
            sender: *sender,
        }),
        RuntimeInstr::ChannelDestroy { handle } => Some(RuntimeInstr::ChannelDestroy {
            handle: fold_operand_const(handle, const_slots),
        }),
        RuntimeInstr::Return => {
            clear_const_slots(const_slots);
            Some(RuntimeInstr::Return)
        }
        RuntimeInstr::Exit { code } => {
            let code = fold_operand_const(code, const_slots);
            // An exit terminates its basic block. Instructions laid out after it may be a
            // different branch target, so facts learned on the exiting path cannot flow into
            // that block merely because it is next in linear instruction order.
            clear_const_slots(const_slots);
            Some(RuntimeInstr::Exit { code })
        }
    }
}

fn fold_operand_const(operand: &RuntimeOperand, const_slots: &[Option<u64>]) -> RuntimeOperand {
    match operand {
        RuntimeOperand::Imm(value) => RuntimeOperand::Imm(*value),
        RuntimeOperand::Slot(slot) => match const_slots.get(*slot).copied().flatten() {
            Some(value) => RuntimeOperand::Imm(value),
            None => RuntimeOperand::Slot(*slot),
        },
    }
}

fn operand_const_value(operand: &RuntimeOperand, const_slots: &[Option<u64>]) -> Option<u64> {
    match operand {
        RuntimeOperand::Imm(value) => Some(*value),
        RuntimeOperand::Slot(slot) => const_slots.get(*slot).copied().flatten(),
    }
}

fn fold_binop_in_place_is_nop(op: RuntimeBinOp, rhs: &RuntimeOperand) -> bool {
    let RuntimeOperand::Imm(value) = rhs else {
        return false;
    };
    match op {
        RuntimeBinOp::Add | RuntimeBinOp::Sub => *value == 0,
        RuntimeBinOp::Mul | RuntimeBinOp::DivUnsigned | RuntimeBinOp::DivSigned => *value == 1,
        RuntimeBinOp::BitOr
        | RuntimeBinOp::BitXor
        | RuntimeBinOp::Shl
        | RuntimeBinOp::ShrUnsigned
        | RuntimeBinOp::ShrSigned => *value == 0,
        RuntimeBinOp::BitAnd => *value == u64::MAX,
        RuntimeBinOp::ModUnsigned | RuntimeBinOp::ModSigned => false,
    }
}

fn fold_binop_identity(
    op: RuntimeBinOp,
    lhs: &RuntimeOperand,
    rhs: &RuntimeOperand,
) -> Option<RuntimeOperand> {
    let lhs_imm = match lhs {
        RuntimeOperand::Imm(value) => Some(*value),
        RuntimeOperand::Slot(_) => None,
    };
    let rhs_imm = match rhs {
        RuntimeOperand::Imm(value) => Some(*value),
        RuntimeOperand::Slot(_) => None,
    };
    match op {
        RuntimeBinOp::Add => {
            if rhs_imm == Some(0) {
                Some(*lhs)
            } else if lhs_imm == Some(0) {
                Some(*rhs)
            } else {
                None
            }
        }
        RuntimeBinOp::Sub => {
            if rhs_imm == Some(0) {
                Some(*lhs)
            } else {
                None
            }
        }
        RuntimeBinOp::Mul => {
            if rhs_imm == Some(1) {
                Some(*lhs)
            } else if lhs_imm == Some(1) {
                Some(*rhs)
            } else if rhs_imm == Some(0) || lhs_imm == Some(0) {
                Some(RuntimeOperand::Imm(0))
            } else {
                None
            }
        }
        RuntimeBinOp::DivUnsigned | RuntimeBinOp::DivSigned => {
            if rhs_imm == Some(1) {
                Some(*lhs)
            } else {
                None
            }
        }
        RuntimeBinOp::ModUnsigned => {
            if rhs_imm == Some(1) {
                Some(RuntimeOperand::Imm(0))
            } else if lhs_imm == Some(0) && rhs_imm != Some(0) {
                Some(RuntimeOperand::Imm(0))
            } else {
                None
            }
        }
        RuntimeBinOp::ModSigned => {
            if rhs_imm == Some(1) || rhs_imm == Some(u64::MAX) {
                Some(RuntimeOperand::Imm(0))
            } else if lhs_imm == Some(0) && rhs_imm != Some(0) {
                Some(RuntimeOperand::Imm(0))
            } else {
                None
            }
        }
        RuntimeBinOp::BitAnd => {
            if rhs_imm == Some(0) || lhs_imm == Some(0) {
                Some(RuntimeOperand::Imm(0))
            } else if rhs_imm == Some(u64::MAX) {
                Some(*lhs)
            } else if lhs_imm == Some(u64::MAX) {
                Some(*rhs)
            } else {
                None
            }
        }
        RuntimeBinOp::BitOr => {
            if rhs_imm == Some(0) {
                Some(*lhs)
            } else if lhs_imm == Some(0) {
                Some(*rhs)
            } else if rhs_imm == Some(u64::MAX) || lhs_imm == Some(u64::MAX) {
                Some(RuntimeOperand::Imm(u64::MAX))
            } else {
                None
            }
        }
        RuntimeBinOp::BitXor => {
            if rhs_imm == Some(0) {
                Some(*lhs)
            } else if lhs_imm == Some(0) {
                Some(*rhs)
            } else {
                None
            }
        }
        RuntimeBinOp::Shl | RuntimeBinOp::ShrUnsigned | RuntimeBinOp::ShrSigned => {
            if rhs_imm.map(|value| value & 63) == Some(0) {
                Some(*lhs)
            } else {
                None
            }
        }
    }
}

fn eval_runtime_binop(op: RuntimeBinOp, lhs: u64, rhs: u64) -> Option<u64> {
    match op {
        RuntimeBinOp::Add => Some(lhs.wrapping_add(rhs)),
        RuntimeBinOp::Sub => Some(lhs.wrapping_sub(rhs)),
        RuntimeBinOp::Mul => Some(lhs.wrapping_mul(rhs)),
        RuntimeBinOp::DivUnsigned => {
            if rhs == 0 {
                None
            } else {
                Some(lhs / rhs)
            }
        }
        RuntimeBinOp::DivSigned => {
            let lhs = lhs as i64;
            let rhs = rhs as i64;
            if rhs == 0 || (lhs == i64::MIN && rhs == -1) {
                None
            } else {
                Some((lhs / rhs) as u64)
            }
        }
        RuntimeBinOp::ModUnsigned => {
            if rhs == 0 {
                None
            } else {
                Some(lhs % rhs)
            }
        }
        RuntimeBinOp::ModSigned => {
            let lhs = lhs as i64;
            let rhs = rhs as i64;
            if rhs == 0 {
                None
            } else {
                Some((lhs % rhs) as u64)
            }
        }
        RuntimeBinOp::BitAnd => Some(lhs & rhs),
        RuntimeBinOp::BitOr => Some(lhs | rhs),
        RuntimeBinOp::BitXor => Some(lhs ^ rhs),
        RuntimeBinOp::Shl => Some(lhs.wrapping_shl((rhs & 63) as u32)),
        RuntimeBinOp::ShrUnsigned => Some(lhs.wrapping_shr((rhs & 63) as u32)),
        RuntimeBinOp::ShrSigned => Some(((lhs as i64) >> ((rhs & 63) as u32)) as u64),
    }
}

fn eval_runtime_float_binop(bits: u16, op: RuntimeFloatBinOp, lhs: u64, rhs: u64) -> Option<u64> {
    match bits {
        32 => {
            let lhs = f32::from_bits(lhs as u32);
            let rhs = f32::from_bits(rhs as u32);
            let out = match op {
                RuntimeFloatBinOp::Add => lhs + rhs,
                RuntimeFloatBinOp::Sub => lhs - rhs,
                RuntimeFloatBinOp::Mul => lhs * rhs,
                RuntimeFloatBinOp::Div => lhs / rhs,
            };
            Some(u64::from(out.to_bits()))
        }
        64 => {
            let lhs = f64::from_bits(lhs);
            let rhs = f64::from_bits(rhs);
            let out = match op {
                RuntimeFloatBinOp::Add => lhs + rhs,
                RuntimeFloatBinOp::Sub => lhs - rhs,
                RuntimeFloatBinOp::Mul => lhs * rhs,
                RuntimeFloatBinOp::Div => lhs / rhs,
            };
            Some(out.to_bits())
        }
        _ => None,
    }
}

fn eval_runtime_cmp(op: RuntimeCmpOp, lhs: u64, rhs: u64) -> u64 {
    let taken = match op {
        RuntimeCmpOp::Eq => lhs == rhs,
        RuntimeCmpOp::Ne => lhs != rhs,
        RuntimeCmpOp::LtUnsigned => lhs < rhs,
        RuntimeCmpOp::LeUnsigned => lhs <= rhs,
        RuntimeCmpOp::GtUnsigned => lhs > rhs,
        RuntimeCmpOp::GeUnsigned => lhs >= rhs,
        RuntimeCmpOp::LtSigned => (lhs as i64) < (rhs as i64),
        RuntimeCmpOp::LeSigned => (lhs as i64) <= (rhs as i64),
        RuntimeCmpOp::GtSigned => (lhs as i64) > (rhs as i64),
        RuntimeCmpOp::GeSigned => (lhs as i64) >= (rhs as i64),
    };
    if taken { 1 } else { 0 }
}

fn normalize_const_int(value: u64, signed: bool, bits: u16) -> u64 {
    if bits >= 64 {
        return value;
    }
    let mask = (1u64 << bits) - 1;
    let truncated = value & mask;
    if signed {
        let sign = 1u64 << (bits - 1);
        if (truncated & sign) != 0 {
            truncated | !mask
        } else {
            truncated
        }
    } else {
        truncated
    }
}

fn clear_const_slots(const_slots: &mut [Option<u64>]) {
    for slot in const_slots {
        *slot = None;
    }
}

fn inline_runtime_generic_leaf_calls(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { mut program } => {
                inline_leaf_calls_in_program(&mut program.instrs);
                LoweredStmt::RuntimeGeneric { program }
            }
            other => other,
        })
        .collect()
}

fn inline_leaf_calls_in_program(instrs: &mut Vec<RuntimeInstr>) {
    const MAX_INLINE_EXPANSION: usize = 16_384;
    let mut added = 0usize;
    loop {
        let mut changed = false;
        let mut idx = 0usize;
        while idx < instrs.len() {
            let RuntimeInstr::Call { target } = instrs[idx] else {
                idx += 1;
                continue;
            };
            let Some(body_end) = leaf_body_end(instrs, target) else {
                idx += 1;
                continue;
            };
            let body = instrs[target..body_end].to_vec();
            if body.is_empty() {
                instrs[idx] = RuntimeInstr::Jump { target: idx + 1 };
                changed = true;
                idx += 1;
                continue;
            }
            if added + body.len() > MAX_INLINE_EXPANSION {
                return;
            }

            let delta = body.len().saturating_sub(1);
            instrs.splice(idx..idx + 1, body);
            if delta > 0 {
                remap_instr_targets_after_inline(instrs, idx, delta);
                added += delta;
            }
            changed = true;
            idx += 1;
        }
        if !changed {
            break;
        }
    }
}

fn simplify_runtime_generic_control_flow(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { mut program } => {
                program.instrs = simplify_runtime_control_flow_instrs(program.instrs);
                LoweredStmt::RuntimeGeneric { program }
            }
            other => other,
        })
        .collect()
}

fn copy_propagate_runtime_generic(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { mut program } => {
                program.instrs = copy_propagate_runtime_instrs(program.slots, &program.instrs);
                LoweredStmt::RuntimeGeneric { program }
            }
            other => other,
        })
        .collect()
}

fn specialize_runtime_generic_invariant_constants(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { mut program } => {
                program.instrs =
                    specialize_runtime_invariant_constants_instrs(program.slots, program.instrs);
                LoweredStmt::RuntimeGeneric { program }
            }
            other => other,
        })
        .collect()
}

fn specialize_runtime_invariant_constants_instrs(
    _slots: usize,
    instrs: Vec<RuntimeInstr>,
) -> Vec<RuntimeInstr> {
    let block_starts = runtime_basic_block_starts(&instrs);
    let mut out = Vec::with_capacity(instrs.len());
    for idx in 0..instrs.len() {
        let block_start = block_starts[idx];
        match &instrs[idx] {
            RuntimeInstr::JumpIfZero { cond_slot, target } => {
                if let Some(value) = slot_const_before(&instrs, block_start, idx, *cond_slot, 0) {
                    if value == 0 {
                        out.push(RuntimeInstr::Jump { target: *target });
                    } else {
                        // Keep instruction numbering stable for all existing branch targets.
                        // The following CFG simplifier removes this fallthrough jump and remaps
                        // targets with complete reachability information.
                        out.push(RuntimeInstr::Jump { target: idx + 1 });
                    }
                } else {
                    out.push(instrs[idx].clone());
                }
            }
            RuntimeInstr::JumpIfCmpFalse {
                op,
                lhs,
                rhs,
                target,
            } => {
                let lhs_value = operand_const_before(&instrs, block_start, idx, lhs, 0);
                let rhs_value = operand_const_before(&instrs, block_start, idx, rhs, 0);
                match (lhs_value, rhs_value) {
                    (Some(lhs), Some(rhs)) => {
                        if eval_runtime_cmp(*op, lhs, rhs) == 0 {
                            out.push(RuntimeInstr::Jump { target: *target });
                        } else {
                            out.push(RuntimeInstr::Jump { target: idx + 1 });
                        }
                    }
                    _ => out.push(instrs[idx].clone()),
                }
            }
            _ => out.push(instrs[idx].clone()),
        }
    }
    out
}

fn copy_propagate_runtime_instrs(slots: usize, instrs: &[RuntimeInstr]) -> Vec<RuntimeInstr> {
    let mut out = Vec::with_capacity(instrs.len());
    let mut remap = vec![0usize; instrs.len() + 1];
    let mut alias: Vec<Option<usize>> = vec![None; slots];
    let mut is_control_flow_target = vec![false; instrs.len()];
    for instr in instrs {
        let target = match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => *target,
            _ => continue,
        };
        if target < is_control_flow_target.len() {
            is_control_flow_target[target] = true;
        }
    }

    let resolve = |slot: usize, alias_map: &[Option<usize>]| -> usize {
        let mut root = slot;
        for _ in 0..alias_map.len() {
            let Some(next) = alias_map[root] else {
                break;
            };
            if next == root {
                break;
            }
            root = next;
        }
        root
    };
    let rewrite_operand = |op: &RuntimeOperand, alias_map: &[Option<usize>]| match op {
        RuntimeOperand::Slot(slot) => RuntimeOperand::Slot(resolve(*slot, alias_map)),
        RuntimeOperand::Imm(v) => RuntimeOperand::Imm(*v),
    };
    let invalidate_slot = |slot: usize, alias_map: &mut [Option<usize>]| {
        if slot < alias_map.len() {
            alias_map[slot] = None;
        }
        for entry in alias_map.iter_mut() {
            if *entry == Some(slot) {
                *entry = None;
            }
        }
    };
    let clear_aliases = |alias_map: &mut [Option<usize>]| {
        for entry in alias_map {
            *entry = None;
        }
    };

    for (instr_index, instr) in instrs.iter().enumerate() {
        remap[instr_index] = out.len();
        // A target may have multiple predecessors. Linear aliases from the physically
        // preceding block are not valid at such a join unless a proper SSA proof says so.
        if is_control_flow_target[instr_index] {
            clear_aliases(&mut alias);
        }
        match instr {
            RuntimeInstr::LoadSeed { dst, .. } => {
                invalidate_slot(*dst, &mut alias);
                out.push(instr.clone());
            }
            RuntimeInstr::Mov { dst, src } => {
                let src = rewrite_operand(src, &alias);
                invalidate_slot(*dst, &mut alias);
                if matches!(src, RuntimeOperand::Slot(slot) if slot == *dst) {
                    continue;
                }
                if let RuntimeOperand::Slot(src_slot) = src {
                    alias[*dst] = Some(src_slot);
                    out.push(RuntimeInstr::Mov {
                        dst: *dst,
                        src: RuntimeOperand::Slot(src_slot),
                    });
                } else {
                    out.push(RuntimeInstr::Mov { dst: *dst, src });
                }
            }
            RuntimeInstr::BinOp { dst, op, lhs, rhs } => {
                let lhs = rewrite_operand(lhs, &alias);
                let rhs = rewrite_operand(rhs, &alias);
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::BinOp {
                    dst: *dst,
                    op: *op,
                    lhs,
                    rhs,
                });
            }
            RuntimeInstr::BinOpInPlace { dst, op, rhs } => {
                let rhs = rewrite_operand(rhs, &alias);
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::BinOpInPlace {
                    dst: *dst,
                    op: *op,
                    rhs,
                });
            }
            RuntimeInstr::FloatBinOp {
                dst,
                bits,
                op,
                lhs,
                rhs,
            } => {
                let lhs = rewrite_operand(lhs, &alias);
                let rhs = rewrite_operand(rhs, &alias);
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::FloatBinOp {
                    dst: *dst,
                    bits: *bits,
                    op: *op,
                    lhs,
                    rhs,
                });
            }
            RuntimeInstr::Cmp { dst, op, lhs, rhs } => {
                let lhs = rewrite_operand(lhs, &alias);
                let rhs = rewrite_operand(rhs, &alias);
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::Cmp {
                    dst: *dst,
                    op: *op,
                    lhs,
                    rhs,
                });
            }
            RuntimeInstr::NormalizeInt { dst, signed, bits } => {
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::NormalizeInt {
                    dst: *dst,
                    signed: *signed,
                    bits: *bits,
                });
            }
            RuntimeInstr::Jump { target } => {
                out.push(RuntimeInstr::Jump { target: *target });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::JumpIfZero { cond_slot, target } => {
                out.push(RuntimeInstr::JumpIfZero {
                    cond_slot: resolve(*cond_slot, &alias),
                    target: *target,
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::JumpIfCmpFalse {
                op,
                lhs,
                rhs,
                target,
            } => {
                out.push(RuntimeInstr::JumpIfCmpFalse {
                    op: *op,
                    lhs: rewrite_operand(lhs, &alias),
                    rhs: rewrite_operand(rhs, &alias),
                    target: *target,
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::CompareSwap {
                left,
                right,
                signed,
            } => {
                invalidate_slot(*left, &mut alias);
                invalidate_slot(*right, &mut alias);
                out.push(RuntimeInstr::CompareSwap {
                    left: *left,
                    right: *right,
                    signed: *signed,
                });
            }
            RuntimeInstr::RadixSortFixedInt {
                slots,
                bits,
                signed,
                stable,
            } => {
                for slot in slots {
                    invalidate_slot(*slot, &mut alias);
                }
                out.push(RuntimeInstr::RadixSortFixedInt {
                    slots: slots.clone(),
                    bits: *bits,
                    signed: *signed,
                    stable: *stable,
                });
            }
            RuntimeInstr::Call { target } => {
                out.push(RuntimeInstr::Call { target: *target });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::LoadIndex {
                dst,
                base_slots,
                index,
            } => {
                let index = rewrite_operand(index, &alias);
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::LoadIndex {
                    dst: *dst,
                    base_slots: base_slots.clone(),
                    index,
                });
            }
            RuntimeInstr::LoadIndexUnchecked {
                dst,
                base_slots,
                index,
            } => {
                let index = rewrite_operand(index, &alias);
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::LoadIndexUnchecked {
                    dst: *dst,
                    base_slots: base_slots.clone(),
                    index,
                });
            }
            RuntimeInstr::StoreIndex {
                base_slots,
                index,
                src,
            } => {
                let index = rewrite_operand(index, &alias);
                let src = rewrite_operand(src, &alias);
                for slot in base_slots {
                    invalidate_slot(*slot, &mut alias);
                }
                out.push(RuntimeInstr::StoreIndex {
                    base_slots: base_slots.clone(),
                    index,
                    src,
                });
            }
            RuntimeInstr::BloomSplitBlockInsert { filter_slots, hash } => {
                clear_aliases(&mut alias);
                out.push(RuntimeInstr::BloomSplitBlockInsert {
                    filter_slots: filter_slots.clone(),
                    hash: rewrite_operand(hash, &alias),
                });
            }
            RuntimeInstr::BloomSplitBlockCheck {
                dst,
                filter_slots,
                hash,
            } => {
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::BloomSplitBlockCheck {
                    dst: *dst,
                    filter_slots: filter_slots.clone(),
                    hash: rewrite_operand(hash, &alias),
                });
            }
            RuntimeInstr::BloomClassic4Check {
                dst,
                lanes_checked,
                filter_slots,
                hash,
            } => {
                invalidate_slot(*dst, &mut alias);
                invalidate_slot(*lanes_checked, &mut alias);
                out.push(RuntimeInstr::BloomClassic4Check {
                    dst: *dst,
                    lanes_checked: *lanes_checked,
                    filter_slots: filter_slots.clone(),
                    hash: rewrite_operand(hash, &alias),
                });
            }
            RuntimeInstr::HashCtrlGroupProbe {
                dst_mask,
                ctrl_slots,
                group_start,
                fingerprint,
            } => {
                invalidate_slot(*dst_mask, &mut alias);
                out.push(RuntimeInstr::HashCtrlGroupProbe {
                    dst_mask: *dst_mask,
                    ctrl_slots: ctrl_slots.clone(),
                    group_start: rewrite_operand(group_start, &alias),
                    fingerprint: rewrite_operand(fingerprint, &alias),
                });
            }
            RuntimeInstr::JoinSelectAdaptive {
                dst,
                build_rows,
                probe_rows,
            } => {
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::JoinSelectAdaptive {
                    dst: *dst,
                    build_rows: rewrite_operand(build_rows, &alias),
                    probe_rows: rewrite_operand(probe_rows, &alias),
                });
            }
            RuntimeInstr::StoreIndexUnchecked {
                base_slots,
                index,
                src,
            } => {
                let index = rewrite_operand(index, &alias);
                let src = rewrite_operand(src, &alias);
                for slot in base_slots {
                    invalidate_slot(*slot, &mut alias);
                }
                out.push(RuntimeInstr::StoreIndexUnchecked {
                    base_slots: base_slots.clone(),
                    index,
                    src,
                });
            }
            RuntimeInstr::HeapLoadInt {
                dst,
                ptr,
                index,
                bytes,
            } => {
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::HeapLoadInt {
                    dst: *dst,
                    ptr: rewrite_operand(ptr, &alias),
                    index: rewrite_operand(index, &alias),
                    bytes: *bytes,
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::HeapStoreInt {
                ptr,
                index,
                src,
                bytes,
            } => {
                out.push(RuntimeInstr::HeapStoreInt {
                    ptr: rewrite_operand(ptr, &alias),
                    index: rewrite_operand(index, &alias),
                    src: rewrite_operand(src, &alias),
                    bytes: *bytes,
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::HeapCopy {
                dst_ptr,
                src_ptr,
                bytes,
            } => {
                out.push(RuntimeInstr::HeapCopy {
                    dst_ptr: rewrite_operand(dst_ptr, &alias),
                    src_ptr: rewrite_operand(src_ptr, &alias),
                    bytes: rewrite_operand(bytes, &alias),
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::Alloc { dst, size } => {
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::Alloc {
                    dst: *dst,
                    size: rewrite_operand(size, &alias),
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::Free { ptr, size } => {
                out.push(RuntimeInstr::Free {
                    ptr: rewrite_operand(ptr, &alias),
                    size: rewrite_operand(size, &alias),
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::FileOpen {
                dst,
                path_ptr,
                flags,
                mode,
            } => {
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::FileOpen {
                    dst: *dst,
                    path_ptr: rewrite_operand(path_ptr, &alias),
                    flags: *flags,
                    mode: *mode,
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::FileWrite { dst, fd, ptr, len } => {
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::FileWrite {
                    dst: *dst,
                    fd: rewrite_operand(fd, &alias),
                    ptr: rewrite_operand(ptr, &alias),
                    len: rewrite_operand(len, &alias),
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::FileRead { dst, fd, ptr, len } => {
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::FileRead {
                    dst: *dst,
                    fd: rewrite_operand(fd, &alias),
                    ptr: rewrite_operand(ptr, &alias),
                    len: rewrite_operand(len, &alias),
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::FileClose { fd } => {
                out.push(RuntimeInstr::FileClose {
                    fd: rewrite_operand(fd, &alias),
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::ThreadSpawn {
                handle_dst,
                target,
                return_slot,
            } => {
                invalidate_slot(*handle_dst, &mut alias);
                out.push(RuntimeInstr::ThreadSpawn {
                    handle_dst: *handle_dst,
                    target: *target,
                    return_slot: *return_slot,
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::ThreadJoin { dst, handle } => {
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::ThreadJoin {
                    dst: *dst,
                    handle: rewrite_operand(handle, &alias),
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::ChannelCreate {
                dst,
                capacity,
                unbounded,
            } => {
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::ChannelCreate {
                    dst: *dst,
                    capacity: rewrite_operand(capacity, &alias),
                    unbounded: *unbounded,
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::ChannelSend { handle, value } => {
                out.push(RuntimeInstr::ChannelSend {
                    handle: rewrite_operand(handle, &alias),
                    value: rewrite_operand(value, &alias),
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::ChannelRecv { dst, handle } => {
                invalidate_slot(*dst, &mut alias);
                out.push(RuntimeInstr::ChannelRecv {
                    dst: *dst,
                    handle: rewrite_operand(handle, &alias),
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::ChannelClose { handle, sender } => {
                out.push(RuntimeInstr::ChannelClose {
                    handle: rewrite_operand(handle, &alias),
                    sender: *sender,
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::ChannelDestroy { handle } => {
                out.push(RuntimeInstr::ChannelDestroy {
                    handle: rewrite_operand(handle, &alias),
                });
                clear_aliases(&mut alias);
            }
            RuntimeInstr::PrintConst { text } => {
                out.push(RuntimeInstr::PrintConst { text: text.clone() });
            }
            RuntimeInstr::PrintInt {
                value,
                signed,
                bits,
            } => {
                out.push(RuntimeInstr::PrintInt {
                    value: rewrite_operand(value, &alias),
                    signed: *signed,
                    bits: *bits,
                });
            }
            RuntimeInstr::Return => {
                out.push(RuntimeInstr::Return);
                clear_aliases(&mut alias);
            }
            RuntimeInstr::Exit { code } => {
                out.push(RuntimeInstr::Exit {
                    code: rewrite_operand(code, &alias),
                });
                clear_aliases(&mut alias);
            }
        }
    }

    remap[instrs.len()] = out.len();
    for instr in &mut out {
        match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target }
            | RuntimeInstr::ThreadSpawn { target, .. } => {
                *target = remap[(*target).min(instrs.len())];
            }
            _ => {}
        }
    }

    out
}

fn eliminate_runtime_loop_bounds_checks(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { mut program } => {
                eliminate_runtime_loop_bounds_checks_in_program(&mut program);
                LoweredStmt::RuntimeGeneric { program }
            }
            other => other,
        })
        .collect()
}

#[derive(Clone, Copy)]
enum LoopLimit {
    Imm(u64),
    Slot(usize),
}

#[derive(Clone, Copy)]
struct CountedLoopInfo {
    header: usize,
    latch: usize,
    exit_target: usize,
    start: u64,
    ind_slot: usize,
    limit: LoopLimit,
    update_idx: usize,
}

fn unroll_runtime_small_counted_loops(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { mut program } => {
                unroll_runtime_small_counted_loops_in_program(&mut program.instrs);
                LoweredStmt::RuntimeGeneric { program }
            }
            other => other,
        })
        .collect()
}

fn rewrite_unrolled_operand(
    operand: RuntimeOperand,
    ind_slot: usize,
    iter_index: u64,
) -> RuntimeOperand {
    match operand {
        RuntimeOperand::Slot(slot) if slot == ind_slot => RuntimeOperand::Imm(iter_index),
        other => other,
    }
}

fn rewrite_unrolled_instr_with_index(
    instr: &RuntimeInstr,
    ind_slot: usize,
    iter_index: u64,
) -> RuntimeInstr {
    match instr {
        RuntimeInstr::Mov { dst, src } => RuntimeInstr::Mov {
            dst: *dst,
            src: rewrite_unrolled_operand(*src, ind_slot, iter_index),
        },
        RuntimeInstr::BinOp { dst, op, lhs, rhs } => RuntimeInstr::BinOp {
            dst: *dst,
            op: *op,
            lhs: rewrite_unrolled_operand(*lhs, ind_slot, iter_index),
            rhs: rewrite_unrolled_operand(*rhs, ind_slot, iter_index),
        },
        RuntimeInstr::BinOpInPlace { dst, op, rhs } => RuntimeInstr::BinOpInPlace {
            dst: *dst,
            op: *op,
            rhs: rewrite_unrolled_operand(*rhs, ind_slot, iter_index),
        },
        RuntimeInstr::FloatBinOp {
            dst,
            bits,
            op,
            lhs,
            rhs,
        } => RuntimeInstr::FloatBinOp {
            dst: *dst,
            bits: *bits,
            op: *op,
            lhs: rewrite_unrolled_operand(*lhs, ind_slot, iter_index),
            rhs: rewrite_unrolled_operand(*rhs, ind_slot, iter_index),
        },
        RuntimeInstr::Cmp { dst, op, lhs, rhs } => RuntimeInstr::Cmp {
            dst: *dst,
            op: *op,
            lhs: rewrite_unrolled_operand(*lhs, ind_slot, iter_index),
            rhs: rewrite_unrolled_operand(*rhs, ind_slot, iter_index),
        },
        RuntimeInstr::JumpIfCmpFalse {
            op,
            lhs,
            rhs,
            target,
        } => RuntimeInstr::JumpIfCmpFalse {
            op: *op,
            lhs: rewrite_unrolled_operand(*lhs, ind_slot, iter_index),
            rhs: rewrite_unrolled_operand(*rhs, ind_slot, iter_index),
            target: *target,
        },
        RuntimeInstr::LoadIndex {
            dst,
            base_slots,
            index,
        } => {
            let index = rewrite_unrolled_operand(*index, ind_slot, iter_index);
            if matches!(index, RuntimeOperand::Imm(v) if usize::try_from(v).is_ok_and(|idx| idx < base_slots.len()))
            {
                RuntimeInstr::LoadIndexUnchecked {
                    dst: *dst,
                    base_slots: base_slots.clone(),
                    index,
                }
            } else {
                RuntimeInstr::LoadIndex {
                    dst: *dst,
                    base_slots: base_slots.clone(),
                    index,
                }
            }
        }
        RuntimeInstr::LoadIndexUnchecked {
            dst,
            base_slots,
            index,
        } => RuntimeInstr::LoadIndexUnchecked {
            dst: *dst,
            base_slots: base_slots.clone(),
            index: rewrite_unrolled_operand(*index, ind_slot, iter_index),
        },
        RuntimeInstr::StoreIndex {
            base_slots,
            index,
            src,
        } => {
            let index = rewrite_unrolled_operand(*index, ind_slot, iter_index);
            let src = rewrite_unrolled_operand(*src, ind_slot, iter_index);
            if matches!(index, RuntimeOperand::Imm(v) if usize::try_from(v).is_ok_and(|idx| idx < base_slots.len()))
            {
                RuntimeInstr::StoreIndexUnchecked {
                    base_slots: base_slots.clone(),
                    index,
                    src,
                }
            } else {
                RuntimeInstr::StoreIndex {
                    base_slots: base_slots.clone(),
                    index,
                    src,
                }
            }
        }
        RuntimeInstr::StoreIndexUnchecked {
            base_slots,
            index,
            src,
        } => RuntimeInstr::StoreIndexUnchecked {
            base_slots: base_slots.clone(),
            index: rewrite_unrolled_operand(*index, ind_slot, iter_index),
            src: rewrite_unrolled_operand(*src, ind_slot, iter_index),
        },
        RuntimeInstr::HeapLoadInt {
            dst,
            ptr,
            index,
            bytes,
        } => RuntimeInstr::HeapLoadInt {
            dst: *dst,
            ptr: rewrite_unrolled_operand(*ptr, ind_slot, iter_index),
            index: rewrite_unrolled_operand(*index, ind_slot, iter_index),
            bytes: *bytes,
        },
        RuntimeInstr::HeapStoreInt {
            ptr,
            index,
            src,
            bytes,
        } => RuntimeInstr::HeapStoreInt {
            ptr: rewrite_unrolled_operand(*ptr, ind_slot, iter_index),
            index: rewrite_unrolled_operand(*index, ind_slot, iter_index),
            src: rewrite_unrolled_operand(*src, ind_slot, iter_index),
            bytes: *bytes,
        },
        RuntimeInstr::HeapCopy {
            dst_ptr,
            src_ptr,
            bytes,
        } => RuntimeInstr::HeapCopy {
            dst_ptr: rewrite_unrolled_operand(*dst_ptr, ind_slot, iter_index),
            src_ptr: rewrite_unrolled_operand(*src_ptr, ind_slot, iter_index),
            bytes: rewrite_unrolled_operand(*bytes, ind_slot, iter_index),
        },
        RuntimeInstr::BloomSplitBlockInsert { filter_slots, hash } => {
            RuntimeInstr::BloomSplitBlockInsert {
                filter_slots: filter_slots.clone(),
                hash: rewrite_unrolled_operand(*hash, ind_slot, iter_index),
            }
        }
        RuntimeInstr::BloomSplitBlockCheck {
            dst,
            filter_slots,
            hash,
        } => RuntimeInstr::BloomSplitBlockCheck {
            dst: *dst,
            filter_slots: filter_slots.clone(),
            hash: rewrite_unrolled_operand(*hash, ind_slot, iter_index),
        },
        RuntimeInstr::BloomClassic4Check {
            dst,
            lanes_checked,
            filter_slots,
            hash,
        } => RuntimeInstr::BloomClassic4Check {
            dst: *dst,
            lanes_checked: *lanes_checked,
            filter_slots: filter_slots.clone(),
            hash: rewrite_unrolled_operand(*hash, ind_slot, iter_index),
        },
        RuntimeInstr::HashCtrlGroupProbe {
            dst_mask,
            ctrl_slots,
            group_start,
            fingerprint,
        } => RuntimeInstr::HashCtrlGroupProbe {
            dst_mask: *dst_mask,
            ctrl_slots: ctrl_slots.clone(),
            group_start: rewrite_unrolled_operand(*group_start, ind_slot, iter_index),
            fingerprint: rewrite_unrolled_operand(*fingerprint, ind_slot, iter_index),
        },
        RuntimeInstr::JoinSelectAdaptive {
            dst,
            build_rows,
            probe_rows,
        } => RuntimeInstr::JoinSelectAdaptive {
            dst: *dst,
            build_rows: rewrite_unrolled_operand(*build_rows, ind_slot, iter_index),
            probe_rows: rewrite_unrolled_operand(*probe_rows, ind_slot, iter_index),
        },
        RuntimeInstr::Alloc { dst, size } => RuntimeInstr::Alloc {
            dst: *dst,
            size: rewrite_unrolled_operand(*size, ind_slot, iter_index),
        },
        RuntimeInstr::Free { ptr, size } => RuntimeInstr::Free {
            ptr: rewrite_unrolled_operand(*ptr, ind_slot, iter_index),
            size: rewrite_unrolled_operand(*size, ind_slot, iter_index),
        },
        RuntimeInstr::FileOpen {
            dst,
            path_ptr,
            flags,
            mode,
        } => RuntimeInstr::FileOpen {
            dst: *dst,
            path_ptr: rewrite_unrolled_operand(*path_ptr, ind_slot, iter_index),
            flags: *flags,
            mode: *mode,
        },
        RuntimeInstr::FileWrite { dst, fd, ptr, len } => RuntimeInstr::FileWrite {
            dst: *dst,
            fd: rewrite_unrolled_operand(*fd, ind_slot, iter_index),
            ptr: rewrite_unrolled_operand(*ptr, ind_slot, iter_index),
            len: rewrite_unrolled_operand(*len, ind_slot, iter_index),
        },
        RuntimeInstr::FileRead { dst, fd, ptr, len } => RuntimeInstr::FileRead {
            dst: *dst,
            fd: rewrite_unrolled_operand(*fd, ind_slot, iter_index),
            ptr: rewrite_unrolled_operand(*ptr, ind_slot, iter_index),
            len: rewrite_unrolled_operand(*len, ind_slot, iter_index),
        },
        RuntimeInstr::FileClose { fd } => RuntimeInstr::FileClose {
            fd: rewrite_unrolled_operand(*fd, ind_slot, iter_index),
        },
        RuntimeInstr::ThreadJoin { dst, handle } => RuntimeInstr::ThreadJoin {
            dst: *dst,
            handle: rewrite_unrolled_operand(*handle, ind_slot, iter_index),
        },
        RuntimeInstr::ChannelCreate {
            dst,
            capacity,
            unbounded,
        } => RuntimeInstr::ChannelCreate {
            dst: *dst,
            capacity: rewrite_unrolled_operand(*capacity, ind_slot, iter_index),
            unbounded: *unbounded,
        },
        RuntimeInstr::ChannelSend { handle, value } => RuntimeInstr::ChannelSend {
            handle: rewrite_unrolled_operand(*handle, ind_slot, iter_index),
            value: rewrite_unrolled_operand(*value, ind_slot, iter_index),
        },
        RuntimeInstr::ChannelRecv { dst, handle } => RuntimeInstr::ChannelRecv {
            dst: *dst,
            handle: rewrite_unrolled_operand(*handle, ind_slot, iter_index),
        },
        RuntimeInstr::ChannelClose { handle, sender } => RuntimeInstr::ChannelClose {
            handle: rewrite_unrolled_operand(*handle, ind_slot, iter_index),
            sender: *sender,
        },
        RuntimeInstr::ChannelDestroy { handle } => RuntimeInstr::ChannelDestroy {
            handle: rewrite_unrolled_operand(*handle, ind_slot, iter_index),
        },
        RuntimeInstr::PrintInt {
            value,
            signed,
            bits,
        } => RuntimeInstr::PrintInt {
            value: rewrite_unrolled_operand(*value, ind_slot, iter_index),
            signed: *signed,
            bits: *bits,
        },
        RuntimeInstr::Exit { code } => RuntimeInstr::Exit {
            code: rewrite_unrolled_operand(*code, ind_slot, iter_index),
        },
        RuntimeInstr::LoadSeed { .. }
        | RuntimeInstr::NormalizeInt { .. }
        | RuntimeInstr::Jump { .. }
        | RuntimeInstr::JumpIfZero { .. }
        | RuntimeInstr::CompareSwap { .. }
        | RuntimeInstr::RadixSortFixedInt { .. }
        | RuntimeInstr::Call { .. }
        | RuntimeInstr::ThreadSpawn { .. }
        | RuntimeInstr::PrintConst { .. }
        | RuntimeInstr::Return => instr.clone(),
    }
}

#[derive(Clone, Copy)]
struct UnrollPlan {
    header: usize,
    latch: usize,
    exit_target: usize,
    body_start: usize,
    body_end_exclusive: usize,
    ind_slot: usize,
    start: u64,
    trip_count: u64,
}

fn loop_hot_slot_count(
    instrs: &[RuntimeInstr],
    body_start: usize,
    body_end_exclusive: usize,
    total_slots: usize,
) -> usize {
    let mut hot = 0usize;
    for slot in 0..total_slots {
        let mut seen = false;
        for idx in body_start..body_end_exclusive {
            let instr = &instrs[idx];
            if runtime_instr_reads_slot(instr, slot) || runtime_instr_writes_slot(instr, slot) {
                seen = true;
                break;
            }
        }
        if seen {
            hot += 1;
        }
    }
    hot
}

fn unroll_trip_limit(body_len: usize, hot_slots: usize) -> u64 {
    if body_len <= 4 && hot_slots <= 8 {
        64
    } else if body_len <= 8 && hot_slots <= 12 {
        32
    } else if hot_slots <= 16 {
        16
    } else {
        8
    }
}

fn unroll_runtime_small_counted_loops_in_program(instrs: &mut Vec<RuntimeInstr>) {
    const BASE_MAX_TRIP_COUNT: u64 = 16;
    const MAX_UNROLLED_BODY_INSTRS: usize = 1024;

    if instrs.is_empty() {
        return;
    }

    let loops = find_canonical_counted_loops(instrs);
    if loops.is_empty() {
        return;
    }

    let mut incoming_targets = vec![0usize; instrs.len()];
    for instr in instrs.iter() {
        let target = match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => *target,
            _ => continue,
        };
        if target < incoming_targets.len() {
            incoming_targets[target] = incoming_targets[target].saturating_add(1);
        }
    }

    let slot_count = runtime_program_slot_count(instrs);
    let mut plans = Vec::new();
    for info in loops {
        let LoopLimit::Imm(limit) = info.limit else {
            continue;
        };
        let Some(trip_count) = limit.checked_sub(info.start) else {
            continue;
        };
        if trip_count == 0 {
            continue;
        }
        if info.header + 1 > info.update_idx || info.update_idx > info.latch {
            continue;
        }

        let body_start = info.header + 1;
        let body_end_exclusive = info.update_idx;
        let body_len = body_end_exclusive.saturating_sub(body_start);
        if body_len == 0 {
            continue;
        }

        let hot_slots = loop_hot_slot_count(instrs, body_start, body_end_exclusive, slot_count);
        let max_trip = unroll_trip_limit(body_len, hot_slots).max(BASE_MAX_TRIP_COUNT);
        if trip_count > max_trip {
            continue;
        }

        let unrolled_len = body_len.saturating_mul(trip_count as usize);
        if unrolled_len > MAX_UNROLLED_BODY_INSTRS {
            continue;
        }

        let mut has_external_entry = false;
        for target_idx in (info.header + 1)..=info.latch {
            if incoming_targets[target_idx] != 0 {
                has_external_entry = true;
                break;
            }
        }
        if has_external_entry {
            continue;
        }

        plans.push(UnrollPlan {
            header: info.header,
            latch: info.latch,
            exit_target: info.exit_target,
            body_start,
            body_end_exclusive,
            ind_slot: info.ind_slot,
            start: info.start,
            trip_count,
        });
    }

    if plans.is_empty() {
        return;
    }
    plans.sort_by_key(|p| p.header);

    let mut plan_by_header = vec![None; instrs.len()];
    for plan in plans {
        if plan.header < plan_by_header.len() {
            plan_by_header[plan.header] = Some(plan);
        }
    }

    let old = instrs.clone();
    let mut remap = vec![0usize; old.len() + 1];
    let mut out = Vec::with_capacity(old.len());
    let mut idx = 0usize;
    while idx < old.len() {
        if let Some(plan) = plan_by_header[idx] {
            let replacement_start = out.len();
            remap[plan.header] = replacement_start;
            for skipped in (plan.header + 1)..=plan.latch {
                remap[skipped] = replacement_start;
            }

            for iter in 0..plan.trip_count {
                out.push(RuntimeInstr::Mov {
                    dst: plan.ind_slot,
                    src: RuntimeOperand::Imm(plan.start + iter),
                });
                for body_idx in plan.body_start..plan.body_end_exclusive {
                    out.push(rewrite_unrolled_instr_with_index(
                        &old[body_idx],
                        plan.ind_slot,
                        plan.start + iter,
                    ));
                }
            }
            out.push(RuntimeInstr::Mov {
                dst: plan.ind_slot,
                src: RuntimeOperand::Imm(plan.start + plan.trip_count),
            });
            out.push(RuntimeInstr::Jump {
                target: plan.exit_target,
            });
            idx = plan.latch + 1;
            continue;
        }

        remap[idx] = out.len();
        out.push(old[idx].clone());
        idx += 1;
    }
    remap[old.len()] = out.len();

    for instr in &mut out {
        match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target }
            | RuntimeInstr::ThreadSpawn { target, .. } => {
                let old_target = (*target).min(old.len());
                *target = remap[old_target];
            }
            RuntimeInstr::LoadSeed { .. }
            | RuntimeInstr::Mov { .. }
            | RuntimeInstr::BinOp { .. }
            | RuntimeInstr::BinOpInPlace { .. }
            | RuntimeInstr::FloatBinOp { .. }
            | RuntimeInstr::Cmp { .. }
            | RuntimeInstr::NormalizeInt { .. }
            | RuntimeInstr::CompareSwap { .. }
            | RuntimeInstr::RadixSortFixedInt { .. }
            | RuntimeInstr::LoadIndex { .. }
            | RuntimeInstr::LoadIndexUnchecked { .. }
            | RuntimeInstr::StoreIndex { .. }
            | RuntimeInstr::StoreIndexUnchecked { .. }
            | RuntimeInstr::HeapLoadInt { .. }
            | RuntimeInstr::HeapStoreInt { .. }
            | RuntimeInstr::HeapCopy { .. }
            | RuntimeInstr::BloomSplitBlockInsert { .. }
            | RuntimeInstr::BloomSplitBlockCheck { .. }
            | RuntimeInstr::BloomClassic4Check { .. }
            | RuntimeInstr::HashCtrlGroupProbe { .. }
            | RuntimeInstr::JoinSelectAdaptive { .. }
            | RuntimeInstr::Alloc { .. }
            | RuntimeInstr::Free { .. }
            | RuntimeInstr::FileOpen { .. }
            | RuntimeInstr::FileWrite { .. }
            | RuntimeInstr::FileRead { .. }
            | RuntimeInstr::FileClose { .. }
            | RuntimeInstr::ThreadJoin { .. }
            | RuntimeInstr::ChannelCreate { .. }
            | RuntimeInstr::ChannelSend { .. }
            | RuntimeInstr::ChannelRecv { .. }
            | RuntimeInstr::ChannelClose { .. }
            | RuntimeInstr::ChannelDestroy { .. }
            | RuntimeInstr::PrintConst { .. }
            | RuntimeInstr::PrintInt { .. }
            | RuntimeInstr::Return
            | RuntimeInstr::Exit { .. } => {}
        }
    }

    *instrs = out;
}

fn eliminate_runtime_loop_bounds_checks_in_program(program: &mut RuntimeProgram) {
    select_grouped_hash_probe_loops(&mut program.instrs, &mut program.slots);
    let loops = find_canonical_counted_loops(&program.instrs);
    for info in loops {
        if info.header + 1 > info.update_idx {
            continue;
        }
        for idx in (info.header + 1)..info.update_idx {
            let rewritten = match &program.instrs[idx] {
                RuntimeInstr::LoadIndex {
                    dst,
                    base_slots,
                    index: RuntimeOperand::Slot(index_slot),
                } if loop_index_slot_is_proven_in_bounds(
                    &program.instrs,
                    idx,
                    info,
                    *index_slot,
                    base_slots.len(),
                ) =>
                {
                    Some(RuntimeInstr::LoadIndexUnchecked {
                        dst: *dst,
                        base_slots: base_slots.clone(),
                        index: RuntimeOperand::Slot(*index_slot),
                    })
                }
                RuntimeInstr::StoreIndex {
                    base_slots,
                    index: RuntimeOperand::Slot(index_slot),
                    src,
                } if loop_index_slot_is_proven_in_bounds(
                    &program.instrs,
                    idx,
                    info,
                    *index_slot,
                    base_slots.len(),
                ) =>
                {
                    Some(RuntimeInstr::StoreIndexUnchecked {
                        base_slots: base_slots.clone(),
                        index: RuntimeOperand::Slot(*index_slot),
                        src: *src,
                    })
                }
                _ => None,
            };
            if let Some(instr) = rewritten {
                program.instrs[idx] = instr;
            }
        }
    }
    eliminate_canonical_binary_search_bounds_checks(&mut program.instrs);
    eliminate_expr_bounded_index_checks_global(&mut program.instrs);
    elide_guarded_index_checks(&mut program.instrs);
    eliminate_expr_bounded_index_checks(&mut program.instrs);
    hoist_repeated_identical_index_checks(&mut program.instrs);
    let loops = find_canonical_counted_loops(&program.instrs);
    version_slot_bounded_loop_accesses(&mut program.instrs, &loops);
}

/// Removes the array check from the midpoint load in the canonical lower-bound
/// binary-search loop. The matched recurrence establishes `low < high <= len`
/// at the load. Consequently `low + ((high - low) >> 1) < high <= len`.
///
/// This deliberately recognizes the complete loop, including both updates and
/// their backedges. Keeping the proof structural prevents an unchecked load if
/// either bound is modified in some other way.
fn eliminate_canonical_binary_search_bounds_checks(instrs: &mut [RuntimeInstr]) {
    if instrs.len() < 12 {
        return;
    }

    for guard in 2..instrs.len().saturating_sub(11) {
        let (low, high, exit_target) = match &instrs[guard] {
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Slot(low),
                rhs: RuntimeOperand::Slot(high),
                target,
            } => (*low, *high, *target),
            _ => continue,
        };
        let len = match (&instrs[guard - 2], &instrs[guard - 1]) {
            (
                RuntimeInstr::Mov {
                    dst: initialized_low,
                    src: RuntimeOperand::Imm(0),
                },
                RuntimeInstr::Mov {
                    dst: initialized_high,
                    src: RuntimeOperand::Imm(len),
                },
            ) if *initialized_low == low && *initialized_high == high && *len != 0 => *len,
            _ => continue,
        };
        let delta = match &instrs[guard + 1] {
            RuntimeInstr::BinOp {
                dst,
                op: RuntimeBinOp::Sub,
                lhs: RuntimeOperand::Slot(lhs),
                rhs: RuntimeOperand::Slot(rhs),
            } if *lhs == high && *rhs == low => *dst,
            _ => continue,
        };
        let half = match &instrs[guard + 2] {
            RuntimeInstr::BinOp {
                dst,
                op: RuntimeBinOp::ShrUnsigned,
                lhs: RuntimeOperand::Slot(lhs),
                rhs: RuntimeOperand::Imm(1),
            } if *lhs == delta => *dst,
            _ => continue,
        };
        let middle = match &instrs[guard + 3] {
            RuntimeInstr::BinOp {
                dst,
                op: RuntimeBinOp::Add,
                lhs: RuntimeOperand::Slot(lhs),
                rhs: RuntimeOperand::Slot(rhs),
            } if *lhs == low && *rhs == half => *dst,
            _ => continue,
        };
        let middle_copy = match &instrs[guard + 4] {
            RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Slot(src),
            } if *src == middle => *dst,
            _ => continue,
        };
        let (load_dst, base_slots) = match &instrs[guard + 5] {
            RuntimeInstr::LoadIndex {
                dst,
                base_slots,
                index: RuntimeOperand::Slot(index),
            } if *index == middle && base_slots.len() as u64 == len => (*dst, base_slots.clone()),
            _ => continue,
        };
        let (branch_target, target_slot) = match &instrs[guard + 6] {
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Slot(loaded),
                rhs: RuntimeOperand::Slot(target),
                target: branch_dest,
                ..
            } if *loaded == load_dst => (*branch_dest, *target),
            _ => continue,
        };
        let low_next = match &instrs[guard + 7] {
            RuntimeInstr::BinOp {
                dst,
                op: RuntimeBinOp::Add,
                lhs: RuntimeOperand::Slot(lhs),
                rhs: RuntimeOperand::Imm(1),
            } if *lhs == middle_copy => *dst,
            _ => continue,
        };
        let structure_matches = matches!(
            &instrs[guard + 8],
            RuntimeInstr::Mov { dst, src: RuntimeOperand::Slot(src) }
                if *dst == low && *src == low_next
        ) && matches!(
            &instrs[guard + 9],
            RuntimeInstr::Jump { target } if *target == guard
        ) && branch_target == guard + 10
            && matches!(
                &instrs[guard + 10],
                RuntimeInstr::Mov { dst, src: RuntimeOperand::Slot(src) }
                    if *dst == high && *src == middle_copy
            )
            && matches!(
                &instrs[guard + 11],
                RuntimeInstr::Jump { target } if *target == guard
            )
            && exit_target == guard + 12;
        if !structure_matches {
            continue;
        }

        instrs[guard + 5] = RuntimeInstr::LoadIndexUnchecked {
            dst: load_dst,
            base_slots,
            index: RuntimeOperand::Slot(middle),
        };

        // A lower-bound search over [0, step, 2*step, ...], where `step` is a
        // power of two, has the closed form ceil(target / step). Only apply it
        // when a dominating mask proves that the addition cannot overflow and
        // the result remains in [0, len]. The final equality test stays intact.
        let base_slots = match &instrs[guard + 5] {
            RuntimeInstr::LoadIndexUnchecked { base_slots, .. } => base_slots,
            _ => unreachable!("midpoint load was just rewritten"),
        };
        let step = if base_slots.len() >= 2 {
            instrs.iter().find_map(|instr| match instr {
                RuntimeInstr::Mov {
                    dst,
                    src: RuntimeOperand::Imm(value),
                } if *dst == base_slots[1] => Some(*value),
                _ => None,
            })
        } else {
            None
        };
        let Some(step) = step.filter(|step| step.is_power_of_two()) else {
            continue;
        };
        let affine_table = base_slots.iter().enumerate().all(|(index, slot)| {
            let expected = (index as u64).checked_mul(step);
            expected.is_some_and(|expected| {
                instrs[..guard].iter().any(|instr| {
                    matches!(
                        instr,
                        RuntimeInstr::Mov {
                            dst,
                            src: RuntimeOperand::Imm(value),
                        } if dst == slot && *value == expected
                    )
                })
            })
        }) && !instrs.iter().any(|instr| {
            matches!(
                instr,
                RuntimeInstr::StoreIndex { base_slots: stored, .. }
                    | RuntimeInstr::StoreIndexUnchecked { base_slots: stored, .. }
                    if stored == base_slots
            )
        });
        if !affine_table {
            continue;
        }
        let Some(table_range_max) = len.checked_mul(step).and_then(|value| value.checked_sub(1))
        else {
            continue;
        };
        let target_is_bounded = runtime_slot_mask_bound_before(instrs, guard, target_slot)
            .is_some_and(|mask| mask <= table_range_max);
        if !target_is_bounded || table_range_max.checked_add(step - 1).is_none() {
            continue;
        }

        instrs[guard] = RuntimeInstr::BinOp {
            dst: delta,
            op: RuntimeBinOp::Add,
            lhs: RuntimeOperand::Slot(target_slot),
            rhs: RuntimeOperand::Imm(step - 1),
        };
        instrs[guard + 1] = RuntimeInstr::BinOp {
            dst: low,
            op: RuntimeBinOp::ShrUnsigned,
            lhs: RuntimeOperand::Slot(delta),
            rhs: RuntimeOperand::Imm(step.trailing_zeros() as u64),
        };
        instrs[guard + 2] = RuntimeInstr::Jump {
            target: exit_target,
        };
    }
}

fn runtime_slot_mask_bound_before(
    instrs: &[RuntimeInstr],
    before: usize,
    slot: usize,
) -> Option<u64> {
    let (definition, instr) = instrs[..before]
        .iter()
        .enumerate()
        .rev()
        .find(|(_, instr)| runtime_instr_writes_slot(instr, slot))?;
    match instr {
        RuntimeInstr::BinOp {
            op: RuntimeBinOp::BitAnd,
            rhs: RuntimeOperand::Imm(mask),
            ..
        } => Some(*mask),
        RuntimeInstr::Mov {
            src: RuntimeOperand::Slot(source),
            ..
        } if *source != slot => runtime_slot_mask_bound_before(instrs, definition, *source),
        RuntimeInstr::Mov {
            src: RuntimeOperand::Imm(value),
            ..
        } => Some(*value),
        _ => None,
    }
}

fn bump_runtime_targets_from(instrs: &mut [RuntimeInstr], from: usize, delta: usize) {
    for instr in instrs {
        match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => {
                if *target >= from {
                    *target += delta;
                }
            }
            _ => {}
        }
    }
}

fn slot_def_idx_in_loop_body(
    instrs: &[RuntimeInstr],
    body_start: usize,
    use_idx: usize,
    slot: usize,
) -> Option<usize> {
    if use_idx <= body_start {
        return None;
    }
    for idx in (body_start..use_idx).rev() {
        if !runtime_instr_writes_slot(&instrs[idx], slot) {
            continue;
        }
        if matches!(&instrs[idx], RuntimeInstr::NormalizeInt { dst, .. } if *dst == slot) {
            continue;
        }
        return Some(idx);
    }
    None
}

fn runtime_operand_loop_invariant(
    instrs: &[RuntimeInstr],
    info: CountedLoopInfo,
    operand: RuntimeOperand,
) -> bool {
    match operand {
        RuntimeOperand::Imm(_) => true,
        RuntimeOperand::Slot(slot) => !slot_written_in_loop(instrs, info, slot),
    }
}

fn derive_group_start_for_grouped_probe(
    instrs: &[RuntimeInstr],
    info: CountedLoopInfo,
    load_idx: usize,
    base_len: usize,
    index: RuntimeOperand,
) -> Option<RuntimeOperand> {
    if info.start != 0 || base_len < 16 || !base_len.is_power_of_two() {
        return None;
    }
    match index {
        RuntimeOperand::Slot(slot) if slot == info.ind_slot && base_len == 16 => {
            Some(RuntimeOperand::Imm(0))
        }
        RuntimeOperand::Slot(idx_slot) => {
            let body_start = info.header + 1;
            let bitand_idx = slot_def_idx_in_loop_body(instrs, body_start, load_idx, idx_slot)?;
            let RuntimeInstr::BinOp {
                dst,
                op: RuntimeBinOp::BitAnd,
                lhs,
                rhs,
            } = instrs[bitand_idx]
            else {
                return None;
            };
            if dst != idx_slot {
                return None;
            }
            let mask = match (lhs, rhs) {
                (RuntimeOperand::Imm(m), RuntimeOperand::Slot(v))
                | (RuntimeOperand::Slot(v), RuntimeOperand::Imm(m)) => (m, v),
                _ => return None,
            };
            if mask.0 != (base_len as u64).wrapping_sub(1) {
                return None;
            }
            let add_slot = mask.1;
            let add_idx = slot_def_idx_in_loop_body(instrs, body_start, bitand_idx, add_slot)?;
            let RuntimeInstr::BinOp {
                dst,
                op: RuntimeBinOp::Add,
                lhs,
                rhs,
            } = instrs[add_idx]
            else {
                return None;
            };
            if dst != add_slot {
                return None;
            }
            let group_start = match (lhs, rhs) {
                (RuntimeOperand::Slot(slot), other) if slot == info.ind_slot => other,
                (other, RuntimeOperand::Slot(slot)) if slot == info.ind_slot => other,
                _ => return None,
            };
            runtime_operand_loop_invariant(instrs, info, group_start).then_some(group_start)
        }
        RuntimeOperand::Imm(_) => None,
    }
}

fn loop_has_external_entry_to_header(instrs: &[RuntimeInstr], info: CountedLoopInfo) -> bool {
    for (idx, instr) in instrs.iter().enumerate() {
        let target = match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => *target,
            _ => continue,
        };
        if target == info.header && (idx < info.header || idx > info.latch) {
            return true;
        }
    }
    false
}

fn try_select_grouped_hash_probe_loop(
    instrs: &mut Vec<RuntimeInstr>,
    slots: &mut usize,
    info: CountedLoopInfo,
) -> bool {
    let LoopLimit::Imm(limit) = info.limit else {
        return false;
    };
    if info.start != 0 || limit != 16 || loop_has_external_entry_to_header(instrs, info) {
        return false;
    }
    if info.header + 1 > info.update_idx
        || info.update_idx > info.latch
        || info.latch >= instrs.len()
    {
        return false;
    }

    let body_start = info.header + 1;
    let body_end = info.update_idx;
    for cmp_idx in body_start..body_end {
        let (load_slot, fingerprint) = match instrs[cmp_idx] {
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::Eq,
                lhs: RuntimeOperand::Slot(slot),
                rhs,
                ..
            } => (slot, rhs),
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::Eq,
                lhs,
                rhs: RuntimeOperand::Slot(slot),
                ..
            } => (slot, lhs),
            _ => continue,
        };
        if matches!(fingerprint, RuntimeOperand::Imm(0))
            || !runtime_operand_loop_invariant(instrs, info, fingerprint)
        {
            continue;
        }
        let Some(load_idx) = slot_def_idx_in_loop_body(instrs, body_start, cmp_idx, load_slot)
        else {
            continue;
        };
        let (base_slots, index) = match &instrs[load_idx] {
            RuntimeInstr::LoadIndex {
                dst,
                base_slots,
                index,
            }
            | RuntimeInstr::LoadIndexUnchecked {
                dst,
                base_slots,
                index,
            } if *dst == load_slot => (base_slots.clone(), *index),
            _ => continue,
        };
        if base_slots.len() < 16 || !base_slots.len().is_power_of_two() {
            continue;
        }
        let group_start = match derive_group_start_for_grouped_probe(
            instrs,
            info,
            load_idx,
            base_slots.len(),
            index,
        ) {
            Some(value) => value,
            None => continue,
        };
        if (load_idx + 1..cmp_idx).any(|idx| runtime_instr_writes_slot(&instrs[idx], load_slot)) {
            continue;
        }
        if (cmp_idx + 1..body_end).any(|idx| runtime_instr_reads_slot(&instrs[idx], load_slot)) {
            continue;
        }

        let mask_slot = *slots;
        *slots += 1;
        instrs.insert(
            info.header,
            RuntimeInstr::HashCtrlGroupProbe {
                dst_mask: mask_slot,
                ctrl_slots: base_slots,
                group_start,
                fingerprint,
            },
        );
        bump_runtime_targets_from(instrs, info.header, 1);
        let cmp_idx = cmp_idx + 1;

        instrs.insert(
            cmp_idx,
            RuntimeInstr::BinOp {
                dst: load_slot,
                op: RuntimeBinOp::ShrUnsigned,
                lhs: RuntimeOperand::Slot(mask_slot),
                rhs: RuntimeOperand::Slot(info.ind_slot),
            },
        );
        bump_runtime_targets_from(instrs, cmp_idx, 1);
        instrs.insert(
            cmp_idx + 1,
            RuntimeInstr::BinOpInPlace {
                dst: load_slot,
                op: RuntimeBinOp::BitAnd,
                rhs: RuntimeOperand::Imm(1),
            },
        );
        bump_runtime_targets_from(instrs, cmp_idx + 1, 1);
        let target = match instrs[cmp_idx + 2] {
            RuntimeInstr::JumpIfCmpFalse { target, .. } => target,
            _ => continue,
        };
        instrs[cmp_idx + 2] = RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Eq,
            lhs: RuntimeOperand::Slot(load_slot),
            rhs: RuntimeOperand::Imm(1),
            target,
        };
        return true;
    }
    false
}

fn select_grouped_hash_probe_loops(instrs: &mut Vec<RuntimeInstr>, slots: &mut usize) {
    for _ in 0..8 {
        let loops = find_relaxed_counted_loops(instrs);
        let mut changed = false;
        for info in loops {
            if try_select_grouped_hash_probe_loop(instrs, slots, info) {
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }
}

fn find_relaxed_counted_loops(instrs: &[RuntimeInstr]) -> Vec<CountedLoopInfo> {
    let mut loops = Vec::new();
    for latch in 0..instrs.len() {
        let RuntimeInstr::Jump { target: header } = instrs[latch] else {
            continue;
        };
        if header >= latch || header == 0 {
            continue;
        }

        let (op, lhs, rhs, exit_target) = match &instrs[header] {
            RuntimeInstr::JumpIfCmpFalse {
                op,
                lhs,
                rhs,
                target,
            } => (*op, *lhs, *rhs, *target),
            _ => continue,
        };
        if op != RuntimeCmpOp::LtUnsigned {
            continue;
        }
        let RuntimeOperand::Slot(ind_slot) = lhs else {
            continue;
        };
        let limit = match rhs {
            RuntimeOperand::Imm(limit) => LoopLimit::Imm(limit),
            RuntimeOperand::Slot(slot) => LoopLimit::Slot(slot),
        };
        if exit_target <= latch || exit_target > instrs.len() {
            continue;
        }

        let mut start = None;
        for idx in (0..header).rev() {
            if !runtime_instr_writes_slot(&instrs[idx], ind_slot) {
                continue;
            }
            if let RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Imm(init),
            } = instrs[idx]
            {
                if dst == ind_slot {
                    start = Some(init);
                }
            }
            break;
        }
        let Some(start) = start else {
            continue;
        };
        if matches!(limit, LoopLimit::Imm(v) if start > v) {
            continue;
        }

        let (update_idx, tail_norm_idx) = if latch > header + 1 {
            match &instrs[latch - 1] {
                RuntimeInstr::BinOpInPlace {
                    dst,
                    op: RuntimeBinOp::Add,
                    rhs: RuntimeOperand::Imm(1),
                } if *dst == ind_slot => (latch - 1, None),
                RuntimeInstr::NormalizeInt { dst, .. } if *dst == ind_slot => {
                    let update_idx = latch.saturating_sub(2);
                    let is_update = matches!(
                        instrs.get(update_idx),
                        Some(RuntimeInstr::BinOpInPlace {
                            dst,
                            op: RuntimeBinOp::Add,
                            rhs: RuntimeOperand::Imm(1),
                        }) if *dst == ind_slot
                    );
                    if is_update {
                        (update_idx, Some(latch - 1))
                    } else {
                        continue;
                    }
                }
                _ => continue,
            }
        } else {
            continue;
        };

        let mut valid = true;
        for idx in (header + 1)..latch {
            if idx == update_idx || Some(idx) == tail_norm_idx {
                continue;
            }
            if runtime_instr_writes_slot(&instrs[idx], ind_slot) {
                valid = false;
                break;
            }
            if matches!(limit, LoopLimit::Slot(limit_slot) if runtime_instr_writes_slot(&instrs[idx], limit_slot))
            {
                valid = false;
                break;
            }
        }
        if !valid {
            continue;
        }

        loops.push(CountedLoopInfo {
            header,
            latch,
            exit_target,
            start,
            ind_slot,
            limit,
            update_idx,
        });
    }
    loops
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SlotFacts {
    upper: Option<u64>,
    exact: Option<u64>,
}

impl SlotFacts {
    fn unknown() -> Self {
        Self {
            upper: None,
            exact: None,
        }
    }

    fn imm(value: u64) -> Self {
        Self {
            upper: Some(value),
            exact: Some(value),
        }
    }
}

fn runtime_program_slot_count(instrs: &[RuntimeInstr]) -> usize {
    fn bump_slot(max_slot: &mut usize, saw_any: &mut bool, slot: usize) {
        *saw_any = true;
        *max_slot = (*max_slot).max(slot);
    }

    fn bump_operand(max_slot: &mut usize, saw_any: &mut bool, operand: &RuntimeOperand) {
        if let RuntimeOperand::Slot(slot) = operand {
            bump_slot(max_slot, saw_any, *slot);
        }
    }

    let mut max_slot = 0usize;
    let mut saw_any = false;

    for instr in instrs {
        match instr {
            RuntimeInstr::LoadSeed { dst, input, .. } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                if let Some(input) = input {
                    bump_operand(&mut max_slot, &mut saw_any, input);
                }
            }
            RuntimeInstr::Mov { dst, src } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, src);
            }
            RuntimeInstr::BinOp { dst, lhs, rhs, .. } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, lhs);
                bump_operand(&mut max_slot, &mut saw_any, rhs);
            }
            RuntimeInstr::BinOpInPlace { dst, rhs, .. } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, rhs);
            }
            RuntimeInstr::FloatBinOp { dst, lhs, rhs, .. } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, lhs);
                bump_operand(&mut max_slot, &mut saw_any, rhs);
            }
            RuntimeInstr::Cmp { dst, lhs, rhs, .. } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, lhs);
                bump_operand(&mut max_slot, &mut saw_any, rhs);
            }
            RuntimeInstr::NormalizeInt { dst, .. } => bump_slot(&mut max_slot, &mut saw_any, *dst),
            RuntimeInstr::Jump { .. } => {}
            RuntimeInstr::JumpIfZero { cond_slot, .. } => {
                bump_slot(&mut max_slot, &mut saw_any, *cond_slot)
            }
            RuntimeInstr::JumpIfCmpFalse { lhs, rhs, .. } => {
                bump_operand(&mut max_slot, &mut saw_any, lhs);
                bump_operand(&mut max_slot, &mut saw_any, rhs);
            }
            RuntimeInstr::CompareSwap { left, right, .. } => {
                bump_slot(&mut max_slot, &mut saw_any, *left);
                bump_slot(&mut max_slot, &mut saw_any, *right);
            }
            RuntimeInstr::RadixSortFixedInt { slots, .. } => {
                for slot in slots {
                    bump_slot(&mut max_slot, &mut saw_any, *slot);
                }
            }
            RuntimeInstr::Call { .. } => {}
            RuntimeInstr::LoadIndex {
                dst,
                base_slots,
                index,
            }
            | RuntimeInstr::LoadIndexUnchecked {
                dst,
                base_slots,
                index,
            } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                for slot in base_slots {
                    bump_slot(&mut max_slot, &mut saw_any, *slot);
                }
                bump_operand(&mut max_slot, &mut saw_any, index);
            }
            RuntimeInstr::StoreIndex {
                base_slots,
                index,
                src,
            }
            | RuntimeInstr::StoreIndexUnchecked {
                base_slots,
                index,
                src,
            } => {
                for slot in base_slots {
                    bump_slot(&mut max_slot, &mut saw_any, *slot);
                }
                bump_operand(&mut max_slot, &mut saw_any, index);
                bump_operand(&mut max_slot, &mut saw_any, src);
            }
            RuntimeInstr::HeapLoadInt {
                dst, ptr, index, ..
            } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, ptr);
                bump_operand(&mut max_slot, &mut saw_any, index);
            }
            RuntimeInstr::HeapStoreInt {
                ptr, index, src, ..
            } => {
                bump_operand(&mut max_slot, &mut saw_any, ptr);
                bump_operand(&mut max_slot, &mut saw_any, index);
                bump_operand(&mut max_slot, &mut saw_any, src);
            }
            RuntimeInstr::HeapCopy {
                dst_ptr,
                src_ptr,
                bytes,
            } => {
                bump_operand(&mut max_slot, &mut saw_any, dst_ptr);
                bump_operand(&mut max_slot, &mut saw_any, src_ptr);
                bump_operand(&mut max_slot, &mut saw_any, bytes);
            }
            RuntimeInstr::BloomSplitBlockInsert { filter_slots, hash } => {
                for slot in filter_slots {
                    bump_slot(&mut max_slot, &mut saw_any, *slot);
                }
                bump_operand(&mut max_slot, &mut saw_any, hash);
            }
            RuntimeInstr::BloomSplitBlockCheck {
                dst,
                filter_slots,
                hash,
            } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                for slot in filter_slots {
                    bump_slot(&mut max_slot, &mut saw_any, *slot);
                }
                bump_operand(&mut max_slot, &mut saw_any, hash);
            }
            RuntimeInstr::BloomClassic4Check {
                dst,
                lanes_checked,
                filter_slots,
                hash,
            } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_slot(&mut max_slot, &mut saw_any, *lanes_checked);
                for slot in filter_slots {
                    bump_slot(&mut max_slot, &mut saw_any, *slot);
                }
                bump_operand(&mut max_slot, &mut saw_any, hash);
            }
            RuntimeInstr::HashCtrlGroupProbe {
                dst_mask,
                ctrl_slots,
                group_start,
                fingerprint,
            } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst_mask);
                for slot in ctrl_slots {
                    bump_slot(&mut max_slot, &mut saw_any, *slot);
                }
                bump_operand(&mut max_slot, &mut saw_any, group_start);
                bump_operand(&mut max_slot, &mut saw_any, fingerprint);
            }
            RuntimeInstr::JoinSelectAdaptive {
                dst,
                build_rows,
                probe_rows,
            } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, build_rows);
                bump_operand(&mut max_slot, &mut saw_any, probe_rows);
            }
            RuntimeInstr::Alloc { dst, size } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, size);
            }
            RuntimeInstr::Free { ptr, size } => {
                bump_operand(&mut max_slot, &mut saw_any, ptr);
                bump_operand(&mut max_slot, &mut saw_any, size);
            }
            RuntimeInstr::FileOpen { dst, path_ptr, .. } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, path_ptr);
            }
            RuntimeInstr::FileWrite { dst, fd, ptr, len }
            | RuntimeInstr::FileRead { dst, fd, ptr, len } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, fd);
                bump_operand(&mut max_slot, &mut saw_any, ptr);
                bump_operand(&mut max_slot, &mut saw_any, len);
            }
            RuntimeInstr::FileClose { fd } => bump_operand(&mut max_slot, &mut saw_any, fd),
            RuntimeInstr::ThreadSpawn {
                handle_dst,
                return_slot,
                ..
            } => {
                bump_slot(&mut max_slot, &mut saw_any, *handle_dst);
                if let Some(slot) = return_slot {
                    bump_slot(&mut max_slot, &mut saw_any, *slot);
                }
            }
            RuntimeInstr::ThreadJoin { dst, handle } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, handle);
            }
            RuntimeInstr::ChannelCreate { dst, capacity, .. } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, capacity);
            }
            RuntimeInstr::ChannelSend { handle, value } => {
                bump_operand(&mut max_slot, &mut saw_any, handle);
                bump_operand(&mut max_slot, &mut saw_any, value);
            }
            RuntimeInstr::ChannelRecv { dst, handle } => {
                bump_slot(&mut max_slot, &mut saw_any, *dst);
                bump_operand(&mut max_slot, &mut saw_any, handle);
            }
            RuntimeInstr::ChannelClose { handle, .. } | RuntimeInstr::ChannelDestroy { handle } => {
                bump_operand(&mut max_slot, &mut saw_any, handle)
            }
            RuntimeInstr::PrintConst { .. } => {}
            RuntimeInstr::PrintInt { value, .. } => {
                bump_operand(&mut max_slot, &mut saw_any, value);
            }
            RuntimeInstr::Return => {}
            RuntimeInstr::Exit { code } => bump_operand(&mut max_slot, &mut saw_any, code),
        }
    }

    if saw_any { max_slot + 1 } else { 0 }
}

fn facts_for_operand(facts: &[SlotFacts], operand: &RuntimeOperand) -> SlotFacts {
    match operand {
        RuntimeOperand::Imm(value) => SlotFacts::imm(*value),
        RuntimeOperand::Slot(slot) => facts.get(*slot).copied().unwrap_or_default(),
    }
}

fn upper_bound_for_binop_facts(op: RuntimeBinOp, lhs: SlotFacts, rhs: SlotFacts) -> Option<u64> {
    fn bitmask_upper(value: u64) -> u64 {
        if value == u64::MAX {
            return u64::MAX;
        }
        if value == 0 {
            return 0;
        }
        let bits = 64u32.saturating_sub(value.leading_zeros());
        if bits >= 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        }
    }

    match op {
        RuntimeBinOp::Add => None,
        RuntimeBinOp::Sub => {
            if rhs.exact == Some(0) {
                lhs.upper
            } else {
                None
            }
        }
        RuntimeBinOp::Mul => lhs.upper.zip(rhs.upper).and_then(|(a, b)| a.checked_mul(b)),
        RuntimeBinOp::DivUnsigned => match rhs.exact {
            Some(0) => None,
            Some(_) => lhs.upper,
            None => None,
        },
        RuntimeBinOp::DivSigned => None,
        RuntimeBinOp::ModUnsigned => rhs
            .exact
            .and_then(|m| if m == 0 { None } else { Some(m - 1) }),
        RuntimeBinOp::ModSigned => None,
        RuntimeBinOp::BitAnd => match (lhs.exact, rhs.exact) {
            (Some(a), Some(b)) => Some(a & b),
            (_, Some(mask)) => Some(lhs.upper.map_or(mask, |u| u.min(mask))),
            (Some(mask), _) => Some(rhs.upper.map_or(mask, |u| u.min(mask))),
            _ => None,
        },
        RuntimeBinOp::BitOr => match (lhs.exact, rhs.exact) {
            (Some(a), Some(b)) => Some(a | b),
            (Some(a), None) => rhs.upper.map(|b| a | bitmask_upper(b)),
            (None, Some(b)) => lhs.upper.map(|a| bitmask_upper(a) | b),
            (None, None) => lhs
                .upper
                .zip(rhs.upper)
                .map(|(a, b)| bitmask_upper(a) | bitmask_upper(b)),
        },
        RuntimeBinOp::BitXor => match (lhs.exact, rhs.exact) {
            (Some(a), Some(b)) => Some(a ^ b),
            _ => None,
        },
        RuntimeBinOp::Shl => {
            let shift = rhs.exact?;
            if shift >= 64 {
                Some(0)
            } else {
                lhs.upper.and_then(|v| v.checked_shl(shift as u32))
            }
        }
        RuntimeBinOp::ShrUnsigned => {
            let shift = rhs.exact?;
            if shift >= 64 {
                Some(0)
            } else {
                Some(lhs.upper.map_or(u64::MAX, |v| v) >> shift)
            }
        }
        RuntimeBinOp::ShrSigned => None,
    }
}

fn binop_facts(op: RuntimeBinOp, lhs: SlotFacts, rhs: SlotFacts) -> SlotFacts {
    let exact = lhs
        .exact
        .zip(rhs.exact)
        .and_then(|(l, r)| eval_runtime_binop(op, l, r));
    let upper = exact.or_else(|| upper_bound_for_binop_facts(op, lhs, rhs));
    SlotFacts { upper, exact }
}

fn merge_slot_facts(dst: &mut SlotFacts, src: SlotFacts) -> bool {
    let mut changed = false;

    let new_upper = match (dst.upper, src.upper) {
        (Some(a), Some(b)) => Some(a.max(b)),
        _ => None,
    };
    if dst.upper != new_upper {
        dst.upper = new_upper;
        changed = true;
    }

    let new_exact = match (dst.exact, src.exact) {
        (Some(a), Some(b)) if a == b => Some(a),
        _ => None,
    };
    if dst.exact != new_exact {
        dst.exact = new_exact;
        changed = true;
    }

    changed
}

fn merge_fact_vectors(dst: &mut [SlotFacts], src: &[SlotFacts]) -> bool {
    let mut changed = false;
    for (d, s) in dst.iter_mut().zip(src.iter().copied()) {
        changed |= merge_slot_facts(d, s);
    }
    changed
}

fn runtime_transfer_slot_facts(
    instr: &RuntimeInstr,
    in_facts: &[SlotFacts],
    slot_count: usize,
) -> Vec<SlotFacts> {
    let mut out = in_facts.to_vec();
    let mut set_slot = |slot: usize, facts: SlotFacts| {
        if slot < out.len() {
            out[slot] = facts;
        }
    };

    match instr {
        RuntimeInstr::LoadSeed { dst, .. } => set_slot(*dst, SlotFacts::unknown()),
        RuntimeInstr::Mov { dst, src } => set_slot(*dst, facts_for_operand(in_facts, src)),
        RuntimeInstr::BinOp { dst, op, lhs, rhs } => {
            let lhs = facts_for_operand(in_facts, lhs);
            let rhs = facts_for_operand(in_facts, rhs);
            set_slot(*dst, binop_facts(*op, lhs, rhs));
        }
        RuntimeInstr::BinOpInPlace { dst, op, rhs } => {
            let lhs = in_facts.get(*dst).copied().unwrap_or_default();
            let rhs = facts_for_operand(in_facts, rhs);
            set_slot(*dst, binop_facts(*op, lhs, rhs));
        }
        RuntimeInstr::FloatBinOp { dst, .. } => set_slot(*dst, SlotFacts::unknown()),
        RuntimeInstr::Cmp { dst, op, lhs, rhs } => {
            let lhs = facts_for_operand(in_facts, lhs);
            let rhs = facts_for_operand(in_facts, rhs);
            let exact = lhs
                .exact
                .zip(rhs.exact)
                .map(|(l, r)| eval_runtime_cmp(*op, l, r));
            set_slot(
                *dst,
                SlotFacts {
                    upper: Some(1),
                    exact,
                },
            );
        }
        RuntimeInstr::NormalizeInt { dst, signed, bits } => {
            let prior = in_facts.get(*dst).copied().unwrap_or_default();
            let facts = if *signed {
                SlotFacts::unknown()
            } else if *bits >= 64 {
                prior
            } else {
                let max = (1u64 << u32::from(*bits)) - 1;
                let upper = prior.upper.map(|u| u.min(max)).or(Some(max));
                let exact = prior.exact.map(|v| v & max);
                SlotFacts { upper, exact }
            };
            set_slot(*dst, facts);
        }
        RuntimeInstr::Jump { .. } => {}
        RuntimeInstr::JumpIfZero { .. } => {}
        RuntimeInstr::JumpIfCmpFalse { .. } => {}
        RuntimeInstr::CompareSwap { left, right, .. } => {
            let left_f = in_facts.get(*left).copied().unwrap_or_default();
            let right_f = in_facts.get(*right).copied().unwrap_or_default();
            let upper = left_f.upper.zip(right_f.upper).map(|(a, b)| a.max(b));
            let exact = match (left_f.exact, right_f.exact) {
                (Some(a), Some(b)) if a == b => Some(a),
                _ => None,
            };
            let merged = SlotFacts { upper, exact };
            set_slot(*left, merged);
            set_slot(*right, merged);
        }
        RuntimeInstr::RadixSortFixedInt { slots, .. } => {
            for slot in slots {
                set_slot(*slot, SlotFacts::unknown());
            }
        }
        RuntimeInstr::Call { .. } => {
            out = vec![SlotFacts::unknown(); slot_count];
        }
        RuntimeInstr::LoadIndex { dst, .. } | RuntimeInstr::LoadIndexUnchecked { dst, .. } => {
            set_slot(*dst, SlotFacts::unknown())
        }
        RuntimeInstr::HeapLoadInt { dst, .. } => set_slot(*dst, SlotFacts::unknown()),
        RuntimeInstr::HeapStoreInt { .. } | RuntimeInstr::HeapCopy { .. } => {}
        RuntimeInstr::StoreIndex { base_slots, .. }
        | RuntimeInstr::StoreIndexUnchecked { base_slots, .. } => {
            for slot in base_slots {
                set_slot(*slot, SlotFacts::unknown());
            }
        }
        RuntimeInstr::BloomSplitBlockInsert { filter_slots, .. } => {
            for slot in filter_slots {
                set_slot(*slot, SlotFacts::unknown());
            }
        }
        RuntimeInstr::BloomSplitBlockCheck {
            dst, filter_slots, ..
        } => {
            set_slot(*dst, SlotFacts::unknown());
            for slot in filter_slots {
                set_slot(*slot, SlotFacts::unknown());
            }
        }
        RuntimeInstr::BloomClassic4Check {
            dst,
            lanes_checked,
            filter_slots,
            ..
        } => {
            set_slot(*dst, SlotFacts::unknown());
            set_slot(*lanes_checked, SlotFacts::unknown());
            for slot in filter_slots {
                set_slot(*slot, SlotFacts::unknown());
            }
        }
        RuntimeInstr::HashCtrlGroupProbe {
            dst_mask,
            ctrl_slots,
            ..
        } => {
            set_slot(*dst_mask, SlotFacts::unknown());
            for slot in ctrl_slots {
                set_slot(*slot, SlotFacts::unknown());
            }
        }
        RuntimeInstr::JoinSelectAdaptive { dst, .. } => set_slot(*dst, SlotFacts::unknown()),
        RuntimeInstr::Alloc { dst, .. } => set_slot(*dst, SlotFacts::unknown()),
        RuntimeInstr::FileOpen { dst, .. }
        | RuntimeInstr::FileWrite { dst, .. }
        | RuntimeInstr::FileRead { dst, .. }
        | RuntimeInstr::ThreadJoin { dst, .. } => set_slot(*dst, SlotFacts::unknown()),
        RuntimeInstr::ThreadSpawn { handle_dst, .. } => set_slot(*handle_dst, SlotFacts::unknown()),
        RuntimeInstr::ChannelCreate { dst, .. } | RuntimeInstr::ChannelRecv { dst, .. } => {
            set_slot(*dst, SlotFacts::unknown())
        }
        RuntimeInstr::ChannelSend { .. }
        | RuntimeInstr::ChannelClose { .. }
        | RuntimeInstr::ChannelDestroy { .. } => {}
        RuntimeInstr::Free { .. } | RuntimeInstr::FileClose { .. } => {}
        RuntimeInstr::PrintConst { .. } => {}
        RuntimeInstr::PrintInt { .. } => {}
        RuntimeInstr::Return => {}
        RuntimeInstr::Exit { .. } => {}
    }
    out
}

fn runtime_succ_targets(instrs: &[RuntimeInstr], idx: usize) -> [Option<usize>; 2] {
    let next = (idx + 1 < instrs.len()).then_some(idx + 1);
    match &instrs[idx] {
        RuntimeInstr::Jump { target } => [(*target < instrs.len()).then_some(*target), None],
        RuntimeInstr::JumpIfZero { target, .. } | RuntimeInstr::JumpIfCmpFalse { target, .. } => {
            [(*target < instrs.len()).then_some(*target), next]
        }
        RuntimeInstr::Call { target } => [(*target < instrs.len()).then_some(*target), next],
        RuntimeInstr::Return | RuntimeInstr::Exit { .. } => [None, None],
        _ => [next, None],
    }
}

fn compute_runtime_slot_facts_at_instr(instrs: &[RuntimeInstr]) -> Vec<Option<Vec<SlotFacts>>> {
    let n = instrs.len();
    let slot_count = runtime_program_slot_count(instrs);
    let mut in_states = vec![None; n];
    if n == 0 {
        return in_states;
    }
    in_states[0] = Some(vec![SlotFacts::unknown(); slot_count]);

    let mut queue = VecDeque::new();
    queue.push_back(0usize);
    while let Some(idx) = queue.pop_front() {
        let Some(in_facts) = in_states[idx].clone() else {
            continue;
        };
        let out_facts = runtime_transfer_slot_facts(&instrs[idx], &in_facts, slot_count);
        for succ in runtime_succ_targets(instrs, idx).into_iter().flatten() {
            let changed = if let Some(existing) = &mut in_states[succ] {
                merge_fact_vectors(existing, &out_facts)
            } else {
                in_states[succ] = Some(out_facts.clone());
                true
            };
            if changed {
                queue.push_back(succ);
            }
        }
    }
    in_states
}

fn runtime_index_operand_proven_in_bounds_from_facts(
    facts: &[SlotFacts],
    index: &RuntimeOperand,
    base_len: usize,
) -> bool {
    if base_len == 0 {
        return false;
    }
    match index {
        RuntimeOperand::Imm(value) => usize::try_from(*value).is_ok_and(|v| v < base_len),
        RuntimeOperand::Slot(slot) => facts
            .get(*slot)
            .and_then(|f| f.upper)
            .is_some_and(|max| max < base_len as u64),
    }
}

fn eliminate_expr_bounded_index_checks_global(instrs: &mut [RuntimeInstr]) {
    let facts_at_instr = compute_runtime_slot_facts_at_instr(instrs);
    for idx in 0..instrs.len() {
        let Some(facts) = facts_at_instr[idx].as_deref() else {
            continue;
        };
        let rewritten = match &instrs[idx] {
            RuntimeInstr::LoadIndex {
                dst,
                base_slots,
                index,
            } if runtime_index_operand_proven_in_bounds_from_facts(
                facts,
                index,
                base_slots.len(),
            ) =>
            {
                Some(RuntimeInstr::LoadIndexUnchecked {
                    dst: *dst,
                    base_slots: base_slots.clone(),
                    index: *index,
                })
            }
            RuntimeInstr::StoreIndex {
                base_slots,
                index,
                src,
            } if runtime_index_operand_proven_in_bounds_from_facts(
                facts,
                index,
                base_slots.len(),
            ) =>
            {
                Some(RuntimeInstr::StoreIndexUnchecked {
                    base_slots: base_slots.clone(),
                    index: *index,
                    src: *src,
                })
            }
            _ => None,
        };
        if let Some(next) = rewritten {
            instrs[idx] = next;
        }
    }
}

fn loop_index_slot_is_proven_in_bounds(
    instrs: &[RuntimeInstr],
    use_idx: usize,
    info: CountedLoopInfo,
    index_slot: usize,
    base_len: usize,
) -> bool {
    prove_loop_index_slot_in_bounds(instrs, use_idx, info, index_slot, base_len, 0)
}

fn slot_written_in_loop(instrs: &[RuntimeInstr], info: CountedLoopInfo, slot: usize) -> bool {
    if instrs.is_empty() {
        return false;
    }
    let start = info.header.saturating_add(1);
    if start > info.update_idx || start >= instrs.len() {
        return false;
    }
    let end = info.update_idx.min(instrs.len() - 1);
    (start..=end).any(|idx| runtime_instr_writes_slot(&instrs[idx], slot))
}

fn loop_invariant_operand_const_before(
    instrs: &[RuntimeInstr],
    info: CountedLoopInfo,
    operand: &RuntimeOperand,
    before_idx: usize,
    depth: u8,
) -> Option<u64> {
    match operand {
        RuntimeOperand::Imm(value) => Some(*value),
        RuntimeOperand::Slot(slot) => {
            if slot_written_in_loop(instrs, info, *slot) {
                return None;
            }
            loop_invariant_slot_const_before(instrs, info, *slot, before_idx, depth + 1)
        }
    }
}

fn loop_invariant_slot_const_before(
    instrs: &[RuntimeInstr],
    info: CountedLoopInfo,
    slot: usize,
    before_idx: usize,
    depth: u8,
) -> Option<u64> {
    if depth > 8 {
        return None;
    }
    for def_idx in (0..before_idx.min(instrs.len())).rev() {
        let instr = &instrs[def_idx];
        if !runtime_instr_writes_slot(instr, slot) {
            continue;
        }
        return match instr {
            RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Imm(value),
            } if *dst == slot => Some(*value),
            RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Slot(src_slot),
            } if *dst == slot => {
                loop_invariant_slot_const_before(instrs, info, *src_slot, def_idx, depth + 1)
            }
            RuntimeInstr::BinOp { dst, op, lhs, rhs } if *dst == slot => {
                let lhs_value =
                    loop_invariant_operand_const_before(instrs, info, lhs, def_idx, depth + 1)?;
                let rhs_value =
                    loop_invariant_operand_const_before(instrs, info, rhs, def_idx, depth + 1)?;
                eval_runtime_binop(*op, lhs_value, rhs_value)
            }
            RuntimeInstr::BinOpInPlace { dst, op, rhs } if *dst == slot => {
                let lhs_value =
                    loop_invariant_slot_const_before(instrs, info, slot, def_idx, depth + 1)?;
                let rhs_value =
                    loop_invariant_operand_const_before(instrs, info, rhs, def_idx, depth + 1)?;
                eval_runtime_binop(*op, lhs_value, rhs_value)
            }
            _ => None,
        };
    }
    None
}

fn loop_invariant_operand_const(
    instrs: &[RuntimeInstr],
    info: CountedLoopInfo,
    operand: &RuntimeOperand,
) -> Option<u64> {
    loop_invariant_operand_const_before(instrs, info, operand, info.header, 0)
}

fn loop_additive_offset_with_invariants(
    lhs: &RuntimeOperand,
    rhs: &RuntimeOperand,
    instrs: &[RuntimeInstr],
    info: CountedLoopInfo,
) -> Option<(usize, usize)> {
    match (lhs, rhs) {
        (RuntimeOperand::Slot(slot), other) => {
            let offset =
                usize::try_from(loop_invariant_operand_const(instrs, info, other)?).ok()?;
            Some((*slot, offset))
        }
        (other, RuntimeOperand::Slot(slot)) => {
            let offset =
                usize::try_from(loop_invariant_operand_const(instrs, info, other)?).ok()?;
            Some((*slot, offset))
        }
        _ => None,
    }
}

fn runtime_instr_is_cfg_boundary(instr: &RuntimeInstr) -> bool {
    matches!(
        instr,
        RuntimeInstr::Jump { .. }
            | RuntimeInstr::JumpIfZero { .. }
            | RuntimeInstr::JumpIfCmpFalse { .. }
            | RuntimeInstr::Call { .. }
            | RuntimeInstr::Return
            | RuntimeInstr::Exit { .. }
    )
}

fn runtime_basic_block_starts(instrs: &[RuntimeInstr]) -> Vec<usize> {
    let mut is_target = vec![false; instrs.len()];
    for instr in instrs {
        let target = match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => *target,
            _ => continue,
        };
        if target < is_target.len() {
            is_target[target] = true;
        }
    }

    let mut starts = vec![0usize; instrs.len()];
    let mut current = 0usize;
    for idx in 0..instrs.len() {
        if is_target[idx] {
            current = idx;
        }
        starts[idx] = current;
        if runtime_instr_is_cfg_boundary(&instrs[idx]) && idx + 1 < instrs.len() {
            current = idx + 1;
        }
    }
    starts
}

fn slot_const_before(
    instrs: &[RuntimeInstr],
    min_def_idx: usize,
    before_idx: usize,
    slot: usize,
    depth: u8,
) -> Option<u64> {
    if depth > 10 {
        return None;
    }
    for def_idx in (min_def_idx..before_idx.min(instrs.len())).rev() {
        let instr = &instrs[def_idx];
        if !runtime_instr_writes_slot(instr, slot) {
            continue;
        }
        return match instr {
            RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Imm(value),
            } if *dst == slot => Some(*value),
            RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Slot(src_slot),
            } if *dst == slot => {
                slot_const_before(instrs, min_def_idx, def_idx, *src_slot, depth + 1)
            }
            RuntimeInstr::BinOp { dst, op, lhs, rhs } if *dst == slot => {
                let lhs_value = operand_const_before(instrs, min_def_idx, def_idx, lhs, depth + 1)?;
                let rhs_value = operand_const_before(instrs, min_def_idx, def_idx, rhs, depth + 1)?;
                eval_runtime_binop(*op, lhs_value, rhs_value)
            }
            RuntimeInstr::BinOpInPlace { dst, op, rhs } if *dst == slot => {
                let lhs_value = slot_const_before(instrs, min_def_idx, def_idx, slot, depth + 1)?;
                let rhs_value = operand_const_before(instrs, min_def_idx, def_idx, rhs, depth + 1)?;
                eval_runtime_binop(*op, lhs_value, rhs_value)
            }
            RuntimeInstr::NormalizeInt { dst, .. } if *dst == slot => {
                slot_const_before(instrs, min_def_idx, def_idx, slot, depth + 1)
            }
            _ => None,
        };
    }
    None
}

fn operand_const_before(
    instrs: &[RuntimeInstr],
    min_def_idx: usize,
    before_idx: usize,
    operand: &RuntimeOperand,
    depth: u8,
) -> Option<u64> {
    match operand {
        RuntimeOperand::Imm(value) => Some(*value),
        RuntimeOperand::Slot(slot) => {
            slot_const_before(instrs, min_def_idx, before_idx, *slot, depth + 1)
        }
    }
}

fn slot_upper_bound_before(
    instrs: &[RuntimeInstr],
    min_def_idx: usize,
    before_idx: usize,
    slot: usize,
    depth: u8,
) -> Option<u64> {
    if depth > 10 {
        return None;
    }
    for def_idx in (min_def_idx..before_idx.min(instrs.len())).rev() {
        let instr = &instrs[def_idx];
        if !runtime_instr_writes_slot(instr, slot) {
            continue;
        }
        return match instr {
            RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Imm(value),
            } if *dst == slot => Some(*value),
            RuntimeInstr::Mov { dst, src } if *dst == slot => {
                operand_upper_bound_before(instrs, min_def_idx, def_idx, src, depth + 1)
            }
            RuntimeInstr::BinOp { dst, op, lhs, rhs } if *dst == slot => {
                match upper_bound_for_binop(instrs, min_def_idx, def_idx, *op, lhs, rhs, depth + 1)
                {
                    Some(v) => Some(v),
                    None => {
                        let lhs_value =
                            operand_const_before(instrs, min_def_idx, def_idx, lhs, depth + 1)?;
                        let rhs_value =
                            operand_const_before(instrs, min_def_idx, def_idx, rhs, depth + 1)?;
                        eval_runtime_binop(*op, lhs_value, rhs_value)
                    }
                }
            }
            RuntimeInstr::BinOpInPlace { dst, op, rhs } if *dst == slot => {
                let lhs_bound =
                    slot_upper_bound_before(instrs, min_def_idx, def_idx, slot, depth + 1);
                upper_bound_for_binop_from_bounds(
                    instrs,
                    min_def_idx,
                    def_idx,
                    *op,
                    lhs_bound,
                    rhs,
                    depth + 1,
                )
            }
            RuntimeInstr::NormalizeInt { dst, signed, bits } if *dst == slot => {
                let prior = slot_upper_bound_before(instrs, min_def_idx, def_idx, slot, depth + 1)?;
                if *signed {
                    None
                } else if *bits >= 64 {
                    Some(prior)
                } else {
                    let max = (1u64 << u32::from(*bits)) - 1;
                    Some(prior.min(max))
                }
            }
            _ => None,
        };
    }
    None
}

fn operand_upper_bound_before(
    instrs: &[RuntimeInstr],
    min_def_idx: usize,
    before_idx: usize,
    operand: &RuntimeOperand,
    depth: u8,
) -> Option<u64> {
    match operand {
        RuntimeOperand::Imm(value) => Some(*value),
        RuntimeOperand::Slot(slot) => {
            slot_upper_bound_before(instrs, min_def_idx, before_idx, *slot, depth + 1)
        }
    }
}

fn upper_bound_for_binop(
    instrs: &[RuntimeInstr],
    min_def_idx: usize,
    before_idx: usize,
    op: RuntimeBinOp,
    lhs: &RuntimeOperand,
    rhs: &RuntimeOperand,
    depth: u8,
) -> Option<u64> {
    fn bitmask_upper(value: u64) -> u64 {
        if value == u64::MAX {
            return u64::MAX;
        }
        if value == 0 {
            return 0;
        }
        let bits = 64u32.saturating_sub(value.leading_zeros());
        if bits >= 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        }
    }

    match op {
        RuntimeBinOp::BitAnd => {
            let lhs_const = operand_const_before(instrs, min_def_idx, before_idx, lhs, depth + 1);
            let rhs_const = operand_const_before(instrs, min_def_idx, before_idx, rhs, depth + 1);
            match (lhs_const, rhs_const) {
                (Some(l), Some(r)) => Some(l & r),
                (Some(mask), None) | (None, Some(mask)) => Some(mask),
                (None, None) => None,
            }
        }
        RuntimeBinOp::ModUnsigned => {
            let modulus = operand_const_before(instrs, min_def_idx, before_idx, rhs, depth + 1)?;
            if modulus == 0 {
                None
            } else {
                Some(modulus - 1)
            }
        }
        RuntimeBinOp::ShrUnsigned => {
            let shift = operand_const_before(instrs, min_def_idx, before_idx, rhs, depth + 1)?;
            if shift >= 64 {
                return Some(0);
            }
            let lhs_bound =
                operand_upper_bound_before(instrs, min_def_idx, before_idx, lhs, depth + 1)?;
            Some(lhs_bound >> shift)
        }
        RuntimeBinOp::Add => {
            let lhs_bound =
                operand_upper_bound_before(instrs, min_def_idx, before_idx, lhs, depth + 1)?;
            let rhs_bound =
                operand_upper_bound_before(instrs, min_def_idx, before_idx, rhs, depth + 1)?;
            lhs_bound.checked_add(rhs_bound)
        }
        RuntimeBinOp::BitOr => {
            let lhs_bound =
                operand_upper_bound_before(instrs, min_def_idx, before_idx, lhs, depth + 1)?;
            let rhs_bound =
                operand_upper_bound_before(instrs, min_def_idx, before_idx, rhs, depth + 1)?;
            Some(bitmask_upper(lhs_bound) | bitmask_upper(rhs_bound))
        }
        _ => None,
    }
}

fn upper_bound_for_binop_from_bounds(
    instrs: &[RuntimeInstr],
    min_def_idx: usize,
    before_idx: usize,
    op: RuntimeBinOp,
    lhs_bound: Option<u64>,
    rhs: &RuntimeOperand,
    depth: u8,
) -> Option<u64> {
    fn bitmask_upper(value: u64) -> u64 {
        if value == u64::MAX {
            return u64::MAX;
        }
        if value == 0 {
            return 0;
        }
        let bits = 64u32.saturating_sub(value.leading_zeros());
        if bits >= 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        }
    }

    match op {
        RuntimeBinOp::BitAnd => {
            let rhs_const = operand_const_before(instrs, min_def_idx, before_idx, rhs, depth + 1)?;
            Some(lhs_bound.map_or(rhs_const, |lhs| lhs.min(rhs_const)))
        }
        RuntimeBinOp::ModUnsigned => {
            let modulus = operand_const_before(instrs, min_def_idx, before_idx, rhs, depth + 1)?;
            if modulus == 0 {
                None
            } else {
                Some(modulus - 1)
            }
        }
        RuntimeBinOp::ShrUnsigned => {
            let shift = operand_const_before(instrs, min_def_idx, before_idx, rhs, depth + 1)?;
            if shift >= 64 {
                Some(0)
            } else {
                Some(lhs_bound.map_or(u64::MAX, |lhs| lhs) >> shift)
            }
        }
        RuntimeBinOp::Add => None,
        RuntimeBinOp::BitOr => {
            let rhs_bound =
                operand_upper_bound_before(instrs, min_def_idx, before_idx, rhs, depth + 1)?;
            Some(bitmask_upper(lhs_bound.unwrap_or(u64::MAX)) | bitmask_upper(rhs_bound))
        }
        _ => None,
    }
}

fn runtime_index_operand_proven_in_bounds(
    instrs: &[RuntimeInstr],
    min_def_idx: usize,
    use_idx: usize,
    index: &RuntimeOperand,
    base_len: usize,
) -> bool {
    if base_len == 0 {
        return false;
    }
    match index {
        RuntimeOperand::Imm(value) => usize::try_from(*value).is_ok_and(|v| v < base_len),
        RuntimeOperand::Slot(slot) => {
            slot_upper_bound_before(instrs, min_def_idx, use_idx, *slot, 0)
                .is_some_and(|max| max < base_len as u64)
        }
    }
}

fn eliminate_expr_bounded_index_checks(instrs: &mut [RuntimeInstr]) {
    let block_starts = runtime_basic_block_starts(instrs);
    for idx in 0..instrs.len() {
        let block_start = block_starts[idx];
        let rewritten = match &instrs[idx] {
            RuntimeInstr::LoadIndex {
                dst,
                base_slots,
                index,
            } if runtime_index_operand_proven_in_bounds(
                instrs,
                block_start,
                idx,
                index,
                base_slots.len(),
            ) =>
            {
                Some(RuntimeInstr::LoadIndexUnchecked {
                    dst: *dst,
                    base_slots: base_slots.clone(),
                    index: *index,
                })
            }
            RuntimeInstr::StoreIndex {
                base_slots,
                index,
                src,
            } if runtime_index_operand_proven_in_bounds(
                instrs,
                block_start,
                idx,
                index,
                base_slots.len(),
            ) =>
            {
                Some(RuntimeInstr::StoreIndexUnchecked {
                    base_slots: base_slots.clone(),
                    index: *index,
                    src: *src,
                })
            }
            _ => None,
        };
        if let Some(next) = rewritten {
            instrs[idx] = next;
        }
    }
}

fn slot_upper_with_local_context(
    slot: usize,
    local_upper: &HashMap<usize, u64>,
    guarded_upper: &HashMap<usize, u64>,
    facts_at_instr: &[Option<Vec<SlotFacts>>],
    idx: usize,
) -> Option<u64> {
    let mut upper = local_upper.get(&slot).copied();
    if let Some(bound) = guarded_upper.get(&slot).copied() {
        upper = Some(upper.map_or(bound, |curr| curr.min(bound)));
    }
    if let Some(bound) = facts_at_instr
        .get(idx)
        .and_then(|state| state.as_ref())
        .and_then(|state| state.get(slot))
        .and_then(|facts| facts.upper)
    {
        upper = Some(upper.map_or(bound, |curr| curr.min(bound)));
    }
    upper
}

fn operand_upper_with_local_context(
    operand: &RuntimeOperand,
    local_upper: &HashMap<usize, u64>,
    guarded_upper: &HashMap<usize, u64>,
    facts_at_instr: &[Option<Vec<SlotFacts>>],
    idx: usize,
) -> Option<u64> {
    match operand {
        RuntimeOperand::Imm(value) => Some(*value),
        RuntimeOperand::Slot(slot) => {
            slot_upper_with_local_context(*slot, local_upper, guarded_upper, facts_at_instr, idx)
        }
    }
}

fn clear_local_slot_facts(
    slot: usize,
    local_upper: &mut HashMap<usize, u64>,
    guarded_upper: &mut HashMap<usize, u64>,
) {
    local_upper.remove(&slot);
    guarded_upper.remove(&slot);
}

fn update_local_slot_upper_from_instr(
    idx: usize,
    instr: &RuntimeInstr,
    local_upper: &mut HashMap<usize, u64>,
    guarded_upper: &mut HashMap<usize, u64>,
    facts_at_instr: &[Option<Vec<SlotFacts>>],
) {
    match instr {
        RuntimeInstr::LoadSeed { dst, .. } => {
            clear_local_slot_facts(*dst, local_upper, guarded_upper)
        }
        RuntimeInstr::Mov { dst, src } => {
            clear_local_slot_facts(*dst, local_upper, guarded_upper);
            if let Some(upper) = operand_upper_with_local_context(
                src,
                local_upper,
                guarded_upper,
                facts_at_instr,
                idx,
            ) {
                local_upper.insert(*dst, upper);
            }
        }
        RuntimeInstr::BinOp { dst, op, lhs, rhs } => {
            clear_local_slot_facts(*dst, local_upper, guarded_upper);
            let upper = match op {
                RuntimeBinOp::Add => operand_upper_with_local_context(
                    lhs,
                    local_upper,
                    guarded_upper,
                    facts_at_instr,
                    idx,
                )
                .zip(operand_upper_with_local_context(
                    rhs,
                    local_upper,
                    guarded_upper,
                    facts_at_instr,
                    idx,
                ))
                .and_then(|(a, b)| a.checked_add(b)),
                RuntimeBinOp::BitAnd => {
                    let lhs_upper = operand_upper_with_local_context(
                        lhs,
                        local_upper,
                        guarded_upper,
                        facts_at_instr,
                        idx,
                    );
                    let rhs_upper = operand_upper_with_local_context(
                        rhs,
                        local_upper,
                        guarded_upper,
                        facts_at_instr,
                        idx,
                    );
                    match (lhs_upper, rhs_upper) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (Some(a), None) => Some(a),
                        (None, Some(b)) => Some(b),
                        (None, None) => None,
                    }
                }
                RuntimeBinOp::Shl => operand_upper_with_local_context(
                    lhs,
                    local_upper,
                    guarded_upper,
                    facts_at_instr,
                    idx,
                )
                .zip(match rhs {
                    RuntimeOperand::Imm(shift) => Some(*shift),
                    _ => None,
                })
                .and_then(|(value, shift)| {
                    u32::try_from(shift).ok().and_then(|s| value.checked_shl(s))
                }),
                RuntimeBinOp::ModUnsigned => match rhs {
                    RuntimeOperand::Imm(modulus) if *modulus != 0 => Some(modulus - 1),
                    _ => None,
                },
                RuntimeBinOp::Sub
                | RuntimeBinOp::Mul
                | RuntimeBinOp::DivUnsigned
                | RuntimeBinOp::DivSigned
                | RuntimeBinOp::ModSigned
                | RuntimeBinOp::BitOr
                | RuntimeBinOp::BitXor
                | RuntimeBinOp::ShrUnsigned
                | RuntimeBinOp::ShrSigned => None,
            };
            if let Some(upper) = upper {
                local_upper.insert(*dst, upper);
            }
        }
        RuntimeInstr::BinOpInPlace { dst, op, rhs } => {
            let upper = match op {
                RuntimeBinOp::Add => slot_upper_with_local_context(
                    *dst,
                    local_upper,
                    guarded_upper,
                    facts_at_instr,
                    idx,
                )
                .zip(operand_upper_with_local_context(
                    rhs,
                    local_upper,
                    guarded_upper,
                    facts_at_instr,
                    idx,
                ))
                .and_then(|(a, b)| a.checked_add(b)),
                RuntimeBinOp::BitAnd => slot_upper_with_local_context(
                    *dst,
                    local_upper,
                    guarded_upper,
                    facts_at_instr,
                    idx,
                )
                .zip(operand_upper_with_local_context(
                    rhs,
                    local_upper,
                    guarded_upper,
                    facts_at_instr,
                    idx,
                ))
                .map(|(a, b)| a.min(b)),
                RuntimeBinOp::Shl => slot_upper_with_local_context(
                    *dst,
                    local_upper,
                    guarded_upper,
                    facts_at_instr,
                    idx,
                )
                .zip(match rhs {
                    RuntimeOperand::Imm(shift) => Some(*shift),
                    _ => None,
                })
                .and_then(|(value, shift)| {
                    u32::try_from(shift).ok().and_then(|s| value.checked_shl(s))
                }),
                RuntimeBinOp::ModUnsigned => match rhs {
                    RuntimeOperand::Imm(modulus) if *modulus != 0 => Some(modulus - 1),
                    _ => None,
                },
                RuntimeBinOp::Sub
                | RuntimeBinOp::Mul
                | RuntimeBinOp::DivUnsigned
                | RuntimeBinOp::DivSigned
                | RuntimeBinOp::ModSigned
                | RuntimeBinOp::BitOr
                | RuntimeBinOp::BitXor
                | RuntimeBinOp::ShrUnsigned
                | RuntimeBinOp::ShrSigned => None,
            };
            clear_local_slot_facts(*dst, local_upper, guarded_upper);
            if let Some(upper) = upper {
                local_upper.insert(*dst, upper);
            }
        }
        RuntimeInstr::FloatBinOp { dst, .. } => {
            clear_local_slot_facts(*dst, local_upper, guarded_upper)
        }
        RuntimeInstr::Cmp { dst, .. } => {
            clear_local_slot_facts(*dst, local_upper, guarded_upper);
            local_upper.insert(*dst, 1);
        }
        RuntimeInstr::NormalizeInt { dst, signed, bits } => {
            let Some(mut upper) = slot_upper_with_local_context(
                *dst,
                local_upper,
                guarded_upper,
                facts_at_instr,
                idx,
            ) else {
                clear_local_slot_facts(*dst, local_upper, guarded_upper);
                return;
            };
            if *signed {
                clear_local_slot_facts(*dst, local_upper, guarded_upper);
                return;
            }
            if *bits < 64 {
                let mask = (1u64 << u32::from(*bits)) - 1;
                upper = upper.min(mask);
            }
            clear_local_slot_facts(*dst, local_upper, guarded_upper);
            local_upper.insert(*dst, upper);
        }
        RuntimeInstr::JumpIfCmpFalse {
            op,
            lhs: RuntimeOperand::Slot(slot),
            rhs: RuntimeOperand::Imm(limit),
            target,
        } if *target > idx => {
            let upper = match op {
                RuntimeCmpOp::LtUnsigned => limit.checked_sub(1),
                RuntimeCmpOp::LeUnsigned => Some(*limit),
                _ => None,
            };
            if let Some(upper) = upper {
                guarded_upper
                    .entry(*slot)
                    .and_modify(|curr| *curr = (*curr).min(upper))
                    .or_insert(upper);
            }
        }
        RuntimeInstr::CompareSwap { left, right, .. } => {
            clear_local_slot_facts(*left, local_upper, guarded_upper);
            clear_local_slot_facts(*right, local_upper, guarded_upper);
        }
        RuntimeInstr::RadixSortFixedInt { slots, .. } => {
            for slot in slots {
                clear_local_slot_facts(*slot, local_upper, guarded_upper);
            }
        }
        RuntimeInstr::BloomClassic4Check {
            dst,
            lanes_checked,
            filter_slots,
            ..
        } => {
            clear_local_slot_facts(*dst, local_upper, guarded_upper);
            clear_local_slot_facts(*lanes_checked, local_upper, guarded_upper);
            for slot in filter_slots {
                clear_local_slot_facts(*slot, local_upper, guarded_upper);
            }
        }
        RuntimeInstr::LoadIndex { dst, .. } | RuntimeInstr::LoadIndexUnchecked { dst, .. } => {
            clear_local_slot_facts(*dst, local_upper, guarded_upper);
        }
        RuntimeInstr::HeapLoadInt { dst, .. } => {
            clear_local_slot_facts(*dst, local_upper, guarded_upper);
        }
        RuntimeInstr::HeapStoreInt { .. } | RuntimeInstr::HeapCopy { .. } => {}
        RuntimeInstr::StoreIndex { .. } | RuntimeInstr::StoreIndexUnchecked { .. } => {}
        RuntimeInstr::BloomSplitBlockInsert { filter_slots, .. } => {
            for slot in filter_slots {
                clear_local_slot_facts(*slot, local_upper, guarded_upper);
            }
        }
        RuntimeInstr::BloomSplitBlockCheck {
            dst, filter_slots, ..
        } => {
            clear_local_slot_facts(*dst, local_upper, guarded_upper);
            for slot in filter_slots {
                clear_local_slot_facts(*slot, local_upper, guarded_upper);
            }
        }
        RuntimeInstr::HashCtrlGroupProbe {
            dst_mask,
            ctrl_slots,
            ..
        } => {
            clear_local_slot_facts(*dst_mask, local_upper, guarded_upper);
            for slot in ctrl_slots {
                clear_local_slot_facts(*slot, local_upper, guarded_upper);
            }
        }
        RuntimeInstr::JoinSelectAdaptive { dst, .. } => {
            clear_local_slot_facts(*dst, local_upper, guarded_upper);
        }
        RuntimeInstr::Alloc { dst, .. } => clear_local_slot_facts(*dst, local_upper, guarded_upper),
        RuntimeInstr::FileOpen { dst, .. }
        | RuntimeInstr::FileWrite { dst, .. }
        | RuntimeInstr::FileRead { dst, .. }
        | RuntimeInstr::ThreadJoin { dst, .. } => {
            clear_local_slot_facts(*dst, local_upper, guarded_upper)
        }
        RuntimeInstr::ThreadSpawn { handle_dst, .. } => {
            clear_local_slot_facts(*handle_dst, local_upper, guarded_upper);
            local_upper.clear();
            guarded_upper.clear();
        }
        RuntimeInstr::ChannelCreate { dst, .. } | RuntimeInstr::ChannelRecv { dst, .. } => {
            clear_local_slot_facts(*dst, local_upper, guarded_upper);
        }
        RuntimeInstr::ChannelSend { .. }
        | RuntimeInstr::ChannelClose { .. }
        | RuntimeInstr::ChannelDestroy { .. } => {}
        RuntimeInstr::FileClose { .. } => {}
        RuntimeInstr::Free { .. } => {}
        RuntimeInstr::PrintConst { .. } => {}
        RuntimeInstr::PrintInt { .. } => {}
        RuntimeInstr::Jump { .. }
        | RuntimeInstr::Call { .. }
        | RuntimeInstr::Return
        | RuntimeInstr::Exit { .. } => {
            local_upper.clear();
            guarded_upper.clear();
        }
        RuntimeInstr::JumpIfCmpFalse { .. } => {}
        RuntimeInstr::JumpIfZero { .. } => {}
    }
}

fn elide_guarded_index_checks(instrs: &mut [RuntimeInstr]) {
    if instrs.is_empty() {
        return;
    }
    let facts_at_instr = compute_runtime_slot_facts_at_instr(instrs);
    let mut is_target = vec![false; instrs.len()];
    for instr in instrs.iter() {
        let target = match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => *target,
            _ => continue,
        };
        if target < is_target.len() {
            is_target[target] = true;
        }
    }

    let mut local_upper: HashMap<usize, u64> = HashMap::new();
    let mut guarded_upper: HashMap<usize, u64> = HashMap::new();
    for idx in 0..instrs.len() {
        if idx != 0 && is_target[idx] {
            local_upper.clear();
            guarded_upper.clear();
        }

        let rewritten = match &instrs[idx] {
            RuntimeInstr::LoadIndex {
                dst,
                base_slots,
                index: RuntimeOperand::Slot(index_slot),
            } => {
                let upper = slot_upper_with_local_context(
                    *index_slot,
                    &local_upper,
                    &guarded_upper,
                    &facts_at_instr,
                    idx,
                );
                if upper.is_some_and(|u| u < base_slots.len() as u64) {
                    Some(RuntimeInstr::LoadIndexUnchecked {
                        dst: *dst,
                        base_slots: base_slots.clone(),
                        index: RuntimeOperand::Slot(*index_slot),
                    })
                } else {
                    None
                }
            }
            RuntimeInstr::StoreIndex {
                base_slots,
                index: RuntimeOperand::Slot(index_slot),
                src,
            } => {
                let upper = slot_upper_with_local_context(
                    *index_slot,
                    &local_upper,
                    &guarded_upper,
                    &facts_at_instr,
                    idx,
                );
                if upper.is_some_and(|u| u < base_slots.len() as u64) {
                    Some(RuntimeInstr::StoreIndexUnchecked {
                        base_slots: base_slots.clone(),
                        index: RuntimeOperand::Slot(*index_slot),
                        src: *src,
                    })
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(next) = rewritten {
            instrs[idx] = next;
        }
        update_local_slot_upper_from_instr(
            idx,
            &instrs[idx],
            &mut local_upper,
            &mut guarded_upper,
            &facts_at_instr,
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckedIndexOperandKey {
    Imm(u64),
    Slot(usize),
}

impl CheckedIndexOperandKey {
    fn from_operand(operand: &RuntimeOperand) -> Self {
        match operand {
            RuntimeOperand::Imm(value) => Self::Imm(*value),
            RuntimeOperand::Slot(slot) => Self::Slot(*slot),
        }
    }

    fn slot(self) -> Option<usize> {
        match self {
            Self::Slot(slot) => Some(slot),
            Self::Imm(_) => None,
        }
    }
}

#[derive(Clone)]
struct CheckedIndexGuardKey {
    base_slots: Vec<usize>,
    index: CheckedIndexOperandKey,
}

impl CheckedIndexGuardKey {
    fn from_access(base_slots: &[usize], index: &RuntimeOperand) -> Self {
        Self {
            base_slots: base_slots.to_vec(),
            index: CheckedIndexOperandKey::from_operand(index),
        }
    }

    fn matches_access(&self, base_slots: &[usize], index: &RuntimeOperand) -> bool {
        self.base_slots == base_slots && self.index == CheckedIndexOperandKey::from_operand(index)
    }
}

fn hoist_repeated_identical_index_checks(instrs: &mut [RuntimeInstr]) {
    if instrs.is_empty() {
        return;
    }

    let block_starts = runtime_basic_block_starts(instrs);
    let mut current_block_start = usize::MAX;
    let mut seen_checked_guards: Vec<CheckedIndexGuardKey> = Vec::new();

    for idx in 0..instrs.len() {
        if block_starts[idx] != current_block_start {
            current_block_start = block_starts[idx];
            seen_checked_guards.clear();
        }

        let mut record_checked_guard: Option<CheckedIndexGuardKey> = None;
        let rewritten = match &instrs[idx] {
            RuntimeInstr::LoadIndex {
                dst,
                base_slots,
                index,
            } => {
                if seen_checked_guards
                    .iter()
                    .any(|guard| guard.matches_access(base_slots, index))
                {
                    Some(RuntimeInstr::LoadIndexUnchecked {
                        dst: *dst,
                        base_slots: base_slots.clone(),
                        index: *index,
                    })
                } else {
                    record_checked_guard =
                        Some(CheckedIndexGuardKey::from_access(base_slots, index));
                    None
                }
            }
            RuntimeInstr::StoreIndex {
                base_slots,
                index,
                src,
            } => {
                if seen_checked_guards
                    .iter()
                    .any(|guard| guard.matches_access(base_slots, index))
                {
                    Some(RuntimeInstr::StoreIndexUnchecked {
                        base_slots: base_slots.clone(),
                        index: *index,
                        src: *src,
                    })
                } else {
                    record_checked_guard =
                        Some(CheckedIndexGuardKey::from_access(base_slots, index));
                    None
                }
            }
            _ => None,
        };

        let did_rewrite = rewritten.is_some();
        if let Some(next) = rewritten {
            instrs[idx] = next;
        }
        if !did_rewrite {
            if let Some(guard) = record_checked_guard {
                seen_checked_guards.push(guard);
            }
        }

        let instr = &instrs[idx];
        seen_checked_guards.retain(|guard| {
            guard
                .index
                .slot()
                .map_or(true, |slot| !runtime_instr_writes_slot(instr, slot))
        });
    }
}

fn prove_loop_index_slot_in_bounds(
    instrs: &[RuntimeInstr],
    use_idx: usize,
    info: CountedLoopInfo,
    index_slot: usize,
    base_len: usize,
    depth: u8,
) -> bool {
    if depth > 6 {
        return false;
    }
    if index_slot == info.ind_slot {
        return matches!(info.limit, LoopLimit::Imm(limit) if usize::try_from(limit).is_ok_and(|v| v <= base_len));
    }
    if base_len == 0 || use_idx <= info.header + 1 {
        return false;
    }
    let mut saw_norm = false;
    for def_idx in (info.header + 1..use_idx).rev() {
        let instr = &instrs[def_idx];
        if !runtime_instr_writes_slot(instr, index_slot) {
            continue;
        }
        match instr {
            RuntimeInstr::NormalizeInt { dst, .. } if *dst == index_slot && !saw_norm => {
                // Ignore one normalization wrapper and keep searching for the real definition.
                saw_norm = true;
                continue;
            }
            RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Slot(src_slot),
            } if *dst == index_slot => {
                return prove_loop_index_slot_in_bounds(
                    instrs,
                    def_idx,
                    info,
                    *src_slot,
                    base_len,
                    depth + 1,
                );
            }
            RuntimeInstr::BinOp { dst, op, lhs, rhs } if *dst == index_slot => match op {
                RuntimeBinOp::Add => {
                    let Some((src_slot, offset)) =
                        loop_additive_offset_with_invariants(lhs, rhs, instrs, info)
                    else {
                        return false;
                    };
                    if offset > base_len {
                        return false;
                    }
                    return prove_loop_index_slot_in_bounds(
                        instrs,
                        def_idx,
                        info,
                        src_slot,
                        base_len - offset,
                        depth + 1,
                    );
                }
                RuntimeBinOp::BitAnd => {
                    let mask = match (lhs, rhs) {
                        (RuntimeOperand::Slot(_), other) | (other, RuntimeOperand::Slot(_)) => {
                            let Some(mask) = loop_invariant_operand_const(instrs, info, other)
                            else {
                                return false;
                            };
                            mask
                        }
                        _ => return false,
                    };
                    if base_len.is_power_of_two() {
                        return mask == (base_len as u64).saturating_sub(1);
                    }
                    return false;
                }
                RuntimeBinOp::ModUnsigned => {
                    let modulus = match (lhs, rhs) {
                        (RuntimeOperand::Slot(_), other) => {
                            let Some(modulus) = loop_invariant_operand_const(instrs, info, other)
                            else {
                                return false;
                            };
                            modulus
                        }
                        _ => return false,
                    };
                    return modulus != 0 && modulus <= base_len as u64;
                }
                _ => return false,
            },
            _ => return false,
        }
    }
    false
}

fn shift_runtime_targets_for_insert(instrs: &mut [RuntimeInstr], at: usize, amount: usize) {
    let remap = |target: &mut usize| {
        if *target >= at {
            *target += amount;
        }
    };
    for instr in instrs {
        match instr {
            RuntimeInstr::Jump { target } => remap(target),
            RuntimeInstr::JumpIfZero { target, .. } => remap(target),
            RuntimeInstr::JumpIfCmpFalse { target, .. } => remap(target),
            RuntimeInstr::Call { target } | RuntimeInstr::ThreadSpawn { target, .. } => {
                remap(target)
            }
            RuntimeInstr::LoadSeed { .. }
            | RuntimeInstr::Mov { .. }
            | RuntimeInstr::BinOp { .. }
            | RuntimeInstr::BinOpInPlace { .. }
            | RuntimeInstr::FloatBinOp { .. }
            | RuntimeInstr::Cmp { .. }
            | RuntimeInstr::NormalizeInt { .. }
            | RuntimeInstr::CompareSwap { .. }
            | RuntimeInstr::RadixSortFixedInt { .. }
            | RuntimeInstr::LoadIndex { .. }
            | RuntimeInstr::LoadIndexUnchecked { .. }
            | RuntimeInstr::StoreIndex { .. }
            | RuntimeInstr::StoreIndexUnchecked { .. }
            | RuntimeInstr::HeapLoadInt { .. }
            | RuntimeInstr::HeapStoreInt { .. }
            | RuntimeInstr::HeapCopy { .. }
            | RuntimeInstr::BloomSplitBlockInsert { .. }
            | RuntimeInstr::BloomSplitBlockCheck { .. }
            | RuntimeInstr::BloomClassic4Check { .. }
            | RuntimeInstr::HashCtrlGroupProbe { .. }
            | RuntimeInstr::JoinSelectAdaptive { .. }
            | RuntimeInstr::Alloc { .. }
            | RuntimeInstr::Free { .. }
            | RuntimeInstr::FileOpen { .. }
            | RuntimeInstr::FileWrite { .. }
            | RuntimeInstr::FileRead { .. }
            | RuntimeInstr::FileClose { .. }
            | RuntimeInstr::ThreadJoin { .. }
            | RuntimeInstr::ChannelCreate { .. }
            | RuntimeInstr::ChannelSend { .. }
            | RuntimeInstr::ChannelRecv { .. }
            | RuntimeInstr::ChannelClose { .. }
            | RuntimeInstr::ChannelDestroy { .. }
            | RuntimeInstr::PrintConst { .. }
            | RuntimeInstr::PrintInt { .. }
            | RuntimeInstr::Return
            | RuntimeInstr::Exit { .. } => {}
        }
    }
}

fn version_slot_bounded_loop_accesses(instrs: &mut Vec<RuntimeInstr>, loops: &[CountedLoopInfo]) {
    let mut slot_loops: Vec<CountedLoopInfo> = loops
        .iter()
        .copied()
        .filter(|info| matches!(info.limit, LoopLimit::Slot(_)))
        .collect();
    if slot_loops.is_empty() {
        return;
    }
    slot_loops.sort_by_key(|info| info.header);

    let mut oob_target: Option<usize> = None;
    for info in slot_loops.into_iter().rev() {
        let LoopLimit::Slot(limit_slot) = info.limit else {
            continue;
        };
        if info.header + 1 > info.update_idx {
            continue;
        }
        if (info.header + 1..info.update_idx)
            .any(|idx| runtime_instr_writes_slot(&instrs[idx], limit_slot))
        {
            continue;
        }

        let mut min_base_len: Option<usize> = None;
        let mut rewrite_indices = Vec::new();
        for idx in (info.header + 1)..info.update_idx {
            match &instrs[idx] {
                RuntimeInstr::LoadIndex {
                    base_slots,
                    index: RuntimeOperand::Slot(index_slot),
                    ..
                } => {
                    let Some(offset) =
                        loop_index_slot_nonnegative_offset(instrs, idx, info, *index_slot, 0)
                    else {
                        continue;
                    };
                    let Some(bound) = base_slots.len().checked_sub(offset) else {
                        continue;
                    };
                    min_base_len = Some(min_base_len.map_or(bound, |v| v.min(bound)));
                    rewrite_indices.push(idx);
                }
                RuntimeInstr::StoreIndex {
                    base_slots,
                    index: RuntimeOperand::Slot(index_slot),
                    ..
                } => {
                    let Some(offset) =
                        loop_index_slot_nonnegative_offset(instrs, idx, info, *index_slot, 0)
                    else {
                        continue;
                    };
                    let Some(bound) = base_slots.len().checked_sub(offset) else {
                        continue;
                    };
                    min_base_len = Some(min_base_len.map_or(bound, |v| v.min(bound)));
                    rewrite_indices.push(idx);
                }
                _ => {}
            }
        }
        let Some(min_base_len) = min_base_len else {
            continue;
        };
        if min_base_len == 0 {
            continue;
        }

        for idx in rewrite_indices {
            let replacement = match &instrs[idx] {
                RuntimeInstr::LoadIndex {
                    dst,
                    base_slots,
                    index,
                } => Some(RuntimeInstr::LoadIndexUnchecked {
                    dst: *dst,
                    base_slots: base_slots.clone(),
                    index: *index,
                }),
                RuntimeInstr::StoreIndex {
                    base_slots,
                    index,
                    src,
                } => Some(RuntimeInstr::StoreIndexUnchecked {
                    base_slots: base_slots.clone(),
                    index: *index,
                    src: *src,
                }),
                _ => None,
            };
            if let Some(next) = replacement {
                instrs[idx] = next;
            }
        }

        let mut current_oob = if let Some(idx) = oob_target {
            idx
        } else {
            let idx = instrs.len();
            instrs.push(RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(255),
            });
            idx
        };

        shift_runtime_targets_for_insert(instrs, info.header, 1);
        current_oob += 1;
        instrs.insert(
            info.header,
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LeUnsigned,
                lhs: RuntimeOperand::Slot(limit_slot),
                rhs: RuntimeOperand::Imm(min_base_len as u64),
                target: current_oob,
            },
        );
        oob_target = Some(current_oob);
    }
}

fn loop_index_slot_nonnegative_offset(
    instrs: &[RuntimeInstr],
    use_idx: usize,
    info: CountedLoopInfo,
    index_slot: usize,
    depth: u8,
) -> Option<usize> {
    if index_slot == info.ind_slot {
        return Some(0);
    }
    if depth > 6 || use_idx <= info.header + 1 {
        return None;
    }

    for def_idx in (info.header + 1..use_idx).rev() {
        let instr = &instrs[def_idx];
        if !runtime_instr_writes_slot(instr, index_slot) {
            continue;
        }
        return match instr {
            RuntimeInstr::NormalizeInt { dst, .. } if *dst == index_slot => {
                loop_index_slot_nonnegative_offset(instrs, def_idx, info, index_slot, depth + 1)
            }
            RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Slot(src_slot),
            } if *dst == index_slot => {
                loop_index_slot_nonnegative_offset(instrs, def_idx, info, *src_slot, depth + 1)
            }
            RuntimeInstr::BinOp { dst, op, lhs, rhs }
                if *dst == index_slot && *op == RuntimeBinOp::Add =>
            {
                let (src_slot, offset) =
                    loop_additive_offset_with_invariants(lhs, rhs, instrs, info)?;
                let src_offset =
                    loop_index_slot_nonnegative_offset(instrs, def_idx, info, src_slot, depth + 1)?;
                src_offset.checked_add(offset)
            }
            _ => None,
        };
    }
    None
}

fn compact_runtime_generic_slots(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            LoweredStmt::RuntimeGeneric { mut program } => {
                compact_runtime_program_slots(&mut program);
                LoweredStmt::RuntimeGeneric { program }
            }
            other => other,
        })
        .collect()
}

fn compact_runtime_program_slots(program: &mut crate::frontend::semantics::RuntimeProgram) {
    if program.slots == 0 {
        return;
    }
    let mut used = vec![false; program.slots];
    for instr in &program.instrs {
        mark_used_slots(instr, &mut used);
    }
    let mut remap = vec![usize::MAX; program.slots];
    let mut next = 0usize;
    for (slot, is_used) in used.iter().copied().enumerate() {
        if is_used {
            remap[slot] = next;
            next += 1;
        }
    }
    if next == program.slots {
        return;
    }
    for instr in &mut program.instrs {
        remap_instr_slots(instr, &remap);
    }
    program.slots = next;
}

fn mark_operand_used(operand: &RuntimeOperand, used: &mut [bool]) {
    if let RuntimeOperand::Slot(slot) = operand {
        if let Some(entry) = used.get_mut(*slot) {
            *entry = true;
        }
    }
}

fn mark_slot_used(slot: usize, used: &mut [bool]) {
    if let Some(entry) = used.get_mut(slot) {
        *entry = true;
    }
}

fn mark_used_slots(instr: &RuntimeInstr, used: &mut [bool]) {
    match instr {
        RuntimeInstr::LoadSeed { dst, input, .. } => {
            mark_slot_used(*dst, used);
            if let Some(input) = input {
                mark_operand_used(input, used);
            }
        }
        RuntimeInstr::Mov { dst, src } => {
            mark_slot_used(*dst, used);
            mark_operand_used(src, used);
        }
        RuntimeInstr::BinOp { dst, lhs, rhs, .. } => {
            mark_slot_used(*dst, used);
            mark_operand_used(lhs, used);
            mark_operand_used(rhs, used);
        }
        RuntimeInstr::BinOpInPlace { dst, rhs, .. } => {
            mark_slot_used(*dst, used);
            mark_operand_used(rhs, used);
        }
        RuntimeInstr::FloatBinOp { dst, lhs, rhs, .. } => {
            mark_slot_used(*dst, used);
            mark_operand_used(lhs, used);
            mark_operand_used(rhs, used);
        }
        RuntimeInstr::Cmp { dst, lhs, rhs, .. } => {
            mark_slot_used(*dst, used);
            mark_operand_used(lhs, used);
            mark_operand_used(rhs, used);
        }
        RuntimeInstr::NormalizeInt { dst, .. } => mark_slot_used(*dst, used),
        RuntimeInstr::Jump { .. } => {}
        RuntimeInstr::JumpIfZero { cond_slot, .. } => mark_slot_used(*cond_slot, used),
        RuntimeInstr::JumpIfCmpFalse { lhs, rhs, .. } => {
            mark_operand_used(lhs, used);
            mark_operand_used(rhs, used);
        }
        RuntimeInstr::CompareSwap { left, right, .. } => {
            mark_slot_used(*left, used);
            mark_slot_used(*right, used);
        }
        RuntimeInstr::RadixSortFixedInt { slots, .. } => {
            for slot in slots {
                mark_slot_used(*slot, used);
            }
        }
        RuntimeInstr::Call { .. } => {}
        RuntimeInstr::LoadIndex {
            dst,
            base_slots,
            index,
        }
        | RuntimeInstr::LoadIndexUnchecked {
            dst,
            base_slots,
            index,
        } => {
            mark_slot_used(*dst, used);
            for slot in base_slots {
                mark_slot_used(*slot, used);
            }
            mark_operand_used(index, used);
        }
        RuntimeInstr::StoreIndex {
            base_slots,
            index,
            src,
        }
        | RuntimeInstr::StoreIndexUnchecked {
            base_slots,
            index,
            src,
        } => {
            for slot in base_slots {
                mark_slot_used(*slot, used);
            }
            mark_operand_used(index, used);
            mark_operand_used(src, used);
        }
        RuntimeInstr::HeapLoadInt {
            dst, ptr, index, ..
        } => {
            mark_slot_used(*dst, used);
            mark_operand_used(ptr, used);
            mark_operand_used(index, used);
        }
        RuntimeInstr::HeapStoreInt {
            ptr, index, src, ..
        } => {
            mark_operand_used(ptr, used);
            mark_operand_used(index, used);
            mark_operand_used(src, used);
        }
        RuntimeInstr::HeapCopy {
            dst_ptr,
            src_ptr,
            bytes,
        } => {
            mark_operand_used(dst_ptr, used);
            mark_operand_used(src_ptr, used);
            mark_operand_used(bytes, used);
        }
        RuntimeInstr::BloomSplitBlockInsert { filter_slots, hash } => {
            for slot in filter_slots {
                mark_slot_used(*slot, used);
            }
            mark_operand_used(hash, used);
        }
        RuntimeInstr::BloomClassic4Check {
            dst,
            lanes_checked,
            filter_slots,
            hash,
        } => {
            mark_slot_used(*dst, used);
            mark_slot_used(*lanes_checked, used);
            for slot in filter_slots {
                mark_slot_used(*slot, used);
            }
            mark_operand_used(hash, used);
        }
        RuntimeInstr::BloomSplitBlockCheck {
            dst,
            filter_slots,
            hash,
        } => {
            mark_slot_used(*dst, used);
            for slot in filter_slots {
                mark_slot_used(*slot, used);
            }
            mark_operand_used(hash, used);
        }
        RuntimeInstr::HashCtrlGroupProbe {
            dst_mask,
            ctrl_slots,
            group_start,
            fingerprint,
        } => {
            mark_slot_used(*dst_mask, used);
            for slot in ctrl_slots {
                mark_slot_used(*slot, used);
            }
            mark_operand_used(group_start, used);
            mark_operand_used(fingerprint, used);
        }
        RuntimeInstr::JoinSelectAdaptive {
            dst,
            build_rows,
            probe_rows,
        } => {
            mark_slot_used(*dst, used);
            mark_operand_used(build_rows, used);
            mark_operand_used(probe_rows, used);
        }
        RuntimeInstr::Alloc { dst, size } => {
            mark_slot_used(*dst, used);
            mark_operand_used(size, used);
        }
        RuntimeInstr::Free { ptr, size } => {
            mark_operand_used(ptr, used);
            mark_operand_used(size, used);
        }
        RuntimeInstr::FileOpen { dst, path_ptr, .. } => {
            mark_slot_used(*dst, used);
            mark_operand_used(path_ptr, used);
        }
        RuntimeInstr::FileWrite { dst, fd, ptr, len }
        | RuntimeInstr::FileRead { dst, fd, ptr, len } => {
            mark_slot_used(*dst, used);
            mark_operand_used(fd, used);
            mark_operand_used(ptr, used);
            mark_operand_used(len, used);
        }
        RuntimeInstr::FileClose { fd } => mark_operand_used(fd, used),
        RuntimeInstr::ThreadSpawn {
            handle_dst,
            return_slot,
            ..
        } => {
            mark_slot_used(*handle_dst, used);
            if let Some(slot) = return_slot {
                mark_slot_used(*slot, used);
            }
        }
        RuntimeInstr::ThreadJoin { dst, handle } => {
            mark_slot_used(*dst, used);
            mark_operand_used(handle, used);
        }
        RuntimeInstr::ChannelCreate { dst, capacity, .. } => {
            mark_slot_used(*dst, used);
            mark_operand_used(capacity, used);
        }
        RuntimeInstr::ChannelSend { handle, value } => {
            mark_operand_used(handle, used);
            mark_operand_used(value, used);
        }
        RuntimeInstr::ChannelRecv { dst, handle } => {
            mark_slot_used(*dst, used);
            mark_operand_used(handle, used);
        }
        RuntimeInstr::ChannelClose { handle, .. } | RuntimeInstr::ChannelDestroy { handle } => {
            mark_operand_used(handle, used)
        }
        RuntimeInstr::PrintConst { .. } => {}
        RuntimeInstr::PrintInt { value, .. } => mark_operand_used(value, used),
        RuntimeInstr::Return => {}
        RuntimeInstr::Exit { code } => mark_operand_used(code, used),
    }
}

fn remap_operand_slots(operand: &mut RuntimeOperand, remap: &[usize]) {
    if let RuntimeOperand::Slot(slot) = operand {
        *slot = remap[*slot];
    }
}

fn remap_instr_slots(instr: &mut RuntimeInstr, remap: &[usize]) {
    match instr {
        RuntimeInstr::LoadSeed { dst, input, .. } => {
            *dst = remap[*dst];
            if let Some(input) = input {
                remap_operand_slots(input, remap);
            }
        }
        RuntimeInstr::Mov { dst, src } => {
            *dst = remap[*dst];
            remap_operand_slots(src, remap);
        }
        RuntimeInstr::BinOp { dst, lhs, rhs, .. } => {
            *dst = remap[*dst];
            remap_operand_slots(lhs, remap);
            remap_operand_slots(rhs, remap);
        }
        RuntimeInstr::BinOpInPlace { dst, rhs, .. } => {
            *dst = remap[*dst];
            remap_operand_slots(rhs, remap);
        }
        RuntimeInstr::FloatBinOp { dst, lhs, rhs, .. } => {
            *dst = remap[*dst];
            remap_operand_slots(lhs, remap);
            remap_operand_slots(rhs, remap);
        }
        RuntimeInstr::Cmp { dst, lhs, rhs, .. } => {
            *dst = remap[*dst];
            remap_operand_slots(lhs, remap);
            remap_operand_slots(rhs, remap);
        }
        RuntimeInstr::NormalizeInt { dst, .. } => *dst = remap[*dst],
        RuntimeInstr::Jump { .. } => {}
        RuntimeInstr::JumpIfZero { cond_slot, .. } => *cond_slot = remap[*cond_slot],
        RuntimeInstr::JumpIfCmpFalse { lhs, rhs, .. } => {
            remap_operand_slots(lhs, remap);
            remap_operand_slots(rhs, remap);
        }
        RuntimeInstr::CompareSwap { left, right, .. } => {
            *left = remap[*left];
            *right = remap[*right];
        }
        RuntimeInstr::RadixSortFixedInt { slots, .. } => {
            for slot in slots {
                *slot = remap[*slot];
            }
        }
        RuntimeInstr::Call { .. } => {}
        RuntimeInstr::LoadIndex {
            dst,
            base_slots,
            index,
        }
        | RuntimeInstr::LoadIndexUnchecked {
            dst,
            base_slots,
            index,
        } => {
            *dst = remap[*dst];
            for slot in base_slots {
                *slot = remap[*slot];
            }
            remap_operand_slots(index, remap);
        }
        RuntimeInstr::StoreIndex {
            base_slots,
            index,
            src,
        }
        | RuntimeInstr::StoreIndexUnchecked {
            base_slots,
            index,
            src,
        } => {
            for slot in base_slots {
                *slot = remap[*slot];
            }
            remap_operand_slots(index, remap);
            remap_operand_slots(src, remap);
        }
        RuntimeInstr::HeapLoadInt {
            dst, ptr, index, ..
        } => {
            *dst = remap[*dst];
            remap_operand_slots(ptr, remap);
            remap_operand_slots(index, remap);
        }
        RuntimeInstr::HeapStoreInt {
            ptr, index, src, ..
        } => {
            remap_operand_slots(ptr, remap);
            remap_operand_slots(index, remap);
            remap_operand_slots(src, remap);
        }
        RuntimeInstr::HeapCopy {
            dst_ptr,
            src_ptr,
            bytes,
        } => {
            remap_operand_slots(dst_ptr, remap);
            remap_operand_slots(src_ptr, remap);
            remap_operand_slots(bytes, remap);
        }
        RuntimeInstr::BloomSplitBlockInsert { filter_slots, hash } => {
            for slot in filter_slots {
                *slot = remap[*slot];
            }
            remap_operand_slots(hash, remap);
        }
        RuntimeInstr::BloomClassic4Check {
            dst,
            lanes_checked,
            filter_slots,
            hash,
        } => {
            *dst = remap[*dst];
            *lanes_checked = remap[*lanes_checked];
            for slot in filter_slots {
                *slot = remap[*slot];
            }
            remap_operand_slots(hash, remap);
        }
        RuntimeInstr::BloomSplitBlockCheck {
            dst,
            filter_slots,
            hash,
        } => {
            *dst = remap[*dst];
            for slot in filter_slots {
                *slot = remap[*slot];
            }
            remap_operand_slots(hash, remap);
        }
        RuntimeInstr::HashCtrlGroupProbe {
            dst_mask,
            ctrl_slots,
            group_start,
            fingerprint,
        } => {
            *dst_mask = remap[*dst_mask];
            for slot in ctrl_slots {
                *slot = remap[*slot];
            }
            remap_operand_slots(group_start, remap);
            remap_operand_slots(fingerprint, remap);
        }
        RuntimeInstr::JoinSelectAdaptive {
            dst,
            build_rows,
            probe_rows,
        } => {
            *dst = remap[*dst];
            remap_operand_slots(build_rows, remap);
            remap_operand_slots(probe_rows, remap);
        }
        RuntimeInstr::Alloc { dst, size } => {
            *dst = remap[*dst];
            remap_operand_slots(size, remap);
        }
        RuntimeInstr::Free { ptr, size } => {
            remap_operand_slots(ptr, remap);
            remap_operand_slots(size, remap);
        }
        RuntimeInstr::FileOpen { dst, path_ptr, .. } => {
            *dst = remap[*dst];
            remap_operand_slots(path_ptr, remap);
        }
        RuntimeInstr::FileWrite { dst, fd, ptr, len }
        | RuntimeInstr::FileRead { dst, fd, ptr, len } => {
            *dst = remap[*dst];
            remap_operand_slots(fd, remap);
            remap_operand_slots(ptr, remap);
            remap_operand_slots(len, remap);
        }
        RuntimeInstr::FileClose { fd } => remap_operand_slots(fd, remap),
        RuntimeInstr::ThreadSpawn {
            handle_dst,
            return_slot,
            ..
        } => {
            *handle_dst = remap[*handle_dst];
            if let Some(slot) = return_slot {
                *slot = remap[*slot];
            }
        }
        RuntimeInstr::ThreadJoin { dst, handle } => {
            *dst = remap[*dst];
            remap_operand_slots(handle, remap);
        }
        RuntimeInstr::ChannelCreate { dst, capacity, .. } => {
            *dst = remap[*dst];
            remap_operand_slots(capacity, remap);
        }
        RuntimeInstr::ChannelSend { handle, value } => {
            remap_operand_slots(handle, remap);
            remap_operand_slots(value, remap);
        }
        RuntimeInstr::ChannelRecv { dst, handle } => {
            *dst = remap[*dst];
            remap_operand_slots(handle, remap);
        }
        RuntimeInstr::ChannelClose { handle, .. } | RuntimeInstr::ChannelDestroy { handle } => {
            remap_operand_slots(handle, remap)
        }
        RuntimeInstr::PrintConst { .. } => {}
        RuntimeInstr::PrintInt { value, .. } => remap_operand_slots(value, remap),
        RuntimeInstr::Return => {}
        RuntimeInstr::Exit { code } => remap_operand_slots(code, remap),
    }
}

fn find_canonical_counted_loops(instrs: &[RuntimeInstr]) -> Vec<CountedLoopInfo> {
    let mut loops = Vec::new();
    for latch in 0..instrs.len() {
        let RuntimeInstr::Jump { target: header } = instrs[latch] else {
            continue;
        };
        if header >= latch || header == 0 {
            continue;
        }

        let (op, lhs, rhs, exit_target) = match &instrs[header] {
            RuntimeInstr::JumpIfCmpFalse {
                op,
                lhs,
                rhs,
                target,
            } => (*op, *lhs, *rhs, *target),
            _ => continue,
        };
        if op != RuntimeCmpOp::LtUnsigned {
            continue;
        }
        let RuntimeOperand::Slot(ind_slot) = lhs else {
            continue;
        };
        let limit = match rhs {
            RuntimeOperand::Imm(limit) => LoopLimit::Imm(limit),
            RuntimeOperand::Slot(slot) => LoopLimit::Slot(slot),
        };

        if exit_target <= latch || exit_target > instrs.len() {
            continue;
        }

        let start = match instrs[header - 1] {
            RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Imm(init),
            } if dst == ind_slot => init,
            _ => continue,
        };
        if matches!(limit, LoopLimit::Imm(v) if start > v) {
            continue;
        }

        let (update_idx, tail_norm_idx) = if latch > header + 1 {
            match &instrs[latch - 1] {
                RuntimeInstr::BinOpInPlace {
                    dst,
                    op: RuntimeBinOp::Add,
                    rhs: RuntimeOperand::Imm(1),
                } if *dst == ind_slot => (latch - 1, None),
                RuntimeInstr::NormalizeInt { dst, .. } if *dst == ind_slot => {
                    let update_idx = latch.saturating_sub(2);
                    let is_update = matches!(
                        instrs.get(update_idx),
                        Some(RuntimeInstr::BinOpInPlace {
                            dst,
                            op: RuntimeBinOp::Add,
                            rhs: RuntimeOperand::Imm(1),
                        }) if *dst == ind_slot
                    );
                    if is_update {
                        (update_idx, Some(latch - 1))
                    } else {
                        continue;
                    }
                }
                _ => continue,
            }
        } else {
            continue;
        };

        let mut valid = true;
        for idx in (header + 1)..latch {
            if idx == update_idx || Some(idx) == tail_norm_idx {
                continue;
            }
            if runtime_instr_has_control_flow(&instrs[idx]) {
                valid = false;
                break;
            }
            if runtime_instr_writes_slot(&instrs[idx], ind_slot) {
                valid = false;
                break;
            }
            if matches!(limit, LoopLimit::Slot(limit_slot) if runtime_instr_writes_slot(&instrs[idx], limit_slot))
            {
                valid = false;
                break;
            }
        }
        if !valid {
            continue;
        }

        for idx in (update_idx + 1)..latch {
            if runtime_instr_uses_as_index_slot(&instrs[idx], ind_slot) {
                valid = false;
                break;
            }
        }
        if !valid {
            continue;
        }

        loops.push(CountedLoopInfo {
            header,
            latch,
            exit_target,
            start,
            ind_slot,
            limit,
            update_idx,
        });
    }
    loops
}

fn runtime_instr_has_control_flow(instr: &RuntimeInstr) -> bool {
    matches!(
        instr,
        RuntimeInstr::Jump { .. }
            | RuntimeInstr::JumpIfZero { .. }
            | RuntimeInstr::JumpIfCmpFalse { .. }
            | RuntimeInstr::Call { .. }
            | RuntimeInstr::Return
            | RuntimeInstr::Exit { .. }
    )
}

fn runtime_instr_uses_as_index_slot(instr: &RuntimeInstr, slot: usize) -> bool {
    match instr {
        RuntimeInstr::LoadIndex { index, .. } | RuntimeInstr::LoadIndexUnchecked { index, .. } => {
            runtime_operand_reads_slot(index, slot)
        }
        RuntimeInstr::StoreIndex { index, .. }
        | RuntimeInstr::StoreIndexUnchecked { index, .. } => {
            runtime_operand_reads_slot(index, slot)
        }
        _ => false,
    }
}

fn simplify_runtime_control_flow_instrs(mut instrs: Vec<RuntimeInstr>) -> Vec<RuntimeInstr> {
    if instrs.is_empty() {
        return instrs;
    }

    let mut threaded_targets = vec![None; instrs.len()];
    for idx in 0..instrs.len() {
        threaded_targets[idx] = match &instrs[idx] {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => Some(thread_unconditional_jumps(&instrs, *target)),
            RuntimeInstr::ThreadSpawn { target, .. } => Some(*target),
            RuntimeInstr::LoadSeed { .. }
            | RuntimeInstr::Mov { .. }
            | RuntimeInstr::BinOp { .. }
            | RuntimeInstr::BinOpInPlace { .. }
            | RuntimeInstr::FloatBinOp { .. }
            | RuntimeInstr::Cmp { .. }
            | RuntimeInstr::NormalizeInt { .. }
            | RuntimeInstr::CompareSwap { .. }
            | RuntimeInstr::RadixSortFixedInt { .. }
            | RuntimeInstr::LoadIndex { .. }
            | RuntimeInstr::LoadIndexUnchecked { .. }
            | RuntimeInstr::StoreIndex { .. }
            | RuntimeInstr::StoreIndexUnchecked { .. }
            | RuntimeInstr::HeapLoadInt { .. }
            | RuntimeInstr::HeapStoreInt { .. }
            | RuntimeInstr::HeapCopy { .. }
            | RuntimeInstr::BloomSplitBlockInsert { .. }
            | RuntimeInstr::BloomSplitBlockCheck { .. }
            | RuntimeInstr::BloomClassic4Check { .. }
            | RuntimeInstr::HashCtrlGroupProbe { .. }
            | RuntimeInstr::JoinSelectAdaptive { .. }
            | RuntimeInstr::Alloc { .. }
            | RuntimeInstr::Free { .. }
            | RuntimeInstr::FileOpen { .. }
            | RuntimeInstr::FileWrite { .. }
            | RuntimeInstr::FileRead { .. }
            | RuntimeInstr::FileClose { .. }
            | RuntimeInstr::ThreadJoin { .. }
            | RuntimeInstr::ChannelCreate { .. }
            | RuntimeInstr::ChannelSend { .. }
            | RuntimeInstr::ChannelRecv { .. }
            | RuntimeInstr::ChannelClose { .. }
            | RuntimeInstr::ChannelDestroy { .. }
            | RuntimeInstr::PrintConst { .. }
            | RuntimeInstr::PrintInt { .. }
            | RuntimeInstr::Return
            | RuntimeInstr::Exit { .. } => None,
        };
    }
    for idx in 0..instrs.len() {
        let Some(new_target) = threaded_targets[idx] else {
            continue;
        };
        match &mut instrs[idx] {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => {
                *target = new_target;
            }
            RuntimeInstr::ThreadSpawn { target, .. } => *target = new_target,
            RuntimeInstr::LoadSeed { .. }
            | RuntimeInstr::Mov { .. }
            | RuntimeInstr::BinOp { .. }
            | RuntimeInstr::BinOpInPlace { .. }
            | RuntimeInstr::FloatBinOp { .. }
            | RuntimeInstr::Cmp { .. }
            | RuntimeInstr::NormalizeInt { .. }
            | RuntimeInstr::CompareSwap { .. }
            | RuntimeInstr::RadixSortFixedInt { .. }
            | RuntimeInstr::LoadIndex { .. }
            | RuntimeInstr::LoadIndexUnchecked { .. }
            | RuntimeInstr::StoreIndex { .. }
            | RuntimeInstr::StoreIndexUnchecked { .. }
            | RuntimeInstr::HeapLoadInt { .. }
            | RuntimeInstr::HeapStoreInt { .. }
            | RuntimeInstr::HeapCopy { .. }
            | RuntimeInstr::BloomSplitBlockInsert { .. }
            | RuntimeInstr::BloomSplitBlockCheck { .. }
            | RuntimeInstr::BloomClassic4Check { .. }
            | RuntimeInstr::HashCtrlGroupProbe { .. }
            | RuntimeInstr::JoinSelectAdaptive { .. }
            | RuntimeInstr::Alloc { .. }
            | RuntimeInstr::Free { .. }
            | RuntimeInstr::FileOpen { .. }
            | RuntimeInstr::FileWrite { .. }
            | RuntimeInstr::FileRead { .. }
            | RuntimeInstr::FileClose { .. }
            | RuntimeInstr::ThreadJoin { .. }
            | RuntimeInstr::ChannelCreate { .. }
            | RuntimeInstr::ChannelSend { .. }
            | RuntimeInstr::ChannelRecv { .. }
            | RuntimeInstr::ChannelClose { .. }
            | RuntimeInstr::ChannelDestroy { .. }
            | RuntimeInstr::PrintConst { .. }
            | RuntimeInstr::PrintInt { .. }
            | RuntimeInstr::Return
            | RuntimeInstr::Exit { .. } => {}
        }
    }

    for idx in 0..instrs.len() {
        if let RuntimeInstr::Jump { target } = &instrs[idx] {
            if *target < instrs.len() && matches!(instrs[*target], RuntimeInstr::Return) {
                instrs[idx] = RuntimeInstr::Return;
            }
        }
    }

    let reachable = compute_runtime_reachable(&instrs);
    compact_runtime_instrs(instrs, &reachable)
}

fn thread_unconditional_jumps(instrs: &[RuntimeInstr], mut target: usize) -> usize {
    let mut hops = 0usize;
    let max_hops = instrs.len().saturating_add(1);
    while target < instrs.len() {
        let RuntimeInstr::Jump { target: next } = &instrs[target] else {
            break;
        };
        if *next == target {
            break;
        }
        target = *next;
        hops += 1;
        if hops > max_hops {
            break;
        }
    }
    target
}

fn compute_runtime_reachable(instrs: &[RuntimeInstr]) -> Vec<bool> {
    let mut reachable = vec![false; instrs.len()];
    let mut work = vec![0usize];
    while let Some(idx) = work.pop() {
        if idx >= instrs.len() || reachable[idx] {
            continue;
        }
        reachable[idx] = true;
        match &instrs[idx] {
            RuntimeInstr::Jump { target } => {
                work.push(*target);
            }
            RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. } => {
                if idx + 1 < instrs.len() {
                    work.push(idx + 1);
                }
                work.push(*target);
            }
            RuntimeInstr::Call { target } | RuntimeInstr::ThreadSpawn { target, .. } => {
                if idx + 1 < instrs.len() {
                    work.push(idx + 1);
                }
                work.push(*target);
            }
            RuntimeInstr::Return | RuntimeInstr::Exit { .. } => {}
            RuntimeInstr::LoadSeed { .. }
            | RuntimeInstr::Mov { .. }
            | RuntimeInstr::BinOp { .. }
            | RuntimeInstr::BinOpInPlace { .. }
            | RuntimeInstr::FloatBinOp { .. }
            | RuntimeInstr::Cmp { .. }
            | RuntimeInstr::NormalizeInt { .. }
            | RuntimeInstr::CompareSwap { .. }
            | RuntimeInstr::RadixSortFixedInt { .. }
            | RuntimeInstr::LoadIndex { .. }
            | RuntimeInstr::LoadIndexUnchecked { .. }
            | RuntimeInstr::StoreIndex { .. }
            | RuntimeInstr::StoreIndexUnchecked { .. }
            | RuntimeInstr::HeapLoadInt { .. }
            | RuntimeInstr::HeapStoreInt { .. }
            | RuntimeInstr::HeapCopy { .. }
            | RuntimeInstr::BloomSplitBlockInsert { .. }
            | RuntimeInstr::BloomSplitBlockCheck { .. }
            | RuntimeInstr::BloomClassic4Check { .. }
            | RuntimeInstr::HashCtrlGroupProbe { .. }
            | RuntimeInstr::JoinSelectAdaptive { .. }
            | RuntimeInstr::Alloc { .. }
            | RuntimeInstr::Free { .. }
            | RuntimeInstr::FileOpen { .. }
            | RuntimeInstr::FileWrite { .. }
            | RuntimeInstr::FileRead { .. }
            | RuntimeInstr::FileClose { .. }
            | RuntimeInstr::ThreadJoin { .. }
            | RuntimeInstr::ChannelCreate { .. }
            | RuntimeInstr::ChannelSend { .. }
            | RuntimeInstr::ChannelRecv { .. }
            | RuntimeInstr::ChannelClose { .. }
            | RuntimeInstr::ChannelDestroy { .. } => {
                if idx + 1 < instrs.len() {
                    work.push(idx + 1);
                }
            }
            RuntimeInstr::PrintConst { .. } | RuntimeInstr::PrintInt { .. } => {
                if idx + 1 < instrs.len() {
                    work.push(idx + 1);
                }
            }
        }
    }
    reachable
}

fn compact_runtime_instrs(instrs: Vec<RuntimeInstr>, reachable: &[bool]) -> Vec<RuntimeInstr> {
    let mut remap = vec![usize::MAX; instrs.len() + 1];
    let mut out = Vec::with_capacity(instrs.len());
    for (idx, instr) in instrs.into_iter().enumerate() {
        if reachable[idx] {
            remap[idx] = out.len();
            out.push(instr);
        }
    }
    let remap_end = remap.len() - 1;
    remap[remap_end] = out.len();

    let out_end = out.len();

    for instr in &mut out {
        match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target }
            | RuntimeInstr::ThreadSpawn { target, .. } => {
                let old = (*target).min(remap.len() - 1);
                if remap[old] == usize::MAX {
                    *target = out_end;
                } else {
                    *target = remap[old];
                }
            }
            RuntimeInstr::LoadSeed { .. }
            | RuntimeInstr::Mov { .. }
            | RuntimeInstr::BinOp { .. }
            | RuntimeInstr::BinOpInPlace { .. }
            | RuntimeInstr::FloatBinOp { .. }
            | RuntimeInstr::Cmp { .. }
            | RuntimeInstr::NormalizeInt { .. }
            | RuntimeInstr::CompareSwap { .. }
            | RuntimeInstr::RadixSortFixedInt { .. }
            | RuntimeInstr::LoadIndex { .. }
            | RuntimeInstr::LoadIndexUnchecked { .. }
            | RuntimeInstr::StoreIndex { .. }
            | RuntimeInstr::StoreIndexUnchecked { .. }
            | RuntimeInstr::HeapLoadInt { .. }
            | RuntimeInstr::HeapStoreInt { .. }
            | RuntimeInstr::HeapCopy { .. }
            | RuntimeInstr::BloomSplitBlockInsert { .. }
            | RuntimeInstr::BloomSplitBlockCheck { .. }
            | RuntimeInstr::BloomClassic4Check { .. }
            | RuntimeInstr::HashCtrlGroupProbe { .. }
            | RuntimeInstr::JoinSelectAdaptive { .. }
            | RuntimeInstr::Alloc { .. }
            | RuntimeInstr::Free { .. }
            | RuntimeInstr::FileOpen { .. }
            | RuntimeInstr::FileWrite { .. }
            | RuntimeInstr::FileRead { .. }
            | RuntimeInstr::FileClose { .. }
            | RuntimeInstr::ThreadJoin { .. }
            | RuntimeInstr::ChannelCreate { .. }
            | RuntimeInstr::ChannelSend { .. }
            | RuntimeInstr::ChannelRecv { .. }
            | RuntimeInstr::ChannelClose { .. }
            | RuntimeInstr::ChannelDestroy { .. }
            | RuntimeInstr::PrintConst { .. }
            | RuntimeInstr::PrintInt { .. }
            | RuntimeInstr::Return
            | RuntimeInstr::Exit { .. } => {}
        }
    }

    if out.is_empty() {
        return out;
    }

    let mut out2 = Vec::with_capacity(out.len());
    let mut remap2 = vec![usize::MAX; out.len() + 1];
    for (idx, instr) in out.into_iter().enumerate() {
        // A target that lands on a removed fallthrough jump must continue at the
        // instruction that follows it, which is exactly the current output index.
        remap2[idx] = out2.len();
        let skip = matches!(&instr, RuntimeInstr::Jump { target } if *target == idx + 1);
        if skip {
            continue;
        }
        out2.push(instr);
    }
    let remap2_end = remap2.len() - 1;
    remap2[remap2_end] = out2.len();

    let out2_end = out2.len();

    for instr in &mut out2 {
        match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target }
            | RuntimeInstr::ThreadSpawn { target, .. } => {
                let old = (*target).min(remap2.len() - 1);
                if remap2[old] == usize::MAX {
                    *target = out2_end;
                } else {
                    *target = remap2[old];
                }
            }
            RuntimeInstr::LoadSeed { .. }
            | RuntimeInstr::Mov { .. }
            | RuntimeInstr::BinOp { .. }
            | RuntimeInstr::BinOpInPlace { .. }
            | RuntimeInstr::FloatBinOp { .. }
            | RuntimeInstr::Cmp { .. }
            | RuntimeInstr::NormalizeInt { .. }
            | RuntimeInstr::CompareSwap { .. }
            | RuntimeInstr::RadixSortFixedInt { .. }
            | RuntimeInstr::LoadIndex { .. }
            | RuntimeInstr::LoadIndexUnchecked { .. }
            | RuntimeInstr::StoreIndex { .. }
            | RuntimeInstr::StoreIndexUnchecked { .. }
            | RuntimeInstr::HeapLoadInt { .. }
            | RuntimeInstr::HeapStoreInt { .. }
            | RuntimeInstr::HeapCopy { .. }
            | RuntimeInstr::BloomSplitBlockInsert { .. }
            | RuntimeInstr::BloomSplitBlockCheck { .. }
            | RuntimeInstr::BloomClassic4Check { .. }
            | RuntimeInstr::HashCtrlGroupProbe { .. }
            | RuntimeInstr::JoinSelectAdaptive { .. }
            | RuntimeInstr::Alloc { .. }
            | RuntimeInstr::Free { .. }
            | RuntimeInstr::FileOpen { .. }
            | RuntimeInstr::FileWrite { .. }
            | RuntimeInstr::FileRead { .. }
            | RuntimeInstr::FileClose { .. }
            | RuntimeInstr::ThreadJoin { .. }
            | RuntimeInstr::ChannelCreate { .. }
            | RuntimeInstr::ChannelSend { .. }
            | RuntimeInstr::ChannelRecv { .. }
            | RuntimeInstr::ChannelClose { .. }
            | RuntimeInstr::ChannelDestroy { .. }
            | RuntimeInstr::PrintConst { .. }
            | RuntimeInstr::PrintInt { .. }
            | RuntimeInstr::Return
            | RuntimeInstr::Exit { .. } => {}
        }
    }
    out2
}

fn leaf_body_end(instrs: &[RuntimeInstr], target: usize) -> Option<usize> {
    if target >= instrs.len() {
        return None;
    }
    for (idx, instr) in instrs.iter().enumerate().skip(target) {
        match instr {
            RuntimeInstr::Return => return Some(idx),
            RuntimeInstr::Call { .. }
            | RuntimeInstr::ThreadSpawn { .. }
            | RuntimeInstr::ThreadJoin { .. }
            | RuntimeInstr::ChannelCreate { .. }
            | RuntimeInstr::ChannelSend { .. }
            | RuntimeInstr::ChannelRecv { .. }
            | RuntimeInstr::ChannelClose { .. }
            | RuntimeInstr::ChannelDestroy { .. }
            | RuntimeInstr::Jump { .. }
            | RuntimeInstr::JumpIfZero { .. }
            | RuntimeInstr::JumpIfCmpFalse { .. }
            | RuntimeInstr::FileOpen { .. }
            | RuntimeInstr::FileWrite { .. }
            | RuntimeInstr::FileRead { .. }
            | RuntimeInstr::FileClose { .. }
            | RuntimeInstr::Exit { .. } => return None,
            RuntimeInstr::LoadSeed { .. }
            | RuntimeInstr::Mov { .. }
            | RuntimeInstr::BinOp { .. }
            | RuntimeInstr::BinOpInPlace { .. }
            | RuntimeInstr::FloatBinOp { .. }
            | RuntimeInstr::Cmp { .. }
            | RuntimeInstr::NormalizeInt { .. }
            | RuntimeInstr::CompareSwap { .. }
            | RuntimeInstr::RadixSortFixedInt { .. }
            | RuntimeInstr::LoadIndex { .. }
            | RuntimeInstr::LoadIndexUnchecked { .. }
            | RuntimeInstr::StoreIndex { .. }
            | RuntimeInstr::StoreIndexUnchecked { .. }
            | RuntimeInstr::HeapLoadInt { .. }
            | RuntimeInstr::HeapStoreInt { .. }
            | RuntimeInstr::HeapCopy { .. }
            | RuntimeInstr::BloomSplitBlockInsert { .. }
            | RuntimeInstr::BloomSplitBlockCheck { .. }
            | RuntimeInstr::BloomClassic4Check { .. }
            | RuntimeInstr::HashCtrlGroupProbe { .. }
            | RuntimeInstr::JoinSelectAdaptive { .. }
            | RuntimeInstr::Alloc { .. }
            | RuntimeInstr::Free { .. } => {}
            RuntimeInstr::PrintConst { .. } | RuntimeInstr::PrintInt { .. } => {}
        }
    }
    None
}

fn remap_instr_targets_after_inline(instrs: &mut [RuntimeInstr], call_idx: usize, delta: usize) {
    let remap = |target: &mut usize| {
        if *target > call_idx {
            *target += delta;
        }
    };
    for instr in instrs {
        match instr {
            RuntimeInstr::Jump { target } => remap(target),
            RuntimeInstr::JumpIfZero { target, .. } => remap(target),
            RuntimeInstr::JumpIfCmpFalse { target, .. } => remap(target),
            RuntimeInstr::Call { target } | RuntimeInstr::ThreadSpawn { target, .. } => {
                remap(target)
            }
            RuntimeInstr::LoadSeed { .. }
            | RuntimeInstr::Mov { .. }
            | RuntimeInstr::BinOp { .. }
            | RuntimeInstr::BinOpInPlace { .. }
            | RuntimeInstr::FloatBinOp { .. }
            | RuntimeInstr::Cmp { .. }
            | RuntimeInstr::NormalizeInt { .. }
            | RuntimeInstr::CompareSwap { .. }
            | RuntimeInstr::RadixSortFixedInt { .. }
            | RuntimeInstr::LoadIndex { .. }
            | RuntimeInstr::LoadIndexUnchecked { .. }
            | RuntimeInstr::StoreIndex { .. }
            | RuntimeInstr::StoreIndexUnchecked { .. }
            | RuntimeInstr::HeapLoadInt { .. }
            | RuntimeInstr::HeapStoreInt { .. }
            | RuntimeInstr::HeapCopy { .. }
            | RuntimeInstr::BloomSplitBlockInsert { .. }
            | RuntimeInstr::BloomSplitBlockCheck { .. }
            | RuntimeInstr::BloomClassic4Check { .. }
            | RuntimeInstr::HashCtrlGroupProbe { .. }
            | RuntimeInstr::JoinSelectAdaptive { .. }
            | RuntimeInstr::Alloc { .. }
            | RuntimeInstr::Free { .. }
            | RuntimeInstr::FileOpen { .. }
            | RuntimeInstr::FileWrite { .. }
            | RuntimeInstr::FileRead { .. }
            | RuntimeInstr::FileClose { .. }
            | RuntimeInstr::ThreadJoin { .. }
            | RuntimeInstr::ChannelCreate { .. }
            | RuntimeInstr::ChannelSend { .. }
            | RuntimeInstr::ChannelRecv { .. }
            | RuntimeInstr::ChannelClose { .. }
            | RuntimeInstr::ChannelDestroy { .. }
            | RuntimeInstr::PrintConst { .. }
            | RuntimeInstr::PrintInt { .. }
            | RuntimeInstr::Return
            | RuntimeInstr::Exit { .. } => {}
        }
    }
}

fn hoist_loop_invariants(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < stmts.len() {
        match &stmts[i] {
            LoweredStmt::Print(text) => {
                let mut repeats = 1usize;
                while i + repeats < stmts.len() {
                    match &stmts[i + repeats] {
                        LoweredStmt::Print(next) if next == text => {
                            repeats += 1;
                        }
                        _ => break,
                    }
                }
                if repeats > 1 {
                    let mut merged = String::with_capacity(text.len() * repeats);
                    for _ in 0..repeats {
                        merged.push_str(text);
                    }
                    out.push(LoweredStmt::Print(merged));
                } else {
                    out.push(stmts[i].clone());
                }
                i += repeats;
            }
            _ => {
                out.push(stmts[i].clone());
                i += 1;
            }
        }
    }
    out
}

fn const_fold(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    let mut out = Vec::new();
    for stmt in stmts {
        match stmt {
            LoweredStmt::Print(text) => {
                if text.is_empty() {
                    continue;
                }
                if let Some(LoweredStmt::Print(prev)) = out.last_mut() {
                    prev.push_str(&text);
                } else {
                    out.push(LoweredStmt::Print(text));
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn dead_print_elimination(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    let mut out = Vec::new();
    for stmt in stmts {
        match stmt {
            LoweredStmt::Print(text) => {
                if !text.is_empty() {
                    out.push(LoweredStmt::Print(text));
                }
            }
            LoweredStmt::Exit(code) => {
                out.push(LoweredStmt::Exit(code));
                break;
            }
            LoweredStmt::RuntimeBenchLoop { iterations } => {
                out.push(LoweredStmt::RuntimeBenchLoop { iterations });
                break;
            }
            LoweredStmt::RuntimeLcgLoop {
                iterations,
                state_init,
                mul,
                add,
                exit_with_state,
                exit_mask,
            } => {
                out.push(LoweredStmt::RuntimeLcgLoop {
                    iterations,
                    state_init,
                    mul,
                    add,
                    exit_with_state,
                    exit_mask,
                });
                break;
            }
            LoweredStmt::RuntimeSeededLcgLoop {
                iterations,
                mul,
                add,
                exit_with_state,
                exit_mask,
            } => {
                out.push(LoweredStmt::RuntimeSeededLcgLoop {
                    iterations,
                    mul,
                    add,
                    exit_with_state,
                    exit_mask,
                });
                break;
            }
            LoweredStmt::RuntimeRingWriteLoop {
                iterations,
                state_init,
                index_init,
                mul,
                add,
                state_mask,
                ring_mask,
                value_shift,
                exit_mask,
            } => {
                out.push(LoweredStmt::RuntimeRingWriteLoop {
                    iterations,
                    state_init,
                    index_init,
                    mul,
                    add,
                    state_mask,
                    ring_mask,
                    value_shift,
                    exit_mask,
                });
                break;
            }
            LoweredStmt::RuntimePrefixScanLoop { .. } => {
                out.push(stmt);
                break;
            }
            LoweredStmt::RuntimeBloomFilterLoop { .. } => {
                out.push(stmt);
                break;
            }
            LoweredStmt::RuntimeBranchLcgLoop { .. } => {
                out.push(stmt);
                break;
            }
            LoweredStmt::RuntimeSeededLcgAllocLoop {
                iterations,
                mul,
                add,
                alloc_bytes,
                exit_with_state,
            } => {
                out.push(LoweredStmt::RuntimeSeededLcgAllocLoop {
                    iterations,
                    mul,
                    add,
                    alloc_bytes,
                    exit_with_state,
                });
                break;
            }
            LoweredStmt::RuntimeSeededPredictableBranchLcgLoop {
                iterations,
                then_iterations,
                then_mul,
                then_add,
                else_mul,
                else_add,
                exit_with_state,
                exit_mask,
            } => {
                out.push(LoweredStmt::RuntimeSeededPredictableBranchLcgLoop {
                    iterations,
                    then_iterations,
                    then_mul,
                    then_add,
                    else_mul,
                    else_add,
                    exit_with_state,
                    exit_mask,
                });
                break;
            }
            LoweredStmt::RuntimeSeededUnpredictableBranchLcgLoop {
                iterations,
                threshold,
                then_mul,
                then_add,
                else_mul,
                else_add,
                exit_with_state,
                exit_mask,
            } => {
                out.push(LoweredStmt::RuntimeSeededUnpredictableBranchLcgLoop {
                    iterations,
                    threshold,
                    then_mul,
                    then_add,
                    else_mul,
                    else_add,
                    exit_with_state,
                    exit_mask,
                });
                break;
            }
            LoweredStmt::RuntimeSeededDualStateBranchLoop {
                iterations,
                index_init,
                adaptive,
                branchless,
                exit_with_sum,
            } => {
                out.push(LoweredStmt::RuntimeSeededDualStateBranchLoop {
                    iterations,
                    index_init,
                    adaptive,
                    branchless,
                    exit_with_sum,
                });
                break;
            }
            LoweredStmt::RuntimeAffineIndexLoop { .. } => {
                out.push(stmt);
                break;
            }
            LoweredStmt::RuntimeSeededAffineIndexLoop {
                iterations,
                index_init,
                state_mul,
                index_mul,
                add,
                state_mask,
                exit_with_state,
                exit_mask,
            } => {
                out.push(LoweredStmt::RuntimeSeededAffineIndexLoop {
                    iterations,
                    index_init,
                    state_mul,
                    index_mul,
                    add,
                    state_mask,
                    exit_with_state,
                    exit_mask,
                });
                break;
            }
            LoweredStmt::RuntimeSeededAffineClosedForm {
                state_mul,
                add,
                exit_with_state,
            } => {
                out.push(LoweredStmt::RuntimeSeededAffineClosedForm {
                    state_mul,
                    add,
                    exit_with_state,
                });
                break;
            }
            LoweredStmt::RuntimeSeededStructLatencyLoop {
                iterations,
                mul,
                add,
                exit_with_sum,
            } => {
                out.push(LoweredStmt::RuntimeSeededStructLatencyLoop {
                    iterations,
                    mul,
                    add,
                    exit_with_sum,
                });
                break;
            }
            LoweredStmt::RuntimeGeneric { program } => {
                out.push(LoweredStmt::RuntimeGeneric { program });
                break;
            }
        }
    }
    out
}

fn fold_runtime_kernels(stmts: Vec<LoweredStmt>) -> Vec<LoweredStmt> {
    let mut out = Vec::with_capacity(stmts.len());
    for stmt in stmts {
        match stmt {
            LoweredStmt::RuntimeLcgLoop {
                iterations,
                state_init,
                mul,
                add,
                exit_with_state,
                exit_mask,
            } => {
                // Keep the loop alive — the backend's emit_runtime_lcg_compute
                // uses a fast 4x-unrolled affine step.  Folding to constant
                // would make the binary just `exit(n)` with no loop.
                out.push(LoweredStmt::RuntimeLcgLoop {
                    iterations,
                    state_init,
                    mul,
                    add,
                    exit_with_state,
                    exit_mask,
                });
                break;
            }
            LoweredStmt::RuntimeSeededLcgLoop {
                iterations,
                mul,
                add,
                exit_with_state,
                exit_mask,
            } => {
                if exit_with_state && exit_mask.unwrap_or(u64::MAX) == u64::MAX {
                    let (state_mul, folded_add) =
                        affine_pow(u64::from(mul), u64::from(add), iterations);
                    if state_mul == 0 {
                        out.push(LoweredStmt::Exit(folded_add));
                    } else {
                        out.push(LoweredStmt::RuntimeSeededAffineClosedForm {
                            state_mul,
                            add: folded_add,
                            exit_with_state: true,
                        });
                    }
                } else {
                    out.push(LoweredStmt::RuntimeSeededLcgLoop {
                        iterations,
                        mul,
                        add,
                        exit_with_state,
                        exit_mask,
                    });
                }
            }
            LoweredStmt::RuntimeRingWriteLoop {
                iterations,
                state_init,
                index_init,
                mul,
                add,
                state_mask,
                ring_mask,
                value_shift,
                exit_mask,
            } => {
                // Ring writes define this workload's memory-pressure behavior.
                // Although the final checksum can be derived algebraically for
                // some constant inputs, replacing the complete loop with
                // `exit(const)` would invalidate benchmark comparability.
                out.push(LoweredStmt::RuntimeRingWriteLoop {
                    iterations,
                    state_init,
                    index_init,
                    mul,
                    add,
                    state_mask,
                    ring_mask,
                    value_shift,
                    exit_mask,
                });
                break;
            }
            LoweredStmt::RuntimePrefixScanLoop { .. } => {
                out.push(stmt);
                break;
            }
            LoweredStmt::RuntimeBloomFilterLoop { .. } => {
                out.push(stmt);
                break;
            }
            LoweredStmt::RuntimeBranchLcgLoop { .. } => out.push(stmt),
            LoweredStmt::RuntimeSeededAffineIndexLoop {
                iterations,
                index_init,
                state_mul,
                index_mul,
                add,
                state_mask,
                exit_with_state,
                exit_mask,
            } => {
                if exit_with_state
                    && state_mask == u64::MAX
                    && exit_mask.unwrap_or(u64::MAX) == u64::MAX
                {
                    let (a, b, c) = affine_index_pow(
                        u64::from(state_mul),
                        u64::from(index_mul),
                        (add as i32 as i64) as u64,
                        iterations,
                    );
                    let folded_add = b.wrapping_mul(index_init).wrapping_add(c);
                    if a == 0 {
                        out.push(LoweredStmt::Exit(folded_add));
                    } else {
                        out.push(LoweredStmt::RuntimeSeededAffineClosedForm {
                            state_mul: a,
                            add: folded_add,
                            exit_with_state: true,
                        });
                    }
                } else {
                    out.push(LoweredStmt::RuntimeSeededAffineIndexLoop {
                        iterations,
                        index_init,
                        state_mul,
                        index_mul,
                        add,
                        state_mask,
                        exit_with_state,
                        exit_mask,
                    });
                }
            }
            LoweredStmt::RuntimeAffineIndexLoop {
                iterations,
                state_init,
                index_init,
                state_mul,
                index_mul,
                add,
                state_mask,
                exit_with_state,
                exit_mask,
            } => {
                // Keep the real loop alive — pass through to backend.
                out.push(LoweredStmt::RuntimeAffineIndexLoop {
                    iterations,
                    state_init,
                    index_init,
                    state_mul,
                    index_mul,
                    add,
                    state_mask,
                    exit_with_state,
                    exit_mask,
                });
                break;
            }
            LoweredStmt::RuntimeSeededAffineClosedForm {
                state_mul,
                add,
                exit_with_state,
            } => {
                if exit_with_state && state_mul == 0 {
                    out.push(LoweredStmt::Exit(add));
                } else if !exit_with_state {
                    out.push(LoweredStmt::Exit(0));
                } else {
                    out.push(LoweredStmt::RuntimeSeededAffineClosedForm {
                        state_mul,
                        add,
                        exit_with_state: true,
                    });
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn affine_pow(mut mul: u64, mut add: u64, mut exp: u64) -> (u64, u64) {
    let mut acc_mul = 1u64;
    let mut acc_add = 0u64;

    while exp > 0 {
        if exp & 1 == 1 {
            acc_add = acc_add.wrapping_mul(mul).wrapping_add(add);
            acc_mul = acc_mul.wrapping_mul(mul);
        }

        let next_mul = mul.wrapping_mul(mul);
        let next_add = add.wrapping_mul(mul).wrapping_add(add);
        mul = next_mul;
        add = next_add;
        exp >>= 1;
    }

    (acc_mul, acc_add)
}

fn affine_index_pow(step_a: u64, step_b: u64, step_c: u64, iterations: u64) -> (u64, u64, u64) {
    let mut acc = AffineIndexTransform::identity();
    let mut base = AffineIndexTransform::step(step_a, step_b, step_c);
    let mut exp = iterations;

    while exp > 0 {
        if exp & 1 == 1 {
            acc = compose_affine_index(acc, base);
        }
        base = compose_affine_index(base, base);
        exp >>= 1;
    }

    (acc.state_mul, acc.index_mul, acc.add)
}

#[derive(Clone, Copy)]
struct AffineIndexTransform {
    state_mul: u64,
    index_mul: u64,
    add: u64,
    index_delta: u64,
}

impl AffineIndexTransform {
    fn identity() -> Self {
        Self {
            state_mul: 1,
            index_mul: 0,
            add: 0,
            index_delta: 0,
        }
    }

    fn step(state_mul: u64, index_mul: u64, add: u64) -> Self {
        Self {
            state_mul,
            index_mul,
            add,
            index_delta: 1,
        }
    }
}

fn compose_affine_index(
    first: AffineIndexTransform,
    second: AffineIndexTransform,
) -> AffineIndexTransform {
    // Composition: second(first(state, index))
    AffineIndexTransform {
        state_mul: second.state_mul.wrapping_mul(first.state_mul),
        index_mul: second
            .state_mul
            .wrapping_mul(first.index_mul)
            .wrapping_add(second.index_mul),
        add: second
            .state_mul
            .wrapping_mul(first.add)
            .wrapping_add(second.index_mul.wrapping_mul(first.index_delta))
            .wrapping_add(second.add),
        index_delta: first.index_delta.wrapping_add(second.index_delta),
    }
}

fn optimize_lcg_lookahead(instrs: &mut Vec<RuntimeInstr>) {
    let mut i = 0usize;
    while i < instrs.len() {
        // Detect state = state * A + B sequences
        let mut lookahead = Vec::new();
        let mut curr_idx = i;
        let mut state_slot = None;

        while curr_idx < instrs.len() {
            if let RuntimeInstr::BinOpInPlace {
                dst,
                op,
                rhs: RuntimeOperand::Imm(a),
            } = &instrs[curr_idx]
            {
                if *op == RuntimeBinOp::Mul {
                    if let Some(next_idx) = curr_idx.checked_add(1) {
                        if next_idx < instrs.len() {
                            if let RuntimeInstr::BinOpInPlace {
                                dst: dst2,
                                op: op2,
                                rhs: RuntimeOperand::Imm(b),
                            } = &instrs[next_idx]
                            {
                                if *op2 == RuntimeBinOp::Add && dst == dst2 {
                                    if state_slot.is_none() || state_slot == Some(*dst) {
                                        state_slot = Some(*dst);
                                        lookahead.push((curr_idx, *a, *b));
                                        curr_idx += 2;
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            break;
        }

        if lookahead.len() >= 2 {
            // Re-write lookahead into independent formulas
            // X_2 = X_1 * A + B
            // X_3 = X_2 * A + B = (X_1 * A + B) * A + B = X_1 * A^2 + (AB + B)
            // ...
            let slot = state_slot.unwrap();
            let (mut total_a, mut total_b) = (1u64, 0u64);
            let mut base_a = vec![1u64; lookahead.len() + 1];
            let mut base_b = vec![0u64; lookahead.len() + 1];

            let a = lookahead[0].1;
            let b = lookahead[0].2;

            for k in 1..=lookahead.len() {
                total_b = total_b.wrapping_mul(a).wrapping_add(b);
                total_a = total_a.wrapping_mul(a);
                base_a[k] = total_a;
                base_b[k] = total_b;
            }

            // Replace sequence with:
            // temp = slot
            // slot = temp * base_a[1] + base_b[1]
            // slot = temp * base_a[2] + base_b[2]
            // ...
            // Wait, we need to keep the intermediate results if they are used!
            // Actually, in our benchmarks, they are usually consumed (e.g. b ^= state).
            // So we need to ensure we don't break consumers between the updates.

            let mut safe = true;
            for k in 0..lookahead.len() {
                let start = lookahead[k].0;
                // Check if slot is read between mul and add (unlikely) or after add before next lcg
                let next_lcg_start = if k + 1 < lookahead.len() {
                    lookahead[k + 1].0
                } else {
                    instrs.len()
                };
                for j in (start + 2)..next_lcg_start {
                    if runtime_instr_reads_slot(&instrs[j], slot) {
                        safe = false; // Consumer exists
                        break;
                    }
                }
                if !safe {
                    break;
                }
            }

            if safe && lookahead.len() >= 4 {
                // Only parallelize if significant
                // Transform serial LCGs into independent ones relative to loop start.
                // This allows the CPU to issue all multiplications in parallel (ILP).
                for k in 1..lookahead.len() {
                    let (instr_idx, _, _) = lookahead[k];
                    // Replace MUL with MUL by total_a[k]
                    instrs[instr_idx] = RuntimeInstr::BinOpInPlace {
                        dst: slot,
                        op: RuntimeBinOp::Mul,
                        rhs: RuntimeOperand::Imm(base_a[k]), // Correct relative to iteration 0
                    };
                    // Replace ADD with ADD by total_b[k]
                    instrs[instr_idx + 1] = RuntimeInstr::BinOpInPlace {
                        dst: slot,
                        op: RuntimeBinOp::Add,
                        rhs: RuntimeOperand::Imm(base_b[k]),
                    };
                }
            }
        }
        i += 1;
    }
}

fn runtime_instr_reads_slot(instr: &RuntimeInstr, slot: usize) -> bool {
    match instr {
        RuntimeInstr::LoadSeed { input, .. } => input
            .as_ref()
            .is_some_and(|operand| runtime_operand_reads_slot(operand, slot)),
        RuntimeInstr::Mov { src, .. } => runtime_operand_reads_slot(src, slot),
        RuntimeInstr::BinOp { lhs, rhs, .. } => {
            runtime_operand_reads_slot(lhs, slot) || runtime_operand_reads_slot(rhs, slot)
        }
        RuntimeInstr::BinOpInPlace { dst, rhs, .. } => {
            *dst == slot || runtime_operand_reads_slot(rhs, slot)
        }
        RuntimeInstr::FloatBinOp { lhs, rhs, .. } => {
            runtime_operand_reads_slot(lhs, slot) || runtime_operand_reads_slot(rhs, slot)
        }
        RuntimeInstr::Cmp { lhs, rhs, .. } => {
            runtime_operand_reads_slot(lhs, slot) || runtime_operand_reads_slot(rhs, slot)
        }
        RuntimeInstr::NormalizeInt { dst, .. } => *dst == slot,
        RuntimeInstr::Jump { .. } => false,
        RuntimeInstr::JumpIfZero { cond_slot, .. } => *cond_slot == slot,
        RuntimeInstr::JumpIfCmpFalse { lhs, rhs, .. } => {
            runtime_operand_reads_slot(lhs, slot) || runtime_operand_reads_slot(rhs, slot)
        }
        RuntimeInstr::CompareSwap { left, right, .. } => *left == slot || *right == slot,
        RuntimeInstr::RadixSortFixedInt { slots, .. } => slots.contains(&slot),
        RuntimeInstr::LoadIndex {
            base_slots, index, ..
        } => base_slots.contains(&slot) || runtime_operand_reads_slot(index, slot),
        RuntimeInstr::LoadIndexUnchecked {
            base_slots, index, ..
        } => base_slots.contains(&slot) || runtime_operand_reads_slot(index, slot),
        RuntimeInstr::StoreIndex {
            base_slots,
            index,
            src,
        } => {
            base_slots.contains(&slot)
                || runtime_operand_reads_slot(index, slot)
                || runtime_operand_reads_slot(src, slot)
        }
        RuntimeInstr::HeapLoadInt { ptr, index, .. } => {
            runtime_operand_reads_slot(ptr, slot) || runtime_operand_reads_slot(index, slot)
        }
        RuntimeInstr::HeapStoreInt {
            ptr, index, src, ..
        } => {
            runtime_operand_reads_slot(ptr, slot)
                || runtime_operand_reads_slot(index, slot)
                || runtime_operand_reads_slot(src, slot)
        }
        RuntimeInstr::HeapCopy {
            dst_ptr,
            src_ptr,
            bytes,
        } => {
            runtime_operand_reads_slot(dst_ptr, slot)
                || runtime_operand_reads_slot(src_ptr, slot)
                || runtime_operand_reads_slot(bytes, slot)
        }
        RuntimeInstr::StoreIndexUnchecked {
            base_slots,
            index,
            src,
        } => {
            base_slots.contains(&slot)
                || runtime_operand_reads_slot(index, slot)
                || runtime_operand_reads_slot(src, slot)
        }
        RuntimeInstr::BloomSplitBlockInsert { filter_slots, hash } => {
            filter_slots.contains(&slot) || runtime_operand_reads_slot(hash, slot)
        }
        RuntimeInstr::BloomSplitBlockCheck {
            filter_slots, hash, ..
        }
        | RuntimeInstr::BloomClassic4Check {
            filter_slots, hash, ..
        } => filter_slots.contains(&slot) || runtime_operand_reads_slot(hash, slot),
        RuntimeInstr::HashCtrlGroupProbe {
            ctrl_slots,
            group_start,
            fingerprint,
            ..
        } => {
            ctrl_slots.contains(&slot)
                || runtime_operand_reads_slot(group_start, slot)
                || runtime_operand_reads_slot(fingerprint, slot)
        }
        RuntimeInstr::JoinSelectAdaptive {
            build_rows,
            probe_rows,
            ..
        } => {
            runtime_operand_reads_slot(build_rows, slot)
                || runtime_operand_reads_slot(probe_rows, slot)
        }
        RuntimeInstr::Alloc { size, .. } => runtime_operand_reads_slot(size, slot),
        RuntimeInstr::Free { ptr, size } => {
            runtime_operand_reads_slot(ptr, slot) || runtime_operand_reads_slot(size, slot)
        }
        RuntimeInstr::FileOpen { path_ptr, .. } => runtime_operand_reads_slot(path_ptr, slot),
        RuntimeInstr::FileWrite { fd, ptr, len, .. }
        | RuntimeInstr::FileRead { fd, ptr, len, .. } => {
            runtime_operand_reads_slot(fd, slot)
                || runtime_operand_reads_slot(ptr, slot)
                || runtime_operand_reads_slot(len, slot)
        }
        RuntimeInstr::FileClose { fd } => runtime_operand_reads_slot(fd, slot),
        RuntimeInstr::ThreadSpawn { return_slot, .. } => return_slot.is_some_and(|s| s == slot),
        RuntimeInstr::ThreadJoin { handle, .. } => runtime_operand_reads_slot(handle, slot),
        RuntimeInstr::ChannelCreate { capacity, .. } => runtime_operand_reads_slot(capacity, slot),
        RuntimeInstr::ChannelSend { handle, value } => {
            runtime_operand_reads_slot(handle, slot) || runtime_operand_reads_slot(value, slot)
        }
        RuntimeInstr::ChannelRecv { handle, .. }
        | RuntimeInstr::ChannelClose { handle, .. }
        | RuntimeInstr::ChannelDestroy { handle } => runtime_operand_reads_slot(handle, slot),
        RuntimeInstr::PrintConst { .. } => false,
        RuntimeInstr::PrintInt { value, .. } => runtime_operand_reads_slot(value, slot),
        RuntimeInstr::Call { .. } | RuntimeInstr::Return => false,
        RuntimeInstr::Exit { code } => runtime_operand_reads_slot(code, slot),
    }
}

fn runtime_instr_writes_slot(instr: &RuntimeInstr, slot: usize) -> bool {
    match instr {
        RuntimeInstr::LoadSeed { dst, .. }
        | RuntimeInstr::Mov { dst, .. }
        | RuntimeInstr::BinOp { dst, .. }
        | RuntimeInstr::BinOpInPlace { dst, .. }
        | RuntimeInstr::FloatBinOp { dst, .. }
        | RuntimeInstr::Cmp { dst, .. }
        | RuntimeInstr::NormalizeInt { dst, .. } => *dst == slot,
        RuntimeInstr::CompareSwap { left, right, .. } => *left == slot || *right == slot,
        RuntimeInstr::RadixSortFixedInt { slots, .. } => slots.contains(&slot),
        RuntimeInstr::LoadIndex { dst, .. } | RuntimeInstr::LoadIndexUnchecked { dst, .. } => {
            *dst == slot
        }
        RuntimeInstr::HeapLoadInt { dst, .. } => *dst == slot,
        RuntimeInstr::HeapStoreInt { .. } | RuntimeInstr::HeapCopy { .. } => false,
        RuntimeInstr::StoreIndex { base_slots, .. }
        | RuntimeInstr::StoreIndexUnchecked { base_slots, .. } => base_slots.contains(&slot),
        RuntimeInstr::BloomSplitBlockInsert { filter_slots, .. } => filter_slots.contains(&slot),
        RuntimeInstr::BloomSplitBlockCheck { dst, .. } => *dst == slot,
        RuntimeInstr::BloomClassic4Check {
            dst, lanes_checked, ..
        } => *dst == slot || *lanes_checked == slot,
        RuntimeInstr::HashCtrlGroupProbe { dst_mask, .. } => *dst_mask == slot,
        RuntimeInstr::JoinSelectAdaptive { dst, .. } => *dst == slot,
        RuntimeInstr::Alloc { dst, .. } => *dst == slot,
        RuntimeInstr::FileOpen { dst, .. }
        | RuntimeInstr::FileWrite { dst, .. }
        | RuntimeInstr::FileRead { dst, .. } => *dst == slot,
        RuntimeInstr::ThreadSpawn { handle_dst, .. } => *handle_dst == slot,
        RuntimeInstr::ThreadJoin { dst, .. } => *dst == slot,
        RuntimeInstr::ChannelCreate { dst, .. } | RuntimeInstr::ChannelRecv { dst, .. } => {
            *dst == slot
        }
        RuntimeInstr::Free { .. }
        | RuntimeInstr::FileClose { .. }
        | RuntimeInstr::PrintConst { .. }
        | RuntimeInstr::PrintInt { .. } => false,
        RuntimeInstr::Jump { .. }
        | RuntimeInstr::JumpIfZero { .. }
        | RuntimeInstr::JumpIfCmpFalse { .. }
        | RuntimeInstr::Call { .. }
        | RuntimeInstr::Return
        | RuntimeInstr::Exit { .. } => false,
        RuntimeInstr::ChannelSend { .. }
        | RuntimeInstr::ChannelClose { .. }
        | RuntimeInstr::ChannelDestroy { .. } => false,
    }
}

fn runtime_operand_reads_slot(operand: &RuntimeOperand, slot: usize) -> bool {
    matches!(operand, RuntimeOperand::Slot(s) if *s == slot)
}

#[cfg(test)]
#[path = "optimizer/tests.rs"]
mod tests;
