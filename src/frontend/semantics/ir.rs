//! Public semantic output and runtime instruction representation.

#[derive(Debug, Clone)]
pub enum LoweredStmt {
    Print(String),
    Exit(u64),
    RuntimeBenchLoop {
        iterations: u64,
    },
    RuntimeLcgLoop {
        iterations: u64,
        state_init: u64,
        mul: u32,
        add: u32,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    },
    RuntimeSeededLcgLoop {
        iterations: u64,
        mul: u32,
        add: u32,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    },
    RuntimeRingWriteLoop {
        iterations: u64,
        state_init: u64,
        index_init: u64,
        mul: u32,
        add: u32,
        state_mask: u64,
        ring_mask: u64,
        value_shift: u8,
        exit_mask: u64,
    },
    RuntimePrefixScanLoop {
        batches: u64,
        state_init: u64,
        mul: u32,
        add: u32,
        state_mask: u64,
        value_mask: u64,
        width: u8,
        exit_mask: u64,
    },
    RuntimeBloomFilterLoop {
        state_init: u64,
        build_iterations: u64,
        query_iterations: u64,
        hits_init: u64,
        exit_mask: u64,
    },
    RuntimeBranchLcgLoop {
        iterations: u64,
        state_init: u64,
        state_mask: u64,
        threshold: u64,
        then_mul: u32,
        then_add: u32,
        else_mul: u32,
        else_add: u32,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    },
    RuntimeSeededLcgAllocLoop {
        iterations: u64,
        mul: u32,
        add: u32,
        alloc_bytes: u64,
        exit_with_state: bool,
    },
    RuntimeSeededPredictableBranchLcgLoop {
        iterations: u64,
        then_iterations: u64,
        then_mul: u32,
        then_add: u32,
        else_mul: u32,
        else_add: u32,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    },
    RuntimeSeededUnpredictableBranchLcgLoop {
        iterations: u64,
        threshold: u64,
        then_mul: u32,
        then_add: u32,
        else_mul: u32,
        else_add: u32,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    },
    RuntimeAffineIndexLoop {
        iterations: u64,
        state_init: u64,
        index_init: u64,
        state_mul: u32,
        index_mul: u32,
        add: u32,
        state_mask: u64,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    },
    RuntimeSeededDualStateBranchLoop {
        iterations: u64,
        index_init: u64,
        adaptive: bool,
        branchless: bool,
        exit_with_sum: bool,
    },
    RuntimeSeededAffineIndexLoop {
        iterations: u64,
        index_init: u64,
        state_mul: u32,
        index_mul: u32,
        add: u32,
        state_mask: u64,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    },
    RuntimeSeededAffineClosedForm {
        state_mul: u64,
        add: u64,
        exit_with_state: bool,
    },
    RuntimeSeededStructLatencyLoop {
        iterations: u64,
        mul: u32,
        add: u32,
        exit_with_sum: bool,
    },
    RuntimeGeneric {
        program: RuntimeProgram,
    },
}

#[derive(Debug, Clone)]
pub struct RuntimeProgram {
    pub slots: usize,
    pub instrs: Vec<RuntimeInstr>,
}

#[derive(Debug, Clone, Copy)]
pub enum RuntimeOperand {
    Slot(usize),
    Imm(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeBinOp {
    Add,
    Sub,
    Mul,
    DivUnsigned,
    DivSigned,
    ModUnsigned,
    ModSigned,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    ShrUnsigned,
    ShrSigned,
}

#[derive(Debug, Clone, Copy)]
pub enum RuntimeFloatBinOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeCmpOp {
    Eq,
    Ne,
    LtUnsigned,
    LeUnsigned,
    GtUnsigned,
    GeUnsigned,
    LtSigned,
    LeSigned,
    GtSigned,
    GeSigned,
}

#[derive(Debug, Clone)]
pub enum RuntimeLoadKind {
    EntropySeed,
    ArgumentCount,
    EntryStackPointer,
    MonotonicNanos,
    WallTimeNanos,
    ProcessId,
}

#[derive(Debug, Clone)]
pub enum RuntimeInstr {
    LoadSeed {
        dst: usize,
        kind: RuntimeLoadKind,
        input: Option<RuntimeOperand>,
    },
    Mov {
        dst: usize,
        src: RuntimeOperand,
    },
    BinOp {
        dst: usize,
        op: RuntimeBinOp,
        lhs: RuntimeOperand,
        rhs: RuntimeOperand,
    },
    BinOpInPlace {
        dst: usize,
        op: RuntimeBinOp,
        rhs: RuntimeOperand,
    },
    FloatBinOp {
        dst: usize,
        bits: u16,
        op: RuntimeFloatBinOp,
        lhs: RuntimeOperand,
        rhs: RuntimeOperand,
    },
    Cmp {
        dst: usize,
        op: RuntimeCmpOp,
        lhs: RuntimeOperand,
        rhs: RuntimeOperand,
    },
    NormalizeInt {
        dst: usize,
        signed: bool,
        bits: u16,
    },
    Jump {
        target: usize,
    },
    JumpIfZero {
        cond_slot: usize,
        target: usize,
    },
    JumpIfCmpFalse {
        op: RuntimeCmpOp,
        lhs: RuntimeOperand,
        rhs: RuntimeOperand,
        target: usize,
    },
    CompareSwap {
        left: usize,
        right: usize,
        signed: bool,
    },
    RadixSortFixedInt {
        slots: Vec<usize>,
        bits: u16,
        signed: bool,
        stable: bool,
    },
    Call {
        target: usize,
    },
    LoadIndex {
        dst: usize,
        base_slots: Vec<usize>,
        index: RuntimeOperand,
    },
    LoadIndexUnchecked {
        dst: usize,
        base_slots: Vec<usize>,
        index: RuntimeOperand,
    },
    StoreIndex {
        base_slots: Vec<usize>,
        index: RuntimeOperand,
        src: RuntimeOperand,
    },
    StoreIndexUnchecked {
        base_slots: Vec<usize>,
        index: RuntimeOperand,
        src: RuntimeOperand,
    },
    HeapLoadInt {
        dst: usize,
        ptr: RuntimeOperand,
        index: RuntimeOperand,
        bytes: u8,
    },
    HeapStoreInt {
        ptr: RuntimeOperand,
        index: RuntimeOperand,
        src: RuntimeOperand,
        bytes: u8,
    },
    HeapCopy {
        dst_ptr: RuntimeOperand,
        src_ptr: RuntimeOperand,
        bytes: RuntimeOperand,
    },
    BloomSplitBlockInsert {
        filter_slots: Vec<usize>,
        hash: RuntimeOperand,
    },
    BloomSplitBlockCheck {
        dst: usize,
        filter_slots: Vec<usize>,
        hash: RuntimeOperand,
    },
    BloomClassic4Check {
        dst: usize,
        lanes_checked: usize,
        filter_slots: Vec<usize>,
        hash: RuntimeOperand,
    },
    HashCtrlGroupProbe {
        dst_mask: usize,
        ctrl_slots: Vec<usize>,
        group_start: RuntimeOperand,
        fingerprint: RuntimeOperand,
    },
    JoinSelectAdaptive {
        dst: usize,
        build_rows: RuntimeOperand,
        probe_rows: RuntimeOperand,
    },
    Alloc {
        dst: usize,
        size: RuntimeOperand,
    },
    Free {
        ptr: RuntimeOperand,
        size: RuntimeOperand,
    },
    FileOpen {
        dst: usize,
        path_ptr: RuntimeOperand,
        flags: u32,
        mode: u32,
    },
    FileWrite {
        dst: usize,
        fd: RuntimeOperand,
        ptr: RuntimeOperand,
        len: RuntimeOperand,
    },
    FileRead {
        dst: usize,
        fd: RuntimeOperand,
        ptr: RuntimeOperand,
        len: RuntimeOperand,
    },
    FileClose {
        fd: RuntimeOperand,
    },
    /// Start a native worker at `target` on an isolated whole-program frame.
    ///
    /// The parent frame is copied before the target runtime creates the worker,
    /// so scalar
    /// function parameters are transferred by value without sharing compiler
    /// slots. `return_slot` is read in the worker frame after the function
    /// returns. The resulting opaque handle owns the worker stack mapping and
    /// must be consumed exactly once by `ThreadJoin`.
    ThreadSpawn {
        handle_dst: usize,
        target: usize,
        return_slot: Option<usize>,
    },
    /// Join a native worker, release its stack mapping, and publish its stable
    /// completion code. A normal `u64` return is preserved; a terminating
    /// failure is represented by the process-style exit code.
    ThreadJoin {
        dst: usize,
        handle: RuntimeOperand,
    },
    ChannelCreate {
        dst: usize,
        capacity: RuntimeOperand,
        unbounded: bool,
    },
    ChannelSend {
        handle: RuntimeOperand,
        value: RuntimeOperand,
    },
    ChannelRecv {
        dst: usize,
        handle: RuntimeOperand,
    },
    ChannelClose {
        handle: RuntimeOperand,
        sender: bool,
    },
    ChannelDestroy {
        handle: RuntimeOperand,
    },
    PrintConst {
        text: String,
    },
    PrintInt {
        value: RuntimeOperand,
        signed: bool,
        bits: u16,
    },
    Return,
    Exit {
        code: RuntimeOperand,
    },
}
