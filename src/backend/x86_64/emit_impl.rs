impl X86Program {
    fn emit_kernel_call(&mut self) {
        match self.runtime.kernel_call_style {
            KernelCallStyle::LinuxSyscall => self.code.extend_from_slice(&[0x0f, 0x05]),
            KernelCallStyle::DarwinSyscall => {
                self.code.extend_from_slice(&[0x0f, 0x05]);
                self.code.extend_from_slice(&[0x73, 0x03]); // jnc success
                self.code.extend_from_slice(&[0x48, 0xf7, 0xd8]); // errno => -errno
            }
            KernelCallStyle::WindowsImport => {
                self.code.push(0xe8);
                let patch = self.code.len();
                self.code.extend_from_slice(&0_i32.to_le_bytes());
                self.kernel_call_patches.push(patch);
            }
        }
    }

    pub fn new() -> Self {
        Self::with_options(X86BackendOptions::default())
    }

    pub fn with_options(options: X86BackendOptions) -> Self {
        let runtime = options.target.native_runtime_abi().unwrap_or_else(|| {
            panic!(
                "target '{}' reached x86 runtime emission without an accepted native ABI",
                options.target.triple()
            )
        });
        Self {
            code: Vec::new(),
            data: Vec::new(),
            data_offsets: HashMap::new(),
            patches: Vec::new(),
            kernel_call_patches: Vec::new(),
            options,
            runtime,
            runtime_generic_metadata: None,
            runtime_allocator: None,
        }
    }

    pub fn runtime_generic_lir_dump(&self) -> Option<String> {
        self.runtime_generic_metadata
            .as_ref()
            .map(|metadata| {
                let mut dump = metadata.lir.dump();
                dump.push_str(&format!(
                    "optimization-report {:?}\n",
                    metadata.optimization_report
                ));
                dump
            })
    }

    pub fn runtime_generic_profile_template(&self) -> Option<String> {
        self.runtime_generic_metadata
            .as_ref()
            .map(|metadata| metadata.profile_template.render())
    }

    pub fn runtime_generic_block_map(&self) -> Option<String> {
        let metadata = self.runtime_generic_metadata.as_ref()?;
        let mut out = String::new();
        for (block_id, start, end) in &metadata.block_offsets {
            out.push_str(&format!("block {} code_start={} code_end={}\n", block_id, start, end));
        }
        Some(out)
    }

    pub fn emit_write(&mut self, message: &[u8]) {
        // mov eax, 1 ; SYS_write
        self.emit_mov_rax_imm(self.runtime.syscalls.write);

        // mov edi, 1 ; fd = stdout
        self.emit_mov_rdi_imm(1);

        // lea rsi, [rip + disp32]
        self.code.extend_from_slice(&[0x48, 0x8D, 0x35]);
        let disp_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        // mov edx, msg_len
        self.emit_mov_rdx_imm(message.len() as u64);

        // syscall
        self.emit_kernel_call();

        let data_offset = if let Some(offset) = self.data_offsets.get(message) {
            *offset
        } else {
            let offset = self.data.len();
            self.data.extend_from_slice(message);
            self.data_offsets.insert(message.to_vec(), offset);
            offset
        };
        self.patches.push(Patch {
            disp_pos,
            data_offset,
        });
    }

    pub fn emit_exit(&mut self, status: u64) {
        if self.options.emit_full_checksum {
            self.emit_mov_rax_imm(status);
            self.emit_raw_checksum_from_rax();
        }
        if self.runtime_allocator.is_some() {
            self.emit_runtime_allocator_teardown_call();
        }
        // mov eax, 60 ; SYS_exit
        self.emit_mov_rax_imm(self.runtime.syscalls.exit);

        // mov edi, status
        self.emit_mov_rdi_imm(status);

        // syscall
        self.emit_kernel_call();
    }

    pub fn emit_runtime_bench_loop(&mut self, iterations: u64) {
        self.emit_runtime_lcg_loop(iterations, 1, 1_664_525, 1_013_904_223, true, None);
    }

    pub fn emit_runtime_lcg_loop(
        &mut self,
        iterations: u64,
        state_init: u64,
        mul: u32,
        add: u32,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    ) {
        // rax = state
        self.emit_mov_rax_imm(state_init);
        self.emit_runtime_lcg_body(
            iterations,
            u64::from(mul),
            u64::from(add),
            exit_with_state,
            exit_mask,
        );
    }

    pub fn emit_runtime_seeded_lcg_loop(
        &mut self,
        iterations: u64,
        mul: u32,
        add: u32,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    ) {
        // rdtsc -> edx:eax
        self.code.extend_from_slice(&[0x0F, 0x31]);
        // shl rdx, 32
        self.code.extend_from_slice(&[0x48, 0xC1, 0xE2, 0x20]);
        // or rax, rdx
        self.code.extend_from_slice(&[0x48, 0x09, 0xD0]);
        self.emit_runtime_lcg_body(
            iterations,
            u64::from(mul),
            u64::from(add),
            exit_with_state,
            exit_mask,
        );
    }

    /// Emits a 16-wide LCG refill followed by the observed terminal prefix sum.
    /// Since the last inclusive-scan element is the sum of every input element,
    /// SSE2 computes the sixteen low-u16 values in parallel without materializing
    /// the temporary array. All x86-64 targets provide SSE2.
    pub fn emit_runtime_prefix_scan_loop(
        &mut self,
        batches: u64,
        state_init: u64,
        mul: u32,
        add: u32,
        state_mask: u64,
        value_mask: u64,
        width: u8,
        exit_mask: u64,
    ) {
        if width != 16 || state_mask != u32::MAX as u64 || value_mask != u16::MAX as u64 {
            self.emit_exit(255);
            return;
        }

        let mut coeffs = [0u16; 16];
        let mut offsets = [0u16; 16];
        let mut coeff = 1u32;
        let mut offset = 0u32;
        for index in 0..16 {
            coeff = coeff.wrapping_mul(mul);
            offset = offset.wrapping_mul(mul).wrapping_add(add);
            coeffs[index] = coeff as u16;
            offsets[index] = offset as u16;
        }
        let vector = |values: &[u16]| {
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>()
        };
        let coeff_lo = vector(&coeffs[..8]);
        let coeff_hi = vector(&coeffs[8..]);
        let offset_lo = vector(&offsets[..8]);
        let offset_hi = vector(&offsets[8..]);
        let (step_mul, step_add) = Self::affine_pow_u64(u64::from(mul), u64::from(add), 16);

        self.emit_mov_reg_imm64(12, state_init); // r12d = state
        self.emit_xor_reg_reg(13, 13); // r13d = checksum
        self.emit_mov_reg_imm64(14, batches); // r14 = remaining batches
        if batches != 0 {
            let loop_start = self.code.len();
            self.emit_movd_xmm_reg32(0, 12);
            self.emit_pshuflw_xmm(0, 0, 0);
            self.emit_punpcklqdq_xmm(0, 0);
            self.emit_movdqa_xmm(1, 0);

            self.emit_movdqu_xmm_rip_data(2, &coeff_lo);
            self.emit_sse2_xmm_xmm(0xD5, 0, 2); // pmullw
            self.emit_movdqu_xmm_rip_data(2, &offset_lo);
            self.emit_sse2_xmm_xmm(0xFD, 0, 2); // paddw
            self.emit_movdqu_xmm_rip_data(2, &coeff_hi);
            self.emit_sse2_xmm_xmm(0xD5, 1, 2);
            self.emit_movdqu_xmm_rip_data(2, &offset_hi);
            self.emit_sse2_xmm_xmm(0xFD, 1, 2);

            self.emit_sse2_xmm_xmm(0xEF, 2, 2); // pxor xmm2, xmm2
            self.emit_movdqa_xmm(3, 0);
            self.emit_sse2_xmm_xmm(0x61, 3, 2); // punpcklwd
            self.emit_sse2_xmm_xmm(0x69, 0, 2); // punpckhwd
            self.emit_sse2_xmm_xmm(0xFE, 3, 0); // paddd
            self.emit_movdqa_xmm(4, 1);
            self.emit_sse2_xmm_xmm(0x61, 4, 2);
            self.emit_sse2_xmm_xmm(0x69, 1, 2);
            self.emit_sse2_xmm_xmm(0xFE, 4, 1);
            self.emit_sse2_xmm_xmm(0xFE, 3, 4);
            self.emit_movdqa_xmm(4, 3);
            self.emit_pshufd_xmm(4, 4, 0x4E);
            self.emit_sse2_xmm_xmm(0xFE, 3, 4);
            self.emit_movdqa_xmm(4, 3);
            self.emit_pshufd_xmm(4, 4, 0xB1);
            self.emit_sse2_xmm_xmm(0xFE, 3, 4);
            self.emit_movd_reg32_xmm(0, 3);
            self.emit_xor_reg_reg32(13, 0);

            self.emit_imul_reg32_reg32_imm32(12, 12, step_mul as u32 as i32);
            self.emit_add_reg32_imm32(12, step_add as u32 as i32);
            self.emit_dec_reg(14);
            self.code.extend_from_slice(&[0x0F, 0x85]); // jnz rel32
            let disp = i32::try_from(loop_start as i64 - (self.code.len() + 4) as i64)
                .expect("prefix-scan loop exceeds rel32");
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
        self.emit_mov_reg32_reg32(0, 12);
        self.emit_xor_reg_reg32(0, 13);
        self.emit_exit_with_rax_or_mask(true, Some(exit_mask));
    }

    /// Emit a ring-write loop: LCG state update + pack + ring buffer store.
        ///
        /// For each iteration:
        ///   state = (state * mul + add) & state_mask
        ///   packed = (state << value_shift) | state
        ///   buf[i & ring_mask] = packed
        ///
        /// Exit: `rax = (packed_final ^ buf[0]) & exit_mask; syscall(60)`
        ///
        /// Register schedule:
        ///   rax = LCG state (updated in place)
        ///   rcx = iteration count (downward)
        ///   r12 = packed temporary
        ///   r13 = ring index (i & ring_mask, wraps naturally at 64)
        ///   rbx = ring-buffer base (= rsp after alloc)
        pub fn emit_runtime_ring_write_loop(
            &mut self,
            iterations: u64,
            state_init: u64,
            index_init: u64,
            mul: u32,
            add: u32,
            state_mask: u64,
            ring_mask: u64,
            value_shift: u8,
            exit_mask: u64,
        ) {
            if iterations == 0 {
                let shift = u32::from(value_shift) & 63;
                let packed = state_init.wrapping_shl(shift) | state_init;
                self.emit_exit(packed & exit_mask);
                return;
            }

            let ring_len = ring_mask + 1;   // 64
            let ring_bytes = ring_len * 8;  // 512
            let stack_alloc = ((ring_bytes as u64 + 15) / 16) * 16;

            // ---------- prologue ----------
            self.code.push(0x55);                               // push rbp
            self.code.extend_from_slice(&[0x48, 0x89, 0xE5]); // mov rbp, rsp
            self.code.extend_from_slice(&[0x48, 0x81, 0xEC]); // sub rsp, N
            self.code.extend_from_slice(&(stack_alloc as u32).to_le_bytes());
            self.code.extend_from_slice(&[0x48, 0x89, 0xE3]); // mov rbx, rsp

            // Preserve source semantics for short loops: the declared ring is
            // zero-initialized even when an element is never overwritten.
            self.code.extend_from_slice(&[0x31, 0xC0]); // xor eax, eax
            self.emit_mov_rcx_imm(ring_len);
            self.code.extend_from_slice(&[0x48, 0x89, 0xDF]); // mov rdi, rbx
            self.code.extend_from_slice(&[0xF3, 0x48, 0xAB]); // rep stosq

            let wide_state_plan = if state_mask == 0xFFFF_FFFF {
                None
            } else {
                Some(self.prepare_affine_step(
                    u64::from(mul),
                    u64::from(add),
                    GpReg::R8,
                    GpReg::R9,
                ))
            };

            // ---------- initial state ----------
            self.emit_mov_rax_imm(state_init);

            // push r12, r13 (callee-save)
            self.code.extend_from_slice(&[0x41, 0x54]); // push r12
            self.code.extend_from_slice(&[0x41, 0x55]); // push r13

            // rcx = iterations (down-counter)
            self.emit_mov_rcx_imm(iterations);
            // r13 = source index modulo ring length
            let initial_ring_index = index_init & ring_mask;
            if initial_ring_index == 0 {
                self.code.extend_from_slice(&[0x45, 0x31, 0xED]); // xor r13d, r13d
            } else {
                self.emit_mov_reg_imm64(13, initial_ring_index);
            }

            // ---------- main loop ----------
            let loop_start = self.code.len();

            // --- LCG step: state = (state * mul + add) & state_mask ---
            // For the common u32-mask LCG, use 32-bit arithmetic: writing EAX
            // zero-extends into RAX and exactly implements `& 0xffffffff`.
            if let Some(plan) = &wide_state_plan {
                self.emit_affine_step(plan);
                self.emit_and_rax_imm(state_mask);
            } else {
                let mul_bytes = (mul as i32).to_le_bytes();
                self.code.extend_from_slice(&[0x69, 0xC0]); // imul eax, eax, imm32
                self.code.extend_from_slice(&mul_bytes);
                if let Ok(add8) = i8::try_from(add as i32) {
                    self.code.extend_from_slice(&[0x83, 0xC0, add8 as u8]); // add eax, imm8
                } else {
                    self.code.extend_from_slice(&[0x05]); // add eax, imm32
                    self.code.extend_from_slice(&add.to_le_bytes());
                }
            }

            // --- pack: r12 = (rax << value_shift) | rax ---
            self.code.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax
            if value_shift > 0 {
                self.code.extend_from_slice(&[0x49, 0xC1, 0xE4, value_shift]); // shl r12, value_shift
            }
            // or r12, rax  (or r12, rax = 49 09 c4: REX.W+REX.B, ModRM.reg=rax, ModRM.rm=r12)
            self.code.extend_from_slice(&[0x49, 0x09, 0xC4]);

            // --- store buf[ring_index] = packed  via [rbx + r13*8] ---
            // mov [rbx + r13*8], r12
            self.code.extend_from_slice(&[0x4E, 0x89, 0x24, 0xEB]); // REX.W+REX.B mov [rbx+r13*8], r12

            // --- ring_index = (ring_index + 1) & ring_mask ---
            self.code.extend_from_slice(&[0x49, 0xFF, 0xC5]); // inc r13
            // and r13d, ring_mask
            if ring_mask <= 0x7F {
                self.code.extend_from_slice(&[0x41, 0x83, 0xE5, ring_mask as u8]); // and r13b, imm8
            } else {
                self.code.extend_from_slice(&[0x49, 0x81, 0xE5]); // and r13, imm32
                self.code.extend_from_slice(&(ring_mask as u32).to_le_bytes());
            }

            // --- dec rcx; jnz loop_start ---
            self.code.extend_from_slice(&[0x48, 0xFF, 0xC9]); // dec rcx
            self.code.extend_from_slice(&[0x0F, 0x85]);         // jnz rel32
            let jnz_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            patch_rel32(&mut self.code, jnz_pos, loop_start);

            // --- epilogue: pop r13, r12 ---
            self.code.extend_from_slice(&[0x41, 0x5D]); // pop r13
            self.code.extend_from_slice(&[0x41, 0x5C]); // pop r12

            // Now rax = final state, rbx still points to ring buffer base
            // Compute exit: packed = (rax << vs) | rax  → rdi
            //            rdi ^= *(uint64_t*)rbx  (buf[0])
            //            rdi &= exit_mask
            //            syscall(60, rdi)
            self.code.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax
            if value_shift > 0 {
                self.code.extend_from_slice(&[0x49, 0xC1, 0xE4, value_shift]); // shl r12, value_shift
            }
            // or r12, rax  → r12 = packed
            // xor r12, [rbx]  (buf[0])
            // xor r12, [rbx] = 4c 33 23: REX.W+REX.R, ModRM.reg=r12, ModRM.rm=[rbx]
            self.code.extend_from_slice(&[0x49, 0x09, 0xC4]); // or r12, rax
            self.code.extend_from_slice(&[0x4C, 0x33, 0x23]); // xor r12, [rbx]
            if self.options.emit_full_checksum {
                self.emit_mov_reg_reg(0, 12);
                self.emit_raw_checksum_from_rax();
            }
            // mov rdi, r12 = 4c 89 e7: REX.W+REX.R, ModRM.reg=r12, ModRM.rm=rdi
            self.code.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12

            // and rdi, exit_mask
            if exit_mask <= 0x7F {
                self.code.push(0x40); // REX prefix (for rdi which is not r8+)
                self.code.extend_from_slice(&[0x83, 0xE7, exit_mask as u8]);
            } else if let Some(imm32) = imm32_sign_extended(exit_mask) {
                self.code.extend_from_slice(&[0x48, 0x81, 0xE7]); // and rdi, imm32
                self.code.extend_from_slice(&imm32.to_le_bytes());
            } else {
                self.emit_mov_rcx_imm(exit_mask);
                self.code.extend_from_slice(&[0x48, 0x21, 0xCF]); // and rdi, rcx
            }

            // syscall(60)
            self.emit_mov_rax_imm(self.runtime.syscalls.exit);
            self.emit_kernel_call();

            // epilogue: leave (unreachable)
            self.code.extend_from_slice(&[0x48, 0x8B, 0xE5]); // mov rsp, rbp
            self.code.push(0x5D);                               // pop rbp
            self.code.extend_from_slice(&[0xC3]);
        }

    pub fn emit_runtime_bloom_filter_loop(
        &mut self,
        state_init: u64,
        build_iterations: u64,
        query_iterations: u64,
        hits_init: u64,
        exit_mask: u64,
    ) {
        const FILTER_WORDS: u32 = 256;
        const FILTER_BYTES: u32 = FILTER_WORDS * 8;

        self.code.push(0x55); // push rbp
        self.code.extend_from_slice(&[0x48, 0x89, 0xE5]); // mov rbp, rsp
        self.code.extend_from_slice(&[0x48, 0x81, 0xEC]); // sub rsp, 2048
        self.code.extend_from_slice(&FILTER_BYTES.to_le_bytes());
        self.code.extend_from_slice(&[0x48, 0x89, 0xE3]); // mov rbx, rsp

        // Preserve the source [0; 256] initialization even for zero-trip builds.
        self.code.extend_from_slice(&[0x31, 0xC0]); // xor eax, eax
        self.code.push(0xB9); // mov ecx, 256
        self.code.extend_from_slice(&FILTER_WORDS.to_le_bytes());
        self.code.extend_from_slice(&[0x48, 0x89, 0xDF]); // mov rdi, rbx
        self.code.extend_from_slice(&[0xF3, 0x48, 0xAB]); // rep stosq

        self.emit_mov_reg_imm64(13, state_init); // r13 = u32 LCG state
        self.emit_mov_reg_imm64(15, hits_init); // r15 = hit count

        if build_iterations != 0 {
            self.emit_mov_reg_imm64(12, build_iterations);
            let build_loop = self.code.len();
            self.emit_bloom_lcg_step_r13d();
            self.code.extend_from_slice(&[0x45, 0x89, 0xEE]); // mov r14d, r13d (hi)
            self.emit_bloom_lcg_step_r13d(); // r13d = lo
            for lane in 0..4 {
                self.emit_bloom_classic_lane_address(lane);
                self.code.extend_from_slice(&[0x48, 0x8B, 0x04, 0xD3]); // mov rax, [rbx+rdx*8]
                self.emit_bts_rax_rcx();
                self.code.extend_from_slice(&[0x48, 0x89, 0x04, 0xD3]); // mov [rbx+rdx*8], rax
            }
            self.code.extend_from_slice(&[0x49, 0xFF, 0xCC]); // dec r12
            self.code.extend_from_slice(&[0x0F, 0x85]); // jnz build_loop
            let patch = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            patch_rel32(&mut self.code, patch, build_loop);
        }

        if query_iterations != 0 {
            let pairs = query_iterations / 2;
            if pairs != 0 {
                self.emit_mov_reg_imm64(12, pairs);
                let query_loop = self.code.len();

                // Advance the loop-carried state with f^2.  Two queries are
                // software-pipelined together, so the critical recurrence has
                // one multiply per query instead of two.  The predecessor bits
                // required by lane 3 are recovered exactly modulo 2^10.
                self.emit_bloom_composed_lcg_step_r13d(); // r13d = lo1
                self.emit_mov_reg32_reg32(9, 13); // r9d = lo1
                self.emit_bloom_composed_lcg_step_r13d(); // r13d = lo2
                self.emit_bloom_recover_hi10(10, 9); // r10d = demanded hi1 bits
                self.emit_bloom_recover_hi10(14, 13); // r14d = demanded hi2 bits
                self.emit_bloom_query_from_regs(9, 10);
                self.emit_bloom_query_from_regs(13, 14);

                self.code.extend_from_slice(&[0x49, 0xFF, 0xCC]); // dec r12
                self.code.extend_from_slice(&[0x0F, 0x85]); // jnz query_loop
                let patch = self.code.len();
                self.code.extend_from_slice(&0_i32.to_le_bytes());
                patch_rel32(&mut self.code, patch, query_loop);
            }
            if query_iterations & 1 != 0 {
                self.emit_bloom_query_iteration();
            }
        }

        if self.options.emit_full_checksum {
            self.emit_mov_reg_reg(0, 15);
            self.emit_raw_checksum_from_rax();
        }
        self.code.extend_from_slice(&[0x4C, 0x89, 0xFF]); // mov rdi, r15
        if let Some(mask) = imm32_sign_extended(exit_mask) {
            self.code.extend_from_slice(&[0x48, 0x81, 0xE7]); // and rdi, imm32
            self.code.extend_from_slice(&mask.to_le_bytes());
        } else {
            self.emit_mov_rcx_imm(exit_mask);
            self.code.extend_from_slice(&[0x48, 0x21, 0xCF]); // and rdi, rcx
        }
        self.emit_mov_rax_imm(self.runtime.syscalls.exit);
        self.emit_kernel_call(); // syscall
    }

    fn emit_bloom_query_iteration(&mut self) {
        self.emit_bloom_composed_lcg_step_r13d();
        self.emit_bloom_recover_hi10(14, 13);
        self.emit_bloom_query_from_regs(13, 14);
    }

    fn emit_bloom_query_from_regs(&mut self, lo_reg: u8, hi_reg: u8) {
        self.code.extend_from_slice(&[0x41, 0xB8, 1, 0, 0, 0]); // mov r8d, 1
        for lane in 0..4 {
            self.emit_bloom_classic_lane_address_regs(lane, lo_reg, hi_reg);
            if self.options.target_features.bmi2 {
                // shrx rax, [rbx+rdx*8], rcx.  This folds the filter load,
                // masks the shift count modulo 64, and leaves the selected
                // bit in bit zero without materializing flags as a boolean.
                self.code
                    .extend_from_slice(&[0xC4, 0xE2, 0xF3, 0xF7, 0x04, 0xD3]);
            } else {
                self.code.extend_from_slice(&[0x48, 0x8B, 0x04, 0xD3]); // mov rax, [rbx+rdx*8]
                self.emit_bt_rax_rcx();
                self.code.extend_from_slice(&[0x0F, 0x92, 0xC0]); // setb al
                self.code.extend_from_slice(&[0x0F, 0xB6, 0xC0]); // movzx eax, al
            }
            self.code.extend_from_slice(&[0x49, 0x21, 0xC0]); // and r8, rax
        }
        self.code.extend_from_slice(&[0x4D, 0x01, 0xC7]); // add r15, r8
    }

    fn emit_bloom_composed_lcg_step_r13d(&mut self) {
        const MUL: u32 = 1_664_525;
        const ADD: u32 = 1_013_904_223;
        const MUL2: u32 = MUL.wrapping_mul(MUL);
        const ADD2: u32 = ADD.wrapping_mul(MUL.wrapping_add(1));
        self.emit_imul_reg32_reg32_imm32(13, 13, MUL2 as i32);
        self.emit_add_reg32_imm32(13, ADD2 as i32);
    }

    fn emit_bloom_recover_hi10(&mut self, dst_reg: u8, lo_reg: u8) {
        // MUL is odd, so it has an inverse modulo every 2^k.  For k=10:
        // inv(1664525) = 197 and ADD mod 1024 = 863.  Lane 3 observes only
        // predecessor bits 4..9, so higher reconstructed bits are irrelevant.
        self.emit_mov_reg32_reg32(dst_reg, lo_reg);
        self.emit_add_reg32_imm32(dst_reg, -(1_013_904_223_i32));
        self.emit_imul_reg32_reg32_imm32(dst_reg, dst_reg, 197);
    }

    fn emit_bloom_lcg_step_r13d(&mut self) {
        self.code.extend_from_slice(&[0x45, 0x69, 0xED]); // imul r13d, r13d, imm32
        self.code.extend_from_slice(&1_664_525u32.to_le_bytes());
        self.code.extend_from_slice(&[0x41, 0x81, 0xC5]); // add r13d, imm32
        self.code.extend_from_slice(&1_013_904_223u32.to_le_bytes());
    }

    fn emit_bloom_classic_lane_address(&mut self, lane: u8) {
        self.emit_bloom_classic_lane_address_regs(lane, 13, 14);
    }

    fn emit_bloom_classic_lane_address_regs(&mut self, lane: u8, lo_reg: u8, hi_reg: u8) {
        self.emit_mov_reg32_reg32(2, lo_reg); // edx = low hash half
        let word_shift = lane * 8;
        if word_shift != 0 {
            self.code.extend_from_slice(&[0xC1, 0xEA, word_shift]); // shr edx, imm8
        }
        if lane != 3 {
            self.code.extend_from_slice(&[0x81, 0xE2]); // and edx, 255
            self.code.extend_from_slice(&255u32.to_le_bytes());
        }

        // BT/BTS register forms mask the bit index modulo 64, exactly
        // implementing the source `& 63` without a separate instruction.
        if lane == 3 {
            self.emit_mov_reg32_reg32(1, hi_reg);
            self.code.extend_from_slice(&[0xC1, 0xE9, 4]); // shr ecx, 4
        } else {
            self.emit_mov_reg32_reg32(1, lo_reg);
            let bit_shift = [3u8, 14, 25][lane as usize];
            self.code.extend_from_slice(&[0xC1, 0xE9, bit_shift]); // shr ecx, imm8
        }
    }

    pub fn emit_runtime_branch_lcg_loop(
        &mut self,
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
    ) {
        self.emit_mov_rax_imm(state_init);
        self.emit_runtime_unpredictable_branch_lcg_body(
            iterations,
            state_mask,
            threshold,
            then_mul,
            then_add,
            else_mul,
            else_add,
            exit_with_state,
            exit_mask,
        );
    }

    pub fn emit_runtime_seeded_lcg_alloc_loop(
        &mut self,
        iterations: u64,
        mul: u32,
        add: u32,
        alloc_bytes: u64,
        exit_with_state: bool,
    ) {
        let mask = alloc_bytes - 1;
        let wrap16_index = alloc_bytes == 65_536;

        // rax = mmap(NULL, alloc_bytes, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)
        self.emit_mov_rax_imm(self.runtime.syscalls.mmap);
        self.emit_mov_rdi_imm(0);
        self.emit_mov_rsi_imm(alloc_bytes);
        self.emit_mov_rdx_imm(self.runtime.prot_read_write);
        self.code.extend_from_slice(&[0x41, 0xBA]); // mov r10d, imm32
        self.code
            .extend_from_slice(&(self.runtime.mmap_private_anonymous as u32).to_le_bytes());
        self.emit_mov_reg_imm(GpReg::R8, u64::MAX); // fd = -1
        self.code.extend_from_slice(&[0x45, 0x31, 0xC9]); // xor r9d, r9d
        self.emit_kernel_call(); // syscall

        // if rax < 0 -> alloc_fail
        self.code.extend_from_slice(&[0x48, 0x85, 0xC0]); // test rax, rax
        self.code.extend_from_slice(&[0x0F, 0x88]); // js rel32
        let js_alloc_fail_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        self.code.extend_from_slice(&[0x48, 0x89, 0xC3]); // mov rbx, rax

        self.code.extend_from_slice(&[0x45, 0x31, 0xDB]); // xor r11d, r11d (ring index)
        if !wrap16_index {
            self.emit_mov_reg_imm(GpReg::R9, mask);
        }

        if iterations > 0 {
            let (a1, b1) = (u64::from(mul), u64::from(add));
            let (a2, b2) = Self::affine_pow_u64(a1, b1, 2);
            let (a3, b3) = Self::affine_pow_u64(a1, b1, 3);
            let (a4, b4) = Self::affine_pow_u64(a1, b1, 4);

            let plan1 = self.prepare_affine_step(a1, b1, GpReg::R8, GpReg::R9);
            let plan2 = self.prepare_affine_step(a2, b2, GpReg::R10, GpReg::R12);
            let plan3 = self.prepare_affine_step(a3, b3, GpReg::R13, GpReg::R14);
            // plan4 must NOT use R11 as a temporary because R11 is the ring index!
            let plan4 = self.prepare_affine_step(a4, b4, GpReg::R15, GpReg::R10); // R10 is a scratch used by plan2, safe to reuse here between plan4 steps? No, better use R11 only if we are careful. Wait, plan4 is used in emit_affine_step which uses the 'temp' register.
            // Let's use R10 as the temporary for plan4 as well, since plan2 is done by the time plan4 runs.
            // Actually, prepare_affine_step(val, a, b, temp)
            // plan4 uses r15 as result and r10 as temp.
            // plan2 uses r10 as result and r12 as temp.
            // This is safe.

            let blocks = iterations / 4;
            let tail = iterations % 4;

            if blocks > 0 {
                self.emit_mov_rcx_imm(blocks);
                let loop_start = self.code.len();
                
                // ILP lookahead: compute 4 states in parallel starting from RAX
                self.emit_mov_reg_reg(8, 0); // r8 = rax
                self.emit_affine_step_to_reg(8, &plan1);
                self.emit_mov_reg_reg(10, 0); // r10 = rax
                self.emit_affine_step_to_reg(10, &plan2);
                self.emit_mov_reg_reg(13, 0); // r13 = rax
                self.emit_affine_step_to_reg(13, &plan3);
                self.emit_affine_step(&plan4); // rax = rax * a4 + b4 (new base)

                // Store 4 results
                self.emit_store_reg_to_ring(8, 0, wrap16_index);
                self.emit_store_reg_to_ring(10, 8, wrap16_index);
                self.emit_store_reg_to_ring(13, 16, wrap16_index);
                self.emit_store_reg_to_ring(0, 24, wrap16_index); // latest rax is state4
                
                // Advanced pointer increment: we stored 4 items (32 bytes)
                self.emit_add_r11_imm(32, wrap16_index);

                self.code.extend_from_slice(&[0x48, 0xFF, 0xC9]); // dec rcx
                self.code.extend_from_slice(&[0x0F, 0x85]); // jnz
                let jnz_pos = self.code.len();
                self.code.extend_from_slice(&0_i32.to_le_bytes());
                patch_rel32(&mut self.code, jnz_pos, loop_start);
            }

            for _ in 0..tail {
                self.emit_touch_step(&plan1, wrap16_index);
            }
        }

        // Explicitly unmap before exit so allocator-path benchmarks include cleanup.
        if exit_with_state {
            self.code.extend_from_slice(&[0x48, 0x89, 0xC2]); // mov rdx, rax
        }
        self.emit_mov_rax_imm(self.runtime.syscalls.munmap); // SYS_munmap
        self.code.extend_from_slice(&[0x48, 0x89, 0xDF]); // mov rdi, rbx
        self.emit_mov_rsi_imm(alloc_bytes);
        self.emit_kernel_call(); // syscall
        if exit_with_state {
            self.code.extend_from_slice(&[0x48, 0x89, 0xD0]); // mov rax, rdx
        }

        self.emit_exit_with_rax_or_zero(exit_with_state);

        let alloc_fail = self.code.len();
        self.emit_mov_rdi_imm(1);
        self.emit_mov_rax_imm(self.runtime.syscalls.exit);
        self.emit_kernel_call();

        patch_rel32(&mut self.code, js_alloc_fail_pos, alloc_fail);
    }

    pub fn emit_runtime_seeded_predictable_branch_lcg_loop(
        &mut self,
        iterations: u64,
        then_iterations: u64,
        then_mul: u32,
        then_add: u32,
        else_mul: u32,
        else_add: u32,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    ) {
        // rdtsc -> edx:eax
        self.code.extend_from_slice(&[0x0F, 0x31]);
        // shl rdx, 32
        self.code.extend_from_slice(&[0x48, 0xC1, 0xE2, 0x20]);
        // or rax, rdx
        self.code.extend_from_slice(&[0x48, 0x09, 0xD0]);

        if iterations == 0 {
            self.emit_exit_with_rax_or_mask(exit_with_state, exit_mask);
            return;
        }

        let then_iterations = then_iterations.min(iterations);
        if then_iterations == 0 {
            self.emit_runtime_lcg_compute(
                iterations,
                u64::from(else_mul),
                u64::from(else_add),
                64,
            );
            self.emit_exit_with_rax_or_mask(exit_with_state, exit_mask);
            return;
        }
        if then_iterations == iterations {
            self.emit_runtime_lcg_compute(
                iterations,
                u64::from(then_mul),
                u64::from(then_add),
                64,
            );
            self.emit_exit_with_rax_or_mask(exit_with_state, exit_mask);
            return;
        }
        self.emit_runtime_lcg_compute(
            then_iterations,
            u64::from(then_mul),
            u64::from(then_add),
            64,
        );
        self.emit_runtime_lcg_compute(
            iterations - then_iterations,
            u64::from(else_mul),
            u64::from(else_add),
            64,
        );
        self.emit_exit_with_rax_or_mask(exit_with_state, exit_mask);
    }

    pub fn emit_runtime_seeded_unpredictable_branch_lcg_loop(
        &mut self,
        iterations: u64,
        threshold: u64,
        then_mul: u32,
        then_add: u32,
        else_mul: u32,
        else_add: u32,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    ) {
        // rdtsc -> edx:eax
        self.code.extend_from_slice(&[0x0F, 0x31]);
        // shl rdx, 32
        self.code.extend_from_slice(&[0x48, 0xC1, 0xE2, 0x20]);
        // or rax, rdx
        self.code.extend_from_slice(&[0x48, 0x09, 0xD0]);

        self.emit_runtime_unpredictable_branch_lcg_body(
            iterations,
            u64::MAX,
            threshold,
            then_mul,
            then_add,
            else_mul,
            else_add,
            exit_with_state,
            exit_mask,
        );
    }

    fn emit_runtime_unpredictable_branch_lcg_body(
        &mut self,
        iterations: u64,
        state_mask: u64,
        threshold: u64,
        then_mul: u32,
        then_add: u32,
        else_mul: u32,
        else_add: u32,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    ) {

        if iterations == 0 {
            self.emit_exit_with_rax_or_mask(exit_with_state, exit_mask);
            return;
        }

        // Coefficient-select branch kernel:
        // choose (mul, add) via cmov, then perform one affine step.
        self.code.extend_from_slice(&[0x41, 0xB8]); // mov r8d, then_mul
        self.code.extend_from_slice(&then_mul.to_le_bytes());
        self.code.extend_from_slice(&[0x41, 0xB9]); // mov r9d, else_mul
        self.code.extend_from_slice(&else_mul.to_le_bytes());
        self.code.extend_from_slice(&[0x41, 0xBA]); // mov r10d, then_add
        self.code.extend_from_slice(&then_add.to_le_bytes());
        self.code.extend_from_slice(&[0x41, 0xBB]); // mov r11d, else_add
        self.code.extend_from_slice(&else_add.to_le_bytes());

        self.emit_mov_rcx_imm(iterations);
        self.emit_mov_rdx_imm(threshold);

        let blocks = iterations / 4;
        let tail = iterations % 4;
        if blocks > 0 {
            self.emit_mov_rcx_imm(blocks);
            let loop_start = self.code.len();
            for _ in 0..4 {
                self.emit_unpredictable_branch_lcg_step_select_coeff(state_mask);
            }
            self.code.extend_from_slice(&[0x48, 0xFF, 0xC9]); // dec rcx
            self.code.extend_from_slice(&[0x0F, 0x85]); // jnz rel32
            let jnz_loop_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            patch_rel32(&mut self.code, jnz_loop_pos, loop_start);
        }

        for _ in 0..tail {
            self.emit_unpredictable_branch_lcg_step_select_coeff(state_mask);
        }

        self.emit_exit_with_rax_or_mask(exit_with_state, exit_mask);
    }

    pub fn emit_runtime_affine_index_loop(
        &mut self,
        iterations: u64,
        state_init: u64,
        index_init: u64,
        state_mul: u32,
        index_mul: u32,
        add: u32,
        state_mask: u64,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    ) {
        self.emit_mov_rax_imm(state_init);
        self.emit_runtime_affine_index_body(
            iterations,
            index_init,
            state_mul,
            index_mul,
            add,
            state_mask,
            exit_with_state,
            exit_mask,
        );
    }

    pub fn emit_runtime_seeded_affine_index_loop(
        &mut self,
        iterations: u64,
        index_init: u64,
        state_mul: u32,
        index_mul: u32,
        add: u32,
        state_mask: u64,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    ) {
        // rdtsc -> edx:eax
        self.code.extend_from_slice(&[0x0F, 0x31]);
        // shl rdx, 32
        self.code.extend_from_slice(&[0x48, 0xC1, 0xE2, 0x20]);
        // or rax, rdx
        self.code.extend_from_slice(&[0x48, 0x09, 0xD0]);

        self.emit_runtime_affine_index_body(
            iterations,
            index_init,
            state_mul,
            index_mul,
            add,
            state_mask,
            exit_with_state,
            exit_mask,
        );
    }

    fn emit_runtime_affine_index_body(
        &mut self,
        iterations: u64,
        index_init: u64,
        state_mul: u32,
        index_mul: u32,
        add: u32,
        state_mask: u64,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    ) {

        if iterations == 0 {
            self.emit_exit_with_rax_or_mask(exit_with_state, exit_mask);
            return;
        }

        const CHUNK: u64 = 32;
        let step_a = u64::from(state_mul);
        let step_b = u64::from(index_mul);
        let step_c = (add as i32 as i64) as u64;
        let (chunk_a, chunk_b, chunk_c) =
            Self::affine_index_chunk(step_a, step_b, step_c, CHUNK);

        let chunk_state_plan =
            self.prepare_affine_step(chunk_a, chunk_c, GpReg::R8, GpReg::R9);
        let chunk_index_plan = self.prepare_coeff_plan(chunk_b, GpReg::R10);
        let tail_state_plan =
            self.prepare_affine_step(step_a, step_c, GpReg::R12, GpReg::R13);
        let tail_index_plan = self.prepare_coeff_plan(step_b, GpReg::R14);

        self.emit_mov_rdx_imm(index_init);

        let blocks = iterations / CHUNK;
        let tail = iterations % CHUNK;
        if blocks > 0 {
            self.emit_mov_rcx_imm(blocks);
            let loop_start = self.code.len();
            self.emit_affine_step(&chunk_state_plan);
            self.emit_add_index_term(&chunk_index_plan);
            if state_mask != u64::MAX {
                self.emit_and_rax_imm(state_mask);
            }
            self.emit_add_reg_imm32(2, CHUNK as i32);
            self.code.extend_from_slice(&[0x48, 0xFF, 0xC9]); // dec rcx
            self.code.extend_from_slice(&[0x0F, 0x85]); // jnz rel32
            let jnz_loop_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            patch_rel32(&mut self.code, jnz_loop_pos, loop_start);
        }

        for _ in 0..tail {
            self.emit_affine_step(&tail_state_plan);
            self.emit_add_index_term(&tail_index_plan);
            if state_mask != u64::MAX {
                self.emit_and_rax_imm(state_mask);
            }
            self.code.extend_from_slice(&[0x48, 0xFF, 0xC2]); // inc rdx
        }

        self.emit_exit_with_rax_or_mask(exit_with_state, exit_mask);
    }

    fn affine_index_chunk(
        step_a: u64,
        step_b: u64,
        step_c: u64,
        iterations: u64,
    ) -> (u64, u64, u64) {
        let mut a = 1u64;
        let mut b = 0u64;
        let mut c = 0u64;
        for completed in 0..iterations {
            a = step_a.wrapping_mul(a);
            b = step_a.wrapping_mul(b).wrapping_add(step_b);
            c = step_a
                .wrapping_mul(c)
                .wrapping_add(step_b.wrapping_mul(completed))
                .wrapping_add(step_c);
        }
        (a, b, c)
    }

    pub fn emit_runtime_seeded_dual_state_branch_loop(
        &mut self,
        iterations: u64,
        index_init: u64,
        adaptive: bool,
        branchless: bool,
        exit_with_sum: bool,
    ) {
        // seed a in rax
        self.code.extend_from_slice(&[0x0F, 0x31]); // rdtsc
        self.code.extend_from_slice(&[0x48, 0xC1, 0xE2, 0x20]); // shl rdx, 32
        self.code.extend_from_slice(&[0x48, 0x09, 0xD0]); // or rax, rdx
        self.emit_mov_reg_reg(12, 0); // r12 = a

        // seed b in rbx
        self.code.extend_from_slice(&[0x0F, 0x31]); // rdtsc
        self.code.extend_from_slice(&[0x48, 0xC1, 0xE2, 0x20]); // shl rdx, 32
        self.code.extend_from_slice(&[0x48, 0x09, 0xD0]); // or rax, rdx
        self.code.extend_from_slice(&[0x48, 0x89, 0xC3]); // mov rbx, rax
        self.emit_mov_reg_reg(0, 12); // rax = a

        if iterations == 0 {
            if exit_with_sum {
                self.code.extend_from_slice(&[0x48, 0x01, 0xD8]); // add rax, rbx
                self.emit_exit_with_rax_or_zero(true);
            } else {
                self.emit_exit_with_rax_or_zero(false);
            }
            return;
        }

        self.emit_mov_rdx_imm(index_init);
        const ADAPTIVE_MIN_ITERS: u64 = 256;
        const ADAPTIVE_SAMPLE_ITERS: u64 = 4096;
        if adaptive && !branchless && iterations >= ADAPTIVE_MIN_ITERS {
            let sample_iters = iterations.min(ADAPTIVE_SAMPLE_ITERS);
            let remaining_iters = iterations - sample_iters;

            // Warmup/sample phase to estimate branch entropy.
            // r14: previous condition bit, r15: condition transition count.
            self.emit_xor_reg_reg(14, 14);
            self.emit_xor_reg_reg(15, 15);
            self.emit_mov_rcx_imm(sample_iters);
            let sample_loop_start = self.code.len();

            self.emit_cmp_reg_reg(0, 3); // cmp a, b
            self.emit_xor_reg_reg(13, 13);
            self.emit_setb_reg8(13); // r13 = cond(a < b)
            self.emit_mov_reg_reg(12, 13); // snapshot cond
            self.emit_xor_reg_reg(13, 14); // delta = cond ^ prev_cond
            self.emit_binop_reg_reg_in_place(RuntimeBinOp::Add, 15, 13); // transitions += delta
            self.emit_mov_reg_reg(14, 12); // prev_cond = cond

            self.emit_dual_state_step(false);
            self.code.extend_from_slice(&[0x48, 0xFF, 0xC2]); // inc rdx
            self.code.extend_from_slice(&[0x48, 0xFF, 0xC9]); // dec rcx
            self.code.extend_from_slice(&[0x0F, 0x85]); // jnz rel32
            let jnz_sample_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            patch_rel32(&mut self.code, jnz_sample_pos, sample_loop_start);

            // If transitions are high, treat the branch as unpredictable and
            // switch to branchless execution for the remaining iterations.
            //
            // transitions ~= count(cond_i != cond_{i-1}). For random branches
            // this tends to about 50% of samples, while strongly predictable
            // branches are close to 0%. We use a 33% cutoff.
            self.emit_mov_reg_reg(13, 15); // r13 = transitions
            self.emit_shl_reg_imm8(13, 1); // transitions * 2
            self.emit_binop_reg_reg_in_place(RuntimeBinOp::Add, 13, 15); // transitions * 3
            let threshold = i32::try_from(sample_iters).expect("adaptive threshold must fit i32");
            self.emit_alu_reg_imm(7, 13, threshold.to_le_bytes()); // cmp r13, threshold
            self.code.extend_from_slice(&[0x0F, 0x87]); // ja rel32
            let ja_branchless_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());

            self.emit_dual_state_iterations(remaining_iters, false);
            self.code.push(0xE9); // jmp rel32
            let jmp_after_dual_loop_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());

            let branchless_start = self.code.len();
            self.emit_dual_state_iterations(remaining_iters, true);
            let after_dual_loop = self.code.len();
            patch_rel32(&mut self.code, ja_branchless_pos, branchless_start);
            patch_rel32(&mut self.code, jmp_after_dual_loop_pos, after_dual_loop);
        } else {
            self.emit_dual_state_iterations(iterations, branchless);
        }

        if exit_with_sum {
            self.code.extend_from_slice(&[0x48, 0x01, 0xD8]); // add rax, rbx
            self.emit_exit_with_rax_or_zero(true);
        } else {
            self.emit_exit_with_rax_or_zero(false);
        }
    }

    pub fn emit_runtime_seeded_affine_closed_form(
        &mut self,
        state_mul: u64,
        add: u64,
        exit_with_state: bool,
    ) {
        if state_mul == 0 {
            self.emit_mov_rax_imm(add);
            self.emit_exit_with_rax_or_zero(exit_with_state);
            return;
        }

        // rdtsc -> edx:eax
        self.code.extend_from_slice(&[0x0F, 0x31]);
        // shl rdx, 32
        self.code.extend_from_slice(&[0x48, 0xC1, 0xE2, 0x20]);
        // or rax, rdx
        self.code.extend_from_slice(&[0x48, 0x09, 0xD0]);

        let plan = self.prepare_affine_step(state_mul, add, GpReg::R8, GpReg::R9);
        self.emit_affine_step(&plan);
        self.emit_exit_with_rax_or_zero(exit_with_state);
    }

    pub fn emit_runtime_seeded_struct_latency_loop(
        &mut self,
        iterations: u64,
        mul: u32,
        add: u32,
        exit_with_sum: bool,
    ) {
        // rdtsc -> edx:eax
        self.code.extend_from_slice(&[0x0F, 0x31]);
        // shl rdx, 32
        self.code.extend_from_slice(&[0x48, 0xC1, 0xE2, 0x20]);
        // or rax, rdx
        self.code.extend_from_slice(&[0x48, 0x09, 0xD0]);

        if iterations == 0 {
            self.emit_exit(0);
            return;
        }

        let state_plan = self.prepare_affine_step(
            u64::from(mul),
            u64::from(add),
            GpReg::R10,
            GpReg::R11,
        );

        // rdx=a, rbx=b, r9=d
        self.emit_xor_reg_reg(2, 2);
        self.emit_xor_reg_reg(3, 3);
        self.emit_xor_reg_reg(9, 9);
        self.emit_mov_rcx_imm(iterations);

        let loop_start = self.code.len();
        self.emit_affine_step(&state_plan); // state = state * mul + add (in rax)
        self.emit_add_reg_reg(2, 0); // a += state
        self.emit_xor_reg_reg(3, 0); // b ^= state
        self.emit_xor_reg_reg(9, 2); // d ^= a
        self.emit_xor_reg_reg(2, 9); // a ^= d
        self.code.extend_from_slice(&[0x48, 0xFF, 0xC9]); // dec rcx
        self.code.extend_from_slice(&[0x0F, 0x85]); // jnz rel32
        let jnz_loop_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        patch_rel32(&mut self.code, jnz_loop_pos, loop_start);

        if exit_with_sum {
            self.emit_mov_reg_reg(0, 2); // rax = a
            self.emit_add_reg_reg(0, 3); // rax += b
            if let Some(imm32) = imm32_non_negative(iterations) {
                self.emit_add_reg_imm32(0, imm32); // rax += c (c == iterations)
            } else {
                self.emit_mov_reg_imm64(8, iterations);
                self.emit_add_reg_reg(0, 8);
            }
            self.emit_add_reg_reg(0, 9); // rax += d
            self.emit_exit_with_rax_or_zero(true);
        } else {
            self.emit_exit(0);
        }
    }

    pub fn emit_runtime_generic_program(&mut self, program: &RuntimeProgram) {
        let optimized = optimize_runtime_program(
            program,
            self.options.runtime_generic_profile.as_ref(),
            &self.options.target_cpu,
        );
        let program = &optimized.program;
        let effective_profile = optimized.profile.as_ref();
        let mut observed_program = None;
        if self.options.preserve_full_checksum || self.options.emit_full_checksum {
            let mut observed = program.clone();
            for index in 0..program.instrs.len() {
                if let Some(full) = full_width_exit_operand(program, index) {
                    observed.instrs[index] = RuntimeInstr::Exit { code: full };
                }
            }
            observed_program = Some(observed);
        }
        let lir_input = observed_program.as_ref().unwrap_or(program);
        let lir = MachineLIRProgram::lower(lir_input, effective_profile)
            .unwrap_or_else(|_| {
                MachineLIRProgram::lower(lir_input, None).expect("runtime generic LIR lowering")
            });
        lir.verify(program.instrs.len())
            .expect("runtime generic MachineLIR verification");
        // Native workers inherit registers at clone time but immediately move
        // onto an isolated frame. Keeping every semantic slot in that frame
        // makes the transfer complete and prevents compiler-owned state from
        // being shared accidentally between parent and worker.
        let uses_threads = program.instrs.iter().any(|instr| {
            matches!(
                instr,
                RuntimeInstr::ThreadSpawn { .. }
                    | RuntimeInstr::ThreadJoin { .. }
                    | RuntimeInstr::ChannelCreate { .. }
                    | RuntimeInstr::ChannelSend { .. }
                    | RuntimeInstr::ChannelRecv { .. }
                    | RuntimeInstr::ChannelClose { .. }
                    | RuntimeInstr::ChannelDestroy { .. }
            )
        });
        let slot_map = if uses_threads {
            RuntimeSlotMap::stack_only(lir.slot_count)
        } else {
            RuntimeSlotMap::from_lir(&lir)
        };

        let uses_shared_allocator = program.instrs.iter().any(|instr| {
            matches!(
                instr,
                RuntimeInstr::Alloc { dst, .. }
                    if slot_map.promoted_alloc_disp(*dst).is_none()
            )
        });
        let allocator_frame = uses_shared_allocator.then(|| {
            let base = slot_map.stack_bytes() as i32;
            RuntimeAllocatorFrame {
                head_disp: -(base + 8),
                cursor_disp: -(base + 16),
                end_disp: -(base + 24),
            }
        });
        let allocator_frame_bytes = usize::from(uses_shared_allocator) * 24;

        let profile_payload_bytes = if self.options.profile_instrument {
            16usize.saturating_add(lir.blocks.len().saturating_mul(8))
        } else {
            0
        };
        let profile_base_disp = -(slot_map
            .stack_bytes()
            .saturating_add(allocator_frame_bytes)
            .saturating_add(profile_payload_bytes) as i32);
        let thread_context_disp = uses_threads.then(|| {
            -(slot_map
                .stack_bytes()
                .saturating_add(allocator_frame_bytes)
                .saturating_add(profile_payload_bytes)
                .saturating_add(8) as i32)
        });
        let uses_entry_state = program.instrs.iter().any(|instr| {
            matches!(
                instr,
                RuntimeInstr::LoadSeed {
                    kind: RuntimeLoadKind::ArgumentCount | RuntimeLoadKind::EntryStackPointer,
                    ..
                }
            )
        });
        let mut stack_size = (slot_map.stack_bytes()
            + allocator_frame_bytes
            + profile_payload_bytes
            + usize::from(uses_threads) * 8
            + 15)
            / 16
            * 16;
        if uses_entry_state && stack_size == 0 {
            stack_size = 16;
        }
        if stack_size > 0 {
            // One whole-program frame is shared by direct internal calls.
            self.code.push(0x55); // push rbp
            self.code.extend_from_slice(&[0x48, 0x89, 0xE5]); // mov rbp, rsp
            self.code.extend_from_slice(&[0x48, 0x81, 0xEC]); // sub rsp, imm32
            self.code
                .extend_from_slice(&(stack_size as u32).to_le_bytes());
        }
        if self.options.profile_instrument {
            self.emit_lea_rdi_rbp_disp(profile_base_disp);
            self.code.extend_from_slice(&[0x31, 0xC0]); // xor eax, eax
            self.code.push(0xB9); // mov ecx, qword count
            self.code
                .extend_from_slice(&((profile_payload_bytes / 8) as u32).to_le_bytes());
            self.code.extend_from_slice(&[0xF3, 0x48, 0xAB]); // rep stosq
            self.emit_mov_rax_imm(u64::from_le_bytes(CompileProfile::INSTRUMENTATION_MAGIC));
            self.emit_mov_rbp_disp_rax(profile_base_disp);
            self.emit_mov_rax_imm(lir.blocks.len() as u64);
            self.emit_mov_rbp_disp_rax(profile_base_disp + 8);
        }
        if let Some(disp) = thread_context_disp {
            self.emit_xor_reg_reg(0, 0);
            self.emit_mov_rbp_disp_rax(disp);
        }
        if let Some(frame) = allocator_frame {
            self.emit_xor_reg_reg(0, 0);
            self.emit_mov_rbp_disp_rax(frame.head_disp);
            self.emit_mov_rbp_disp_rax(frame.cursor_disp);
            self.emit_mov_rbp_disp_rax(frame.end_disp);
            self.runtime_allocator = Some(RuntimeAllocatorEmission {
                frame,
                alloc_call_patches: Vec::new(),
                free_call_patches: Vec::new(),
                teardown_call_patches: Vec::new(),
            });
        }

        let mut instr_offsets = vec![0usize; program.instrs.len()];
        let exact_unroll_plan = if self.options.profile_instrument {
            RuntimeExactUnrollEmissionPlan {
                suppress_guard: vec![false; program.instrs.len()],
                induction_increment: vec![None; program.instrs.len()],
            }
        } else {
            runtime_exact_unroll_emission_plan(program)
        };
        let mut jump_patches: Vec<(usize, usize)> = Vec::new();
        let mut oob_exit_patches: Vec<usize> = Vec::new();
        let mut has_incoming_target = vec![false; program.instrs.len()];
        for instr in &program.instrs {
            let target = match instr {
                RuntimeInstr::Jump { target }
                | RuntimeInstr::JumpIfZero { target, .. }
                | RuntimeInstr::JumpIfCmpFalse { target, .. }
                | RuntimeInstr::Call { target } => *target,
                _ => continue,
            };
            if target < has_incoming_target.len() {
                has_incoming_target[target] = true;
            }
        }
        let mut hot_align_starts = vec![false; program.instrs.len()];
        let mut profile_block_at_start = vec![None; program.instrs.len()];
        if self.options.profile_instrument {
            for block in &lir.blocks {
                if let Some(&start) = block.instr_indices.first() {
                    profile_block_at_start[start] = Some(block.id);
                }
            }
        }
        if effective_profile.is_some() {
            let hottest = lir
                .blocks
                .iter()
                .map(|block| block.frequency)
                .max()
                .unwrap_or(1);
            for loop_plan in &lir.loops {
                if let Some(block) = lir.blocks.get(loop_plan.header) {
                    if block.frequency.saturating_mul(8) >= hottest {
                        if let Some(start) = block.instr_indices.first() {
                            hot_align_starts[*start] = true;
                        }
                    }
                }
            }
        }
        let mut idx = 0usize;
        while idx < program.instrs.len() {
            if hot_align_starts[idx] {
                while self.code.len() % 16 != 0 {
                    self.code.push(0x90);
                }
            }
            if let Some(block_id) = profile_block_at_start[idx] {
                instr_offsets[idx] = self.code.len();
                self.emit_inc_qword_rbp_disp(profile_base_disp + 16 + block_id as i32 * 8);
            }
            let mut run_end = idx;
            let mut run_width = None;
            while run_end < program.instrs.len() {
                if run_end != idx
                    && (has_incoming_target[run_end]
                        || profile_block_at_start[run_end].is_some())
                {
                    break;
                }
                match &program.instrs[run_end] {
                    RuntimeInstr::Mov {
                        dst,
                        src: RuntimeOperand::Imm(0),
                    } if slot_map.reg(*dst).is_none()
                        && run_width.is_none_or(|width| width == slot_map.element_width(*dst)) =>
                    {
                        run_width.get_or_insert(slot_map.element_width(*dst));
                        run_end += 1;
                    }
                    _ => break,
                }
            }
            if run_end > idx + 1 {
                let run_pos = self.code.len();
                for (offset, entry) in instr_offsets
                    .iter_mut()
                    .enumerate()
                    .take(run_end)
                    .skip(idx)
                {
                    if offset != idx || profile_block_at_start[idx].is_none() {
                        *entry = run_pos;
                    }
                }
                if !self.try_emit_bulk_zero_stack_run(&program.instrs[idx..run_end], &slot_map) {
                    self.code.extend_from_slice(&[0x48, 0x31, 0xC0]); // xor rax, rax
                    for zero_idx in idx..run_end {
                        let RuntimeInstr::Mov { dst, .. } = program.instrs[zero_idx] else {
                            unreachable!("zero run only contains stack zero mov instructions");
                        };
                        self.emit_store_rax_to_slot(dst, &slot_map);
                    }
                }
                idx = run_end;
                continue;
            }

            let instr = &program.instrs[idx];
            if profile_block_at_start[idx].is_none() {
                instr_offsets[idx] = self.code.len();
            }
            if exact_unroll_plan.suppress_guard[idx] {
                idx += 1;
                continue;
            }
            if let Some(amount) = exact_unroll_plan.induction_increment[idx] {
                if amount != 0 {
                    let RuntimeInstr::BinOpInPlace { dst, .. } = instr else {
                        unreachable!("exact-unroll induction plan must target an in-place add");
                    };
                    if !self.emit_binop_slot_imm_in_place(
                        *dst,
                        RuntimeBinOp::Add,
                        amount as i32,
                        &slot_map,
                    ) {
                        self.emit_load_slot_to_rax(*dst, &slot_map);
                        self.emit_alu_reg_imm(0, 0, (amount as i32).to_le_bytes());
                        self.emit_store_rax_to_slot(*dst, &slot_map);
                    }
                }
                idx += 1;
                continue;
            }

            if let Some(fusion) =
                runtime_u32_affine_fusion_candidate(program, idx, &has_incoming_target)
            {
                for offset in 1..fusion.consumed {
                    instr_offsets[idx + offset] = self.code.len();
                }
                if let Some(dst_reg) = slot_map.reg(fusion.narrowed_slot) {
                    if let RuntimeOperand::Slot(lhs_slot) = fusion.lhs {
                        if let Some(lhs_reg) = slot_map.reg(lhs_slot) {
                            self.emit_imul_reg32_reg32_imm32(
                                dst_reg,
                                lhs_reg,
                                fusion.mul as i32,
                            );
                        } else {
                            self.emit_load_operand_to_reg(dst_reg, &fusion.lhs, &slot_map);
                            self.emit_imul_reg32_reg32_imm32(
                                dst_reg,
                                dst_reg,
                                fusion.mul as i32,
                            );
                        }
                    } else {
                        self.emit_load_operand_to_reg(dst_reg, &fusion.lhs, &slot_map);
                        self.emit_imul_reg32_reg32_imm32(
                            dst_reg,
                            dst_reg,
                            fusion.mul as i32,
                        );
                    }
                    self.emit_add_reg32_imm32(dst_reg, fusion.add as i32);
                    if let Some(state_slot) = fusion.state_slot
                        && state_slot != fusion.narrowed_slot
                    {
                        self.emit_store_reg_to_slot(dst_reg, state_slot, &slot_map);
                    }
                } else {
                    self.emit_load_operand_to_rax(&fusion.lhs, &slot_map);
                    self.code.extend_from_slice(&[0x69, 0xC0]); // imul eax, eax, imm32
                    self.code.extend_from_slice(&fusion.mul.to_le_bytes());
                    self.code.push(0x05); // add eax, imm32
                    self.code.extend_from_slice(&fusion.add.to_le_bytes());
                    self.emit_store_rax_to_slot(fusion.narrowed_slot, &slot_map);
                    if let Some(state_slot) = fusion.state_slot
                        && state_slot != fusion.narrowed_slot
                    {
                        self.emit_store_rax_to_slot(state_slot, &slot_map);
                    }
                }
                idx += fusion.consumed;
                continue;
            }
            if let Some((op, lhs, rhs, target)) =
                runtime_cmp_jumpifzero_fusion_candidate(program, idx)
            {
                instr_offsets[idx + 1] = self.code.len();
                self.emit_jump_if_cmp_false(op, &lhs, &rhs, target, &slot_map, &mut jump_patches);
                idx += 2;
                continue;
            }
            if let Some(fusion) =
                runtime_bloom_classic4_jump_fusion_candidate(program, idx, &has_incoming_target)
            {
                instr_offsets[idx + 1] = self.code.len();
                if self.emit_runtime_bloom_classic4_check_jump(
                    &fusion.filter_slots,
                    &fusion.hash,
                    fusion.target,
                    &slot_map,
                    &mut jump_patches,
                ) {
                    idx += 2;
                    continue;
                }
            }
            if let Some(fusion) =
                runtime_shift_or_fusion_candidate(program, idx, &has_incoming_target)
            {
                let can_merge = slot_map.reg(fusion.dst).is_some()
                    && match fusion.rhs {
                        RuntimeOperand::Imm(value) => imm32_sign_extended(value).is_some(),
                        RuntimeOperand::Slot(slot) => slot_map.reg(slot).is_some(),
                    };
                if can_merge {
                    instr_offsets[idx + 1] = self.code.len();
                    let dst_reg = slot_map.reg(fusion.dst).expect("checked register result");
                    self.emit_load_operand_to_reg(dst_reg, &fusion.value, &slot_map);
                    self.emit_shl_reg_imm8(dst_reg, fusion.shift);
                    if self.try_emit_binop_reg_operand(
                        dst_reg,
                        RuntimeBinOp::BitOr,
                        &fusion.rhs,
                        &slot_map,
                    ) {
                        idx += 2;
                        continue;
                    }
                    unreachable!("shift-or merge preconditions guarantee register lowering");
                }
            }
            if let Some(fusion) =
                runtime_index_increment_fusion_candidate(program, idx, &has_incoming_target)
            {
                instr_offsets[idx + 1] = self.code.len();
                instr_offsets[idx + 2] = self.code.len();
                if self.emit_runtime_index_increment_direct(
                    &fusion.base_slots,
                    &fusion.index,
                    &slot_map,
                ) {
                    idx += 3;
                    continue;
                }
            }
            if let Some(fusion) =
                runtime_load_index_cmp_jump_fusion_candidate(program, idx, &has_incoming_target)
            {
                instr_offsets[idx + 1] = self.code.len();
                if self.emit_runtime_index_cmp_jump_if_false(
                    fusion.op,
                    &fusion.base_slots,
                    &fusion.index,
                    fusion.checked,
                    &fusion.other,
                    fusion.target,
                    &slot_map,
                    &mut jump_patches,
                    &mut oob_exit_patches,
                ) {
                    idx += 2;
                    continue;
                }
            }
            if let Some(fusion) =
                runtime_bit_test_indexed_fusion_candidate(program, idx, &has_incoming_target)
            {
                instr_offsets[idx + 1] = self.code.len();
                instr_offsets[idx + 2] = self.code.len();
                if self.emit_runtime_bit_test_indexed_to_slot(
                    fusion.dst,
                    &fusion.base_slots,
                    &fusion.index,
                    &fusion.bit,
                    fusion.checked,
                    &slot_map,
                    &mut oob_exit_patches,
                ) {
                    idx += 3;
                    continue;
                }
            }
            if let Some(fusion) =
                runtime_bit_test_bool_fusion_candidate(program, idx, &has_incoming_target)
            {
                instr_offsets[idx + 1] = self.code.len();
                self.emit_load_operand_to_rax(&fusion.value, &slot_map);
                self.emit_load_operand_to_rcx(&fusion.bit, &slot_map);
                self.emit_bt_rax_rcx();
                self.code.extend_from_slice(&[0x0F, 0x92, 0xC0]); // setb al (CF)
                self.code.extend_from_slice(&[0x48, 0x0F, 0xB6, 0xC0]); // movzx rax, al
                self.emit_store_rax_to_slot(fusion.dst, &slot_map);
                idx += 2;
                continue;
            }
            if let Some(fusion) =
                runtime_bitset_store_fusion_candidate(program, idx, &has_incoming_target)
            {
                instr_offsets[idx + 1] = self.code.len();
                instr_offsets[idx + 2] = self.code.len();
                instr_offsets[idx + 3] = self.code.len();

                let checked = fusion.load_checked || fusion.store_checked;
                if !fusion.merged_read_later
                    && self.emit_runtime_bit_set_indexed_direct(
                        &fusion.base_slots,
                        &fusion.index,
                        &fusion.bit,
                        checked,
                        &slot_map,
                        &mut oob_exit_patches,
                    )
                {
                    idx += 4;
                    continue;
                }

                if fusion.load_checked {
                    self.emit_runtime_load_index(
                        fusion.word_slot,
                        &fusion.base_slots,
                        &fusion.index,
                        &slot_map,
                        &mut oob_exit_patches,
                    );
                } else {
                    self.emit_runtime_load_index_unchecked(
                        fusion.word_slot,
                        &fusion.base_slots,
                        &fusion.index,
                        &slot_map,
                    );
                }

                self.emit_load_slot_to_rax(fusion.word_slot, &slot_map);
                self.emit_load_operand_to_rcx(&fusion.bit, &slot_map);
                self.emit_bts_rax_rcx();
                self.emit_store_rax_to_slot(fusion.merged_slot, &slot_map);

                let src = RuntimeOperand::Slot(fusion.merged_slot);
                if fusion.store_checked {
                    self.emit_runtime_store_index(
                        &fusion.base_slots,
                        &fusion.index,
                        &src,
                        &slot_map,
                        &mut oob_exit_patches,
                    );
                } else {
                    self.emit_runtime_store_index_unchecked(
                        &fusion.base_slots,
                        &fusion.index,
                        &src,
                        &slot_map,
                    );
                }
                idx += 4;
                continue;
            }

            match instr {
                RuntimeInstr::LoadSeed {
                    dst,
                    kind,
                    input: _,
                } => {
                    match kind {
                        RuntimeLoadKind::EntropySeed => {
                            self.code.extend_from_slice(&[0x0F, 0x31]); // rdtsc
                            self.code.extend_from_slice(&[0x48, 0xC1, 0xE2, 0x20]);
                            self.code.extend_from_slice(&[0x48, 0x09, 0xD0]);
                        }
                        RuntimeLoadKind::ArgumentCount => {
                            match self.runtime.process_entry {
                                ProcessEntryAbi::LinuxInitialStack => {
                                    // rbp+8 is the untouched process-entry rsp.
                                    self.code.extend_from_slice(&[0x48, 0x8B, 0x45, 0x08]);
                                }
                                ProcessEntryAbi::DarwinInitialStack => {
                                    self.code.extend_from_slice(&[0x48, 0x8B, 0x45, 0x08]);
                                }
                                ProcessEntryAbi::WindowsLoader => {
                                    self.emit_mov_rax_imm(0);
                                }
                            }
                        }
                        RuntimeLoadKind::EntryStackPointer => {
                            match self.runtime.process_entry {
                                ProcessEntryAbi::LinuxInitialStack => {
                                    self.code.extend_from_slice(&[0x48, 0x8D, 0x45, 0x08]);
                                }
                                ProcessEntryAbi::DarwinInitialStack => {
                                    self.code.extend_from_slice(&[0x48, 0x8D, 0x45, 0x08]);
                                }
                                ProcessEntryAbi::WindowsLoader => {
                                    self.emit_mov_rax_imm(0);
                                }
                            }
                        }
                        RuntimeLoadKind::MonotonicNanos => {
                            self.emit_runtime_clock_nanos(
                                self.runtime.clock_monotonic,
                                &slot_map,
                            );
                        }
                        RuntimeLoadKind::WallTimeNanos => {
                            self.emit_runtime_clock_nanos(
                                self.runtime.clock_realtime,
                                &slot_map,
                            );
                        }
                        RuntimeLoadKind::ProcessId => {
                            let preserved = self.emit_preserve_slot_regs_for_heap_syscall(&slot_map);
                            self.emit_mov_rax_imm(self.runtime.syscalls.getpid); // SYS_getpid
                            self.emit_kernel_call();
                            self.emit_restore_preserved_regs_reverse(&preserved);
                        }
                    }
                    self.emit_store_rax_to_slot(*dst, &slot_map);
                }
                RuntimeInstr::Mov { dst, src } => {
                    self.emit_mov_slot_operand(*dst, src, &slot_map);
                }
                RuntimeInstr::BinOp { dst, op, lhs, rhs } => {
                    if lir.demanded_width_for_instruction(idx) <= 32
                        && self.emit_binop_slot_operand_u32(
                            *dst,
                            *op,
                            lhs,
                            rhs,
                            &slot_map,
                        )
                    {
                        idx += 1;
                        continue;
                    }
                    self.emit_binop_slot_operand(*dst, *op, lhs, rhs, &slot_map);
                }
                RuntimeInstr::BinOpInPlace { dst, op, rhs } => {
                    if lir.demanded_width_for_instruction(idx) <= 32
                        && self.emit_binop_slot_operand_u32(
                            *dst,
                            *op,
                            &RuntimeOperand::Slot(*dst),
                            rhs,
                            &slot_map,
                        )
                    {
                        idx += 1;
                        continue;
                    }
                    if let RuntimeOperand::Slot(rhs_slot) = rhs {
                        if self.emit_binop_slot_slot_in_place(*dst, *rhs_slot, *op, &slot_map) {
                            idx += 1;
                            continue;
                        }
                    }
                    if let RuntimeOperand::Imm(imm) = rhs {
                        if *op == RuntimeBinOp::BitAnd && *imm == u64::from(u32::MAX) {
                            if let Some(reg) = slot_map.reg(*dst) {
                                self.emit_mov_reg32_reg32(reg, reg);
                            } else {
                                self.emit_load_slot_to_rax(*dst, &slot_map);
                                self.emit_mov_reg32_reg32(0, 0);
                                self.emit_store_rax_to_slot(*dst, &slot_map);
                            }
                            idx += 1;
                            continue;
                        }
                        if let Some(imm32) = imm32_sign_extended(*imm) {
                            if self.emit_binop_slot_imm_in_place(*dst, *op, imm32, &slot_map) {
                                idx += 1;
                                continue;
                            }
                        }
                    }
                    self.emit_load_slot_to_rax(*dst, &slot_map);
                    self.emit_load_operand_to_rcx(rhs, &slot_map);
                    self.emit_runtime_binop_rax_rcx(*op);
                    self.emit_store_rax_to_slot(*dst, &slot_map);
                }
                RuntimeInstr::FloatBinOp {
                    dst,
                    bits,
                    op,
                    lhs,
                    rhs,
                } => {
                    self.emit_load_operand_to_xmm0(lhs, *bits, &slot_map);
                    self.emit_load_operand_to_xmm1(rhs, *bits, &slot_map);
                    self.emit_runtime_float_binop_xmm0_xmm1(*op, *bits);
                    self.emit_store_xmm0_to_slot(*dst, *bits, &slot_map);
                }
                RuntimeInstr::Cmp { dst, op, lhs, rhs } => {
                    self.emit_runtime_cmp_to_slot(*dst, *op, lhs, rhs, &slot_map);
                }
                RuntimeInstr::NormalizeInt { dst, signed, bits } => {
                    self.emit_normalize_slot_int(*dst, *signed, *bits, &slot_map);
                }
                RuntimeInstr::Jump { target } => {
                    if *target < idx {
                        let target_pos = instr_offsets[*target];
                        let rel = target_pos as isize - (self.code.len() + 2) as isize;
                        if let Ok(rel8) = i8::try_from(rel) {
                            self.code.extend_from_slice(&[0xEB, rel8 as u8]); // jmp rel8
                            idx += 1;
                            continue;
                        }
                    }
                    self.code.push(0xE9); // jmp rel32
                    let disp_pos = self.code.len();
                    self.code.extend_from_slice(&0_i32.to_le_bytes());
                    jump_patches.push((disp_pos, *target));
                }
                RuntimeInstr::JumpIfZero { cond_slot, target } => {
                    self.emit_load_slot_to_rax(*cond_slot, &slot_map);
                    self.code.extend_from_slice(&[0x48, 0x85, 0xC0]); // test rax, rax
                    if *target < idx {
                        let target_pos = instr_offsets[*target];
                        let rel = target_pos as isize - (self.code.len() + 2) as isize;
                        if let Ok(rel8) = i8::try_from(rel) {
                            self.code.extend_from_slice(&[0x74, rel8 as u8]); // jz rel8
                            idx += 1;
                            continue;
                        }
                    }
                    self.code.extend_from_slice(&[0x0F, 0x84]); // jz rel32
                    let disp_pos = self.code.len();
                    self.code.extend_from_slice(&0_i32.to_le_bytes());
                    jump_patches.push((disp_pos, *target));
                }
                RuntimeInstr::JumpIfCmpFalse {
                    op,
                    lhs,
                    rhs,
                    target,
                } => {
                    self.emit_jump_if_cmp_false(
                        *op,
                        lhs,
                        rhs,
                        *target,
                        &slot_map,
                        &mut jump_patches,
                    );
                }
                RuntimeInstr::LoadIndex { dst, base_slots, index } => {
                    self.emit_runtime_load_index(
                        *dst,
                        base_slots,
                        index,
                        &slot_map,
                        &mut oob_exit_patches,
                    );
                }
                RuntimeInstr::LoadIndexUnchecked {
                    dst,
                    base_slots,
                    index,
                } => {
                    self.emit_runtime_load_index_unchecked(*dst, base_slots, index, &slot_map);
                }
                RuntimeInstr::StoreIndex { base_slots, index, src } => {
                    self.emit_runtime_store_index(
                        base_slots,
                        index,
                        src,
                        &slot_map,
                        &mut oob_exit_patches,
                    );
                }
                RuntimeInstr::StoreIndexUnchecked {
                    base_slots,
                    index,
                    src,
                } => {
                    self.emit_runtime_store_index_unchecked(base_slots, index, src, &slot_map);
                }
                RuntimeInstr::HeapLoadInt {
                    dst,
                    ptr,
                    index,
                    bytes,
                } => {
                    self.emit_runtime_heap_load_int(*dst, ptr, index, *bytes, &slot_map);
                }
                RuntimeInstr::HeapStoreInt {
                    ptr,
                    index,
                    src,
                    bytes,
                } => {
                    self.emit_runtime_heap_store_int(ptr, index, src, *bytes, &slot_map);
                }
                RuntimeInstr::HeapCopy {
                    dst_ptr,
                    src_ptr,
                    bytes,
                } => {
                    self.emit_runtime_heap_copy(dst_ptr, src_ptr, bytes, &slot_map);
                }
                RuntimeInstr::BloomSplitBlockInsert { filter_slots, hash } => {
                    self.emit_runtime_bloom_split_block_insert(filter_slots, hash, &slot_map);
                }
                RuntimeInstr::BloomSplitBlockCheck {
                    dst,
                    filter_slots,
                    hash,
                } => {
                    self.emit_runtime_bloom_split_block_check(*dst, filter_slots, hash, &slot_map);
                }
                RuntimeInstr::BloomClassic4Check {
                    dst,
                    lanes_checked,
                    filter_slots,
                    hash,
                } => {
                    self.emit_runtime_bloom_classic4_check(
                        *dst,
                        *lanes_checked,
                        filter_slots,
                        hash,
                        &slot_map,
                    );
                }
                RuntimeInstr::HashCtrlGroupProbe {
                    dst_mask,
                    ctrl_slots,
                    group_start,
                    fingerprint,
                } => {
                    self.emit_runtime_hash_ctrl_group_probe(
                        *dst_mask,
                        ctrl_slots,
                        group_start,
                        fingerprint,
                        &slot_map,
                    );
                }
                RuntimeInstr::JoinSelectAdaptive {
                    dst,
                    build_rows,
                    probe_rows,
                } => {
                    self.emit_runtime_join_select_adaptive(
                        *dst,
                        build_rows,
                        probe_rows,
                        &slot_map,
                    );
                }
                RuntimeInstr::Alloc { dst, size } => {
                    if let Some(disp) = slot_map.promoted_alloc_disp(*dst) {
                        self.emit_lea_rax_rbp_disp(disp);
                        self.emit_store_rax_to_slot(*dst, &slot_map);
                    } else {
                        self.emit_runtime_alloc(*dst, size, &slot_map);
                    }
                }
                RuntimeInstr::Free { ptr, size } => {
                    let promoted = match ptr {
                        RuntimeOperand::Slot(slot) => {
                            slot_map.promoted_alloc_disp(*slot).is_some()
                        }
                        RuntimeOperand::Imm(_) => false,
                    };
                    if !promoted {
                        self.emit_runtime_free(ptr, size, &slot_map);
                    }
                }
                RuntimeInstr::FileOpen {
                    dst,
                    path_ptr,
                    flags,
                    mode,
                } => {
                    self.emit_runtime_file_open(
                        *dst,
                        path_ptr,
                        *flags,
                        *mode,
                        &slot_map,
                    );
                }
                RuntimeInstr::FileWrite { dst, fd, ptr, len } => {
                    self.emit_runtime_file_io(
                        *dst,
                        fd,
                        ptr,
                        len,
                        self.runtime.syscalls.write,
                        &slot_map,
                    );
                }
                RuntimeInstr::FileRead { dst, fd, ptr, len } => {
                    self.emit_runtime_file_io(
                        *dst,
                        fd,
                        ptr,
                        len,
                        self.runtime.syscalls.read,
                        &slot_map,
                    );
                }
                RuntimeInstr::FileClose { fd } => {
                    self.emit_runtime_file_close(fd, &slot_map);
                }
                RuntimeInstr::ThreadSpawn {
                    handle_dst,
                    target,
                    return_slot,
                } => {
                    let disp_pos = self.emit_runtime_thread_spawn(
                        *handle_dst,
                        *return_slot,
                        stack_size,
                        allocator_frame,
                        thread_context_disp,
                        &slot_map,
                    );
                    jump_patches.push((disp_pos, *target));
                }
                RuntimeInstr::ThreadJoin { dst, handle } => {
                    self.emit_runtime_thread_join(*dst, handle, stack_size, &slot_map);
                }
                RuntimeInstr::ChannelCreate {
                    dst,
                    capacity,
                    unbounded,
                } => self.emit_runtime_channel_create(
                    *dst,
                    capacity,
                    *unbounded,
                    thread_context_disp,
                    &slot_map,
                ),
                RuntimeInstr::ChannelSend { handle, value } => {
                    self.emit_runtime_channel_send(handle, value, thread_context_disp, &slot_map)
                }
                RuntimeInstr::ChannelRecv { dst, handle } => {
                    self.emit_runtime_channel_recv(*dst, handle, thread_context_disp, &slot_map)
                }
                RuntimeInstr::ChannelClose { handle, sender } => {
                    self.emit_runtime_channel_close(handle, *sender, &slot_map)
                }
                RuntimeInstr::ChannelDestroy { handle } => {
                    self.emit_runtime_channel_destroy(handle, &slot_map)
                }
                RuntimeInstr::PrintConst { text } => {
                    self.emit_runtime_print_const(text, &slot_map);
                }
                RuntimeInstr::PrintInt { value, signed, bits } => {
                    self.emit_runtime_print_int(value, *signed, *bits, &slot_map);
                }
                RuntimeInstr::CompareSwap {
                    left,
                    right,
                    signed,
                } => {
                    self.emit_runtime_compare_swap(*left, *right, *signed, &slot_map);
                }
                RuntimeInstr::RadixSortFixedInt {
                    slots,
                    bits,
                    signed,
                    stable,
                } => {
                    self.emit_runtime_radix_sort_fixed_int(
                        slots,
                        *bits,
                        *signed,
                        *stable,
                        &slot_map,
                    );
                }
                RuntimeInstr::Call { target } => {
                    let tail_call = idx + 1 < program.instrs.len()
                        && matches!(program.instrs[idx + 1], RuntimeInstr::Return)
                        && !has_incoming_target[idx + 1];
                    if tail_call {
                        // Tail-call: replace call+ret with jmp to avoid one return-address push/pop.
                        instr_offsets[idx + 1] = self.code.len();
                        self.code.push(0xE9); // jmp rel32
                        let disp_pos = self.code.len();
                        self.code.extend_from_slice(&0_i32.to_le_bytes());
                        jump_patches.push((disp_pos, *target));
                        idx += 2;
                        continue;
                    }
                    self.code.push(0xE8); // call rel32
                    let disp_pos = self.code.len();
                    self.code.extend_from_slice(&0_i32.to_le_bytes());
                    jump_patches.push((disp_pos, *target));
                }
                RuntimeInstr::Return => {
                    self.code.push(0xC3); // ret
                }
                RuntimeInstr::Exit { code } => {
                    if self.options.emit_full_checksum {
                        let full = full_width_exit_operand(program, idx).unwrap_or(*code);
                        self.emit_load_operand_to_rax(&full, &slot_map);
                        self.emit_raw_checksum_from_rax();
                    }
                    self.emit_load_operand_to_rax(code, &slot_map);
                    if let Some(context_disp) = thread_context_disp {
                        self.code.extend_from_slice(&[0x48, 0x8B, 0x8D]);
                        self.code.extend_from_slice(&context_disp.to_le_bytes());
                        self.code.extend_from_slice(&[0x48, 0x85, 0xC9]); // test rcx,rcx
                        self.code.extend_from_slice(&[0x0F, 0x84]);
                        let main_exit_patch = self.code.len();
                        self.code.extend_from_slice(&0_i32.to_le_bytes());
                        self.code.extend_from_slice(&[0x48, 0x89, 0x41, 0x08]);
                        let main_exit = self.code.len();
                        patch_rel32(&mut self.code, main_exit_patch, main_exit);
                    }
                    if self.options.profile_instrument {
                        self.code.push(0x50); // preserve exit value
                        self.emit_raw_profile_from_frame(profile_base_disp, profile_payload_bytes);
                        self.code.push(0x58);
                    }
                    if self.runtime_allocator.is_some() {
                        self.code.push(0x50); // preserve exit value across allocator teardown
                        self.emit_runtime_allocator_teardown_call();
                        self.code.push(0x58);
                    }
                    self.code.extend_from_slice(&[0x48, 0x89, 0xC7]); // mov rdi, rax
                    self.emit_mov_rax_imm(self.runtime.syscalls.exit);
                    self.emit_kernel_call(); // syscall
                }
            }
            idx += 1;
        }

        let end_pos = self.code.len();
        for (disp_pos, target_instr) in jump_patches {
            let target_pos = if target_instr < instr_offsets.len() {
                instr_offsets[target_instr]
            } else {
                end_pos
            };
            patch_rel32(&mut self.code, disp_pos, target_pos);
        }
        if !oob_exit_patches.is_empty() {
            let oob_target = self.code.len();
            self.emit_exit(255);
            for disp_pos in oob_exit_patches {
                patch_rel32(&mut self.code, disp_pos, oob_target);
            }
        }
        if self.runtime_allocator.is_some() {
            self.emit_runtime_allocator_routines();
            self.runtime_allocator = None;
        }

        let mut block_offsets = Vec::with_capacity(lir.blocks.len());
        for block in &lir.blocks {
            let start = block
                .instr_indices
                .first()
                .and_then(|instr_index| instr_offsets.get(*instr_index))
                .copied()
                .unwrap_or(end_pos);
            let end = block
                .instr_indices
                .last()
                .and_then(|instr_index| instr_offsets.get(*instr_index))
                .copied()
                .map(|offset| {
                    let next_instr = block.instr_indices.last().copied().unwrap_or(0) + 1;
                    instr_offsets.get(next_instr).copied().unwrap_or(end_pos).max(offset)
                })
                .unwrap_or(start);
            block_offsets.push((block.id, start, end));
        }
        self.runtime_generic_metadata = Some(RuntimeGenericMetadata {
            profile_template: lir.build_profile_template("runtime_generic"),
            lir,
            block_offsets,
            optimization_report: optimized.report,
        });
    }

    fn emit_runtime_cmp_to_slot(
        &mut self,
        dst: usize,
        op: RuntimeCmpOp,
        lhs: &RuntimeOperand,
        rhs: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        self.emit_load_operand_to_rax(lhs, slot_map);
        self.emit_load_operand_to_rcx(rhs, slot_map);
        self.code.extend_from_slice(&[0x48, 0x39, 0xC8]); // cmp rax, rcx
        let setcc = match op {
            RuntimeCmpOp::Eq => [0x0F, 0x94, 0xC0],         // sete al
            RuntimeCmpOp::Ne => [0x0F, 0x95, 0xC0],         // setne al
            RuntimeCmpOp::LtUnsigned => [0x0F, 0x92, 0xC0], // setb al
            RuntimeCmpOp::LeUnsigned => [0x0F, 0x96, 0xC0], // setbe al
            RuntimeCmpOp::GtUnsigned => [0x0F, 0x97, 0xC0], // seta al
            RuntimeCmpOp::GeUnsigned => [0x0F, 0x93, 0xC0], // setae al
            RuntimeCmpOp::LtSigned => [0x0F, 0x9C, 0xC0],   // setl al
            RuntimeCmpOp::LeSigned => [0x0F, 0x9E, 0xC0],   // setle al
            RuntimeCmpOp::GtSigned => [0x0F, 0x9F, 0xC0],   // setg al
            RuntimeCmpOp::GeSigned => [0x0F, 0x9D, 0xC0],   // setge al
        };
        self.code.extend_from_slice(&setcc);
        self.code.extend_from_slice(&[0x48, 0x0F, 0xB6, 0xC0]); // movzx rax, al
        self.emit_store_rax_to_slot(dst, slot_map);
    }

    fn emit_jump_if_cmp_false(
        &mut self,
        op: RuntimeCmpOp,
        lhs: &RuntimeOperand,
        rhs: &RuntimeOperand,
        target: usize,
        slot_map: &RuntimeSlotMap,
        jump_patches: &mut Vec<(usize, usize)>,
    ) {
        if let (RuntimeOperand::Slot(lhs_slot), RuntimeOperand::Slot(rhs_slot)) = (lhs, rhs) {
            if self.emit_cmp_slot_slot(*lhs_slot, *rhs_slot, slot_map) {
                self.code.extend_from_slice(&[0x0F, false_jcc_opcode(op)]);
                let disp_pos = self.code.len();
                self.code.extend_from_slice(&0_i32.to_le_bytes());
                jump_patches.push((disp_pos, target));
                return;
            }
        }
        if let (RuntimeOperand::Slot(slot), RuntimeOperand::Imm(imm)) = (lhs, rhs) {
            if let Some(imm32) = imm32_sign_extended(*imm) {
                if self.emit_cmp_slot_imm(*slot, imm32, slot_map) {
                    self.code.extend_from_slice(&[0x0F, false_jcc_opcode(op)]);
                    let disp_pos = self.code.len();
                    self.code.extend_from_slice(&0_i32.to_le_bytes());
                    jump_patches.push((disp_pos, target));
                    return;
                }
            }
        }
        self.emit_load_operand_to_rax(lhs, slot_map);
        self.emit_load_operand_to_rcx(rhs, slot_map);
        self.code.extend_from_slice(&[0x48, 0x39, 0xC8]); // cmp rax, rcx
        self.code.extend_from_slice(&[0x0F, false_jcc_opcode(op)]);
        let disp_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        jump_patches.push((disp_pos, target));
    }

    fn emit_runtime_index_cmp_jump_if_false(
        &mut self,
        op: RuntimeCmpOp,
        base_slots: &[usize],
        index: &RuntimeOperand,
        checked: bool,
        other: &RuntimeOperand,
        target: usize,
        slot_map: &RuntimeSlotMap,
        jump_patches: &mut Vec<(usize, usize)>,
        oob_exit_patches: &mut Vec<usize>,
    ) -> bool {
        if !self.emit_contiguous_stack_index_access(
            base_slots,
            index,
            slot_map,
            0,
            true,
            checked,
            oob_exit_patches,
        ) {
            return false;
        }

        match other {
            RuntimeOperand::Imm(imm) => {
                if let Some(imm32) = imm32_sign_extended(*imm) {
                    self.emit_alu_reg_imm(7, 0, imm32.to_le_bytes()); // cmp rax, imm32
                } else {
                    self.emit_load_operand_to_rcx(other, slot_map);
                    self.code.extend_from_slice(&[0x48, 0x39, 0xC8]); // cmp rax, rcx
                }
            }
            RuntimeOperand::Slot(slot) => {
                if let Some(rhs_reg) = slot_map.reg(*slot) {
                    self.emit_cmp_reg_reg(0, rhs_reg);
                } else {
                    self.emit_load_operand_to_rcx(other, slot_map);
                    self.code.extend_from_slice(&[0x48, 0x39, 0xC8]); // cmp rax, rcx
                }
            }
        }
        self.code.extend_from_slice(&[0x0F, false_jcc_opcode(op)]);
        let disp_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        jump_patches.push((disp_pos, target));
        true
    }

    fn emit_runtime_bloom_split_block_insert(
        &mut self,
        filter_slots: &[usize],
        hash: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        if filter_slots.len() < 4
            || (filter_slots.len() & 3) != 0
            || !filter_slots.len().is_power_of_two()
        {
            self.emit_exit(255);
            return;
        }
        let block_count = filter_slots.len() / 4;
        let Some((base_disp, needs_neg, 8)) = self.contiguous_stack_base_access(filter_slots, slot_map) else {
            self.emit_exit(255);
            return;
        };
        let block_mask = (block_count as u32).wrapping_sub(1);
        self.emit_runtime_bloom_insert_word(base_disp, block_mask, needs_neg, 0, 1203114875, hash, slot_map);
        self.emit_runtime_bloom_insert_word(base_disp, block_mask, needs_neg, 1, 1150766481, hash, slot_map);
        self.emit_runtime_bloom_insert_word(base_disp, block_mask, needs_neg, 2, 2284105051, hash, slot_map);
        self.emit_runtime_bloom_insert_word(base_disp, block_mask, needs_neg, 3, 2729918621, hash, slot_map);
    }

    fn emit_runtime_bloom_split_block_check(
        &mut self,
        dst: usize,
        filter_slots: &[usize],
        hash: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        if filter_slots.len() < 4
            || (filter_slots.len() & 3) != 0
            || !filter_slots.len().is_power_of_two()
        {
            self.emit_exit(255);
            return;
        }
        let block_count = filter_slots.len() / 4;
        let Some((base_disp, needs_neg, 8)) = self.contiguous_stack_base_access(filter_slots, slot_map) else {
            self.emit_exit(255);
            return;
        };
        let block_mask = (block_count as u32).wrapping_sub(1);
        let mut fail_jumps = Vec::with_capacity(2);
        fail_jumps.push(self.emit_runtime_bloom_check_word(
            base_disp,
            block_mask,
            needs_neg,
            0,
            1203114875,
            hash,
            slot_map,
        ));
        fail_jumps.push(self.emit_runtime_bloom_check_word(
            base_disp,
            block_mask,
            needs_neg,
            1,
            1150766481,
            hash,
            slot_map,
        ));
        fail_jumps.push(self.emit_runtime_bloom_check_word(
            base_disp,
            block_mask,
            needs_neg,
            2,
            2284105051,
            hash,
            slot_map,
        ));
        fail_jumps.push(self.emit_runtime_bloom_check_word(
            base_disp,
            block_mask,
            needs_neg,
            3,
            2729918621,
            hash,
            slot_map,
        ));

        self.emit_mov_rax_imm(1);
        self.code.push(0xE9);
        let done_jmp_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        let fail_pos = self.code.len();
        self.emit_mov_rax_imm(0);
        let done_pos = self.code.len();
        patch_rel32(&mut self.code, done_jmp_pos, done_pos);
        for jump in fail_jumps {
            patch_rel32(&mut self.code, jump, fail_pos);
        }

        self.emit_store_rax_to_slot(dst, slot_map);
    }

    fn emit_runtime_bloom_classic4_check(
        &mut self,
        dst: usize,
        lanes_checked: usize,
        filter_slots: &[usize],
        hash: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        if filter_slots.len() != 64 {
            self.emit_exit(255);
            return;
        }
        let Some((base_disp, needs_neg, 8)) = self.contiguous_stack_base_access(filter_slots, slot_map)
        else {
            self.emit_exit(255);
            return;
        };

        self.emit_load_operand_to_rax(hash, slot_map);
        let mut fail_jumps = Vec::with_capacity(4);
        for lane in 0..4u8 {
            self.emit_mov_reg_reg(1, 0); // rcx = hash
            let bit_shift = lane * 13;
            if bit_shift != 0 {
                self.emit_shr_reg_imm8(1, bit_shift);
            }
            self.emit_mov_reg_reg(2, 0); // rdx = hash
            self.emit_shr_reg_imm8(2, bit_shift + 6); // rdx = word index
            self.emit_and_reg_imm32(2, 63);
            if needs_neg {
                self.emit_neg_reg(2);
            }
            self.emit_indexed_rbp_mem_reg_with_index(2, 2, base_disp, 8, true);
            self.code.extend_from_slice(&[0x48, 0x0F, 0xA3, 0xCA]); // bt rdx, rcx
            self.code.extend_from_slice(&[0x0F, 0x83]); // jnc fail_lane
            let patch = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            fail_jumps.push(patch);
        }

        self.emit_mov_rax_imm(1);
        self.emit_store_rax_to_slot(dst, slot_map);
        self.emit_mov_rax_imm(4);
        self.emit_store_rax_to_slot(lanes_checked, slot_map);
        self.code.push(0xE9);
        let success_done = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        let mut fail_done = Vec::with_capacity(4);
        for (lane, patch) in fail_jumps.into_iter().enumerate() {
            let fail_pos = self.code.len();
            patch_rel32(&mut self.code, patch, fail_pos);
            self.emit_mov_rax_imm(0);
            self.emit_store_rax_to_slot(dst, slot_map);
            self.emit_mov_rax_imm(lane as u64);
            self.emit_store_rax_to_slot(lanes_checked, slot_map);
            self.code.push(0xE9);
            let done_patch = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            fail_done.push(done_patch);
        }

        let done = self.code.len();
        patch_rel32(&mut self.code, success_done, done);
        for patch in fail_done {
            patch_rel32(&mut self.code, patch, done);
        }
    }

    fn emit_runtime_bloom_classic4_check_jump(
        &mut self,
        filter_slots: &[usize],
        hash: &RuntimeOperand,
        target: usize,
        slot_map: &RuntimeSlotMap,
        jump_patches: &mut Vec<(usize, usize)>,
    ) -> bool {
        if filter_slots.len() != 64 {
            return false;
        }
        let Some((base_disp, needs_neg, 8)) = self.contiguous_stack_base_access(filter_slots, slot_map)
        else {
            return false;
        };

        self.emit_load_operand_to_rax(hash, slot_map);
        for lane in 0..4u8 {
            let bit_shift = lane * 13;
            self.emit_mov_reg_reg(1, 0); // rcx = hash / bit index
            if bit_shift != 0 {
                self.emit_shr_reg_imm8(1, bit_shift);
            }
            self.emit_mov_reg_reg(2, 0); // rdx = hash / word index
            self.emit_shr_reg_imm8(2, bit_shift + 6);
            self.emit_and_reg_imm32(2, 63);
            if needs_neg {
                self.emit_neg_reg(2);
            }
            self.emit_indexed_rbp_mem_reg_with_index(2, 2, base_disp, 8, true);
            self.code.extend_from_slice(&[0x48, 0x0F, 0xA3, 0xCA]); // bt rdx, rcx
            self.code.extend_from_slice(&[0x0F, 0x83]); // jnc target
            let disp_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            jump_patches.push((disp_pos, target));
        }
        true
    }

    fn emit_runtime_bloom_prepare_index_reg(
        &mut self,
        block_mask: u32,
        needs_neg: bool,
        word_offset: i32,
        hash: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        self.emit_load_operand_to_rax(hash, slot_map);
        self.emit_and_reg_imm32(0, block_mask);
        self.emit_shl_reg_imm8(0, 2);
        if word_offset != 0 {
            self.emit_add_reg_imm32(0, word_offset);
        }
        self.emit_mov_reg_reg(2, 0); // rdx = word index
        if needs_neg {
            self.emit_neg_reg(2);
        }
    }

    fn emit_runtime_bloom_prepare_bit_reg(
        &mut self,
        salt: u32,
        hash: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        self.emit_load_operand_to_rax(hash, slot_map);
        self.emit_mov_reg32_reg32(0, 0); // zero-extend low 32 bits of the hash
        self.emit_imul_reg_reg_imm32(0, 0, salt as i32);
        self.emit_shr_reg_imm8(0, 27);
        self.emit_and_reg_imm32(0, 31);
        self.emit_mov_reg_reg(1, 0); // rcx = bit index
    }

    fn emit_runtime_bloom_insert_word(
        &mut self,
        base_disp: i32,
        block_mask: u32,
        needs_neg: bool,
        word_offset: i32,
        salt: u32,
        hash: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        self.emit_runtime_bloom_prepare_index_reg(
            block_mask,
            needs_neg,
            word_offset,
            hash,
            slot_map,
        );
        self.emit_runtime_bloom_prepare_bit_reg(salt, hash, slot_map);
        self.emit_indexed_rbp_mem_reg_with_index(0, 2, base_disp, 8, true); // rax = word
        self.emit_bts_rax_rcx();
        self.emit_indexed_rbp_mem_reg_with_index(0, 2, base_disp, 8, false); // word = rax
    }

    fn emit_runtime_bloom_check_word(
        &mut self,
        base_disp: i32,
        block_mask: u32,
        needs_neg: bool,
        word_offset: i32,
        salt: u32,
        hash: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) -> usize {
        self.emit_runtime_bloom_prepare_index_reg(
            block_mask,
            needs_neg,
            word_offset,
            hash,
            slot_map,
        );
        self.emit_runtime_bloom_prepare_bit_reg(salt, hash, slot_map);
        self.emit_indexed_rbp_mem_reg_with_index(0, 2, base_disp, 8, true); // rax = word
        self.emit_bt_rax_rcx();
        self.code.extend_from_slice(&[0x0F, 0x83]); // jnc fail
        let fail_disp = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        fail_disp
    }

    fn emit_runtime_hash_ctrl_group_probe(
        &mut self,
        dst_mask: usize,
        ctrl_slots: &[usize],
        group_start: &RuntimeOperand,
        fingerprint: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        if ctrl_slots.len() < 16 || !ctrl_slots.len().is_power_of_two() {
            self.emit_exit(255);
            return;
        }
        let Some((base_disp, needs_neg, width)) = self.contiguous_stack_base_access(ctrl_slots, slot_map) else {
            self.emit_exit(255);
            return;
        };
        let ctrl_mask = (ctrl_slots.len() as u32).wrapping_sub(1);
        if width == 1 {
            self.emit_runtime_hash_ctrl_group_probe_packed(
                dst_mask,
                base_disp,
                needs_neg,
                ctrl_mask,
                group_start,
                fingerprint,
                slot_map,
            );
            return;
        }

        self.emit_sub_rsp_imm32(24);
        self.emit_load_operand_to_rax(group_start, slot_map);
        self.emit_and_reg_imm32(0, ctrl_mask);
        self.emit_mov_rsp_disp_from_rax(16); // start idx
        self.emit_load_operand_to_rax(fingerprint, slot_map);
        self.emit_and_reg_imm32(0, 0xFF);
        self.emit_mov_rsp_disp_from_rax(0);
        self.emit_mov_rsp_disp_from_rax(8);
        self.emit_movdqu_xmm_rsp_disp(1, 0, true); // xmm1 = [fp, fp]

        self.emit_xor_reg_reg(2, 2); // rdx = output bitmask
        for pair in 0..8u32 {
            self.emit_mov_rax_from_rsp_disp(16);
            self.emit_mov_reg_reg(1, 0); // rcx
            if pair != 0 {
                self.emit_add_reg_imm32(1, (pair * 2) as i32);
            }
            self.emit_and_reg_imm32(1, ctrl_mask);
            if needs_neg {
                self.emit_neg_reg(1);
            }

            self.emit_movdqu_indexed_rbp_mem_xmm(0, 1, base_disp, 8, true);
            self.emit_pcmpeqb_xmm(0, 1);
            self.emit_pmovmskb_eax_xmm(0);
            // Branchless byte-lane mask extraction:
            // low  lane hit: ((mask & 0xFF) + 1) >> 8
            // high lane hit: ((((mask >> 8) & 0xFF) + 1) & 0x100) >> 7  (already << 1)
            self.emit_mov_reg32_reg32(1, 0); // ecx = eax
            self.emit_and_reg_imm32(1, 0x00FF);
            self.emit_add_reg_imm32(1, 1);
            self.emit_shr_reg_imm8(1, 8);

            self.emit_mov_reg32_reg32(6, 0); // esi = eax
            self.emit_shr_reg_imm8(6, 8);
            self.emit_and_reg_imm32(6, 0x00FF);
            self.emit_add_reg_imm32(6, 1);
            self.emit_and_reg_imm32(6, 0x0100);
            self.emit_shr_reg_imm8(6, 7);

            self.emit_or_reg_reg(1, 6); // ecx |= esi
            if pair != 0 {
                self.emit_shl_reg_imm8(1, (pair * 2) as u8);
            }
            self.emit_or_reg_reg(2, 1); // rdx |= rcx
        }

        self.emit_mov_reg_reg(0, 2);
        self.emit_store_rax_to_slot(dst_mask, slot_map);
        self.emit_add_rsp_imm32(24);
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_runtime_hash_ctrl_group_probe_packed(
        &mut self,
        dst_mask: usize,
        base_disp: i32,
        needs_neg: bool,
        ctrl_mask: u32,
        group_start: &RuntimeOperand,
        fingerprint: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        self.emit_sub_rsp_imm32(24);
        self.emit_load_operand_to_rax(group_start, slot_map);
        self.emit_and_reg_imm32(0, ctrl_mask);
        self.emit_mov_rsp_disp_from_rax(16); // logical start

        self.emit_load_operand_to_rax(fingerprint, slot_map);
        self.emit_and_reg_imm32(0, 0xff);
        self.emit_mov_rsp_disp_from_rax(0); // scalar fingerprint
        self.emit_mov_rcx_imm(0x0101_0101_0101_0101);
        self.emit_imul_reg_reg(0, 1); // broadcast byte through the qword
        self.emit_mov_rsp_disp_from_rax(8);
        self.emit_mov_rsp_disp_from_rax(0);
        self.emit_movdqu_xmm_rsp_disp(1, 0, true); // xmm1 = 16 fingerprint bytes

        let fallback_jump = if !needs_neg {
            self.emit_mov_rax_from_rsp_disp(16);
            self.emit_alu_reg_imm(7, 0, ctrl_mask.saturating_sub(15).to_le_bytes());
            self.code.extend_from_slice(&[0x0F, 0x87]); // ja fallback
            let patch = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            self.emit_mov_reg_reg(1, 0); // rcx = start
            self.emit_movdqu_indexed_rbp_mem_xmm(0, 1, base_disp, 1, true);
            self.emit_pcmpeqb_xmm(0, 1);
            self.emit_pmovmskb_eax_xmm(0);
            self.emit_mov_reg_reg(2, 0); // rdx = complete 16-lane mask
            self.code.push(0xE9);
            let done_jump = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());

            let fallback = self.code.len();
            patch_rel32(&mut self.code, patch, fallback);
            Some(done_jump)
        } else {
            None
        };

        self.emit_mov_rax_from_rsp_disp(0);
        self.emit_mov_reg_reg(6, 0); // rsi = scalar fingerprint
        self.emit_and_reg_imm32(6, 0xff);
        self.emit_xor_reg_reg(2, 2); // rdx = mask
        for lane in 0..16u8 {
            self.emit_mov_rax_from_rsp_disp(16);
            self.emit_mov_reg_reg(1, 0);
            if lane != 0 {
                self.emit_add_reg_imm32(1, i32::from(lane));
            }
            self.emit_and_reg_imm32(1, ctrl_mask);
            if needs_neg {
                self.emit_neg_reg(1);
            }
            self.emit_indexed_rbp_mem_reg_with_index(0, 1, base_disp, 1, true);
            self.emit_cmp_reg_reg(0, 6);
            self.code.extend_from_slice(&[0x0F, 0x94, 0xC0]); // sete al
            self.code.extend_from_slice(&[0x0F, 0xB6, 0xC0]); // movzx eax, al
            if lane != 0 {
                self.emit_shl_reg_imm8(0, lane);
            }
            self.emit_or_reg_reg(2, 0);
        }

        if let Some(done_jump) = fallback_jump {
            let done = self.code.len();
            patch_rel32(&mut self.code, done_jump, done);
        }
        self.emit_mov_reg_reg(0, 2);
        self.emit_store_rax_to_slot(dst_mask, slot_map);
        self.emit_add_rsp_imm32(24);
    }

    fn emit_runtime_join_select_adaptive(
        &mut self,
        dst: usize,
        build_rows: &RuntimeOperand,
        probe_rows: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        self.emit_load_operand_to_rax(build_rows, slot_map);
        self.emit_alu_reg_imm(7, 0, 128_u32.to_le_bytes()); // cmp rax, 128
        self.code.extend_from_slice(&[0x0F, 0x82]); // jb false
        let build_fail = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        self.emit_load_operand_to_rcx(probe_rows, slot_map);
        self.emit_alu_reg_imm(7, 1, 200_000_u32.to_le_bytes()); // cmp rcx, 200000
        self.code.extend_from_slice(&[0x0F, 0x82]); // jb false
        let probe_fail = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        self.emit_mov_rax_imm(1);
        self.code.push(0xE9);
        let done_jump = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        let false_pos = self.code.len();
        self.emit_mov_rax_imm(0);
        let done_pos = self.code.len();
        patch_rel32(&mut self.code, build_fail, false_pos);
        patch_rel32(&mut self.code, probe_fail, false_pos);
        patch_rel32(&mut self.code, done_jump, done_pos);
        self.emit_store_rax_to_slot(dst, slot_map);
    }

    fn emit_runtime_alloc(
        &mut self,
        dst: usize,
        size: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        let preserved = self.emit_preserve_slot_regs_for_heap_syscall(slot_map);
        self.emit_load_operand_to_reg(6, size, slot_map); // rsi = size
        self.code.push(0xE8); // call shared allocator
        let call_disp = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.runtime_allocator
            .as_mut()
            .expect("runtime allocation requires allocator frame")
            .alloc_call_patches
            .push(call_disp);

        self.emit_restore_preserved_regs_reverse(&preserved);
        self.emit_store_rax_to_slot(dst, slot_map);
    }

    fn emit_runtime_clock_nanos(&mut self, clock_id: u64, slot_map: &RuntimeSlotMap) {
        let preserved = self.emit_preserve_slot_regs_for_heap_syscall(slot_map);
        self.code.extend_from_slice(&[0x48, 0x83, 0xEC, 0x10]); // sub rsp, 16
        self.emit_mov_reg_imm64(7, clock_id); // rdi = clock id
        self.code.extend_from_slice(&[0x48, 0x89, 0xE6]); // mov rsi, rsp
        self.emit_mov_rax_imm(self.runtime.syscalls.clock_gettime); // SYS_clock_gettime
        self.emit_kernel_call();
        self.code.extend_from_slice(&[0x48, 0x85, 0xC0]); // test rax,rax
        self.code.extend_from_slice(&[0x0F, 0x88]); // js failure
        let failure_patch = self.code.len();
        self.code.extend_from_slice(&0i32.to_le_bytes());
        self.code.extend_from_slice(&[0x48, 0x8B, 0x04, 0x24]); // mov rax, [rsp]
        self.code.extend_from_slice(&[0x48, 0x69, 0xC0]); // imul rax, rax, 1e9
        self.code.extend_from_slice(&1_000_000_000u32.to_le_bytes());
        self.code.extend_from_slice(&[0x48, 0x03, 0x44, 0x24, 0x08]); // add rax,[rsp+8]
        self.code.push(0xE9);
        let done_patch = self.code.len();
        self.code.extend_from_slice(&0i32.to_le_bytes());
        let failure = self.code.len();
        patch_rel32(&mut self.code, failure_patch, failure);
        self.emit_mov_rax_imm(u64::MAX);
        let done = self.code.len();
        patch_rel32(&mut self.code, done_patch, done);
        self.code.extend_from_slice(&[0x48, 0x83, 0xC4, 0x10]); // add rsp, 16
        self.emit_restore_preserved_regs_reverse(&preserved);
    }

    fn emit_runtime_file_open(
        &mut self,
        dst: usize,
        path_ptr: &RuntimeOperand,
        flags: u32,
        mode: u32,
        slot_map: &RuntimeSlotMap,
    ) {
        let preserved = self.emit_preserve_slot_regs_for_heap_syscall(slot_map);
        self.emit_load_operand_to_reg(6, path_ptr, slot_map); // rsi = NUL-terminated path
        self.emit_mov_rax_imm(self.runtime.at_fdcwd as u64);
        self.emit_mov_reg_reg(7, 0); // rdi = AT_FDCWD
        let target_flags = if flags == 577 {
            self.runtime.file_create_flags
        } else {
            self.runtime.file_read_flags
        };
        self.emit_mov_rdx_imm(u64::from(target_flags));
        self.emit_mov_reg_imm(GpReg::R10, u64::from(mode));
        let retry = self.code.len();
        self.emit_mov_rax_imm(self.runtime.syscalls.openat); // SYS_openat
        self.emit_kernel_call();
        self.emit_alu_reg_imm(
            7,
            0,
            (self.runtime.interrupted_error as i32).to_le_bytes(),
        ); // cmp rax, -EINTR
        self.code.extend_from_slice(&[0x0F, 0x84]); // je retry
        let retry_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        patch_rel32(&mut self.code, retry_patch, retry);
        self.emit_restore_preserved_regs_reverse(&preserved);
        self.emit_store_rax_to_slot(dst, slot_map);
    }

    fn emit_runtime_file_io(
        &mut self,
        dst: usize,
        fd: &RuntimeOperand,
        ptr: &RuntimeOperand,
        len: &RuntimeOperand,
        syscall: u64,
        slot_map: &RuntimeSlotMap,
    ) {
        let preserved = self.emit_preserve_slot_regs_for_heap_syscall(slot_map);
        self.emit_load_operand_to_rax(fd, slot_map);
        self.emit_push_reg(0);
        self.emit_load_operand_to_rax(ptr, slot_map);
        self.emit_push_reg(0);
        self.emit_load_operand_to_rax(len, slot_map);
        self.emit_mov_reg_reg(2, 0); // rdx = remaining bytes
        self.emit_pop_reg(6); // rsi = current pointer
        self.emit_pop_reg(7); // rdi = fd
        self.emit_xor_reg_reg(8, 8); // r8 = completed bytes

        let loop_start = self.code.len();
        self.code.extend_from_slice(&[0x48, 0x85, 0xD2]); // test rdx, rdx
        self.code.extend_from_slice(&[0x0F, 0x84]); // jz success
        let empty_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.emit_mov_rax_imm(syscall);
        self.emit_kernel_call();
        self.code.extend_from_slice(&[0x48, 0x85, 0xC0]); // test rax, rax
        self.code.extend_from_slice(&[0x0F, 0x88]); // js error
        let error_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.code.extend_from_slice(&[0x0F, 0x84]); // jz EOF/success
        let eof_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.emit_add_reg_reg(6, 0); // rsi += transferred
        self.emit_sub_reg_reg(2, 0); // rdx -= transferred
        self.emit_add_reg_reg(8, 0); // r8 += transferred
        self.code.push(0xE9);
        let loop_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        patch_rel32(&mut self.code, loop_patch, loop_start);

        let error = self.code.len();
        patch_rel32(&mut self.code, error_patch, error);
        self.emit_alu_reg_imm(
            7,
            0,
            (self.runtime.interrupted_error as i32).to_le_bytes(),
        ); // cmp rax, -EINTR
        self.code.extend_from_slice(&[0x0F, 0x84]); // je retry
        let interrupted_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        patch_rel32(&mut self.code, interrupted_patch, loop_start);
        self.emit_mov_rax_imm(u64::MAX);
        self.code.push(0xE9);
        let failure_done = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        let success = self.code.len();
        patch_rel32(&mut self.code, empty_patch, success);
        patch_rel32(&mut self.code, eof_patch, success);
        self.emit_mov_reg_reg(0, 8);
        let done = self.code.len();
        patch_rel32(&mut self.code, failure_done, done);
        self.emit_restore_preserved_regs_reverse(&preserved);
        self.emit_store_rax_to_slot(dst, slot_map);
    }

    fn emit_runtime_file_close(
        &mut self,
        fd: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        let preserved = self.emit_preserve_slot_regs_for_heap_syscall(slot_map);
        self.emit_load_operand_to_reg(7, fd, slot_map);
        self.code.extend_from_slice(&[0x48, 0x85, 0xFF]); // test rdi, rdi
        self.code.extend_from_slice(&[0x0F, 0x88]); // js done
        let invalid_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.emit_mov_rax_imm(self.runtime.syscalls.close); // SYS_close
        self.emit_kernel_call();
        let done = self.code.len();
        patch_rel32(&mut self.code, invalid_patch, done);
        self.emit_restore_preserved_regs_reverse(&preserved);
    }

    fn runtime_thread_mapping_bytes(stack_size: usize) -> usize {
        const WORKER_CALL_STACK: usize = 1024 * 1024;
        let required = 16usize
            .saturating_add(WORKER_CALL_STACK)
            .saturating_add(stack_size);
        required.saturating_add(4095) / 4096 * 4096
    }

    /// Emit a target-runtime native worker. The Linux mapping is laid out as
    /// `[clear_tid, result, worker call stack..., copied Aziky frame]`.
    fn emit_runtime_thread_spawn(
        &mut self,
        handle_dst: usize,
        return_slot: Option<usize>,
        stack_size: usize,
        allocator_frame: Option<RuntimeAllocatorFrame>,
        thread_context_disp: Option<i32>,
        slot_map: &RuntimeSlotMap,
    ) -> usize {
        let mapping_bytes = Self::runtime_thread_mapping_bytes(stack_size);
        let mapping_i32 = i32::try_from(mapping_bytes).expect("thread mapping size");
        let child_rsp_offset = mapping_i32 - stack_size as i32;

        self.emit_mov_rax_imm(self.runtime.syscalls.mmap); // mmap
        self.emit_mov_rdi_imm(0);
        self.emit_mov_rsi_imm(mapping_bytes as u64);
        self.emit_mov_reg_imm64(2, self.runtime.prot_read_write);
        self.emit_mov_reg_imm64(10, self.runtime.mmap_private_anonymous);
        self.emit_mov_reg_imm64(8, u64::MAX);
        self.emit_mov_reg_imm64(9, 0);
        self.emit_kernel_call();
        self.code.extend_from_slice(&[0x48, 0x85, 0xC0]);
        self.code.extend_from_slice(&[0x0F, 0x88]);
        let mmap_failure_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        self.code.extend_from_slice(&[0x48, 0x89, 0xC3]); // rbx = mapping
        self.code.extend_from_slice(&[0xC7, 0x03, 0, 0, 0, 0]);
        self.code
            .extend_from_slice(&[0x48, 0xC7, 0x43, 0x08, 109, 0, 0, 0]);
        if stack_size != 0 {
            self.code.extend_from_slice(&[0x48, 0x8D, 0xB5]);
            self.code.extend_from_slice(&(-(stack_size as i32)).to_le_bytes());
            self.code.extend_from_slice(&[0x48, 0x8D, 0xBB]);
            self.code.extend_from_slice(&child_rsp_offset.to_le_bytes());
            self.emit_mov_reg_imm64(1, (stack_size / 8) as u64);
            self.code.extend_from_slice(&[0xF3, 0x48, 0xA5]);
        }

        self.emit_mov_rax_imm(self.runtime.syscalls.clone); // clone
        self.emit_mov_rdi_imm(self.runtime.clone_thread_flags);
        self.code.extend_from_slice(&[0x48, 0x8D, 0xB3]);
        self.code.extend_from_slice(&child_rsp_offset.to_le_bytes());
        self.code.extend_from_slice(&[0x48, 0x89, 0xDA]); // parent_tid
        self.code.extend_from_slice(&[0x49, 0x89, 0xDA]); // child_tid
        self.emit_mov_reg_imm64(8, 0);
        self.emit_kernel_call();
        self.code.extend_from_slice(&[0x48, 0x85, 0xC0]);
        self.code.extend_from_slice(&[0x0F, 0x88]);
        let clone_failure_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.code.extend_from_slice(&[0x0F, 0x84]);
        let child_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        self.code.extend_from_slice(&[0x48, 0x89, 0xD8]);
        self.emit_store_rax_to_slot(handle_dst, slot_map);
        self.code.push(0xE9);
        let parent_done_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        let child_label = self.code.len();
        patch_rel32(&mut self.code, child_patch, child_label);
        self.code.extend_from_slice(&[0x48, 0x8D, 0xAB]);
        self.code.extend_from_slice(&mapping_i32.to_le_bytes()); // rbp = mapping top
        self.code.extend_from_slice(&[0x48, 0x8D, 0xA5]);
        self.code.extend_from_slice(&(-(stack_size as i32)).to_le_bytes());
        if let Some(frame) = allocator_frame {
            for disp in [frame.head_disp, frame.cursor_disp, frame.end_disp] {
                self.code.extend_from_slice(&[0x48, 0xC7, 0x85]);
                self.code.extend_from_slice(&disp.to_le_bytes());
                self.code.extend_from_slice(&0_i32.to_le_bytes());
            }
        }
        if let Some(disp) = thread_context_disp {
            self.code.extend_from_slice(&[0x48, 0x89, 0x9D]);
            self.code.extend_from_slice(&disp.to_le_bytes()); // worker control mapping
        }
        self.code.push(0x53);
        self.code.push(0xE8);
        let target_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.code.push(0x5B);
        if let Some(slot) = return_slot {
            self.emit_load_slot_to_rax(slot, slot_map);
        } else {
            self.emit_mov_rax_imm(0);
        }
        self.code.extend_from_slice(&[0x48, 0x89, 0x43, 0x08]);
        if self.runtime_allocator.is_some() {
            self.code.push(0x53);
            self.emit_runtime_allocator_teardown_call();
            self.code.push(0x5B);
        }
        self.emit_mov_rdi_imm(0);
        self.emit_mov_rax_imm(self.runtime.syscalls.thread_exit);
        self.emit_kernel_call();

        let clone_failure = self.code.len();
        patch_rel32(&mut self.code, clone_failure_patch, clone_failure);
        self.emit_mov_rax_imm(self.runtime.syscalls.munmap);
        self.code.extend_from_slice(&[0x48, 0x89, 0xDF]);
        self.emit_mov_rsi_imm(mapping_bytes as u64);
        self.emit_kernel_call();
        let failure = self.code.len();
        patch_rel32(&mut self.code, mmap_failure_patch, failure);
        self.emit_mov_rdi_imm(109);
        self.emit_mov_rax_imm(self.runtime.syscalls.exit);
        self.emit_kernel_call();

        let done = self.code.len();
        patch_rel32(&mut self.code, parent_done_patch, done);
        target_patch
    }

    fn emit_runtime_thread_join(
        &mut self,
        dst: usize,
        handle: &RuntimeOperand,
        stack_size: usize,
        slot_map: &RuntimeSlotMap,
    ) {
        let mapping_bytes = Self::runtime_thread_mapping_bytes(stack_size);
        self.emit_load_operand_to_rax(handle, slot_map);
        self.code.extend_from_slice(&[0x48, 0x89, 0xC3]);
        let wait = self.code.len();
        self.code.extend_from_slice(&[0x8B, 0x13]);
        self.code.extend_from_slice(&[0x85, 0xD2]);
        self.code.extend_from_slice(&[0x0F, 0x84]);
        let complete_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.emit_mov_rax_imm(self.runtime.syscalls.futex); // futex WAIT
        self.code.extend_from_slice(&[0x48, 0x89, 0xDF]);
        self.emit_mov_rsi_imm(self.runtime.futex_wait);
        self.emit_mov_reg_imm64(10, 0);
        self.emit_mov_reg_imm64(8, 0);
        self.emit_mov_reg_imm64(9, 0);
        self.emit_kernel_call();
        self.code.push(0xE9);
        let retry_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        patch_rel32(&mut self.code, retry_patch, wait);
        let complete = self.code.len();
        patch_rel32(&mut self.code, complete_patch, complete);
        self.code.extend_from_slice(&[0x4C, 0x8B, 0x63, 0x08]);
        self.emit_mov_rax_imm(self.runtime.syscalls.munmap);
        self.code.extend_from_slice(&[0x48, 0x89, 0xDF]);
        self.emit_mov_rsi_imm(mapping_bytes as u64);
        self.emit_kernel_call();
        self.emit_store_reg_to_slot(12, dst, slot_map);
    }

    fn emit_runtime_channel_create(
        &mut self,
        dst: usize,
        capacity: &RuntimeOperand,
        unbounded: bool,
        thread_context_disp: Option<i32>,
        slot_map: &RuntimeSlotMap,
    ) {
        if unbounded {
            self.emit_mov_reg_imm64(12, 1 << 30); // virtual u64 cells; committed on demand
        } else {
            self.emit_load_operand_to_reg(12, capacity, slot_map);
            self.code.extend_from_slice(&[0x4D, 0x85, 0xE4]); // test r12,r12
            self.code.extend_from_slice(&[0x0F, 0x84]);
            let invalid = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            self.code.extend_from_slice(&[0x49, 0x81, 0xFC]); // cmp r12, 0x1fffffff
            self.code.extend_from_slice(&0x1fff_ffff_u32.to_le_bytes());
            self.code.extend_from_slice(&[0x0F, 0x87]); // ja invalid
            let too_large = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            let valid = self.code.len();
            patch_rel32(&mut self.code, invalid, valid + 5);
            patch_rel32(&mut self.code, too_large, valid + 5);
            self.code.push(0xE9);
            let continue_patch = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            self.emit_runtime_publish_worker_status(110, thread_context_disp);
            self.emit_mov_rdi_imm(110);
            self.emit_mov_rax_imm(self.runtime.syscalls.exit);
            self.emit_kernel_call();
            let continued = self.code.len();
            patch_rel32(&mut self.code, continue_patch, continued);
        }

        self.code.extend_from_slice(&[0x4D, 0x89, 0xE5]); // r13 = r12
        self.code.extend_from_slice(&[0x49, 0xC1, 0xE5, 0x03]); // * 8
        self.code.extend_from_slice(&[0x49, 0x83, 0xC5, 0x40]); // + header
        self.code.extend_from_slice(&[0x49, 0x81, 0xC5, 0xFF, 0x0F, 0, 0]);
        self.code
            .extend_from_slice(&[0x49, 0x81, 0xE5, 0, 0xF0, 0xFF, 0xFF]); // page align
        self.emit_mov_rax_imm(self.runtime.syscalls.mmap);
        self.emit_mov_rdi_imm(0);
        self.code.extend_from_slice(&[0x4C, 0x89, 0xEE]); // rsi = r13
        self.emit_mov_reg_imm64(2, self.runtime.prot_read_write);
        self.emit_mov_reg_imm64(
            10,
            self.runtime.mmap_private_anonymous
                | if unbounded {
                    self.runtime.mmap_no_reserve
                } else {
                    0
                },
        );
        self.emit_mov_reg_imm64(8, u64::MAX);
        self.emit_mov_reg_imm64(9, 0);
        self.emit_kernel_call();
        self.code.extend_from_slice(&[0x48, 0x85, 0xC0]);
        self.code.extend_from_slice(&[0x0F, 0x88]);
        let mmap_failure = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.code.extend_from_slice(&[0x48, 0x89, 0xC3]);
        self.code.extend_from_slice(&[0x4C, 0x89, 0x23]); // capacity
        for disp in [8u8, 16, 24, 32] {
            self.code.extend_from_slice(&[0x48, 0xC7, 0x43, disp, 0, 0, 0, 0]);
        }
        self.code.extend_from_slice(&[
            0x48,
            0xC7,
            0x43,
            40,
            u8::from(unbounded),
            0,
            0,
            0,
        ]);
        self.code.extend_from_slice(&[0x4C, 0x89, 0x6B, 48]); // mapping bytes
        self.code.extend_from_slice(&[0x48, 0x89, 0xD8]);
        self.emit_store_rax_to_slot(dst, slot_map);
        self.code.push(0xE9);
        let done_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        let failure = self.code.len();
        patch_rel32(&mut self.code, mmap_failure, failure);
        self.emit_runtime_publish_worker_status(110, thread_context_disp);
        self.emit_mov_rdi_imm(110);
        self.emit_mov_rax_imm(self.runtime.syscalls.exit);
        self.emit_kernel_call();
        let done = self.code.len();
        patch_rel32(&mut self.code, done_patch, done);
    }

    fn emit_runtime_publish_worker_status(&mut self, code: u32, context_disp: Option<i32>) {
        let Some(context_disp) = context_disp else {
            return;
        };
        self.code.extend_from_slice(&[0x48, 0x8B, 0x8D]);
        self.code.extend_from_slice(&context_disp.to_le_bytes());
        self.code.extend_from_slice(&[0x48, 0x85, 0xC9]);
        self.code.extend_from_slice(&[0x0F, 0x84]);
        let main_task_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.code.extend_from_slice(&[0x48, 0xC7, 0x41, 0x08]);
        self.code.extend_from_slice(&code.to_le_bytes());
        let done = self.code.len();
        patch_rel32(&mut self.code, main_task_patch, done);
    }

    fn emit_runtime_channel_send(
        &mut self,
        handle: &RuntimeOperand,
        value: &RuntimeOperand,
        thread_context_disp: Option<i32>,
        slot_map: &RuntimeSlotMap,
    ) {
        self.emit_load_operand_to_reg(3, handle, slot_map);
        self.emit_load_operand_to_reg(12, value, slot_map);
        let retry = self.code.len();
        self.code.extend_from_slice(&[0x48, 0x83, 0x7B, 32, 0]);
        self.code.extend_from_slice(&[0x0F, 0x85]);
        let closed_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.code.extend_from_slice(&[0x4C, 0x8B, 0x6B, 16]); // tail
        self.code.extend_from_slice(&[0x4C, 0x8B, 0x73, 8]); // head
        self.code.extend_from_slice(&[0x4D, 0x89, 0xEF]);
        self.code.extend_from_slice(&[0x4D, 0x29, 0xF7]); // used
        self.code.extend_from_slice(&[0x4C, 0x3B, 0x3B]); // cmp used, capacity
        self.code.extend_from_slice(&[0x0F, 0x82]);
        let writable_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.emit_mov_rax_imm(self.runtime.syscalls.futex);
        self.code.extend_from_slice(&[0x48, 0x8D, 0x7B, 8]);
        self.emit_mov_rsi_imm(self.runtime.futex_wait);
        self.code.extend_from_slice(&[0x44, 0x89, 0xF2]); // expected head low32
        self.emit_mov_reg_imm64(10, 0);
        self.emit_kernel_call();
        self.code.push(0xE9);
        let retry_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        patch_rel32(&mut self.code, retry_patch, retry);
        let writable = self.code.len();
        patch_rel32(&mut self.code, writable_patch, writable);
        self.code.extend_from_slice(&[0x4C, 0x89, 0xE8]); // rax = tail
        self.code.extend_from_slice(&[0x31, 0xD2]);
        self.code.extend_from_slice(&[0x48, 0xF7, 0x33]); // div [capacity]
        self.code.extend_from_slice(&[0x4C, 0x89, 0x64, 0xD3, 64]);
        self.code.extend_from_slice(&[0x49, 0xFF, 0xC5]);
        self.code.extend_from_slice(&[0x4C, 0x89, 0x6B, 16]);
        self.emit_mov_rax_imm(self.runtime.syscalls.futex);
        self.code.extend_from_slice(&[0x48, 0x8D, 0x7B, 16]);
        self.emit_mov_rsi_imm(self.runtime.futex_wake);
        self.emit_mov_reg_imm64(2, 1);
        self.emit_kernel_call();
        self.code.push(0xE9);
        let done_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        let closed = self.code.len();
        patch_rel32(&mut self.code, closed_patch, closed);
        self.emit_runtime_publish_worker_status(111, thread_context_disp);
        self.emit_mov_rdi_imm(111);
        self.emit_mov_rax_imm(self.runtime.syscalls.exit);
        self.emit_kernel_call();
        let done = self.code.len();
        patch_rel32(&mut self.code, done_patch, done);
    }

    fn emit_runtime_channel_recv(
        &mut self,
        dst: usize,
        handle: &RuntimeOperand,
        thread_context_disp: Option<i32>,
        slot_map: &RuntimeSlotMap,
    ) {
        self.emit_load_operand_to_reg(3, handle, slot_map);
        let retry = self.code.len();
        self.code.extend_from_slice(&[0x4C, 0x8B, 0x6B, 8]); // head
        self.code.extend_from_slice(&[0x4C, 0x8B, 0x73, 16]); // tail
        self.code.extend_from_slice(&[0x4D, 0x39, 0xF5]);
        self.code.extend_from_slice(&[0x0F, 0x82]); // head < tail
        let readable_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.code.extend_from_slice(&[0x48, 0x83, 0x7B, 24, 0]);
        self.code.extend_from_slice(&[0x0F, 0x85]);
        let closed_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.emit_mov_rax_imm(self.runtime.syscalls.futex);
        self.code.extend_from_slice(&[0x48, 0x8D, 0x7B, 16]);
        self.emit_mov_rsi_imm(self.runtime.futex_wait);
        self.code.extend_from_slice(&[0x44, 0x89, 0xF2]);
        self.emit_mov_reg_imm64(10, 0);
        self.emit_kernel_call();
        self.code.push(0xE9);
        let retry_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        patch_rel32(&mut self.code, retry_patch, retry);
        let readable = self.code.len();
        patch_rel32(&mut self.code, readable_patch, readable);
        self.code.extend_from_slice(&[0x4C, 0x89, 0xE8]);
        self.code.extend_from_slice(&[0x31, 0xD2]);
        self.code.extend_from_slice(&[0x48, 0xF7, 0x33]);
        self.code.extend_from_slice(&[0x4C, 0x8B, 0x64, 0xD3, 64]);
        self.code.extend_from_slice(&[0x49, 0xFF, 0xC5]);
        self.code.extend_from_slice(&[0x4C, 0x89, 0x6B, 8]);
        self.emit_mov_rax_imm(self.runtime.syscalls.futex);
        self.code.extend_from_slice(&[0x48, 0x8D, 0x7B, 8]);
        self.emit_mov_rsi_imm(self.runtime.futex_wake);
        self.emit_mov_reg_imm64(2, 1);
        self.emit_kernel_call();
        self.emit_store_reg_to_slot(12, dst, slot_map);
        self.code.push(0xE9);
        let done_patch = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        let closed = self.code.len();
        patch_rel32(&mut self.code, closed_patch, closed);
        self.emit_runtime_publish_worker_status(112, thread_context_disp);
        self.emit_mov_rdi_imm(112);
        self.emit_mov_rax_imm(self.runtime.syscalls.exit);
        self.emit_kernel_call();
        let done = self.code.len();
        patch_rel32(&mut self.code, done_patch, done);
    }

    fn emit_runtime_channel_close(
        &mut self,
        handle: &RuntimeOperand,
        sender: bool,
        slot_map: &RuntimeSlotMap,
    ) {
        self.emit_load_operand_to_reg(3, handle, slot_map);
        let (closed_disp, wake_disp) = if sender { (24u8, 16u8) } else { (32, 8) };
        self.code
            .extend_from_slice(&[0x48, 0xC7, 0x43, closed_disp, 1, 0, 0, 0]);
        self.emit_mov_rax_imm(self.runtime.syscalls.futex);
        self.code.extend_from_slice(&[0x48, 0x8D, 0x7B, wake_disp]);
        self.emit_mov_rsi_imm(self.runtime.futex_wake);
        self.emit_mov_reg_imm64(2, i32::MAX as u64);
        self.emit_kernel_call();
    }

    fn emit_runtime_channel_destroy(
        &mut self,
        handle: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        self.emit_load_operand_to_reg(3, handle, slot_map);
        self.code.extend_from_slice(&[0x4C, 0x8B, 0x6B, 48]);
        self.emit_mov_rax_imm(self.runtime.syscalls.munmap);
        self.code.extend_from_slice(&[0x48, 0x89, 0xDF]);
        self.code.extend_from_slice(&[0x4C, 0x89, 0xEE]);
        self.emit_kernel_call();
    }

    fn emit_runtime_heap_load_int(
        &mut self,
        dst: usize,
        ptr: &RuntimeOperand,
        index: &RuntimeOperand,
        bytes: u8,
        slot_map: &RuntimeSlotMap,
    ) {
        let preserved = self.emit_preserve_slot_regs_for_heap_syscall(slot_map);
        // Stage operands through non-allocatable rax and the machine stack. Loading
        // directly into r10/r11 is incorrect when the allocator assigns the two
        // source slots to each other's scratch register.
        self.emit_load_operand_to_rax(ptr, slot_map);
        self.emit_push_reg(0);
        self.emit_load_operand_to_rax(index, slot_map);
        self.emit_push_reg(0);
        self.emit_pop_reg(11);
        self.emit_pop_reg(10);
        match bytes {
            1 => self
                .code
                .extend_from_slice(&[0x43, 0x0F, 0xB6, 0x04, 0x1A]), // movzx eax, byte [r10+r11]
            2 => self
                .code
                .extend_from_slice(&[0x43, 0x0F, 0xB7, 0x04, 0x5A]), // movzx eax, word [r10+r11*2]
            4 => self.code.extend_from_slice(&[0x43, 0x8B, 0x04, 0x9A]), // mov eax, [r10+r11*4]
            8 => self.code.extend_from_slice(&[0x4B, 0x8B, 0x04, 0xDA]), // mov rax, [r10+r11*8]
            _ => unreachable!("validated heap integer width"),
        }
        self.emit_restore_preserved_regs_reverse(&preserved);
        self.emit_store_rax_to_slot(dst, slot_map);
    }

    fn emit_runtime_heap_store_int(
        &mut self,
        ptr: &RuntimeOperand,
        index: &RuntimeOperand,
        src: &RuntimeOperand,
        bytes: u8,
        slot_map: &RuntimeSlotMap,
    ) {
        let preserved = self.emit_preserve_slot_regs_for_heap_syscall(slot_map);
        self.emit_load_operand_to_rax(ptr, slot_map);
        self.emit_push_reg(0);
        self.emit_load_operand_to_rax(index, slot_map);
        self.emit_push_reg(0);
        self.emit_load_operand_to_rax(src, slot_map);
        self.emit_pop_reg(11);
        self.emit_pop_reg(10);
        match bytes {
            1 => self.code.extend_from_slice(&[0x43, 0x88, 0x04, 0x1A]), // mov byte [r10+r11], al
            2 => self
                .code
                .extend_from_slice(&[0x66, 0x43, 0x89, 0x04, 0x5A]), // mov word [r10+r11*2], ax
            4 => self.code.extend_from_slice(&[0x43, 0x89, 0x04, 0x9A]), // mov dword [r10+r11*4], eax
            8 => self.code.extend_from_slice(&[0x4B, 0x89, 0x04, 0xDA]), // mov qword [r10+r11*8], rax
            _ => unreachable!("validated heap integer width"),
        }
        self.emit_restore_preserved_regs_reverse(&preserved);
    }

    fn emit_runtime_heap_copy(
        &mut self,
        dst_ptr: &RuntimeOperand,
        src_ptr: &RuntimeOperand,
        bytes: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        let preserved = self.emit_preserve_slot_regs_for_heap_syscall(slot_map);
        self.emit_load_operand_to_rax(dst_ptr, slot_map);
        self.emit_push_reg(0);
        self.emit_load_operand_to_rax(src_ptr, slot_map);
        self.emit_push_reg(0);
        self.emit_load_operand_to_rax(bytes, slot_map);
        self.emit_push_reg(0);
        self.emit_pop_reg(1);
        self.emit_pop_reg(6);
        self.emit_pop_reg(7);
        self.code.extend_from_slice(&[0xF3, 0xA4]); // rep movsb
        self.emit_restore_preserved_regs_reverse(&preserved);
    }

    fn emit_runtime_free(
        &mut self,
        ptr: &RuntimeOperand,
        size: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        let preserved = self.emit_preserve_slot_regs_for_heap_syscall(slot_map);
        self.emit_load_operand_to_rax(ptr, slot_map);
        self.emit_push_reg(0);
        self.emit_load_operand_to_rax(size, slot_map);
        self.emit_mov_reg_reg(6, 0); // rsi = size
        self.emit_pop_reg(7); // rdi = ptr
        self.code.push(0xE8); // call shared release path
        let call_disp = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.runtime_allocator
            .as_mut()
            .expect("runtime release requires allocator frame")
            .free_call_patches
            .push(call_disp);

        self.emit_restore_preserved_regs_reverse(&preserved);
    }

    fn emit_runtime_allocator_teardown_call(&mut self) {
        self.code.push(0xE8);
        let call_disp = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.runtime_allocator
            .as_mut()
            .expect("allocator teardown requires allocator frame")
            .teardown_call_patches
            .push(call_disp);
    }

    fn emit_runtime_allocator_routines(&mut self) {
        const SLAB_BYTES: u64 = 64 * 1024;
        const ALLOCATION_MAGIC: u64 = 0x415A_4B59_4F57_4E44;

        let emission = self
            .runtime_allocator
            .as_ref()
            .expect("allocator routines require emission state")
            .clone();

        self.code.push(0xE9); // keep end-of-program fallthrough out of helper code
        let skip_helpers = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        let alloc_entry = self.code.len();
        self.emit_mov_reg_reg(8, 6); // r8 = requested payload bytes
        self.emit_alu_reg_imm(7, 8, 0_i32.to_le_bytes()); // cmp r8, 0
        self.code.extend_from_slice(&[0x0F, 0x85]); // jne normalized
        let nonzero = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.emit_mov_reg_imm(GpReg::R8, 1);
        let normalize = self.code.len();
        patch_rel32(&mut self.code, nonzero, normalize);
        self.emit_alu_reg_imm(0, 8, 15_i32.to_le_bytes()); // add r8, 15
        self.code.extend_from_slice(&[0x0F, 0x82]); // jc allocation failure
        let rounded_overflow = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.emit_alu_reg_imm(4, 8, (-16_i32).to_le_bytes()); // and r8, -16
        self.emit_mov_reg_reg(9, 8); // r9 = rounded payload
        self.emit_alu_reg_imm(0, 9, 16_i32.to_le_bytes()); // add allocation header
        self.code.extend_from_slice(&[0x0F, 0x82]); // jc allocation failure
        let total_overflow = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        let retry = self.code.len();
        self.emit_load_rbp_disp_to_reg(0, emission.frame.cursor_disp);
        self.code.extend_from_slice(&[0x48, 0x85, 0xC0]); // test rax, rax
        self.code.extend_from_slice(&[0x0F, 0x84]); // jz new slab
        let no_current_slab = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.emit_mov_reg_reg(2, 0); // rdx = current block header
        self.emit_add_reg_reg(2, 9); // rdx = candidate cursor
        self.emit_cmp_reg_rbp_disp(2, emission.frame.end_disp);
        self.code.extend_from_slice(&[0x0F, 0x86]); // jbe allocation success
        let fits_current_slab = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        let new_slab = self.code.len();
        patch_rel32(&mut self.code, no_current_slab, new_slab);
        self.emit_mov_reg_reg(2, 9); // rdx = block bytes
        self.emit_alu_reg_imm(0, 2, 16_i32.to_le_bytes()); // plus slab header
        self.code.extend_from_slice(&[0x0F, 0x82]); // jc allocation failure
        let slab_overflow = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.emit_alu_reg_imm(7, 2, (SLAB_BYTES as i32).to_le_bytes());
        self.code.extend_from_slice(&[0x0F, 0x83]); // jae map-size ready
        let large_enough = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.emit_mov_rdx_imm(SLAB_BYTES);
        let map_size_ready = self.code.len();
        patch_rel32(&mut self.code, large_enough, map_size_ready);
        self.emit_load_rbp_disp_to_reg(10, emission.frame.head_disp);
        self.emit_push_reg(8); // rounded payload
        self.emit_push_reg(9); // total block bytes
        self.emit_push_reg(10); // previous slab
        self.emit_push_reg(2); // mapping bytes
        self.emit_mov_reg_reg(6, 2); // rsi = mapping bytes
        self.emit_xor_reg_reg(7, 7); // rdi = NULL
        self.emit_mov_rdx_imm(self.runtime.prot_read_write);
        self.code.extend_from_slice(&[0x41, 0xBA]);
        self.code
            .extend_from_slice(&(self.runtime.mmap_private_anonymous as u32).to_le_bytes());
        self.emit_mov_reg_imm(GpReg::R8, u64::MAX); // fd = -1
        self.code.extend_from_slice(&[0x45, 0x31, 0xC9]); // offset = 0
        self.emit_mov_rax_imm(self.runtime.syscalls.mmap); // SYS_mmap
        self.emit_kernel_call();
        self.emit_pop_reg(2);
        self.emit_pop_reg(10);
        self.emit_pop_reg(9);
        self.emit_pop_reg(8);
        self.code.extend_from_slice(&[0x48, 0x85, 0xC0]); // test rax, rax
        self.code.extend_from_slice(&[0x0F, 0x88]); // js allocation failure
        let mmap_failed = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.code.extend_from_slice(&[0x4C, 0x89, 0x10]); // [rax] = previous slab
        self.code.extend_from_slice(&[0x48, 0x89, 0x50, 0x08]); // [rax+8] = map bytes
        self.emit_store_reg_to_rbp_disp(emission.frame.head_disp, 0);
        self.emit_mov_reg_reg(10, 0);
        self.emit_alu_reg_imm(0, 10, 16_i32.to_le_bytes());
        self.emit_store_reg_to_rbp_disp(emission.frame.cursor_disp, 10);
        self.emit_add_reg_reg(2, 0);
        self.emit_store_reg_to_rbp_disp(emission.frame.end_disp, 2);
        self.code.push(0xE9);
        let retry_jump = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        patch_rel32(&mut self.code, retry_jump, retry);

        let allocation_success = self.code.len();
        patch_rel32(&mut self.code, fits_current_slab, allocation_success);
        self.emit_store_reg_to_rbp_disp(emission.frame.cursor_disp, 2);
        self.code.extend_from_slice(&[0x4C, 0x89, 0x00]); // [rax] = rounded payload
        self.emit_mov_reg_imm(GpReg::R11, ALLOCATION_MAGIC);
        self.code.extend_from_slice(&[0x4C, 0x89, 0x58, 0x08]); // [rax+8] = magic
        self.emit_alu_reg_imm(0, 0, 16_i32.to_le_bytes()); // return payload pointer
        self.code.push(0xC3);

        let allocation_failure = self.code.len();
        for patch in [rounded_overflow, total_overflow, slab_overflow, mmap_failed] {
            patch_rel32(&mut self.code, patch, allocation_failure);
        }
        self.emit_xor_reg_reg(0, 0);
        self.code.push(0xC3);

        let free_entry = self.code.len();
        self.code.extend_from_slice(&[0x48, 0x85, 0xFF]); // test rdi, rdi
        self.code.extend_from_slice(&[0x0F, 0x84]); // jz free done
        let free_null = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.code.extend_from_slice(&[0x4C, 0x8D, 0x57, 0xF0]); // r10 = ptr - 16
        self.code.extend_from_slice(&[0x49, 0x8B, 0x42, 0x08]); // rax = magic
        self.emit_mov_reg_imm(GpReg::R11, ALLOCATION_MAGIC);
        self.emit_cmp_reg_reg(0, 11);
        self.code.extend_from_slice(&[0x0F, 0x85]); // jne free done
        let free_invalid = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.code.extend_from_slice(&[0x49, 0xC7, 0x42, 0x08]); // clear magic
        self.code.extend_from_slice(&0_u32.to_le_bytes());
        self.emit_load_rbp_disp_to_reg(0, emission.frame.cursor_disp);
        self.emit_mov_reg_reg(2, 7); // rdx = payload pointer
        self.code.extend_from_slice(&[0x49, 0x03, 0x12]); // rdx += rounded payload
        self.emit_cmp_reg_reg(2, 0);
        self.code.extend_from_slice(&[0x0F, 0x85]); // non-LIFO release stays retired
        let free_not_last = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.emit_store_reg_to_rbp_disp(emission.frame.cursor_disp, 10);
        let free_done = self.code.len();
        for patch in [free_null, free_invalid, free_not_last] {
            patch_rel32(&mut self.code, patch, free_done);
        }
        self.code.push(0xC3);

        let teardown_entry = self.code.len();
        self.emit_load_rbp_disp_to_reg(10, emission.frame.head_disp);
        let teardown_loop = self.code.len();
        self.code.extend_from_slice(&[0x4D, 0x85, 0xD2]); // test r10, r10
        self.code.extend_from_slice(&[0x0F, 0x84]); // jz teardown done
        let no_more_slabs = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        self.code.extend_from_slice(&[0x4D, 0x8B, 0x02]); // r8 = previous slab
        self.code.extend_from_slice(&[0x49, 0x8B, 0x72, 0x08]); // rsi = map bytes
        self.emit_mov_reg_reg(7, 10); // rdi = slab
        self.emit_mov_rax_imm(self.runtime.syscalls.munmap); // SYS_munmap
        self.emit_kernel_call();
        self.emit_mov_reg_reg(10, 8);
        self.code.push(0xE9);
        let teardown_backedge = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        patch_rel32(&mut self.code, teardown_backedge, teardown_loop);
        let teardown_done = self.code.len();
        patch_rel32(&mut self.code, no_more_slabs, teardown_done);
        self.emit_store_reg_to_rbp_disp(emission.frame.head_disp, 10);
        self.emit_store_reg_to_rbp_disp(emission.frame.cursor_disp, 10);
        self.emit_store_reg_to_rbp_disp(emission.frame.end_disp, 10);
        self.code.push(0xC3);

        let after_helpers = self.code.len();
        patch_rel32(&mut self.code, skip_helpers, after_helpers);
        for patch in emission.alloc_call_patches {
            patch_rel32(&mut self.code, patch, alloc_entry);
        }
        for patch in emission.free_call_patches {
            patch_rel32(&mut self.code, patch, free_entry);
        }
        for patch in emission.teardown_call_patches {
            patch_rel32(&mut self.code, patch, teardown_entry);
        }
    }

    fn emit_runtime_print_const(&mut self, text: &str, slot_map: &RuntimeSlotMap) {
        let preserved = self.emit_preserve_slot_regs_for_runtime_print(slot_map);
        self.emit_write(text.as_bytes());
        self.emit_restore_preserved_regs_reverse(&preserved);
    }

    fn emit_runtime_print_int(
        &mut self,
        value: &RuntimeOperand,
        signed: bool,
        _bits: u16,
        slot_map: &RuntimeSlotMap,
    ) {
        // Clobbers rax, rbx, rcx, rdx, rsi, rdi, r8, r10.
        let preserved = self.emit_preserve_slot_regs_for_runtime_print(slot_map);
        self.emit_load_operand_to_rax(value, slot_map);

        // Reserve temporary conversion buffer.
        self.code.extend_from_slice(&[0x48, 0x83, 0xEC, 0x40]); // sub rsp, 64
        self.code.extend_from_slice(&[0x48, 0x8D, 0x74, 0x24, 0x40]); // lea rsi, [rsp+64]
        self.code.extend_from_slice(&[0x4D, 0x31, 0xC0]); // xor r8, r8 (len = 0)

        if signed {
            self.code.extend_from_slice(&[0x45, 0x31, 0xD2]); // xor r10d, r10d
            self.code.extend_from_slice(&[0x48, 0x85, 0xC0]); // test rax, rax
            self.code.extend_from_slice(&[0x0F, 0x89]); // jns rel32
            let jns_positive = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            self.code.extend_from_slice(&[0x41, 0xBA, 0x01, 0x00, 0x00, 0x00]); // mov r10d, 1
            let positive_label = self.code.len();
            patch_rel32(&mut self.code, jns_positive, positive_label);

            let loop_start = self.code.len();
            self.code.extend_from_slice(&[0x48, 0x99]); // cqo
            self.code.extend_from_slice(&[0xB9, 0x0A, 0x00, 0x00, 0x00]); // mov ecx, 10
            self.code.extend_from_slice(&[0x48, 0xF7, 0xF9]); // idiv rcx
            self.code.extend_from_slice(&[0x48, 0x89, 0xD3]); // mov rbx, rdx
            self.code.extend_from_slice(&[0x48, 0x85, 0xDB]); // test rbx, rbx
            self.code.extend_from_slice(&[0x0F, 0x8D]); // jge rel32
            let jge_non_negative = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            self.code.extend_from_slice(&[0x48, 0xF7, 0xDB]); // neg rbx
            let rem_non_negative = self.code.len();
            patch_rel32(&mut self.code, jge_non_negative, rem_non_negative);
            self.code.extend_from_slice(&[0x80, 0xC3, b'0']); // add bl, '0'
            self.code.extend_from_slice(&[0x48, 0xFF, 0xCE]); // dec rsi
            self.code.extend_from_slice(&[0x88, 0x1E]); // mov [rsi], bl
            self.code.extend_from_slice(&[0x49, 0xFF, 0xC0]); // inc r8
            self.code.extend_from_slice(&[0x48, 0x85, 0xC0]); // test rax, rax
            self.code.extend_from_slice(&[0x0F, 0x85]); // jne rel32
            let jne_loop = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            patch_rel32(&mut self.code, jne_loop, loop_start);

            self.code.extend_from_slice(&[0x45, 0x85, 0xD2]); // test r10d, r10d
            self.code.extend_from_slice(&[0x0F, 0x84]); // jz rel32
            let jz_write = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            self.code.extend_from_slice(&[0x48, 0xFF, 0xCE]); // dec rsi
            self.code.extend_from_slice(&[0xC6, 0x06, b'-']); // mov byte [rsi], '-'
            self.code.extend_from_slice(&[0x49, 0xFF, 0xC0]); // inc r8
            let write_label = self.code.len();
            patch_rel32(&mut self.code, jz_write, write_label);
        } else {
            let loop_start = self.code.len();
            self.code.extend_from_slice(&[0x31, 0xD2]); // xor edx, edx
            self.code.extend_from_slice(&[0xB9, 0x0A, 0x00, 0x00, 0x00]); // mov ecx, 10
            self.code.extend_from_slice(&[0x48, 0xF7, 0xF1]); // div rcx
            self.code.extend_from_slice(&[0x88, 0xD3]); // mov bl, dl
            self.code.extend_from_slice(&[0x80, 0xC3, b'0']); // add bl, '0'
            self.code.extend_from_slice(&[0x48, 0xFF, 0xCE]); // dec rsi
            self.code.extend_from_slice(&[0x88, 0x1E]); // mov [rsi], bl
            self.code.extend_from_slice(&[0x49, 0xFF, 0xC0]); // inc r8
            self.code.extend_from_slice(&[0x48, 0x85, 0xC0]); // test rax, rax
            self.code.extend_from_slice(&[0x0F, 0x85]); // jne rel32
            let jne_loop = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            patch_rel32(&mut self.code, jne_loop, loop_start);
        }

        self.emit_mov_rax_imm(self.runtime.syscalls.write);
        self.emit_mov_rdi_imm(1); // stdout
        self.code.extend_from_slice(&[0x4C, 0x89, 0xC2]); // mov rdx, r8
        self.emit_kernel_call(); // syscall
        self.code.extend_from_slice(&[0x48, 0x83, 0xC4, 0x40]); // add rsp, 64

        self.emit_restore_preserved_regs_reverse(&preserved);
    }

    fn emit_preserve_slot_regs_for_heap_syscall(&mut self, slot_map: &RuntimeSlotMap) -> Vec<u8> {
        let mut regs = Vec::new();
        for reg in slot_map.reg_by_slot.iter().flatten().copied() {
            if matches!(reg, 2 | 6 | 7 | 8 | 9 | 10 | 11) && !regs.contains(&reg) {
                regs.push(reg);
            }
        }
        regs.sort_unstable();
        for reg in regs.iter().copied() {
            self.emit_push_reg(reg);
        }
        regs
    }

    fn emit_preserve_slot_regs_for_runtime_print(&mut self, slot_map: &RuntimeSlotMap) -> Vec<u8> {
        let mut regs = Vec::new();
        for reg in slot_map.reg_by_slot.iter().flatten().copied() {
            if matches!(reg, 3 | 6 | 7 | 8 | 9 | 10 | 11) && !regs.contains(&reg) {
                regs.push(reg);
            }
        }
        regs.sort_unstable();
        for reg in regs.iter().copied() {
            self.emit_push_reg(reg);
        }
        regs
    }

    fn emit_restore_preserved_regs_reverse(&mut self, regs: &[u8]) {
        for reg in regs.iter().rev().copied() {
            self.emit_pop_reg(reg);
        }
    }

    fn emit_runtime_compare_swap(
        &mut self,
        left: usize,
        right: usize,
        signed: bool,
        slot_map: &RuntimeSlotMap,
    ) {
        if left == right {
            return;
        }
        if let (Some(left_reg), Some(right_reg)) = (slot_map.reg(left), slot_map.reg(right)) {
            self.emit_mov_reg_reg(2, left_reg); // rdx = a
            self.emit_cmp_reg_reg(left_reg, right_reg); // cmp a, b
            if signed {
                self.emit_cmovg_reg_reg(left_reg, right_reg); // a = min(a, b)
                self.emit_cmovg_reg_reg(right_reg, 2); // b = max(a, b)
            } else {
                self.emit_cmova_reg_reg(left_reg, right_reg); // a = min(a, b)
                self.emit_cmova_reg_reg(right_reg, 2); // b = max(a, b)
            }
            return;
        }
        self.emit_load_slot_to_rax(left, slot_map);
        self.emit_load_slot_to_rcx(right, slot_map);
        self.emit_mov_reg_reg(2, 0); // rdx = a
        self.emit_cmp_reg_reg(0, 1); // cmp a, b
        if signed {
            self.emit_cmovg_reg_reg(0, 1); // a = min(a, b)
            self.emit_cmovg_reg_reg(1, 2); // b = max(a, b)
        } else {
            self.emit_cmova_reg_reg(0, 1); // a = min(a, b)
            self.emit_cmova_reg_reg(1, 2); // b = max(a, b)
        }
        self.emit_store_rax_to_slot(left, slot_map);
        self.emit_store_rcx_to_slot(right, slot_map);
    }

    fn emit_runtime_radix_sort_fixed_int(
        &mut self,
        slots: &[usize],
        bits: u16,
        signed: bool,
        _stable: bool,
        slot_map: &RuntimeSlotMap,
    ) {
        if slots.len() <= 1 || !matches!(bits, 32 | 64) {
            return;
        }

        // Small-array hot path: dedicated fixed 64-lane branchless network without
        // per-call CPU dispatch.
        if slots.len() == 64 {
            self.emit_runtime_sort_network_power2_kernel(slots, signed, slot_map);
            return;
        }

        let allow_avx2 = self.options.target_features.avx2;
        let allow_avx512f = self.options.target_features.avx512f;
        if !allow_avx2 && !allow_avx512f {
            self.emit_runtime_radix_sort_fixed_int_kernel(slots, bits, signed, slot_map);
            return;
        }

        // Startup dispatch: AVX-512 path, AVX2 path, then scalar fallback.
        // CPUID clobbers rbx, so preserve it regardless of slot pinning.
        self.code.push(0x53); // push rbx
        self.emit_mov_rax_imm(self.runtime.syscalls.write);
        self.emit_xor_reg_reg(1, 1); // xor rcx, rcx
        self.code.extend_from_slice(&[0x0F, 0xA2]); // cpuid
        self.code.extend_from_slice(&[0xF7, 0xC1]); // test ecx, imm32
        self.code
            .extend_from_slice(&((1u32 << 27) | (1u32 << 28)).to_le_bytes()); // OSXSAVE | AVX
        self.code.extend_from_slice(&[0x0F, 0x84]); // jz scalar_dispatch
        let jz_scalar_no_avx_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        self.emit_xor_reg_reg(1, 1); // xor rcx, rcx
        self.code.extend_from_slice(&[0x0F, 0x01, 0xD0]); // xgetbv
        self.code.push(0x25); // and eax, imm32
        self.code.extend_from_slice(&0x0000_0006u32.to_le_bytes());
        self.code.push(0x3D); // cmp eax, imm32
        self.code.extend_from_slice(&0x0000_0006u32.to_le_bytes());
        self.code.extend_from_slice(&[0x0F, 0x85]); // jne scalar_dispatch
        let jne_scalar_xsave_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        self.emit_mov_rax_imm(7);
        self.emit_xor_reg_reg(1, 1); // xor rcx, rcx
        self.code.extend_from_slice(&[0x0F, 0xA2]); // cpuid leaf 7
        let mut jne_avx512_dispatch_pos = None;
        if allow_avx512f {
            self.code.extend_from_slice(&[0xF7, 0xC3]); // test ebx, imm32
            self.code.extend_from_slice(&(1u32 << 16).to_le_bytes()); // AVX-512F
            self.code.extend_from_slice(&[0x0F, 0x85]); // jne avx512_dispatch
            jne_avx512_dispatch_pos = Some(self.code.len());
            self.code.extend_from_slice(&0_i32.to_le_bytes());
        }

        let mut jne_avx2_dispatch_pos = None;
        if allow_avx2 {
            self.code.extend_from_slice(&[0xF7, 0xC3]); // test ebx, imm32
            self.code.extend_from_slice(&(1u32 << 5).to_le_bytes()); // AVX2
            self.code.extend_from_slice(&[0x0F, 0x85]); // jne avx2_dispatch
            jne_avx2_dispatch_pos = Some(self.code.len());
            self.code.extend_from_slice(&0_i32.to_le_bytes());
        }

        let scalar_dispatch = self.code.len();
        self.code.push(0x5B); // pop rbx
        self.code.push(0xE9); // jmp scalar_kernel
        let jmp_scalar_kernel_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        let avx2_dispatch = self.code.len();
        self.code.push(0x5B); // pop rbx
        self.code.push(0xE9); // jmp avx2_kernel
        let jmp_avx2_kernel_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        let avx512_dispatch = self.code.len();
        self.code.push(0x5B); // pop rbx
        self.code.push(0xE9); // jmp avx512_kernel
        let jmp_avx512_kernel_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        patch_rel32(&mut self.code, jz_scalar_no_avx_pos, scalar_dispatch);
        patch_rel32(&mut self.code, jne_scalar_xsave_pos, scalar_dispatch);
        if let Some(jne_avx512_dispatch_pos) = jne_avx512_dispatch_pos {
            patch_rel32(&mut self.code, jne_avx512_dispatch_pos, avx512_dispatch);
        }
        if let Some(jne_avx2_dispatch_pos) = jne_avx2_dispatch_pos {
            patch_rel32(&mut self.code, jne_avx2_dispatch_pos, avx2_dispatch);
        }

        let scalar_kernel = self.code.len();
        self.emit_runtime_radix_sort_fixed_int_kernel(slots, bits, signed, slot_map);
        self.code.push(0xE9); // jmp done
        let jmp_done_from_scalar_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        let avx2_kernel = self.code.len();
        self.emit_vzeroupper();
        self.emit_runtime_radix_sort_fixed_int_avx2_kernel(slots, bits, signed, slot_map);
        self.emit_vzeroupper();
        self.code.push(0xE9); // jmp done
        let jmp_done_from_avx2_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        let avx512_kernel = self.code.len();
        self.emit_vzeroupper();
        self.emit_runtime_radix_sort_fixed_int_kernel(slots, bits, signed, slot_map);
        self.emit_vzeroupper();
        let done = self.code.len();

        patch_rel32(&mut self.code, jmp_scalar_kernel_pos, scalar_kernel);
        patch_rel32(&mut self.code, jmp_avx2_kernel_pos, avx2_kernel);
        patch_rel32(&mut self.code, jmp_avx512_kernel_pos, avx512_kernel);
        patch_rel32(&mut self.code, jmp_done_from_scalar_pos, done);
        patch_rel32(&mut self.code, jmp_done_from_avx2_pos, done);
    }

    fn emit_runtime_sort_network_power2_kernel(
        &mut self,
        slots: &[usize],
        signed: bool,
        slot_map: &RuntimeSlotMap,
    ) {
        debug_assert!(slots.len().is_power_of_two());
        let pairs = Self::runtime_oddeven_merge_power2_pairs(slots.len());
        for (left, right) in pairs {
            self.emit_runtime_compare_swap(slots[left], slots[right], signed, slot_map);
        }
    }

    fn runtime_oddeven_merge_power2_pairs(len: usize) -> Vec<(usize, usize)> {
        debug_assert!(len.is_power_of_two());
        let mut pairs = Vec::with_capacity(len * len / 2);
        Self::runtime_oddeven_merge_sort_rec(0, len, &mut pairs);
        pairs
    }

    fn runtime_oddeven_merge_sort_rec(lo: usize, n: usize, pairs: &mut Vec<(usize, usize)>) {
        if n <= 1 {
            return;
        }
        let m = n / 2;
        Self::runtime_oddeven_merge_sort_rec(lo, m, pairs);
        Self::runtime_oddeven_merge_sort_rec(lo + m, m, pairs);
        Self::runtime_oddeven_merge_rec(lo, n, 1, pairs);
    }

    fn runtime_oddeven_merge_rec(
        lo: usize,
        n: usize,
        r: usize,
        pairs: &mut Vec<(usize, usize)>,
    ) {
        let step = r * 2;
        if step < n {
            Self::runtime_oddeven_merge_rec(lo, n, step, pairs);
            Self::runtime_oddeven_merge_rec(lo + r, n, step, pairs);
            let mut i = lo + r;
            while i + r < lo + n {
                pairs.push((i, i + r));
                i += step;
            }
        } else {
            pairs.push((lo, lo + r));
        }
    }

    fn emit_runtime_radix_sort_fixed_int_avx2_kernel(
        &mut self,
        slots: &[usize],
        bits: u16,
        signed: bool,
        slot_map: &RuntimeSlotMap,
    ) {
        let elem_count = slots.len();
        let passes = (bits / 8) as usize;
        let src_off = 0usize;
        let dst_off = elem_count * 8;
        // 4 parallel histograms for ILP
        let h_size = 256 * 4;
        let h0_off = dst_off + elem_count * 8;
        let h1_off = h0_off + h_size;
        let h2_off = h1_off + h_size;
        let h3_off = h2_off + h_size;
        let prefix_off = h3_off + h_size;
        let total_bytes = ((prefix_off + 256 * 4 + 15) / 16) * 16;
        if total_bytes > i32::MAX as usize {
            self.emit_runtime_radix_sort_fixed_int_kernel(slots, bits, signed, slot_map);
            return;
        }
        let total_bytes_i32 = total_bytes as i32;
        self.emit_sub_rsp_imm32(total_bytes_i32);

        for (idx, slot) in slots.iter().enumerate() {
            self.emit_load_slot_to_rax(*slot, slot_map);
            self.emit_mov_rsp_disp_from_rax((src_off + idx * 8) as i32);
        }

        for pass in 0..passes {
            let shift = (pass * 8) as u8;
            let is_final_pass = pass + 1 == passes;
            let src_base = if pass % 2 == 0 { src_off } else { dst_off };
            let dst_base = if pass % 2 == 0 { dst_off } else { src_off };

            // Zero 4 histograms
            self.emit_xor_reg_reg(0, 0); // zero rax
            let zeros = (4 * h_size) / 8;
            for i in 0..zeros {
                self.emit_mov_rsp_disp_from_rax((h0_off + i * 8) as i32);
            }

            // Histogram pass with 4-way ILP
            let blocks = elem_count / 4;
            let tail = elem_count % 4;

            for i in 0..blocks {
                let base = src_base + i * 4 * 8;
                // Process 4 elements in parallel
                self.emit_mov_rdx_from_rsp_disp(base as i32);
                self.emit_runtime_radix_prepare_bucket(bits, signed, is_final_pass, shift);
                self.emit_inc_dword_rsp_rax4_disp(h0_off as i32);

                self.emit_mov_rdx_from_rsp_disp((base + 8) as i32);
                self.emit_runtime_radix_prepare_bucket(bits, signed, is_final_pass, shift);
                self.emit_inc_dword_rsp_rax4_disp(h1_off as i32);

                self.emit_mov_rdx_from_rsp_disp((base + 16) as i32);
                self.emit_runtime_radix_prepare_bucket(bits, signed, is_final_pass, shift);
                self.emit_inc_dword_rsp_rax4_disp(h2_off as i32);

                self.emit_mov_rdx_from_rsp_disp((base + 24) as i32);
                self.emit_runtime_radix_prepare_bucket(bits, signed, is_final_pass, shift);
                self.emit_inc_dword_rsp_rax4_disp(h3_off as i32);
            }
            for i in 0..tail {
                self.emit_mov_rdx_from_rsp_disp((src_base + (blocks * 4 + i) * 8) as i32);
                self.emit_runtime_radix_prepare_bucket(bits, signed, is_final_pass, shift);
                self.emit_inc_dword_rsp_rax4_disp(h0_off as i32);
            }

            // Combine histograms and build prefix sum
            self.emit_xor_reg_reg(1, 1); // ecx = current prefix
            for bucket in 0..256usize {
                self.emit_mov_eax_from_rsp_disp((h0_off + bucket * 4) as i32);
                self.emit_add_eax_from_rsp_disp((h1_off + bucket * 4) as i32);
                self.emit_add_eax_from_rsp_disp((h2_off + bucket * 4) as i32);
                self.emit_add_eax_from_rsp_disp((h3_off + bucket * 4) as i32);
                
                self.emit_mov_dword_rsp_disp_from_ecx((prefix_off + bucket * 4) as i32);
                self.emit_add_ecx_eax();
            }

            // Shuffle pass (remains scalar but benefits from L1 hits)
            for idx in 0..elem_count {
                self.emit_mov_rdx_from_rsp_disp((src_base + idx * 8) as i32);
                self.emit_runtime_radix_prepare_bucket(bits, signed, is_final_pass, shift);
                self.emit_mov_ecx_from_rsp_rax4_disp(prefix_off as i32);
                self.emit_mov_rsp_rcx8_disp_from_rdx(dst_base as i32);
                self.emit_inc_dword_rsp_rax4_disp(prefix_off as i32);
            }
        }

        let final_base = if passes % 2 == 0 { src_off } else { dst_off };
        for (idx, slot) in slots.iter().enumerate() {
            self.emit_mov_rax_from_rsp_disp((final_base + idx * 8) as i32);
            self.emit_store_rax_to_slot(*slot, slot_map);
        }

        self.emit_add_rsp_imm32(total_bytes_i32);
    }

    fn emit_runtime_radix_sort_fixed_int_kernel(
        &mut self,
        slots: &[usize],
        bits: u16,
        signed: bool,
        slot_map: &RuntimeSlotMap,
    ) {
        let elem_count = slots.len();
        let passes = (bits / 8) as usize;
        let src_off = 0usize;
        let dst_off = elem_count * 8;
        let counts_off = dst_off + elem_count * 8;
        let prefix_off = counts_off + 256 * 4;
        let total_bytes = ((prefix_off + 256 * 4 + 15) / 16) * 16;
        if total_bytes > i32::MAX as usize {
            return;
        }
        let total_bytes_i32 = total_bytes as i32;
        self.emit_sub_rsp_imm32(total_bytes_i32);

        for (idx, slot) in slots.iter().enumerate() {
            self.emit_load_slot_to_rax(*slot, slot_map);
            self.emit_mov_rsp_disp_from_rax((src_off + idx * 8) as i32);
        }

        for pass in 0..passes {
            let shift = (pass * 8) as u8;
            let is_final_pass = pass + 1 == passes;
            let src_base = if pass % 2 == 0 { src_off } else { dst_off };
            let dst_base = if pass % 2 == 0 { dst_off } else { src_off };

            for bucket in 0..256usize {
                self.emit_mov_dword_rsp_disp_imm32((counts_off + bucket * 4) as i32, 0);
            }

            for idx in 0..elem_count {
                self.emit_mov_rdx_from_rsp_disp((src_base + idx * 8) as i32);
                self.emit_runtime_radix_prepare_bucket(bits, signed, is_final_pass, shift);
                self.emit_inc_dword_rsp_rax4_disp(counts_off as i32);
            }

            self.emit_xor_reg_reg(1, 1); // xor rcx, rcx (running prefix)
            for bucket in 0..256usize {
                self.emit_mov_eax_from_rsp_disp((counts_off + bucket * 4) as i32);
                self.emit_mov_dword_rsp_disp_from_ecx((prefix_off + bucket * 4) as i32);
                self.emit_add_ecx_eax();
            }

            for idx in 0..elem_count {
                self.emit_mov_rdx_from_rsp_disp((src_base + idx * 8) as i32);
                self.emit_runtime_radix_prepare_bucket(bits, signed, is_final_pass, shift);
                self.emit_mov_ecx_from_rsp_rax4_disp(prefix_off as i32);
                self.emit_mov_rsp_rcx8_disp_from_rdx(dst_base as i32);
                self.emit_inc_dword_rsp_rax4_disp(prefix_off as i32);
            }
        }

        let final_base = if passes % 2 == 0 { src_off } else { dst_off };
        for (idx, slot) in slots.iter().enumerate() {
            self.emit_mov_rax_from_rsp_disp((final_base + idx * 8) as i32);
            self.emit_store_rax_to_slot(*slot, slot_map);
        }

        self.emit_add_rsp_imm32(total_bytes_i32);
    }

    fn emit_runtime_radix_prepare_bucket(
        &mut self,
        bits: u16,
        signed: bool,
        is_final_pass: bool,
        shift: u8,
    ) {
        match bits {
            32 => {
                self.emit_mov_reg32_reg32(0, 2); // eax = edx
                if signed && is_final_pass {
                    self.emit_alu_reg_imm(6, 0, 0x8000_0000u32.to_le_bytes());
                }
            }
            64 => {
                self.emit_mov_reg_reg(0, 2); // rax = rdx
                if signed && is_final_pass {
                    self.emit_mov_rcx_imm(0x8000_0000_0000_0000u64);
                    self.code.extend_from_slice(&[0x48, 0x31, 0xC8]); // xor rax, rcx
                }
            }
            _ => return,
        }
        if shift != 0 {
            self.emit_shr_reg_imm8(0, shift);
        }
        self.emit_and_reg_imm32(0, 0xFF);
    }

    fn emit_vzeroupper(&mut self) {
        self.code.extend_from_slice(&[0xC5, 0xF8, 0x77]); // vzeroupper
    }

    fn emit_runtime_lcg_body(
        &mut self,
        iterations: u64,
        mul: u64,
        add: u64,
        exit_with_state: bool,
        exit_mask: Option<u64>,
    ) {
        let demanded_width = exit_mask
            .map(|mask| (u64::BITS - mask.leading_zeros()).max(1) as u8)
            .unwrap_or(64);
        self.emit_runtime_lcg_compute(iterations, mul, add, demanded_width);
        self.emit_exit_with_rax_or_mask(exit_with_state, exit_mask);
    }

    fn affine_composition_chunk(iterations: u64) -> u64 {
        // A target-independent, deterministic policy. Composition is capped at
        // 128 source iterations and retains at least 1024 dynamic loop blocks;
        // it may accelerate a recurrence but never folds a measured loop away.
        if iterations >= 131_072 {
            128
        } else if iterations >= 32_768 {
            64
        } else if iterations >= 8_192 {
            32
        } else {
            1
        }
    }

    fn emit_runtime_lcg_compute(
        &mut self,
        iterations: u64,
        mul: u64,
        add: u64,
        demanded_width: u8,
    ) {
        if iterations == 0 {
            return;
        }

        let chunk = Self::affine_composition_chunk(iterations);
        // Compose `chunk` recurrence steps exactly in wrapping arithmetic. The
        // emitted hot loop remains one multiply and one add regardless of the
        // composition width.
        let (chunk_mul, chunk_add) = Self::affine_pow_u64(mul, add, chunk);
        let main_blocks = iterations / chunk;
        let tail = iterations % chunk;
        let use_u32 = demanded_width <= 32;

        if use_u32 {
            self.code.push(0xBA); // mov edx, imm32
            self.code
                .extend_from_slice(&(chunk_mul as u32).to_le_bytes());
            self.code.push(0xBE); // mov esi, imm32
            self.code
                .extend_from_slice(&(chunk_add as u32).to_le_bytes());
        } else {
            self.emit_mov_reg_imm64(2, chunk_mul);
            self.emit_mov_reg_imm64(6, chunk_add);
        }

        if main_blocks > 0 {
            self.emit_mov_rcx_imm(main_blocks);

            // Align loop start to 16-byte boundary for µop cache efficiency
            let loop_start = self.code.len();
            let align_offset = (16 - (loop_start % 16)) % 16;
            for _ in 0..align_offset {
                self.code.push(0x90); // nop
            }
            let loop_start = self.code.len();

            // Hot loop: imul rax, rdx; add rax, rsi — 7 bytes, no REX prefix
            if use_u32 {
                self.code.extend_from_slice(&[0x0F, 0xAF, 0xC2]); // imul eax, edx
                self.code.extend_from_slice(&[0x01, 0xF0]); // add eax, esi
            } else {
                self.code.extend_from_slice(&[0x48, 0x0F, 0xAF, 0xC2]); // imul rax, rdx
                self.code.extend_from_slice(&[0x48, 0x01, 0xF0]); // add rax, rsi
            }

            // dec rcx + jnz rel8 (loop body is <128 bytes, rel8 fits)
            self.code.extend_from_slice(&[0x48, 0xFF, 0xC9]); // dec rcx (3 bytes)
            self.code.push(0x75); // jne rel8 (2 bytes = 5 total for dec+jne)
            let jne_pos = self.code.len();
            self.code.push(0); // placeholder rel8
            let loop_end = self.code.len();
            let rel = (loop_start as isize - loop_end as isize) as i8;
            self.code[jne_pos] = rel as u8;
        }

        // Tail: handle remaining iterations with scalar steps.
        // Reload 1x composite into rdx, rsi (same registers as main loop)
        if tail > 0 {
            if use_u32 {
                self.code.push(0xBA); // mov edx, imm32
                self.code.extend_from_slice(&(mul as u32).to_le_bytes());
                self.code.push(0xBE); // mov esi, imm32
                self.code.extend_from_slice(&(add as u32).to_le_bytes());
            } else {
                self.emit_mov_reg_imm64(2, mul); // mov rdx, mul
                self.emit_mov_reg_imm64(6, add); // mov rsi, add
            }
            for _ in 0..tail {
                if use_u32 {
                    self.code.extend_from_slice(&[0x0F, 0xAF, 0xC2]);
                    self.code.extend_from_slice(&[0x01, 0xF0]);
                } else {
                    self.code.extend_from_slice(&[0x48, 0x0F, 0xAF, 0xC2]);
                    self.code.extend_from_slice(&[0x48, 0x01, 0xF0]);
                }
            }
        }
    }

    fn emit_mov_reg_imm64(&mut self, reg: u8, value: u64) {
        let mut rex = 0x48u8; // REX.W
        if reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0xB8 | (reg & 0x7));
        self.code.extend_from_slice(&value.to_le_bytes());
    }

    fn emit_mov_rax_imm(&mut self, value: u64) {
        if u32::try_from(value).is_ok() {
            self.code.push(0xB8);
            self.code.extend_from_slice(&(value as u32).to_le_bytes());
        } else {
            self.code.extend_from_slice(&[0x48, 0xB8]);
            self.code.extend_from_slice(&value.to_le_bytes());
        }
    }

    fn emit_push_reg(&mut self, reg: u8) {
        if reg >= 8 {
            self.code.push(0x41);
        }
        self.code.push(0x50 | (reg & 0x7));
    }

    fn emit_pop_reg(&mut self, reg: u8) {
        if reg >= 8 {
            self.code.push(0x41);
        }
        self.code.push(0x58 | (reg & 0x7));
    }

    fn emit_mov_rdi_imm(&mut self, value: u64) {
        if u32::try_from(value).is_ok() {
            self.code.push(0xBF);
            self.code.extend_from_slice(&(value as u32).to_le_bytes());
        } else {
            self.code.extend_from_slice(&[0x48, 0xBF]);
            self.code.extend_from_slice(&value.to_le_bytes());
        }
    }

    fn emit_mov_rdx_imm(&mut self, value: u64) {
        if u32::try_from(value).is_ok() {
            self.code.push(0xBA);
            self.code.extend_from_slice(&(value as u32).to_le_bytes());
        } else {
            self.code.extend_from_slice(&[0x48, 0xBA]);
            self.code.extend_from_slice(&value.to_le_bytes());
        }
    }

    fn emit_mov_rsi_imm(&mut self, value: u64) {
        if u32::try_from(value).is_ok() {
            self.code.push(0xBE);
            self.code.extend_from_slice(&(value as u32).to_le_bytes());
        } else {
            self.code.extend_from_slice(&[0x48, 0xBE]);
            self.code.extend_from_slice(&value.to_le_bytes());
        }
    }

    fn emit_mov_rcx_imm(&mut self, value: u64) {
        if u32::try_from(value).is_ok() {
            self.code.push(0xB9);
            self.code.extend_from_slice(&(value as u32).to_le_bytes());
        } else {
            self.code.extend_from_slice(&[0x48, 0xB9]);
            self.code.extend_from_slice(&value.to_le_bytes());
        }
    }

    fn emit_load_operand_to_rax(&mut self, operand: &RuntimeOperand, slot_map: &RuntimeSlotMap) {
        match operand {
            RuntimeOperand::Imm(value) => self.emit_mov_rax_imm(*value),
            RuntimeOperand::Slot(slot) => self.emit_load_slot_to_rax(*slot, slot_map),
        }
    }

    fn emit_load_operand_to_rcx(&mut self, operand: &RuntimeOperand, slot_map: &RuntimeSlotMap) {
        match operand {
            RuntimeOperand::Imm(value) => self.emit_mov_rcx_imm(*value),
            RuntimeOperand::Slot(slot) => self.emit_load_slot_to_rcx(*slot, slot_map),
        }
    }

    fn emit_load_operand_to_xmm0(
        &mut self,
        operand: &RuntimeOperand,
        _bits: u16,
        slot_map: &RuntimeSlotMap,
    ) {
        self.emit_load_operand_to_rax(operand, slot_map);
        // movq xmm0, rax
        self.code.extend_from_slice(&[0x66, 0x48, 0x0F, 0x6E, 0xC0]);
    }

    fn emit_load_operand_to_xmm1(
        &mut self,
        operand: &RuntimeOperand,
        _bits: u16,
        slot_map: &RuntimeSlotMap,
    ) {
        self.emit_load_operand_to_rcx(operand, slot_map);
        // movq xmm1, rcx
        self.code.extend_from_slice(&[0x66, 0x48, 0x0F, 0x6E, 0xC9]);
    }

    fn emit_store_xmm0_to_slot(&mut self, slot: usize, bits: u16, slot_map: &RuntimeSlotMap) {
        // movq rax, xmm0
        self.code.extend_from_slice(&[0x66, 0x48, 0x0F, 0x7E, 0xC0]);
        if bits == 32 {
            // zero-extend low 32 bits for canonical f32 slot encoding.
            self.emit_mov_reg32_reg32(0, 0);
        }
        self.emit_store_rax_to_slot(slot, slot_map);
    }

    fn emit_load_slot_to_rax(&mut self, slot: usize, slot_map: &RuntimeSlotMap) {
        if let Some(reg) = slot_map.reg(slot) {
            self.emit_mov_reg_reg(0, reg); // rax <- reg
            return;
        }
        let disp = slot_map
            .stack_disp(slot)
            .expect("missing stack displacement for non-pinned runtime slot");
        if slot_map.element_width(slot) == 1 {
            self.emit_load_stack_byte_to_reg32(0, disp);
            return;
        }
        if i8::try_from(disp).is_ok() {
            self.code
                .extend_from_slice(&[0x48, 0x8B, 0x45, disp as i8 as u8]); // mov rax, [rbp+disp8]
        } else {
            self.code.extend_from_slice(&[0x48, 0x8B, 0x85]); // mov rax, [rbp+disp32]
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_load_stack_byte_to_reg32(&mut self, reg: u8, disp: i32) {
        let mut rex = 0x40u8;
        if reg >= 8 {
            rex |= 0x04; // REX.R
            self.code.push(rex);
        }
        self.code.extend_from_slice(&[0x0F, 0xB6]); // movzx r32, r/m8
        if i8::try_from(disp).is_ok() {
            self.code.push(0x45 | ((reg & 0x7) << 3));
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x85 | ((reg & 0x7) << 3));
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_store_reg8_to_stack(&mut self, reg: u8, disp: i32) {
        let mut rex = 0x40u8;
        if reg >= 8 {
            rex |= 0x04; // REX.R
        }
        if reg >= 4 {
            self.code.push(rex); // select SPL/BPL/SIL/DIL or r8b+
        }
        self.code.push(0x88); // mov r/m8, r8
        if i8::try_from(disp).is_ok() {
            self.code.push(0x45 | ((reg & 0x7) << 3));
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x85 | ((reg & 0x7) << 3));
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_load_slot_to_rcx(&mut self, slot: usize, slot_map: &RuntimeSlotMap) {
        if let Some(reg) = slot_map.reg(slot) {
            self.emit_mov_reg_reg(1, reg); // rcx <- reg
            return;
        }
        let disp = slot_map
            .stack_disp(slot)
            .expect("missing stack displacement for non-pinned runtime slot");
        if slot_map.element_width(slot) == 1 {
            self.emit_load_stack_byte_to_reg32(1, disp);
            return;
        }
        if i8::try_from(disp).is_ok() {
            self.code
                .extend_from_slice(&[0x48, 0x8B, 0x4D, disp as i8 as u8]); // mov rcx, [rbp+disp8]
        } else {
            self.code.extend_from_slice(&[0x48, 0x8B, 0x8D]); // mov rcx, [rbp+disp32]
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_store_rax_to_slot(&mut self, slot: usize, slot_map: &RuntimeSlotMap) {
        if let Some(reg) = slot_map.reg(slot) {
            self.emit_mov_reg_reg(reg, 0); // reg <- rax
            return;
        }
        let disp = slot_map
            .stack_disp(slot)
            .expect("missing stack displacement for non-pinned runtime slot");
        if slot_map.element_width(slot) == 1 {
            self.emit_store_reg8_to_stack(0, disp);
            return;
        }
        if i8::try_from(disp).is_ok() {
            self.code
                .extend_from_slice(&[0x48, 0x89, 0x45, disp as i8 as u8]); // mov [rbp+disp8], rax
        } else {
            self.code.extend_from_slice(&[0x48, 0x89, 0x85]); // mov [rbp+disp32], rax
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_store_rcx_to_slot(&mut self, slot: usize, slot_map: &RuntimeSlotMap) {
        if let Some(reg) = slot_map.reg(slot) {
            self.emit_mov_reg_reg(reg, 1); // reg <- rcx
            return;
        }
        let disp = slot_map
            .stack_disp(slot)
            .expect("missing stack displacement for non-pinned runtime slot");
        if slot_map.element_width(slot) == 1 {
            self.emit_store_reg8_to_stack(1, disp);
            return;
        }
        if i8::try_from(disp).is_ok() {
            self.code
                .extend_from_slice(&[0x48, 0x89, 0x4D, disp as i8 as u8]); // mov [rbp+disp8], rcx
        } else {
            self.code.extend_from_slice(&[0x48, 0x89, 0x8D]); // mov [rbp+disp32], rcx
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_mov_slot_operand(&mut self, dst_slot: usize, src: &RuntimeOperand, slot_map: &RuntimeSlotMap) {
        match src {
            RuntimeOperand::Imm(val) => self.emit_mov_slot_imm(dst_slot, *val, slot_map),
            RuntimeOperand::Slot(src_slot) => {
                if dst_slot == *src_slot { return; }
                self.emit_mov_slot_slot(dst_slot, *src_slot, slot_map);
            }
        }
    }

    fn emit_mov_slot_imm(&mut self, dst_slot: usize, val: u64, slot_map: &RuntimeSlotMap) {
        if let Some(reg) = slot_map.reg(dst_slot) {
            self.emit_mov_reg_imm64(reg, val);
            return;
        }
        self.emit_mov_rax_imm(val);
        self.emit_store_rax_to_slot(dst_slot, slot_map);
    }

    fn emit_mov_slot_slot(&mut self, dst_slot: usize, src_slot: usize, slot_map: &RuntimeSlotMap) {
        if let (Some(dst_reg), Some(src_reg)) = (slot_map.reg(dst_slot), slot_map.reg(src_slot)) {
            if dst_reg == src_reg {
                return;
            }
            self.emit_mov_reg_reg(dst_reg, src_reg);
            return;
        }
        if let Some(dst_reg) = slot_map.reg(dst_slot) {
            self.emit_load_slot_to_reg(dst_reg, src_slot, slot_map);
            return;
        }
        if let Some(src_reg) = slot_map.reg(src_slot) {
            self.emit_store_reg_to_slot(src_reg, dst_slot, slot_map);
            return;
        }
        self.emit_load_slot_to_rax(src_slot, slot_map);
        self.emit_store_rax_to_slot(dst_slot, slot_map);
    }

    fn emit_binop_slot_operand_u32(
        &mut self,
        dst_slot: usize,
        op: RuntimeBinOp,
        lhs: &RuntimeOperand,
        rhs: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) -> bool {
        if !matches!(
            op,
            RuntimeBinOp::Add
                | RuntimeBinOp::Sub
                | RuntimeBinOp::Mul
                | RuntimeBinOp::BitAnd
                | RuntimeBinOp::BitOr
                | RuntimeBinOp::BitXor
                | RuntimeBinOp::Shl
                | RuntimeBinOp::ShrUnsigned
        ) {
            return false;
        }
        if matches!(op, RuntimeBinOp::Shl | RuntimeBinOp::ShrUnsigned)
            && !matches!(rhs, RuntimeOperand::Imm(shift) if *shift < 32)
        {
            return false;
        }

        self.emit_load_operand_to_rax(lhs, slot_map);
        match rhs {
            RuntimeOperand::Imm(value) => match op {
                RuntimeBinOp::Add => {
                    self.code.push(0x05); // add eax, imm32
                    self.code.extend_from_slice(&(*value as u32).to_le_bytes());
                }
                RuntimeBinOp::Sub => {
                    self.code.push(0x2D); // sub eax, imm32
                    self.code.extend_from_slice(&(*value as u32).to_le_bytes());
                }
                RuntimeBinOp::Mul => {
                    self.code.extend_from_slice(&[0x69, 0xC0]); // imul eax, eax, imm32
                    self.code.extend_from_slice(&(*value as u32).to_le_bytes());
                }
                RuntimeBinOp::BitAnd => {
                    self.code.push(0x25); // and eax, imm32
                    self.code.extend_from_slice(&(*value as u32).to_le_bytes());
                }
                RuntimeBinOp::BitOr => {
                    self.code.push(0x0D); // or eax, imm32
                    self.code.extend_from_slice(&(*value as u32).to_le_bytes());
                }
                RuntimeBinOp::BitXor => {
                    self.code.push(0x35); // xor eax, imm32
                    self.code.extend_from_slice(&(*value as u32).to_le_bytes());
                }
                RuntimeBinOp::Shl => {
                    self.code.extend_from_slice(&[0xC1, 0xE0, *value as u8]); // shl eax, imm8
                }
                RuntimeBinOp::ShrUnsigned => {
                    self.code.extend_from_slice(&[0xC1, 0xE8, *value as u8]); // shr eax, imm8
                }
                _ => return false,
            },
            RuntimeOperand::Slot(_) => {
                self.emit_load_operand_to_rcx(rhs, slot_map);
                match op {
                    RuntimeBinOp::Add => self.code.extend_from_slice(&[0x01, 0xC8]),
                    RuntimeBinOp::Sub => self.code.extend_from_slice(&[0x29, 0xC8]),
                    RuntimeBinOp::Mul => self.code.extend_from_slice(&[0x0F, 0xAF, 0xC1]),
                    RuntimeBinOp::BitAnd => self.code.extend_from_slice(&[0x21, 0xC8]),
                    RuntimeBinOp::BitOr => self.code.extend_from_slice(&[0x09, 0xC8]),
                    RuntimeBinOp::BitXor => self.code.extend_from_slice(&[0x31, 0xC8]),
                    _ => return false,
                }
            }
        }
        self.emit_store_rax_to_slot(dst_slot, slot_map);
        true
    }

    fn emit_load_slot_to_reg(&mut self, reg: u8, slot: usize, slot_map: &RuntimeSlotMap) {
        if let Some(sreg) = slot_map.reg(slot) {
            self.emit_mov_reg_reg(reg, sreg);
            return;
        }
        let disp = slot_map.stack_disp(slot).expect("slot");
        if slot_map.element_width(slot) == 1 {
            self.emit_load_stack_byte_to_reg32(reg, disp);
            return;
        }
        let mut rex = 0x48u8;
        if reg >= 8 { rex |= 0x04; }
        self.code.push(rex);
        if i8::try_from(disp).is_ok() {
            self.code.extend_from_slice(&[0x8B, 0x45 | ((reg & 0x7) << 3), disp as i8 as u8]);
        } else {
            self.code.extend_from_slice(&[0x8B, 0x85 | ((reg & 0x7) << 3)]);
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_store_reg_to_slot(&mut self, reg: u8, slot: usize, slot_map: &RuntimeSlotMap) {
        if let Some(dreg) = slot_map.reg(slot) {
            self.emit_mov_reg_reg(dreg, reg);
            return;
        }
        let disp = slot_map.stack_disp(slot).expect("slot");
        if slot_map.element_width(slot) == 1 {
            self.emit_store_reg8_to_stack(reg, disp);
            return;
        }
        let mut rex = 0x48u8;
        if reg >= 8 { rex |= 0x04; } // REX.R for source reg in ModRM.reg
        self.code.push(rex);
        if i8::try_from(disp).is_ok() {
            self.code.extend_from_slice(&[0x89, 0x45 | ((reg & 0x7) << 3), disp as i8 as u8]);
        } else {
            self.code.extend_from_slice(&[0x89, 0x85 | ((reg & 0x7) << 3)]);
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_binop_slot_operand(
        &mut self,
        dst_slot: usize,
        op: RuntimeBinOp,
        lhs: &RuntimeOperand,
        rhs: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        if op == RuntimeBinOp::BitAnd
            && matches!(rhs, RuntimeOperand::Imm(value) if *value == u64::from(u32::MAX))
        {
            self.emit_load_operand_to_rax(lhs, slot_map);
            self.emit_mov_reg32_reg32(0, 0);
            self.emit_store_rax_to_slot(dst_slot, slot_map);
            return;
        }

        // Optimization: if dst == lhs, we can potentially avoid a MOV.
        let is_in_place = match lhs {
            RuntimeOperand::Slot(s) => *s == dst_slot,
            _ => false,
        };

        if is_in_place {
            if matches!(rhs, RuntimeOperand::Imm(value) if *value == u64::from(u32::MAX))
                && op == RuntimeBinOp::BitAnd
            {
                if let Some(reg) = slot_map.reg(dst_slot) {
                    self.emit_mov_reg32_reg32(reg, reg);
                } else {
                    self.emit_load_slot_to_rax(dst_slot, slot_map);
                    self.emit_mov_reg32_reg32(0, 0);
                    self.emit_store_rax_to_slot(dst_slot, slot_map);
                }
                return;
            }
            // Try to emit in-place directly.
            if let RuntimeOperand::Slot(rhs_slot) = rhs {
                if self.emit_binop_slot_slot_in_place(dst_slot, *rhs_slot, op, slot_map) {
                    return;
                }
            }
            if let RuntimeOperand::Imm(imm) = rhs {
                if let Some(imm32) = imm32_sign_extended(*imm) {
                    if self.emit_binop_slot_imm_in_place(dst_slot, op, imm32, slot_map) {
                        return;
                    }
                }
            }
        }

        // A register-assigned destination may also be an input on the RHS
        // (for example `bit = 1 << bit`). Preserve that input before loading
        // the LHS into the destination register; otherwise the two-address
        // lowering silently shifts by the newly loaded LHS value.
        if matches!(rhs, RuntimeOperand::Slot(slot) if *slot == dst_slot)
            && slot_map.reg(dst_slot).is_some()
        {
            self.emit_load_operand_to_rcx(rhs, slot_map);
            self.emit_load_operand_to_rax(lhs, slot_map);
            self.emit_runtime_binop_rax_rcx(op);
            self.emit_store_rax_to_slot(dst_slot, slot_map);
            return;
        }

        // General path: load lhs to RAX, rhs to RCX, op, store to dst.
        // But we can do better if dst is in a register.
        if let Some(dst_reg) = slot_map.reg(dst_slot) {
            self.emit_load_operand_to_reg(dst_reg, lhs, slot_map);
            // First try the simple in-place reg path.
            if self.try_emit_binop_reg_operand(dst_reg, op, rhs, slot_map) {
                return;
            }
            // For div/shift with an immediate, check for fast paths before rax round-trip.
            if let RuntimeOperand::Imm(imm_val) = rhs {
                if let Some(imm32) = imm32_sign_extended(*imm_val) {
                    match op {
                        RuntimeBinOp::DivUnsigned => {
                            if imm32 == 1 { return; }
                            if imm32 > 0 {
                                if let Some(shift) = pow2_shift_u32(imm32 as u32) {
                                    self.emit_shr_reg_imm8(dst_reg, shift);
                                    return;
                                }
                            }
                        }
                        RuntimeBinOp::ModUnsigned => {
                            if imm32 == 1 { self.emit_xor_reg_reg(dst_reg, dst_reg); return; }
                            if imm32 > 0 && (imm32 as u32).is_power_of_two() {
                                let mask = (imm32 - 1) as i32;
                                self.emit_alu_reg_imm(4, dst_reg, mask.to_le_bytes());
                                return;
                            }
                        }
                        RuntimeBinOp::Shl => {
                            if imm32 >= 0 && imm32 < 64 {
                                self.emit_shl_reg_imm8(dst_reg, imm32 as u8);
                                return;
                            }
                        }
                        RuntimeBinOp::ShrUnsigned => {
                            if imm32 >= 0 && imm32 < 64 {
                                self.emit_shr_reg_imm8(dst_reg, imm32 as u8);
                                return;
                            }
                        }
                        RuntimeBinOp::ShrSigned => {
                            if imm32 >= 0 && imm32 < 64 {
                                self.emit_sar_reg_imm8(dst_reg, imm32 as u8);
                                return;
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Fallback for complex ops (div/mod/shift with non-immediate rhs)
            self.emit_mov_reg_reg(0, dst_reg); // rax = dst_reg
            self.emit_load_operand_to_rcx(rhs, slot_map);
            self.emit_runtime_binop_rax_rcx(op);
            self.emit_mov_reg_reg(dst_reg, 0); // dst_reg = rax
            return;
        }

        // Absolute fallback: use RAX as accumulator.
        self.emit_load_operand_to_rax(lhs, slot_map);
        // Fast-paths: div/shift by immediate can avoid loading rcx.
        if let RuntimeOperand::Imm(imm_val) = rhs {
            if let Some(imm32) = imm32_sign_extended(*imm_val) {
                let handled = match op {
                    RuntimeBinOp::DivUnsigned if imm32 == 1 => { true }
                    RuntimeBinOp::DivUnsigned if imm32 > 0 => {
                        if let Some(shift) = pow2_shift_u32(imm32 as u32) {
                            self.emit_shr_reg_imm8(0, shift); // shr rax, shift
                            true
                        } else { false }
                    }
                    RuntimeBinOp::ModUnsigned if imm32 == 1 => {
                        self.emit_xor_reg_reg(0, 0); // xor rax, rax
                        true
                    }
                    RuntimeBinOp::ModUnsigned if imm32 > 0 && (imm32 as u32).is_power_of_two() => {
                        self.emit_alu_reg_imm(4, 0, (imm32 - 1).to_le_bytes()); // and rax, mask
                        true
                    }
                    RuntimeBinOp::Shl if imm32 >= 0 && imm32 < 64 => {
                        self.emit_shl_reg_imm8(0, imm32 as u8);
                        true
                    }
                    RuntimeBinOp::ShrUnsigned if imm32 >= 0 && imm32 < 64 => {
                        self.emit_shr_reg_imm8(0, imm32 as u8);
                        true
                    }
                    RuntimeBinOp::ShrSigned if imm32 >= 0 && imm32 < 64 => {
                        self.emit_sar_reg_imm8(0, imm32 as u8);
                        true
                    }
                    _ => false,
                };
                if handled {
                    self.emit_store_rax_to_slot(dst_slot, slot_map);
                    return;
                }
            }
        }
        self.emit_load_operand_to_rcx(rhs, slot_map);
        self.emit_runtime_binop_rax_rcx(op);
        self.emit_store_rax_to_slot(dst_slot, slot_map);
    }

    fn emit_load_operand_to_reg(&mut self, reg: u8, op: &RuntimeOperand, slot_map: &RuntimeSlotMap) {
        match op {
            RuntimeOperand::Imm(val) => self.emit_mov_reg_imm64(reg, *val),
            RuntimeOperand::Slot(slot) => self.emit_load_slot_to_reg(reg, *slot, slot_map),
        }
    }

    fn try_emit_binop_reg_operand(
        &mut self,
        dst_reg: u8,
        op: RuntimeBinOp,
        rhs: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) -> bool {
        if matches!(
            op,
            RuntimeBinOp::DivUnsigned
                | RuntimeBinOp::DivSigned
                | RuntimeBinOp::ModUnsigned
                | RuntimeBinOp::ModSigned
                | RuntimeBinOp::Shl
                | RuntimeBinOp::ShrUnsigned
                | RuntimeBinOp::ShrSigned
        ) {
            return false;
        }
        match rhs {
            RuntimeOperand::Imm(val) => {
                if let Some(imm32) = imm32_sign_extended(*val) {
                    self.emit_binop_reg_imm_in_place(op, dst_reg, imm32);
                    return true;
                }
                false
            }
            RuntimeOperand::Slot(slot) => {
                if let Some(src_reg) = slot_map.reg(*slot) {
                    self.emit_binop_reg_reg_in_place(op, dst_reg, src_reg);
                    return true;
                }
                false
            }
        }
    }

    fn try_emit_bulk_zero_stack_run(
        &mut self,
        instrs: &[RuntimeInstr],
        slot_map: &RuntimeSlotMap,
    ) -> bool {
        const MIN_VECTOR_ZERO_QWORDS: usize = 8;
        const MIN_ERMS_ZERO_QWORDS: usize = 64;
        if instrs.len() < MIN_VECTOR_ZERO_QWORDS {
            return false;
        }

        let packed_byte_run = instrs.iter().all(|instr| {
            matches!(instr, RuntimeInstr::Mov { dst, src: RuntimeOperand::Imm(0) }
                if slot_map.element_width(*dst) == 1)
        });
        if packed_byte_run {
            let mut disps = Vec::with_capacity(instrs.len());
            for instr in instrs {
                let RuntimeInstr::Mov { dst, .. } = instr else {
                    unreachable!();
                };
                disps.push(slot_map.stack_disp(*dst).expect("packed byte stack slot"));
            }
            disps.sort_unstable();
            disps.dedup();
            if disps.len() != instrs.len()
                || disps
                    .windows(2)
                    .any(|window| window[1] != window[0].saturating_add(1))
            {
                return false;
            }
            self.code.push(0x57); // push rdi
            self.emit_lea_rdi_rbp_disp(disps[0]);
            self.code.extend_from_slice(&[0x31, 0xC0]); // xor eax, eax
            self.code.push(0xB9); // mov ecx, byte count
            self.code
                .extend_from_slice(&(disps.len() as u32).to_le_bytes());
            self.code.extend_from_slice(&[0xF3, 0xAA]); // rep stosb
            self.code.push(0x5F); // pop rdi
            return true;
        }

        let mut stack_indices = Vec::with_capacity(instrs.len());
        for instr in instrs {
            let RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Imm(0),
            } = instr
            else {
                return false;
            };
            let Some(stack_idx) = slot_map.stack_index(*dst) else {
                return false;
            };
            stack_indices.push(stack_idx);
        }

        stack_indices.sort_unstable();
        stack_indices.dedup();
        if stack_indices.len() != instrs.len() {
            return false;
        }
        let first = stack_indices[0];
        let last = *stack_indices.last().expect("non-empty after len check");
        if last - first + 1 != stack_indices.len() {
            return false;
        }

        // Stack slots are [rbp - 8], [rbp - 16], ...; start at the most negative
        // address and clear upward. Use size-tiered zeroing:
        // small ranges fall back to scalar stores (caller path),
        // medium ranges use vector stores,
        // large ranges use ERMS-style rep stosq.
        self.code.push(0x57); // push rdi
        self.emit_lea_rdi_rbp_disp(stack_slot_disp(last));
        if stack_indices.len() >= MIN_ERMS_ZERO_QWORDS {
            self.code.extend_from_slice(&[0x31, 0xC0]); // xor eax, eax
            self.code.push(0xB9); // mov ecx, imm32
            self.code
                .extend_from_slice(&(stack_indices.len() as u32).to_le_bytes());
            self.code.extend_from_slice(&[0xF3, 0x48, 0xAB]); // rep stosq
        } else {
            self.code.extend_from_slice(&[0x66, 0x0F, 0xEF, 0xC0]); // pxor xmm0, xmm0
            let vec_chunks = stack_indices.len() / 2;
            for i in 0..vec_chunks {
                self.emit_movups_rdi_disp_xmm0((i * 16) as i32);
            }
            if (stack_indices.len() & 1) != 0 {
                self.code.extend_from_slice(&[0x31, 0xC0]); // xor eax, eax
                self.emit_mov_rdi_disp_rax((vec_chunks * 16) as i32);
            }
        }
        self.code.push(0x5F); // pop rdi
        true
    }

    fn contiguous_stack_base_access(
        &self,
        base_slots: &[usize],
        slot_map: &RuntimeSlotMap,
    ) -> Option<(i32, bool, u8)> {
        if base_slots.is_empty() {
            return None;
        }
        let width = slot_map.element_width(base_slots[0]);
        let mut first_disp = None;
        let mut step = 0i32;
        for (i, &slot) in base_slots.iter().enumerate() {
            if slot_map.reg(slot).is_some() {
                return None;
            }
            if slot_map.element_width(slot) != width {
                return None;
            }
            let disp = slot_map.stack_disp(slot)?;
            if i == 0 {
                first_disp = Some(disp);
            } else {
                let first = first_disp.expect("set when i == 0");
                if i == 1 {
                    let diff = disp - first;
                    if diff != i32::from(width) && diff != -i32::from(width) {
                        return None;
                    }
                    step = diff;
                }
                let expected = first + step * i as i32;
                if disp != expected {
                    return None;
                }
            }
        }
        let first = first_disp.expect("contiguous slots should set first displacement");
        let needs_neg = step < 0;
        Some((first, needs_neg, width))
    }

    fn emit_indexed_rbp_mem_reg_with_index(
        &mut self,
        reg: u8,
        index_reg: u8,
        base_disp: i32,
        width: u8,
        load: bool,
    ) {
        debug_assert!(width == 1 || width == 8);
        let mut rex = if width == 8 { 0x48u8 } else { 0x40u8 };
        if reg >= 8 {
            rex |= 0x04; // REX.R selects high ModRM.reg bit
        }
        if index_reg >= 8 {
            rex |= 0x02; // REX.X selects high SIB.index bit
        }
        if width == 8 || rex != 0x40 || (!load && reg >= 4) {
            self.code.push(rex);
        }
        if width == 1 && load {
            self.code.extend_from_slice(&[0x0F, 0xB6]); // movzx r32, byte ptr
        } else {
            self.code.push(if load { 0x8B } else if width == 1 { 0x88 } else { 0x89 });
        }
        let scale = if width == 8 { 0xC0 } else { 0x00 };
        let sib = scale | ((index_reg & 0x7) << 3) | 0x05;
        if i8::try_from(base_disp).is_ok() {
            self.code.push(0x44 | ((reg & 0x7) << 3)); // mod=01, rm=sib
            self.code.push(sib);
            self.code.push(base_disp as i8 as u8);
        } else {
            self.code.push(0x84 | ((reg & 0x7) << 3)); // mod=10, rm=sib
            self.code.push(sib);
            self.code.extend_from_slice(&base_disp.to_le_bytes());
        }
    }

    fn emit_indexed_rbp_mem_reg(&mut self, reg: u8, base_disp: i32, width: u8, load: bool) {
        self.emit_indexed_rbp_mem_reg_with_index(reg, 1, base_disp, width, load); // RCX index
    }

    fn emit_indexed_rbp_mem_bitop_reg(
        &mut self,
        opcode: u8,
        bit_reg: u8,
        index_reg: u8,
        base_disp: i32,
    ) {
        let mut rex = 0x48u8; // REX.W
        if bit_reg >= 8 {
            rex |= 0x04; // REX.R
        }
        if index_reg >= 8 {
            rex |= 0x02; // REX.X
        }
        self.code.push(rex);
        self.code.push(0x0F);
        self.code.push(opcode); // 0xA3 = bt r/m64, r64 ; 0xAB = bts r/m64, r64
        let sib = 0xC0u8 | ((index_reg & 0x7) << 3) | 0x05; // scale=8,index,base=rbp
        if i8::try_from(base_disp).is_ok() {
            self.code.push(0x44 | ((bit_reg & 0x7) << 3)); // mod=01, rm=sib
            self.code.push(sib);
            self.code.push(base_disp as i8 as u8);
        } else {
            self.code.push(0x84 | ((bit_reg & 0x7) << 3)); // mod=10, rm=sib
            self.code.push(sib);
            self.code.extend_from_slice(&base_disp.to_le_bytes());
        }
    }

    fn emit_contiguous_stack_bitop_indexed(
        &mut self,
        base_slots: &[usize],
        index: &RuntimeOperand,
        bit: &RuntimeOperand,
        checked: bool,
        allow_dynamic_bit: bool,
        opcode: u8,
        slot_map: &RuntimeSlotMap,
        oob_exit_patches: &mut Vec<usize>,
    ) -> bool {
        let dynamic_bit = !matches!(bit, RuntimeOperand::Imm(_));
        if dynamic_bit && !allow_dynamic_bit {
            return false;
        }
        if dynamic_bit && base_slots.len() > 16 {
            // Large-table dynamic bit tests are typically faster as load+shift+and.
            return false;
        }
        let Some((base_disp, needs_neg, width)) = self.contiguous_stack_base_access(base_slots, slot_map) else {
            return false;
        };
        if width != 8 {
            return false;
        }

        self.emit_load_operand_to_rcx(index, slot_map);
        if checked {
            self.code.extend_from_slice(&[0x48, 0x81, 0xF9]); // cmp rcx, imm32
            self.code
                .extend_from_slice(&(base_slots.len() as u32).to_le_bytes());
            self.code.extend_from_slice(&[0x0F, 0x83]); // jae rel32
            let pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            oob_exit_patches.push(pos);
        }
        if needs_neg {
            self.code.extend_from_slice(&[0x48, 0xF7, 0xD9]); // neg rcx
        }

        self.emit_load_operand_to_reg(2, bit, slot_map); // rdx
        if dynamic_bit {
            self.emit_and_reg_imm32(2, 63);
        }
        self.emit_indexed_rbp_mem_bitop_reg(opcode, 2, 1, base_disp); // [rbp+rcx*8+disp], rdx
        true
    }

    fn emit_runtime_bit_set_indexed_direct(
        &mut self,
        base_slots: &[usize],
        index: &RuntimeOperand,
        bit: &RuntimeOperand,
        checked: bool,
        slot_map: &RuntimeSlotMap,
        oob_exit_patches: &mut Vec<usize>,
    ) -> bool {
        self.emit_contiguous_stack_bitop_indexed(
            base_slots,
            index,
            bit,
            checked,
            false,
            0xAB, // bts r/m64, r64
            slot_map,
            oob_exit_patches,
        )
    }

    fn emit_runtime_bit_test_indexed_to_slot(
        &mut self,
        dst: usize,
        base_slots: &[usize],
        index: &RuntimeOperand,
        bit: &RuntimeOperand,
        checked: bool,
        slot_map: &RuntimeSlotMap,
        oob_exit_patches: &mut Vec<usize>,
    ) -> bool {
        if !self.emit_contiguous_stack_bitop_indexed(
            base_slots,
            index,
            bit,
            checked,
            true,
            0xA3, // bt r/m64, r64
            slot_map,
            oob_exit_patches,
        ) {
            return false;
        }
        self.code.extend_from_slice(&[0x0F, 0x92, 0xC0]); // setb al (CF)
        self.code.extend_from_slice(&[0x48, 0x0F, 0xB6, 0xC0]); // movzx rax, al
        self.emit_store_rax_to_slot(dst, slot_map);
        true
    }

    fn emit_contiguous_stack_index_access(
        &mut self,
        base_slots: &[usize],
        index: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
        reg: u8,
        load: bool,
        checked: bool,
        oob_exit_patches: &mut Vec<usize>,
    ) -> bool {
        let Some((base_disp, needs_neg, width)) = self.contiguous_stack_base_access(base_slots, slot_map) else {
            return false;
        };

        let direct_index = if needs_neg {
            None
        } else if let RuntimeOperand::Slot(index_slot) = index {
            slot_map.reg(*index_slot).map(|reg| (*index_slot, reg))
        } else {
            None
        };

        if let Some((index_slot, index_reg)) = direct_index {
            if checked {
                let len_i32 = i32::try_from(base_slots.len()).unwrap_or(i32::MAX);
                if self.emit_cmp_slot_imm(index_slot, len_i32, slot_map) {
                    self.code.extend_from_slice(&[0x0F, 0x83]); // jae rel32
                    let pos = self.code.len();
                    self.code.extend_from_slice(&0_i32.to_le_bytes());
                    oob_exit_patches.push(pos);
                } else {
                    self.emit_load_operand_to_rcx(index, slot_map);
                    self.code.extend_from_slice(&[0x48, 0x81, 0xF9]); // cmp rcx, imm32
                    self.code
                        .extend_from_slice(&(base_slots.len() as u32).to_le_bytes());
                    self.code.extend_from_slice(&[0x0F, 0x83]); // jae rel32
                    let pos = self.code.len();
                    self.code.extend_from_slice(&0_i32.to_le_bytes());
                    oob_exit_patches.push(pos);
                }
            }
            self.emit_indexed_rbp_mem_reg_with_index(reg, index_reg, base_disp, width, load);
            return true;
        }

        self.emit_load_operand_to_rcx(index, slot_map);
        if checked {
            self.code.extend_from_slice(&[0x48, 0x81, 0xF9]); // cmp rcx, imm32
            self.code.extend_from_slice(&(base_slots.len() as u32).to_le_bytes());
            self.code.extend_from_slice(&[0x0F, 0x83]); // jae rel32
            let pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            oob_exit_patches.push(pos);
        }
        if needs_neg {
            self.code.extend_from_slice(&[0x48, 0xF7, 0xD9]); // neg rcx
        }
        self.emit_indexed_rbp_mem_reg(reg, base_disp, width, load);
        true
    }

    fn emit_runtime_index_increment_direct(
        &mut self,
        base_slots: &[usize],
        index: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) -> bool {
        let Some((base_disp, needs_neg, width)) =
            self.contiguous_stack_base_access(base_slots, slot_map)
        else {
            return false;
        };
        if width != 8 {
            return false;
        }

        let index_reg = if !needs_neg
            && let RuntimeOperand::Slot(index_slot) = index
            && let Some(reg) = slot_map.reg(*index_slot)
        {
            reg
        } else {
            self.emit_load_operand_to_rcx(index, slot_map);
            if needs_neg {
                self.code.extend_from_slice(&[0x48, 0xF7, 0xD9]); // neg rcx
            }
            1
        };

        let mut rex = 0x48u8; // REX.W
        if index_reg >= 8 {
            rex |= 0x02; // REX.X
        }
        self.code.push(rex);
        self.code.push(0xFF); // inc r/m64 (/0)
        let sib = 0xC0 | ((index_reg & 7) << 3) | 0x05;
        if i8::try_from(base_disp).is_ok() {
            self.code.push(0x44); // mod=01, /0, rm=sib
            self.code.push(sib);
            self.code.push(base_disp as i8 as u8);
        } else {
            self.code.push(0x84); // mod=10, /0, rm=sib
            self.code.push(sib);
            self.code.extend_from_slice(&base_disp.to_le_bytes());
        }
        true
    }

    fn try_emit_stack_index_access(
        &mut self,
        base_slots: &[usize],
        index: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
        load_to_rax: bool,
        oob_exit_patches: &mut Vec<usize>,
    ) -> bool {
        self.emit_contiguous_stack_index_access(
            base_slots,
            index,
            slot_map,
            0,
            load_to_rax,
            true,
            oob_exit_patches,
        )
    }

    fn try_emit_stack_index_access_unchecked(
        &mut self,
        base_slots: &[usize],
        index: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
        load_to_rax: bool,
    ) -> bool {
        let mut discard_patches = Vec::new();
        self.emit_contiguous_stack_index_access(
            base_slots,
            index,
            slot_map,
            0,
            load_to_rax,
            false,
            &mut discard_patches,
        )
    }

    fn emit_runtime_load_index(
        &mut self,
        dst: usize,
        base_slots: &[usize],
        index: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
        oob_exit_patches: &mut Vec<usize>,
    ) {
        if let RuntimeOperand::Imm(raw_index) = index {
            if let Ok(idx) = usize::try_from(*raw_index) {
                if idx < base_slots.len() {
                    self.emit_mov_slot_slot(dst, base_slots[idx], slot_map);
                    return;
                }
            }
            self.emit_exit(255);
            return;
        }

        if let Some(dst_reg) = slot_map.reg(dst) {
            if self.emit_contiguous_stack_index_access(
                base_slots,
                index,
                slot_map,
                dst_reg,
                true,
                true,
                oob_exit_patches,
            ) {
                return;
            }
        }
        if self.try_emit_stack_index_access(base_slots, index, slot_map, true, oob_exit_patches) {
            self.emit_store_rax_to_slot(dst, slot_map);
            return;
        }
        // Fallback: O(1) indexed jump table.
        self.emit_load_operand_to_rcx(index, slot_map);
        self.code.extend_from_slice(&[0x48, 0x81, 0xF9]); // cmp rcx, imm32
        self.code
            .extend_from_slice(&(base_slots.len() as u32).to_le_bytes());
        self.code.extend_from_slice(&[0x0F, 0x83]); // jae rel32
        let jae_oob_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        // lea rdx, [rip + table]
        self.code.extend_from_slice(&[0x48, 0x8D, 0x15]);
        let lea_table_disp_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        // movsxd rcx, dword [rdx + rcx*4]
        self.code.extend_from_slice(&[0x48, 0x63, 0x0C, 0x8A]);
        // add rcx, rdx
        self.code.extend_from_slice(&[0x48, 0x01, 0xD1]);
        // jmp rcx
        self.code.extend_from_slice(&[0xFF, 0xE1]);

        let table_start = self.code.len();
        let mut table_disp_pos = Vec::with_capacity(base_slots.len());
        for (i, &slot) in base_slots.iter().enumerate() {
            let _ = slot;
            let _ = i;
            table_disp_pos.push(self.code.len());
            self.code.extend_from_slice(&0_i32.to_le_bytes());
        }

        let mut done_jumps = Vec::with_capacity(base_slots.len());
        for (i, &slot) in base_slots.iter().enumerate() {
            let case_label = self.code.len();
            let case_disp = i32::try_from((case_label as i64) - (table_start as i64))
                .expect("index jump-table displacement out of range");
            self.code[table_disp_pos[i]..table_disp_pos[i] + 4]
                .copy_from_slice(&case_disp.to_le_bytes());

            self.emit_load_slot_to_rax(slot, slot_map);
            self.emit_store_rax_to_slot(dst, slot_map);

            self.code.push(0xE9); // jmp rel32
            let done_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            done_jumps.push(done_pos);
        }

        let oob_target = self.code.len();
        self.emit_exit(255);
        let end_target = self.code.len();
        for p in done_jumps {
            patch_rel32(&mut self.code, p, end_target);
        }
        patch_rel32(&mut self.code, jae_oob_pos, oob_target);
        patch_rel32(&mut self.code, lea_table_disp_pos, table_start);
    }

    fn emit_runtime_store_index(
        &mut self,
        base_slots: &[usize],
        index: &RuntimeOperand,
        src: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
        oob_exit_patches: &mut Vec<usize>,
    ) {
        if let RuntimeOperand::Imm(raw_index) = index {
            if let Ok(idx) = usize::try_from(*raw_index) {
                if idx < base_slots.len() {
                    self.emit_mov_slot_operand(base_slots[idx], src, slot_map);
                    return;
                }
            }
            self.emit_exit(255);
            return;
        }

        if let RuntimeOperand::Slot(src_slot) = src {
            if let Some(src_reg) = slot_map.reg(*src_slot) {
                if self.emit_contiguous_stack_index_access(
                    base_slots,
                    index,
                    slot_map,
                    src_reg,
                    false,
                    true,
                    oob_exit_patches,
                ) {
                    return;
                }
            }
        }
        self.emit_load_operand_to_rax(src, slot_map);
        if self.try_emit_stack_index_access(base_slots, index, slot_map, false, oob_exit_patches) {
            return;
        }
        // Fallback: O(1) indexed jump table.
        self.emit_load_operand_to_rcx(index, slot_map);
        self.code.extend_from_slice(&[0x48, 0x81, 0xF9]); // cmp rcx, imm32
        self.code
            .extend_from_slice(&(base_slots.len() as u32).to_le_bytes());
        self.code.extend_from_slice(&[0x0F, 0x83]); // jae rel32
        let jae_oob_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());

        // lea rdx, [rip + table]
        self.code.extend_from_slice(&[0x48, 0x8D, 0x15]);
        let lea_table_disp_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        // movsxd rcx, dword [rdx + rcx*4]
        self.code.extend_from_slice(&[0x48, 0x63, 0x0C, 0x8A]);
        // add rcx, rdx
        self.code.extend_from_slice(&[0x48, 0x01, 0xD1]);
        // jmp rcx
        self.code.extend_from_slice(&[0xFF, 0xE1]);

        let table_start = self.code.len();
        let mut table_disp_pos = Vec::with_capacity(base_slots.len());
        for (i, &slot) in base_slots.iter().enumerate() {
            let _ = slot;
            let _ = i;
            table_disp_pos.push(self.code.len());
            self.code.extend_from_slice(&0_i32.to_le_bytes());
        }

        let mut done_jumps = Vec::with_capacity(base_slots.len());
        for (i, &slot) in base_slots.iter().enumerate() {
            let case_label = self.code.len();
            let case_disp = i32::try_from((case_label as i64) - (table_start as i64))
                .expect("index jump-table displacement out of range");
            self.code[table_disp_pos[i]..table_disp_pos[i] + 4]
                .copy_from_slice(&case_disp.to_le_bytes());

            self.emit_store_rax_to_slot(slot, slot_map);

            self.code.push(0xE9); // jmp rel32
            let done_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            done_jumps.push(done_pos);
        }

        let oob_target = self.code.len();
        self.emit_exit(255);
        let end_target = self.code.len();
        for p in done_jumps {
            patch_rel32(&mut self.code, p, end_target);
        }
        patch_rel32(&mut self.code, jae_oob_pos, oob_target);
        patch_rel32(&mut self.code, lea_table_disp_pos, table_start);
    }

    fn emit_runtime_load_index_unchecked(
        &mut self,
        dst: usize,
        base_slots: &[usize],
        index: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        if let Some(idx) = runtime_const_index_for_access(base_slots, index) {
            self.emit_mov_slot_slot(dst, base_slots[idx], slot_map);
            return;
        }

        if let Some(dst_reg) = slot_map.reg(dst) {
            let mut discard_patches = Vec::new();
            if self.emit_contiguous_stack_index_access(
                base_slots,
                index,
                slot_map,
                dst_reg,
                true,
                false,
                &mut discard_patches,
            ) {
                return;
            }
        }
        if self.try_emit_stack_index_access_unchecked(base_slots, index, slot_map, true) {
            self.emit_store_rax_to_slot(dst, slot_map);
            return;
        }
        // Unchecked fallback: jump-table dispatch without OOB guard.
        self.emit_load_operand_to_rcx(index, slot_map);
        // lea rdx, [rip + table]
        self.code.extend_from_slice(&[0x48, 0x8D, 0x15]);
        let lea_table_disp_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        // movsxd rcx, dword [rdx + rcx*4]
        self.code.extend_from_slice(&[0x48, 0x63, 0x0C, 0x8A]);
        // add rcx, rdx
        self.code.extend_from_slice(&[0x48, 0x01, 0xD1]);
        // jmp rcx
        self.code.extend_from_slice(&[0xFF, 0xE1]);

        let table_start = self.code.len();
        let mut table_disp_pos = Vec::with_capacity(base_slots.len());
        for _ in base_slots {
            table_disp_pos.push(self.code.len());
            self.code.extend_from_slice(&0_i32.to_le_bytes());
        }

        let mut done_jumps = Vec::with_capacity(base_slots.len());
        for (i, &slot) in base_slots.iter().enumerate() {
            let case_label = self.code.len();
            let case_disp = i32::try_from((case_label as i64) - (table_start as i64))
                .expect("index jump-table displacement out of range");
            self.code[table_disp_pos[i]..table_disp_pos[i] + 4]
                .copy_from_slice(&case_disp.to_le_bytes());

            self.emit_load_slot_to_rax(slot, slot_map);
            self.emit_store_rax_to_slot(dst, slot_map);

            self.code.push(0xE9); // jmp rel32
            let done_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            done_jumps.push(done_pos);
        }

        let end_target = self.code.len();
        for p in done_jumps {
            patch_rel32(&mut self.code, p, end_target);
        }
        patch_rel32(&mut self.code, lea_table_disp_pos, table_start);
    }

    fn emit_runtime_store_index_unchecked(
        &mut self,
        base_slots: &[usize],
        index: &RuntimeOperand,
        src: &RuntimeOperand,
        slot_map: &RuntimeSlotMap,
    ) {
        if let Some(idx) = runtime_const_index_for_access(base_slots, index) {
            self.emit_mov_slot_operand(base_slots[idx], src, slot_map);
            return;
        }

        if let RuntimeOperand::Slot(src_slot) = src {
            if let Some(src_reg) = slot_map.reg(*src_slot) {
                let mut discard_patches = Vec::new();
                if self.emit_contiguous_stack_index_access(
                    base_slots,
                    index,
                    slot_map,
                    src_reg,
                    false,
                    false,
                    &mut discard_patches,
                ) {
                    return;
                }
            }
        }
        self.emit_load_operand_to_rax(src, slot_map);
        if self.try_emit_stack_index_access_unchecked(base_slots, index, slot_map, false) {
            return;
        }
        // Unchecked fallback: jump-table dispatch without OOB guard.
        self.emit_load_operand_to_rcx(index, slot_map);
        // lea rdx, [rip + table]
        self.code.extend_from_slice(&[0x48, 0x8D, 0x15]);
        let lea_table_disp_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        // movsxd rcx, dword [rdx + rcx*4]
        self.code.extend_from_slice(&[0x48, 0x63, 0x0C, 0x8A]);
        // add rcx, rdx
        self.code.extend_from_slice(&[0x48, 0x01, 0xD1]);
        // jmp rcx
        self.code.extend_from_slice(&[0xFF, 0xE1]);

        let table_start = self.code.len();
        let mut table_disp_pos = Vec::with_capacity(base_slots.len());
        for _ in base_slots {
            table_disp_pos.push(self.code.len());
            self.code.extend_from_slice(&0_i32.to_le_bytes());
        }

        let mut done_jumps = Vec::with_capacity(base_slots.len());
        for (i, &slot) in base_slots.iter().enumerate() {
            let case_label = self.code.len();
            let case_disp = i32::try_from((case_label as i64) - (table_start as i64))
                .expect("index jump-table displacement out of range");
            self.code[table_disp_pos[i]..table_disp_pos[i] + 4]
                .copy_from_slice(&case_disp.to_le_bytes());

            self.emit_store_rax_to_slot(slot, slot_map);

            self.code.push(0xE9); // jmp rel32
            let done_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            done_jumps.push(done_pos);
        }

        let end_target = self.code.len();
        for p in done_jumps {
            patch_rel32(&mut self.code, p, end_target);
        }
        patch_rel32(&mut self.code, lea_table_disp_pos, table_start);
    }

    fn emit_runtime_binop_rax_rcx(&mut self, op: RuntimeBinOp) {
        match op {
            RuntimeBinOp::Add => self.code.extend_from_slice(&[0x48, 0x01, 0xC8]), // add rax, rcx
            RuntimeBinOp::Sub => self.code.extend_from_slice(&[0x48, 0x29, 0xC8]), // sub rax, rcx
            RuntimeBinOp::Mul => self.code.extend_from_slice(&[0x48, 0x0F, 0xAF, 0xC1]), // imul rax, rcx
            RuntimeBinOp::DivUnsigned => {
                self.code.extend_from_slice(&[0x31, 0xD2]); // xor edx, edx
                self.code.extend_from_slice(&[0x48, 0xF7, 0xF1]); // div rcx
            }
            RuntimeBinOp::DivSigned => {
                self.code.extend_from_slice(&[0x48, 0x99]); // cqo
                self.code.extend_from_slice(&[0x48, 0xF7, 0xF9]); // idiv rcx
            }
            RuntimeBinOp::ModUnsigned => {
                self.code.extend_from_slice(&[0x31, 0xD2]); // xor edx, edx
                self.code.extend_from_slice(&[0x48, 0xF7, 0xF1]); // div rcx
                self.code.extend_from_slice(&[0x48, 0x89, 0xD0]); // mov rax, rdx
            }
            RuntimeBinOp::ModSigned => {
                self.code.extend_from_slice(&[0x48, 0x99]); // cqo
                self.code.extend_from_slice(&[0x48, 0xF7, 0xF9]); // idiv rcx
                self.code.extend_from_slice(&[0x48, 0x89, 0xD0]); // mov rax, rdx
            }
            RuntimeBinOp::BitAnd => self.code.extend_from_slice(&[0x48, 0x21, 0xC8]), // and rax, rcx
            RuntimeBinOp::BitOr => self.code.extend_from_slice(&[0x48, 0x09, 0xC8]),  // or rax, rcx
            RuntimeBinOp::BitXor => self.code.extend_from_slice(&[0x48, 0x31, 0xC8]), // xor rax, rcx
            RuntimeBinOp::Shl => self.code.extend_from_slice(&[0x48, 0xD3, 0xE0]),    // shl rax, cl
            RuntimeBinOp::ShrUnsigned => self.code.extend_from_slice(&[0x48, 0xD3, 0xE8]), // shr rax, cl
            RuntimeBinOp::ShrSigned => self.code.extend_from_slice(&[0x48, 0xD3, 0xF8]), // sar rax, cl
        }
    }

    fn emit_bt_rax_rcx(&mut self) {
        self.code.extend_from_slice(&[0x48, 0x0F, 0xA3, 0xC8]); // bt rax, rcx
    }

    fn emit_bts_rax_rcx(&mut self) {
        self.code.extend_from_slice(&[0x48, 0x0F, 0xAB, 0xC8]); // bts rax, rcx
    }

    fn emit_runtime_float_binop_xmm0_xmm1(&mut self, op: RuntimeFloatBinOp, bits: u16) {
        let opcode = match op {
            RuntimeFloatBinOp::Add => 0x58,
            RuntimeFloatBinOp::Sub => 0x5C,
            RuntimeFloatBinOp::Mul => 0x59,
            RuntimeFloatBinOp::Div => 0x5E,
        };
        match bits {
            32 => self.code.extend_from_slice(&[0xF3, 0x0F, opcode, 0xC1]), // *ss xmm0, xmm1
            64 => self.code.extend_from_slice(&[0xF2, 0x0F, opcode, 0xC1]), // *sd xmm0, xmm1
            _ => panic!("unsupported runtime float width"),
        }
    }

    fn emit_binop_slot_imm_in_place(
        &mut self,
        slot: usize,
        op: RuntimeBinOp,
        imm32: i32,
        slot_map: &RuntimeSlotMap,
    ) -> bool {
        let imm_bytes = imm32.to_le_bytes();
        if let Some(reg) = slot_map.reg(slot) {
            match op {
                RuntimeBinOp::Add => self.emit_alu_reg_imm(0, reg, imm_bytes), // add
                RuntimeBinOp::Sub => self.emit_alu_reg_imm(5, reg, imm_bytes), // sub
                RuntimeBinOp::Mul => {
                    if self.emit_optimal_mul_imm64(reg, imm32 as u64) {
                        return true;
                    }
                    // imul reg, reg, imm32 fallback
                    let mut rex = 0x48u8;
                    if reg >= 8 {
                        rex |= 0x05; // REX.R + REX.B
                    }
                    self.code.push(rex);
                    self.code.push(0x69);
                    self.code.push(0xC0 | ((reg & 0x7) << 3) | (reg & 0x7));
                    self.code.extend_from_slice(&imm_bytes);
                    return true;
                }
                RuntimeBinOp::DivUnsigned => {
                    if imm32 == 1 {
                        return true; // no-op
                    }
                    if imm32 > 0 {
                        if let Some(shift) = pow2_shift_u32(imm32 as u32) {
                            self.emit_shr_reg_imm8(reg, shift);
                            return true;
                        }
                        
                        let d = imm32 as u32;
                        let s = 32 - (d - 1).leading_zeros();
                        let p = 64 + s;
                        let m = (((1u128 << p) + (d as u128) - 1) / (d as u128)) as u64;

                        // Scratch: RAX(0), RCX(1), RDX(2). None are pinned in slot_map.
                        self.emit_mov_reg_reg(0, reg);
                        self.emit_mov_reg_imm64(1, m); // mov rcx, m
                        self.code.extend_from_slice(&[0x48, 0xF7, 0xE1]); // mul rcx (RDX:RAX = RAX * RCX)
                        self.emit_mov_reg_reg(0, reg); // rax = x
                        self.emit_sub_reg_reg(0, 2); // rax = x - q
                        self.emit_shr_reg_imm8(0, 1); // rax = (x-q) >> 1
                        self.emit_add_reg_reg(2, 0); // rdx = q + (x-q)>>1
                        self.emit_shr_reg_imm8(2, (s - 1) as u8);
                        self.emit_mov_reg_reg(reg, 2);
                        return true;
                    }
                    return false;
                }
                RuntimeBinOp::DivSigned => {
                    if imm32 == 1 {
                        return true; // no-op
                    }
                    return false;
                }
                RuntimeBinOp::ModUnsigned => {
                    if imm32 == 1 {
                        self.emit_xor_reg_reg(reg, reg);
                        return true;
                    }
                    if imm32 > 0 && (imm32 as u32).is_power_of_two() {
                        let mask = imm32 - 1;
                        self.emit_alu_reg_imm(4, reg, mask.to_le_bytes());
                        return true;
                    }
                    if imm32 > 0 {
                        let d = imm32 as u32;
                        let s = 32 - (d - 1).leading_zeros();
                        let p = 64 + s;
                        let m = (((1u128 << p) + (d as u128) - 1) / (d as u128)) as u64;

                        self.emit_mov_reg_reg(0, reg);
                        self.emit_mov_reg_imm64(1, m); // mov rcx, m
                        self.code.extend_from_slice(&[0x48, 0xF7, 0xE1]); // mul rcx
                        self.emit_mov_reg_reg(0, reg); // rax = x
                        self.emit_sub_reg_reg(0, 2); 
                        self.emit_shr_reg_imm8(0, 1);
                        self.emit_add_reg_reg(2, 0);
                        self.emit_shr_reg_imm8(2, (s - 1) as u8);
                        
                        // rdx has q. We need x - q*d. 
                        self.code.extend_from_slice(&[0x48, 0x69, 0xD2]);
                        self.code.extend_from_slice(&imm32.to_le_bytes());
                        
                        self.emit_mov_reg_reg(0, reg); // rax = x
                        self.emit_sub_reg_reg(0, 2); // rax = x - q*d
                        self.emit_mov_reg_reg(reg, 0);
                        return true;
                    }
                    return false;
                }
                RuntimeBinOp::ModSigned => {
                    if imm32 == 1 || imm32 == -1 {
                        self.emit_xor_reg_reg(reg, reg);
                        return true;
                    }
                    return false;
                }
                RuntimeBinOp::BitAnd => self.emit_alu_reg_imm(4, reg, imm_bytes), // and
                RuntimeBinOp::BitOr => self.emit_alu_reg_imm(1, reg, imm_bytes),  // or
                RuntimeBinOp::BitXor => self.emit_alu_reg_imm(6, reg, imm_bytes), // xor
                RuntimeBinOp::Shl => {
                    let shift = (imm32 as u32 & 63) as u8;
                    if shift == 0 {
                        return true;
                    }
                    self.emit_shl_reg_imm8(reg, shift);
                }
                RuntimeBinOp::ShrUnsigned => {
                    let shift = (imm32 as u32 & 63) as u8;
                    if shift == 0 {
                        return true;
                    }
                    self.emit_shr_reg_imm8(reg, shift);
                }
                RuntimeBinOp::ShrSigned => {
                    let shift = (imm32 as u32 & 63) as u8;
                    if shift == 0 {
                        return true;
                    }
                    self.emit_sar_reg_imm8(reg, shift);
                }
            }
            return true;
        }
        match op {
            RuntimeBinOp::Add => {
                self.emit_alu_slot_mem_imm(0, slot, imm_bytes, slot_map); // add qword [rbp+disp], imm32
                true
            }
            RuntimeBinOp::Sub => {
                self.emit_alu_slot_mem_imm(5, slot, imm_bytes, slot_map); // sub qword [rbp+disp], imm32
                true
            }
            RuntimeBinOp::Mul => {
                if imm32 == 0 {
                    self.emit_mov_slot_mem_imm32(slot, 0, slot_map);
                    return true;
                }
                if imm32 == 1 {
                    return true; // no-op
                }
                if let Some(shift) = pow2_shift_u32(imm32 as u32) {
                    self.emit_shl_slot_mem_imm8(slot, shift, slot_map);
                    return true;
                }
                false
            }
            RuntimeBinOp::DivUnsigned => {
                if imm32 == 1 {
                    return true;
                }
                if imm32 > 0 {
                    if let Some(shift) = pow2_shift_u32(imm32 as u32) {
                        self.emit_shr_slot_mem_imm8(slot, shift, slot_map);
                        return true;
                    }
                }
                false
            }
            RuntimeBinOp::DivSigned => {
                if imm32 == 1 {
                    return true;
                }
                false
            }
            RuntimeBinOp::ModUnsigned => {
                if imm32 == 1 {
                    self.emit_mov_slot_mem_imm32(slot, 0, slot_map);
                    return true;
                }
                if imm32 > 0 && (imm32 as u32).is_power_of_two() {
                    self.emit_alu_slot_mem_imm(4, slot, (imm32 - 1).to_le_bytes(), slot_map);
                    return true;
                }
                false
            }
            RuntimeBinOp::ModSigned => {
                if imm32 == 1 || imm32 == -1 {
                    self.emit_mov_slot_mem_imm32(slot, 0, slot_map);
                    return true;
                }
                false
            }
            RuntimeBinOp::BitAnd => {
                self.emit_alu_slot_mem_imm(4, slot, imm_bytes, slot_map); // and
                true
            }
            RuntimeBinOp::BitOr => {
                self.emit_alu_slot_mem_imm(1, slot, imm_bytes, slot_map); // or
                true
            }
            RuntimeBinOp::BitXor => {
                self.emit_alu_slot_mem_imm(6, slot, imm_bytes, slot_map); // xor
                true
            }
            RuntimeBinOp::Shl => {
                let shift = (imm32 as u32 & 63) as u8;
                if shift == 0 {
                    return true;
                }
                self.emit_shl_slot_mem_imm8(slot, shift, slot_map);
                true
            }
            RuntimeBinOp::ShrUnsigned => {
                let shift = (imm32 as u32 & 63) as u8;
                if shift == 0 {
                    return true;
                }
                self.emit_shr_slot_mem_imm8(slot, shift, slot_map);
                true
            }
            RuntimeBinOp::ShrSigned => {
                let shift = (imm32 as u32 & 63) as u8;
                if shift == 0 {
                    return true;
                }
                self.emit_sar_slot_mem_imm8(slot, shift, slot_map);
                true
            }
        }
    }

    fn emit_binop_slot_slot_in_place(
        &mut self,
        dst_slot: usize,
        rhs_slot: usize,
        op: RuntimeBinOp,
        slot_map: &RuntimeSlotMap,
    ) -> bool {
        if matches!(
            op,
            RuntimeBinOp::DivUnsigned
                | RuntimeBinOp::DivSigned
                | RuntimeBinOp::ModUnsigned
                | RuntimeBinOp::ModSigned
                | RuntimeBinOp::Shl
                | RuntimeBinOp::ShrUnsigned
                | RuntimeBinOp::ShrSigned
        ) {
            return false;
        }
        match (slot_map.reg(dst_slot), slot_map.reg(rhs_slot)) {
            (Some(dst_reg), Some(rhs_reg)) => {
                self.emit_binop_reg_reg_in_place(op, dst_reg, rhs_reg);
                true
            }
            _ => false,
        }
    }

    fn emit_binop_reg_reg_in_place(&mut self, op: RuntimeBinOp, dst_reg: u8, src_reg: u8) {
        let mut rex = 0x48u8; // REX.W
        if src_reg >= 8 {
            rex |= 0x04; // REX.R
        }
        if dst_reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        match op {
            RuntimeBinOp::Add => {
                self.code.push(0x01); // add r/m64, r64
                self.code
                    .push(0xC0 | ((src_reg & 0x7) << 3) | (dst_reg & 0x7));
            }
            RuntimeBinOp::Sub => {
                self.code.push(0x29); // sub r/m64, r64
                self.code
                    .push(0xC0 | ((src_reg & 0x7) << 3) | (dst_reg & 0x7));
            }
            RuntimeBinOp::Mul => {
                self.code.push(0x0F);
                self.code.push(0xAF); // imul r64, r/m64
                self.code
                    .push(0xC0 | ((dst_reg & 0x7) << 3) | (src_reg & 0x7));
            }
            RuntimeBinOp::BitAnd => {
                self.code.push(0x21); // and r/m64, r64
                self.code
                    .push(0xC0 | ((src_reg & 0x7) << 3) | (dst_reg & 0x7));
            }
            RuntimeBinOp::BitOr => {
                self.code.push(0x09); // or r/m64, r64
                self.code
                    .push(0xC0 | ((src_reg & 0x7) << 3) | (dst_reg & 0x7));
            }
            RuntimeBinOp::BitXor => {
                self.code.push(0x31); // xor r/m64, r64
                self.code
                    .push(0xC0 | ((src_reg & 0x7) << 3) | (dst_reg & 0x7));
            }
            RuntimeBinOp::DivUnsigned
            | RuntimeBinOp::DivSigned
            | RuntimeBinOp::ModUnsigned
            | RuntimeBinOp::ModSigned
            | RuntimeBinOp::Shl
            | RuntimeBinOp::ShrUnsigned
            | RuntimeBinOp::ShrSigned => unreachable!(),
        }
    }

    fn emit_cmp_slot_slot(
        &mut self,
        lhs_slot: usize,
        rhs_slot: usize,
        slot_map: &RuntimeSlotMap,
    ) -> bool {
        match (slot_map.reg(lhs_slot), slot_map.reg(rhs_slot)) {
            (Some(lhs_reg), Some(rhs_reg)) => {
                self.emit_cmp_reg_reg(lhs_reg, rhs_reg); // cmp lhs, rhs
                true
            }
            _ => false,
        }
    }

    fn emit_add_reg_reg(&mut self, dst_reg: u8, src_reg: u8) {
        let mut rex = 0x48u8; // REX.W
        if src_reg >= 8 { rex |= 0x04; } // REX.R
        if dst_reg >= 8 { rex |= 0x01; } // REX.B
        self.code.push(rex);
        self.code.push(0x01); // add r/m64, r64
        self.code.push(0xC0 | ((src_reg & 0x7) << 3) | (dst_reg & 0x7));
    }

    fn emit_sub_reg_reg(&mut self, dst_reg: u8, src_reg: u8) {
        let mut rex = 0x48u8; // REX.W
        if src_reg >= 8 { rex |= 0x04; }
        if dst_reg >= 8 { rex |= 0x01; }
        self.code.push(rex);
        self.code.push(0x29); // sub r/m64, r64
        self.code.push(0xC0 | ((src_reg & 0x7) << 3) | (dst_reg & 0x7));
    }

    fn emit_cmp_reg_reg(&mut self, lhs_reg: u8, rhs_reg: u8) {
        let mut rex = 0x48u8; // REX.W
        if rhs_reg >= 8 {
            rex |= 0x04; // REX.R
        }
        if lhs_reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0x39); // cmp r/m64, r64
        self.code
            .push(0xC0 | ((rhs_reg & 0x7) << 3) | (lhs_reg & 0x7));
    }

    fn emit_cmovb_reg_reg(&mut self, dst_reg: u8, src_reg: u8) {
        let mut rex = 0x48u8; // REX.W
        if dst_reg >= 8 {
            rex |= 0x04; // REX.R
        }
        if src_reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0x0F);
        self.code.push(0x42); // cmovb r64, r/m64
        self.code
            .push(0xC0 | ((dst_reg & 0x7) << 3) | (src_reg & 0x7));
    }

    fn emit_cmova_reg_reg(&mut self, dst_reg: u8, src_reg: u8) {
        let mut rex = 0x48u8; // REX.W
        if dst_reg >= 8 {
            rex |= 0x04; // REX.R
        }
        if src_reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0x0F);
        self.code.push(0x47); // cmova r64, r/m64
        self.code
            .push(0xC0 | ((dst_reg & 0x7) << 3) | (src_reg & 0x7));
    }

    fn emit_cmovg_reg_reg(&mut self, dst_reg: u8, src_reg: u8) {
        let mut rex = 0x48u8; // REX.W
        if dst_reg >= 8 {
            rex |= 0x04; // REX.R
        }
        if src_reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0x0F);
        self.code.push(0x4F); // cmovg r64, r/m64
        self.code
            .push(0xC0 | ((dst_reg & 0x7) << 3) | (src_reg & 0x7));
    }

    fn emit_setb_reg8(&mut self, reg: u8) {
        let mut rex = 0x40u8;
        if reg >= 8 {
            rex |= 0x01; // REX.B
        }
        if reg >= 4 {
            // spl/bpl/sil/dil and all r8b-r15b encodings need a REX prefix.
            self.code.push(rex);
        }
        self.code.push(0x0F);
        self.code.push(0x92); // setb r/m8
        self.code.push(0xC0 | (reg & 0x7));
    }

    fn emit_cmp_slot_imm(&mut self, slot: usize, imm32: i32, slot_map: &RuntimeSlotMap) -> bool {
        if let Some(reg) = slot_map.reg(slot) {
            self.emit_alu_reg_imm(7, reg, imm32.to_le_bytes()); // cmp reg, imm32
            return true;
        }
        self.emit_alu_slot_mem_imm(7, slot, imm32.to_le_bytes(), slot_map); // cmp qword [rbp+disp], imm32
        true
    }

    fn emit_normalize_slot_int(
        &mut self,
        slot: usize,
        signed: bool,
        bits: u16,
        slot_map: &RuntimeSlotMap,
    ) {
        if bits >= 64 {
            return;
        }
        if let Some(reg) = slot_map.reg(slot) {
            self.emit_normalize_reg_int(reg, signed, bits);
            return;
        }
        self.emit_load_slot_to_rax(slot, slot_map);
        self.emit_normalize_reg_int(0, signed, bits);
        self.emit_store_rax_to_slot(slot, slot_map);
    }

    fn emit_normalize_reg_int(&mut self, reg: u8, signed: bool, bits: u16) {
        if bits >= 64 {
            return;
        }
        if signed {
            let shift = (64 - bits) as u8;
            self.emit_shl_reg_imm8(reg, shift);
            self.emit_sar_reg_imm8(reg, shift);
            return;
        }

        match bits {
            8 => self.emit_and_reg_imm32(reg, 0xFF),
            16 => self.emit_and_reg_imm32(reg, 0xFFFF),
            32 => self.emit_mov_reg32_reg32(reg, reg),
            _ => {}
        }
    }

    fn emit_lea_mult_add(&mut self, dst_reg: u8, src_reg: u8, scale: u8) {
        let mut rex = 0x48u8; // REX.W
        if dst_reg >= 8 {
            rex |= 0x04; // REX.R
        }
        if src_reg >= 8 {
            rex |= 0x03; // REX.X + REX.B
        }
        self.code.push(rex);
        self.code.push(0x8D); // LEA
        
        let base = src_reg & 7;
        let mod_val = if base == 5 { 0x40 } else { 0x00 };
        
        let mod_rm = mod_val | ((dst_reg & 7) << 3) | 0x04;
        self.code.push(mod_rm);
        
        let ss = match scale {
            2 => 1,
            4 => 2,
            8 => 3,
            _ => 0,
        } << 6;
        let sib = ss | (base << 3) | base;
        self.code.push(sib);

        if base == 5 {
            self.code.push(0x00);
        }
    }

    fn emit_binop_reg_imm_in_place(&mut self, op: RuntimeBinOp, reg: u8, imm32: i32) {
        match op {
            RuntimeBinOp::Mul => {
                let mut rex = 0x48u8;
                if reg >= 8 {
                    rex |= 0x05; // REX.R | REX.B
                }
                self.code.push(rex);
                self.code.push(0x69); // imul r64, r/m64, imm32
                self.code.push(0xC0 | ((reg & 0x7) << 3) | (reg & 0x7));
                self.code.extend_from_slice(&imm32.to_le_bytes());
            }
            _ => {
                let opcode_ext = match op {
                    RuntimeBinOp::Add => 0,
                    RuntimeBinOp::Sub => 5,
                    RuntimeBinOp::BitAnd => 4,
                    RuntimeBinOp::BitOr => 1,
                    RuntimeBinOp::BitXor => 6,
                    _ => unreachable!(),
                };
                self.emit_alu_reg_imm(opcode_ext, reg, imm32.to_le_bytes());
            }
        }
    }

    fn emit_alu_reg_imm(&mut self, opcode_ext: u8, reg: u8, imm_bytes: [u8; 4]) {
        // 81 /ext r64, imm32
        let mut rex = 0x48u8; // REX.W
        if reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0x81);
        self.code
            .push(0xC0 | ((opcode_ext & 0x7) << 3) | (reg & 0x7));
        self.code.extend_from_slice(&imm_bytes);
    }

    fn emit_xor_reg_reg(&mut self, dst: u8, src: u8) {
        let mut rex = 0x48u8; // REX.W
        if src >= 8 {
            rex |= 0x04; // REX.R
        }
        if dst >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0x31); // xor r/m64, r64
        self.code.push(0xC0 | ((src & 0x7) << 3) | (dst & 0x7));
    }

    fn emit_xor_reg_reg32(&mut self, dst: u8, src: u8) {
        let mut rex = 0x40u8;
        if src >= 8 {
            rex |= 0x04;
        }
        if dst >= 8 {
            rex |= 0x01;
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        self.code.push(0x31);
        self.code.push(0xC0 | ((src & 7) << 3) | (dst & 7));
    }

    fn emit_dec_reg(&mut self, reg: u8) {
        let mut rex = 0x48u8;
        if reg >= 8 {
            rex |= 0x01;
        }
        self.code.push(rex);
        self.code.push(0xFF);
        self.code.push(0xC8 | (reg & 7));
    }

    fn emit_movdqu_xmm_rip_data(&mut self, xmm: u8, bytes: &[u8]) {
        debug_assert_eq!(bytes.len(), 16);
        self.code.push(0xF3);
        if xmm >= 8 {
            self.code.push(0x44); // REX.R
        }
        self.code.extend_from_slice(&[0x0F, 0x6F]);
        self.code.push(0x05 | ((xmm & 7) << 3));
        let disp_pos = self.code.len();
        self.code.extend_from_slice(&0_i32.to_le_bytes());
        let data_offset = if let Some(offset) = self.data_offsets.get(bytes) {
            *offset
        } else {
            let offset = self.data.len();
            self.data.extend_from_slice(bytes);
            self.data_offsets.insert(bytes.to_vec(), offset);
            offset
        };
        self.patches.push(Patch {
            disp_pos,
            data_offset,
        });
    }

    fn emit_sse2_xmm_xmm(&mut self, opcode: u8, dst: u8, src: u8) {
        self.code.push(0x66);
        let mut rex = 0x40u8;
        if dst >= 8 {
            rex |= 0x04;
        }
        if src >= 8 {
            rex |= 0x01;
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        self.code.extend_from_slice(&[0x0F, opcode]);
        self.code.push(0xC0 | ((dst & 7) << 3) | (src & 7));
    }

    fn emit_movdqa_xmm(&mut self, dst: u8, src: u8) {
        self.emit_sse2_xmm_xmm(0x6F, dst, src);
    }

    fn emit_movd_xmm_reg32(&mut self, xmm: u8, reg: u8) {
        self.code.push(0x66);
        let mut rex = 0x40u8;
        if xmm >= 8 {
            rex |= 0x04;
        }
        if reg >= 8 {
            rex |= 0x01;
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        self.code.extend_from_slice(&[0x0F, 0x6E]);
        self.code.push(0xC0 | ((xmm & 7) << 3) | (reg & 7));
    }

    fn emit_movd_reg32_xmm(&mut self, reg: u8, xmm: u8) {
        self.code.push(0x66);
        let mut rex = 0x40u8;
        if xmm >= 8 {
            rex |= 0x04;
        }
        if reg >= 8 {
            rex |= 0x01;
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        self.code.extend_from_slice(&[0x0F, 0x7E]);
        self.code.push(0xC0 | ((xmm & 7) << 3) | (reg & 7));
    }

    fn emit_pshuflw_xmm(&mut self, dst: u8, src: u8, control: u8) {
        self.code.push(0xF2);
        self.code.extend_from_slice(&[0x0F, 0x70]);
        self.code.push(0xC0 | ((dst & 7) << 3) | (src & 7));
        self.code.push(control);
    }

    fn emit_pshufd_xmm(&mut self, dst: u8, src: u8, control: u8) {
        self.code.extend_from_slice(&[0x66, 0x0F, 0x70]);
        self.code.push(0xC0 | ((dst & 7) << 3) | (src & 7));
        self.code.push(control);
    }

    fn emit_punpcklqdq_xmm(&mut self, dst: u8, src: u8) {
        self.emit_sse2_xmm_xmm(0x6C, dst, src);
    }

    fn emit_or_reg_reg(&mut self, dst: u8, src: u8) {
        let mut rex = 0x48u8; // REX.W
        if src >= 8 {
            rex |= 0x04; // REX.R
        }
        if dst >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0x09); // or r/m64, r64
        self.code.push(0xC0 | ((src & 0x7) << 3) | (dst & 0x7));
    }

    fn emit_shl_reg_imm8(&mut self, reg: u8, shift: u8) {
        // C1 /4 r64, imm8
        let mut rex = 0x48u8; // REX.W
        if reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0xC1);
        self.code.push(0xC0 | (4 << 3) | (reg & 0x7));
        self.code.push(shift);
    }

    fn emit_shl_reg_cl(&mut self, reg: u8) {
        // D3 /4 r64, cl
        let mut rex = 0x48u8; // REX.W
        if reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0xD3);
        self.code.push(0xC0 | (4 << 3) | (reg & 0x7));
    }

    fn emit_shr_reg_imm8(&mut self, reg: u8, shift: u8) {
        // C1 /5 r64, imm8
        let mut rex = 0x48u8; // REX.W
        if reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0xC1);
        self.code.push(0xC0 | (5 << 3) | (reg & 0x7));
        self.code.push(shift);
    }

    fn emit_sar_reg_imm8(&mut self, reg: u8, shift: u8) {
        // C1 /7 r64, imm8
        let mut rex = 0x48u8; // REX.W
        if reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0xC1);
        self.code.push(0xC0 | (7 << 3) | (reg & 0x7));
        self.code.push(shift);
    }

    fn emit_and_reg_imm32(&mut self, reg: u8, imm32: u32) {
        // 81 /4 r64, imm32
        let mut rex = 0x48u8; // REX.W
        if reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0x81);
        self.code.push(0xC0 | (4 << 3) | (reg & 0x7));
        self.code.extend_from_slice(&imm32.to_le_bytes());
    }

    fn emit_neg_reg(&mut self, reg: u8) {
        // F7 /3 r64
        let mut rex = 0x48u8; // REX.W
        if reg >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0xF7);
        self.code.push(0xD8 | (reg & 0x7));
    }

    fn emit_movdqu_xmm_rsp_disp(&mut self, xmm: u8, disp: i32, load: bool) {
        let mut rex = 0x40u8;
        if xmm >= 8 {
            rex |= 0x04; // REX.R
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        self.code.extend_from_slice(&[0xF3, 0x0F, if load { 0x6F } else { 0x7F }]);
        if i8::try_from(disp).is_ok() {
            self.code.push(0x44 | ((xmm & 0x7) << 3)); // mod=01, rm=sib
            self.code.push(0x24); // sib base=rsp index=none
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x84 | ((xmm & 0x7) << 3)); // mod=10, rm=sib
            self.code.push(0x24);
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_movdqu_indexed_rbp_mem_xmm(
        &mut self,
        xmm: u8,
        index_reg: u8,
        base_disp: i32,
        width: u8,
        load: bool,
    ) {
        let mut rex = 0x40u8;
        if xmm >= 8 {
            rex |= 0x04; // REX.R
        }
        if index_reg >= 8 {
            rex |= 0x02; // REX.X
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        self.code.extend_from_slice(&[0xF3, 0x0F, if load { 0x6F } else { 0x7F }]);
        let scale = if width == 8 { 0xC0 } else { 0x00 };
        let sib = scale | ((index_reg & 0x7) << 3) | 0x05;
        if i8::try_from(base_disp).is_ok() {
            self.code.push(0x44 | ((xmm & 0x7) << 3)); // mod=01, rm=sib
            self.code.push(sib);
            self.code.push(base_disp as i8 as u8);
        } else {
            self.code.push(0x84 | ((xmm & 0x7) << 3)); // mod=10, rm=sib
            self.code.push(sib);
            self.code.extend_from_slice(&base_disp.to_le_bytes());
        }
    }

    fn emit_por_xmm(&mut self, dst: u8, src: u8) {
        let mut rex = 0x40u8;
        if dst >= 8 {
            rex |= 0x04; // REX.R
        }
        if src >= 8 {
            rex |= 0x01; // REX.B
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        self.code.extend_from_slice(&[0x66, 0x0F, 0xEB]);
        self.code.push(0xC0 | ((dst & 0x7) << 3) | (src & 0x7));
    }

    fn emit_pand_xmm(&mut self, dst: u8, src: u8) {
        let mut rex = 0x40u8;
        if dst >= 8 {
            rex |= 0x04; // REX.R
        }
        if src >= 8 {
            rex |= 0x01; // REX.B
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        self.code.extend_from_slice(&[0x66, 0x0F, 0xDB]);
        self.code.push(0xC0 | ((dst & 0x7) << 3) | (src & 0x7));
    }

    fn emit_pcmpeqb_xmm(&mut self, dst: u8, src: u8) {
        let mut rex = 0x40u8;
        if dst >= 8 {
            rex |= 0x04; // REX.R
        }
        if src >= 8 {
            rex |= 0x01; // REX.B
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        self.code.extend_from_slice(&[0x66, 0x0F, 0x74]);
        self.code.push(0xC0 | ((dst & 0x7) << 3) | (src & 0x7));
    }

    fn emit_pmovmskb_eax_xmm(&mut self, xmm: u8) {
        let mut rex = 0x40u8;
        if xmm >= 8 {
            rex |= 0x01; // REX.B
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        self.code.extend_from_slice(&[0x66, 0x0F, 0xD7]);
        self.code.push(0xC0 | (xmm & 0x7)); // reg=eax(0), rm=xmm
    }

    fn emit_mov_reg32_reg32(&mut self, dst: u8, src: u8) {
        // 89 /r r/m32, r32 (32-bit destination zero-extends into 64-bit reg)
        let mut rex = 0x40u8;
        if src >= 8 {
            rex |= 0x04; // REX.R
        }
        if dst >= 8 {
            rex |= 0x01; // REX.B
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        self.code.push(0x89);
        self.code.push(0xC0 | ((src & 0x7) << 3) | (dst & 0x7));
    }

    fn emit_alu_slot_mem_imm(
        &mut self,
        opcode_ext: u8,
        slot: usize,
        imm_bytes: [u8; 4],
        slot_map: &RuntimeSlotMap,
    ) {
        // 81 /ext r/m64, imm32 with [rbp+disp8|disp32]
        let stack_index = slot_map
            .stack_index(slot)
            .expect("missing stack index for non-pinned runtime slot");
        let disp = stack_slot_disp(stack_index);
        self.code.extend_from_slice(&[0x48, 0x81]); // REX.W + group-1 imm32
        if i8::try_from(disp).is_ok() {
            self.code.push(0x40 | ((opcode_ext & 0x7) << 3) | 0x05); // mod=01, rm=rbp
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x80 | ((opcode_ext & 0x7) << 3) | 0x05); // mod=10, rm=rbp
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
        self.code.extend_from_slice(&imm_bytes);
    }

    fn emit_mov_slot_mem_imm32(&mut self, slot: usize, imm32: i32, slot_map: &RuntimeSlotMap) {
        // C7 /0 r/m64, imm32
        let stack_index = slot_map
            .stack_index(slot)
            .expect("missing stack index for non-pinned runtime slot");
        let disp = stack_slot_disp(stack_index);
        self.code.extend_from_slice(&[0x48, 0xC7]); // REX.W + mov r/m64, imm32
        if i8::try_from(disp).is_ok() {
            self.code.push(0x40 | 0x05); // mod=01, /0, rm=rbp
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x80 | 0x05); // mod=10, /0, rm=rbp
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
        self.code.extend_from_slice(&imm32.to_le_bytes());
    }

    fn emit_shl_slot_mem_imm8(&mut self, slot: usize, shift: u8, slot_map: &RuntimeSlotMap) {
        // C1 /4 r/m64, imm8
        let stack_index = slot_map
            .stack_index(slot)
            .expect("missing stack index for non-pinned runtime slot");
        let disp = stack_slot_disp(stack_index);
        self.code.extend_from_slice(&[0x48, 0xC1]); // REX.W + grp2 imm8
        if i8::try_from(disp).is_ok() {
            self.code.push(0x40 | (4 << 3) | 0x05); // mod=01, /4, rm=rbp
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x80 | (4 << 3) | 0x05); // mod=10, /4, rm=rbp
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
        self.code.push(shift);
    }

    fn emit_shr_slot_mem_imm8(&mut self, slot: usize, shift: u8, slot_map: &RuntimeSlotMap) {
        // C1 /5 r/m64, imm8
        let stack_index = slot_map
            .stack_index(slot)
            .expect("missing stack index for non-pinned runtime slot");
        let disp = stack_slot_disp(stack_index);
        self.code.extend_from_slice(&[0x48, 0xC1]); // REX.W + grp2 imm8
        if i8::try_from(disp).is_ok() {
            self.code.push(0x40 | (5 << 3) | 0x05); // mod=01, /5, rm=rbp
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x80 | (5 << 3) | 0x05); // mod=10, /5, rm=rbp
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
        self.code.push(shift);
    }

    fn emit_sar_slot_mem_imm8(&mut self, slot: usize, shift: u8, slot_map: &RuntimeSlotMap) {
        // C1 /7 r/m64, imm8
        let stack_index = slot_map
            .stack_index(slot)
            .expect("missing stack index for non-pinned runtime slot");
        let disp = stack_slot_disp(stack_index);
        self.code.extend_from_slice(&[0x48, 0xC1]); // REX.W + grp2 imm8
        if i8::try_from(disp).is_ok() {
            self.code.push(0x40 | (7 << 3) | 0x05); // mod=01, /7, rm=rbp
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x80 | (7 << 3) | 0x05); // mod=10, /7, rm=rbp
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
        self.code.push(shift);
    }

    fn emit_mov_reg_reg(&mut self, dst: u8, src: u8) {
        if dst == src {
            return;
        }
        // mov dst, src
        let mut rex = 0x48u8; // REX.W
        if src >= 8 {
            rex |= 0x04; // REX.R
        }
        if dst >= 8 {
            rex |= 0x01; // REX.B
        }
        self.code.push(rex);
        self.code.push(0x89);
        self.code.push(0xC0 | ((src & 0x7) << 3) | (dst & 0x7));
    }

    fn emit_lea_rdi_rbp_disp(&mut self, disp: i32) {
        if i8::try_from(disp).is_ok() {
            self.code
                .extend_from_slice(&[0x48, 0x8D, 0x7D, disp as i8 as u8]); // lea rdi, [rbp+disp8]
        } else {
            self.code.extend_from_slice(&[0x48, 0x8D, 0xBD]); // lea rdi, [rbp+disp32]
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_mov_rbp_disp_rax(&mut self, disp: i32) {
        if i8::try_from(disp).is_ok() {
            self.code.extend_from_slice(&[0x48, 0x89, 0x45, disp as i8 as u8]);
        } else {
            self.code.extend_from_slice(&[0x48, 0x89, 0x85]);
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_load_rbp_disp_to_reg(&mut self, reg: u8, disp: i32) {
        let mut rex = 0x48u8;
        if reg >= 8 {
            rex |= 0x04;
        }
        self.code.push(rex);
        self.code.push(0x8B);
        if i8::try_from(disp).is_ok() {
            self.code.push(0x40 | ((reg & 0x7) << 3) | 0x05);
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x80 | ((reg & 0x7) << 3) | 0x05);
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_store_reg_to_rbp_disp(&mut self, disp: i32, reg: u8) {
        let mut rex = 0x48u8;
        if reg >= 8 {
            rex |= 0x04;
        }
        self.code.push(rex);
        self.code.push(0x89);
        if i8::try_from(disp).is_ok() {
            self.code.push(0x40 | ((reg & 0x7) << 3) | 0x05);
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x80 | ((reg & 0x7) << 3) | 0x05);
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_cmp_reg_rbp_disp(&mut self, reg: u8, disp: i32) {
        let mut rex = 0x48u8;
        if reg >= 8 {
            rex |= 0x04;
        }
        self.code.push(rex);
        self.code.push(0x3B);
        if i8::try_from(disp).is_ok() {
            self.code.push(0x40 | ((reg & 0x7) << 3) | 0x05);
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x80 | ((reg & 0x7) << 3) | 0x05);
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_inc_qword_rbp_disp(&mut self, disp: i32) {
        if i8::try_from(disp).is_ok() {
            self.code.extend_from_slice(&[0x48, 0xFF, 0x45, disp as i8 as u8]);
        } else {
            self.code.extend_from_slice(&[0x48, 0xFF, 0x85]);
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_raw_profile_from_frame(&mut self, disp: i32, bytes: usize) {
        self.emit_mov_rdi_imm(2); // stderr keeps program stdout independent
        self.emit_lea_rax_rbp_disp(disp);
        self.emit_mov_reg_reg(6, 0); // rsi = profile record
        self.emit_mov_rdx_imm(bytes as u64);
        self.emit_mov_rax_imm(1);
        self.emit_kernel_call();
    }

    fn emit_lea_rax_rbp_disp(&mut self, disp: i32) {
        if i8::try_from(disp).is_ok() {
            self.code
                .extend_from_slice(&[0x48, 0x8D, 0x45, disp as i8 as u8]); // lea rax, [rbp+disp8]
        } else {
            self.code.extend_from_slice(&[0x48, 0x8D, 0x85]); // lea rax, [rbp+disp32]
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_movups_rdi_disp_xmm0(&mut self, disp: i32) {
        self.code.extend_from_slice(&[0x0F, 0x11]); // movups [rdi+disp], xmm0
        if disp == 0 {
            self.code.push(0x07); // mod=00, reg=xmm0, rm=rdi
        } else if i8::try_from(disp).is_ok() {
            self.code.push(0x47); // mod=01, reg=xmm0, rm=rdi
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x87); // mod=10, reg=xmm0, rm=rdi
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_mov_rdi_disp_rax(&mut self, disp: i32) {
        self.code.extend_from_slice(&[0x48, 0x89]); // mov [rdi+disp], rax
        if disp == 0 {
            self.code.push(0x07); // mod=00, reg=rax, rm=rdi
        } else if i8::try_from(disp).is_ok() {
            self.code.push(0x47); // mod=01, reg=rax, rm=rdi
            self.code.push(disp as i8 as u8);
        } else {
            self.code.push(0x87); // mod=10, reg=rax, rm=rdi
            self.code.extend_from_slice(&disp.to_le_bytes());
        }
    }

    fn emit_sub_rsp_imm32(&mut self, imm32: i32) {
        self.code.extend_from_slice(&[0x48, 0x81, 0xEC]); // sub rsp, imm32
        self.code.extend_from_slice(&imm32.to_le_bytes());
    }

    fn emit_add_rsp_imm32(&mut self, imm32: i32) {
        self.code.extend_from_slice(&[0x48, 0x81, 0xC4]); // add rsp, imm32
        self.code.extend_from_slice(&imm32.to_le_bytes());
    }

    fn emit_mov_rsp_disp_from_rax(&mut self, disp: i32) {
        self.code.extend_from_slice(&[0x48, 0x89, 0x84, 0x24]); // mov [rsp+disp32], rax
        self.code.extend_from_slice(&disp.to_le_bytes());
    }

    fn emit_mov_rax_from_rsp_disp(&mut self, disp: i32) {
        self.code.extend_from_slice(&[0x48, 0x8B, 0x84, 0x24]); // mov rax, [rsp+disp32]
        self.code.extend_from_slice(&disp.to_le_bytes());
    }

    fn emit_mov_rdx_from_rsp_disp(&mut self, disp: i32) {
        self.code.extend_from_slice(&[0x48, 0x8B, 0x94, 0x24]); // mov rdx, [rsp+disp32]
        self.code.extend_from_slice(&disp.to_le_bytes());
    }

    fn emit_mov_dword_rsp_disp_imm32(&mut self, disp: i32, imm: i32) {
        self.code.extend_from_slice(&[0xC7, 0x84, 0x24]); // mov dword [rsp+disp32], imm32
        self.code.extend_from_slice(&disp.to_le_bytes());
        self.code.extend_from_slice(&imm.to_le_bytes());
    }

    fn emit_mov_eax_from_rsp_disp(&mut self, disp: i32) {
        self.code.push(0x8B);
        self.code.push(0x84);
        self.code.push(0x24);
        self.code.extend_from_slice(&disp.to_le_bytes());
    }

    fn emit_add_eax_from_rsp_disp(&mut self, disp: i32) {
        self.code.push(0x03);
        self.code.push(0x84);
        self.code.push(0x24);
        self.code.extend_from_slice(&disp.to_le_bytes());
    }

    fn emit_mov_dword_rsp_disp_from_ecx(&mut self, disp: i32) {
        self.code.extend_from_slice(&[0x89, 0x8C, 0x24]); // mov [rsp+disp32], ecx
        self.code.extend_from_slice(&disp.to_le_bytes());
    }

    fn emit_add_ecx_eax(&mut self) {
        self.code.extend_from_slice(&[0x01, 0xC1]); // add ecx, eax
    }

    fn emit_inc_dword_rsp_rax4_disp(&mut self, disp: i32) {
        self.code.extend_from_slice(&[0xFF, 0x84, 0x84]); // inc dword [rsp + rax*4 + disp32]
        self.code.extend_from_slice(&disp.to_le_bytes());
    }

    fn emit_mov_ecx_from_rsp_rax4_disp(&mut self, disp: i32) {
        self.code.extend_from_slice(&[0x8B, 0x8C, 0x84]); // mov ecx, [rsp + rax*4 + disp32]
        self.code.extend_from_slice(&disp.to_le_bytes());
    }

    fn emit_mov_rsp_rcx8_disp_from_rdx(&mut self, disp: i32) {
        self.code
            .extend_from_slice(&[0x48, 0x89, 0x94, 0xCC]); // mov [rsp + rcx*8 + disp32], rdx
        self.code.extend_from_slice(&disp.to_le_bytes());
    }

    fn emit_exit_with_rax_or_zero(&mut self, exit_with_state: bool) {
        self.emit_exit_with_rax_or_zero_impl(exit_with_state, true);
    }

    fn emit_exit_with_rax_or_zero_impl(&mut self, exit_with_state: bool, report: bool) {
        if self.options.emit_full_checksum && report {
            if !exit_with_state {
                self.emit_mov_rax_imm(0);
            }
            self.emit_raw_checksum_from_rax();
        }
        if exit_with_state {
            // mov rdi, rax
            self.code.extend_from_slice(&[0x48, 0x89, 0xC7]);
        } else {
            self.emit_mov_rdi_imm(0);
        }
        self.emit_mov_rax_imm(self.runtime.syscalls.exit); // SYS_exit
        self.emit_kernel_call();
    }

    fn emit_exit_with_rax_or_mask(&mut self, exit_with_state: bool, exit_mask: Option<u64>) {
        if exit_with_state {
            if self.options.emit_full_checksum {
                self.emit_raw_checksum_from_rax();
            }
            if let Some(mask) = exit_mask {
                self.emit_and_rax_imm(mask);
            }
            self.emit_exit_with_rax_or_zero_impl(true, false);
        } else {
            self.emit_exit_with_rax_or_zero(false);
        }
    }

    fn emit_raw_checksum_from_rax(&mut self) {
        // Preserve the result across write(1, rsp, 8).  This is used only by
        // explicit verification builds and therefore adds no timed-path cost.
        self.code.push(0x50); // push rax
        self.emit_mov_rax_imm(self.runtime.syscalls.write);
        self.emit_mov_rdi_imm(1); // stdout
        self.code.extend_from_slice(&[0x48, 0x89, 0xE6]); // mov rsi, rsp
        self.emit_mov_rdx_imm(8);
        self.emit_kernel_call(); // syscall
        self.code.push(0x58); // pop rax
    }

    fn emit_and_rax_imm(&mut self, imm: u64) {
        // Peephole: and rax, 0xFFFFFFFF → mov eax, eax (2 bytes vs 13 bytes)
        // 32-bit mov zero-extends to 64-bit, same semantic effect.
        if imm == 0xFFFFFFFF {
            self.code.extend_from_slice(&[0x89, 0xC0]); // mov eax, eax
            return;
        }
        // Peephole: and rax, 0xFFFF → movzx eax, ax (3 bytes vs 13 bytes)
        if imm == 0xFFFF {
            self.code.extend_from_slice(&[0x0F, 0xB7, 0xC0]); // movzx eax, ax
            return;
        }
        // Peephole: and rax, 0xFF → movzx eax, al (3 bytes vs 13 bytes)
        if imm == 0xFF {
            self.code.extend_from_slice(&[0x0F, 0xB6, 0xC0]); // movzx eax, al
            return;
        }
        if let Some(imm32) = imm32_sign_extended(imm) {
            self.emit_alu_reg_imm(4, 0, imm32.to_le_bytes()); // and rax, imm32
        } else {
            self.emit_mov_reg_imm64(8, imm);
            self.emit_binop_reg_reg_in_place(RuntimeBinOp::BitAnd, 0, 8);
        }
    }

    fn prepare_affine_step(
        &mut self,
        mul: u64,
        add: u64,
        mul_reg: GpReg,
        add_reg: GpReg,
    ) -> AffinePlan {
        let mul = match mul {
            0 => MulPlan::Zero,
            1 => MulPlan::One,
            _ => match imm32_non_negative(mul) {
                Some(v) => MulPlan::Imm32(v),
                None => {
                    self.emit_mov_reg_imm(mul_reg, mul);
                    MulPlan::Reg(mul_reg)
                }
            },
        };

        let add = if add == 0 {
            AddPlan::Zero
        } else {
            match imm32_non_negative(add) {
                Some(v) => AddPlan::Imm32(v),
                None => {
                    self.emit_mov_reg_imm(add_reg, add);
                    AddPlan::Reg(add_reg)
                }
            }
        };

        AffinePlan { mul, add }
    }

    fn emit_affine_step(&mut self, plan: &AffinePlan) {
        match plan.mul {
            MulPlan::Zero => {
                // xor rax, rax
                self.code.extend_from_slice(&[0x48, 0x31, 0xC0]);
            }
            MulPlan::One => {}
            MulPlan::Imm32(value) => {
                // imul rax, rax, imm32 (signed imm32; only used for non-negative <= i32::MAX)
                self.code.extend_from_slice(&[0x48, 0x69, 0xC0]);
                self.code.extend_from_slice(&value.to_le_bytes());
            }
            MulPlan::Reg(reg) => self.emit_imul_rax_reg(reg),
        }

        match plan.add {
            AddPlan::Zero => {}
            AddPlan::Imm32(value) => {
                // add rax, imm32 (signed imm32; only used for non-negative <= i32::MAX)
                self.code.extend_from_slice(&[0x48, 0x05]);
                self.code.extend_from_slice(&value.to_le_bytes());
            }
            AddPlan::Reg(reg) => self.emit_add_rax_reg(reg),
        }
    }

    fn emit_mov_reg_imm(&mut self, reg: GpReg, value: u64) {
        self.code
            .extend_from_slice(&[0x49, 0xB8 + gp_reg_low3(reg)]);
        self.code.extend_from_slice(&value.to_le_bytes());
    }

    fn emit_imul_rax_reg(&mut self, reg: GpReg) {
        // imul rax, reg
        self.code
            .extend_from_slice(&[0x49, 0x0F, 0xAF, 0xC0 | gp_reg_low3(reg)]);
    }

    fn emit_add_rax_reg(&mut self, reg: GpReg) {
        // add rax, reg
        self.code
            .extend_from_slice(&[0x4C, 0x01, 0xC0 | (gp_reg_low3(reg) << 3)]);
    }

    fn emit_touch_step(&mut self, plan: &AffinePlan, wrap16_index: bool) {
        self.emit_affine_step(plan);

        // [rbx + r11] = al
        self.code.extend_from_slice(&[0x42, 0x88, 0x04, 0x1B]);
        if wrap16_index {
            // For 64KiB rings, 16-bit increment wraps naturally and avoids a mask op.
            self.code.extend_from_slice(&[0x66, 0x41, 0xFF, 0xC3]); // inc r11w
        } else {
            // r11 = (r11 + 1) & mask
            self.code.extend_from_slice(&[0x49, 0xFF, 0xC3]); // inc r11
            self.code.extend_from_slice(&[0x4D, 0x21, 0xCB]); // and r11, r9
        }
    }

    fn emit_unpredictable_branch_lcg_step_select_coeff(&mut self, state_mask: u64) {
        if state_mask == 0xFFFF_FFFF {
            // Keep the state and selected coefficients in 32-bit registers.
            // Every 32-bit destination write zero-extends, implementing the
            // source-level mask without a separate AND instruction.
            self.code.extend_from_slice(&[0x48, 0x39, 0xD0]); // cmp rax, rdx
            self.code.extend_from_slice(&[0x44, 0x89, 0xCB]); // mov ebx, r9d
            self.code.extend_from_slice(&[0x41, 0x0F, 0x42, 0xD8]); // cmovb ebx, r8d
            self.code.extend_from_slice(&[0x44, 0x89, 0xDE]); // mov esi, r11d
            self.code.extend_from_slice(&[0x41, 0x0F, 0x42, 0xF2]); // cmovb esi, r10d
            self.code.extend_from_slice(&[0x0F, 0xAF, 0xC3]); // imul eax, ebx
            self.code.extend_from_slice(&[0x01, 0xF0]); // add eax, esi
            return;
        }

        // cmp state, threshold
        self.code.extend_from_slice(&[0x48, 0x39, 0xD0]); // cmp rax, rdx
        // selected_mul = else_mul; if state<threshold -> then_mul
        self.code.extend_from_slice(&[0x4C, 0x89, 0xCB]); // mov rbx, r9
        self.code.extend_from_slice(&[0x49, 0x0F, 0x42, 0xD8]); // cmovb rbx, r8
        // selected_add = else_add; if state<threshold -> then_add
        self.code.extend_from_slice(&[0x4C, 0x89, 0xDE]); // mov rsi, r11
        self.code.extend_from_slice(&[0x49, 0x0F, 0x42, 0xF2]); // cmovb rsi, r10
        // state = state * selected_mul + selected_add
        self.code.extend_from_slice(&[0x48, 0x0F, 0xAF, 0xC3]); // imul rax, rbx
        self.code.extend_from_slice(&[0x48, 0x01, 0xF0]); // add rax, rsi
        if state_mask != u64::MAX {
            self.emit_and_rax_imm(state_mask);
        }
    }

    fn prepare_coeff_plan(&mut self, coeff: u64, reg: GpReg) -> CoeffPlan {
        if coeff == 0 {
            CoeffPlan::Zero
        } else if coeff == 1 {
            CoeffPlan::One
        } else if let Some(shift) = pow2_shift_u64(coeff) {
            CoeffPlan::Pow2(shift)
        } else if let Some(v) = imm32_non_negative(coeff) {
            CoeffPlan::Imm32(v)
        } else {
            self.emit_mov_reg_imm(reg, coeff);
            CoeffPlan::Reg(reg)
        }
    }

    fn emit_add_index_term(&mut self, plan: &CoeffPlan) {
        match plan {
            CoeffPlan::Zero => {}
            CoeffPlan::One => {
                self.code.extend_from_slice(&[0x48, 0x01, 0xD0]); // add rax, rdx
            }
            CoeffPlan::Pow2(shift) => {
                self.code.extend_from_slice(&[0x49, 0x89, 0xD3]); // mov r11, rdx
                self.code.extend_from_slice(&[0x49, 0xC1, 0xE3, *shift]); // shl r11, imm8
                self.emit_add_rax_reg(GpReg::R11);
            }
            CoeffPlan::Imm32(value) => {
                self.code.extend_from_slice(&[0x49, 0x89, 0xD3]); // mov r11, rdx
                self.code.extend_from_slice(&[0x4D, 0x69, 0xDB]); // imul r11, r11, imm32
                self.code.extend_from_slice(&value.to_le_bytes());
                self.emit_add_rax_reg(GpReg::R11);
            }
            CoeffPlan::Reg(reg) => {
                self.code.extend_from_slice(&[0x49, 0x89, 0xD3]); // mov r11, rdx
                // imul r11, reg
                self.code.push(0x4D); // REX.W + R + B
                self.code.push(0x0F);
                self.code.push(0xAF);
                self.code.push(0xC0 | (0x03 << 3) | gp_reg_low3(*reg));
                self.emit_add_rax_reg(GpReg::R11);
            }
        }
    }

    fn emit_dual_state_step(&mut self, branchless: bool) {
        if branchless {
            // then_a in r9: a + i + 1
            self.code.extend_from_slice(&[0x4C, 0x8D, 0x4C, 0x10, 0x01]); // lea r9, [rax + rdx + 1]

            // then_b in r11: b * 4
            self.emit_mov_reg_reg(11, 3);
            self.emit_shl_reg_imm8(11, 2);

            self.emit_cmp_reg_reg(0, 3); // cmp original a, b

            // Default to else values directly in-place; LEA keeps cmp flags intact.
            self.code.extend_from_slice(&[0x48, 0x8D, 0x40, 0x03]); // lea rax, [rax + 3]
            self.code.extend_from_slice(&[0x48, 0x8D, 0x5C, 0x03, 0x02]); // lea rbx, [rbx + rax + 2]

            // Override with then values when a < b.
            self.emit_cmovb_reg_reg(0, 9); // if (a_orig < b_orig) a = then_a
            self.emit_cmovb_reg_reg(3, 11); // if (a_orig < b_orig) b = then_b
        } else {
            self.emit_cmp_reg_reg(0, 3); // cmp a, b
            self.code.extend_from_slice(&[0x0F, 0x82]); // jb then_path
            let jb_then_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());

            // else-path: b = b + old_a + 5 ; a = old_a + 3
            // Emitting b-update first removes dependency on updated rax.
            self.code.extend_from_slice(&[0x48, 0x8D, 0x5C, 0x03, 0x05]); // lea rbx, [rbx + rax + 5]
            self.code.extend_from_slice(&[0x48, 0x8D, 0x40, 0x03]); // lea rax, [rax + 3]
            self.code.push(0xE9); // jmp after_if (cold path)
            let jmp_after_if_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());

            let then_path = self.code.len();
            // then-path: a = a + i + 1 ; b <<= 2
            self.code.extend_from_slice(&[0x48, 0x01, 0xD0]); // add rax, rdx
            self.code.extend_from_slice(&[0x48, 0xFF, 0xC0]); // inc rax
            self.emit_shl_reg_imm8(3, 2);

            let after_if = self.code.len();
            patch_rel32(&mut self.code, jb_then_pos, then_path);
            patch_rel32(&mut self.code, jmp_after_if_pos, after_if);
        }
    }

    fn emit_dual_state_iterations(&mut self, iterations: u64, branchless: bool) {
        let (unroll, _step) = if branchless { (4, 4) } else { (8, 8) };
        let blocks = iterations / unroll;
        let tail = iterations % unroll;

        if blocks > 0 {
            self.emit_mov_rcx_imm(blocks);
            let loop_start = self.code.len();
            for _ in 0..unroll {
                self.emit_dual_state_step(branchless);
                self.code.extend_from_slice(&[0x48, 0xFF, 0xC2]); // inc rdx
            }
            self.code.extend_from_slice(&[0x48, 0xFF, 0xC9]); // dec rcx
            self.code.extend_from_slice(&[0x0F, 0x85]); // jnz rel32
            let jnz_loop_pos = self.code.len();
            self.code.extend_from_slice(&0_i32.to_le_bytes());
            patch_rel32(&mut self.code, jnz_loop_pos, loop_start);
        }

        for _ in 0..tail {
            self.emit_dual_state_step(branchless);
            self.code.extend_from_slice(&[0x48, 0xFF, 0xC2]); // inc rdx
        }
    }

    fn emit_optimal_mul_imm64(&mut self, reg: u8, imm: u64) -> bool {
        if imm == 0 {
            self.emit_xor_reg_reg(reg, reg);
            return true;
        }
        if imm == 1 {
            return true;
        }
        if let Some(shift) = pow2_shift_u64(imm) {
            self.emit_shl_reg_imm8(reg, shift);
            return true;
        }

        if (reg & 7) != 4 && (reg & 7) != 5 { // RSP and RBP cannot be SIB indices without careful scaling
            match imm {
                3 => { self.emit_lea_mult_add(reg, reg, 2); return true; }
                5 => { self.emit_lea_mult_add(reg, reg, 4); return true; }
                9 => { self.emit_lea_mult_add(reg, reg, 8); return true; }
                _ => {}
            }
        }

        // Factorization: imm = a * b
        for factor in [3, 5, 9, 2, 4, 8] {
            if imm > factor && imm % factor == 0 {
                let remaining = imm / factor;
                if self.emit_optimal_mul_imm64(reg, remaining) {
                    self.emit_optimal_mul_imm64(reg, factor);
                    return true;
                }
            }
        }

        // (x << n) - x
        if imm > 1 && (imm + 1).is_power_of_two() {
            let shift = (imm + 1).trailing_zeros() as u8;
            let scratch = if reg != 0 { 0 } else { 1 };
            self.emit_mov_reg_reg(scratch, reg);
            self.emit_shl_reg_imm8(reg, shift);
            self.emit_sub_reg_reg(reg, scratch);
            return true;
        }

        // (x << n) + x
        if imm > 1 && (imm - 1).is_power_of_two() {
            let shift = (imm - 1).trailing_zeros() as u8;
            let scratch = if reg != 0 { 0 } else { 1 };
            self.emit_mov_reg_reg(scratch, reg);
            self.emit_shl_reg_imm8(reg, shift);
            self.emit_add_reg_reg(reg, scratch);
            return true;
        }

        false
    }

    fn affine_pow_u64(mut mul: u64, mut add: u64, mut exp: u64) -> (u64, u64) {
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

    fn emit_affine_step_to_reg(&mut self, reg: u8, plan: &AffinePlan) {
        match plan.mul {
            MulPlan::Zero => self.emit_xor_reg_reg(reg, reg),
            MulPlan::One => {}
            MulPlan::Imm32(v) => self.emit_imul_reg_reg_imm32(reg, reg, v),
            MulPlan::Reg(r) => self.emit_imul_reg_reg(reg, r as u8),
        }
        match plan.add {
            AddPlan::Zero => {}
            AddPlan::Imm32(v) => self.emit_add_reg_imm32(reg, v),
            AddPlan::Reg(r) => self.emit_add_reg_reg(reg, r as u8),
        }
    }

    fn emit_store_reg_to_ring(&mut self, reg: u8, offset: i32, _wrap16: bool) {
        // [rbx + r11 + offset]
        let mut rex = 0x48u8;
        if reg >= 8 { rex |= 0x04; } // REX.R
        rex |= 0x02; // REX.X (r11 index)
        self.code.push(rex);
        self.code.push(0x89);
        self.code.push(0x84 | ((reg & 7) << 3));
        self.code.push(0x1B); // SIB: scale=1 (index=r11, base=rbx)
        self.code.extend_from_slice(&offset.to_le_bytes());
    }

    fn emit_add_r11_imm(&mut self, val: i32, wrap16: bool) {
        if wrap16 {
            if let Ok(v8) = i8::try_from(val) {
                self.code.extend_from_slice(&[0x66, 0x41, 0x83, 0xC3, v8 as u8]); // add r11w, imm8
                return;
            }
            if let Ok(v16) = i16::try_from(val) {
                self.code.extend_from_slice(&[0x66, 0x41, 0x81, 0xC3]); // add r11w, imm16
                self.code.extend_from_slice(&v16.to_le_bytes());
                return;
            }
        }
        let rex = 0x49u8; // REX.W + REX.B (r11)
        self.code.push(rex);
        if let Ok(v8) = i8::try_from(val) {
            self.code.push(0x83);
            self.code.push(0xC3);
            self.code.push(v8 as u8);
        } else {
            self.code.push(0x81);
            self.code.push(0xC3);
            self.code.extend_from_slice(&val.to_le_bytes());
        }
        if wrap16 {
            self.code.extend_from_slice(&[0x41, 0x81, 0xE3, 0xFF, 0xFF, 0x00, 0x00]); // and r11, 0xFFFF
        }
    }

    fn emit_add_reg_imm32(&mut self, reg: u8, imm: i32) {
        let mut rex = 0x48u8;
        if reg >= 8 { rex |= 0x01; }
        self.code.push(rex);
        if let Ok(v8) = i8::try_from(imm) {
            self.code.push(0x83);
            self.code.push(0xC0 | (reg & 7));
            self.code.push(v8 as u8);
        } else {
            self.code.push(0x81);
            self.code.push(0xC0 | (reg & 7));
            self.code.extend_from_slice(&imm.to_le_bytes());
        }
    }

    fn emit_add_reg32_imm32(&mut self, reg: u8, imm: i32) {
        let mut rex = 0x40u8;
        if reg >= 8 {
            rex |= 0x01;
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        if let Ok(v8) = i8::try_from(imm) {
            self.code.push(0x83);
            self.code.push(0xC0 | (reg & 7));
            self.code.push(v8 as u8);
        } else {
            self.code.push(0x81);
            self.code.push(0xC0 | (reg & 7));
            self.code.extend_from_slice(&imm.to_le_bytes());
        }
    }

    fn emit_imul_reg32_reg32_imm32(&mut self, dst: u8, src: u8, imm: i32) {
        let mut rex = 0x40u8;
        if dst >= 8 {
            rex |= 0x04;
        }
        if src >= 8 {
            rex |= 0x01;
        }
        if rex != 0x40 {
            self.code.push(rex);
        }
        self.code.push(0x69);
        self.code
            .push(0xC0 | ((dst & 7) << 3) | (src & 7));
        self.code.extend_from_slice(&imm.to_le_bytes());
    }

    fn emit_imul_reg_reg_imm32(&mut self, dst: u8, src: u8, imm: i32) {
        let mut rex = 0x48u8;
        if dst >= 8 { rex |= 0x04; }
        if src >= 8 { rex |= 0x01; }
        self.code.push(rex);
        self.code.push(0x69);
        self.code.push(0xC0 | ((dst & 7) << 3) | (src & 7));
        self.code.extend_from_slice(&imm.to_le_bytes());
    }

    fn emit_imul_reg_reg(&mut self, dst: u8, src: u8) {
        let mut rex = 0x48u8;
        if dst >= 8 { rex |= 0x04; }
        if src >= 8 { rex |= 0x01; }
        self.code.push(rex);
        self.code.extend_from_slice(&[0x0F, 0xAF]);
        self.code.push(0xC0 | ((dst & 7) << 3) | (src & 7));
    }

    pub fn finalize(mut self) -> Vec<u8> {
        if self.runtime.kernel_call_style == KernelCallStyle::WindowsImport {
            let dispatcher = self.code.len();
            self.code
                .extend_from_slice(&windows::build_dispatcher(self.runtime));
            for patch in &self.kernel_call_patches {
                let displacement = i32::try_from(dispatcher as i64 - (*patch + 4) as i64)
                    .expect("Windows runtime dispatcher exceeds rel32");
                self.code[*patch..*patch + 4].copy_from_slice(&displacement.to_le_bytes());
            }
        }
        let code_len = self.code.len();
        for patch in &self.patches {
            let rip_after_disp = patch.disp_pos + 4;
            let msg_addr = code_len + patch.data_offset;
            let disp = (msg_addr as i64) - (rip_after_disp as i64);
            let disp = i32::try_from(disp).expect("message offset exceeds rel32 range");
            self.code[patch.disp_pos..patch.disp_pos + 4].copy_from_slice(&disp.to_le_bytes());
        }

        self.code.extend_from_slice(&self.data);
        self.code
    }
}
