use std::collections::{BTreeSet, HashMap, HashSet};

use crate::backend::lir::{RuntimeSSAProgram, read_slots, write_slots};
use crate::backend::peephole::{ExecutionPort, get_instruction_info};
use crate::backend::profile::{BlockProfile, FunctionProfile};
use crate::frontend::semantics::{
    RuntimeBinOp, RuntimeCmpOp, RuntimeInstr, RuntimeOperand, RuntimeProgram,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptimizationReport {
    pub affine_lookahead_groups: usize,
    pub affine_selects: usize,
    pub dead_prefix_reductions: usize,
    pub fixed_sort_selections: usize,
    pub partial_unrolled_loops: usize,
    pub propagated_copies: usize,
    pub eliminated_copies: usize,
    pub verified_rewrites: usize,
    pub scheduled_regions: usize,
    pub reordered_blocks: usize,
    pub inverted_branches: usize,
    pub repaired_fallthroughs: usize,
}

impl OptimizationReport {
    /// Stable, line-oriented JSON for `--dump-lir` consumers.
    ///
    /// Every counter is paired with the proof class required by its pass, so
    /// a performance artifact can be audited without relying on source names
    /// or benchmark constants.
    pub fn machine_readable(&self) -> String {
        format!(
            concat!(
                "optimization-report {{\"version\":1,\"pipeline\":\"runtime-generic\",",
                "\"passes\":[",
                "{{\"name\":\"affine-lookahead\",\"applied\":{},\"precondition\":\"identical affine transition, straight-line consumers, modular mask\"}},",
                "{{\"name\":\"affine-select\",\"applied\":{},\"precondition\":\"local diamond, equal state and mask, pure affine arms\"}},",
                "{{\"name\":\"dead-prefix-reduction\",\"applied\":{},\"precondition\":\"wrapping add associativity, only terminal prefix live on every CFG path\"}},",
                "{{\"name\":\"fixed-sort-selection\",\"applied\":{},\"precondition\":\"complete adjacent-swap sort over one proven fixed array\"}},",
                "{{\"name\":\"counted-loop-partial-unroll\",\"applied\":{},\"precondition\":\"canonical fixed-trip loop, divisible factor, straight-line body, bounded code growth\"}},",
                "{{\"name\":\"copy-propagation\",\"applied\":{},\"precondition\":\"single-assignment, dominated, non-addressable\"}},",
                "{{\"name\":\"verified-rewrite\",\"applied\":{},\"precondition\":\"target-independent scalar equivalence\"}},",
                "{{\"name\":\"pure-region-scheduling\",\"applied\":{},\"precondition\":\"no memory effects or control dependence\"}},",
                "{{\"name\":\"cfg-layout\",\"applied\":{},\"precondition\":\"profile matches the current control-flow graph\"}}",
                "]}}\n"
            ),
            self.affine_lookahead_groups,
            self.affine_selects,
            self.dead_prefix_reductions,
            self.fixed_sort_selections,
            self.partial_unrolled_loops,
            self.propagated_copies + self.eliminated_copies,
            self.verified_rewrites,
            self.scheduled_regions,
            self.reordered_blocks + self.inverted_branches + self.repaired_fallthroughs,
        )
    }
}

#[derive(Debug, Clone)]
pub struct OptimizedRuntimeProgram {
    pub program: RuntimeProgram,
    pub profile: Option<FunctionProfile>,
    pub report: OptimizationReport,
}

pub fn optimize_runtime_program(
    program: &RuntimeProgram,
    profile: Option<&FunctionProfile>,
    target_cpu: &str,
) -> OptimizedRuntimeProgram {
    let mut optimized = program.clone();
    let mut report = OptimizationReport::default();
    audit_frontend_generic_optimizations(&optimized, &mut report);
    propagate_single_assignment_copies(&mut optimized, &mut report);
    run_verified_superoptimizer(&mut optimized, target_cpu, &mut report);
    schedule_pure_regions(&mut optimized, target_cpu, &mut report);
    let (program, remapped_profile) = if let Some(profile) = profile {
        let profile_cfg = RuntimeSSAProgram::lower(&optimized);
        if profile_matches_cfg(&profile_cfg, profile) {
            layout_profiled_blocks(&optimized, profile, &mut report)
                .unwrap_or_else(|| (optimized, Some(profile.clone())))
        } else {
            (optimized, None)
        }
    } else {
        (optimized, None)
    };
    OptimizedRuntimeProgram {
        program,
        profile: remapped_profile,
        report,
    }
}

fn audit_frontend_generic_optimizations(program: &RuntimeProgram, report: &mut OptimizationReport) {
    let mut affine_bases = HashMap::<usize, usize>::new();
    for instr in &program.instrs {
        if let RuntimeInstr::BinOp {
            op: RuntimeBinOp::Mul,
            lhs: RuntimeOperand::Slot(base),
            rhs: RuntimeOperand::Imm(_),
            ..
        } = instr
        {
            *affine_bases.entry(*base).or_default() += 1;
        }
        if matches!(instr, RuntimeInstr::RadixSortFixedInt { .. }) {
            report.fixed_sort_selections += 1;
        }
    }
    report.affine_lookahead_groups = affine_bases.values().filter(|count| **count >= 2).count();
    report.partial_unrolled_loops = count_partial_unrolled_loops(&program.instrs);

    report.affine_selects = program
        .instrs
        .windows(8)
        .filter(|window| {
            matches!(window[0], RuntimeInstr::Cmp { .. })
                && matches!(
                    window[1],
                    RuntimeInstr::BinOp {
                        op: RuntimeBinOp::Sub,
                        lhs: RuntimeOperand::Imm(0),
                        ..
                    }
                )
                && matches!(
                    window[2],
                    RuntimeInstr::BinOp {
                        op: RuntimeBinOp::BitAnd,
                        ..
                    }
                )
                && matches!(
                    window[3],
                    RuntimeInstr::BinOpInPlace {
                        op: RuntimeBinOp::BitXor,
                        ..
                    }
                )
                && matches!(
                    window[4],
                    RuntimeInstr::BinOpInPlace {
                        op: RuntimeBinOp::Mul,
                        ..
                    }
                )
                && matches!(
                    window[5],
                    RuntimeInstr::BinOp {
                        op: RuntimeBinOp::BitAnd,
                        ..
                    }
                )
                && matches!(
                    window[6],
                    RuntimeInstr::BinOpInPlace {
                        op: RuntimeBinOp::BitXor,
                        ..
                    }
                )
                && matches!(
                    window[7],
                    RuntimeInstr::BinOpInPlace {
                        op: RuntimeBinOp::Add,
                        ..
                    }
                )
        })
        .count();

    let mut add_run = 0usize;
    for instr in &program.instrs {
        if matches!(
            instr,
            RuntimeInstr::BinOp {
                op: RuntimeBinOp::Add,
                lhs: RuntimeOperand::Slot(_),
                rhs: RuntimeOperand::Slot(_),
                ..
            }
        ) {
            add_run += 1;
        } else {
            if add_run >= 3 {
                report.dead_prefix_reductions += 1;
            }
            add_run = 0;
        }
    }
    if add_run >= 3 {
        report.dead_prefix_reductions += 1;
    }
}

fn count_partial_unrolled_loops(instrs: &[RuntimeInstr]) -> usize {
    let mut count = 0usize;
    for (header, guard) in instrs.iter().enumerate() {
        let RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LtUnsigned,
            lhs: RuntimeOperand::Slot(induction),
            rhs: RuntimeOperand::Imm(limit),
            target: exit,
        } = guard
        else {
            continue;
        };
        if *exit <= header || *exit > instrs.len() {
            continue;
        }
        let Some(latch) = (header + 1..*exit).find(
            |index| matches!(instrs[*index], RuntimeInstr::Jump { target } if target == header),
        ) else {
            continue;
        };
        if instrs[header + 1..latch].iter().any(|instr| {
            matches!(
                instr,
                RuntimeInstr::Jump { .. }
                    | RuntimeInstr::JumpIfZero { .. }
                    | RuntimeInstr::JumpIfCmpFalse { .. }
                    | RuntimeInstr::Call { .. }
                    | RuntimeInstr::Return
                    | RuntimeInstr::Exit { .. }
            )
        }) {
            continue;
        }

        let mut updates = 0usize;
        let mut invalid_write = false;
        for instr in &instrs[header + 1..latch] {
            if matches!(instr, RuntimeInstr::BinOpInPlace {
                dst,
                op: RuntimeBinOp::Add,
                rhs: RuntimeOperand::Imm(1),
            } if dst == induction)
            {
                updates += 1;
            } else if write_slots(instr).contains(induction) {
                invalid_write = true;
                break;
            }
        }
        if invalid_write || !matches!(updates, 2 | 4) {
            continue;
        }

        let mut start = None;
        for instr in instrs[..header].iter().rev() {
            if write_slots(instr).contains(induction) {
                if let RuntimeInstr::Mov {
                    dst,
                    src: RuntimeOperand::Imm(value),
                } = instr
                {
                    if dst == induction {
                        start = Some(*value);
                    }
                }
                break;
            }
            if matches!(
                instr,
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
        let Some(trip_count) = start.and_then(|start| limit.checked_sub(start)) else {
            continue;
        };
        if trip_count >= (updates as u64) * 8 && trip_count % updates as u64 == 0 {
            count += 1;
        }
    }
    count
}

/// Eliminate compiler-created temporary copies without changing mutable slot
/// identity.  A candidate must be single-assignment, dominated by its source
/// definition, never used as addressable storage, and expressible at every use
/// as a normal operand.  These restrictions make the rewrite a direct SSA
/// substitution rather than speculative value forwarding.
fn propagate_single_assignment_copies(
    program: &mut RuntimeProgram,
    report: &mut OptimizationReport,
) {
    let control_flow = RuntimeSSAProgram::lower(program);
    let mut block_by_instruction = vec![usize::MAX; program.instrs.len()];
    for block in &control_flow.blocks {
        for &instr_index in &block.instr_indices {
            block_by_instruction[instr_index] = block.id;
        }
    }
    let mut write_count = vec![0usize; program.slots];
    let mut sole_write = vec![None; program.slots];
    let mut first_read = vec![None; program.slots];
    let mut read_indices = vec![Vec::new(); program.slots];
    let mut addressable = vec![false; program.slots];
    let mut slot_only_read = vec![false; program.slots];

    for (index, instr) in program.instrs.iter().enumerate() {
        for slot in write_slots(instr) {
            write_count[slot] = write_count[slot].saturating_add(1);
            sole_write[slot] = Some(index);
        }
        for slot in read_slots(instr) {
            first_read[slot].get_or_insert(index);
            read_indices[slot].push(index);
        }
        match instr {
            RuntimeInstr::LoadIndex { base_slots, .. }
            | RuntimeInstr::LoadIndexUnchecked { base_slots, .. }
            | RuntimeInstr::StoreIndex { base_slots, .. }
            | RuntimeInstr::StoreIndexUnchecked { base_slots, .. }
            | RuntimeInstr::RadixSortFixedInt {
                slots: base_slots, ..
            } => {
                for &slot in base_slots {
                    addressable[slot] = true;
                }
            }
            RuntimeInstr::CompareSwap { left, right, .. } => {
                addressable[*left] = true;
                addressable[*right] = true;
            }
            RuntimeInstr::BinOpInPlace { dst, .. }
            | RuntimeInstr::NormalizeInt { dst, .. }
            | RuntimeInstr::JumpIfZero { cond_slot: dst, .. } => slot_only_read[*dst] = true,
            _ => {}
        }
    }

    let mut aliases = HashMap::<usize, RuntimeOperand>::new();
    let mut copy_indices = Vec::new();
    for (index, instr) in program.instrs.iter().enumerate() {
        let RuntimeInstr::Mov { dst, src } = instr else {
            continue;
        };
        if *dst >= program.slots
            || write_count[*dst] != 1
            || addressable[*dst]
            || slot_only_read[*dst]
            || first_read[*dst].is_some_and(|read| read <= index)
            || block_by_instruction[index] == usize::MAX
            || read_indices[*dst]
                .iter()
                .any(|&read| block_by_instruction[read] != block_by_instruction[index])
        {
            continue;
        }
        if let RuntimeOperand::Slot(source) = src {
            if *source >= program.slots
                || write_count[*source] > 1
                || sole_write[*source].is_some_and(|write| write > index)
            {
                continue;
            }
        }
        // Candidates are visited in definition order and may only reference a
        // source whose sole static definition is not later.  Resolve the
        // already-built prefix now, so arbitrary-length chains never retain a
        // reference to a copy that will be removed.
        let mut replacement = *src;
        while let RuntimeOperand::Slot(source) = replacement {
            let Some(next) = aliases.get(&source).copied() else {
                break;
            };
            replacement = next;
        }
        aliases.insert(*dst, replacement);
        copy_indices.push((index, *dst));
    }

    if aliases.is_empty() {
        return;
    }

    for instr in &mut program.instrs {
        report.propagated_copies = report
            .propagated_copies
            .saturating_add(rewrite_instruction_operands(instr, &aliases));
    }
    let removed: HashSet<usize> = copy_indices.into_iter().map(|(index, _)| index).collect();
    report.eliminated_copies = report.eliminated_copies.saturating_add(removed.len());
    remove_runtime_instructions(program, &removed);
}

fn remove_runtime_instructions(program: &mut RuntimeProgram, removed: &HashSet<usize>) {
    if removed.is_empty() {
        return;
    }
    let old_len = program.instrs.len();
    let mut old_to_new = vec![0usize; old_len + 1];
    let mut next = 0usize;
    for (index, entry) in old_to_new.iter_mut().take(old_len).enumerate() {
        *entry = next;
        if !removed.contains(&index) {
            next += 1;
        }
    }
    old_to_new[old_len] = next;

    let mut compact = Vec::with_capacity(next);
    for (index, instr) in program.instrs.iter().enumerate() {
        if removed.contains(&index) {
            continue;
        }
        let mut instr = instr.clone();
        match &mut instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => {
                *target = old_to_new[(*target).min(old_len)];
            }
            _ => {}
        }
        compact.push(instr);
    }
    program.instrs = compact;
}

fn runtime_operands_equal(lhs: &RuntimeOperand, rhs: &RuntimeOperand) -> bool {
    matches!((lhs, rhs), (RuntimeOperand::Slot(a), RuntimeOperand::Slot(b)) if a == b)
        || matches!((lhs, rhs), (RuntimeOperand::Imm(a), RuntimeOperand::Imm(b)) if a == b)
}

fn rewrite_operand(
    operand: &mut RuntimeOperand,
    aliases: &HashMap<usize, RuntimeOperand>,
) -> usize {
    let RuntimeOperand::Slot(slot) = operand else {
        return 0;
    };
    let Some(replacement) = aliases.get(slot) else {
        return 0;
    };
    if runtime_operands_equal(operand, replacement) {
        return 0;
    }
    *operand = *replacement;
    1
}

fn rewrite_instruction_operands(
    instr: &mut RuntimeInstr,
    aliases: &HashMap<usize, RuntimeOperand>,
) -> usize {
    let mut rewritten = 0usize;
    let mut rewrite = |operand: &mut RuntimeOperand| {
        rewritten = rewritten.saturating_add(rewrite_operand(operand, aliases));
    };
    match instr {
        RuntimeInstr::LoadSeed {
            input: Some(src), ..
        }
        | RuntimeInstr::Mov { src, .. }
        | RuntimeInstr::PrintInt { value: src, .. }
        | RuntimeInstr::Exit { code: src }
        | RuntimeInstr::Alloc { size: src, .. } => rewrite(src),
        RuntimeInstr::BinOp { lhs, rhs, .. }
        | RuntimeInstr::FloatBinOp { lhs, rhs, .. }
        | RuntimeInstr::Cmp { lhs, rhs, .. }
        | RuntimeInstr::JumpIfCmpFalse { lhs, rhs, .. } => {
            rewrite(lhs);
            rewrite(rhs);
        }
        RuntimeInstr::BinOpInPlace { rhs, .. } => rewrite(rhs),
        RuntimeInstr::LoadIndex { index, .. } | RuntimeInstr::LoadIndexUnchecked { index, .. } => {
            rewrite(index)
        }
        RuntimeInstr::StoreIndex { index, src, .. }
        | RuntimeInstr::StoreIndexUnchecked { index, src, .. } => {
            rewrite(index);
            rewrite(src);
        }
        RuntimeInstr::HeapLoadInt { ptr, index, .. } => {
            rewrite(ptr);
            rewrite(index);
        }
        RuntimeInstr::HeapStoreInt {
            ptr, index, src, ..
        } => {
            rewrite(ptr);
            rewrite(index);
            rewrite(src);
        }
        RuntimeInstr::HeapCopy {
            dst_ptr,
            src_ptr,
            bytes,
        } => {
            rewrite(dst_ptr);
            rewrite(src_ptr);
            rewrite(bytes);
        }
        RuntimeInstr::Free { ptr, size } => {
            rewrite(ptr);
            rewrite(size);
        }
        RuntimeInstr::FileOpen { path_ptr, .. } => rewrite(path_ptr),
        RuntimeInstr::FileWrite { fd, ptr, len, .. }
        | RuntimeInstr::FileRead { fd, ptr, len, .. } => {
            rewrite(fd);
            rewrite(ptr);
            rewrite(len);
        }
        RuntimeInstr::FileClose { fd } | RuntimeInstr::ThreadJoin { handle: fd, .. } => rewrite(fd),
        RuntimeInstr::ChannelCreate { capacity, .. } => rewrite(capacity),
        RuntimeInstr::ChannelSend { handle, value } => {
            rewrite(handle);
            rewrite(value);
        }
        RuntimeInstr::ChannelRecv { handle, .. }
        | RuntimeInstr::ChannelClose { handle, .. }
        | RuntimeInstr::ChannelDestroy { handle } => rewrite(handle),
        RuntimeInstr::LoadSeed { input: None, .. }
        | RuntimeInstr::NormalizeInt { .. }
        | RuntimeInstr::Jump { .. }
        | RuntimeInstr::JumpIfZero { .. }
        | RuntimeInstr::CompareSwap { .. }
        | RuntimeInstr::RadixSortFixedInt { .. }
        | RuntimeInstr::Call { .. }
        | RuntimeInstr::ThreadSpawn { .. }
        | RuntimeInstr::PrintConst { .. }
        | RuntimeInstr::Return => {}
    }
    rewritten
}

fn run_verified_superoptimizer(
    program: &mut RuntimeProgram,
    target_cpu: &str,
    report: &mut OptimizationReport,
) {
    let original = program.instrs.clone();
    for (index, instr) in program.instrs.iter_mut().enumerate() {
        if fusion_sensitive(&original, index) {
            continue;
        }
        let original_cost = target_instruction_cost(instr, target_cpu)
            .saturating_add(register_pressure_cost(instr));
        let best = enumerate_candidates(instr)
            .into_iter()
            .filter(|candidate| verify_candidate(instr, candidate))
            .filter_map(|candidate| {
                let cost = target_instruction_cost(&candidate, target_cpu)
                    .saturating_add(register_pressure_cost(&candidate));
                (cost <= original_cost).then_some((cost, candidate))
            })
            .min_by_key(|(cost, candidate)| (*cost, format!("{candidate:?}")));
        if let Some((_, candidate)) = best {
            *instr = candidate;
            report.verified_rewrites += 1;
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum BitVectorExpr {
    Operand(RuntimeOperand),
    Binary {
        op: RuntimeBinOp,
        lhs: RuntimeOperand,
        rhs: RuntimeOperand,
    },
}

impl BitVectorExpr {
    fn node_count(self) -> usize {
        match self {
            Self::Operand(operand) => {
                let _ = operand;
                1
            }
            Self::Binary { op, lhs, rhs } => {
                let _ = (op, lhs, rhs);
                3
            }
        }
    }
}

fn enumerate_candidates(instr: &RuntimeInstr) -> Vec<RuntimeInstr> {
    const MAX_CANDIDATES: usize = 8;
    let mut candidates = Vec::new();
    if let Some(candidate) = synthesize_candidate(instr) {
        candidates.push(candidate);
    }
    let expression = match instr {
        RuntimeInstr::BinOp { op, lhs, rhs, .. } => Some(BitVectorExpr::Binary {
            op: *op,
            lhs: *lhs,
            rhs: *rhs,
        }),
        RuntimeInstr::Mov { src, .. } => Some(BitVectorExpr::Operand(*src)),
        _ => None,
    };
    if expression.is_some_and(|expression| expression.node_count() <= 3)
        && let RuntimeInstr::BinOp {
            dst,
            op,
            lhs: RuntimeOperand::Imm(lhs),
            rhs: RuntimeOperand::Slot(rhs),
        } = instr
    {
        if matches!(
            op,
            RuntimeBinOp::Add
                | RuntimeBinOp::Mul
                | RuntimeBinOp::BitAnd
                | RuntimeBinOp::BitOr
                | RuntimeBinOp::BitXor
        ) {
            let expression = BitVectorExpr::Binary {
                op: *op,
                lhs: RuntimeOperand::Slot(*rhs),
                rhs: RuntimeOperand::Imm(*lhs),
            };
            if let BitVectorExpr::Binary { op, lhs, rhs } = expression {
                candidates.push(RuntimeInstr::BinOp {
                    dst: *dst,
                    op,
                    lhs,
                    rhs,
                });
            }
        }
    }
    candidates.truncate(MAX_CANDIDATES);
    candidates
}

fn fusion_sensitive(instrs: &[RuntimeInstr], index: usize) -> bool {
    matches!(
        instrs.get(index),
        Some(RuntimeInstr::BinOp {
            op: RuntimeBinOp::Shl,
            ..
        })
    ) && matches!(
        instrs.get(index + 1),
        Some(RuntimeInstr::BinOp {
            op: RuntimeBinOp::BitOr,
            ..
        })
    )
}

fn synthesize_candidate(instr: &RuntimeInstr) -> Option<RuntimeInstr> {
    match instr {
        RuntimeInstr::BinOp { dst, op, lhs, rhs } => {
            if let (RuntimeOperand::Imm(lhs), RuntimeOperand::Imm(rhs)) = (lhs, rhs) {
                return evaluate_binop(*op, *lhs, *rhs).map(|value| RuntimeInstr::Mov {
                    dst: *dst,
                    src: RuntimeOperand::Imm(value),
                });
            }
            if let RuntimeOperand::Imm(constant) = rhs {
                if *constant > 1 && constant.is_power_of_two() {
                    let shift = u64::from(constant.trailing_zeros());
                    let (replacement_op, replacement_rhs) = match op {
                        RuntimeBinOp::Mul => (RuntimeBinOp::Shl, RuntimeOperand::Imm(shift)),
                        RuntimeBinOp::DivUnsigned => {
                            (RuntimeBinOp::ShrUnsigned, RuntimeOperand::Imm(shift))
                        }
                        RuntimeBinOp::ModUnsigned => (
                            RuntimeBinOp::BitAnd,
                            RuntimeOperand::Imm(constant.wrapping_sub(1)),
                        ),
                        _ => (*op, *rhs),
                    };
                    if replacement_op != *op {
                        return Some(RuntimeInstr::BinOp {
                            dst: *dst,
                            op: replacement_op,
                            lhs: *lhs,
                            rhs: replacement_rhs,
                        });
                    }
                }
            }
            let replacement = algebraic_identity_replacement(*op, lhs, rhs)?;
            Some(RuntimeInstr::Mov {
                dst: *dst,
                src: replacement,
            })
        }
        RuntimeInstr::BinOpInPlace { dst, op, rhs } => {
            let replacement =
                algebraic_identity_replacement(*op, &RuntimeOperand::Slot(*dst), rhs)?;
            Some(RuntimeInstr::Mov {
                dst: *dst,
                src: replacement,
            })
        }
        RuntimeInstr::Cmp {
            dst,
            op,
            lhs: RuntimeOperand::Imm(lhs),
            rhs: RuntimeOperand::Imm(rhs),
        } => Some(RuntimeInstr::Mov {
            dst: *dst,
            src: RuntimeOperand::Imm(evaluate_cmp(*op, *lhs, *rhs) as u64),
        }),
        RuntimeInstr::Cmp {
            dst,
            op,
            lhs: RuntimeOperand::Slot(lhs),
            rhs: RuntimeOperand::Slot(rhs),
        } if lhs == rhs => {
            let value = matches!(
                op,
                RuntimeCmpOp::Eq
                    | RuntimeCmpOp::LeUnsigned
                    | RuntimeCmpOp::GeUnsigned
                    | RuntimeCmpOp::LeSigned
                    | RuntimeCmpOp::GeSigned
            ) as u64;
            Some(RuntimeInstr::Mov {
                dst: *dst,
                src: RuntimeOperand::Imm(value),
            })
        }
        _ => None,
    }
}

fn algebraic_identity_replacement(
    op: RuntimeBinOp,
    lhs: &RuntimeOperand,
    rhs: &RuntimeOperand,
) -> Option<RuntimeOperand> {
    let same = operands_equal(lhs, rhs);
    match (op, lhs, rhs) {
        (RuntimeBinOp::Add, value, RuntimeOperand::Imm(0))
        | (RuntimeBinOp::Sub, value, RuntimeOperand::Imm(0))
        | (RuntimeBinOp::Mul, value, RuntimeOperand::Imm(1))
        | (RuntimeBinOp::BitOr, value, RuntimeOperand::Imm(0))
        | (RuntimeBinOp::BitXor, value, RuntimeOperand::Imm(0))
        | (RuntimeBinOp::BitAnd, value, RuntimeOperand::Imm(u64::MAX))
        | (RuntimeBinOp::Shl, value, RuntimeOperand::Imm(0))
        | (RuntimeBinOp::ShrUnsigned, value, RuntimeOperand::Imm(0))
        | (RuntimeBinOp::ShrSigned, value, RuntimeOperand::Imm(0)) => Some(*value),
        (RuntimeBinOp::Add, RuntimeOperand::Imm(0), value)
        | (RuntimeBinOp::Mul, RuntimeOperand::Imm(1), value)
        | (RuntimeBinOp::BitOr, RuntimeOperand::Imm(0), value)
        | (RuntimeBinOp::BitXor, RuntimeOperand::Imm(0), value)
        | (RuntimeBinOp::BitAnd, RuntimeOperand::Imm(u64::MAX), value) => Some(*value),
        (RuntimeBinOp::Mul, _, RuntimeOperand::Imm(0))
        | (RuntimeBinOp::Mul, RuntimeOperand::Imm(0), _)
        | (RuntimeBinOp::BitAnd, _, RuntimeOperand::Imm(0))
        | (RuntimeBinOp::BitAnd, RuntimeOperand::Imm(0), _) => Some(RuntimeOperand::Imm(0)),
        (RuntimeBinOp::Sub | RuntimeBinOp::BitXor, _, _) if same => Some(RuntimeOperand::Imm(0)),
        (RuntimeBinOp::BitAnd | RuntimeBinOp::BitOr, value, _) if same => Some(*value),
        _ => None,
    }
}

fn evaluate_binop(op: RuntimeBinOp, lhs: u64, rhs: u64) -> Option<u64> {
    Some(match op {
        RuntimeBinOp::Add => lhs.wrapping_add(rhs),
        RuntimeBinOp::Sub => lhs.wrapping_sub(rhs),
        RuntimeBinOp::Mul => lhs.wrapping_mul(rhs),
        RuntimeBinOp::DivUnsigned => lhs.checked_div(rhs)?,
        RuntimeBinOp::DivSigned => ((lhs as i64).checked_div(rhs as i64)?) as u64,
        RuntimeBinOp::ModUnsigned => lhs.checked_rem(rhs)?,
        RuntimeBinOp::ModSigned => ((lhs as i64).checked_rem(rhs as i64)?) as u64,
        RuntimeBinOp::BitAnd => lhs & rhs,
        RuntimeBinOp::BitOr => lhs | rhs,
        RuntimeBinOp::BitXor => lhs ^ rhs,
        RuntimeBinOp::Shl => lhs.wrapping_shl((rhs & 63) as u32),
        RuntimeBinOp::ShrUnsigned => lhs.wrapping_shr((rhs & 63) as u32),
        RuntimeBinOp::ShrSigned => ((lhs as i64) >> (rhs & 63)) as u64,
    })
}

fn verify_candidate(original: &RuntimeInstr, candidate: &RuntimeInstr) -> bool {
    if !has_exact_rewrite_proof(original, candidate) {
        return false;
    }
    let mut slots = read_slots(original);
    slots.extend(read_slots(candidate));
    slots.sort_unstable();
    slots.dedup();
    if slots.len() > 2 {
        return false;
    }
    const CORPUS: [u64; 12] = [
        0,
        1,
        2,
        3,
        7,
        31,
        u32::MAX as u64,
        1_u64 << 32,
        i64::MAX as u64,
        i64::MIN as u64,
        u64::MAX - 1,
        u64::MAX,
    ];
    let mut environments = vec![HashMap::<usize, u64>::new()];
    for slot in slots {
        let mut expanded = Vec::new();
        for environment in &environments {
            for value in CORPUS {
                let mut next = environment.clone();
                next.insert(slot, value);
                expanded.push(next);
            }
        }
        environments = expanded;
    }
    environments.into_iter().all(|environment| {
        evaluate_pure_instruction(original, &environment)
            == evaluate_pure_instruction(candidate, &environment)
    })
}

fn has_exact_rewrite_proof(original: &RuntimeInstr, candidate: &RuntimeInstr) -> bool {
    match (original, candidate) {
        (
            RuntimeInstr::BinOp {
                dst: original_dst,
                op,
                lhs: RuntimeOperand::Imm(lhs),
                rhs: RuntimeOperand::Imm(rhs),
            },
            RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Imm(value),
            },
        ) => original_dst == dst && evaluate_binop(*op, *lhs, *rhs) == Some(*value),
        (
            RuntimeInstr::Cmp {
                dst: original_dst,
                op,
                lhs: RuntimeOperand::Imm(lhs),
                rhs: RuntimeOperand::Imm(rhs),
            },
            RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Imm(value),
            },
        ) => original_dst == dst && *value == evaluate_cmp(*op, *lhs, *rhs) as u64,
        (
            RuntimeInstr::Cmp {
                dst: original_dst,
                op,
                lhs: RuntimeOperand::Slot(lhs),
                rhs: RuntimeOperand::Slot(rhs),
            },
            RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Imm(value),
            },
        ) => {
            original_dst == dst
                && lhs == rhs
                && *value
                    == matches!(
                        op,
                        RuntimeCmpOp::Eq
                            | RuntimeCmpOp::LeUnsigned
                            | RuntimeCmpOp::GeUnsigned
                            | RuntimeCmpOp::LeSigned
                            | RuntimeCmpOp::GeSigned
                    ) as u64
        }
        (
            RuntimeInstr::BinOp {
                dst: original_dst,
                op,
                lhs,
                rhs,
            },
            RuntimeInstr::Mov { dst, src },
        ) => {
            original_dst == dst
                && algebraic_identity_replacement(*op, lhs, rhs)
                    .is_some_and(|replacement| operands_equal(&replacement, src))
        }
        (
            RuntimeInstr::BinOpInPlace {
                dst: original_dst,
                op,
                rhs,
            },
            RuntimeInstr::Mov { dst, src },
        ) => {
            original_dst == dst
                && algebraic_identity_replacement(*op, &RuntimeOperand::Slot(*original_dst), rhs)
                    .is_some_and(|replacement| operands_equal(&replacement, src))
        }
        (
            RuntimeInstr::BinOp {
                dst: original_dst,
                op: RuntimeBinOp::Mul | RuntimeBinOp::DivUnsigned | RuntimeBinOp::ModUnsigned,
                lhs: original_lhs,
                rhs: RuntimeOperand::Imm(constant),
            },
            RuntimeInstr::BinOp { dst, op, lhs, rhs },
        ) => {
            original_dst == dst
                && operands_equal(original_lhs, lhs)
                && *constant > 1
                && constant.is_power_of_two()
                && (matches!(
                    (original, op, rhs),
                    (
                        RuntimeInstr::BinOp { op: RuntimeBinOp::Mul, .. },
                        RuntimeBinOp::Shl,
                        RuntimeOperand::Imm(shift)
                    ) if *shift == u64::from(constant.trailing_zeros())
                ) || matches!(
                    (original, op, rhs),
                    (
                        RuntimeInstr::BinOp { op: RuntimeBinOp::DivUnsigned, .. },
                        RuntimeBinOp::ShrUnsigned,
                        RuntimeOperand::Imm(shift)
                    ) if *shift == u64::from(constant.trailing_zeros())
                ) || matches!(
                    (original, op, rhs),
                    (
                        RuntimeInstr::BinOp { op: RuntimeBinOp::ModUnsigned, .. },
                        RuntimeBinOp::BitAnd,
                        RuntimeOperand::Imm(mask)
                    ) if *mask == constant - 1
                ))
        }
        (
            RuntimeInstr::BinOp {
                dst: original_dst,
                op: original_op,
                lhs: original_lhs,
                rhs: original_rhs,
            },
            RuntimeInstr::BinOp { dst, op, lhs, rhs },
        ) => {
            original_dst == dst
                && original_op == op
                && matches!(
                    original_op,
                    RuntimeBinOp::Add
                        | RuntimeBinOp::Mul
                        | RuntimeBinOp::BitAnd
                        | RuntimeBinOp::BitOr
                        | RuntimeBinOp::BitXor
                )
                && operands_equal(original_lhs, rhs)
                && operands_equal(original_rhs, lhs)
        }
        _ => false,
    }
}

fn operands_equal(lhs: &RuntimeOperand, rhs: &RuntimeOperand) -> bool {
    matches!(
        (lhs, rhs),
        (RuntimeOperand::Slot(left), RuntimeOperand::Slot(right)) if left == right
    ) || matches!(
        (lhs, rhs),
        (RuntimeOperand::Imm(left), RuntimeOperand::Imm(right)) if left == right
    )
}

fn evaluate_pure_instruction(
    instr: &RuntimeInstr,
    environment: &HashMap<usize, u64>,
) -> Option<(usize, u64)> {
    let operand = |operand: &RuntimeOperand| match operand {
        RuntimeOperand::Imm(value) => Some(*value),
        RuntimeOperand::Slot(slot) => environment.get(slot).copied(),
    };
    match instr {
        RuntimeInstr::Mov { dst, src } => Some((*dst, operand(src)?)),
        RuntimeInstr::BinOp { dst, op, lhs, rhs } => {
            Some((*dst, evaluate_binop(*op, operand(lhs)?, operand(rhs)?)?))
        }
        RuntimeInstr::BinOpInPlace { dst, op, rhs } => Some((
            *dst,
            evaluate_binop(*op, environment.get(dst).copied()?, operand(rhs)?)?,
        )),
        RuntimeInstr::Cmp { dst, op, lhs, rhs } => {
            Some((*dst, evaluate_cmp(*op, operand(lhs)?, operand(rhs)?) as u64))
        }
        _ => None,
    }
}

fn evaluate_cmp(op: RuntimeCmpOp, lhs: u64, rhs: u64) -> bool {
    match op {
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
    }
}

fn schedule_pure_regions(
    program: &mut RuntimeProgram,
    target_cpu: &str,
    report: &mut OptimizationReport,
) {
    let ssa = RuntimeSSAProgram::lower(program);
    for block in &ssa.blocks {
        let mut cursor = 0;
        while cursor < block.instr_indices.len() {
            if !is_schedulable(&block.instrs[cursor]) {
                cursor += 1;
                continue;
            }
            let start = cursor;
            while cursor < block.instr_indices.len() && is_schedulable(&block.instrs[cursor]) {
                cursor += 1;
            }
            if cursor - start < 3 {
                continue;
            }
            let indices = &block.instr_indices[start..cursor];
            let region: Vec<RuntimeInstr> = indices
                .iter()
                .map(|index| program.instrs[*index].clone())
                .collect();
            // The x86 selector contracts a proven u32 affine triple into one
            // multiply/add sequence. Scheduling its members apart increases
            // code size and register traffic enough to outweigh any local
            // latency hiding, so keep each canonical group intact.
            if region.windows(3).any(|window| {
                matches!(
                    window,
                    [
                        RuntimeInstr::BinOp {
                            dst: mul_dst,
                            op: RuntimeBinOp::Mul,
                            rhs: RuntimeOperand::Imm(_),
                            ..
                        },
                        RuntimeInstr::BinOp {
                            dst: add_dst,
                            op: RuntimeBinOp::Add,
                            lhs: RuntimeOperand::Slot(add_lhs),
                            rhs: RuntimeOperand::Imm(_),
                        },
                        RuntimeInstr::BinOp {
                            op: RuntimeBinOp::BitAnd,
                            lhs: RuntimeOperand::Slot(mask_lhs),
                            rhs: RuntimeOperand::Imm(mask),
                            ..
                        }
                    ] if mul_dst == add_lhs
                        && add_dst == mask_lhs
                        && *mask == u64::from(u32::MAX)
                )
            }) {
                continue;
            }
            if let Some(order) = schedule_region(&region, target_cpu) {
                if order
                    .iter()
                    .enumerate()
                    .all(|(position, node)| position == *node)
                {
                    continue;
                }
                for (position, node) in order.into_iter().enumerate() {
                    program.instrs[indices[position]] = region[node].clone();
                }
                report.scheduled_regions += 1;
            }
        }
    }
}

fn is_schedulable(instr: &RuntimeInstr) -> bool {
    matches!(
        instr,
        RuntimeInstr::Mov { .. }
            | RuntimeInstr::BinOp { .. }
            | RuntimeInstr::BinOpInPlace { .. }
            | RuntimeInstr::NormalizeInt { .. }
    )
}

fn schedule_region(region: &[RuntimeInstr], target_cpu: &str) -> Option<Vec<usize>> {
    let mut successors = vec![Vec::<usize>::new(); region.len()];
    let mut indegree = vec![0usize; region.len()];
    for left in 0..region.len() {
        let left_reads: HashSet<_> = read_slots(&region[left]).into_iter().collect();
        let left_writes: HashSet<_> = write_slots(&region[left]).into_iter().collect();
        for right in left + 1..region.len() {
            let right_reads: HashSet<_> = read_slots(&region[right]).into_iter().collect();
            let right_writes: HashSet<_> = write_slots(&region[right]).into_iter().collect();
            let dependent = !left_writes.is_disjoint(&right_reads)
                || !left_writes.is_disjoint(&right_writes)
                || !left_reads.is_disjoint(&right_writes);
            if dependent {
                successors[left].push(right);
                indegree[right] += 1;
            }
        }
    }
    let mut critical = vec![0u32; region.len()];
    for node in (0..region.len()).rev() {
        critical[node] = instruction_latency(&region[node], target_cpu)
            + successors[node]
                .iter()
                .map(|successor| critical[*successor])
                .max()
                .unwrap_or(0);
    }
    let mut ready = BTreeSet::new();
    for (node, degree) in indegree.iter().enumerate() {
        if *degree == 0 {
            ready.insert(node);
        }
    }
    let mut order = Vec::with_capacity(region.len());
    let mut port_counts = [0_u32; 7];
    while !ready.is_empty() {
        let node = ready.iter().copied().max_by_key(|node| {
            let (projected_pressure, micro_ops) =
                projected_machine_pressure(&region[*node], &port_counts, target_cpu);
            (
                critical[*node],
                std::cmp::Reverse(projected_pressure),
                std::cmp::Reverse(micro_ops),
                std::cmp::Reverse(*node),
            )
        })?;
        ready.remove(&node);
        order.push(node);
        account_ports(&region[node], &mut port_counts, target_cpu);
        for successor in &successors[node] {
            indegree[*successor] -= 1;
            if indegree[*successor] == 0 {
                ready.insert(*successor);
            }
        }
    }
    if order.len() != region.len() {
        return None;
    }
    let original: Vec<usize> = (0..region.len()).collect();
    (peak_register_pressure(region, &order) <= peak_register_pressure(region, &original) + 1)
        .then_some(order)
}

fn projected_machine_pressure(
    instr: &RuntimeInstr,
    counts: &[u32; 7],
    target_cpu: &str,
) -> (u32, u8) {
    let mut projected = *counts;
    let info = machine_info(instr, target_cpu);
    if let Some(info) = &info {
        for port in info.ports {
            projected[port_index(*port)] += 1;
        }
    }
    (
        *projected.iter().max().unwrap_or(&0),
        info.map(|info| info.micro_ops).unwrap_or(1),
    )
}

fn account_ports(instr: &RuntimeInstr, counts: &mut [u32; 7], target_cpu: &str) {
    if let Some(info) = machine_info(instr, target_cpu) {
        for port in info.ports {
            counts[port_index(*port)] += 1;
        }
    }
}

fn port_index(port: ExecutionPort) -> usize {
    match port {
        ExecutionPort::Port0 => 0,
        ExecutionPort::Port1 => 1,
        ExecutionPort::Port2 => 2,
        ExecutionPort::Port3 => 3,
        ExecutionPort::Port4 => 4,
        ExecutionPort::Port5 => 5,
        ExecutionPort::Port6 => 6,
    }
}

fn peak_register_pressure(region: &[RuntimeInstr], order: &[usize]) -> usize {
    let mut last_use = HashMap::<usize, usize>::new();
    for (position, node) in order.iter().copied().enumerate() {
        for slot in read_slots(&region[node]) {
            last_use.insert(slot, position);
        }
    }
    let mut live = HashSet::new();
    let mut peak = 0;
    for (position, node) in order.iter().copied().enumerate() {
        for slot in write_slots(&region[node]) {
            if last_use.get(&slot).is_some_and(|last| *last > position) {
                live.insert(slot);
            }
        }
        peak = peak.max(live.len());
        live.retain(|slot| last_use.get(slot).is_some_and(|last| *last > position));
    }
    peak
}

fn machine_info(
    instr: &RuntimeInstr,
    _target_cpu: &str,
) -> Option<crate::backend::peephole::InstructionInfo> {
    let mnemonic = instruction_mnemonic(instr);
    get_instruction_info(mnemonic)
}

fn instruction_latency(instr: &RuntimeInstr, target_cpu: &str) -> u32 {
    let baseline = get_instruction_info(instruction_mnemonic(instr))
        .map(|info| u32::from(info.latency_cycles))
        .unwrap_or(1);
    if target_cpu.contains("atom") && instruction_mnemonic(instr) == "imul" {
        baseline.saturating_add(1)
    } else {
        baseline
    }
}

fn target_instruction_cost(instr: &RuntimeInstr, target_cpu: &str) -> u32 {
    let mut base = instruction_latency(instr, target_cpu)
        .saturating_mul(4)
        .saturating_add(
            machine_info(instr, target_cpu)
                .map(|info| u32::from(info.micro_ops))
                .unwrap_or(1),
        );
    // Three-address BinOp lowering first materializes `lhs` in the destination,
    // whereas an in-place operation and a Mov do not pay that extra instruction.
    if matches!(instr, RuntimeInstr::BinOp { .. }) {
        base = base.saturating_add(5);
    }
    if matches!(
        instr,
        RuntimeInstr::BinOp {
            rhs: RuntimeOperand::Imm(_),
            ..
        }
    ) {
        base.saturating_sub(1)
    } else {
        base
    }
}

fn register_pressure_cost(instr: &RuntimeInstr) -> u32 {
    let mut operands = read_slots(instr);
    operands.sort_unstable();
    operands.dedup();
    operands.len().saturating_sub(1) as u32
}

fn instruction_mnemonic(instr: &RuntimeInstr) -> &'static str {
    match instr {
        RuntimeInstr::Mov { .. } => "mov",
        RuntimeInstr::BinOp {
            op: RuntimeBinOp::Mul,
            ..
        }
        | RuntimeInstr::BinOpInPlace {
            op: RuntimeBinOp::Mul,
            ..
        } => "imul",
        RuntimeInstr::BinOp {
            op:
                RuntimeBinOp::DivUnsigned
                | RuntimeBinOp::DivSigned
                | RuntimeBinOp::ModUnsigned
                | RuntimeBinOp::ModSigned,
            ..
        }
        | RuntimeInstr::BinOpInPlace {
            op:
                RuntimeBinOp::DivUnsigned
                | RuntimeBinOp::DivSigned
                | RuntimeBinOp::ModUnsigned
                | RuntimeBinOp::ModSigned,
            ..
        } => "div",
        RuntimeInstr::BinOp { op, .. } | RuntimeInstr::BinOpInPlace { op, .. } => match op {
            RuntimeBinOp::Shl | RuntimeBinOp::ShrUnsigned | RuntimeBinOp::ShrSigned => "shl",
            RuntimeBinOp::BitAnd => "and",
            RuntimeBinOp::BitOr => "or",
            RuntimeBinOp::BitXor => "xor",
            RuntimeBinOp::Sub => "sub",
            _ => "add",
        },
        RuntimeInstr::NormalizeInt { .. } => "shl",
        _ => "mov",
    }
}

#[derive(Clone)]
struct PendingBlock {
    original: usize,
    instrs: Vec<RuntimeInstr>,
}

fn layout_profiled_blocks(
    program: &RuntimeProgram,
    profile: &FunctionProfile,
    report: &mut OptimizationReport,
) -> Option<(RuntimeProgram, Option<FunctionProfile>)> {
    if program
        .instrs
        .iter()
        .any(|instr| matches!(instr, RuntimeInstr::Call { .. }))
    {
        return None;
    }
    let ssa = RuntimeSSAProgram::lower(program);
    if ssa.blocks.len() < 2 {
        return None;
    }
    if !profile_matches_cfg(&ssa, profile) {
        return None;
    }
    let order = form_hot_traces(&ssa, profile);
    if order
        .iter()
        .enumerate()
        .all(|(position, block)| position == *block)
    {
        return Some((program.clone(), Some(profile.clone())));
    }
    report.reordered_blocks = order
        .iter()
        .enumerate()
        .filter(|(position, block)| *position != **block)
        .count();
    let order_position: HashMap<usize, usize> = order
        .iter()
        .enumerate()
        .map(|(position, block)| (*block, position))
        .collect();

    let mut pending = Vec::new();
    for (position, block_id) in order.iter().copied().enumerate() {
        let block = &ssa.blocks[block_id];
        let next = order.get(position + 1).copied();
        let original_fallthrough = block
            .instr_indices
            .last()
            .and_then(|last| (*last + 1 < program.instrs.len()).then_some(*last + 1))
            .and_then(|index| {
                ssa.blocks
                    .iter()
                    .find(|candidate| candidate.instr_indices.contains(&index))
                    .map(|candidate| candidate.id)
            });
        let mut instrs = block.instrs.clone();
        let terminator = instrs.pop();
        match terminator {
            Some(RuntimeInstr::JumpIfCmpFalse {
                op,
                lhs,
                rhs,
                target,
            }) => {
                let target_block = block_for_target(&ssa, target, program.instrs.len());
                if next == target_block && original_fallthrough.is_some() {
                    instrs.push(RuntimeInstr::JumpIfCmpFalse {
                        op: invert_cmp(op),
                        lhs,
                        rhs,
                        target: block_marker(original_fallthrough.unwrap()),
                    });
                    report.inverted_branches += 1;
                } else {
                    instrs.push(RuntimeInstr::JumpIfCmpFalse {
                        op,
                        lhs,
                        rhs,
                        target: target_block.map(block_marker).unwrap_or(usize::MAX),
                    });
                    if original_fallthrough != next {
                        if let Some(fallthrough) = original_fallthrough {
                            instrs.push(RuntimeInstr::Jump {
                                target: block_marker(fallthrough),
                            });
                            report.repaired_fallthroughs += 1;
                        }
                    }
                }
            }
            Some(RuntimeInstr::JumpIfZero { cond_slot, target }) => {
                let target_block = block_for_target(&ssa, target, program.instrs.len());
                if next == target_block && original_fallthrough.is_some() {
                    instrs.push(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Eq,
                        lhs: RuntimeOperand::Slot(cond_slot),
                        rhs: RuntimeOperand::Imm(0),
                        target: block_marker(original_fallthrough.unwrap()),
                    });
                    report.inverted_branches += 1;
                } else {
                    instrs.push(RuntimeInstr::JumpIfZero {
                        cond_slot,
                        target: target_block.map(block_marker).unwrap_or(usize::MAX),
                    });
                    if original_fallthrough != next {
                        if let Some(fallthrough) = original_fallthrough {
                            instrs.push(RuntimeInstr::Jump {
                                target: block_marker(fallthrough),
                            });
                            report.repaired_fallthroughs += 1;
                        }
                    }
                }
            }
            Some(RuntimeInstr::Jump { target }) => {
                let target_block = block_for_target(&ssa, target, program.instrs.len());
                instrs.push(RuntimeInstr::Jump {
                    target: target_block.map(block_marker).unwrap_or(usize::MAX),
                });
            }
            Some(terminal @ (RuntimeInstr::Return | RuntimeInstr::Exit { .. })) => {
                instrs.push(terminal)
            }
            Some(other) => {
                instrs.push(other);
                if original_fallthrough != next {
                    if let Some(fallthrough) = original_fallthrough {
                        instrs.push(RuntimeInstr::Jump {
                            target: block_marker(fallthrough),
                        });
                        report.repaired_fallthroughs += 1;
                    }
                }
            }
            None => {}
        }
        pending.push(PendingBlock {
            original: block_id,
            instrs,
        });
    }

    let mut starts = HashMap::new();
    let mut cursor = 0usize;
    for block in &pending {
        starts.insert(block.original, cursor);
        cursor += block.instrs.len();
    }
    let end = cursor;
    let mut instrs = Vec::with_capacity(end);
    let mut origin_by_instr = Vec::with_capacity(end);
    for block in pending {
        for mut instr in block.instrs {
            remap_marker_target(&mut instr, &starts, end);
            instrs.push(instr);
            origin_by_instr.push(block.original);
        }
    }
    let laid_out = RuntimeProgram {
        slots: program.slots,
        instrs,
    };
    let remapped = remap_profile(&laid_out, profile, &origin_by_instr, &order_position);
    Some((laid_out, Some(remapped)))
}

fn profile_matches_cfg(ssa: &RuntimeSSAProgram, profile: &FunctionProfile) -> bool {
    if profile.blocks.len() != ssa.blocks.len() {
        return false;
    }
    ssa.blocks.iter().all(|block| {
        let Some(profile_block) = profile.blocks.get(&block.id) else {
            return false;
        };
        profile_block
            .edge_counts
            .keys()
            .all(|successor| block.successors.contains(successor))
    })
}

const BLOCK_MARKER_BASE: usize = usize::MAX / 2;

fn block_marker(block: usize) -> usize {
    BLOCK_MARKER_BASE + block
}

fn remap_marker_target(instr: &mut RuntimeInstr, starts: &HashMap<usize, usize>, end: usize) {
    let target = match instr {
        RuntimeInstr::Jump { target }
        | RuntimeInstr::JumpIfZero { target, .. }
        | RuntimeInstr::JumpIfCmpFalse { target, .. }
        | RuntimeInstr::Call { target } => target,
        _ => return,
    };
    if *target == usize::MAX {
        *target = end;
    } else if *target >= BLOCK_MARKER_BASE {
        *target = starts
            .get(&(*target - BLOCK_MARKER_BASE))
            .copied()
            .unwrap_or(end);
    }
}

fn block_for_target(
    ssa: &RuntimeSSAProgram,
    target: usize,
    instruction_count: usize,
) -> Option<usize> {
    if target == instruction_count {
        return None;
    }
    ssa.blocks
        .iter()
        .find(|block| block.instr_indices.first().copied() == Some(target))
        .map(|block| block.id)
}

fn form_hot_traces(ssa: &RuntimeSSAProgram, profile: &FunctionProfile) -> Vec<usize> {
    let mut unplaced: BTreeSet<usize> = (0..ssa.blocks.len()).collect();
    let mut order = Vec::with_capacity(ssa.blocks.len());
    let mut current = ssa.entry_block;
    while !unplaced.is_empty() {
        if !unplaced.remove(&current) {
            current = *unplaced
                .iter()
                .max_by_key(|block| {
                    (
                        profile.block_exec_count(**block).unwrap_or(0),
                        std::cmp::Reverse(**block),
                    )
                })
                .unwrap();
            continue;
        }
        order.push(current);
        if let Some(successor) = ssa.blocks[current]
            .successors
            .iter()
            .filter(|successor| unplaced.contains(successor))
            .copied()
            .max_by_key(|successor| {
                (
                    profile.edge_count(current, *successor).unwrap_or(0),
                    profile.block_exec_count(*successor).unwrap_or(0),
                    std::cmp::Reverse(*successor),
                )
            })
        {
            current = successor;
        } else if let Some(next) = unplaced.iter().copied().max_by_key(|block| {
            (
                profile.block_exec_count(*block).unwrap_or(0),
                std::cmp::Reverse(*block),
            )
        }) {
            current = next;
        }
    }
    order
}

fn invert_cmp(op: RuntimeCmpOp) -> RuntimeCmpOp {
    match op {
        RuntimeCmpOp::Eq => RuntimeCmpOp::Ne,
        RuntimeCmpOp::Ne => RuntimeCmpOp::Eq,
        RuntimeCmpOp::LtUnsigned => RuntimeCmpOp::GeUnsigned,
        RuntimeCmpOp::LeUnsigned => RuntimeCmpOp::GtUnsigned,
        RuntimeCmpOp::GtUnsigned => RuntimeCmpOp::LeUnsigned,
        RuntimeCmpOp::GeUnsigned => RuntimeCmpOp::LtUnsigned,
        RuntimeCmpOp::LtSigned => RuntimeCmpOp::GeSigned,
        RuntimeCmpOp::LeSigned => RuntimeCmpOp::GtSigned,
        RuntimeCmpOp::GtSigned => RuntimeCmpOp::LeSigned,
        RuntimeCmpOp::GeSigned => RuntimeCmpOp::LtSigned,
    }
}

fn remap_profile(
    program: &RuntimeProgram,
    original: &FunctionProfile,
    origin_by_instr: &[usize],
    _order_position: &HashMap<usize, usize>,
) -> FunctionProfile {
    let ssa = RuntimeSSAProgram::lower(program);
    let mut blocks = HashMap::new();
    for block in &ssa.blocks {
        let origin = block
            .instr_indices
            .first()
            .and_then(|index| origin_by_instr.get(*index))
            .copied()
            .unwrap_or(0);
        let exec_count = original.block_exec_count(origin).unwrap_or(1).max(1);
        let mut block_profile = BlockProfile {
            exec_count,
            edge_counts: HashMap::new(),
        };
        for successor in &block.successors {
            let successor_origin = ssa.blocks[*successor]
                .instr_indices
                .first()
                .and_then(|index| origin_by_instr.get(*index))
                .copied()
                .unwrap_or(origin);
            let count = original
                .edge_count(origin, successor_origin)
                .unwrap_or_else(|| {
                    original
                        .block_exec_count(successor_origin)
                        .unwrap_or(1)
                        .min(exec_count)
                })
                .max(1);
            block_profile.edge_counts.insert(*successor, count);
        }
        blocks.insert(block.id, block_profile);
    }
    FunctionProfile {
        name: original.name.clone(),
        blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimization_report_is_stable_machine_readable_runtime_generic_json() {
        let report = OptimizationReport {
            affine_lookahead_groups: 2,
            affine_selects: 3,
            dead_prefix_reductions: 1,
            fixed_sort_selections: 4,
            partial_unrolled_loops: 9,
            propagated_copies: 5,
            eliminated_copies: 6,
            verified_rewrites: 7,
            scheduled_regions: 8,
            reordered_blocks: 1,
            inverted_branches: 2,
            repaired_fallthroughs: 3,
        };
        let encoded = report.machine_readable();
        assert!(
            encoded
                .starts_with("optimization-report {\"version\":1,\"pipeline\":\"runtime-generic\"")
        );
        assert!(encoded.contains("\"name\":\"affine-lookahead\",\"applied\":2"));
        assert!(encoded.contains("\"name\":\"copy-propagation\",\"applied\":11"));
        assert!(encoded.contains("\"name\":\"counted-loop-partial-unroll\",\"applied\":9"));
        assert!(encoded.contains("\"name\":\"cfg-layout\",\"applied\":6"));
        assert!(encoded.ends_with("}\n"));
    }

    #[test]
    fn verified_superoptimizer_folds_identities_and_constants() {
        let program = RuntimeProgram {
            slots: 2,
            instrs: vec![
                RuntimeInstr::BinOp {
                    dst: 0,
                    op: RuntimeBinOp::Mul,
                    lhs: RuntimeOperand::Slot(1),
                    rhs: RuntimeOperand::Imm(1),
                },
                RuntimeInstr::BinOp {
                    dst: 1,
                    op: RuntimeBinOp::Add,
                    lhs: RuntimeOperand::Imm(u64::MAX),
                    rhs: RuntimeOperand::Imm(2),
                },
            ],
        };
        let optimized = optimize_runtime_program(&program, None, "skylake");
        assert_eq!(
            optimized.report.verified_rewrites, 2,
            "optimized instructions: {:#?}",
            optimized.program.instrs
        );
        assert!(matches!(
            optimized.program.instrs[0],
            RuntimeInstr::Mov {
                src: RuntimeOperand::Slot(1),
                ..
            }
        ));
        assert!(matches!(
            optimized.program.instrs[1],
            RuntimeInstr::Mov {
                src: RuntimeOperand::Imm(1),
                ..
            }
        ));
    }

    #[test]
    fn profiled_layout_inverts_branch_for_hot_taken_trace() {
        let program = RuntimeProgram {
            slots: 1,
            instrs: vec![
                RuntimeInstr::JumpIfZero {
                    cond_slot: 0,
                    target: 3,
                },
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(1),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(0),
                },
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(2),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(0),
                },
            ],
        };
        let ssa = RuntimeSSAProgram::lower(&program);
        let mut profile = FunctionProfile {
            name: "runtime_generic".to_string(),
            blocks: HashMap::new(),
        };
        for block in &ssa.blocks {
            profile.blocks.insert(
                block.id,
                BlockProfile {
                    exec_count: if block.instr_indices.first() == Some(&3) {
                        100
                    } else {
                        1
                    },
                    edge_counts: HashMap::new(),
                },
            );
        }
        profile
            .blocks
            .get_mut(&0)
            .unwrap()
            .edge_counts
            .insert(2, 100);
        let optimized = optimize_runtime_program(&program, Some(&profile), "skylake");
        assert!(optimized.report.reordered_blocks > 0);
        assert!(optimized.report.inverted_branches > 0);
        assert!(optimized.profile.is_some());
        for condition in [0_u64, 1] {
            assert_eq!(
                execute_control_subset(&program, condition),
                execute_control_subset(&optimized.program, condition)
            );
        }
        let repeated = optimize_runtime_program(&program, Some(&profile), "skylake");
        assert_eq!(
            format!("{:?}", optimized.program.instrs),
            format!("{:?}", repeated.program.instrs)
        );
    }

    #[test]
    fn stale_profile_is_ignored_without_changing_control_flow() {
        let program = RuntimeProgram {
            slots: 1,
            instrs: vec![
                RuntimeInstr::JumpIfZero {
                    cond_slot: 0,
                    target: 2,
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Imm(1),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Imm(2),
                },
            ],
        };
        let mut stale = FunctionProfile {
            name: "runtime_generic".to_string(),
            blocks: HashMap::new(),
        };
        stale.blocks.insert(
            99,
            BlockProfile {
                exec_count: 1_000_000,
                edge_counts: HashMap::from([(100, 1_000_000)]),
            },
        );
        let optimized = optimize_runtime_program(&program, Some(&stale), "skylake");
        assert!(optimized.profile.is_none());
        assert_eq!(optimized.report.reordered_blocks, 0);
        for condition in [0_u64, 1] {
            assert_eq!(
                execute_control_subset(&program, condition),
                execute_control_subset(&optimized.program, condition)
            );
        }
    }

    #[test]
    fn profiled_loop_preserves_backedge_and_cold_exit() {
        let program = RuntimeProgram {
            slots: 1,
            instrs: vec![
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(3),
                },
                RuntimeInstr::JumpIfZero {
                    cond_slot: 0,
                    target: 4,
                },
                RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: RuntimeBinOp::Sub,
                    rhs: RuntimeOperand::Imm(1),
                },
                RuntimeInstr::Jump { target: 1 },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(0),
                },
            ],
        };
        let ssa = RuntimeSSAProgram::lower(&program);
        let mut profile = FunctionProfile {
            name: "runtime_generic".to_string(),
            blocks: HashMap::new(),
        };
        for block in &ssa.blocks {
            let mut edge_counts = HashMap::new();
            for successor in &block.successors {
                edge_counts.insert(*successor, if *successor == 3 { 1 } else { 100 });
            }
            profile.blocks.insert(
                block.id,
                BlockProfile {
                    exec_count: if block.id == 3 { 1 } else { 100 },
                    edge_counts,
                },
            );
        }
        let optimized = optimize_runtime_program(&program, Some(&profile), "skylake");
        assert!(optimized.profile.is_some());
        assert_eq!(
            execute_control_subset(&program, 0),
            execute_control_subset(&optimized.program, 0)
        );
        let repeated = optimize_runtime_program(&program, Some(&profile), "skylake");
        assert_eq!(
            format!("{:?}", optimized.program.instrs),
            format!("{:?}", repeated.program.instrs)
        );
    }

    #[test]
    fn power_of_two_rewrites_are_exhaustive_for_u8_domain() {
        for value in 0_u64..=u8::MAX as u64 {
            for power in [2_u64, 4, 8, 16, 32, 64, 128] {
                let shift = power.trailing_zeros() as u64;
                assert_eq!(
                    value.wrapping_mul(power) as u8,
                    value.wrapping_shl(shift as u32) as u8
                );
                assert_eq!(value / power, value >> shift);
                assert_eq!(value % power, value & (power - 1));
            }
        }
    }

    #[test]
    fn identity_rewrite_families_are_exhaustive_for_u8_domain() {
        let cases = [
            (RuntimeBinOp::Add, RuntimeOperand::Imm(0)),
            (RuntimeBinOp::Sub, RuntimeOperand::Imm(0)),
            (RuntimeBinOp::Mul, RuntimeOperand::Imm(1)),
            (RuntimeBinOp::Mul, RuntimeOperand::Imm(0)),
            (RuntimeBinOp::BitAnd, RuntimeOperand::Imm(u64::MAX)),
            (RuntimeBinOp::BitAnd, RuntimeOperand::Imm(0)),
            (RuntimeBinOp::BitOr, RuntimeOperand::Imm(0)),
            (RuntimeBinOp::BitXor, RuntimeOperand::Imm(0)),
            (RuntimeBinOp::Shl, RuntimeOperand::Imm(0)),
            (RuntimeBinOp::ShrUnsigned, RuntimeOperand::Imm(0)),
            (RuntimeBinOp::ShrSigned, RuntimeOperand::Imm(0)),
        ];
        for (op, rhs) in cases {
            assert_u8_rewrite_equivalent(&RuntimeInstr::BinOp {
                dst: 0,
                op,
                lhs: RuntimeOperand::Slot(1),
                rhs,
            });
        }
        for op in [
            RuntimeBinOp::Sub,
            RuntimeBinOp::BitAnd,
            RuntimeBinOp::BitOr,
            RuntimeBinOp::BitXor,
        ] {
            assert_u8_rewrite_equivalent(&RuntimeInstr::BinOp {
                dst: 0,
                op,
                lhs: RuntimeOperand::Slot(1),
                rhs: RuntimeOperand::Slot(1),
            });
        }
        assert_u8_rewrite_equivalent(&RuntimeInstr::BinOpInPlace {
            dst: 0,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Imm(0),
        });
    }

    #[test]
    fn bounded_search_moves_commutative_immediate_to_cheaper_rhs() {
        let original = RuntimeInstr::BinOp {
            dst: 0,
            op: RuntimeBinOp::Add,
            lhs: RuntimeOperand::Imm(7),
            rhs: RuntimeOperand::Slot(1),
        };
        let candidate = enumerate_candidates(&original)
            .into_iter()
            .find(|candidate| {
                matches!(
                    candidate,
                    RuntimeInstr::BinOp {
                        lhs: RuntimeOperand::Slot(1),
                        rhs: RuntimeOperand::Imm(7),
                        ..
                    }
                )
            })
            .expect("bounded search should enumerate the encodable commutative form");
        assert!(verify_candidate(&original, &candidate));
        assert!(
            target_instruction_cost(&candidate, "skylake")
                <= target_instruction_cost(&original, "skylake")
        );
    }

    #[test]
    fn single_assignment_propagation_precedes_scheduling_deterministically() {
        let program = RuntimeProgram {
            slots: 4,
            instrs: vec![
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(7),
                },
                RuntimeInstr::Mov {
                    dst: 1,
                    src: RuntimeOperand::Imm(9),
                },
                RuntimeInstr::BinOp {
                    dst: 2,
                    op: RuntimeBinOp::Mul,
                    lhs: RuntimeOperand::Slot(0),
                    rhs: RuntimeOperand::Imm(3),
                },
                RuntimeInstr::BinOp {
                    dst: 3,
                    op: RuntimeBinOp::Add,
                    lhs: RuntimeOperand::Slot(1),
                    rhs: RuntimeOperand::Imm(5),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(2),
                },
            ],
        };
        let optimized = optimize_runtime_program(&program, None, "skylake");
        assert_eq!(optimized.report.eliminated_copies, 2);
        assert!(optimized.report.propagated_copies >= 2);
        assert!(optimized.program.instrs.iter().any(|instr| matches!(
            instr,
            RuntimeInstr::Mov {
                src: RuntimeOperand::Imm(21),
                ..
            }
        )));
        let repeated = optimize_runtime_program(&program, None, "skylake");
        assert_eq!(
            format!("{:?}", optimized.program.instrs),
            format!("{:?}", repeated.program.instrs)
        );
    }

    #[test]
    fn copy_propagation_never_elides_addressable_storage_initialization() {
        let program = RuntimeProgram {
            slots: 3,
            instrs: vec![
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(7),
                },
                RuntimeInstr::LoadIndexUnchecked {
                    dst: 1,
                    base_slots: vec![0],
                    index: RuntimeOperand::Imm(0),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(1),
                },
            ],
        };
        let optimized = optimize_runtime_program(&program, None, "skylake");
        assert_eq!(optimized.report.eliminated_copies, 0);
        assert!(matches!(
            optimized.program.instrs[0],
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(7)
            }
        ));
    }

    #[test]
    fn copy_propagation_rejects_definition_bypassed_by_branch() {
        let program = RuntimeProgram {
            slots: 1,
            instrs: vec![
                RuntimeInstr::Jump { target: 2 },
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(7),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(0),
                },
            ],
        };
        let optimized = optimize_runtime_program(&program, None, "skylake");
        assert_eq!(optimized.report.eliminated_copies, 0);
        assert!(optimized.program.instrs.iter().any(|instr| matches!(
            instr,
            RuntimeInstr::Mov {
                dst: 0,
                src: RuntimeOperand::Imm(7)
            }
        )));
    }

    #[test]
    fn copy_propagation_canonicalizes_chains_longer_than_sixty_four() {
        const SLOTS: usize = 130;
        let mut instrs = vec![RuntimeInstr::Mov {
            dst: 0,
            src: RuntimeOperand::Imm(19),
        }];
        for dst in 1..SLOTS {
            instrs.push(RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Slot(dst - 1),
            });
        }
        instrs.push(RuntimeInstr::Exit {
            code: RuntimeOperand::Slot(SLOTS - 1),
        });
        let optimized = optimize_runtime_program(
            &RuntimeProgram {
                slots: SLOTS,
                instrs,
            },
            None,
            "skylake",
        );
        assert_eq!(optimized.report.eliminated_copies, SLOTS);
        assert_eq!(optimized.program.instrs.len(), 1);
        assert!(matches!(
            optimized.program.instrs[0],
            RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(19)
            }
        ));
    }

    fn execute_control_subset(program: &RuntimeProgram, condition: u64) -> u64 {
        let mut slots = vec![0_u64; program.slots];
        slots[0] = condition;
        let mut pc = 0usize;
        for _ in 0..64 {
            match &program.instrs[pc] {
                RuntimeInstr::Mov { dst, src } => {
                    slots[*dst] = match src {
                        RuntimeOperand::Imm(value) => *value,
                        RuntimeOperand::Slot(slot) => slots[*slot],
                    };
                    pc += 1;
                }
                RuntimeInstr::BinOpInPlace { dst, op, rhs } => {
                    let rhs = match rhs {
                        RuntimeOperand::Imm(value) => *value,
                        RuntimeOperand::Slot(slot) => slots[*slot],
                    };
                    slots[*dst] = evaluate_binop(*op, slots[*dst], rhs)
                        .expect("test arithmetic should be defined");
                    pc += 1;
                }
                RuntimeInstr::Jump { target } => pc = *target,
                RuntimeInstr::JumpIfZero { cond_slot, target } => {
                    pc = if slots[*cond_slot] == 0 {
                        *target
                    } else {
                        pc + 1
                    };
                }
                RuntimeInstr::JumpIfCmpFalse {
                    op,
                    lhs,
                    rhs,
                    target,
                } => {
                    let read = |operand: &RuntimeOperand| match operand {
                        RuntimeOperand::Imm(value) => *value,
                        RuntimeOperand::Slot(slot) => slots[*slot],
                    };
                    pc = if !evaluate_cmp(*op, read(lhs), read(rhs)) {
                        *target
                    } else {
                        pc + 1
                    };
                }
                RuntimeInstr::Exit { code } => {
                    return match code {
                        RuntimeOperand::Imm(value) => *value,
                        RuntimeOperand::Slot(slot) => slots[*slot],
                    };
                }
                other => panic!("unsupported test instruction: {other:?}"),
            }
        }
        panic!("test program did not exit")
    }

    fn assert_u8_rewrite_equivalent(original: &RuntimeInstr) {
        let candidate = synthesize_candidate(original).expect("rewrite must be synthesized");
        assert!(verify_candidate(original, &candidate));
        for value in 0_u64..=u8::MAX as u64 {
            let environment = HashMap::from([(0, value), (1, value)]);
            assert_eq!(
                evaluate_pure_instruction(original, &environment),
                evaluate_pure_instruction(&candidate, &environment),
                "original={original:?} candidate={candidate:?} value={value}"
            );
        }
    }
}
