//! Public semantic output and runtime instruction representation.

#[derive(Debug, Clone)]
pub enum LoweredStmt {
    Print(String),
    Exit(u64),
    RuntimeGeneric { program: RuntimeProgram },
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
