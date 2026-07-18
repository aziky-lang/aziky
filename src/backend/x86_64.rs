use std::collections::{HashMap, HashSet};

use crate::backend::lir::{MachineLIRProgram, RuntimeSSAProgram};
use crate::backend::optimizer::{OptimizationReport, optimize_runtime_program};
use crate::backend::profile::{CompileProfile, FunctionProfile};
use crate::frontend::semantics::{
    RuntimeBinOp, RuntimeCmpOp, RuntimeFloatBinOp, RuntimeInstr, RuntimeLoadKind, RuntimeOperand,
    RuntimeProgram,
};
use crate::target::{KernelCallStyle, NativeRuntimeAbi, ProcessEntryAbi, TargetSpec};

#[path = "x86_64/windows.rs"]
mod windows;

/// Supported syscall programs for the Binary Zero milestone.
pub enum ProgramKind<'a> {
    ExitOnly,
    WriteAndExit { message: &'a [u8] },
}

pub struct X86Program {
    code: Vec<u8>,
    data: Vec<u8>,
    data_offsets: HashMap<Vec<u8>, usize>,
    patches: Vec<Patch>,
    kernel_call_patches: Vec<usize>,
    options: X86BackendOptions,
    runtime: NativeRuntimeAbi,
    runtime_generic_metadata: Option<RuntimeGenericMetadata>,
    runtime_allocator: Option<RuntimeAllocatorEmission>,
}

#[derive(Debug, Clone, Copy)]
struct RuntimeAllocatorFrame {
    head_disp: i32,
    cursor_disp: i32,
    end_disp: i32,
}

#[derive(Debug, Clone)]
struct RuntimeAllocatorEmission {
    frame: RuntimeAllocatorFrame,
    alloc_call_patches: Vec<usize>,
    free_call_patches: Vec<usize>,
    teardown_call_patches: Vec<usize>,
}

struct Patch {
    disp_pos: usize,
    data_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetFeatureSet {
    pub avx2: bool,
    pub avx512f: bool,
    pub bmi2: bool,
    pub popcnt: bool,
}

impl Default for TargetFeatureSet {
    fn default() -> Self {
        Self {
            avx2: true,
            avx512f: false,
            bmi2: true,
            popcnt: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X86BackendOptions {
    pub target: TargetSpec,
    pub target_cpu: String,
    pub target_features: TargetFeatureSet,
    pub runtime_generic_profile: Option<FunctionProfile>,
    /// Verification builds write the full pre-exit-mask result as eight
    /// little-endian bytes.  Normal binaries have no instrumentation.
    pub emit_full_checksum: bool,
    /// Keep the full value feeding a final small exit mask live in demanded-bit
    /// analysis.  The benchmark harness enables this for timed and verification
    /// binaries so both execute the same workload-defining state transitions.
    pub preserve_full_checksum: bool,
    /// Training builds count executions of every runtime-generic basic block
    /// and write a deterministic raw profile record to stderr on normal exit.
    pub profile_instrument: bool,
}

impl Default for X86BackendOptions {
    fn default() -> Self {
        Self {
            target: TargetSpec::default(),
            target_cpu: "native".to_string(),
            target_features: TargetFeatureSet::default(),
            runtime_generic_profile: None,
            emit_full_checksum: false,
            preserve_full_checksum: false,
            profile_instrument: false,
        }
    }
}

fn full_width_exit_operand(program: &RuntimeProgram, exit_index: usize) -> Option<RuntimeOperand> {
    let RuntimeInstr::Exit {
        code: RuntimeOperand::Slot(exit_slot),
    } = program.instrs.get(exit_index)?
    else {
        return None;
    };
    let control_flow = RuntimeSSAProgram::lower(program);
    let block = control_flow
        .blocks
        .iter()
        .find(|block| block.instr_indices.contains(&exit_index))?;
    for &instr_index in block.instr_indices.iter().rev() {
        if instr_index >= exit_index {
            continue;
        }
        let instr = &program.instrs[instr_index];
        if !runtime_instr_writes_slot(instr, *exit_slot) {
            continue;
        }
        return match instr {
            RuntimeInstr::BinOp {
                dst,
                op: RuntimeBinOp::BitAnd,
                lhs,
                rhs: RuntimeOperand::Imm(mask),
            } if dst == exit_slot && *mask <= 0xff => Some(*lhs),
            _ => None,
        };
    }
    None
}

#[derive(Debug, Clone)]
struct RuntimeGenericMetadata {
    lir: MachineLIRProgram,
    profile_template: CompileProfile,
    block_offsets: Vec<(usize, usize, usize)>,
    optimization_report: OptimizationReport,
}

#[derive(Clone, Copy)]
enum GpReg {
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    R14,
    R15,
}

enum MulPlan {
    Zero,
    One,
    Imm32(i32),
    Reg(GpReg),
}

enum AddPlan {
    Zero,
    Imm32(i32),
    Reg(GpReg),
}

enum CoeffPlan {
    Zero,
    One,
    Pow2(u8),
    Imm32(i32),
    Reg(GpReg),
}

struct AffinePlan {
    mul: MulPlan,
    add: AddPlan,
}

struct RuntimeSlotMap {
    reg_by_slot: Vec<Option<u8>>,
    stack_index_by_slot: Vec<Option<usize>>,
    byte_disp_by_slot: Vec<Option<i32>>,
    #[cfg(test)]
    stack_slots: usize,
    stack_bytes: usize,
    promoted_alloc_disp_by_slot: Vec<Option<i32>>,
}

impl RuntimeSlotMap {
    fn stack_only(slot_count: usize) -> Self {
        let mut stack_index_by_slot = vec![None; slot_count];
        for slot in (0..slot_count).rev() {
            stack_index_by_slot[slot] = Some(slot_count - 1 - slot);
        }
        Self {
            reg_by_slot: vec![None; slot_count],
            stack_index_by_slot,
            byte_disp_by_slot: vec![None; slot_count],
            #[cfg(test)]
            stack_slots: slot_count,
            stack_bytes: slot_count * 8,
            promoted_alloc_disp_by_slot: vec![None; slot_count],
        }
    }

    #[allow(dead_code)]
    fn build(program: &RuntimeProgram) -> Self {
        Self::build_with_profile(program, None)
    }

    fn build_with_profile(program: &RuntimeProgram, profile: Option<&FunctionProfile>) -> Self {
        match MachineLIRProgram::lower(program, profile) {
            Ok(lir) => Self::from_lir(&lir),
            Err(_) => Self::build_legacy(program),
        }
    }

    fn build_legacy(program: &RuntimeProgram) -> Self {
        let mut counts = vec![0u32; program.slots];
        let mut force_stack = vec![false; program.slots];
        for instr in &program.instrs {
            bump_instr_uses(&mut counts, instr, 1);
            mark_force_stack(&mut force_stack, instr);
        }

        const LOOP_WEIGHT: u32 = 8;
        for (idx, instr) in program.instrs.iter().enumerate() {
            let back_target = match instr {
                RuntimeInstr::Jump { target } if *target <= idx => Some(*target),
                RuntimeInstr::JumpIfZero { target, .. } if *target <= idx => Some(*target),
                RuntimeInstr::JumpIfCmpFalse { target, .. } if *target <= idx => Some(*target),
                _ => None,
            };
            if let Some(start) = back_target {
                for loop_instr in &program.instrs[start..=idx] {
                    bump_instr_uses(&mut counts, loop_instr, LOOP_WEIGHT);
                }
            }
        }

        let mut slots: Vec<usize> = (0..program.slots).collect();
        slots.sort_by_key(|&slot| (std::cmp::Reverse(counts[slot]), slot));
        let mut reg_by_slot = vec![None; program.slots];
        for (slot, reg) in slots
            .into_iter()
            .filter(|slot| counts[*slot] > 0 && !force_stack[*slot])
            .zip(allocatable_regs())
        {
            reg_by_slot[slot] = Some(reg);
        }

        let mut stack_index_by_slot = vec![None; program.slots];
        let mut stack_slots = 0usize;
        for slot in (0..program.slots).rev() {
            if reg_by_slot[slot].is_none() {
                stack_index_by_slot[slot] = Some(stack_slots);
                stack_slots += 1;
            }
        }

        Self {
            reg_by_slot,
            stack_index_by_slot,
            byte_disp_by_slot: vec![None; program.slots],
            #[cfg(test)]
            stack_slots,
            stack_bytes: stack_slots * 8,
            promoted_alloc_disp_by_slot: vec![None; program.slots],
        }
    }

    fn from_lir(lir: &MachineLIRProgram) -> Self {
        let mut reg_by_slot = vec![None; lir.slot_count];
        let intervals = lir.compute_live_intervals();
        let regs = allocatable_regs();
        let mut interval_by_slot = vec![None; lir.slot_count];
        for (index, interval) in intervals.iter().enumerate() {
            interval_by_slot[interval.slot] = Some(index);
        }
        let mut allocation_order: Vec<usize> = (0..intervals.len())
            .filter(|index| !intervals[*index].force_stack)
            .collect();
        allocation_order.sort_by_key(|index| {
            let interval = &intervals[*index];
            let effective_weight = if interval.rematerializable.is_some() {
                interval.spill_weight / 4
            } else {
                interval.spill_weight
            };
            (
                std::cmp::Reverse(effective_weight),
                std::cmp::Reverse(interval.segments.len()),
                interval.start,
                interval.slot,
            )
        });

        let mut assigned_by_reg = vec![Vec::<usize>::new(); regs.len()];
        for interval_index in allocation_order {
            let interval = &intervals[interval_index];
            let preferred = interval
                .copy_hint
                .and_then(|slot| reg_by_slot.get(slot).copied().flatten())
                .or_else(|| {
                    intervals.iter().find_map(|other| {
                        (other.copy_hint == Some(interval.slot))
                            .then(|| reg_by_slot.get(other.slot).copied().flatten())
                            .flatten()
                    })
                })
                .and_then(|reg| regs.iter().position(|candidate| *candidate == reg));
            let mut register_order = Vec::with_capacity(regs.len());
            if let Some(preferred) = preferred {
                register_order.push(preferred);
            }
            for register in 0..regs.len() {
                if Some(register) != preferred {
                    register_order.push(register);
                }
            }
            if let Some(register) = register_order.into_iter().find(|register| {
                assigned_by_reg[*register].iter().all(|other| {
                    !interval.interferes(&intervals[*other])
                        || copy_coalescible(lir, interval, &intervals[*other])
                })
            }) {
                reg_by_slot[interval.slot] = Some(regs[register]);
                assigned_by_reg[register].push(interval_index);
            }
        }

        let mut stack_index_by_slot = vec![None; lir.slot_count];
        let mut stack_slots = 0usize;
        let mut packed_byte = vec![false; lir.slot_count];
        for object in &lir.objects {
            if object.representation == crate::backend::lir::MemoryRepresentation::PackedBytes {
                for &slot in &object.slots {
                    packed_byte[slot] = true;
                }
            }
        }
        // Addressable objects retain unique, contiguous stack slots. Reverse slot
        // order preserves the existing ascending-address array convention.
        for slot in (0..lir.slot_count).rev() {
            let Some(interval_index) = interval_by_slot[slot] else {
                continue;
            };
            if intervals[interval_index].force_stack && !packed_byte[slot] {
                stack_index_by_slot[slot] = Some(stack_slots);
                stack_slots += 1;
            }
        }

        // Scalar spills with disjoint segmented lifetimes share frame storage.
        let mut stack_colors = Vec::<(usize, Vec<usize>)>::new();
        let mut spilled_intervals: Vec<usize> = (0..intervals.len())
            .filter(|index| {
                !intervals[*index].force_stack && reg_by_slot[intervals[*index].slot].is_none()
            })
            .collect();
        spilled_intervals.sort_by_key(|index| {
            (
                std::cmp::Reverse(intervals[*index].spill_weight),
                intervals[*index].slot,
            )
        });
        for interval_index in spilled_intervals {
            let interval = &intervals[interval_index];
            let color = stack_colors.iter_mut().find(|(_, occupants)| {
                occupants
                    .iter()
                    .all(|other| !interval.interferes(&intervals[*other]))
            });
            let stack_index = if let Some((stack_index, occupants)) = color {
                occupants.push(interval_index);
                *stack_index
            } else {
                let stack_index = stack_slots;
                stack_slots += 1;
                stack_colors.push((stack_index, vec![interval_index]));
                stack_index
            };
            stack_index_by_slot[interval.slot] = Some(stack_index);
        }

        let (promoted_alloc_disp_by_slot, promoted_words) =
            plan_promoted_allocations(lir, stack_slots);
        stack_slots += promoted_words;

        let mut byte_disp_by_slot = vec![None; lir.slot_count];
        let mut packed_bytes = 0usize;
        for object in &lir.objects {
            if object.representation != crate::backend::lir::MemoryRepresentation::PackedBytes {
                continue;
            }
            let base = stack_slots * 8 + packed_bytes + object.slots.len();
            for (index, &slot) in object.slots.iter().enumerate() {
                byte_disp_by_slot[slot] = Some(-(base as i32) + index as i32);
            }
            packed_bytes += object.slots.len();
        }

        Self {
            reg_by_slot,
            stack_index_by_slot,
            byte_disp_by_slot,
            #[cfg(test)]
            stack_slots,
            stack_bytes: stack_slots * 8 + packed_bytes,
            promoted_alloc_disp_by_slot,
        }
    }

    fn reg(&self, slot: usize) -> Option<u8> {
        self.reg_by_slot.get(slot).copied().flatten()
    }

    fn stack_index(&self, slot: usize) -> Option<usize> {
        self.stack_index_by_slot.get(slot).copied().flatten()
    }

    #[cfg(test)]
    fn stack_slots(&self) -> usize {
        self.stack_slots
    }

    fn stack_bytes(&self) -> usize {
        self.stack_bytes
    }

    fn stack_disp(&self, slot: usize) -> Option<i32> {
        self.byte_disp_by_slot
            .get(slot)
            .copied()
            .flatten()
            .or_else(|| self.stack_index(slot).map(stack_slot_disp))
    }

    fn element_width(&self, slot: usize) -> u8 {
        if self
            .byte_disp_by_slot
            .get(slot)
            .copied()
            .flatten()
            .is_some()
        {
            1
        } else {
            8
        }
    }

    fn promoted_alloc_disp(&self, slot: usize) -> Option<i32> {
        self.promoted_alloc_disp_by_slot
            .get(slot)
            .copied()
            .flatten()
    }
}

fn plan_promoted_allocations(
    lir: &MachineLIRProgram,
    scalar_stack_words: usize,
) -> (Vec<Option<i32>>, usize) {
    const MAX_STACK_PROMOTION: u64 = 4096;
    let mut plan = vec![None; lir.slot_count];
    let mut added_words = 0usize;
    let mut instructions: Vec<(usize, &RuntimeInstr, usize)> = lir
        .blocks
        .iter()
        .flat_map(|block| {
            block
                .instr_indices
                .iter()
                .copied()
                .zip(block.instrs.iter())
                .map(move |(index, instr)| (index, instr, block.loop_depth))
        })
        .collect();
    instructions.sort_by_key(|(index, _, _)| *index);

    for (_, instr, loop_depth) in &instructions {
        let RuntimeInstr::Alloc {
            dst,
            size: RuntimeOperand::Imm(size),
        } = instr
        else {
            continue;
        };
        if *loop_depth != 0 || *size == 0 || *size > MAX_STACK_PROMOTION {
            continue;
        }
        let writes = instructions
            .iter()
            .filter(|(_, candidate, _)| runtime_instr_writes_slot(candidate, *dst))
            .count();
        if writes != 1 {
            continue;
        }
        let mut saw_free = false;
        let no_escape = instructions.iter().all(|(_, candidate, _)| {
            if !runtime_instr_reads_slot(candidate, *dst) {
                return true;
            }
            match candidate {
                RuntimeInstr::Free {
                    ptr: RuntimeOperand::Slot(ptr),
                    size: RuntimeOperand::Imm(free_size),
                } if ptr == dst && free_size == size => {
                    saw_free = true;
                    true
                }
                _ => false,
            }
        });
        if !no_escape || !saw_free {
            continue;
        }
        let words = (*size as usize).div_ceil(8).max(1);
        added_words = added_words.saturating_add(words);
        let end_word = scalar_stack_words.saturating_add(added_words);
        plan[*dst] = Some(-((end_word as i32) * 8));
    }
    (plan, added_words)
}

fn copy_coalescible(
    lir: &MachineLIRProgram,
    first: &crate::backend::lir::LiveInterval,
    second: &crate::backend::lir::LiveInterval,
) -> bool {
    let (destination, source) = if first.copy_hint == Some(second.slot) {
        (first, second)
    } else if second.copy_hint == Some(first.slot) {
        (second, first)
    } else {
        return false;
    };
    if destination.copy_hint != Some(source.slot) || destination.start != source.end {
        return false;
    }
    lir.blocks.iter().any(|block| {
        block
            .instr_indices
            .iter()
            .zip(&block.instrs)
            .any(|(index, instr)| {
                *index == destination.start
                    && (matches!(
                        instr,
                        RuntimeInstr::Mov {
                            dst,
                            src: RuntimeOperand::Slot(src),
                        } if *dst == destination.slot && *src == source.slot
                    ) || matches!(
                        instr,
                        RuntimeInstr::BinOp {
                            dst,
                            lhs: RuntimeOperand::Slot(src),
                            ..
                        } if *dst == destination.slot && *src == source.slot
                    ))
            })
    })
}

fn allocatable_regs() -> [u8; 11] {
    [12u8, 13u8, 14u8, 15u8, 3u8, 8u8, 9u8, 10u8, 11u8, 6u8, 7u8]
}

fn mark_force_stack(force_stack: &mut [bool], instr: &RuntimeInstr) {
    match instr {
        RuntimeInstr::LoadIndex {
            base_slots, index, ..
        }
        | RuntimeInstr::LoadIndexUnchecked {
            base_slots, index, ..
        }
        | RuntimeInstr::StoreIndex {
            base_slots, index, ..
        }
        | RuntimeInstr::StoreIndexUnchecked {
            base_slots, index, ..
        } => {
            if runtime_const_index_for_access(base_slots, index).is_none() {
                for &slot in base_slots {
                    if let Some(entry) = force_stack.get_mut(slot) {
                        *entry = true;
                    }
                }
            }
        }
        RuntimeInstr::BloomSplitBlockInsert { filter_slots, .. }
        | RuntimeInstr::BloomSplitBlockCheck { filter_slots, .. } => {
            for &slot in filter_slots {
                if let Some(entry) = force_stack.get_mut(slot) {
                    *entry = true;
                }
            }
        }
        RuntimeInstr::HashCtrlGroupProbe { ctrl_slots, .. } => {
            for &slot in ctrl_slots {
                if let Some(entry) = force_stack.get_mut(slot) {
                    *entry = true;
                }
            }
        }
        _ => {}
    }
}

include!("x86_64/emit_impl.rs");

fn patch_rel32(code: &mut [u8], disp_pos: usize, target: usize) {
    let rip_after = (disp_pos + 4) as i64;
    let disp = (target as i64) - rip_after;
    let disp = i32::try_from(disp).expect("rel32 displacement out of range");
    code[disp_pos..disp_pos + 4].copy_from_slice(&disp.to_le_bytes());
}

fn stack_slot_disp(stack_index: usize) -> i32 {
    -((stack_index as i32 + 1) * 8)
}

fn runtime_cmp_jumpifzero_fusion_candidate(
    program: &RuntimeProgram,
    idx: usize,
) -> Option<(RuntimeCmpOp, RuntimeOperand, RuntimeOperand, usize)> {
    if idx + 1 >= program.instrs.len() {
        return None;
    }
    let (dst, op, lhs, rhs) = match &program.instrs[idx] {
        RuntimeInstr::Cmp { dst, op, lhs, rhs } => (*dst, *op, *lhs, *rhs),
        _ => return None,
    };
    let target = match &program.instrs[idx + 1] {
        RuntimeInstr::JumpIfZero { cond_slot, target } if *cond_slot == dst => *target,
        _ => return None,
    };
    // Keep cmp materialization if that temporary is observed later.
    if runtime_slot_read_before_write(program, idx + 2, dst) {
        return None;
    }
    Some((op, lhs, rhs, target))
}

struct RuntimeBloomClassic4JumpFusion {
    filter_slots: Vec<usize>,
    hash: RuntimeOperand,
    target: usize,
}

fn runtime_bloom_classic4_jump_fusion_candidate(
    program: &RuntimeProgram,
    idx: usize,
    has_incoming_target: &[bool],
) -> Option<RuntimeBloomClassic4JumpFusion> {
    if idx + 1 >= program.instrs.len() || has_incoming_target.get(idx + 1).copied().unwrap_or(false)
    {
        return None;
    }

    let (dst, lanes_checked, filter_slots, hash) = match &program.instrs[idx] {
        RuntimeInstr::BloomClassic4Check {
            dst,
            lanes_checked,
            filter_slots,
            hash,
        } => (*dst, *lanes_checked, filter_slots.clone(), *hash),
        _ => return None,
    };
    let (op, lhs, rhs, target) = match &program.instrs[idx + 1] {
        RuntimeInstr::JumpIfCmpFalse {
            op,
            lhs,
            rhs,
            target,
        } => (*op, *lhs, *rhs, *target),
        _ => return None,
    };
    let checks_success = op == RuntimeCmpOp::Eq
        && (matches!((lhs, rhs), (RuntimeOperand::Slot(slot), RuntimeOperand::Imm(1)) if slot == dst)
            || matches!((lhs, rhs), (RuntimeOperand::Imm(1), RuntimeOperand::Slot(slot)) if slot == dst));
    if !checks_success {
        return None;
    }

    // The fused instruction branches before materializing either source-level
    // loop variable.  That is legal only when the branch is the final reader
    // of `dst` and the lane counter is not observed anywhere.
    if runtime_slot_read_before_write(program, idx + 2, dst)
        || program.instrs.iter().enumerate().any(|(other_idx, instr)| {
            other_idx != idx && runtime_instr_reads_slot(instr, lanes_checked)
        })
    {
        return None;
    }

    Some(RuntimeBloomClassic4JumpFusion {
        filter_slots,
        hash,
        target,
    })
}

struct RuntimeBitTestBoolFusion {
    dst: usize,
    value: RuntimeOperand,
    bit: RuntimeOperand,
}

struct RuntimeShiftOrFusion {
    dst: usize,
    value: RuntimeOperand,
    shift: u8,
    rhs: RuntimeOperand,
}

fn runtime_shift_or_fusion_candidate(
    program: &RuntimeProgram,
    idx: usize,
    has_incoming_target: &[bool],
) -> Option<RuntimeShiftOrFusion> {
    if idx + 1 >= program.instrs.len() || has_incoming_target.get(idx + 1).copied().unwrap_or(false)
    {
        return None;
    }
    let (shifted_slot, value, shift) = match &program.instrs[idx] {
        RuntimeInstr::BinOp {
            dst,
            op: RuntimeBinOp::Shl,
            lhs,
            rhs: RuntimeOperand::Imm(shift),
        } if *shift < 64 => (*dst, *lhs, *shift as u8),
        _ => return None,
    };
    let (dst, rhs) = match &program.instrs[idx + 1] {
        RuntimeInstr::BinOp {
            dst,
            op: RuntimeBinOp::BitOr,
            lhs: RuntimeOperand::Slot(lhs_slot),
            rhs,
        } if *lhs_slot == shifted_slot => (*dst, *rhs),
        _ => return None,
    };
    if runtime_slot_read_before_write(program, idx + 2, shifted_slot) {
        return None;
    }
    if let RuntimeOperand::Slot(value_slot) = value {
        if value_slot != dst && runtime_slot_read_before_write(program, idx + 2, value_slot) {
            return None;
        }
    }
    Some(RuntimeShiftOrFusion {
        dst,
        value,
        shift,
        rhs,
    })
}

struct RuntimeBitTestIndexedFusion {
    dst: usize,
    base_slots: Vec<usize>,
    index: RuntimeOperand,
    bit: RuntimeOperand,
    checked: bool,
}

struct RuntimeLoadIndexCmpJumpFusion {
    base_slots: Vec<usize>,
    index: RuntimeOperand,
    checked: bool,
    op: RuntimeCmpOp,
    other: RuntimeOperand,
    target: usize,
}

fn runtime_bit_test_bool_fusion_candidate(
    program: &RuntimeProgram,
    idx: usize,
    has_incoming_target: &[bool],
) -> Option<RuntimeBitTestBoolFusion> {
    if idx + 1 >= program.instrs.len() {
        return None;
    }
    if has_incoming_target.get(idx + 1).copied().unwrap_or(false) {
        return None;
    }

    let (shift_dst, value, bit) = match &program.instrs[idx] {
        RuntimeInstr::BinOp {
            dst,
            op: RuntimeBinOp::ShrUnsigned,
            lhs,
            rhs,
        } => (*dst, *lhs, *rhs),
        _ => return None,
    };
    let (dst, lhs, rhs) = match &program.instrs[idx + 1] {
        RuntimeInstr::BinOp {
            dst,
            op: RuntimeBinOp::BitAnd,
            lhs,
            rhs,
        } => (*dst, *lhs, *rhs),
        _ => return None,
    };

    let uses_shift_and_one = matches!(
        (lhs, rhs),
        (RuntimeOperand::Slot(s), RuntimeOperand::Imm(1)) if s == shift_dst
    ) || matches!(
        (lhs, rhs),
        (RuntimeOperand::Imm(1), RuntimeOperand::Slot(s)) if s == shift_dst
    );
    if !uses_shift_and_one {
        return None;
    }

    if shift_dst != dst && runtime_slot_read_before_write(program, idx + 2, shift_dst) {
        return None;
    }

    Some(RuntimeBitTestBoolFusion { dst, value, bit })
}

fn runtime_bit_test_indexed_fusion_candidate(
    program: &RuntimeProgram,
    idx: usize,
    has_incoming_target: &[bool],
) -> Option<RuntimeBitTestIndexedFusion> {
    if idx + 2 >= program.instrs.len() {
        return None;
    }
    for lookahead in 1..=2 {
        if has_incoming_target
            .get(idx + lookahead)
            .copied()
            .unwrap_or(false)
        {
            return None;
        }
    }

    let (word_slot, base_slots, index, checked) = match &program.instrs[idx] {
        RuntimeInstr::LoadIndex {
            dst,
            base_slots,
            index,
        } => (*dst, base_slots.clone(), *index, true),
        RuntimeInstr::LoadIndexUnchecked {
            dst,
            base_slots,
            index,
        } => (*dst, base_slots.clone(), *index, false),
        _ => return None,
    };
    let (shift_dst, bit) = match &program.instrs[idx + 1] {
        RuntimeInstr::BinOp {
            dst,
            op: RuntimeBinOp::ShrUnsigned,
            lhs: RuntimeOperand::Slot(lhs_slot),
            rhs,
        } if *lhs_slot == word_slot => (*dst, *rhs),
        _ => return None,
    };
    let dst = match &program.instrs[idx + 2] {
        RuntimeInstr::BinOp {
            dst,
            op: RuntimeBinOp::BitAnd,
            lhs,
            rhs,
        } if (matches!((lhs, rhs), (RuntimeOperand::Slot(s), RuntimeOperand::Imm(1)) if *s == shift_dst)
            || matches!((lhs, rhs), (RuntimeOperand::Imm(1), RuntimeOperand::Slot(s)) if *s == shift_dst)) =>
        {
            *dst
        }
        _ => return None,
    };

    if runtime_slot_read_before_write(program, idx + 3, word_slot) {
        return None;
    }

    Some(RuntimeBitTestIndexedFusion {
        dst,
        base_slots,
        index,
        bit,
        checked,
    })
}

fn runtime_load_index_cmp_jump_fusion_candidate(
    program: &RuntimeProgram,
    idx: usize,
    has_incoming_target: &[bool],
) -> Option<RuntimeLoadIndexCmpJumpFusion> {
    if idx + 1 >= program.instrs.len() {
        return None;
    }
    if has_incoming_target.get(idx + 1).copied().unwrap_or(false) {
        return None;
    }

    let (dst, base_slots, index, checked) = match &program.instrs[idx] {
        RuntimeInstr::LoadIndex {
            dst,
            base_slots,
            index,
        } => (*dst, base_slots.clone(), *index, true),
        RuntimeInstr::LoadIndexUnchecked {
            dst,
            base_slots,
            index,
        } => (*dst, base_slots.clone(), *index, false),
        _ => return None,
    };
    let (op, lhs, rhs, target) = match &program.instrs[idx + 1] {
        RuntimeInstr::JumpIfCmpFalse {
            op,
            lhs,
            rhs,
            target,
        } => (*op, *lhs, *rhs, *target),
        _ => return None,
    };
    if target <= idx {
        // Keep loop/back-edge value materialization conservative.
        return None;
    }

    let (op, other) = match (lhs, rhs) {
        (RuntimeOperand::Slot(slot), other) if slot == dst => (op, other),
        (other, RuntimeOperand::Slot(slot)) if slot == dst => (flip_cmp_operands(op), other),
        _ => return None,
    };
    if matches!(other, RuntimeOperand::Slot(slot) if slot == dst) {
        return None;
    }
    if runtime_slot_read_before_write(program, idx + 2, dst) {
        return None;
    }

    Some(RuntimeLoadIndexCmpJumpFusion {
        base_slots,
        index,
        checked,
        op,
        other,
        target,
    })
}

struct RuntimeBitSetStoreFusion {
    word_slot: usize,
    bit: RuntimeOperand,
    merged_slot: usize,
    base_slots: Vec<usize>,
    index: RuntimeOperand,
    load_checked: bool,
    store_checked: bool,
    merged_read_later: bool,
}

struct RuntimeIndexIncrementFusion {
    base_slots: Vec<usize>,
    index: RuntimeOperand,
}

struct RuntimeExactUnrollEmissionPlan {
    suppress_guard: Vec<bool>,
    induction_increment: Vec<Option<u64>>,
}

/// Keeps the IR guards that define basic blocks and SSA lifetimes, but omits
/// provably redundant machine-level guards in an exact eager-unroll group.
/// This avoids changing register-allocation semantics while giving the emitted
/// loop the same single-guard shape as a conventional unroller.
fn runtime_exact_unroll_emission_plan(program: &RuntimeProgram) -> RuntimeExactUnrollEmissionPlan {
    let mut plan = RuntimeExactUnrollEmissionPlan {
        suppress_guard: vec![false; program.instrs.len()],
        induction_increment: vec![None; program.instrs.len()],
    };
    for (latch, instr) in program.instrs.iter().enumerate() {
        let RuntimeInstr::Jump { target: header } = instr else {
            continue;
        };
        if *header >= latch {
            continue;
        }
        let (induction, limit, exit) = match &program.instrs[*header] {
            RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Slot(induction),
                rhs: RuntimeOperand::Imm(limit),
                target: exit,
            } if *exit == latch + 1 => (*induction, *limit, *exit),
            _ => continue,
        };
        let start = program.instrs[..*header]
            .iter()
            .rev()
            .find(|candidate| runtime_instr_writes_slot(candidate, induction))
            .and_then(|candidate| match candidate {
                RuntimeInstr::Mov {
                    dst,
                    src: RuntimeOperand::Imm(start),
                } if *dst == induction => Some(*start),
                _ => None,
            });
        let Some(start) = start.filter(|start| *start <= limit) else {
            continue;
        };

        let mut guards = Vec::new();
        let mut increments = Vec::new();
        let mut valid = true;
        for idx in *header..latch {
            match &program.instrs[idx] {
                RuntimeInstr::JumpIfCmpFalse {
                    op: RuntimeCmpOp::LtUnsigned,
                    lhs: RuntimeOperand::Slot(slot),
                    rhs: RuntimeOperand::Imm(bound),
                    target,
                } if *slot == induction && *bound == limit && *target == exit => {
                    guards.push(idx);
                }
                RuntimeInstr::BinOpInPlace {
                    dst,
                    op: RuntimeBinOp::Add,
                    rhs: RuntimeOperand::Imm(1),
                } if *dst == induction => increments.push(idx),
                candidate if runtime_instr_writes_slot(candidate, induction) => {
                    valid = false;
                    break;
                }
                RuntimeInstr::Jump { .. }
                | RuntimeInstr::JumpIfZero { .. }
                | RuntimeInstr::JumpIfCmpFalse { .. }
                | RuntimeInstr::Call { .. }
                | RuntimeInstr::Return
                | RuntimeInstr::Exit { .. } => {
                    valid = false;
                    break;
                }
                _ => {}
            }
        }
        if !valid
            || guards.len() < 2
            || guards.len() != increments.len()
            || guards[0] != *header
            || (limit - start) % guards.len() as u64 != 0
        {
            continue;
        }
        for guard in guards.iter().skip(1) {
            plan.suppress_guard[*guard] = true;
        }

        let induction_used_by_body = (*header..latch).any(|idx| {
            !guards.contains(&idx)
                && !increments.contains(&idx)
                && runtime_instr_reads_slot(&program.instrs[idx], induction)
        });
        if !induction_used_by_body {
            for increment in increments.iter().take(increments.len() - 1) {
                plan.induction_increment[*increment] = Some(0);
            }
            plan.induction_increment[*increments.last().expect("non-empty increments")] =
                Some(increments.len() as u64);
        }
    }
    plan
}

fn runtime_index_increment_fusion_candidate(
    program: &RuntimeProgram,
    idx: usize,
    has_incoming_target: &[bool],
) -> Option<RuntimeIndexIncrementFusion> {
    if idx + 2 >= program.instrs.len()
        || (1..=2).any(|offset| {
            has_incoming_target
                .get(idx + offset)
                .copied()
                .unwrap_or(false)
        })
    {
        return None;
    }
    let (loaded, base_slots, index) = match &program.instrs[idx] {
        RuntimeInstr::LoadIndexUnchecked {
            dst,
            base_slots,
            index,
        } => (*dst, base_slots, *index),
        _ => return None,
    };
    let incremented = match &program.instrs[idx + 1] {
        RuntimeInstr::BinOp {
            dst,
            op: RuntimeBinOp::Add,
            lhs: RuntimeOperand::Slot(source),
            rhs: RuntimeOperand::Imm(1),
        } if *source == loaded => *dst,
        RuntimeInstr::BinOpInPlace {
            dst,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Imm(1),
        } if *dst == loaded => loaded,
        _ => return None,
    };
    let (stored_base, stored_index) = match &program.instrs[idx + 2] {
        RuntimeInstr::StoreIndexUnchecked {
            base_slots,
            index,
            src: RuntimeOperand::Slot(source),
        } if *source == incremented => (base_slots, index),
        _ => return None,
    };
    if base_slots != stored_base
        || !runtime_operand_same(&index, stored_index)
        || runtime_slot_read_before_write(program, idx + 3, loaded)
        || (incremented != loaded && runtime_slot_read_before_write(program, idx + 3, incremented))
    {
        return None;
    }
    Some(RuntimeIndexIncrementFusion {
        base_slots: base_slots.clone(),
        index,
    })
}

struct RuntimeU32AffineFusion {
    lhs: RuntimeOperand,
    mul: u32,
    add: u32,
    narrowed_slot: usize,
    state_slot: Option<usize>,
    consumed: usize,
}

fn runtime_u32_affine_fusion_candidate(
    program: &RuntimeProgram,
    idx: usize,
    has_incoming_target: &[bool],
) -> Option<RuntimeU32AffineFusion> {
    if idx + 2 >= program.instrs.len()
        || (1..=2).any(|offset| {
            has_incoming_target
                .get(idx + offset)
                .copied()
                .unwrap_or(false)
        })
    {
        return None;
    }
    let (mul_slot, lhs, mul) = match &program.instrs[idx] {
        RuntimeInstr::BinOp {
            dst,
            op: RuntimeBinOp::Mul,
            lhs,
            rhs: RuntimeOperand::Imm(mul),
        } => (*dst, *lhs, *mul as u32),
        _ => return None,
    };
    let (add_slot, add) = match &program.instrs[idx + 1] {
        RuntimeInstr::BinOp {
            dst,
            op: RuntimeBinOp::Add,
            lhs: RuntimeOperand::Slot(source),
            rhs: RuntimeOperand::Imm(add),
        } if *source == mul_slot => (*dst, *add as u32),
        _ => return None,
    };
    let narrowed_slot = match &program.instrs[idx + 2] {
        RuntimeInstr::BinOp {
            dst,
            op: RuntimeBinOp::BitAnd,
            lhs: RuntimeOperand::Slot(source),
            rhs: RuntimeOperand::Imm(mask),
        } if *source == add_slot && *mask == u64::from(u32::MAX) => *dst,
        _ => return None,
    };
    let state_slot = program.instrs.get(idx + 3).and_then(|instr| match instr {
        RuntimeInstr::Mov {
            dst,
            src: RuntimeOperand::Slot(source),
        } if *source == narrowed_slot
            && !has_incoming_target.get(idx + 3).copied().unwrap_or(false) =>
        {
            Some(*dst)
        }
        _ => None,
    });
    let consumed = if state_slot.is_some() { 4 } else { 3 };
    if [add_slot, narrowed_slot].contains(&mul_slot)
        || narrowed_slot == add_slot
        || state_slot.is_some_and(|state_slot| state_slot == mul_slot || state_slot == add_slot)
        || runtime_slot_read_before_write(program, idx + consumed, mul_slot)
        || runtime_slot_read_before_write(program, idx + consumed, add_slot)
    {
        return None;
    }
    Some(RuntimeU32AffineFusion {
        lhs,
        mul,
        add,
        narrowed_slot,
        state_slot,
        consumed,
    })
}

fn runtime_operand_same(lhs: &RuntimeOperand, rhs: &RuntimeOperand) -> bool {
    match (lhs, rhs) {
        (RuntimeOperand::Imm(a), RuntimeOperand::Imm(b)) => a == b,
        (RuntimeOperand::Slot(a), RuntimeOperand::Slot(b)) => a == b,
        _ => false,
    }
}

fn runtime_bitset_store_fusion_candidate(
    program: &RuntimeProgram,
    idx: usize,
    has_incoming_target: &[bool],
) -> Option<RuntimeBitSetStoreFusion> {
    if idx + 3 >= program.instrs.len() {
        return None;
    }
    for lookahead in 1..=3 {
        if has_incoming_target
            .get(idx + lookahead)
            .copied()
            .unwrap_or(false)
        {
            return None;
        }
    }

    let (word_slot, load_base, load_index, load_checked) = match &program.instrs[idx] {
        RuntimeInstr::LoadIndex {
            dst,
            base_slots,
            index,
        } => (*dst, base_slots, *index, true),
        RuntimeInstr::LoadIndexUnchecked {
            dst,
            base_slots,
            index,
        } => (*dst, base_slots, *index, false),
        _ => return None,
    };

    let (mask_slot, bit) = match &program.instrs[idx + 1] {
        RuntimeInstr::BinOp {
            dst,
            op: RuntimeBinOp::Shl,
            lhs: RuntimeOperand::Imm(1),
            rhs,
        } => (*dst, *rhs),
        _ => return None,
    };
    if mask_slot == word_slot {
        return None;
    }

    let merged_slot = match &program.instrs[idx + 2] {
        RuntimeInstr::BinOp {
            dst,
            op: RuntimeBinOp::BitOr,
            lhs: RuntimeOperand::Slot(lhs_slot),
            rhs: RuntimeOperand::Slot(rhs_slot),
        } if (*lhs_slot == word_slot && *rhs_slot == mask_slot)
            || (*lhs_slot == mask_slot && *rhs_slot == word_slot) =>
        {
            *dst
        }
        _ => return None,
    };

    let (store_base, store_index, src_slot, store_checked) = match &program.instrs[idx + 3] {
        RuntimeInstr::StoreIndex {
            base_slots,
            index,
            src: RuntimeOperand::Slot(src_slot),
        } => (base_slots, index, *src_slot, true),
        RuntimeInstr::StoreIndexUnchecked {
            base_slots,
            index,
            src: RuntimeOperand::Slot(src_slot),
        } => (base_slots, index, *src_slot, false),
        _ => return None,
    };

    if src_slot != merged_slot {
        return None;
    }
    if load_base != store_base || !runtime_operand_same(&load_index, store_index) {
        return None;
    }
    if mask_slot != merged_slot && runtime_slot_read_before_write(program, idx + 4, mask_slot) {
        return None;
    }

    Some(RuntimeBitSetStoreFusion {
        word_slot,
        bit,
        merged_slot,
        base_slots: load_base.clone(),
        index: load_index,
        load_checked,
        store_checked,
        merged_read_later: runtime_slot_read_before_write(program, idx + 4, merged_slot),
    })
}

fn runtime_slot_read_before_write(program: &RuntimeProgram, start: usize, slot: usize) -> bool {
    let mut pending = vec![start];
    let mut visited = HashSet::new();
    while let Some(index) = pending.pop() {
        if index >= program.instrs.len() || !visited.insert(index) {
            continue;
        }
        let instr = &program.instrs[index];
        if runtime_instr_reads_slot(instr, slot) {
            return true;
        }
        if runtime_instr_writes_slot(instr, slot) {
            continue;
        }
        match instr {
            RuntimeInstr::Jump { target } => pending.push(*target),
            RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. } => {
                pending.push(*target);
                pending.push(index + 1);
            }
            RuntimeInstr::Call { target } => {
                pending.push(*target);
                pending.push(index + 1);
            }
            RuntimeInstr::Return | RuntimeInstr::Exit { .. } => {}
            _ => pending.push(index + 1),
        }
    }
    false
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
        RuntimeInstr::Call { .. } | RuntimeInstr::Return => false,
        RuntimeInstr::Exit { code } => runtime_operand_reads_slot(code, slot),
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
        RuntimeInstr::Exit { .. }
        | RuntimeInstr::Jump { .. }
        | RuntimeInstr::JumpIfZero { .. }
        | RuntimeInstr::JumpIfCmpFalse { .. }
        | RuntimeInstr::Call { .. }
        | RuntimeInstr::Return => false,
        RuntimeInstr::LoadIndex { dst, .. } => *dst == slot,
        RuntimeInstr::LoadIndexUnchecked { dst, .. } => *dst == slot,
        RuntimeInstr::HeapLoadInt { dst, .. } => *dst == slot,
        RuntimeInstr::StoreIndex { base_slots, .. } => base_slots.contains(&slot),
        RuntimeInstr::StoreIndexUnchecked { base_slots, .. } => base_slots.contains(&slot),
        RuntimeInstr::BloomSplitBlockInsert { filter_slots, .. } => filter_slots.contains(&slot),
        RuntimeInstr::BloomSplitBlockCheck { dst, .. } => *dst == slot,
        RuntimeInstr::BloomClassic4Check {
            dst, lanes_checked, ..
        } => *dst == slot || *lanes_checked == slot,
        RuntimeInstr::HashCtrlGroupProbe { dst_mask, .. } => *dst_mask == slot,
        RuntimeInstr::JoinSelectAdaptive { dst, .. } => *dst == slot,
        RuntimeInstr::Alloc { dst, .. }
        | RuntimeInstr::FileOpen { dst, .. }
        | RuntimeInstr::FileWrite { dst, .. }
        | RuntimeInstr::FileRead { dst, .. }
        | RuntimeInstr::ThreadSpawn {
            handle_dst: dst, ..
        }
        | RuntimeInstr::ThreadJoin { dst, .. } => *dst == slot,
        RuntimeInstr::ChannelCreate { dst, .. } | RuntimeInstr::ChannelRecv { dst, .. } => {
            *dst == slot
        }
        RuntimeInstr::Free { .. }
        | RuntimeInstr::FileClose { .. }
        | RuntimeInstr::PrintConst { .. }
        | RuntimeInstr::PrintInt { .. } => false,
        RuntimeInstr::ChannelSend { .. }
        | RuntimeInstr::ChannelClose { .. }
        | RuntimeInstr::ChannelDestroy { .. } => false,
        RuntimeInstr::HeapStoreInt { .. } | RuntimeInstr::HeapCopy { .. } => false,
    }
}

fn runtime_operand_reads_slot(operand: &RuntimeOperand, slot: usize) -> bool {
    matches!(operand, RuntimeOperand::Slot(s) if *s == slot)
}

fn runtime_const_index_for_access(base_slots: &[usize], index: &RuntimeOperand) -> Option<usize> {
    let RuntimeOperand::Imm(value) = index else {
        return None;
    };
    let idx = usize::try_from(*value).ok()?;
    (idx < base_slots.len()).then_some(idx)
}

fn bump_slot_use_weight(counts: &mut [u32], slot: usize, weight: u32) {
    if let Some(count) = counts.get_mut(slot) {
        *count = count.saturating_add(weight);
    }
}

fn bump_operand_use_weight(counts: &mut [u32], operand: &RuntimeOperand, weight: u32) {
    if let RuntimeOperand::Slot(slot) = operand {
        bump_slot_use_weight(counts, *slot, weight);
    }
}

fn bump_instr_uses(counts: &mut [u32], instr: &RuntimeInstr, weight: u32) {
    match instr {
        RuntimeInstr::LoadSeed { dst, input, .. } => {
            bump_slot_use_weight(counts, *dst, weight);
            if let Some(input) = input {
                bump_operand_use_weight(counts, input, weight);
            }
        }
        RuntimeInstr::Mov { dst, src } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, src, weight);
        }
        RuntimeInstr::BinOp { dst, lhs, rhs, .. } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, lhs, weight);
            bump_operand_use_weight(counts, rhs, weight);
        }
        RuntimeInstr::BinOpInPlace { dst, rhs, .. } => {
            // In-place updates are both read and write hot.
            bump_slot_use_weight(counts, *dst, weight);
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, rhs, weight);
        }
        RuntimeInstr::FloatBinOp { dst, lhs, rhs, .. } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, lhs, weight);
            bump_operand_use_weight(counts, rhs, weight);
        }
        RuntimeInstr::Cmp { dst, lhs, rhs, .. } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, lhs, weight);
            bump_operand_use_weight(counts, rhs, weight);
        }
        RuntimeInstr::NormalizeInt { dst, .. } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_slot_use_weight(counts, *dst, weight);
        }
        RuntimeInstr::Jump { .. } => {}
        RuntimeInstr::JumpIfZero { cond_slot, .. } => {
            bump_slot_use_weight(counts, *cond_slot, weight);
        }
        RuntimeInstr::JumpIfCmpFalse { lhs, rhs, .. } => {
            bump_operand_use_weight(counts, lhs, weight);
            bump_operand_use_weight(counts, rhs, weight);
        }
        RuntimeInstr::CompareSwap { left, right, .. } => {
            bump_slot_use_weight(counts, *left, weight);
            bump_slot_use_weight(counts, *left, weight);
            bump_slot_use_weight(counts, *right, weight);
            bump_slot_use_weight(counts, *right, weight);
        }
        RuntimeInstr::RadixSortFixedInt { slots, bits, .. } => {
            let slot_weight = if slots.len() == 64 {
                // Dedicated 64-lane network path performs many compare-swaps.
                64
            } else {
                // Radix kernels touch each element repeatedly across passes.
                (((*bits as u32) + 7) / 8).saturating_mul(4).max(8)
            };
            for slot in slots {
                bump_slot_use_weight(counts, *slot, weight.saturating_mul(slot_weight));
                bump_slot_use_weight(counts, *slot, weight.saturating_mul(slot_weight));
            }
        }
        RuntimeInstr::LoadIndex {
            dst,
            base_slots,
            index,
        } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, index, weight);
            for slot in base_slots {
                bump_slot_use_weight(counts, *slot, weight / 4);
            }
        }
        RuntimeInstr::LoadIndexUnchecked {
            dst,
            base_slots,
            index,
        } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, index, weight);
            for slot in base_slots {
                bump_slot_use_weight(counts, *slot, weight / 4);
            }
        }
        RuntimeInstr::StoreIndex {
            base_slots,
            index,
            src,
        } => {
            bump_operand_use_weight(counts, index, weight);
            bump_operand_use_weight(counts, src, weight);
            for slot in base_slots {
                bump_slot_use_weight(counts, *slot, weight / 4);
            }
        }
        RuntimeInstr::StoreIndexUnchecked {
            base_slots,
            index,
            src,
        } => {
            bump_operand_use_weight(counts, index, weight);
            bump_operand_use_weight(counts, src, weight);
            for slot in base_slots {
                bump_slot_use_weight(counts, *slot, weight / 4);
            }
        }
        RuntimeInstr::HeapLoadInt {
            dst, ptr, index, ..
        } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, ptr, weight);
            bump_operand_use_weight(counts, index, weight);
        }
        RuntimeInstr::HeapStoreInt {
            ptr, index, src, ..
        } => {
            bump_operand_use_weight(counts, ptr, weight);
            bump_operand_use_weight(counts, index, weight);
            bump_operand_use_weight(counts, src, weight);
        }
        RuntimeInstr::HeapCopy {
            dst_ptr,
            src_ptr,
            bytes,
        } => {
            bump_operand_use_weight(counts, dst_ptr, weight);
            bump_operand_use_weight(counts, src_ptr, weight);
            bump_operand_use_weight(counts, bytes, weight);
        }
        RuntimeInstr::BloomSplitBlockInsert { filter_slots, hash } => {
            bump_operand_use_weight(counts, hash, weight);
            for slot in filter_slots {
                bump_slot_use_weight(counts, *slot, weight / 2);
            }
        }
        RuntimeInstr::BloomSplitBlockCheck {
            dst,
            filter_slots,
            hash,
        } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, hash, weight);
            for slot in filter_slots {
                bump_slot_use_weight(counts, *slot, weight / 2);
            }
        }
        RuntimeInstr::BloomClassic4Check {
            dst,
            lanes_checked,
            filter_slots,
            hash,
        } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_slot_use_weight(counts, *lanes_checked, weight);
            bump_operand_use_weight(counts, hash, weight);
            for slot in filter_slots {
                bump_slot_use_weight(counts, *slot, weight / 2);
            }
        }
        RuntimeInstr::HashCtrlGroupProbe {
            dst_mask,
            ctrl_slots,
            group_start,
            fingerprint,
        } => {
            bump_slot_use_weight(counts, *dst_mask, weight);
            bump_operand_use_weight(counts, group_start, weight);
            bump_operand_use_weight(counts, fingerprint, weight);
            for slot in ctrl_slots {
                bump_slot_use_weight(counts, *slot, weight / 2);
            }
        }
        RuntimeInstr::JoinSelectAdaptive {
            dst,
            build_rows,
            probe_rows,
        } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, build_rows, weight);
            bump_operand_use_weight(counts, probe_rows, weight);
        }
        RuntimeInstr::Alloc { dst, size } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, size, weight);
        }
        RuntimeInstr::Free { ptr, size } => {
            bump_operand_use_weight(counts, ptr, weight);
            bump_operand_use_weight(counts, size, weight);
        }
        RuntimeInstr::FileOpen { dst, path_ptr, .. } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, path_ptr, weight);
        }
        RuntimeInstr::FileWrite {
            dst, fd, ptr, len, ..
        }
        | RuntimeInstr::FileRead {
            dst, fd, ptr, len, ..
        } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, fd, weight);
            bump_operand_use_weight(counts, ptr, weight);
            bump_operand_use_weight(counts, len, weight);
        }
        RuntimeInstr::FileClose { fd } => bump_operand_use_weight(counts, fd, weight),
        RuntimeInstr::ThreadSpawn {
            handle_dst,
            return_slot,
            ..
        } => {
            bump_slot_use_weight(counts, *handle_dst, weight);
            if let Some(slot) = return_slot {
                bump_slot_use_weight(counts, *slot, weight);
            }
        }
        RuntimeInstr::ThreadJoin { dst, handle } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, handle, weight);
        }
        RuntimeInstr::ChannelCreate { dst, capacity, .. } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, capacity, weight);
        }
        RuntimeInstr::ChannelSend { handle, value } => {
            bump_operand_use_weight(counts, handle, weight);
            bump_operand_use_weight(counts, value, weight);
        }
        RuntimeInstr::ChannelRecv { dst, handle } => {
            bump_slot_use_weight(counts, *dst, weight);
            bump_operand_use_weight(counts, handle, weight);
        }
        RuntimeInstr::ChannelClose { handle, .. } | RuntimeInstr::ChannelDestroy { handle } => {
            bump_operand_use_weight(counts, handle, weight)
        }
        RuntimeInstr::PrintConst { .. } => {}
        RuntimeInstr::PrintInt { value, .. } => bump_operand_use_weight(counts, value, weight),
        RuntimeInstr::Call { .. } | RuntimeInstr::Return => {}
        RuntimeInstr::Exit { code } => bump_operand_use_weight(counts, code, weight),
    }
}

fn false_jcc_opcode(op: RuntimeCmpOp) -> u8 {
    match op {
        RuntimeCmpOp::Eq => 0x85,         // jne
        RuntimeCmpOp::Ne => 0x84,         // je
        RuntimeCmpOp::LtUnsigned => 0x83, // jae
        RuntimeCmpOp::LeUnsigned => 0x87, // ja
        RuntimeCmpOp::GtUnsigned => 0x86, // jbe
        RuntimeCmpOp::GeUnsigned => 0x82, // jb
        RuntimeCmpOp::LtSigned => 0x8D,   // jge
        RuntimeCmpOp::LeSigned => 0x8F,   // jg
        RuntimeCmpOp::GtSigned => 0x8E,   // jle
        RuntimeCmpOp::GeSigned => 0x8C,   // jl
    }
}

fn flip_cmp_operands(op: RuntimeCmpOp) -> RuntimeCmpOp {
    match op {
        RuntimeCmpOp::Eq => RuntimeCmpOp::Eq,
        RuntimeCmpOp::Ne => RuntimeCmpOp::Ne,
        RuntimeCmpOp::LtUnsigned => RuntimeCmpOp::GtUnsigned,
        RuntimeCmpOp::LeUnsigned => RuntimeCmpOp::GeUnsigned,
        RuntimeCmpOp::GtUnsigned => RuntimeCmpOp::LtUnsigned,
        RuntimeCmpOp::GeUnsigned => RuntimeCmpOp::LeUnsigned,
        RuntimeCmpOp::LtSigned => RuntimeCmpOp::GtSigned,
        RuntimeCmpOp::LeSigned => RuntimeCmpOp::GeSigned,
        RuntimeCmpOp::GtSigned => RuntimeCmpOp::LtSigned,
        RuntimeCmpOp::GeSigned => RuntimeCmpOp::LeSigned,
    }
}

fn gp_reg_low3(reg: GpReg) -> u8 {
    match reg {
        GpReg::R8 => 0,
        GpReg::R9 => 1,
        GpReg::R10 => 2,
        GpReg::R11 => 3,
        GpReg::R12 => 4,
        GpReg::R13 => 5,
        GpReg::R14 => 6,
        GpReg::R15 => 7,
    }
}

fn imm32_non_negative(value: u64) -> Option<i32> {
    if value <= i32::MAX as u64 {
        Some(value as i32)
    } else {
        None
    }
}

fn imm32_sign_extended(value: u64) -> Option<i32> {
    let imm32 = value as i32;
    let sign_extended = (imm32 as i64) as u64;
    if sign_extended == value {
        Some(imm32)
    } else {
        None
    }
}

fn pow2_shift_u32(value: u32) -> Option<u8> {
    if value.is_power_of_two() {
        Some(value.trailing_zeros() as u8)
    } else {
        None
    }
}

fn pow2_shift_u64(value: u64) -> Option<u8> {
    if value.is_power_of_two() {
        Some(value.trailing_zeros() as u8)
    } else {
        None
    }
}

/// Emits Linux x86_64 machine code.
pub fn emit_linux_program(kind: ProgramKind<'_>) -> Vec<u8> {
    match kind {
        ProgramKind::ExitOnly => {
            let mut program = X86Program::new();
            program.emit_exit(0);
            program.finalize()
        }
        ProgramKind::WriteAndExit { message } => {
            let mut program = X86Program::new();
            program.emit_write(message);
            program.emit_exit(0);
            program.finalize()
        }
    }
}

#[cfg(test)]
#[path = "x86_64/tests.rs"]
mod tests;
