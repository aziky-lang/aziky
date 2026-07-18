/// x86-64 Peephole Optimization Patterns
///
/// Concrete instruction sequences that give real speedups on modern x86-64 CPUs.
/// Based on Agner Fog's optimization manuals, uops.info measurements, and
/// Intel/AMD architecture references.
///
/// Targets: Skylake (Intel), Zen3+ (AMD)

/// Macro-operation fusion rules.
/// On modern x86, certain instruction pairs are fused into a single macro-op
/// during decode. This saves decode bandwidth and improves performance.
pub struct FusionRules;

impl FusionRules {
    /// CMP/TEST + Jcc fuses into 1 macro-op on Skylake+ and Zen3+.
    /// CRITICAL: Any instruction between CMP and JCC breaks fusion.
    pub const CMP_JCC_FUSES: bool = true;
    /// TEST + JCC fuses (same as CMP+JCC)
    pub const TEST_JCC_FUSES: bool = true;
    /// CMP/TEST + CMOV does NOT fuse
    pub const CMP_CMOV_FUSES: bool = false;
    /// CMP/TEST + SETCC does NOT fuse
    pub const CMP_SETCC_FUSES: bool = false;
}

/// Port usage for Skylake-class CPUs.
/// Port 5 is the bottleneck for simple ALU ops (LEA, shifts, flag-setting).
/// Port 6 is specialized for INC/DEC and flag-only operations.
#[derive(Debug, Clone, Copy)]
pub enum ExecutionPort {
    /// Complex ALU (multiply, divide, vector FP/INT multiply)
    Port0,
    /// Simple ALU (ADD, XOR, CMOV, vector ALU, shuffle)
    Port1,
    /// Load port
    Port2,
    /// Load port
    Port3,
    /// Store address generation
    Port4,
    /// Simple ALU with flag output (LEA simple, shifts, CMP, TEST)
    Port5,
    /// Flag-only operations (INC, DEC)
    Port6,
}

/// Instruction latency/throughput characteristics for Skylake.
#[derive(Debug, Clone)]
pub struct InstructionInfo {
    pub name: &'static str,
    pub latency_cycles: u8,
    pub throughput_cycles: f32,
    pub micro_ops: u8,
    pub ports: &'static [ExecutionPort],
    pub fuses_with_next: bool,
}

/// Returns the instruction info for a given instruction type.
pub fn get_instruction_info(mnemonic: &str) -> Option<InstructionInfo> {
    match mnemonic {
        "mov" => Some(InstructionInfo {
            name: "MOV",
            latency_cycles: 1,
            throughput_cycles: 0.25,
            micro_ops: 1,
            ports: &[ExecutionPort::Port5],
            fuses_with_next: false,
        }),
        "add" => Some(InstructionInfo {
            name: "ADD",
            latency_cycles: 1,
            throughput_cycles: 0.25,
            micro_ops: 1,
            ports: &[
                ExecutionPort::Port0,
                ExecutionPort::Port1,
                ExecutionPort::Port5,
            ],
            fuses_with_next: false,
        }),
        "sub" => Some(InstructionInfo {
            name: "SUB",
            latency_cycles: 1,
            throughput_cycles: 0.25,
            micro_ops: 1,
            ports: &[
                ExecutionPort::Port0,
                ExecutionPort::Port1,
                ExecutionPort::Port5,
            ],
            fuses_with_next: false,
        }),
        "and" => Some(InstructionInfo {
            name: "AND",
            latency_cycles: 1,
            throughput_cycles: 0.25,
            micro_ops: 1,
            ports: &[
                ExecutionPort::Port0,
                ExecutionPort::Port1,
                ExecutionPort::Port5,
            ],
            fuses_with_next: false,
        }),
        "or" => Some(InstructionInfo {
            name: "OR",
            latency_cycles: 1,
            throughput_cycles: 0.25,
            micro_ops: 1,
            ports: &[
                ExecutionPort::Port0,
                ExecutionPort::Port1,
                ExecutionPort::Port5,
            ],
            fuses_with_next: false,
        }),
        "xor" => Some(InstructionInfo {
            name: "XOR",
            latency_cycles: 1,
            throughput_cycles: 0.25,
            micro_ops: 1,
            ports: &[
                ExecutionPort::Port0,
                ExecutionPort::Port1,
                ExecutionPort::Port5,
            ],
            fuses_with_next: false,
        }),
        "lea" => Some(InstructionInfo {
            name: "LEA",
            latency_cycles: 1,
            throughput_cycles: 0.5,
            micro_ops: 1,
            ports: &[ExecutionPort::Port1, ExecutionPort::Port5],
            fuses_with_next: false,
        }),
        "cmp" => Some(InstructionInfo {
            name: "CMP",
            latency_cycles: 1,
            throughput_cycles: 0.25,
            micro_ops: 1,
            ports: &[
                ExecutionPort::Port0,
                ExecutionPort::Port1,
                ExecutionPort::Port6,
            ],
            fuses_with_next: true,
        }),
        "test" => Some(InstructionInfo {
            name: "TEST",
            latency_cycles: 1,
            throughput_cycles: 0.25,
            micro_ops: 1,
            ports: &[
                ExecutionPort::Port0,
                ExecutionPort::Port1,
                ExecutionPort::Port6,
            ],
            fuses_with_next: true,
        }),
        "je" | "jne" | "jl" | "jg" | "jle" | "jge" | "jb" | "ja" | "jbe" | "jae" | "jz" | "jnz"
        | "js" | "jns" => Some(InstructionInfo {
            name: "Jcc",
            latency_cycles: 0,
            throughput_cycles: 0.25,
            micro_ops: 0,
            ports: &[],
            fuses_with_next: false,
        }),
        "cmove" | "cmovne" | "cmovl" | "cmovg" | "cmovle" | "cmovge" | "cmovz" | "cmovnz" => {
            Some(InstructionInfo {
                name: "CMOVcc",
                latency_cycles: 1,
                throughput_cycles: 0.5,
                micro_ops: 1,
                ports: &[
                    ExecutionPort::Port0,
                    ExecutionPort::Port1,
                    ExecutionPort::Port5,
                ],
                fuses_with_next: false,
            })
        }
        "inc" => Some(InstructionInfo {
            name: "INC",
            latency_cycles: 1,
            throughput_cycles: 0.25,
            micro_ops: 1,
            ports: &[ExecutionPort::Port6],
            fuses_with_next: false,
        }),
        "dec" => Some(InstructionInfo {
            name: "DEC",
            latency_cycles: 1,
            throughput_cycles: 0.25,
            micro_ops: 1,
            ports: &[ExecutionPort::Port6],
            fuses_with_next: false,
        }),
        "popcnt" => Some(InstructionInfo {
            name: "POPCNT",
            latency_cycles: 3,
            throughput_cycles: 1.0,
            micro_ops: 1,
            ports: &[ExecutionPort::Port1],
            fuses_with_next: false,
        }),
        "tzcnt" | "lzcnt" | "bsf" | "bsr" => Some(InstructionInfo {
            name: "TZCNT/LZCNT",
            latency_cycles: 3,
            throughput_cycles: 1.0,
            micro_ops: 1,
            ports: &[ExecutionPort::Port0, ExecutionPort::Port1],
            fuses_with_next: false,
        }),
        "imul" => Some(InstructionInfo {
            name: "IMUL",
            latency_cycles: 3,
            throughput_cycles: 1.0,
            micro_ops: 3,
            ports: &[ExecutionPort::Port1],
            fuses_with_next: false,
        }),
        "div" => Some(InstructionInfo {
            name: "DIV",
            latency_cycles: 23,
            throughput_cycles: 8.0,
            micro_ops: 1,
            ports: &[ExecutionPort::Port0],
            fuses_with_next: false,
        }),
        "idiv" => Some(InstructionInfo {
            name: "IDIV",
            latency_cycles: 23,
            throughput_cycles: 8.0,
            micro_ops: 1,
            ports: &[ExecutionPort::Port0],
            fuses_with_next: false,
        }),
        "sete" | "setne" | "setl" | "setg" | "setle" | "setge" | "setb" | "seta" | "setbe"
        | "setae" | "setz" | "setnz" => Some(InstructionInfo {
            name: "SETcc",
            latency_cycles: 2,
            throughput_cycles: 0.5,
            micro_ops: 1,
            ports: &[ExecutionPort::Port5],
            fuses_with_next: false,
        }),
        "bswap" => Some(InstructionInfo {
            name: "BSWAP",
            latency_cycles: 1,
            throughput_cycles: 1.0,
            micro_ops: 1,
            ports: &[ExecutionPort::Port1],
            fuses_with_next: false,
        }),
        "btr" | "bts" | "bt" | "btc" => Some(InstructionInfo {
            name: "BT*",
            latency_cycles: 1,
            throughput_cycles: 1.0,
            micro_ops: 1,
            ports: &[ExecutionPort::Port0],
            fuses_with_next: false,
        }),
        "blsr" | "blsmsk" | "blsi" => Some(InstructionInfo {
            name: "BMI1",
            latency_cycles: 1,
            throughput_cycles: 1.0,
            micro_ops: 1,
            ports: &[ExecutionPort::Port0],
            fuses_with_next: false,
        }),
        "andn" => Some(InstructionInfo {
            name: "ANDN",
            latency_cycles: 1,
            throughput_cycles: 1.0,
            micro_ops: 1,
            ports: &[ExecutionPort::Port1],
            fuses_with_next: false,
        }),
        "bzhi" | "pdep" | "pext" | "shrx" | "shlx" | "sarx" => Some(InstructionInfo {
            name: "BMI2",
            latency_cycles: 3,
            throughput_cycles: 1.0,
            micro_ops: 1,
            ports: &[ExecutionPort::Port1],
            fuses_with_next: false,
        }),
        "shl" | "shr" | "sar" => Some(InstructionInfo {
            name: "SHIFT",
            latency_cycles: 1,
            throughput_cycles: 0.5,
            micro_ops: 1,
            ports: &[
                ExecutionPort::Port0,
                ExecutionPort::Port1,
                ExecutionPort::Port5,
            ],
            fuses_with_next: false,
        }),
        _ => None,
    }
}

/// Peephole optimization patterns that the compiler should apply.
pub struct PeepholePatterns;

impl PeepholePatterns {
    // ── Strength Reduction ──

    /// Division by power of 2 → right shift (unsigned)
    pub fn div_by_power_of_2_unsigned(n: u8) -> ShiftRight {
        ShiftRight { shift_amount: n }
    }

    /// Division by power of 2 → arithmetic right shift (signed)
    pub fn div_by_power_of_2_signed(n: u8) -> SignedShiftRight {
        SignedShiftRight { shift_amount: n }
    }

    /// Modulo by power of 2 → AND mask
    pub fn mod_by_power_of_2(n: u8) -> AndMask {
        AndMask {
            mask: (1u64 << n) - 1,
        }
    }

    /// Multiplication by small constant → LEA chain
    pub fn mul_by_constant(x_reg: &str, constant: u64, dest_reg: &str) -> Vec<String> {
        match constant {
            0 => vec![format!("xor {}, {}", dest_reg, dest_reg)],
            1 => vec![format!("mov {}, {}", dest_reg, x_reg)],
            2 => vec![format!("lea {}, [{}*2]", dest_reg, x_reg)],
            3 => vec![format!("lea {}, [{}+{}*2]", dest_reg, x_reg, x_reg)],
            4 => vec![format!("lea {}, [{}*4]", dest_reg, x_reg)],
            5 => vec![format!("lea {}, [{}+{}*4]", dest_reg, x_reg, x_reg)],
            6 => vec![
                format!("lea {}, [{}+{}*2]", dest_reg, x_reg, x_reg),
                format!("shl {}, 1", dest_reg),
            ],
            7 => vec![
                format!("lea {}, [{}*8]", dest_reg, x_reg),
                format!("sub {}, {}", dest_reg, x_reg),
            ],
            8 => vec![format!("lea {}, [{}*8]", dest_reg, x_reg)],
            9 => vec![
                format!("lea {}, [{}*8]", dest_reg, x_reg),
                format!("add {}, {}", dest_reg, x_reg),
            ],
            10 => vec![
                format!("lea {}, [{}+{}*4]", dest_reg, x_reg, x_reg),
                format!("shl {}, 1", dest_reg),
            ],
            _ => vec![format!("imul {}, {}, {}", dest_reg, x_reg, constant)],
        }
    }

    /// Magic number for unsigned division by common constants.
    pub fn unsigned_div_magic(d: u64) -> Option<(u64, u32)> {
        match d {
            3 => Some((0xAAAAAAAB, 1)),
            5 => Some((0xCCCCCCCD, 2)),
            6 => Some((0xAAAAAAAB, 1)),
            7 => Some((0x24924925, 2)),
            9 => Some((0x38E38E39, 2)),
            10 => Some((0xCCCCCCCD, 3)),
            11 => Some((0x2E8BA2E9, 2)),
            12 => Some((0xAAAAAAAB, 2)),
            15 => Some((0x88888889, 3)),
            16 => Some((0x10000000, 4)),
            _ => None,
        }
    }

    /// Absolute value without branch
    pub fn abs_without_branch(x_reg: &str) -> Vec<String> {
        vec![
            format!("mov ecx, {}", x_reg),
            format!("sar eax, 31"),
            format!("xor eax, ecx"),
            format!("sub eax, ecx"),
        ]
    }

    /// Sign of integer without branch: 0 or 1
    pub fn sign_without_branch(x_reg: &str) -> String {
        format!("shr {}, 31", x_reg)
    }

    /// Min without branch (CMOV)
    pub fn min_without_branch(a_reg: &str, b_reg: &str) -> Vec<String> {
        vec![
            format!("cmp {}, {}", a_reg, b_reg),
            format!("cmovg {}, {}", a_reg, b_reg),
        ]
    }

    /// Max without branch (CMOV)
    pub fn max_without_branch(a_reg: &str, b_reg: &str) -> Vec<String> {
        vec![
            format!("cmp {}, {}", a_reg, b_reg),
            format!("cmovl {}, {}", a_reg, b_reg),
        ]
    }

    /// Check if power of 2: `x & (x-1) == 0 && x != 0`
    pub fn is_power_of_2(x_reg: &str) -> Vec<String> {
        vec![
            format!("mov ecx, {}", x_reg),
            format!("sub ecx, 1"),
            format!("test {}, {}", x_reg, "ecx"),
            format!("setz al"),
        ]
    }

    /// Zero a register: use `xor reg, reg` (dependency-breaking)
    pub fn zero_reg(reg: &str) -> String {
        format!("xor {}, {}", reg, reg)
    }

    /// Add without modifying flags: use LEA instead of ADD
    pub fn add_preserving_flags(dest: &str, src: &str) -> String {
        format!("lea {}, [{}+{}]", dest, dest, src)
    }

    /// `x & (x-1)` → `blsr dest, src`
    pub fn clear_lowest_set_bit(src: &str, dest: &str) -> String {
        format!("blsr {}, {}", dest, src)
    }

    /// `x & (-x)` → `blsi dest, src`
    pub fn isolate_lowest_set_bit(src: &str, dest: &str) -> String {
        format!("blsi {}, {}", dest, src)
    }

    /// `x ^ (x-1)` → `blsmsk dest, src`
    pub fn mask_up_to_lowest_set_bit(src: &str, dest: &str) -> String {
        format!("blsmsk {}, {}", dest, src)
    }

    /// `~a & b` → `andn dest, a, b`
    pub fn and_not(a: &str, b: &str, dest: &str) -> String {
        format!("andn {}, {}, {}", dest, a, b)
    }

    /// Variable shift right without flag effects
    pub fn var_shift_right(dest: &str, src: &str, count: &str) -> String {
        format!("shrx {}, {}, {}", dest, src, count)
    }

    /// Zero high bits above position
    pub fn zero_high_bits(dest: &str, src: &str, pos: &str) -> String {
        format!("bzhi {}, {}, {}", dest, src, pos)
    }

    /// Copy 16 bytes (one XMM register)
    pub fn copy_16_bytes(src: &str, dest: &str) -> Vec<String> {
        vec![
            format!("movdqu xmm0, [{}]", src),
            format!("movdqu [{}], xmm0", dest),
        ]
    }

    /// Compare 4x32-bit integers in parallel
    pub fn cmpeqd_parallel(a: &str, b: &str, result_reg: &str) -> Vec<String> {
        vec![
            format!("movdqu xmm0, [{}]", a),
            format!("pcmpeqd xmm0, [{}]", b),
            format!("pmovmskb {}, xmm0", result_reg),
        ]
    }
}

/// Helper types for generated patterns
pub struct ShiftRight {
    pub shift_amount: u8,
}

pub struct SignedShiftRight {
    pub shift_amount: u8,
}

pub struct AndMask {
    pub mask: u64,
}

/// Port pressure analysis for instruction sequences.
pub struct PortPressureAnalyzer {
    port_counts: [u32; 7],
}

impl PortPressureAnalyzer {
    pub fn new() -> Self {
        Self {
            port_counts: [0; 7],
        }
    }

    pub fn add_instruction(&mut self, info: &InstructionInfo) {
        for port in info.ports {
            let idx = match port {
                ExecutionPort::Port0 => 0,
                ExecutionPort::Port1 => 1,
                ExecutionPort::Port2 => 2,
                ExecutionPort::Port3 => 3,
                ExecutionPort::Port4 => 4,
                ExecutionPort::Port5 => 5,
                ExecutionPort::Port6 => 6,
            };
            self.port_counts[idx] += 1;
        }
    }

    /// Returns the most overloaded port (bottleneck).
    pub fn bottleneck_port(&self) -> Option<ExecutionPort> {
        let max_idx = self
            .port_counts
            .iter()
            .enumerate()
            .max_by_key(|&(_, count)| count)
            .map(|(idx, _)| idx)?;
        Some(match max_idx {
            0 => ExecutionPort::Port0,
            1 => ExecutionPort::Port1,
            2 => ExecutionPort::Port2,
            3 => ExecutionPort::Port3,
            4 => ExecutionPort::Port4,
            5 => ExecutionPort::Port5,
            6 => ExecutionPort::Port6,
            _ => unreachable!(),
        })
    }

    /// Returns the bottleneck port count.
    pub fn bottleneck_count(&self) -> u32 {
        *self.port_counts.iter().max().unwrap_or(&0)
    }

    /// Check if this sequence is port-balanced.
    pub fn is_balanced(&self) -> bool {
        let max = self.bottleneck_count();
        let min = *self.port_counts.iter().min().unwrap_or(&0);
        max <= min * 2 + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mul_by_constant() {
        let result = PeepholePatterns::mul_by_constant("rcx", 3, "rax");
        assert_eq!(result, vec!["lea rax, [rcx+rcx*2]"]);

        let result = PeepholePatterns::mul_by_constant("rcx", 5, "rax");
        assert_eq!(result, vec!["lea rax, [rcx+rcx*4]"]);

        let result = PeepholePatterns::mul_by_constant("rcx", 7, "rax");
        assert_eq!(result, vec!["lea rax, [rcx*8]", "sub rax, rcx"]);
    }

    #[test]
    fn test_zero_reg() {
        assert_eq!(PeepholePatterns::zero_reg("rax"), "xor rax, rax");
    }

    #[test]
    fn test_port_pressure() {
        let mut analyzer = PortPressureAnalyzer::new();
        let add_info = get_instruction_info("add").unwrap();
        let lea_info = get_instruction_info("lea").unwrap();

        for _ in 0..10 {
            analyzer.add_instruction(&add_info);
        }
        assert!(analyzer.bottleneck_count() > 0);

        analyzer.add_instruction(&lea_info);
    }

    #[test]
    fn test_div_magic_numbers() {
        let (magic, shift) = PeepholePatterns::unsigned_div_magic(10).unwrap();
        assert_eq!(magic, 0xCCCCCCCD);
        assert_eq!(shift, 3);
    }

    #[test]
    fn test_instruction_info() {
        let info = get_instruction_info("popcnt").unwrap();
        assert_eq!(info.latency_cycles, 3);
        assert_eq!(info.throughput_cycles, 1.0);
    }
}
