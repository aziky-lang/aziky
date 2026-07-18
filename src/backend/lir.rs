use std::collections::{BTreeSet, HashMap, HashSet};

use crate::backend::profile::{CompileProfile, FunctionProfile};
use crate::frontend::semantics::{RuntimeInstr, RuntimeOperand, RuntimeProgram};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BorrowRegion(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasClass {
    Unique,
    ReadOnlyShared,
    Conservative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Argument escape is reserved for the next runtime-argument IR extension.
pub enum EscapeClass {
    NoEscape,
    ArgEscape,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRepresentation {
    /// Every access has a statically selected element, so elements may live in GPRs.
    Scalarized,
    /// Runtime indexing requires addressable, contiguous eight-byte elements.
    ContiguousStack,
    /// The source object is a bitset; compact bit-addressing operations are legal.
    PackedBitset,
    /// Every stored value is proven to fit in one byte and the object does not
    /// escape, so logical u64 loads may use zero-extending byte storage.
    PackedBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryObject {
    pub id: ObjectId,
    pub slots: Vec<usize>,
    pub alias_class: AliasClass,
    pub escape_class: EscapeClass,
    pub borrow_region: Option<BorrowRegion>,
    pub representation: MemoryRepresentation,
}

#[derive(Debug, Clone)]
pub struct RuntimeSSABlock {
    pub id: usize,
    /// SSA value ids entering this block through phi/block parameters.
    pub params: Vec<usize>,
    pub instrs: Vec<RuntimeInstr>,
    pub instr_indices: Vec<usize>,
    pub predecessors: Vec<usize>,
    pub successors: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SSAValueDef {
    Entry,
    Phi { block: usize },
    Instr { instr_index: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SSAValue {
    pub id: usize,
    pub slot: usize,
    pub def: SSAValueDef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SSAUse {
    pub slot: usize,
    pub value: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SSAInstruction {
    pub instr_index: usize,
    pub reads: Vec<SSAUse>,
    pub writes: Vec<SSAUse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SSAPhiInput {
    /// `None` is the synthetic function-entry predecessor.
    pub predecessor: Option<usize>,
    pub value: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SSAPhi {
    pub block: usize,
    pub slot: usize,
    pub value: usize,
    pub inputs: Vec<SSAPhiInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownBits {
    pub zero: u64,
    pub one: u64,
}

impl KnownBits {
    pub const UNKNOWN: Self = Self { zero: 0, one: 0 };

    pub fn constant(value: u64) -> Self {
        Self {
            zero: !value,
            one: value,
        }
    }

    pub fn unsigned_min(self) -> u64 {
        self.one
    }

    pub fn unsigned_max(self) -> u64 {
        !self.zero
    }

    pub fn width(self) -> u8 {
        let max = self.unsigned_max();
        if max == 0 {
            1
        } else {
            (u64::BITS - max.leading_zeros()) as u8
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryAccessKind {
    Read,
    Write,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryVersionDef {
    Entry,
    Phi { block: usize },
    Instr { instr_index: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryVersion {
    pub id: usize,
    pub object: ObjectId,
    pub def: MemoryVersionDef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPhi {
    pub block: usize,
    pub object: ObjectId,
    pub version: usize,
    pub inputs: Vec<SSAPhiInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryAccess {
    pub instr_index: usize,
    pub object: ObjectId,
    pub kind: MemoryAccessKind,
    pub incoming_version: usize,
    pub outgoing_version: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSiteABI {
    pub instr_index: usize,
    pub target: usize,
    pub continuation: usize,
    pub tail: bool,
    pub callee_leaf: bool,
    pub live_across: Vec<usize>,
    pub callee_reads: Vec<usize>,
    pub callee_writes: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopDependencyKind {
    Flow,
    Anti,
    Output,
    Memory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopDependency {
    pub from_instr: usize,
    pub to_instr: usize,
    pub kind: LoopDependencyKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AffineRecurrence {
    pub slot: usize,
    pub mul: u64,
    pub add: u64,
    pub mask: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopDescriptor {
    pub header: usize,
    pub latch: usize,
    pub blocks: Vec<usize>,
    pub dependencies: Vec<LoopDependency>,
    pub recurrences: Vec<AffineRecurrence>,
}

#[derive(Debug, Clone)]
pub struct RuntimeSSAProgram {
    pub slot_count: usize,
    pub entry_block: usize,
    pub blocks: Vec<RuntimeSSABlock>,
    pub objects: Vec<MemoryObject>,
    pub values: Vec<SSAValue>,
    pub instructions: Vec<SSAInstruction>,
    pub phis: Vec<SSAPhi>,
    pub known_bits: Vec<KnownBits>,
    pub demanded_bits: Vec<u64>,
    pub memory_versions: Vec<MemoryVersion>,
    pub memory_phis: Vec<MemoryPhi>,
    pub memory_accesses: Vec<MemoryAccess>,
    pub call_sites: Vec<CallSiteABI>,
}

#[derive(Debug, Clone)]
pub struct MachineLIRBlock {
    pub id: usize,
    pub instrs: Vec<RuntimeInstr>,
    pub instr_indices: Vec<usize>,
    pub predecessors: Vec<usize>,
    pub successors: Vec<usize>,
    pub frequency: u64,
    pub loop_depth: usize,
}

#[derive(Debug, Clone)]
pub struct MachineLIRProgram {
    pub slot_count: usize,
    pub entry_block: usize,
    pub blocks: Vec<MachineLIRBlock>,
    pub objects: Vec<MemoryObject>,
    pub values: Vec<SSAValue>,
    pub instructions: Vec<SSAInstruction>,
    pub phis: Vec<SSAPhi>,
    pub known_bits: Vec<KnownBits>,
    pub demanded_bits: Vec<u64>,
    pub memory_versions: Vec<MemoryVersion>,
    pub memory_phis: Vec<MemoryPhi>,
    pub memory_accesses: Vec<MemoryAccess>,
    pub call_sites: Vec<CallSiteABI>,
    pub loops: Vec<LoopDescriptor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveSegment {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveInterval {
    pub slot: usize,
    pub start: usize,
    pub end: usize,
    pub segments: Vec<LiveSegment>,
    pub spill_weight: u64,
    pub force_stack: bool,
    pub rematerializable: Option<u64>,
    pub copy_hint: Option<usize>,
}

impl LiveInterval {
    pub fn interferes(&self, other: &Self) -> bool {
        let mut left = 0;
        let mut right = 0;
        while left < self.segments.len() && right < other.segments.len() {
            let a = self.segments[left];
            let b = other.segments[right];
            if a.end < b.start {
                left += 1;
            } else if b.end < a.start {
                right += 1;
            } else {
                return true;
            }
        }
        false
    }
}

impl RuntimeSSAProgram {
    pub fn lower(program: &RuntimeProgram) -> Self {
        let (starts, successors) = build_block_graph(program);
        let mut blocks = Vec::with_capacity(starts.len());
        let mut index_to_block = vec![0usize; program.instrs.len()];

        for (block_id, start) in starts.iter().copied().enumerate() {
            let end = starts
                .get(block_id + 1)
                .copied()
                .unwrap_or(program.instrs.len());
            for instr_index in start..end {
                index_to_block[instr_index] = block_id;
            }
        }

        let predecessors = invert_successors(successors.len(), &successors);
        for (block_id, start) in starts.iter().copied().enumerate() {
            let end = starts
                .get(block_id + 1)
                .copied()
                .unwrap_or(program.instrs.len());
            blocks.push(RuntimeSSABlock {
                id: block_id,
                params: Vec::new(),
                instrs: program.instrs[start..end].to_vec(),
                instr_indices: (start..end).collect(),
                predecessors: predecessors[block_id].clone(),
                successors: successors[block_id].clone(),
            });
        }

        debug_assert_eq!(starts.first().copied().unwrap_or(0), 0);

        let objects = infer_memory_objects(program);
        let mut ssa = Self {
            slot_count: program.slots,
            entry_block: 0,
            blocks,
            objects,
            values: Vec::new(),
            instructions: Vec::new(),
            phis: Vec::new(),
            known_bits: Vec::new(),
            demanded_bits: Vec::new(),
            memory_versions: Vec::new(),
            memory_phis: Vec::new(),
            memory_accesses: Vec::new(),
            call_sites: Vec::new(),
        };
        build_value_ssa(&mut ssa);
        refine_memory_representations(program, &mut ssa);
        ssa.demanded_bits = compute_demanded_bits(&ssa);
        build_memory_ssa(&mut ssa);
        ssa.call_sites = analyze_call_sites(program);
        ssa
    }

    #[allow(dead_code)]
    pub fn verify(&self, expected_instr_count: usize) -> Result<(), String> {
        if self.blocks.is_empty() && expected_instr_count != 0 {
            return Err("ssa program has no blocks".to_string());
        }

        let mut covered = BTreeSet::new();
        for (expected_block_id, block) in self.blocks.iter().enumerate() {
            if block.id != expected_block_id {
                return Err(format!(
                    "ssa block ids must be dense (expected {}, got {})",
                    expected_block_id, block.id
                ));
            }
            if block.instrs.len() != block.instr_indices.len() {
                return Err(format!(
                    "ssa block {} has mismatched instruction metadata",
                    block.id
                ));
            }
            for &succ in &block.successors {
                if succ >= self.blocks.len() {
                    return Err(format!(
                        "ssa block {} has invalid successor {}",
                        block.id, succ
                    ));
                }
            }
            for &pred in &block.predecessors {
                if pred >= self.blocks.len() {
                    return Err(format!(
                        "ssa block {} has invalid predecessor {}",
                        block.id, pred
                    ));
                }
            }
            for &instr_index in &block.instr_indices {
                if !covered.insert(instr_index) {
                    return Err(format!(
                        "instruction {} is covered by multiple ssa blocks",
                        instr_index
                    ));
                }
            }
        }

        if covered.len() != expected_instr_count {
            return Err(format!(
                "ssa coverage mismatch: expected {} instructions, covered {}",
                expected_instr_count,
                covered.len()
            ));
        }
        if covered.first().copied().unwrap_or(0) != 0
            || covered.last().copied().unwrap_or(0) + 1 != expected_instr_count
        {
            return Err("ssa instructions are not fully contiguous".to_string());
        }

        for phi in &self.phis {
            if phi.block >= self.blocks.len() || phi.slot >= self.slot_count {
                return Err("ssa phi references an invalid block or slot".to_string());
            }
            let value = self
                .values
                .get(phi.value)
                .ok_or_else(|| "ssa phi references an invalid value".to_string())?;
            if value.slot != phi.slot || value.def != (SSAValueDef::Phi { block: phi.block }) {
                return Err("ssa phi value definition mismatch".to_string());
            }
            for input in &phi.inputs {
                if input.value >= self.values.len() {
                    return Err("ssa phi input references an invalid value".to_string());
                }
            }
        }
        if self.known_bits.len() != self.values.len() {
            return Err("known-bit facts do not cover every ssa value".to_string());
        }
        if self.demanded_bits.len() != self.values.len() {
            return Err("demanded-bit facts do not cover every ssa value".to_string());
        }
        for instruction in &self.instructions {
            for usage in instruction.reads.iter().chain(instruction.writes.iter()) {
                let value = self
                    .values
                    .get(usage.value)
                    .ok_or_else(|| "ssa instruction references an invalid value".to_string())?;
                if value.slot != usage.slot {
                    return Err("ssa instruction slot/value mismatch".to_string());
                }
            }
        }
        for (expected_id, version) in self.memory_versions.iter().enumerate() {
            if version.id != expected_id || version.object.0 >= self.objects.len() {
                return Err("memory SSA version has an invalid id or object".to_string());
            }
        }
        for phi in &self.memory_phis {
            let version = self
                .memory_versions
                .get(phi.version)
                .ok_or_else(|| "memory phi references an invalid version".to_string())?;
            if phi.block >= self.blocks.len()
                || phi.object.0 >= self.objects.len()
                || version.object != phi.object
                || version.def != (MemoryVersionDef::Phi { block: phi.block })
            {
                return Err("memory phi definition mismatch".to_string());
            }
            for input in &phi.inputs {
                let input_version = self
                    .memory_versions
                    .get(input.value)
                    .ok_or_else(|| "memory phi input references an invalid version".to_string())?;
                if input_version.object != phi.object {
                    return Err("memory phi merges different objects".to_string());
                }
            }
        }
        for access in &self.memory_accesses {
            let incoming = self
                .memory_versions
                .get(access.incoming_version)
                .ok_or_else(|| "memory access has an invalid incoming version".to_string())?;
            let outgoing = self
                .memory_versions
                .get(access.outgoing_version)
                .ok_or_else(|| "memory access has an invalid outgoing version".to_string())?;
            if incoming.object != access.object || outgoing.object != access.object {
                return Err("memory access version/object mismatch".to_string());
            }
            if access.kind == MemoryAccessKind::Read {
                if access.incoming_version != access.outgoing_version {
                    return Err("read-only memory access unexpectedly defines memory".to_string());
                }
            } else if outgoing.def
                != (MemoryVersionDef::Instr {
                    instr_index: access.instr_index,
                })
            {
                return Err("memory write version definition mismatch".to_string());
            }
        }

        Ok(())
    }
}

fn build_value_ssa(ssa: &mut RuntimeSSAProgram) {
    let mut values = Vec::new();
    let mut entry_values = Vec::with_capacity(ssa.slot_count);
    for slot in 0..ssa.slot_count {
        let id = values.len();
        values.push(SSAValue {
            id,
            slot,
            def: SSAValueDef::Entry,
        });
        entry_values.push(id);
    }

    let mut instruction_defs = HashMap::<(usize, usize), usize>::new();
    for block in &ssa.blocks {
        for (&instr_index, instr) in block.instr_indices.iter().zip(&block.instrs) {
            let mut writes = write_slots(instr);
            writes.sort_unstable();
            writes.dedup();
            for slot in writes {
                let id = values.len();
                values.push(SSAValue {
                    id,
                    slot,
                    def: SSAValueDef::Instr { instr_index },
                });
                instruction_defs.insert((instr_index, slot), id);
            }
        }
    }

    let mut phi_ids = HashMap::<(usize, usize), usize>::new();
    let mut block_in = vec![entry_values.clone(); ssa.blocks.len()];
    let mut block_out = vec![entry_values.clone(); ssa.blocks.len()];
    let iteration_limit = ssa
        .blocks
        .len()
        .saturating_mul(ssa.slot_count.max(1))
        .saturating_mul(4)
        .saturating_add(8);

    for _ in 0..iteration_limit {
        let mut changed = false;
        for block in &ssa.blocks {
            let mut incoming = entry_values.clone();
            for slot in 0..ssa.slot_count {
                let mut candidates = Vec::new();
                if block.id == ssa.entry_block || block.predecessors.is_empty() {
                    candidates.push(entry_values[slot]);
                }
                for &pred in &block.predecessors {
                    candidates.push(block_out[pred][slot]);
                }
                candidates.sort_unstable();
                candidates.dedup();
                incoming[slot] = if candidates.len() == 1 {
                    candidates[0]
                } else {
                    *phi_ids.entry((block.id, slot)).or_insert_with(|| {
                        let id = values.len();
                        values.push(SSAValue {
                            id,
                            slot,
                            def: SSAValueDef::Phi { block: block.id },
                        });
                        id
                    })
                };
            }

            let mut outgoing = incoming.clone();
            for (&instr_index, instr) in block.instr_indices.iter().zip(&block.instrs) {
                let mut writes = write_slots(instr);
                writes.sort_unstable();
                writes.dedup();
                for slot in writes {
                    outgoing[slot] = instruction_defs[&(instr_index, slot)];
                }
            }
            if block_in[block.id] != incoming || block_out[block.id] != outgoing {
                block_in[block.id] = incoming;
                block_out[block.id] = outgoing;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut phis = Vec::new();
    let mut phi_entries: Vec<((usize, usize), usize)> = phi_ids.into_iter().collect();
    phi_entries.sort_by_key(|((block, slot), _)| (*block, *slot));
    for ((block, slot), value) in phi_entries {
        let mut inputs = Vec::new();
        if block == ssa.entry_block || ssa.blocks[block].predecessors.is_empty() {
            inputs.push(SSAPhiInput {
                predecessor: None,
                value: entry_values[slot],
            });
        }
        for &pred in &ssa.blocks[block].predecessors {
            inputs.push(SSAPhiInput {
                predecessor: Some(pred),
                value: block_out[pred][slot],
            });
        }
        inputs.sort_by_key(|input| (input.predecessor, input.value));
        inputs.dedup();
        phis.push(SSAPhi {
            block,
            slot,
            value,
            inputs,
        });
    }

    for block in &mut ssa.blocks {
        block.params = phis
            .iter()
            .filter(|phi| phi.block == block.id)
            .map(|phi| phi.value)
            .collect();
    }

    let mut instructions = Vec::new();
    for block in &ssa.blocks {
        let mut current = block_in[block.id].clone();
        for (&instr_index, instr) in block.instr_indices.iter().zip(&block.instrs) {
            let reads = read_slots(instr)
                .into_iter()
                .map(|slot| SSAUse {
                    slot,
                    value: current[slot],
                })
                .collect();
            let mut writes = Vec::new();
            let mut written_slots = write_slots(instr);
            written_slots.sort_unstable();
            written_slots.dedup();
            for slot in written_slots {
                let value = instruction_defs[&(instr_index, slot)];
                writes.push(SSAUse { slot, value });
                current[slot] = value;
            }
            instructions.push(SSAInstruction {
                instr_index,
                reads,
                writes,
            });
        }
    }
    instructions.sort_by_key(|instruction| instruction.instr_index);

    ssa.values = values;
    ssa.instructions = instructions;
    ssa.phis = phis;
    ssa.known_bits = compute_known_bits(ssa);
}

fn compute_known_bits(ssa: &RuntimeSSAProgram) -> Vec<KnownBits> {
    let mut facts = vec![KnownBits::UNKNOWN; ssa.values.len()];
    let instruction_by_index: HashMap<usize, &SSAInstruction> = ssa
        .instructions
        .iter()
        .map(|instruction| (instruction.instr_index, instruction))
        .collect();
    let runtime_instr_by_index: HashMap<usize, &RuntimeInstr> = ssa
        .blocks
        .iter()
        .flat_map(|block| block.instr_indices.iter().copied().zip(block.instrs.iter()))
        .collect();
    let phi_by_value: HashMap<usize, &SSAPhi> =
        ssa.phis.iter().map(|phi| (phi.value, phi)).collect();

    let limit = ssa.values.len().saturating_mul(4).saturating_add(8);
    for _ in 0..limit {
        let previous = facts.clone();
        for value in &ssa.values {
            let next = match value.def {
                SSAValueDef::Entry => KnownBits::UNKNOWN,
                SSAValueDef::Phi { .. } => phi_by_value
                    .get(&value.id)
                    .map(|phi| known_bits_for_phi(phi, &previous))
                    .unwrap_or(KnownBits::UNKNOWN),
                SSAValueDef::Instr { instr_index } => {
                    match (
                        runtime_instr_by_index.get(&instr_index),
                        instruction_by_index.get(&instr_index),
                    ) {
                        (Some(instr), Some(ssa_instr)) => {
                            known_bits_for_instruction(instr, ssa_instr, value.slot, &previous)
                        }
                        _ => KnownBits::UNKNOWN,
                    }
                }
            };
            facts[value.id] = KnownBits {
                zero: next.zero & !next.one,
                one: next.one & !next.zero,
            };
        }
        if facts == previous {
            break;
        }
    }
    facts
}

fn known_bits_for_phi(phi: &SSAPhi, facts: &[KnownBits]) -> KnownBits {
    let mut inputs = phi.inputs.iter();
    let Some(first) = inputs.next() else {
        return KnownBits::UNKNOWN;
    };
    let mut result = facts
        .get(first.value)
        .copied()
        .unwrap_or(KnownBits::UNKNOWN);
    for input in inputs {
        let fact = facts
            .get(input.value)
            .copied()
            .unwrap_or(KnownBits::UNKNOWN);
        result.zero &= fact.zero;
        result.one &= fact.one;
    }
    result
}

fn known_bits_for_instruction(
    instr: &RuntimeInstr,
    ssa_instr: &SSAInstruction,
    written_slot: usize,
    facts: &[KnownBits],
) -> KnownBits {
    let operand_fact = |operand: &RuntimeOperand| match operand {
        RuntimeOperand::Imm(value) => KnownBits::constant(*value),
        RuntimeOperand::Slot(slot) => ssa_instr
            .reads
            .iter()
            .find(|usage| usage.slot == *slot)
            .and_then(|usage| facts.get(usage.value))
            .copied()
            .unwrap_or(KnownBits::UNKNOWN),
    };
    let slot_fact = |slot: usize| {
        ssa_instr
            .reads
            .iter()
            .find(|usage| usage.slot == slot)
            .and_then(|usage| facts.get(usage.value))
            .copied()
            .unwrap_or(KnownBits::UNKNOWN)
    };

    match instr {
        RuntimeInstr::Mov { src, .. } => operand_fact(src),
        RuntimeInstr::BinOp { op, lhs, rhs, .. } => {
            known_bits_for_binop(*op, lhs, rhs, operand_fact(lhs), operand_fact(rhs))
        }
        RuntimeInstr::BinOpInPlace { dst, op, rhs } => known_bits_for_binop(
            *op,
            &RuntimeOperand::Slot(*dst),
            rhs,
            slot_fact(*dst),
            operand_fact(rhs),
        ),
        RuntimeInstr::Cmp { .. }
        | RuntimeInstr::BloomSplitBlockCheck { .. }
        | RuntimeInstr::JoinSelectAdaptive { .. } => KnownBits { zero: !1, one: 0 },
        RuntimeInstr::BloomClassic4Check { dst, .. } if written_slot == *dst => {
            KnownBits { zero: !1, one: 0 }
        }
        RuntimeInstr::BloomClassic4Check { lanes_checked, .. }
            if written_slot == *lanes_checked =>
        {
            KnownBits { zero: !7, one: 0 }
        }
        RuntimeInstr::NormalizeInt {
            dst, signed, bits, ..
        } => {
            let input = slot_fact(*dst);
            let width = (*bits).min(64) as u32;
            if width == 64 {
                input
            } else if !*signed {
                let low_mask = if width == 0 { 0 } else { (1_u64 << width) - 1 };
                KnownBits {
                    zero: input.zero | !low_mask,
                    one: input.one & low_mask,
                }
            } else {
                KnownBits::UNKNOWN
            }
        }
        _ => KnownBits::UNKNOWN,
    }
}

fn known_bits_for_binop(
    op: crate::frontend::semantics::RuntimeBinOp,
    lhs_operand: &RuntimeOperand,
    rhs_operand: &RuntimeOperand,
    lhs: KnownBits,
    rhs: KnownBits,
) -> KnownBits {
    use crate::frontend::semantics::RuntimeBinOp;
    match op {
        RuntimeBinOp::BitAnd => KnownBits {
            zero: lhs.zero | rhs.zero,
            one: lhs.one & rhs.one,
        },
        RuntimeBinOp::BitOr => KnownBits {
            zero: lhs.zero & rhs.zero,
            one: lhs.one | rhs.one,
        },
        RuntimeBinOp::BitXor => KnownBits {
            zero: (lhs.zero & rhs.zero) | (lhs.one & rhs.one),
            one: (lhs.zero & rhs.one) | (lhs.one & rhs.zero),
        },
        RuntimeBinOp::Shl => match rhs_operand {
            RuntimeOperand::Imm(shift) if *shift < 64 => KnownBits {
                zero: (lhs.zero << *shift) | ((1_u64 << *shift).wrapping_sub(1)),
                one: lhs.one << *shift,
            },
            _ => KnownBits::UNKNOWN,
        },
        RuntimeBinOp::ShrUnsigned => match rhs_operand {
            RuntimeOperand::Imm(shift) if *shift < 64 => {
                let introduced_zero = if *shift == 0 {
                    0
                } else {
                    !0_u64 << (64 - *shift as u32)
                };
                KnownBits {
                    zero: (lhs.zero >> *shift) | introduced_zero,
                    one: lhs.one >> *shift,
                }
            }
            _ => KnownBits::UNKNOWN,
        },
        RuntimeBinOp::Add => match (lhs_operand, rhs_operand) {
            (_, RuntimeOperand::Imm(0)) => lhs,
            (RuntimeOperand::Imm(0), _) => rhs,
            _ => {
                let lhs_max = !lhs.zero;
                let rhs_max = !rhs.zero;
                match lhs_max.checked_add(rhs_max) {
                    Some(max) => KnownBits {
                        zero: !if max == 0 {
                            0
                        } else {
                            u64::MAX >> max.leading_zeros()
                        },
                        one: 0,
                    },
                    None => KnownBits::UNKNOWN,
                }
            }
        },
        RuntimeBinOp::Sub if matches!(rhs_operand, RuntimeOperand::Imm(0)) => lhs,
        RuntimeBinOp::Mul => match (lhs_operand, rhs_operand) {
            (_, RuntimeOperand::Imm(0)) | (RuntimeOperand::Imm(0), _) => KnownBits::constant(0),
            (_, RuntimeOperand::Imm(1)) => lhs,
            (RuntimeOperand::Imm(1), _) => rhs,
            _ => KnownBits::UNKNOWN,
        },
        _ => KnownBits::UNKNOWN,
    }
}

fn compute_demanded_bits(ssa: &RuntimeSSAProgram) -> Vec<u64> {
    let mut demanded = vec![0_u64; ssa.values.len()];
    let instruction_by_index: HashMap<usize, &SSAInstruction> = ssa
        .instructions
        .iter()
        .map(|instruction| (instruction.instr_index, instruction))
        .collect();
    let mut runtime_instructions: Vec<(usize, &RuntimeInstr)> = ssa
        .blocks
        .iter()
        .flat_map(|block| block.instr_indices.iter().copied().zip(block.instrs.iter()))
        .collect();
    runtime_instructions.sort_by_key(|(index, _)| *index);

    let limit = ssa.values.len().saturating_mul(4).saturating_add(8);
    for _ in 0..limit {
        let previous = demanded.clone();
        for phi in &ssa.phis {
            let mask = demanded[phi.value];
            for input in &phi.inputs {
                demanded[input.value] |= mask;
            }
        }
        for (instr_index, instr) in runtime_instructions.iter().rev() {
            let Some(ssa_instr) = instruction_by_index.get(instr_index) else {
                continue;
            };
            let output_demand = ssa_instr
                .writes
                .iter()
                .fold(0_u64, |mask, write| mask | demanded[write.value]);
            let mut demand_read = |slot: usize, mask: u64| {
                for usage in ssa_instr.reads.iter().filter(|usage| usage.slot == slot) {
                    demanded[usage.value] |= mask;
                }
            };
            let demand_operand =
                |operand: &RuntimeOperand, mask: u64, demand_read: &mut dyn FnMut(usize, u64)| {
                    if let RuntimeOperand::Slot(slot) = operand {
                        demand_read(*slot, mask);
                    }
                };

            match instr {
                RuntimeInstr::Mov { src, .. } => {
                    demand_operand(src, output_demand, &mut demand_read)
                }
                RuntimeInstr::BinOp { op, lhs, rhs, .. } => {
                    demand_binop_inputs(*op, lhs, rhs, output_demand, &mut demand_read);
                }
                RuntimeInstr::BinOpInPlace { dst, op, rhs } => {
                    demand_binop_inputs(
                        *op,
                        &RuntimeOperand::Slot(*dst),
                        rhs,
                        output_demand,
                        &mut demand_read,
                    );
                }
                RuntimeInstr::NormalizeInt { dst, bits, .. } => {
                    let width = (*bits).min(64) as u32;
                    let mask = if width == 64 {
                        u64::MAX
                    } else if width == 0 {
                        0
                    } else {
                        (1_u64 << width) - 1
                    };
                    demand_read(*dst, output_demand & mask);
                }
                RuntimeInstr::Cmp { lhs, rhs, .. } | RuntimeInstr::FloatBinOp { lhs, rhs, .. } => {
                    if output_demand != 0 {
                        demand_operand(lhs, u64::MAX, &mut demand_read);
                        demand_operand(rhs, u64::MAX, &mut demand_read);
                    }
                }
                RuntimeInstr::LoadIndex {
                    base_slots, index, ..
                }
                | RuntimeInstr::LoadIndexUnchecked {
                    base_slots, index, ..
                } => {
                    if output_demand != 0 {
                        for slot in base_slots {
                            demand_read(*slot, u64::MAX);
                        }
                        demand_operand(index, u64::MAX, &mut demand_read);
                    }
                }
                RuntimeInstr::LoadSeed { .. } | RuntimeInstr::Alloc { .. }
                    if output_demand == 0 => {}
                RuntimeInstr::Jump { .. }
                | RuntimeInstr::Call { .. }
                | RuntimeInstr::Return
                | RuntimeInstr::PrintConst { .. } => {}
                _ => {
                    // Memory operations, control predicates, syscalls and compound
                    // kernels observe every bit of their explicit inputs.
                    for usage in &ssa_instr.reads {
                        demanded[usage.value] |= u64::MAX;
                    }
                }
            }
        }
        if demanded == previous {
            break;
        }
    }
    demanded
}

fn demand_binop_inputs(
    op: crate::frontend::semantics::RuntimeBinOp,
    lhs: &RuntimeOperand,
    rhs: &RuntimeOperand,
    output: u64,
    demand_read: &mut dyn FnMut(usize, u64),
) {
    use crate::frontend::semantics::RuntimeBinOp;
    let demand_operand =
        |operand: &RuntimeOperand, mask: u64, demand_read: &mut dyn FnMut(usize, u64)| {
            if let RuntimeOperand::Slot(slot) = operand {
                demand_read(*slot, mask);
            }
        };
    match op {
        RuntimeBinOp::Add
        | RuntimeBinOp::Sub
        | RuntimeBinOp::Mul
        | RuntimeBinOp::BitOr
        | RuntimeBinOp::BitXor => {
            demand_operand(lhs, output, demand_read);
            demand_operand(rhs, output, demand_read);
        }
        RuntimeBinOp::BitAnd => {
            let lhs_mask = match rhs {
                RuntimeOperand::Imm(mask) => output & *mask,
                RuntimeOperand::Slot(_) => output,
            };
            let rhs_mask = match lhs {
                RuntimeOperand::Imm(mask) => output & *mask,
                RuntimeOperand::Slot(_) => output,
            };
            demand_operand(lhs, lhs_mask, demand_read);
            demand_operand(rhs, rhs_mask, demand_read);
        }
        RuntimeBinOp::Shl => {
            let lhs_mask = match rhs {
                RuntimeOperand::Imm(shift) if *shift < 64 => output >> *shift,
                _ => u64::MAX,
            };
            demand_operand(lhs, lhs_mask, demand_read);
            demand_operand(rhs, u64::MAX, demand_read);
        }
        RuntimeBinOp::ShrUnsigned | RuntimeBinOp::ShrSigned => {
            let lhs_mask = match rhs {
                RuntimeOperand::Imm(shift) if *shift < 64 => output << *shift,
                _ => u64::MAX,
            };
            demand_operand(lhs, lhs_mask, demand_read);
            demand_operand(rhs, u64::MAX, demand_read);
        }
        RuntimeBinOp::DivUnsigned
        | RuntimeBinOp::DivSigned
        | RuntimeBinOp::ModUnsigned
        | RuntimeBinOp::ModSigned => {
            demand_operand(lhs, u64::MAX, demand_read);
            demand_operand(rhs, u64::MAX, demand_read);
        }
    }
}

fn analyze_loops(ssa: &RuntimeSSAProgram) -> Vec<LoopDescriptor> {
    let mut loops = Vec::new();
    for block in &ssa.blocks {
        for &successor in &block.successors {
            if successor > block.id {
                continue;
            }
            let block_ids: Vec<usize> = (successor..=block.id).collect();
            let mut instructions: Vec<(usize, &RuntimeInstr)> = block_ids
                .iter()
                .flat_map(|block_id| {
                    ssa.blocks[*block_id]
                        .instr_indices
                        .iter()
                        .copied()
                        .zip(ssa.blocks[*block_id].instrs.iter())
                })
                .collect();
            instructions.sort_by_key(|(index, _)| *index);
            let mut dependencies = Vec::new();
            for left in 0..instructions.len() {
                let left_reads: HashSet<_> = read_slots(instructions[left].1).into_iter().collect();
                let left_writes: HashSet<_> =
                    write_slots(instructions[left].1).into_iter().collect();
                for right in left + 1..instructions.len() {
                    let right_reads: HashSet<_> =
                        read_slots(instructions[right].1).into_iter().collect();
                    let right_writes: HashSet<_> =
                        write_slots(instructions[right].1).into_iter().collect();
                    if !left_writes.is_disjoint(&right_reads) {
                        dependencies.push(LoopDependency {
                            from_instr: instructions[left].0,
                            to_instr: instructions[right].0,
                            kind: LoopDependencyKind::Flow,
                        });
                    }
                    if !left_reads.is_disjoint(&right_writes) {
                        dependencies.push(LoopDependency {
                            from_instr: instructions[left].0,
                            to_instr: instructions[right].0,
                            kind: LoopDependencyKind::Anti,
                        });
                    }
                    if !left_writes.is_disjoint(&right_writes) {
                        dependencies.push(LoopDependency {
                            from_instr: instructions[left].0,
                            to_instr: instructions[right].0,
                            kind: LoopDependencyKind::Output,
                        });
                    }
                    let left_memory: Vec<_> = ssa
                        .memory_accesses
                        .iter()
                        .filter(|access| access.instr_index == instructions[left].0)
                        .collect();
                    let right_memory: Vec<_> = ssa
                        .memory_accesses
                        .iter()
                        .filter(|access| access.instr_index == instructions[right].0)
                        .collect();
                    if left_memory.iter().any(|left_access| {
                        right_memory.iter().any(|right_access| {
                            left_access.object == right_access.object
                                && (left_access.kind != MemoryAccessKind::Read
                                    || right_access.kind != MemoryAccessKind::Read)
                        })
                    }) {
                        dependencies.push(LoopDependency {
                            from_instr: instructions[left].0,
                            to_instr: instructions[right].0,
                            kind: LoopDependencyKind::Memory,
                        });
                    }
                }
            }
            dependencies.sort_by_key(|dependency| {
                (
                    dependency.from_instr,
                    dependency.to_instr,
                    dependency.kind as u8,
                )
            });
            dependencies.dedup();
            loops.push(LoopDescriptor {
                header: successor,
                latch: block.id,
                blocks: block_ids,
                dependencies,
                recurrences: detect_affine_recurrences(&instructions),
            });
        }
    }
    loops.sort_by_key(|descriptor| (descriptor.header, descriptor.latch));
    loops.dedup_by_key(|descriptor| (descriptor.header, descriptor.latch));
    loops
}

fn detect_affine_recurrences(instructions: &[(usize, &RuntimeInstr)]) -> Vec<AffineRecurrence> {
    let mut state = HashMap::<usize, (Option<u64>, Option<u64>, Option<u64>)>::new();
    for (_, instr) in instructions {
        let (slot, op, rhs) = match instr {
            RuntimeInstr::BinOpInPlace { dst, op, rhs } => (*dst, *op, rhs),
            RuntimeInstr::BinOp {
                dst,
                op,
                lhs: RuntimeOperand::Slot(lhs),
                rhs,
            } if dst == lhs => (*dst, *op, rhs),
            _ => continue,
        };
        let RuntimeOperand::Imm(value) = rhs else {
            continue;
        };
        let entry = state.entry(slot).or_insert((None, None, None));
        match op {
            crate::frontend::semantics::RuntimeBinOp::Mul => entry.0 = Some(*value),
            crate::frontend::semantics::RuntimeBinOp::Add => entry.1 = Some(*value),
            crate::frontend::semantics::RuntimeBinOp::BitAnd => entry.2 = Some(*value),
            _ => {}
        }
    }
    let mut recurrences: Vec<_> = state
        .into_iter()
        .filter_map(|(slot, (mul, add, mask))| {
            Some(AffineRecurrence {
                slot,
                mul: mul?,
                add: add?,
                mask: mask.unwrap_or(u64::MAX),
            })
        })
        .collect();
    recurrences.sort_by_key(|recurrence| recurrence.slot);
    recurrences
}

fn build_memory_ssa(ssa: &mut RuntimeSSAProgram) {
    if ssa.objects.is_empty() {
        return;
    }
    let mut versions = Vec::new();
    let mut entry_versions = Vec::with_capacity(ssa.objects.len());
    for object in &ssa.objects {
        let id = versions.len();
        versions.push(MemoryVersion {
            id,
            object: object.id,
            def: MemoryVersionDef::Entry,
        });
        entry_versions.push(id);
    }

    let mut accesses_by_instr = HashMap::<usize, Vec<(usize, MemoryAccessKind)>>::new();
    let mut write_versions = HashMap::<(usize, usize), usize>::new();
    for block in &ssa.blocks {
        for (&instr_index, instr) in block.instr_indices.iter().zip(&block.instrs) {
            let accesses = memory_accesses_for_instruction(instr, &ssa.objects);
            for &(object, kind) in &accesses {
                if kind != MemoryAccessKind::Read {
                    let id = versions.len();
                    versions.push(MemoryVersion {
                        id,
                        object: ObjectId(object),
                        def: MemoryVersionDef::Instr { instr_index },
                    });
                    write_versions.insert((instr_index, object), id);
                }
            }
            if !accesses.is_empty() {
                accesses_by_instr.insert(instr_index, accesses);
            }
        }
    }

    let mut phi_versions = HashMap::<(usize, usize), usize>::new();
    let mut block_in = vec![entry_versions.clone(); ssa.blocks.len()];
    let mut block_out = vec![entry_versions.clone(); ssa.blocks.len()];
    let limit = ssa
        .blocks
        .len()
        .saturating_mul(ssa.objects.len())
        .saturating_mul(4)
        .saturating_add(8);
    for _ in 0..limit {
        let mut changed = false;
        for block in &ssa.blocks {
            let mut incoming = entry_versions.clone();
            for object in 0..ssa.objects.len() {
                let mut candidates = Vec::new();
                if block.id == ssa.entry_block || block.predecessors.is_empty() {
                    candidates.push(entry_versions[object]);
                }
                for &pred in &block.predecessors {
                    candidates.push(block_out[pred][object]);
                }
                candidates.sort_unstable();
                candidates.dedup();
                incoming[object] = if candidates.len() == 1 {
                    candidates[0]
                } else {
                    *phi_versions.entry((block.id, object)).or_insert_with(|| {
                        let id = versions.len();
                        versions.push(MemoryVersion {
                            id,
                            object: ObjectId(object),
                            def: MemoryVersionDef::Phi { block: block.id },
                        });
                        id
                    })
                };
            }
            let mut outgoing = incoming.clone();
            for &instr_index in &block.instr_indices {
                if let Some(accesses) = accesses_by_instr.get(&instr_index) {
                    for &(object, kind) in accesses {
                        if kind != MemoryAccessKind::Read {
                            outgoing[object] = write_versions[&(instr_index, object)];
                        }
                    }
                }
            }
            if block_in[block.id] != incoming || block_out[block.id] != outgoing {
                block_in[block.id] = incoming;
                block_out[block.id] = outgoing;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut memory_phis = Vec::new();
    let mut phi_entries: Vec<_> = phi_versions.into_iter().collect();
    phi_entries.sort_by_key(|((block, object), _)| (*block, *object));
    for ((block, object), version) in phi_entries {
        let mut inputs = Vec::new();
        if block == ssa.entry_block || ssa.blocks[block].predecessors.is_empty() {
            inputs.push(SSAPhiInput {
                predecessor: None,
                value: entry_versions[object],
            });
        }
        for &pred in &ssa.blocks[block].predecessors {
            inputs.push(SSAPhiInput {
                predecessor: Some(pred),
                value: block_out[pred][object],
            });
        }
        inputs.sort_by_key(|input| (input.predecessor, input.value));
        inputs.dedup();
        memory_phis.push(MemoryPhi {
            block,
            object: ObjectId(object),
            version,
            inputs,
        });
    }

    let mut memory_accesses = Vec::new();
    for block in &ssa.blocks {
        let mut current = block_in[block.id].clone();
        for &instr_index in &block.instr_indices {
            if let Some(accesses) = accesses_by_instr.get(&instr_index) {
                for &(object, kind) in accesses {
                    let incoming_version = current[object];
                    let outgoing_version = if kind == MemoryAccessKind::Read {
                        incoming_version
                    } else {
                        write_versions[&(instr_index, object)]
                    };
                    current[object] = outgoing_version;
                    memory_accesses.push(MemoryAccess {
                        instr_index,
                        object: ObjectId(object),
                        kind,
                        incoming_version,
                        outgoing_version,
                    });
                }
            }
        }
    }
    memory_accesses.sort_by_key(|access| (access.instr_index, access.object.0));
    ssa.memory_versions = versions;
    ssa.memory_phis = memory_phis;
    ssa.memory_accesses = memory_accesses;
}

fn memory_accesses_for_instruction(
    instr: &RuntimeInstr,
    objects: &[MemoryObject],
) -> Vec<(usize, MemoryAccessKind)> {
    let (slots, kind): (&[usize], MemoryAccessKind) = match instr {
        RuntimeInstr::LoadIndex { base_slots, .. }
        | RuntimeInstr::LoadIndexUnchecked { base_slots, .. }
        | RuntimeInstr::BloomSplitBlockCheck {
            filter_slots: base_slots,
            ..
        }
        | RuntimeInstr::BloomClassic4Check {
            filter_slots: base_slots,
            ..
        }
        | RuntimeInstr::HashCtrlGroupProbe {
            ctrl_slots: base_slots,
            ..
        } => (base_slots, MemoryAccessKind::Read),
        RuntimeInstr::StoreIndex { base_slots, .. }
        | RuntimeInstr::StoreIndexUnchecked { base_slots, .. } => {
            (base_slots, MemoryAccessKind::Write)
        }
        RuntimeInstr::BloomSplitBlockInsert {
            filter_slots: base_slots,
            ..
        } => (base_slots, MemoryAccessKind::ReadWrite),
        RuntimeInstr::RadixSortFixedInt { slots, .. } => (slots, MemoryAccessKind::ReadWrite),
        _ => return Vec::new(),
    };
    let touched: HashSet<usize> = slots.iter().copied().collect();
    objects
        .iter()
        .filter(|object| object.slots.iter().any(|slot| touched.contains(slot)))
        .map(|object| (object.id.0, kind))
        .collect()
}

impl MachineLIRProgram {
    pub fn lower(
        program: &RuntimeProgram,
        profile: Option<&FunctionProfile>,
    ) -> Result<Self, String> {
        verify_runtime_program(program)?;
        let ssa = RuntimeSSAProgram::lower(program);
        ssa.verify(program.instrs.len())?;

        let loop_depths = compute_loop_depths(&ssa.blocks);
        let loops = analyze_loops(&ssa);
        let mut blocks = Vec::with_capacity(ssa.blocks.len());
        for block in &ssa.blocks {
            let static_freq = 1_u64 << loop_depths[block.id].min(8);
            let frequency = profile
                .and_then(|function_profile| function_profile.block_exec_count(block.id))
                .unwrap_or(static_freq);
            blocks.push(MachineLIRBlock {
                id: block.id,
                instrs: block.instrs.clone(),
                instr_indices: block.instr_indices.clone(),
                predecessors: block.predecessors.clone(),
                successors: block.successors.clone(),
                frequency: frequency.max(1),
                loop_depth: loop_depths[block.id],
            });
        }

        Ok(Self {
            slot_count: ssa.slot_count,
            entry_block: ssa.entry_block,
            blocks,
            objects: ssa.objects,
            values: ssa.values,
            instructions: ssa.instructions,
            phis: ssa.phis,
            known_bits: ssa.known_bits,
            demanded_bits: ssa.demanded_bits,
            memory_versions: ssa.memory_versions,
            memory_phis: ssa.memory_phis,
            memory_accesses: ssa.memory_accesses,
            call_sites: ssa.call_sites,
            loops,
        })
    }

    pub fn verify(&self, expected_instr_count: usize) -> Result<(), String> {
        let ssa = RuntimeSSAProgram {
            slot_count: self.slot_count,
            entry_block: self.entry_block,
            blocks: self
                .blocks
                .iter()
                .map(|block| RuntimeSSABlock {
                    id: block.id,
                    params: Vec::new(),
                    instrs: block.instrs.clone(),
                    instr_indices: block.instr_indices.clone(),
                    predecessors: block.predecessors.clone(),
                    successors: block.successors.clone(),
                })
                .collect(),
            objects: self.objects.clone(),
            values: self.values.clone(),
            instructions: self.instructions.clone(),
            phis: self.phis.clone(),
            known_bits: self.known_bits.clone(),
            demanded_bits: self.demanded_bits.clone(),
            memory_versions: self.memory_versions.clone(),
            memory_phis: self.memory_phis.clone(),
            memory_accesses: self.memory_accesses.clone(),
            call_sites: self.call_sites.clone(),
        };
        ssa.verify(expected_instr_count)
    }

    pub fn dump(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "MachineLIR slot_count={} entry_block={}\n",
            self.slot_count, self.entry_block
        ));
        if !self.objects.is_empty() {
            out.push_str("objects:\n");
            for object in &self.objects {
                out.push_str(&format!(
                    "  object {} slots={:?} alias={:?} escape={:?} repr={:?} borrow={:?}\n",
                    object.id.0,
                    object.slots,
                    object.alias_class,
                    object.escape_class,
                    object.representation,
                    object.borrow_region.map(|region| region.0)
                ));
            }
        }
        if !self.phis.is_empty() {
            out.push_str("ssa-phis:\n");
            for phi in &self.phis {
                out.push_str(&format!(
                    "  v{} = phi slot{} block{} {:?}\n",
                    phi.value, phi.slot, phi.block, phi.inputs
                ));
            }
        }
        out.push_str("ssa-values:\n");
        for value in &self.values {
            let known = self.known_bits[value.id];
            out.push_str(&format!(
                "  v{} slot{} def={:?} known_zero={:#018x} known_one={:#018x} range={:#x}..={:#x} width={} demanded={:#018x}\n",
                value.id,
                value.slot,
                value.def,
                known.zero,
                known.one,
                known.unsigned_min(),
                known.unsigned_max(),
                known.width(),
                self.demanded_bits[value.id]
            ));
        }
        if !self.memory_accesses.is_empty() {
            out.push_str("memory-ssa:\n");
            for access in &self.memory_accesses {
                out.push_str(&format!(
                    "  i{} object{} {:?} m{} -> m{}\n",
                    access.instr_index,
                    access.object.0,
                    access.kind,
                    access.incoming_version,
                    access.outgoing_version
                ));
            }
        }
        if !self.call_sites.is_empty() {
            out.push_str("internal-abi:\n");
            for call in &self.call_sites {
                out.push_str(&format!(
                    "  call i{} -> i{} cont=i{} tail={} leaf={} live={:?} reads={:?} writes={:?}\n",
                    call.instr_index,
                    call.target,
                    call.continuation,
                    call.tail,
                    call.callee_leaf,
                    call.live_across,
                    call.callee_reads,
                    call.callee_writes
                ));
            }
        }
        if !self.loops.is_empty() {
            out.push_str("loop-plans:\n");
            for loop_plan in &self.loops {
                out.push_str(&format!(
                    "  header={} latch={} blocks={:?} deps={} recurrences={:?}\n",
                    loop_plan.header,
                    loop_plan.latch,
                    loop_plan.blocks,
                    loop_plan.dependencies.len(),
                    loop_plan.recurrences
                ));
            }
        }
        for block in &self.blocks {
            out.push_str(&format!(
                "block {} freq={} loop_depth={} preds={:?} succs={:?}\n",
                block.id, block.frequency, block.loop_depth, block.predecessors, block.successors
            ));
            for (instr_index, instr) in block.instr_indices.iter().zip(block.instrs.iter()) {
                out.push_str(&format!("  i{}: {:?}\n", instr_index, instr));
            }
        }
        out
    }

    pub fn demanded_width_for_instruction(&self, instr_index: usize) -> u8 {
        let demanded = self
            .instructions
            .iter()
            .find(|instruction| instruction.instr_index == instr_index)
            .map(|instruction| {
                instruction
                    .writes
                    .iter()
                    .fold(0_u64, |mask, write| mask | self.demanded_bits[write.value])
            })
            .unwrap_or(u64::MAX);
        if demanded == 0 {
            1
        } else {
            (u64::BITS - demanded.leading_zeros()) as u8
        }
    }

    pub fn build_profile_template(&self, function_name: &str) -> CompileProfile {
        let mut function = FunctionProfile {
            name: function_name.to_string(),
            blocks: HashMap::new(),
        };
        for block in &self.blocks {
            let entry = function.blocks.entry(block.id).or_default();
            entry.exec_count = block.frequency;
            for &succ in &block.successors {
                let edge_hint = self
                    .blocks
                    .get(succ)
                    .map(|target| target.frequency.min(block.frequency))
                    .unwrap_or(1)
                    .max(1);
                entry.edge_counts.insert(succ, edge_hint);
            }
        }
        let mut profile = CompileProfile::default();
        profile
            .functions
            .insert(function_name.to_string(), function);
        profile
    }

    pub fn compute_live_intervals(&self) -> Vec<LiveInterval> {
        let mut block_use = vec![HashSet::new(); self.blocks.len()];
        let mut block_def = vec![HashSet::new(); self.blocks.len()];
        for block in &self.blocks {
            let mut seen_def = HashSet::new();
            for instr in &block.instrs {
                for read_slot in read_slots(instr) {
                    if !seen_def.contains(&read_slot) {
                        block_use[block.id].insert(read_slot);
                    }
                }
                for write_slot in write_slots(instr) {
                    seen_def.insert(write_slot);
                    block_def[block.id].insert(write_slot);
                }
            }
        }

        let mut live_in = vec![HashSet::new(); self.blocks.len()];
        let mut live_out = vec![HashSet::new(); self.blocks.len()];
        let mut changed = true;
        while changed {
            changed = false;
            for block_id in (0..self.blocks.len()).rev() {
                let mut out = HashSet::new();
                for &succ in &self.blocks[block_id].successors {
                    out.extend(live_in[succ].iter().copied());
                }

                let mut incoming = block_use[block_id].clone();
                for slot in &out {
                    if !block_def[block_id].contains(slot) {
                        incoming.insert(*slot);
                    }
                }

                if out != live_out[block_id] || incoming != live_in[block_id] {
                    live_out[block_id] = out;
                    live_in[block_id] = incoming;
                    changed = true;
                }
            }
        }

        let mut positions = HashMap::new();
        let mut next_pos = 0usize;
        for block in &self.blocks {
            for &instr_index in &block.instr_indices {
                positions.insert(instr_index, next_pos);
                next_pos += 1;
            }
        }

        let mut segment_ranges = vec![Vec::<LiveSegment>::new(); self.slot_count];
        let mut weights = vec![0u64; self.slot_count];
        let force_stack = compute_force_stack(self.slot_count, self);

        for block in &self.blocks {
            let Some(&block_start_pos) = block
                .instr_indices
                .first()
                .and_then(|instr_index| positions.get(instr_index))
            else {
                continue;
            };
            let Some(&block_end_pos) = block
                .instr_indices
                .last()
                .and_then(|instr_index| positions.get(instr_index))
            else {
                continue;
            };

            let mut block_first = vec![usize::MAX; self.slot_count];
            let mut block_last = vec![0usize; self.slot_count];
            for &slot in &live_in[block.id] {
                block_first[slot] = block_start_pos;
                block_last[slot] = block_start_pos;
                weights[slot] = weights[slot].saturating_add(block.frequency);
            }

            for (instr_index, instr) in block.instr_indices.iter().zip(block.instrs.iter()) {
                let pos = positions[instr_index];
                let mut touched = HashSet::new();
                for slot in read_slots(instr)
                    .into_iter()
                    .chain(write_slots(instr).into_iter())
                {
                    block_first[slot] = block_first[slot].min(pos);
                    block_last[slot] = block_last[slot].max(pos);
                    if touched.insert(slot) {
                        weights[slot] = weights[slot].saturating_add(block.frequency);
                    }
                }
            }

            for &slot in &live_out[block.id] {
                block_first[slot] = block_first[slot].min(block_start_pos);
                block_last[slot] = block_end_pos;
                weights[slot] = weights[slot].saturating_add(block.frequency);
            }
            for slot in 0..self.slot_count {
                if block_first[slot] != usize::MAX {
                    segment_ranges[slot].push(LiveSegment {
                        start: block_first[slot],
                        end: block_last[slot].max(block_first[slot]),
                    });
                }
            }
        }

        // The internal ABI exchanges only slots proven live across a call.
        // Bias those values toward registers so direct calls need no generic
        // caller-save packet or stack argument area.
        for call in &self.call_sites {
            let frequency = self
                .blocks
                .iter()
                .find(|block| block.instr_indices.contains(&call.instr_index))
                .map(|block| block.frequency)
                .unwrap_or(1);
            for &slot in &call.live_across {
                weights[slot] = weights[slot].saturating_add(frequency.saturating_mul(2));
            }
        }

        let rematerializable = compute_rematerializable_slots(self.slot_count, self);
        let copy_hints = compute_copy_hints(self.slot_count, self);
        let mut intervals = Vec::new();
        for slot in 0..self.slot_count {
            if segment_ranges[slot].is_empty() {
                continue;
            }
            let mut segments = std::mem::take(&mut segment_ranges[slot]);
            segments.sort_by_key(|segment| (segment.start, segment.end));
            let mut merged = Vec::<LiveSegment>::new();
            for segment in segments {
                if let Some(last) = merged.last_mut() {
                    if segment.start <= last.end.saturating_add(1) {
                        last.end = last.end.max(segment.end);
                        continue;
                    }
                }
                merged.push(segment);
            }
            let start = merged.first().map(|segment| segment.start).unwrap_or(0);
            let end = merged.last().map(|segment| segment.end).unwrap_or(start);
            intervals.push(LiveInterval {
                slot,
                start,
                end,
                segments: merged,
                spill_weight: weights[slot],
                force_stack: force_stack[slot],
                rematerializable: rematerializable[slot],
                copy_hint: copy_hints[slot],
            });
        }
        intervals.sort_by_key(|interval| (interval.start, interval.end, interval.slot));
        intervals
    }
}

fn verify_runtime_program(program: &RuntimeProgram) -> Result<(), String> {
    for (instr_index, instr) in program.instrs.iter().enumerate() {
        for slot in read_slots(instr)
            .into_iter()
            .chain(write_slots(instr).into_iter())
        {
            if slot >= program.slots {
                return Err(format!(
                    "runtime instruction {instr_index} references slot {slot}, but slot count is {}",
                    program.slots
                ));
            }
        }

        let target = match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => Some(*target),
            _ => None,
        };
        if let Some(target) = target {
            if target > program.instrs.len() {
                return Err(format!(
                    "runtime instruction {instr_index} targets instruction {target}, but instruction count is {}",
                    program.instrs.len()
                ));
            }
        }
    }
    Ok(())
}

fn build_block_graph(program: &RuntimeProgram) -> (Vec<usize>, Vec<Vec<usize>>) {
    if program.instrs.is_empty() {
        return (vec![0], vec![Vec::new()]);
    }

    let mut starts = BTreeSet::new();
    starts.insert(0usize);
    for (idx, instr) in program.instrs.iter().enumerate() {
        match instr {
            RuntimeInstr::Jump { target }
            | RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. }
            | RuntimeInstr::Call { target } => {
                if *target < program.instrs.len() {
                    starts.insert(*target);
                }
            }
            _ => {}
        }
        if matches!(
            instr,
            RuntimeInstr::Jump { .. }
                | RuntimeInstr::JumpIfZero { .. }
                | RuntimeInstr::JumpIfCmpFalse { .. }
                | RuntimeInstr::Call { .. }
                | RuntimeInstr::Return
                | RuntimeInstr::Exit { .. }
        ) && idx + 1 < program.instrs.len()
        {
            starts.insert(idx + 1);
        }
    }

    let starts: Vec<usize> = starts.into_iter().collect();
    let mut instr_to_block = vec![0usize; program.instrs.len()];
    for (block_id, start) in starts.iter().copied().enumerate() {
        let end = starts
            .get(block_id + 1)
            .copied()
            .unwrap_or(program.instrs.len());
        for instr_index in start..end {
            instr_to_block[instr_index] = block_id;
        }
    }

    let call_continuations: Vec<usize> = program
        .instrs
        .iter()
        .enumerate()
        .filter_map(|(index, instr)| {
            matches!(instr, RuntimeInstr::Call { .. })
                .then_some(index + 1)
                .filter(|continuation| *continuation < instr_to_block.len())
                .map(|continuation| instr_to_block[continuation])
        })
        .collect();
    let mut successors = vec![Vec::new(); starts.len()];
    for (block_id, start) in starts.iter().copied().enumerate() {
        let end = starts
            .get(block_id + 1)
            .copied()
            .unwrap_or(program.instrs.len());
        if end == start {
            continue;
        }
        let last_instr_index = end - 1;
        let last_instr = &program.instrs[last_instr_index];
        match last_instr {
            RuntimeInstr::Jump { target } => {
                if *target < instr_to_block.len() {
                    successors[block_id].push(instr_to_block[*target]);
                }
            }
            RuntimeInstr::Call { target } => {
                if *target < instr_to_block.len() {
                    successors[block_id].push(instr_to_block[*target]);
                }
                if end < instr_to_block.len() {
                    successors[block_id].push(instr_to_block[end]);
                }
            }
            RuntimeInstr::JumpIfZero { target, .. }
            | RuntimeInstr::JumpIfCmpFalse { target, .. } => {
                if *target < instr_to_block.len() {
                    successors[block_id].push(instr_to_block[*target]);
                }
                if end < instr_to_block.len() {
                    successors[block_id].push(instr_to_block[end]);
                }
            }
            RuntimeInstr::Return => {
                successors[block_id].extend(call_continuations.iter().copied());
            }
            RuntimeInstr::Exit { .. } => {}
            _ => {
                if end < instr_to_block.len() {
                    successors[block_id].push(instr_to_block[end]);
                }
            }
        }
        successors[block_id].sort_unstable();
        successors[block_id].dedup();
    }

    (starts, successors)
}

fn invert_successors(block_count: usize, successors: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut predecessors = vec![Vec::new(); block_count];
    for (block_id, succs) in successors.iter().enumerate() {
        for &succ in succs {
            predecessors[succ].push(block_id);
        }
    }
    for preds in &mut predecessors {
        preds.sort_unstable();
        preds.dedup();
    }
    predecessors
}

fn analyze_call_sites(program: &RuntimeProgram) -> Vec<CallSiteABI> {
    let mut call_sites = Vec::new();
    for (instr_index, instr) in program.instrs.iter().enumerate() {
        let RuntimeInstr::Call { target } = instr else {
            continue;
        };
        let continuation = instr_index + 1;
        let reachable = reachable_instruction_indices(program, *target);
        let mut callee_reads = BTreeSet::new();
        let mut callee_writes = BTreeSet::new();
        let mut callee_leaf = true;
        for index in &reachable {
            let candidate = &program.instrs[*index];
            callee_reads.extend(read_slots(candidate));
            callee_writes.extend(write_slots(candidate));
            callee_leaf &= !matches!(candidate, RuntimeInstr::Call { .. });
        }
        let live_across = (0..program.slots)
            .filter(|slot| slot_read_before_overwrite_on_any_path(program, continuation, *slot))
            .collect();
        let tail = continuation < program.instrs.len()
            && matches!(program.instrs[continuation], RuntimeInstr::Return);
        call_sites.push(CallSiteABI {
            instr_index,
            target: *target,
            continuation,
            tail,
            callee_leaf,
            live_across,
            callee_reads: callee_reads.into_iter().collect(),
            callee_writes: callee_writes.into_iter().collect(),
        });
    }
    call_sites
}

fn reachable_instruction_indices(program: &RuntimeProgram, start: usize) -> BTreeSet<usize> {
    let mut reachable = BTreeSet::new();
    let mut pending = vec![start];
    while let Some(index) = pending.pop() {
        if index >= program.instrs.len() || !reachable.insert(index) {
            continue;
        }
        match &program.instrs[index] {
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
    reachable
}

fn slot_read_before_overwrite_on_any_path(
    program: &RuntimeProgram,
    start: usize,
    slot: usize,
) -> bool {
    let mut pending = vec![start];
    let mut visited = HashSet::new();
    while let Some(index) = pending.pop() {
        if index >= program.instrs.len() || !visited.insert(index) {
            continue;
        }
        let instr = &program.instrs[index];
        if read_slots(instr).contains(&slot) {
            return true;
        }
        if write_slots(instr).contains(&slot) {
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

fn compute_loop_depths(blocks: &[RuntimeSSABlock]) -> Vec<usize> {
    let mut depths = vec![0usize; blocks.len()];
    for block in blocks {
        for &succ in &block.successors {
            if succ <= block.id {
                for depth in depths.iter_mut().take(block.id + 1).skip(succ) {
                    *depth = depth.saturating_add(1);
                }
            }
        }
    }
    depths
}

fn infer_memory_objects(program: &RuntimeProgram) -> Vec<MemoryObject> {
    let mut object_slots = Vec::<Vec<usize>>::new();
    for instr in &program.instrs {
        let slots: Option<&[usize]> = match instr {
            RuntimeInstr::LoadIndex { base_slots, .. }
            | RuntimeInstr::LoadIndexUnchecked { base_slots, .. }
            | RuntimeInstr::StoreIndex { base_slots, .. }
            | RuntimeInstr::StoreIndexUnchecked { base_slots, .. } => Some(base_slots),
            RuntimeInstr::BloomSplitBlockInsert { filter_slots, .. }
            | RuntimeInstr::BloomSplitBlockCheck { filter_slots, .. } => Some(filter_slots),
            RuntimeInstr::HashCtrlGroupProbe { ctrl_slots, .. } => Some(ctrl_slots),
            _ => None,
        };
        if let Some(slots) = slots {
            let mut normalized = slots.to_vec();
            normalized.sort_unstable();
            normalized.dedup();
            if !normalized.is_empty()
                && !object_slots.iter().any(|existing| existing == &normalized)
            {
                object_slots.push(normalized);
            }
        }
    }

    let mut slot_membership = HashMap::<usize, usize>::new();
    for slots in &object_slots {
        for &slot in slots {
            *slot_membership.entry(slot).or_insert(0) += 1;
        }
    }

    object_slots
        .into_iter()
        .enumerate()
        .map(|(index, slots)| {
            let unique = slots
                .iter()
                .all(|slot| slot_membership.get(slot).copied().unwrap_or(0) == 1);
            let mut written = false;
            let mut dynamic_index = false;
            let mut bitset = false;
            for instr in &program.instrs {
                let (access_slots, writes, dynamic, is_bitset): (
                    Option<&[usize]>,
                    bool,
                    bool,
                    bool,
                ) = match instr {
                    RuntimeInstr::LoadIndex {
                        base_slots, index, ..
                    }
                    | RuntimeInstr::LoadIndexUnchecked {
                        base_slots, index, ..
                    } => (
                        Some(base_slots),
                        false,
                        runtime_const_index_for_access(base_slots, index).is_none(),
                        false,
                    ),
                    RuntimeInstr::StoreIndex {
                        base_slots, index, ..
                    }
                    | RuntimeInstr::StoreIndexUnchecked {
                        base_slots, index, ..
                    } => (
                        Some(base_slots),
                        true,
                        runtime_const_index_for_access(base_slots, index).is_none(),
                        false,
                    ),
                    RuntimeInstr::BloomSplitBlockInsert { filter_slots, .. } => {
                        (Some(filter_slots), true, true, true)
                    }
                    RuntimeInstr::BloomSplitBlockCheck { filter_slots, .. }
                    | RuntimeInstr::BloomClassic4Check { filter_slots, .. } => {
                        (Some(filter_slots), false, true, true)
                    }
                    RuntimeInstr::HashCtrlGroupProbe { ctrl_slots, .. } => {
                        (Some(ctrl_slots), false, true, false)
                    }
                    RuntimeInstr::RadixSortFixedInt {
                        slots: radix_slots, ..
                    } => (Some(radix_slots), true, true, false),
                    _ => (None, false, false, false),
                };
                if access_slots.is_some_and(|access| {
                    access.iter().any(|slot| slots.binary_search(slot).is_ok())
                }) {
                    written |= writes;
                    dynamic_index |= dynamic;
                    bitset |= is_bitset;
                }
            }
            let alias_class = if !written {
                AliasClass::ReadOnlyShared
            } else if unique {
                AliasClass::Unique
            } else {
                AliasClass::Conservative
            };
            let representation = if bitset {
                MemoryRepresentation::PackedBitset
            } else if dynamic_index {
                MemoryRepresentation::ContiguousStack
            } else {
                MemoryRepresentation::Scalarized
            };
            MemoryObject {
                id: ObjectId(index),
                slots,
                alias_class,
                escape_class: EscapeClass::NoEscape,
                borrow_region: Some(BorrowRegion(index)),
                representation,
            }
        })
        .collect()
}

fn refine_memory_representations(program: &RuntimeProgram, ssa: &mut RuntimeSSAProgram) {
    let instruction_by_index: HashMap<usize, &SSAInstruction> = ssa
        .instructions
        .iter()
        .map(|instruction| (instruction.instr_index, instruction))
        .collect();
    let facts = &ssa.known_bits;
    let fits_byte = |fact: KnownBits| fact.zero & !0xff == !0xff;

    // Loop-header phis can conservatively forget a range even when every
    // dynamic value comes from one static, dominating definition.  Recover
    // that fact only when both uniqueness and dominance are proven.
    let mut instr_block = vec![0usize; program.instrs.len()];
    for block in &ssa.blocks {
        for &instr_index in &block.instr_indices {
            instr_block[instr_index] = block.id;
        }
    }
    let block_count = ssa.blocks.len();
    let mut dominators = vec![vec![true; block_count]; block_count];
    if block_count != 0 {
        dominators[ssa.entry_block].fill(false);
        dominators[ssa.entry_block][ssa.entry_block] = true;
        let mut changed = true;
        while changed {
            changed = false;
            for block in &ssa.blocks {
                if block.id == ssa.entry_block {
                    continue;
                }
                let mut next = vec![true; block_count];
                if block.predecessors.is_empty() {
                    next.fill(false);
                } else {
                    for &predecessor in &block.predecessors {
                        for (candidate, is_dominator) in next.iter_mut().enumerate() {
                            *is_dominator &= dominators[predecessor][candidate];
                        }
                    }
                }
                next[block.id] = true;
                if next != dominators[block.id] {
                    dominators[block.id] = next;
                    changed = true;
                }
            }
        }
    }

    let mut unique_definition = vec![None::<(usize, KnownBits)>; ssa.slot_count];
    let mut multiply_defined = vec![false; ssa.slot_count];
    for instruction in &ssa.instructions {
        for write in &instruction.writes {
            let fact = facts
                .get(write.value)
                .copied()
                .unwrap_or(KnownBits::UNKNOWN);
            if unique_definition[write.slot].is_some() {
                multiply_defined[write.slot] = true;
            } else {
                unique_definition[write.slot] = Some((instruction.instr_index, fact));
            }
        }
    }
    let dominates_instruction = |definition: usize, usage: usize| {
        let definition_block = instr_block[definition];
        let usage_block = instr_block[usage];
        if definition_block == usage_block {
            definition <= usage
        } else {
            dominators[usage_block][definition_block]
        }
    };

    for object in &mut ssa.objects {
        if object.representation != MemoryRepresentation::ContiguousStack
            || object.alias_class == AliasClass::Conservative
            || object.escape_class != EscapeClass::NoEscape
        {
            continue;
        }
        let mut legal = true;
        for (index, instr) in program.instrs.iter().enumerate() {
            let Some(ssa_instr) = instruction_by_index.get(&index).copied() else {
                legal = false;
                break;
            };
            let operand_fact = |operand: &RuntimeOperand| match operand {
                RuntimeOperand::Imm(value) => KnownBits::constant(*value),
                RuntimeOperand::Slot(slot) => {
                    let local = ssa_instr
                        .reads
                        .iter()
                        .find(|usage| usage.slot == *slot)
                        .and_then(|usage| facts.get(usage.value))
                        .copied()
                        .unwrap_or(KnownBits::UNKNOWN);
                    if fits_byte(local) || multiply_defined[*slot] {
                        local
                    } else if let Some((definition, definition_fact)) = unique_definition[*slot] {
                        if dominates_instruction(definition, index) {
                            definition_fact
                        } else {
                            local
                        }
                    } else {
                        local
                    }
                }
            };
            let direct_writes_object = write_slots(instr)
                .into_iter()
                .any(|slot| object.slots.binary_search(&slot).is_ok());
            if !direct_writes_object {
                continue;
            }

            let write_is_byte = match instr {
                RuntimeInstr::Mov { dst, src } if object.slots.binary_search(dst).is_ok() => {
                    fits_byte(operand_fact(src))
                }
                RuntimeInstr::StoreIndex {
                    base_slots, src, ..
                }
                | RuntimeInstr::StoreIndexUnchecked {
                    base_slots, src, ..
                } if base_slots == &object.slots => fits_byte(operand_fact(src)),
                _ => false,
            };
            if !write_is_byte {
                legal = false;
                break;
            }
        }
        if legal {
            object.representation = MemoryRepresentation::PackedBytes;
        }
    }
}

fn compute_force_stack(slot_count: usize, lir: &MachineLIRProgram) -> Vec<bool> {
    let mut force_stack = vec![false; slot_count];
    for object in &lir.objects {
        if object.representation != MemoryRepresentation::Scalarized {
            for &slot in &object.slots {
                if slot < force_stack.len() {
                    force_stack[slot] = true;
                }
            }
        }
    }
    for block in &lir.blocks {
        for instr in &block.instrs {
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
                            if slot < force_stack.len() {
                                force_stack[slot] = true;
                            }
                        }
                    }
                }
                RuntimeInstr::BloomSplitBlockInsert { filter_slots, .. }
                | RuntimeInstr::BloomSplitBlockCheck { filter_slots, .. } => {
                    for &slot in filter_slots {
                        if slot < force_stack.len() {
                            force_stack[slot] = true;
                        }
                    }
                }
                RuntimeInstr::HashCtrlGroupProbe { ctrl_slots, .. }
                | RuntimeInstr::BloomClassic4Check {
                    filter_slots: ctrl_slots,
                    ..
                } => {
                    for &slot in ctrl_slots {
                        if slot < force_stack.len() {
                            force_stack[slot] = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    force_stack
}

fn compute_rematerializable_slots(slot_count: usize, lir: &MachineLIRProgram) -> Vec<Option<u64>> {
    let mut candidate = vec![None; slot_count];
    let mut seen_write = vec![false; slot_count];
    let mut invalid = vec![false; slot_count];
    for block in &lir.blocks {
        for instr in &block.instrs {
            let writes = write_slots(instr);
            for slot in writes {
                seen_write[slot] = true;
                let value = match instr {
                    RuntimeInstr::Mov {
                        dst,
                        src: RuntimeOperand::Imm(value),
                    } if *dst == slot => Some(*value),
                    _ => None,
                };
                match (candidate[slot], value) {
                    (None, Some(value)) if !invalid[slot] => candidate[slot] = Some(value),
                    (Some(previous), Some(value)) if previous == value => {}
                    _ => invalid[slot] = true,
                }
            }
        }
    }
    for instruction in &lir.instructions {
        for usage in &instruction.reads {
            if matches!(
                lir.values.get(usage.value).map(|value| value.def),
                Some(SSAValueDef::Entry)
            ) {
                invalid[usage.slot] = true;
            }
        }
    }
    for slot in 0..slot_count {
        if !seen_write[slot] || invalid[slot] {
            candidate[slot] = None;
        }
    }
    candidate
}

fn compute_copy_hints(slot_count: usize, lir: &MachineLIRProgram) -> Vec<Option<usize>> {
    let mut weights = vec![HashMap::<usize, u64>::new(); slot_count];
    for block in &lir.blocks {
        for instr in &block.instrs {
            let copy = match instr {
                RuntimeInstr::Mov {
                    dst,
                    src: RuntimeOperand::Slot(src),
                } => Some((*dst, *src)),
                // x86 integer arithmetic is predominantly two-address.  When
                // the LHS dies at this definition, assigning the result to the
                // same register removes the otherwise mandatory setup copy.
                RuntimeInstr::BinOp {
                    dst,
                    lhs: RuntimeOperand::Slot(src),
                    ..
                } => Some((*dst, *src)),
                _ => None,
            };
            if let Some((dst, src)) = copy
                && dst != src
            {
                let entry = weights[dst].entry(src).or_default();
                *entry = entry.saturating_add(block.frequency);
            }
        }
    }
    weights
        .into_iter()
        .map(|hints| {
            hints
                .into_iter()
                .max_by_key(|(source, weight)| (*weight, std::cmp::Reverse(*source)))
                .map(|(source, _)| source)
        })
        .collect()
}

fn runtime_const_index_for_access(base_slots: &[usize], index: &RuntimeOperand) -> Option<usize> {
    let RuntimeOperand::Imm(value) = index else {
        return None;
    };
    let idx = usize::try_from(*value).ok()?;
    (idx < base_slots.len()).then_some(idx)
}

pub(crate) fn read_slots(instr: &RuntimeInstr) -> Vec<usize> {
    match instr {
        // Platform-load inputs are pinned by the backend's explicit syscall
        // liveness pass; treating a call-ABI resource input as an SSA edge here
        // would create a cross-call value cycle.
        RuntimeInstr::LoadSeed { .. } | RuntimeInstr::Jump { .. } | RuntimeInstr::Call { .. } => {
            Vec::new()
        }
        RuntimeInstr::ThreadSpawn { return_slot, .. } => return_slot.iter().copied().collect(),
        RuntimeInstr::Mov { src, .. } => operand_slots(src),
        RuntimeInstr::BinOp { lhs, rhs, .. }
        | RuntimeInstr::FloatBinOp { lhs, rhs, .. }
        | RuntimeInstr::Cmp { lhs, rhs, .. }
        | RuntimeInstr::JumpIfCmpFalse { lhs, rhs, .. } => {
            let mut out = operand_slots(lhs);
            out.extend(operand_slots(rhs));
            out
        }
        RuntimeInstr::BinOpInPlace { dst, rhs, .. } => {
            let mut out = vec![*dst];
            out.extend(operand_slots(rhs));
            out
        }
        RuntimeInstr::NormalizeInt { dst, .. }
        | RuntimeInstr::JumpIfZero { cond_slot: dst, .. } => {
            vec![*dst]
        }
        RuntimeInstr::CompareSwap { left, right, .. } => vec![*left, *right],
        RuntimeInstr::RadixSortFixedInt { slots, .. } => slots.clone(),
        RuntimeInstr::LoadIndex {
            base_slots, index, ..
        }
        | RuntimeInstr::LoadIndexUnchecked {
            base_slots, index, ..
        } => {
            let mut out = base_slots.clone();
            out.extend(operand_slots(index));
            out
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
            let mut out = base_slots.clone();
            out.extend(operand_slots(index));
            out.extend(operand_slots(src));
            out
        }
        RuntimeInstr::HeapLoadInt { ptr, index, .. } => {
            let mut out = operand_slots(ptr);
            out.extend(operand_slots(index));
            out
        }
        RuntimeInstr::HeapStoreInt {
            ptr, index, src, ..
        } => {
            let mut out = operand_slots(ptr);
            out.extend(operand_slots(index));
            out.extend(operand_slots(src));
            out
        }
        RuntimeInstr::HeapCopy {
            dst_ptr,
            src_ptr,
            bytes,
        } => {
            let mut out = operand_slots(dst_ptr);
            out.extend(operand_slots(src_ptr));
            out.extend(operand_slots(bytes));
            out
        }
        RuntimeInstr::BloomSplitBlockInsert { filter_slots, hash } => {
            let mut out = filter_slots.clone();
            out.extend(operand_slots(hash));
            out
        }
        RuntimeInstr::BloomSplitBlockCheck {
            filter_slots, hash, ..
        }
        | RuntimeInstr::BloomClassic4Check {
            filter_slots, hash, ..
        } => {
            let mut out = filter_slots.clone();
            out.extend(operand_slots(hash));
            out
        }
        RuntimeInstr::HashCtrlGroupProbe {
            ctrl_slots,
            group_start,
            fingerprint,
            ..
        } => {
            let mut out = ctrl_slots.clone();
            out.extend(operand_slots(group_start));
            out.extend(operand_slots(fingerprint));
            out
        }
        RuntimeInstr::JoinSelectAdaptive {
            build_rows,
            probe_rows,
            ..
        } => {
            let mut out = operand_slots(build_rows);
            out.extend(operand_slots(probe_rows));
            out
        }
        RuntimeInstr::Alloc { size, .. } => operand_slots(size),
        RuntimeInstr::Free { ptr, size } => {
            let mut out = operand_slots(ptr);
            out.extend(operand_slots(size));
            out
        }
        RuntimeInstr::FileOpen { path_ptr, .. } => operand_slots(path_ptr),
        RuntimeInstr::FileWrite { fd, ptr, len, .. }
        | RuntimeInstr::FileRead { fd, ptr, len, .. } => {
            let mut out = operand_slots(fd);
            out.extend(operand_slots(ptr));
            out.extend(operand_slots(len));
            out
        }
        RuntimeInstr::FileClose { fd } => operand_slots(fd),
        RuntimeInstr::ThreadJoin { handle, .. } => operand_slots(handle),
        RuntimeInstr::ChannelCreate { capacity, .. } => operand_slots(capacity),
        RuntimeInstr::ChannelSend { handle, value } => {
            let mut out = operand_slots(handle);
            out.extend(operand_slots(value));
            out
        }
        RuntimeInstr::ChannelRecv { handle, .. }
        | RuntimeInstr::ChannelClose { handle, .. }
        | RuntimeInstr::ChannelDestroy { handle } => operand_slots(handle),
        RuntimeInstr::PrintConst { .. } | RuntimeInstr::Return => Vec::new(),
        RuntimeInstr::PrintInt { value, .. } | RuntimeInstr::Exit { code: value } => {
            operand_slots(value)
        }
    }
}

pub(crate) fn write_slots(instr: &RuntimeInstr) -> Vec<usize> {
    match instr {
        RuntimeInstr::LoadSeed { dst, .. }
        | RuntimeInstr::Mov { dst, .. }
        | RuntimeInstr::BinOp { dst, .. }
        | RuntimeInstr::BinOpInPlace { dst, .. }
        | RuntimeInstr::FloatBinOp { dst, .. }
        | RuntimeInstr::Cmp { dst, .. }
        | RuntimeInstr::NormalizeInt { dst, .. }
        | RuntimeInstr::LoadIndex { dst, .. }
        | RuntimeInstr::LoadIndexUnchecked { dst, .. }
        | RuntimeInstr::HeapLoadInt { dst, .. }
        | RuntimeInstr::BloomSplitBlockCheck { dst, .. }
        | RuntimeInstr::HashCtrlGroupProbe { dst_mask: dst, .. }
        | RuntimeInstr::JoinSelectAdaptive { dst, .. }
        | RuntimeInstr::Alloc { dst, .. }
        | RuntimeInstr::FileOpen { dst, .. }
        | RuntimeInstr::FileWrite { dst, .. }
        | RuntimeInstr::FileRead { dst, .. }
        | RuntimeInstr::ThreadSpawn {
            handle_dst: dst, ..
        }
        | RuntimeInstr::ThreadJoin { dst, .. } => vec![*dst],
        RuntimeInstr::ChannelCreate { dst, .. } | RuntimeInstr::ChannelRecv { dst, .. } => {
            vec![*dst]
        }
        RuntimeInstr::BloomClassic4Check {
            dst, lanes_checked, ..
        } => vec![*dst, *lanes_checked],
        RuntimeInstr::CompareSwap { left, right, .. } => vec![*left, *right],
        RuntimeInstr::RadixSortFixedInt { slots, .. }
        | RuntimeInstr::StoreIndex {
            base_slots: slots, ..
        }
        | RuntimeInstr::StoreIndexUnchecked {
            base_slots: slots, ..
        }
        | RuntimeInstr::BloomSplitBlockInsert {
            filter_slots: slots,
            ..
        } => slots.clone(),
        RuntimeInstr::Jump { .. }
        | RuntimeInstr::JumpIfZero { .. }
        | RuntimeInstr::JumpIfCmpFalse { .. }
        | RuntimeInstr::Call { .. }
        | RuntimeInstr::Return
        | RuntimeInstr::Exit { .. }
        | RuntimeInstr::Free { .. }
        | RuntimeInstr::FileClose { .. }
        | RuntimeInstr::ChannelSend { .. }
        | RuntimeInstr::ChannelClose { .. }
        | RuntimeInstr::ChannelDestroy { .. }
        | RuntimeInstr::PrintConst { .. }
        | RuntimeInstr::PrintInt { .. } => Vec::new(),
        RuntimeInstr::HeapStoreInt { .. } | RuntimeInstr::HeapCopy { .. } => Vec::new(),
    }
}

fn operand_slots(operand: &RuntimeOperand) -> Vec<usize> {
    match operand {
        RuntimeOperand::Slot(slot) => vec![*slot],
        RuntimeOperand::Imm(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::semantics::{RuntimeBinOp, RuntimeCmpOp};

    #[test]
    fn lir_lowering_builds_blocks_and_intervals() {
        let program = RuntimeProgram {
            slots: 3,
            instrs: vec![
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(1),
                },
                RuntimeInstr::JumpIfCmpFalse {
                    op: RuntimeCmpOp::LtUnsigned,
                    lhs: RuntimeOperand::Slot(0),
                    rhs: RuntimeOperand::Imm(4),
                    target: 4,
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
        let lir = MachineLIRProgram::lower(&program, None).expect("lir lowering should succeed");
        assert_eq!(lir.blocks.len(), 4);
        assert!(lir.verify(program.instrs.len()).is_ok());
        let loop_phi = lir
            .phis
            .iter()
            .find(|phi| phi.slot == 0 && lir.blocks[phi.block].instr_indices.contains(&1))
            .expect("loop header must carry slot 0 through an SSA phi");
        assert!(
            loop_phi
                .inputs
                .iter()
                .any(|input| input.predecessor.is_some())
        );
        assert!(loop_phi.inputs.len() >= 2);
        let intervals = lir.compute_live_intervals();
        assert!(intervals.iter().any(|interval| interval.slot == 0));
    }

    #[test]
    fn lir_lowering_rejects_out_of_range_slots() {
        let program = RuntimeProgram {
            slots: 1,
            instrs: vec![RuntimeInstr::Exit {
                code: RuntimeOperand::Slot(1),
            }],
        };
        let err = MachineLIRProgram::lower(&program, None).expect_err("invalid slot must fail");
        assert!(err.contains("references slot 1"), "err={err}");
    }

    #[test]
    fn lir_lowering_rejects_out_of_range_targets() {
        let program = RuntimeProgram {
            slots: 0,
            instrs: vec![RuntimeInstr::Jump { target: 2 }],
        };
        let err = MachineLIRProgram::lower(&program, None).expect_err("invalid target must fail");
        assert!(err.contains("targets instruction 2"), "err={err}");
    }

    #[test]
    fn lir_lowering_accepts_end_of_program_target() {
        let program = RuntimeProgram {
            slots: 0,
            instrs: vec![RuntimeInstr::Jump { target: 1 }],
        };
        let lir = MachineLIRProgram::lower(&program, None)
            .expect("one-past-final target must represent program end");
        assert!(lir.blocks[0].successors.is_empty());
    }

    #[test]
    fn ssa_inserts_phi_for_diamond_merge() {
        let program = RuntimeProgram {
            slots: 2,
            instrs: vec![
                RuntimeInstr::JumpIfZero {
                    cond_slot: 0,
                    target: 3,
                },
                RuntimeInstr::Mov {
                    dst: 1,
                    src: RuntimeOperand::Imm(10),
                },
                RuntimeInstr::Jump { target: 4 },
                RuntimeInstr::Mov {
                    dst: 1,
                    src: RuntimeOperand::Imm(20),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(1),
                },
            ],
        };
        let ssa = RuntimeSSAProgram::lower(&program);
        let phi = ssa
            .phis
            .iter()
            .find(|phi| phi.slot == 1 && ssa.blocks[phi.block].instr_indices.contains(&4))
            .expect("merge must receive an explicit slot-1 phi");
        assert_eq!(phi.inputs.len(), 2);
        assert!(ssa.verify(program.instrs.len()).is_ok());
    }

    #[test]
    fn demanded_bits_flow_back_through_affine_arithmetic() {
        let program = RuntimeProgram {
            slots: 1,
            instrs: vec![
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(7),
                },
                RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: RuntimeBinOp::Mul,
                    rhs: RuntimeOperand::Imm(1_664_525),
                },
                RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: RuntimeBinOp::Add,
                    rhs: RuntimeOperand::Imm(1_013_904_223),
                },
                RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: RuntimeBinOp::BitAnd,
                    rhs: RuntimeOperand::Imm(0xff),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(0),
                },
            ],
        };
        let lir = MachineLIRProgram::lower(&program, None).expect("valid lir");
        assert_eq!(lir.demanded_width_for_instruction(1), 8);
        assert_eq!(lir.demanded_width_for_instruction(2), 8);
        let masked = lir
            .values
            .iter()
            .find(|value| value.def == (SSAValueDef::Instr { instr_index: 3 }))
            .unwrap();
        assert_eq!(lir.known_bits[masked.id].width(), 8);
    }

    #[test]
    fn memory_ssa_tracks_owned_dynamic_object_versions() {
        let base_slots = vec![0, 1];
        let program = RuntimeProgram {
            slots: 4,
            instrs: vec![
                RuntimeInstr::StoreIndexUnchecked {
                    base_slots: base_slots.clone(),
                    index: RuntimeOperand::Slot(2),
                    src: RuntimeOperand::Imm(900),
                },
                RuntimeInstr::LoadIndexUnchecked {
                    dst: 3,
                    base_slots,
                    index: RuntimeOperand::Slot(2),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(3),
                },
            ],
        };
        let lir = MachineLIRProgram::lower(&program, None).expect("valid lir");
        assert_eq!(lir.objects.len(), 1);
        assert_eq!(
            lir.objects[0].representation,
            MemoryRepresentation::ContiguousStack
        );
        assert_eq!(lir.objects[0].alias_class, AliasClass::Unique);
        assert_eq!(lir.memory_accesses.len(), 2);
        assert_ne!(
            lir.memory_accesses[0].incoming_version,
            lir.memory_accesses[0].outgoing_version
        );
        assert_eq!(
            lir.memory_accesses[1].incoming_version,
            lir.memory_accesses[0].outgoing_version
        );
        let intervals = lir.compute_live_intervals();
        assert!(
            intervals
                .iter()
                .filter(|interval| interval.slot < 2)
                .all(|interval| interval.force_stack),
            "dynamic-index representation must remain contiguous and addressable"
        );
    }

    #[test]
    fn owned_non_escaping_byte_range_object_uses_packed_storage() {
        let base_slots = vec![0, 1];
        let program = RuntimeProgram {
            slots: 7,
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
                    dst: 3,
                    src: RuntimeOperand::Imm(u64::MAX),
                },
                RuntimeInstr::BinOp {
                    dst: 4,
                    op: RuntimeBinOp::BitAnd,
                    lhs: RuntimeOperand::Slot(3),
                    rhs: RuntimeOperand::Imm(127),
                },
                RuntimeInstr::BinOp {
                    dst: 5,
                    op: RuntimeBinOp::Add,
                    lhs: RuntimeOperand::Slot(4),
                    rhs: RuntimeOperand::Imm(1),
                },
                RuntimeInstr::StoreIndexUnchecked {
                    base_slots: base_slots.clone(),
                    index: RuntimeOperand::Slot(2),
                    src: RuntimeOperand::Slot(5),
                },
                RuntimeInstr::LoadIndexUnchecked {
                    dst: 6,
                    base_slots,
                    index: RuntimeOperand::Slot(2),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(6),
                },
            ],
        };
        let lir = MachineLIRProgram::lower(&program, None).expect("valid lir");
        assert_eq!(
            lir.objects[0].representation,
            MemoryRepresentation::PackedBytes
        );
        assert!(lir.known_bits.iter().any(|fact| fact.width() == 8));
    }

    #[test]
    fn ownership_selects_scalarized_read_only_representation() {
        let program = RuntimeProgram {
            slots: 3,
            instrs: vec![
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(11),
                },
                RuntimeInstr::Mov {
                    dst: 1,
                    src: RuntimeOperand::Imm(13),
                },
                RuntimeInstr::LoadIndexUnchecked {
                    dst: 2,
                    base_slots: vec![0, 1],
                    index: RuntimeOperand::Imm(1),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(2),
                },
            ],
        };
        let lir = MachineLIRProgram::lower(&program, None).expect("valid lir");
        assert_eq!(lir.objects[0].alias_class, AliasClass::ReadOnlyShared);
        assert_eq!(
            lir.objects[0].representation,
            MemoryRepresentation::Scalarized
        );
        let intervals = lir.compute_live_intervals();
        assert!(
            intervals
                .iter()
                .filter(|interval| interval.slot < 2)
                .all(|interval| !interval.force_stack)
        );
    }

    #[test]
    fn internal_abi_tracks_call_return_liveness() {
        let program = RuntimeProgram {
            slots: 1,
            instrs: vec![
                RuntimeInstr::Call { target: 2 },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(0),
                },
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(7),
                },
                RuntimeInstr::Return,
            ],
        };
        let lir = MachineLIRProgram::lower(&program, None).expect("valid lir");
        assert_eq!(lir.call_sites.len(), 1);
        assert!(lir.call_sites[0].callee_leaf);
        assert_eq!(lir.call_sites[0].callee_writes, vec![0]);
        assert_eq!(lir.call_sites[0].live_across, vec![0]);
        let call_block = lir
            .blocks
            .iter()
            .find(|block| block.instr_indices.contains(&0))
            .unwrap();
        assert!(call_block.successors.len() >= 2);
    }

    #[test]
    fn internal_abi_handles_recursion_and_multiple_callsites() {
        let program = RuntimeProgram {
            slots: 1,
            instrs: vec![
                RuntimeInstr::Call { target: 3 },
                RuntimeInstr::Call { target: 3 },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(0),
                },
                RuntimeInstr::JumpIfZero {
                    cond_slot: 0,
                    target: 7,
                },
                RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: RuntimeBinOp::Sub,
                    rhs: RuntimeOperand::Imm(1),
                },
                RuntimeInstr::Call { target: 3 },
                RuntimeInstr::Return,
                RuntimeInstr::Return,
            ],
        };
        let lir = MachineLIRProgram::lower(&program, None).expect("valid recursive lir");
        assert_eq!(lir.call_sites.len(), 3);
        assert!(lir.call_sites.iter().any(|call| !call.callee_leaf));
        assert!(lir.call_sites.iter().any(|call| call.tail));
        assert!(lir.verify(program.instrs.len()).is_ok());
    }

    #[test]
    fn loop_plan_detects_affine_recurrence_and_dependencies() {
        let program = RuntimeProgram {
            slots: 1,
            instrs: vec![
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(1),
                },
                RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: RuntimeBinOp::Mul,
                    rhs: RuntimeOperand::Imm(5),
                },
                RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: RuntimeBinOp::Add,
                    rhs: RuntimeOperand::Imm(3),
                },
                RuntimeInstr::BinOpInPlace {
                    dst: 0,
                    op: RuntimeBinOp::BitAnd,
                    rhs: RuntimeOperand::Imm(u32::MAX as u64),
                },
                RuntimeInstr::Jump { target: 1 },
            ],
        };
        let lir = MachineLIRProgram::lower(&program, None).expect("valid lir");
        assert_eq!(lir.loops.len(), 1);
        assert_eq!(
            lir.loops[0].recurrences,
            vec![AffineRecurrence {
                slot: 0,
                mul: 5,
                add: 3,
                mask: u32::MAX as u64,
            }]
        );
        assert!(!lir.loops[0].dependencies.is_empty());
    }

    #[test]
    fn live_intervals_keep_lifetime_holes_and_rematerialization_facts() {
        let program = RuntimeProgram {
            slots: 1,
            instrs: vec![
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(7),
                },
                RuntimeInstr::PrintInt {
                    value: RuntimeOperand::Slot(0),
                    signed: false,
                    bits: 64,
                },
                RuntimeInstr::Jump { target: 4 },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Imm(0),
                },
                RuntimeInstr::Mov {
                    dst: 0,
                    src: RuntimeOperand::Imm(7),
                },
                RuntimeInstr::Exit {
                    code: RuntimeOperand::Slot(0),
                },
            ],
        };
        let lir = MachineLIRProgram::lower(&program, None).expect("valid lir");
        let interval = lir
            .compute_live_intervals()
            .into_iter()
            .find(|interval| interval.slot == 0)
            .expect("slot 0 interval");
        assert_eq!(interval.rematerializable, Some(7));
        assert_eq!(interval.segments.len(), 2);
        assert!(interval.segments[0].end < interval.segments[1].start);
    }
}
