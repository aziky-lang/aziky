//! Core runtime-native builder, ownership, layout, enum, and call ABI support.

use super::*;

impl<'a> RuntimeGenericBuilder<'a> {
    pub(super) fn new(
        functions: &'a HashMap<String, Function>,
        struct_layouts: &'a HashMap<String, Vec<LayoutField>>,
        enums: &'a HashMap<String, EnumDef>,
    ) -> Self {
        Self {
            functions,
            struct_layouts,
            enums,
            scopes: RuntimeGenericScopeStack::new(),
            slots: 0,
            instrs: Vec::new(),
            pending_functions: VecDeque::new(),
            queued_functions: HashSet::new(),
            lowered_functions: HashSet::new(),
            function_entries: HashMap::new(),
            function_param_slots: HashMap::new(),
            function_return_slots: HashMap::new(),
            active_function: None,
            call_patches: Vec::new(),
            loop_frames: Vec::new(),
            unchecked_array_loop_accesses: Vec::new(),
            slot_unsigned_upper_bounds: HashMap::new(),
        }
    }

    pub(super) fn alloc_slot(&mut self) -> usize {
        let slot = self.slots;
        self.slots += 1;
        slot
    }

    pub(super) fn infer_expr_runtime_type_name(
        &self,
        expr: &Expr,
    ) -> RuntimeGenericLowerResult<TypeName> {
        if matches!(expr, Expr::Bool { .. }) {
            return Ok(TypeName::Bool);
        }
        if let Expr::Ident { name, .. } = expr
            && let Some(binding) = self.scopes.get(name)
        {
            match binding {
                RuntimeGenericBinding::OwnedListScalar { elem_ty, .. } => {
                    return Ok(TypeName::List {
                        elem: Box::new(runtime_scalar_type_name(*elem_ty)),
                    });
                }
                RuntimeGenericBinding::OwnedListStruct { layout, .. } => {
                    return Ok(TypeName::List {
                        elem: Box::new(TypeName::Struct(layout.struct_name.clone())),
                    });
                }
                RuntimeGenericBinding::OwnedStruct { struct_name, .. }
                | RuntimeGenericBinding::StructSlots { struct_name, .. } => {
                    return Ok(TypeName::Struct(struct_name.clone()));
                }
                RuntimeGenericBinding::OwnedString { is_path: true, .. } => {
                    return Ok(TypeName::Path);
                }
                RuntimeGenericBinding::OwnedString { .. } => return Ok(TypeName::String),
                RuntimeGenericBinding::OwnedFile { .. } => return Ok(TypeName::File),
                RuntimeGenericBinding::OwnedThread { .. } => return Ok(TypeName::Thread),
                RuntimeGenericBinding::OwnedChannel { .. } => {
                    return Ok(TypeName::Applied {
                        name: "Channel".to_string(),
                        args: vec![TypeName::Int {
                            signed: false,
                            bits: 64,
                        }],
                    });
                }
                RuntimeGenericBinding::OwnedSender { .. } => {
                    return Ok(TypeName::Applied {
                        name: "Sender".to_string(),
                        args: vec![TypeName::Int {
                            signed: false,
                            bits: 64,
                        }],
                    });
                }
                RuntimeGenericBinding::OwnedReceiver { .. } => {
                    return Ok(TypeName::Applied {
                        name: "Receiver".to_string(),
                        args: vec![TypeName::Int {
                            signed: false,
                            bits: 64,
                        }],
                    });
                }
                RuntimeGenericBinding::OwnedMap { layout, .. } => {
                    return Ok(TypeName::Map {
                        key: Box::new(runtime_scalar_type_name(layout.key_ty)),
                        value: Box::new(runtime_scalar_type_name(layout.value_ty)),
                    });
                }
                _ => {}
            }
        }
        self.infer_expr_scalar_type(expr)
            .map(runtime_scalar_type_name)
    }

    pub(super) fn emit(&mut self, instr: RuntimeInstr) -> usize {
        self.instrs.push(instr);
        self.instrs.len() - 1
    }

    pub(super) fn emit_entry_stack_pointer(&mut self) -> usize {
        let slot = self.alloc_slot();
        self.emit(RuntimeInstr::LoadSeed {
            dst: slot,
            kind: RuntimeLoadKind::EntryStackPointer,
            input: None,
        });
        slot
    }

    pub(super) fn bind_owned_c_string(
        &mut self,
        name: &str,
        ptr_slot: usize,
        mutable: bool,
    ) -> RuntimeGenericLowerResult<()> {
        let len_slot = self.alloc_slot();
        self.emit(RuntimeInstr::Mov {
            dst: len_slot,
            src: RuntimeOperand::Imm(0),
        });
        let byte_slot = self.alloc_slot();
        let scan = self.instrs.len();
        self.emit(RuntimeInstr::HeapLoadInt {
            dst: byte_slot,
            ptr: RuntimeOperand::Slot(ptr_slot),
            index: RuntimeOperand::Slot(len_slot),
            bytes: 1,
        });
        let done = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Ne,
            lhs: RuntimeOperand::Slot(byte_slot),
            rhs: RuntimeOperand::Imm(0),
            target: usize::MAX,
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: len_slot,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Imm(1),
        });
        self.emit(RuntimeInstr::Jump { target: scan });
        let scanned = self.instrs.len();
        self.patch_target(done, scanned)?;

        let capacity_slot = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: capacity_slot,
            op: RuntimeBinOp::Add,
            lhs: RuntimeOperand::Slot(len_slot),
            rhs: RuntimeOperand::Imm(1),
        });
        self.emit_guard_failure_exit(
            RuntimeCmpOp::GtUnsigned,
            RuntimeOperand::Slot(capacity_slot),
            RuntimeOperand::Slot(len_slot),
            101,
        )?;
        let owned_ptr = self.alloc_slot();
        self.emit(RuntimeInstr::Alloc {
            dst: owned_ptr,
            size: RuntimeOperand::Slot(capacity_slot),
        });
        self.emit_guard_failure_exit(
            RuntimeCmpOp::Ne,
            RuntimeOperand::Slot(owned_ptr),
            RuntimeOperand::Imm(0),
            101,
        )?;
        self.emit(RuntimeInstr::HeapCopy {
            dst_ptr: RuntimeOperand::Slot(owned_ptr),
            src_ptr: RuntimeOperand::Slot(ptr_slot),
            bytes: RuntimeOperand::Slot(len_slot),
        });
        self.emit(RuntimeInstr::HeapStoreInt {
            ptr: RuntimeOperand::Slot(owned_ptr),
            index: RuntimeOperand::Slot(len_slot),
            src: RuntimeOperand::Imm(0),
            bytes: 1,
        });
        self.scopes.insert(
            name.to_string(),
            RuntimeGenericBinding::OwnedString {
                ptr_slot: owned_ptr,
                len_slot,
                capacity_slot,
                allocation_bytes_slot: capacity_slot,
                mutable,
                is_path: false,
            },
        );
        Ok(())
    }

    pub(super) fn emit_owned_cleanup_from(&mut self, depth: usize) {
        for (handle_slot, sender) in self.scopes.owned_channel_endpoints_from(depth) {
            self.emit(RuntimeInstr::ChannelClose {
                handle: RuntimeOperand::Slot(handle_slot),
                sender,
            });
        }
        for handle_slot in self.scopes.owned_threads_from(depth) {
            let ignored = self.alloc_slot();
            self.emit(RuntimeInstr::ThreadJoin {
                dst: ignored,
                handle: RuntimeOperand::Slot(handle_slot),
            });
            self.emit(RuntimeInstr::Mov {
                dst: handle_slot,
                src: RuntimeOperand::Imm(0),
            });
        }
        for handle_slot in self.scopes.owned_channels_from(depth) {
            self.emit(RuntimeInstr::ChannelDestroy {
                handle: RuntimeOperand::Slot(handle_slot),
            });
        }
        for fd_slot in self.scopes.owned_files_from(depth) {
            self.emit(RuntimeInstr::FileClose {
                fd: RuntimeOperand::Slot(fd_slot),
            });
            self.emit(RuntimeInstr::Mov {
                dst: fd_slot,
                src: RuntimeOperand::Imm(u64::MAX),
            });
        }
        for (ptr_slot, size_slot) in self.scopes.owned_allocations_from(depth, false) {
            self.emit(RuntimeInstr::Free {
                ptr: RuntimeOperand::Slot(ptr_slot),
                size: RuntimeOperand::Slot(size_slot),
            });
            self.emit(RuntimeInstr::Mov {
                dst: ptr_slot,
                src: RuntimeOperand::Imm(0),
            });
        }
        self.emit_tagged_enum_cleanup_from(depth, false);
    }

    pub(super) fn emit_tagged_enum_cleanup_from(&mut self, depth: usize, include_borrowed: bool) {
        for (tag_slot, expected_tag, ptr_slot, size_slot) in self
            .scopes
            .tagged_enum_allocations_from(depth, include_borrowed)
        {
            let skip = self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::Eq,
                lhs: RuntimeOperand::Slot(tag_slot),
                rhs: RuntimeOperand::Imm(expected_tag),
                target: usize::MAX,
            });
            self.emit(RuntimeInstr::Free {
                ptr: RuntimeOperand::Slot(ptr_slot),
                size: RuntimeOperand::Slot(size_slot),
            });
            self.emit(RuntimeInstr::Mov {
                dst: ptr_slot,
                src: RuntimeOperand::Imm(0),
            });
            let done = self.instrs.len();
            // Cleanup targets are emitted structurally and cannot fail to patch.
            self.patch_target(skip, done)
                .expect("tagged enum cleanup jump must be patchable");
        }
    }

    pub(super) fn emit_runtime_enum_resource_cleanup(
        &mut self,
        layout: &RuntimeEnumLayout,
        tag_slot: usize,
        payload_slots: &[usize],
    ) -> RuntimeGenericLowerResult<()> {
        if payload_slots.len() < 4 && layout.owns_resources() {
            return Err(RuntimeGenericLowerError::Unsupported);
        }
        for variant in &layout.variants {
            let nested_resource = variant
                .nested
                .as_ref()
                .filter(|nested| nested.layout.owns_resources());
            if variant.resource.is_none() && nested_resource.is_none() {
                continue;
            }
            let skip = self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::Eq,
                lhs: RuntimeOperand::Slot(tag_slot),
                rhs: RuntimeOperand::Imm(variant.tag),
                target: usize::MAX,
            });
            if variant.resource.is_some() {
                self.emit(RuntimeInstr::Free {
                    ptr: RuntimeOperand::Slot(payload_slots[0]),
                    size: RuntimeOperand::Slot(payload_slots[3]),
                });
                self.emit(RuntimeInstr::Mov {
                    dst: payload_slots[0],
                    src: RuntimeOperand::Imm(0),
                });
            } else if let Some(nested) = nested_resource {
                self.emit_runtime_enum_resource_cleanup(
                    &nested.layout,
                    payload_slots[0],
                    &payload_slots[1..],
                )?;
            }
            let done = self.instrs.len();
            self.patch_target(skip, done)?;
        }
        Ok(())
    }

    pub(super) fn pop_scope_with_cleanup(&mut self) {
        let depth = self.scopes.depth().saturating_sub(1);
        self.emit_owned_cleanup_from(depth);
        self.scopes.pop();
    }

    pub(super) fn emit_owned_cleanup_all(&mut self) {
        self.emit_owned_cleanup_from(0);
    }

    pub(super) fn emit_terminal_cleanup_all(&mut self) {
        for (handle_slot, sender) in self.scopes.owned_channel_endpoints_from(0) {
            self.emit(RuntimeInstr::ChannelClose {
                handle: RuntimeOperand::Slot(handle_slot),
                sender,
            });
        }
        for handle_slot in self.scopes.owned_threads_from(0) {
            let ignored = self.alloc_slot();
            self.emit(RuntimeInstr::ThreadJoin {
                dst: ignored,
                handle: RuntimeOperand::Slot(handle_slot),
            });
            self.emit(RuntimeInstr::Mov {
                dst: handle_slot,
                src: RuntimeOperand::Imm(0),
            });
        }
        for handle_slot in self.scopes.owned_channels_from(0) {
            self.emit(RuntimeInstr::ChannelDestroy {
                handle: RuntimeOperand::Slot(handle_slot),
            });
        }
        for fd_slot in self.scopes.owned_files_from(0) {
            self.emit(RuntimeInstr::FileClose {
                fd: RuntimeOperand::Slot(fd_slot),
            });
            self.emit(RuntimeInstr::Mov {
                dst: fd_slot,
                src: RuntimeOperand::Imm(u64::MAX),
            });
        }
        for (ptr_slot, size_slot) in self.scopes.owned_allocations_from(0, true) {
            self.emit(RuntimeInstr::Free {
                ptr: RuntimeOperand::Slot(ptr_slot),
                size: RuntimeOperand::Slot(size_slot),
            });
            self.emit(RuntimeInstr::Mov {
                dst: ptr_slot,
                src: RuntimeOperand::Imm(0),
            });
        }
        self.emit_tagged_enum_cleanup_from(0, true);
    }

    pub(super) fn reject_moved_resource(
        &self,
        name: &str,
        span: Span,
    ) -> RuntimeGenericLowerResult<()> {
        if let Some(RuntimeGenericBinding::MovedResource { kind }) = self.scopes.get(name) {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!("{kind} '{name}' was moved or consumed and cannot be used again").as_str(),
                span,
            )));
        }
        Ok(())
    }

    pub(super) fn lower_file_close_owner(
        &mut self,
        owner_name: &str,
        span: Span,
    ) -> RuntimeGenericLowerResult<()> {
        self.reject_moved_resource(owner_name, span)?;
        let fd_slot = self
            .scopes
            .get_current(owner_name)
            .and_then(|binding| match binding {
                RuntimeGenericBinding::OwnedFile { fd_slot } => Some(*fd_slot),
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "close() requires a File owner in the current lexical scope",
                    span,
                ))
            })?;
        self.emit(RuntimeInstr::FileClose {
            fd: RuntimeOperand::Slot(fd_slot),
        });
        self.emit(RuntimeInstr::Mov {
            dst: fd_slot,
            src: RuntimeOperand::Imm(u64::MAX),
        });
        let consumed = self.scopes.take_current(owner_name).ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                "file ownership source disappeared during close",
                span,
            ))
        })?;
        debug_assert!(matches!(consumed, RuntimeGenericBinding::OwnedFile { .. }));
        self.scopes.insert(
            owner_name.to_string(),
            RuntimeGenericBinding::MovedResource { kind: "file owner" },
        );
        Ok(())
    }

    pub(super) fn emit_validate_file_path(
        &mut self,
        ptr_slot: usize,
        len_slot: usize,
    ) -> RuntimeGenericLowerResult<()> {
        let index_slot = self.alloc_slot();
        let byte_slot = self.alloc_slot();
        self.emit(RuntimeInstr::Mov {
            dst: index_slot,
            src: RuntimeOperand::Imm(0),
        });
        let loop_start = self.instrs.len();
        let done = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LtUnsigned,
            lhs: RuntimeOperand::Slot(index_slot),
            rhs: RuntimeOperand::Slot(len_slot),
            target: usize::MAX,
        });
        self.emit(RuntimeInstr::HeapLoadInt {
            dst: byte_slot,
            ptr: RuntimeOperand::Slot(ptr_slot),
            index: RuntimeOperand::Slot(index_slot),
            bytes: 1,
        });
        let invalid = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Ne,
            lhs: RuntimeOperand::Slot(byte_slot),
            rhs: RuntimeOperand::Imm(0),
            target: usize::MAX,
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: index_slot,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Imm(1),
        });
        self.emit(RuntimeInstr::Jump { target: loop_start });
        let invalid_target = self.instrs.len();
        self.patch_target(invalid, invalid_target)?;
        self.emit_terminal_cleanup_all();
        self.emit(RuntimeInstr::Exit {
            code: RuntimeOperand::Imm(105),
        });
        let done_target = self.instrs.len();
        self.patch_target(done, done_target)?;
        Ok(())
    }

    pub(super) fn emit_guard_failure_exit(
        &mut self,
        op: RuntimeCmpOp,
        lhs: RuntimeOperand,
        rhs: RuntimeOperand,
        code: u64,
    ) -> RuntimeGenericLowerResult<()> {
        let fail = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op,
            lhs,
            rhs,
            target: usize::MAX,
        });
        let success = self.emit(RuntimeInstr::Jump { target: usize::MAX });
        let fail_target = self.instrs.len();
        self.patch_target(fail, fail_target)?;
        self.emit_terminal_cleanup_all();
        self.emit(RuntimeInstr::Exit {
            code: RuntimeOperand::Imm(code),
        });
        let success_target = self.instrs.len();
        self.patch_target(success, success_target)
    }

    pub(super) fn emit_owned_list_scalar_push(
        &mut self,
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        value: RuntimeOperand,
        elem_ty: RuntimeScalarType,
    ) -> RuntimeGenericLowerResult<()> {
        self.emit_guard_failure_exit(
            RuntimeCmpOp::Ne,
            RuntimeOperand::Slot(len_slot),
            RuntimeOperand::Imm(u64::MAX),
            101,
        )?;

        let required_slot = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: required_slot,
            op: RuntimeBinOp::Add,
            lhs: RuntimeOperand::Slot(len_slot),
            rhs: RuntimeOperand::Imm(1),
        });

        self.emit_owned_list_ensure_capacity(
            ptr_slot,
            len_slot,
            capacity_slot,
            allocation_bytes_slot,
            RuntimeOperand::Slot(required_slot),
            u64::from(elem_ty.storage_bytes()),
        )?;
        let value = self.canonicalize_scalar_operand(value, elem_ty);
        self.emit(RuntimeInstr::HeapStoreInt {
            ptr: RuntimeOperand::Slot(ptr_slot),
            index: RuntimeOperand::Slot(len_slot),
            src: value,
            bytes: elem_ty.storage_bytes(),
        });
        self.emit(RuntimeInstr::Mov {
            dst: len_slot,
            src: RuntimeOperand::Slot(required_slot),
        });
        Ok(())
    }

    pub(super) fn emit_owned_string_char_count(
        &mut self,
        ptr_slot: usize,
        len_slot: usize,
    ) -> RuntimeGenericLowerResult<RuntimeOperand> {
        let byte_index = self.alloc_slot();
        let count = self.alloc_slot();
        let byte = self.alloc_slot();
        for slot in [byte_index, count] {
            self.emit(RuntimeInstr::Mov {
                dst: slot,
                src: RuntimeOperand::Imm(0),
            });
        }
        let loop_start = self.instrs.len();
        let done = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LtUnsigned,
            lhs: RuntimeOperand::Slot(byte_index),
            rhs: RuntimeOperand::Slot(len_slot),
            target: usize::MAX,
        });
        self.emit(RuntimeInstr::HeapLoadInt {
            dst: byte,
            ptr: RuntimeOperand::Slot(ptr_slot),
            index: RuntimeOperand::Slot(byte_index),
            bytes: 1,
        });
        let masked = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: masked,
            op: RuntimeBinOp::BitAnd,
            lhs: RuntimeOperand::Slot(byte),
            rhs: RuntimeOperand::Imm(0xc0),
        });
        let is_leading = self.alloc_slot();
        self.emit(RuntimeInstr::Cmp {
            dst: is_leading,
            op: RuntimeCmpOp::Ne,
            lhs: RuntimeOperand::Slot(masked),
            rhs: RuntimeOperand::Imm(0x80),
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: count,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Slot(is_leading),
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: byte_index,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Imm(1),
        });
        self.emit(RuntimeInstr::Jump { target: loop_start });
        let done_target = self.instrs.len();
        self.patch_target(done, done_target)?;
        Ok(RuntimeOperand::Slot(count))
    }

    pub(super) fn emit_owned_string_char_at(
        &mut self,
        ptr_slot: usize,
        len_slot: usize,
        target_scalar: RuntimeOperand,
    ) -> RuntimeGenericLowerResult<(RuntimeOperand, RuntimeOperand)> {
        let byte_index = self.alloc_slot();
        let scalar_index = self.alloc_slot();
        let tag = self.alloc_slot();
        let payload = self.alloc_slot();
        for slot in [byte_index, scalar_index, tag, payload] {
            self.emit(RuntimeInstr::Mov {
                dst: slot,
                src: RuntimeOperand::Imm(0),
            });
        }
        let loop_start = self.instrs.len();
        let exhausted = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LtUnsigned,
            lhs: RuntimeOperand::Slot(byte_index),
            rhs: RuntimeOperand::Slot(len_slot),
            target: usize::MAX,
        });
        let b0 = self.alloc_slot();
        self.emit(RuntimeInstr::HeapLoadInt {
            dst: b0,
            ptr: RuntimeOperand::Slot(ptr_slot),
            index: RuntimeOperand::Slot(byte_index),
            bytes: 1,
        });
        let prefix = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: prefix,
            op: RuntimeBinOp::BitAnd,
            lhs: RuntimeOperand::Slot(b0),
            rhs: RuntimeOperand::Imm(0xc0),
        });
        let continuation = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Ne,
            lhs: RuntimeOperand::Slot(prefix),
            rhs: RuntimeOperand::Imm(0x80),
            target: usize::MAX,
        });
        let not_target = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Eq,
            lhs: RuntimeOperand::Slot(scalar_index),
            rhs: target_scalar,
            target: usize::MAX,
        });

        let non_ascii = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LtUnsigned,
            lhs: RuntimeOperand::Slot(b0),
            rhs: RuntimeOperand::Imm(0x80),
            target: usize::MAX,
        });
        self.emit(RuntimeInstr::Mov {
            dst: payload,
            src: RuntimeOperand::Slot(b0),
        });
        let decoded_ascii = self.emit(RuntimeInstr::Jump { target: usize::MAX });
        let non_ascii_target = self.instrs.len();
        self.patch_target(non_ascii, non_ascii_target)?;

        let b1_index = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: b1_index,
            op: RuntimeBinOp::Add,
            lhs: RuntimeOperand::Slot(byte_index),
            rhs: RuntimeOperand::Imm(1),
        });
        let b1 = self.alloc_slot();
        self.emit(RuntimeInstr::HeapLoadInt {
            dst: b1,
            ptr: RuntimeOperand::Slot(ptr_slot),
            index: RuntimeOperand::Slot(b1_index),
            bytes: 1,
        });
        let lead = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: lead,
            op: RuntimeBinOp::BitAnd,
            lhs: RuntimeOperand::Slot(b0),
            rhs: RuntimeOperand::Imm(0x1f),
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: lead,
            op: RuntimeBinOp::Shl,
            rhs: RuntimeOperand::Imm(6),
        });
        let tail1 = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: tail1,
            op: RuntimeBinOp::BitAnd,
            lhs: RuntimeOperand::Slot(b1),
            rhs: RuntimeOperand::Imm(0x3f),
        });
        self.emit(RuntimeInstr::BinOp {
            dst: payload,
            op: RuntimeBinOp::BitOr,
            lhs: RuntimeOperand::Slot(lead),
            rhs: RuntimeOperand::Slot(tail1),
        });
        let decoded_two = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LtUnsigned,
            lhs: RuntimeOperand::Slot(b0),
            rhs: RuntimeOperand::Imm(0xe0),
            target: usize::MAX,
        });
        let finish_two = self.emit(RuntimeInstr::Jump { target: usize::MAX });
        let decode_three = self.instrs.len();
        self.patch_target(decoded_two, decode_three)?;

        let b2_index = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: b2_index,
            op: RuntimeBinOp::Add,
            lhs: RuntimeOperand::Slot(byte_index),
            rhs: RuntimeOperand::Imm(2),
        });
        let b2 = self.alloc_slot();
        self.emit(RuntimeInstr::HeapLoadInt {
            dst: b2,
            ptr: RuntimeOperand::Slot(ptr_slot),
            index: RuntimeOperand::Slot(b2_index),
            bytes: 1,
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: payload,
            op: RuntimeBinOp::Shl,
            rhs: RuntimeOperand::Imm(6),
        });
        let tail2 = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: tail2,
            op: RuntimeBinOp::BitAnd,
            lhs: RuntimeOperand::Slot(b2),
            rhs: RuntimeOperand::Imm(0x3f),
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: payload,
            op: RuntimeBinOp::BitOr,
            rhs: RuntimeOperand::Slot(tail2),
        });
        let decoded_three = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LtUnsigned,
            lhs: RuntimeOperand::Slot(b0),
            rhs: RuntimeOperand::Imm(0xf0),
            target: usize::MAX,
        });
        let finish_three = self.emit(RuntimeInstr::Jump { target: usize::MAX });
        let decode_four = self.instrs.len();
        self.patch_target(decoded_three, decode_four)?;

        self.emit(RuntimeInstr::BinOp {
            dst: payload,
            op: RuntimeBinOp::BitAnd,
            lhs: RuntimeOperand::Slot(b0),
            rhs: RuntimeOperand::Imm(0x07),
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: payload,
            op: RuntimeBinOp::Shl,
            rhs: RuntimeOperand::Imm(6),
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: payload,
            op: RuntimeBinOp::BitOr,
            rhs: RuntimeOperand::Slot(tail1),
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: payload,
            op: RuntimeBinOp::Shl,
            rhs: RuntimeOperand::Imm(6),
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: payload,
            op: RuntimeBinOp::BitOr,
            rhs: RuntimeOperand::Slot(tail2),
        });

        let b3_index = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: b3_index,
            op: RuntimeBinOp::Add,
            lhs: RuntimeOperand::Slot(byte_index),
            rhs: RuntimeOperand::Imm(3),
        });
        let b3 = self.alloc_slot();
        self.emit(RuntimeInstr::HeapLoadInt {
            dst: b3,
            ptr: RuntimeOperand::Slot(ptr_slot),
            index: RuntimeOperand::Slot(b3_index),
            bytes: 1,
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: payload,
            op: RuntimeBinOp::Shl,
            rhs: RuntimeOperand::Imm(6),
        });
        let tail3 = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: tail3,
            op: RuntimeBinOp::BitAnd,
            lhs: RuntimeOperand::Slot(b3),
            rhs: RuntimeOperand::Imm(0x3f),
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: payload,
            op: RuntimeBinOp::BitOr,
            rhs: RuntimeOperand::Slot(tail3),
        });

        let decoded = self.instrs.len();
        self.patch_target(decoded_ascii, decoded)?;
        self.patch_target(finish_two, decoded)?;
        self.patch_target(finish_three, decoded)?;
        self.emit(RuntimeInstr::Mov {
            dst: tag,
            src: RuntimeOperand::Imm(1),
        });
        let found = self.emit(RuntimeInstr::Jump { target: usize::MAX });

        let advance_leading = self.instrs.len();
        self.patch_target(not_target, advance_leading)?;
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: scalar_index,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Imm(1),
        });
        let advance_byte = self.instrs.len();
        self.patch_target(continuation, advance_byte)?;
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: byte_index,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Imm(1),
        });
        self.emit(RuntimeInstr::Jump { target: loop_start });
        let done = self.instrs.len();
        self.patch_target(exhausted, done)?;
        self.patch_target(found, done)?;
        Ok((RuntimeOperand::Slot(tag), RuntimeOperand::Slot(payload)))
    }

    pub(super) fn emit_owned_list_ensure_capacity(
        &mut self,
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        required: RuntimeOperand,
        element_bytes: u64,
    ) -> RuntimeGenericLowerResult<()> {
        let grow = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::GeUnsigned,
            lhs: RuntimeOperand::Slot(capacity_slot),
            rhs: required,
            target: usize::MAX,
        });
        let capacity_ready = self.emit(RuntimeInstr::Jump { target: usize::MAX });
        let grow_target = self.instrs.len();
        self.patch_target(grow, grow_target)?;

        let half_slot = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: half_slot,
            op: RuntimeBinOp::DivUnsigned,
            lhs: RuntimeOperand::Slot(capacity_slot),
            rhs: RuntimeOperand::Imm(2),
        });
        let candidate_slot = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: candidate_slot,
            op: RuntimeBinOp::Add,
            lhs: RuntimeOperand::Slot(capacity_slot),
            rhs: RuntimeOperand::Slot(half_slot),
        });
        self.emit_guard_failure_exit(
            RuntimeCmpOp::GeUnsigned,
            RuntimeOperand::Slot(candidate_slot),
            RuntimeOperand::Slot(capacity_slot),
            101,
        )?;

        let keep_minimum = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::GeUnsigned,
            lhs: RuntimeOperand::Slot(candidate_slot),
            rhs: RuntimeOperand::Imm(4),
            target: usize::MAX,
        });
        let after_minimum = self.emit(RuntimeInstr::Jump { target: usize::MAX });
        let set_minimum = self.instrs.len();
        self.patch_target(keep_minimum, set_minimum)?;
        self.emit(RuntimeInstr::Mov {
            dst: candidate_slot,
            src: RuntimeOperand::Imm(4),
        });
        let minimum_ready = self.instrs.len();
        self.patch_target(after_minimum, minimum_ready)?;

        let keep_required = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::GeUnsigned,
            lhs: RuntimeOperand::Slot(candidate_slot),
            rhs: required,
            target: usize::MAX,
        });
        let after_required = self.emit(RuntimeInstr::Jump { target: usize::MAX });
        let set_required = self.instrs.len();
        self.patch_target(keep_required, set_required)?;
        self.emit(RuntimeInstr::Mov {
            dst: candidate_slot,
            src: required,
        });
        let candidate_ready = self.instrs.len();
        self.patch_target(after_required, candidate_ready)?;

        self.emit_guard_failure_exit(
            RuntimeCmpOp::LeUnsigned,
            RuntimeOperand::Slot(candidate_slot),
            RuntimeOperand::Imm(u64::MAX / element_bytes),
            101,
        )?;
        let new_bytes_slot = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: new_bytes_slot,
            op: RuntimeBinOp::Mul,
            lhs: RuntimeOperand::Slot(candidate_slot),
            rhs: RuntimeOperand::Imm(element_bytes),
        });
        let new_ptr_slot = self.alloc_slot();
        self.emit(RuntimeInstr::Alloc {
            dst: new_ptr_slot,
            size: RuntimeOperand::Slot(new_bytes_slot),
        });
        self.emit_guard_failure_exit(
            RuntimeCmpOp::Ne,
            RuntimeOperand::Slot(new_ptr_slot),
            RuntimeOperand::Imm(0),
            101,
        )?;
        let copy_bytes_slot = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: copy_bytes_slot,
            op: RuntimeBinOp::Mul,
            lhs: RuntimeOperand::Slot(len_slot),
            rhs: RuntimeOperand::Imm(element_bytes),
        });
        self.emit(RuntimeInstr::HeapCopy {
            dst_ptr: RuntimeOperand::Slot(new_ptr_slot),
            src_ptr: RuntimeOperand::Slot(ptr_slot),
            bytes: RuntimeOperand::Slot(copy_bytes_slot),
        });
        self.emit(RuntimeInstr::Free {
            ptr: RuntimeOperand::Slot(ptr_slot),
            size: RuntimeOperand::Slot(allocation_bytes_slot),
        });
        self.emit(RuntimeInstr::Mov {
            dst: ptr_slot,
            src: RuntimeOperand::Slot(new_ptr_slot),
        });
        self.emit(RuntimeInstr::Mov {
            dst: capacity_slot,
            src: RuntimeOperand::Slot(candidate_slot),
        });
        self.emit(RuntimeInstr::Mov {
            dst: allocation_bytes_slot,
            src: RuntimeOperand::Slot(new_bytes_slot),
        });

        let ready_target = self.instrs.len();
        self.patch_target(capacity_ready, ready_target)?;
        Ok(())
    }

    pub(super) fn emit_owned_list_shrink_to(
        &mut self,
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        minimum: RuntimeOperand,
        element_bytes: u64,
    ) -> RuntimeGenericLowerResult<()> {
        let target_slot = self.alloc_slot();
        self.emit(RuntimeInstr::Mov {
            dst: target_slot,
            src: RuntimeOperand::Slot(len_slot),
        });
        let keep_len = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::GeUnsigned,
            lhs: RuntimeOperand::Slot(target_slot),
            rhs: minimum,
            target: usize::MAX,
        });
        let target_ready = self.emit(RuntimeInstr::Jump { target: usize::MAX });
        let use_minimum = self.instrs.len();
        self.patch_target(keep_len, use_minimum)?;
        self.emit(RuntimeInstr::Mov {
            dst: target_slot,
            src: minimum,
        });
        let target_is_ready = self.instrs.len();
        self.patch_target(target_ready, target_is_ready)?;

        let no_shrink = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::GtUnsigned,
            lhs: RuntimeOperand::Slot(capacity_slot),
            rhs: RuntimeOperand::Slot(target_slot),
            target: usize::MAX,
        });
        let zero_target = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Ne,
            lhs: RuntimeOperand::Slot(target_slot),
            rhs: RuntimeOperand::Imm(0),
            target: usize::MAX,
        });

        self.emit_guard_failure_exit(
            RuntimeCmpOp::LeUnsigned,
            RuntimeOperand::Slot(target_slot),
            RuntimeOperand::Imm(u64::MAX / element_bytes),
            101,
        )?;
        let new_bytes_slot = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: new_bytes_slot,
            op: RuntimeBinOp::Mul,
            lhs: RuntimeOperand::Slot(target_slot),
            rhs: RuntimeOperand::Imm(element_bytes),
        });
        let new_ptr_slot = self.alloc_slot();
        self.emit(RuntimeInstr::Alloc {
            dst: new_ptr_slot,
            size: RuntimeOperand::Slot(new_bytes_slot),
        });
        self.emit_guard_failure_exit(
            RuntimeCmpOp::Ne,
            RuntimeOperand::Slot(new_ptr_slot),
            RuntimeOperand::Imm(0),
            101,
        )?;
        let copy_bytes_slot = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: copy_bytes_slot,
            op: RuntimeBinOp::Mul,
            lhs: RuntimeOperand::Slot(len_slot),
            rhs: RuntimeOperand::Imm(element_bytes),
        });
        self.emit(RuntimeInstr::HeapCopy {
            dst_ptr: RuntimeOperand::Slot(new_ptr_slot),
            src_ptr: RuntimeOperand::Slot(ptr_slot),
            bytes: RuntimeOperand::Slot(copy_bytes_slot),
        });
        self.emit(RuntimeInstr::Free {
            ptr: RuntimeOperand::Slot(ptr_slot),
            size: RuntimeOperand::Slot(allocation_bytes_slot),
        });
        self.emit(RuntimeInstr::Mov {
            dst: ptr_slot,
            src: RuntimeOperand::Slot(new_ptr_slot),
        });
        self.emit(RuntimeInstr::Mov {
            dst: capacity_slot,
            src: RuntimeOperand::Slot(target_slot),
        });
        self.emit(RuntimeInstr::Mov {
            dst: allocation_bytes_slot,
            src: RuntimeOperand::Slot(new_bytes_slot),
        });
        let shrink_done = self.emit(RuntimeInstr::Jump { target: usize::MAX });

        let zero_target_start = self.instrs.len();
        self.patch_target(zero_target, zero_target_start)?;
        self.emit(RuntimeInstr::Free {
            ptr: RuntimeOperand::Slot(ptr_slot),
            size: RuntimeOperand::Slot(allocation_bytes_slot),
        });
        for slot in [ptr_slot, capacity_slot, allocation_bytes_slot] {
            self.emit(RuntimeInstr::Mov {
                dst: slot,
                src: RuntimeOperand::Imm(0),
            });
        }

        let done = self.instrs.len();
        self.patch_target(no_shrink, done)?;
        self.patch_target(shrink_done, done)?;
        Ok(())
    }

    pub(super) fn runtime_struct_list_layout(
        &self,
        struct_name: &str,
        span: Span,
    ) -> RuntimeGenericLowerResult<RuntimeStructListLayout> {
        let fields = self.runtime_struct_scalar_fields(struct_name, span)?;
        if fields.is_empty() {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                "runtime native struct lists require at least one stored field",
                span,
            )));
        }

        let align_up = |value: u64, alignment: u64| {
            value
                .checked_add(alignment - 1)
                .map(|rounded| rounded & !(alignment - 1))
                .ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime struct layout exceeds addressable storage",
                        span,
                    ))
                })
        };
        let mut offset_bytes = 0u64;
        let mut max_alignment = 1u64;
        let mut runtime_fields = Vec::with_capacity(fields.len());
        for (name, ty) in fields {
            let alignment = u64::from(ty.storage_bytes());
            max_alignment = max_alignment.max(alignment);
            offset_bytes = align_up(offset_bytes, alignment)?;
            runtime_fields.push(RuntimeStructFieldLayout {
                name,
                ty,
                offset_bytes,
            });
            offset_bytes = offset_bytes.checked_add(alignment).ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "runtime struct layout exceeds addressable storage",
                    span,
                ))
            })?;
        }
        let stride_bytes = align_up(offset_bytes, max_alignment)?;
        Ok(RuntimeStructListLayout {
            struct_name: struct_name.to_owned(),
            fields: runtime_fields,
            stride_bytes,
        })
    }

    pub(super) fn runtime_enum_layout(
        &self,
        ty: &TypeName,
        span: Span,
    ) -> RuntimeGenericLowerResult<RuntimeEnumLayout> {
        let (enum_name, type_args) = match ty {
            TypeName::Struct(name) => (name.clone(), Vec::new()),
            TypeName::Applied { name, args } => (name.clone(), args.clone()),
            _ => return Err(RuntimeGenericLowerError::Unsupported),
        };
        let def = self.enums.get(&enum_name).ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                format!("unknown enum '{enum_name}'").as_str(),
                span,
            ))
        })?;
        if type_args.len() != def.type_params.len() {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!(
                    "enum '{}' expects {} type {}, got {}",
                    enum_name,
                    def.type_params.len(),
                    argument_noun(def.type_params.len()),
                    type_args.len()
                )
                .as_str(),
                span,
            )));
        }
        let substitutions: HashMap<String, TypeName> = def
            .type_params
            .iter()
            .cloned()
            .zip(type_args.iter().cloned())
            .collect();
        let mut variants = Vec::with_capacity(def.variants.len());
        let mut payload_slots = 0usize;
        for (tag, variant) in def.variants.iter().enumerate() {
            let payload_fields: Vec<(String, TypeName, Span)> = match &variant.payload {
                EnumVariantPayloadDef::Unit => Vec::new(),
                EnumVariantPayloadDef::Tuple(fields) => fields
                    .iter()
                    .enumerate()
                    .map(|(index, field)| {
                        (
                            index.to_string(),
                            instantiate_generic_type(&field.ty, &substitutions),
                            field.span,
                        )
                    })
                    .collect(),
                EnumVariantPayloadDef::Named(fields) => fields
                    .iter()
                    .map(|field| {
                        (
                            field.name.clone(),
                            instantiate_generic_type(&field.ty, &substitutions),
                            field.span,
                        )
                    })
                    .collect(),
            };
            let resource_count = payload_fields
                .iter()
                .filter(|(_, ty, _)| {
                    matches!(
                        ty,
                        TypeName::List { .. } | TypeName::String | TypeName::Map { .. }
                    )
                })
                .count();
            if resource_count > 0 && payload_fields.len() != 1 {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "runtime-native resource enum variants currently require exactly one owned payload field",
                    payload_fields[0].2,
                )));
            }
            let nested_field = payload_fields.first().filter(|(_, ty, _)| match ty {
                TypeName::Struct(name) | TypeName::Applied { name, .. } => {
                    self.enums.contains_key(name)
                }
                _ => false,
            });
            if nested_field.is_some() && payload_fields.len() != 1 {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "runtime-native nested enum variants currently require exactly one payload field",
                    payload_fields[0].2,
                )));
            }
            let (fields, resource, nested) = if let Some((name, resource_ty, field_span)) =
                payload_fields.first().filter(|(_, ty, _)| {
                    matches!(
                        ty,
                        TypeName::List { .. } | TypeName::String | TypeName::Map { .. }
                    )
                }) {
                let kind = match resource_ty {
                    TypeName::List { elem } => {
                        if let TypeName::Struct(struct_name) = elem.as_ref() {
                            RuntimeEnumResourceKind::ListStruct(
                                self.runtime_struct_list_layout(struct_name, *field_span)?,
                            )
                        } else {
                            RuntimeEnumResourceKind::ListScalar(
                                ensure_runtime_generic_scalar_type(elem, *field_span)
                                    .map_err(RuntimeGenericLowerError::Diagnostic)?,
                            )
                        }
                    }
                    TypeName::String => RuntimeEnumResourceKind::String,
                    TypeName::Map { key, value } => RuntimeEnumResourceKind::Map(
                        self.runtime_map_layout(key, value, *field_span)?,
                    ),
                    _ => unreachable!("filtered resource enum type"),
                };
                (
                    Vec::new(),
                    Some(RuntimeEnumResourceLayout {
                        name: name.clone(),
                        kind,
                    }),
                    None,
                )
            } else if let Some((name, nested_ty, field_span)) = nested_field {
                let nested_layout = self.runtime_enum_layout(nested_ty, *field_span)?;
                (
                    Vec::new(),
                    None,
                    Some(RuntimeEnumNestedLayout {
                        name: name.clone(),
                        layout: Box::new(nested_layout),
                    }),
                )
            } else {
                let fields = payload_fields
                    .into_iter()
                    .map(|(name, ty, field_span)| {
                        let scalar = ensure_runtime_generic_scalar_type(&ty, field_span).map_err(
                            |_| {
                                RuntimeGenericLowerError::Diagnostic(type_error(
                                    "runtime-native enum payload fields must be scalar or one owned string/list/map",
                                    field_span,
                                ))
                            },
                        )?;
                        Ok((name, scalar))
                    })
                    .collect::<RuntimeGenericLowerResult<Vec<_>>>()?;
                (fields, None, None)
            };
            let variant_payload_slots = if resource.is_some() {
                4
            } else if let Some(nested) = &nested {
                1 + nested.layout.payload_slots
            } else {
                fields.len()
            };
            payload_slots = payload_slots.max(variant_payload_slots);
            variants.push(RuntimeEnumVariantLayout {
                name: variant.name.clone(),
                tag: tag as u64,
                fields,
                resource,
                nested,
            });
        }
        Ok(RuntimeEnumLayout {
            enum_name,
            type_args,
            variants,
            payload_slots,
        })
    }

    pub(super) fn runtime_map_layout(
        &self,
        key: &TypeName,
        value: &TypeName,
        span: Span,
    ) -> RuntimeGenericLowerResult<RuntimeMapLayout> {
        let key_ty = ensure_runtime_generic_scalar_type(key, span).map_err(|_| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                "runtime-native map keys currently require scalar values",
                span,
            ))
        })?;
        let value_ty = ensure_runtime_generic_scalar_type(value, span).map_err(|_| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                "runtime-native map values currently require scalar values",
                span,
            ))
        })?;
        let key_bytes = u64::from(key_ty.storage_bytes());
        let value_bytes = u64::from(value_ty.storage_bytes());
        let align_up = |input: u64, alignment: u64| {
            input
                .checked_add(alignment.saturating_sub(1))
                .map(|value| value / alignment * alignment)
                .ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime map entry layout exceeds addressable storage",
                        span,
                    ))
                })
        };
        let value_offset_bytes = align_up(key_bytes, value_bytes)?;
        let alignment = key_bytes.max(value_bytes);
        let stride_bytes = align_up(
            value_offset_bytes.checked_add(value_bytes).ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "runtime map entry layout exceeds addressable storage",
                    span,
                ))
            })?,
            alignment,
        )?;
        Ok(RuntimeMapLayout {
            key_ty,
            value_ty,
            value_offset_bytes,
            stride_bytes,
        })
    }

    fn emit_parse_error_text(
        &mut self,
        payload_slots: &[usize],
        message: &[u8],
    ) -> RuntimeGenericLowerResult<()> {
        if payload_slots.len() < 4 {
            return Err(RuntimeGenericLowerError::Unsupported);
        }
        let allocation_bytes = (message.len() as u64) + 1;
        self.emit(RuntimeInstr::Mov {
            dst: payload_slots[1],
            src: RuntimeOperand::Imm(message.len() as u64),
        });
        self.emit(RuntimeInstr::Mov {
            dst: payload_slots[2],
            src: RuntimeOperand::Imm(allocation_bytes),
        });
        self.emit(RuntimeInstr::Mov {
            dst: payload_slots[3],
            src: RuntimeOperand::Imm(allocation_bytes),
        });
        self.emit(RuntimeInstr::Alloc {
            dst: payload_slots[0],
            size: RuntimeOperand::Imm(allocation_bytes),
        });
        self.emit_guard_failure_exit(
            RuntimeCmpOp::Ne,
            RuntimeOperand::Slot(payload_slots[0]),
            RuntimeOperand::Imm(0),
            101,
        )?;
        for (index, byte) in message
            .iter()
            .copied()
            .chain(std::iter::once(0))
            .enumerate()
        {
            self.emit(RuntimeInstr::HeapStoreInt {
                ptr: RuntimeOperand::Slot(payload_slots[0]),
                index: RuntimeOperand::Imm(index as u64),
                src: RuntimeOperand::Imm(u64::from(byte)),
                bytes: 1,
            });
        }
        Ok(())
    }

    pub(super) fn lower_native_integer_parse(
        &mut self,
        receiver: &Expr,
        method: &str,
        expected_ty: &TypeName,
        mutable: bool,
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<RuntimeGenericBinding>> {
        let Some((signed, bits)) = (match method {
            "parse_i8" => Some((true, 8)),
            "parse_i16" => Some((true, 16)),
            "parse_i32" => Some((true, 32)),
            "parse_i64" => Some((true, 64)),
            "parse_u8" => Some((false, 8)),
            "parse_u16" => Some((false, 16)),
            "parse_u32" => Some((false, 32)),
            "parse_u64" => Some((false, 64)),
            _ => None,
        }) else {
            return Ok(None);
        };
        let TypeName::Applied { name, args } = expected_ty else {
            return Ok(None);
        };
        if name != "Result"
            || !matches!(
                args.as_slice(),
                [TypeName::Int { signed: actual_signed, bits: actual_bits }, TypeName::String]
                    if *actual_signed == signed && *actual_bits == bits
            )
        {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                "native integer parsing requires Result with the matching integer and string error types",
                span,
            )));
        }
        let Expr::Ident {
            name: receiver_name,
            span: receiver_span,
        } = receiver
        else {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                "native parsing requires a named owned string receiver",
                receiver.span(),
            )));
        };
        self.reject_moved_resource(receiver_name, *receiver_span)?;
        let (ptr_slot, len_slot, _, _, _) = self
            .scopes
            .get(receiver_name)
            .and_then(RuntimeGenericBinding::as_owned_string)
            .ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "native parsing requires an owned string receiver",
                    *receiver_span,
                ))
            })?;
        let layout = self.runtime_enum_layout(expected_ty, span)?;
        let ok_tag = layout
            .variants
            .iter()
            .find(|variant| variant.name == "Ok")
            .map(|variant| variant.tag)
            .ok_or(RuntimeGenericLowerError::Unsupported)?;
        let err_tag = layout
            .variants
            .iter()
            .find(|variant| variant.name == "Err")
            .map(|variant| variant.tag)
            .ok_or(RuntimeGenericLowerError::Unsupported)?;

        let tag_slot = self.alloc_slot();
        self.emit(RuntimeInstr::Mov {
            dst: tag_slot,
            src: RuntimeOperand::Imm(err_tag),
        });
        let payload_slots = (0..layout.payload_slots)
            .map(|_| {
                let slot = self.alloc_slot();
                self.emit(RuntimeInstr::Mov {
                    dst: slot,
                    src: RuntimeOperand::Imm(0),
                });
                slot
            })
            .collect::<Vec<_>>();
        if payload_slots.len() < 4 {
            return Err(RuntimeGenericLowerError::Unsupported);
        }

        let mut error_jumps = Vec::new();
        error_jumps.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Ne,
            lhs: RuntimeOperand::Slot(len_slot),
            rhs: RuntimeOperand::Imm(0),
            target: usize::MAX,
        }));
        let index_slot = self.alloc_slot();
        let negative_slot = self.alloc_slot();
        self.emit(RuntimeInstr::Mov {
            dst: index_slot,
            src: RuntimeOperand::Imm(0),
        });
        self.emit(RuntimeInstr::Mov {
            dst: negative_slot,
            src: RuntimeOperand::Imm(0),
        });
        let first_byte = self.alloc_slot();
        self.emit(RuntimeInstr::HeapLoadInt {
            dst: first_byte,
            ptr: RuntimeOperand::Slot(ptr_slot),
            index: RuntimeOperand::Imm(0),
            bytes: 1,
        });
        let negative_done = if signed {
            let not_negative = self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::Eq,
                lhs: RuntimeOperand::Slot(first_byte),
                rhs: RuntimeOperand::Imm(u64::from(b'-')),
                target: usize::MAX,
            });
            self.emit(RuntimeInstr::Mov {
                dst: negative_slot,
                src: RuntimeOperand::Imm(1),
            });
            self.emit(RuntimeInstr::Mov {
                dst: index_slot,
                src: RuntimeOperand::Imm(1),
            });
            let done = self.emit(RuntimeInstr::Jump { target: usize::MAX });
            self.patch_target(not_negative, self.instrs.len())?;
            Some(done)
        } else {
            None
        };
        let not_positive = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Eq,
            lhs: RuntimeOperand::Slot(first_byte),
            rhs: RuntimeOperand::Imm(u64::from(b'+')),
            target: usize::MAX,
        });
        self.emit(RuntimeInstr::Mov {
            dst: index_slot,
            src: RuntimeOperand::Imm(1),
        });
        let sign_done = self.instrs.len();
        self.patch_target(not_positive, sign_done)?;
        if let Some(negative_done) = negative_done {
            self.patch_target(negative_done, sign_done)?;
        }
        error_jumps.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LtUnsigned,
            lhs: RuntimeOperand::Slot(index_slot),
            rhs: RuntimeOperand::Slot(len_slot),
            target: usize::MAX,
        }));

        let magnitude_slot = self.alloc_slot();
        self.emit(RuntimeInstr::Mov {
            dst: magnitude_slot,
            src: RuntimeOperand::Imm(0),
        });
        let loop_start = self.instrs.len();
        let loop_done = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LtUnsigned,
            lhs: RuntimeOperand::Slot(index_slot),
            rhs: RuntimeOperand::Slot(len_slot),
            target: usize::MAX,
        });
        let byte_slot = self.alloc_slot();
        self.emit(RuntimeInstr::HeapLoadInt {
            dst: byte_slot,
            ptr: RuntimeOperand::Slot(ptr_slot),
            index: RuntimeOperand::Slot(index_slot),
            bytes: 1,
        });
        error_jumps.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::GeUnsigned,
            lhs: RuntimeOperand::Slot(byte_slot),
            rhs: RuntimeOperand::Imm(u64::from(b'0')),
            target: usize::MAX,
        }));
        error_jumps.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LeUnsigned,
            lhs: RuntimeOperand::Slot(byte_slot),
            rhs: RuntimeOperand::Imm(u64::from(b'9')),
            target: usize::MAX,
        }));
        let digit_slot = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: digit_slot,
            op: RuntimeBinOp::Sub,
            lhs: RuntimeOperand::Slot(byte_slot),
            rhs: RuntimeOperand::Imm(u64::from(b'0')),
        });
        let max_magnitude = if signed {
            if bits == 64 {
                1u64 << 63
            } else {
                1u64 << (bits - 1)
            }
        } else if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        let positive_limit = if signed {
            max_magnitude - 1
        } else {
            max_magnitude
        };
        let limit_slot = self.alloc_slot();
        self.emit(RuntimeInstr::Mov {
            dst: limit_slot,
            src: RuntimeOperand::Imm(positive_limit),
        });
        if signed {
            let positive = self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::Ne,
                lhs: RuntimeOperand::Slot(negative_slot),
                rhs: RuntimeOperand::Imm(0),
                target: usize::MAX,
            });
            self.emit(RuntimeInstr::Mov {
                dst: limit_slot,
                src: RuntimeOperand::Imm(max_magnitude),
            });
            self.patch_target(positive, self.instrs.len())?;
        }
        let quotient_slot = self.alloc_slot();
        let remainder_slot = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: quotient_slot,
            op: RuntimeBinOp::DivUnsigned,
            lhs: RuntimeOperand::Slot(limit_slot),
            rhs: RuntimeOperand::Imm(10),
        });
        self.emit(RuntimeInstr::BinOp {
            dst: remainder_slot,
            op: RuntimeBinOp::ModUnsigned,
            lhs: RuntimeOperand::Slot(limit_slot),
            rhs: RuntimeOperand::Imm(10),
        });
        error_jumps.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LeUnsigned,
            lhs: RuntimeOperand::Slot(magnitude_slot),
            rhs: RuntimeOperand::Slot(quotient_slot),
            target: usize::MAX,
        }));
        let not_at_limit = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Eq,
            lhs: RuntimeOperand::Slot(magnitude_slot),
            rhs: RuntimeOperand::Slot(quotient_slot),
            target: usize::MAX,
        });
        error_jumps.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LeUnsigned,
            lhs: RuntimeOperand::Slot(digit_slot),
            rhs: RuntimeOperand::Slot(remainder_slot),
            target: usize::MAX,
        }));
        self.patch_target(not_at_limit, self.instrs.len())?;
        self.emit(RuntimeInstr::BinOp {
            dst: magnitude_slot,
            op: RuntimeBinOp::Mul,
            lhs: RuntimeOperand::Slot(magnitude_slot),
            rhs: RuntimeOperand::Imm(10),
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: magnitude_slot,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Slot(digit_slot),
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: index_slot,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Imm(1),
        });
        self.emit(RuntimeInstr::Jump { target: loop_start });

        let success = self.instrs.len();
        self.patch_target(loop_done, success)?;
        self.emit(RuntimeInstr::Mov {
            dst: payload_slots[0],
            src: RuntimeOperand::Slot(magnitude_slot),
        });
        if signed {
            let non_negative = self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::Ne,
                lhs: RuntimeOperand::Slot(negative_slot),
                rhs: RuntimeOperand::Imm(0),
                target: usize::MAX,
            });
            self.emit(RuntimeInstr::BinOp {
                dst: payload_slots[0],
                op: RuntimeBinOp::Sub,
                lhs: RuntimeOperand::Imm(0),
                rhs: RuntimeOperand::Slot(magnitude_slot),
            });
            self.patch_target(non_negative, self.instrs.len())?;
        }
        let result_ty = RuntimeIntType::new(signed, bits)?;
        self.normalize_slot(payload_slots[0], result_ty);
        self.emit(RuntimeInstr::Mov {
            dst: tag_slot,
            src: RuntimeOperand::Imm(ok_tag),
        });
        let success_done = self.emit(RuntimeInstr::Jump { target: usize::MAX });

        let error = self.instrs.len();
        for jump in error_jumps {
            self.patch_target(jump, error)?;
        }
        self.emit_parse_error_text(&payload_slots, b"invalid integer")?;
        self.patch_target(success_done, self.instrs.len())?;
        Ok(Some(RuntimeGenericBinding::EnumSlots {
            layout,
            tag_slot,
            payload_slots,
            mutable,
            owns_cleanup: true,
        }))
    }

    pub(super) fn lower_native_bool_parse(
        &mut self,
        receiver: &Expr,
        method: &str,
        expected_ty: &TypeName,
        mutable: bool,
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<RuntimeGenericBinding>> {
        if method != "parse_bool" {
            return Ok(None);
        }
        if !matches!(
            expected_ty,
            TypeName::Applied { name, args }
                if name == "Result" && matches!(args.as_slice(), [TypeName::Bool, TypeName::String])
        ) {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                "native boolean parsing requires Result<bool, string>",
                span,
            )));
        }
        let Expr::Ident {
            name: receiver_name,
            span: receiver_span,
        } = receiver
        else {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                "native parsing requires a named owned string receiver",
                receiver.span(),
            )));
        };
        self.reject_moved_resource(receiver_name, *receiver_span)?;
        let (ptr_slot, len_slot, _, _, _) = self
            .scopes
            .get(receiver_name)
            .and_then(RuntimeGenericBinding::as_owned_string)
            .ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "native parsing requires an owned string receiver",
                    *receiver_span,
                ))
            })?;
        let layout = self.runtime_enum_layout(expected_ty, span)?;
        let ok_tag = layout
            .variants
            .iter()
            .find(|variant| variant.name == "Ok")
            .map(|variant| variant.tag)
            .ok_or(RuntimeGenericLowerError::Unsupported)?;
        let err_tag = layout
            .variants
            .iter()
            .find(|variant| variant.name == "Err")
            .map(|variant| variant.tag)
            .ok_or(RuntimeGenericLowerError::Unsupported)?;
        let tag_slot = self.alloc_slot();
        self.emit(RuntimeInstr::Mov {
            dst: tag_slot,
            src: RuntimeOperand::Imm(err_tag),
        });
        let payload_slots = (0..layout.payload_slots)
            .map(|_| {
                let slot = self.alloc_slot();
                self.emit(RuntimeInstr::Mov {
                    dst: slot,
                    src: RuntimeOperand::Imm(0),
                });
                slot
            })
            .collect::<Vec<_>>();
        if payload_slots.len() < 4 {
            return Err(RuntimeGenericLowerError::Unsupported);
        }

        let not_true_len = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Eq,
            lhs: RuntimeOperand::Slot(len_slot),
            rhs: RuntimeOperand::Imm(4),
            target: usize::MAX,
        });
        let mut error_jumps = Vec::new();
        for (index, byte) in b"true".iter().enumerate() {
            let loaded = self.alloc_slot();
            self.emit(RuntimeInstr::HeapLoadInt {
                dst: loaded,
                ptr: RuntimeOperand::Slot(ptr_slot),
                index: RuntimeOperand::Imm(index as u64),
                bytes: 1,
            });
            error_jumps.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::Eq,
                lhs: RuntimeOperand::Slot(loaded),
                rhs: RuntimeOperand::Imm(u64::from(*byte)),
                target: usize::MAX,
            }));
        }
        self.emit(RuntimeInstr::Mov {
            dst: payload_slots[0],
            src: RuntimeOperand::Imm(1),
        });
        self.emit(RuntimeInstr::Mov {
            dst: tag_slot,
            src: RuntimeOperand::Imm(ok_tag),
        });
        let true_done = self.emit(RuntimeInstr::Jump { target: usize::MAX });

        let false_start = self.instrs.len();
        self.patch_target(not_true_len, false_start)?;
        error_jumps.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Eq,
            lhs: RuntimeOperand::Slot(len_slot),
            rhs: RuntimeOperand::Imm(5),
            target: usize::MAX,
        }));
        for (index, byte) in b"false".iter().enumerate() {
            let loaded = self.alloc_slot();
            self.emit(RuntimeInstr::HeapLoadInt {
                dst: loaded,
                ptr: RuntimeOperand::Slot(ptr_slot),
                index: RuntimeOperand::Imm(index as u64),
                bytes: 1,
            });
            error_jumps.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::Eq,
                lhs: RuntimeOperand::Slot(loaded),
                rhs: RuntimeOperand::Imm(u64::from(*byte)),
                target: usize::MAX,
            }));
        }
        self.emit(RuntimeInstr::Mov {
            dst: tag_slot,
            src: RuntimeOperand::Imm(ok_tag),
        });
        let false_done = self.emit(RuntimeInstr::Jump { target: usize::MAX });

        let error = self.instrs.len();
        for jump in error_jumps {
            self.patch_target(jump, error)?;
        }
        self.emit_parse_error_text(&payload_slots, b"invalid boolean")?;
        let done = self.instrs.len();
        self.patch_target(true_done, done)?;
        self.patch_target(false_done, done)?;
        Ok(Some(RuntimeGenericBinding::EnumSlots {
            layout,
            tag_slot,
            payload_slots,
            mutable,
            owns_cleanup: true,
        }))
    }

    pub(super) fn runtime_map_field_index(
        &mut self,
        entry: RuntimeOperand,
        layout: &RuntimeMapLayout,
        offset_bytes: u64,
        field_ty: RuntimeScalarType,
    ) -> RuntimeOperand {
        let field_bytes = u64::from(field_ty.storage_bytes());
        let stride_units = layout.stride_bytes / field_bytes;
        let offset_units = offset_bytes / field_bytes;
        let scaled = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: scaled,
            op: RuntimeBinOp::Mul,
            lhs: entry,
            rhs: RuntimeOperand::Imm(stride_units),
        });
        if offset_units == 0 {
            RuntimeOperand::Slot(scaled)
        } else {
            let indexed = self.alloc_slot();
            self.emit(RuntimeInstr::BinOp {
                dst: indexed,
                op: RuntimeBinOp::Add,
                lhs: RuntimeOperand::Slot(scaled),
                rhs: RuntimeOperand::Imm(offset_units),
            });
            RuntimeOperand::Slot(indexed)
        }
    }

    pub(super) fn emit_runtime_map_find(
        &mut self,
        ptr_slot: usize,
        len_slot: usize,
        layout: &RuntimeMapLayout,
        key: RuntimeOperand,
    ) -> RuntimeGenericLowerResult<(usize, usize)> {
        let index = self.alloc_slot();
        let found = self.alloc_slot();
        for slot in [index, found] {
            self.emit(RuntimeInstr::Mov {
                dst: slot,
                src: RuntimeOperand::Imm(0),
            });
        }
        let loop_start = self.instrs.len();
        let exhausted = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LtUnsigned,
            lhs: RuntimeOperand::Slot(index),
            rhs: RuntimeOperand::Slot(len_slot),
            target: usize::MAX,
        });
        let loaded = self.alloc_slot();
        let key_index =
            self.runtime_map_field_index(RuntimeOperand::Slot(index), layout, 0, layout.key_ty);
        self.emit(RuntimeInstr::HeapLoadInt {
            dst: loaded,
            ptr: RuntimeOperand::Slot(ptr_slot),
            index: key_index,
            bytes: layout.key_ty.storage_bytes(),
        });
        self.normalize_scalar_slot(loaded, layout.key_ty);
        let equal = match layout.key_ty {
            RuntimeScalarType::Int(_) => {
                let slot = self.alloc_slot();
                self.emit(RuntimeInstr::Cmp {
                    dst: slot,
                    op: RuntimeCmpOp::Eq,
                    lhs: RuntimeOperand::Slot(loaded),
                    rhs: key,
                });
                slot
            }
            RuntimeScalarType::Float(bits) => {
                self.emit_float_eq(RuntimeOperand::Slot(loaded), key, bits)?
            }
        };
        let miss = self.emit(RuntimeInstr::JumpIfZero {
            cond_slot: equal,
            target: usize::MAX,
        });
        self.emit(RuntimeInstr::Mov {
            dst: found,
            src: RuntimeOperand::Imm(1),
        });
        let found_done = self.emit(RuntimeInstr::Jump { target: usize::MAX });
        let next = self.instrs.len();
        self.patch_target(miss, next)?;
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: index,
            op: RuntimeBinOp::Add,
            rhs: RuntimeOperand::Imm(1),
        });
        self.emit(RuntimeInstr::Jump { target: loop_start });
        let done = self.instrs.len();
        self.patch_target(exhausted, done)?;
        self.patch_target(found_done, done)?;
        Ok((found, index))
    }

    pub(super) fn emit_runtime_map_store_field(
        &mut self,
        ptr_slot: usize,
        entry: RuntimeOperand,
        layout: &RuntimeMapLayout,
        offset_bytes: u64,
        ty: RuntimeScalarType,
        value: RuntimeOperand,
    ) {
        let index = self.runtime_map_field_index(entry, layout, offset_bytes, ty);
        let value = self.canonicalize_scalar_operand(value, ty);
        self.emit(RuntimeInstr::HeapStoreInt {
            ptr: RuntimeOperand::Slot(ptr_slot),
            index,
            src: value,
            bytes: ty.storage_bytes(),
        });
    }

    pub(super) fn lower_runtime_enum_constructor(
        &mut self,
        expected_ty: &TypeName,
        expr: &Expr,
        mutable: bool,
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<RuntimeGenericBinding>> {
        let (expr_enum, expr_variant) = match expr {
            Expr::EnumVariant {
                enum_name, variant, ..
            }
            | Expr::EnumTupleVariant {
                enum_name, variant, ..
            }
            | Expr::EnumStructVariant {
                enum_name, variant, ..
            } => (enum_name, variant),
            _ => return Ok(None),
        };
        let layout = self.runtime_enum_layout(expected_ty, span)?;
        if expr_enum != &layout.enum_name {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                "enum constructor does not match binding type",
                span,
            )));
        }
        let variant = layout
            .variants
            .iter()
            .find(|variant| variant.name == *expr_variant)
            .cloned()
            .ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("unknown variant '{}::{}'", expr_enum, expr_variant).as_str(),
                    span,
                ))
            })?;
        let enum_def = self
            .enums
            .get(&layout.enum_name)
            .cloned()
            .ok_or(RuntimeGenericLowerError::Unsupported)?;
        let variant_def = enum_def
            .variants
            .iter()
            .find(|candidate| candidate.name == variant.name)
            .ok_or(RuntimeGenericLowerError::Unsupported)?;
        let mut inferred: HashMap<String, TypeName> = enum_def
            .type_params
            .iter()
            .cloned()
            .zip(layout.type_args.iter().cloned())
            .collect();
        match (expr, &variant_def.payload) {
            (Expr::EnumTupleVariant { args, .. }, EnumVariantPayloadDef::Tuple(fields)) => {
                for (arg, field) in args.iter().zip(fields) {
                    let actual = self.infer_expr_runtime_type_name(arg)?;
                    infer_generic_type_bindings(
                        &field.ty,
                        &actual,
                        &enum_def.type_params,
                        &mut inferred,
                        arg.span(),
                    )
                    .map_err(RuntimeGenericLowerError::Diagnostic)?;
                }
            }
            (Expr::EnumStructVariant { fields, .. }, EnumVariantPayloadDef::Named(field_defs)) => {
                for field in fields {
                    let Some(field_def) = field_defs
                        .iter()
                        .find(|candidate| candidate.name == field.name)
                    else {
                        continue;
                    };
                    let actual = self.infer_expr_runtime_type_name(&field.expr)?;
                    infer_generic_type_bindings(
                        &field_def.ty,
                        &actual,
                        &enum_def.type_params,
                        &mut inferred,
                        field.span,
                    )
                    .map_err(RuntimeGenericLowerError::Diagnostic)?;
                }
            }
            _ => {}
        }
        if let Some(resource) = &variant.resource {
            let payload_expr = match expr {
                Expr::EnumTupleVariant { args, .. } if args.len() == 1 => &args[0],
                Expr::EnumStructVariant { fields, .. } if fields.len() == 1 => {
                    let field = fields
                        .iter()
                        .find(|field| field.name == resource.name)
                        .ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("missing enum payload field '{}'", resource.name).as_str(),
                                span,
                            ))
                        })?;
                    &field.expr
                }
                _ => {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "resource enum variant requires exactly one owned payload",
                        span,
                    )));
                }
            };
            let Expr::Ident {
                name: source_name,
                span: source_span,
            } = payload_expr
            else {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "resource enum payload must move a named current-scope owner",
                    payload_expr.span(),
                )));
            };
            self.reject_moved_resource(source_name, *source_span)?;
            let source = self.scopes.get(source_name).cloned().ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "resource enum payload requires a current-scope owner",
                    *source_span,
                ))
            })?;
            let source_slots = match (&resource.kind, &source) {
                (
                    RuntimeEnumResourceKind::ListScalar(expected),
                    RuntimeGenericBinding::OwnedListScalar {
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        elem_ty,
                        ..
                    },
                ) if expected == elem_ty => {
                    [*ptr_slot, *len_slot, *capacity_slot, *allocation_bytes_slot]
                }
                (
                    RuntimeEnumResourceKind::ListStruct(expected),
                    RuntimeGenericBinding::OwnedListStruct {
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        layout,
                        ..
                    },
                ) if expected == layout => {
                    [*ptr_slot, *len_slot, *capacity_slot, *allocation_bytes_slot]
                }
                (
                    RuntimeEnumResourceKind::String,
                    RuntimeGenericBinding::OwnedString {
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        ..
                    },
                ) => [*ptr_slot, *len_slot, *capacity_slot, *allocation_bytes_slot],
                (
                    RuntimeEnumResourceKind::Map(expected),
                    RuntimeGenericBinding::OwnedMap {
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        layout,
                        ..
                    },
                ) if expected == layout => {
                    [*ptr_slot, *len_slot, *capacity_slot, *allocation_bytes_slot]
                }
                _ => {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "resource enum payload type does not match its variant",
                        *source_span,
                    )));
                }
            };
            let tag_slot = self.alloc_slot();
            self.emit(RuntimeInstr::Mov {
                dst: tag_slot,
                src: RuntimeOperand::Imm(variant.tag),
            });
            let mut payload_slots = Vec::with_capacity(layout.payload_slots);
            for index in 0..layout.payload_slots {
                let slot = self.alloc_slot();
                self.emit(RuntimeInstr::Mov {
                    dst: slot,
                    src: source_slots
                        .get(index)
                        .copied()
                        .map(RuntimeOperand::Slot)
                        .unwrap_or(RuntimeOperand::Imm(0)),
                });
                payload_slots.push(slot);
            }
            let _ = self.scopes.take(source_name);
            self.scopes.insert(
                source_name.clone(),
                RuntimeGenericBinding::MovedResource {
                    kind: "resource owner",
                },
            );
            return Ok(Some(RuntimeGenericBinding::EnumSlots {
                layout,
                tag_slot,
                payload_slots,
                mutable,
                owns_cleanup: true,
            }));
        }
        if let Some(nested) = &variant.nested {
            let payload_expr = match expr {
                Expr::EnumTupleVariant { args, .. } if args.len() == 1 => &args[0],
                Expr::EnumStructVariant { fields, .. } if fields.len() == 1 => {
                    &fields
                        .iter()
                        .find(|field| field.name == nested.name)
                        .ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("missing enum payload field '{}'", nested.name).as_str(),
                                span,
                            ))
                        })?
                        .expr
                }
                _ => {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "nested enum variant requires exactly one named enum payload",
                        span,
                    )));
                }
            };
            let Expr::Ident {
                name: source_name,
                span: source_span,
            } = payload_expr
            else {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "nested enum payload must use a named binding",
                    payload_expr.span(),
                )));
            };
            let source = self.scopes.get(source_name).cloned().ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "nested enum payload requires a visible enum binding",
                    *source_span,
                ))
            })?;
            let RuntimeGenericBinding::EnumSlots {
                layout: source_layout,
                tag_slot: source_tag,
                payload_slots: source_payload,
                owns_cleanup,
                ..
            } = source
            else {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "nested enum payload type does not match its variant",
                    *source_span,
                )));
            };
            if source_layout != *nested.layout {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "nested enum payload type does not match its variant",
                    *source_span,
                )));
            }
            let tag_slot = self.alloc_slot();
            self.emit(RuntimeInstr::Mov {
                dst: tag_slot,
                src: RuntimeOperand::Imm(variant.tag),
            });
            let mut payload_slots = Vec::with_capacity(layout.payload_slots);
            for index in 0..layout.payload_slots {
                let source = if index == 0 {
                    Some(source_tag)
                } else {
                    source_payload.get(index - 1).copied()
                };
                let slot = self.alloc_slot();
                self.emit(RuntimeInstr::Mov {
                    dst: slot,
                    src: source
                        .map(RuntimeOperand::Slot)
                        .unwrap_or(RuntimeOperand::Imm(0)),
                });
                payload_slots.push(slot);
            }
            if nested.layout.owns_resources() && owns_cleanup {
                let _ = self.scopes.take(source_name);
                self.scopes.insert(
                    source_name.clone(),
                    RuntimeGenericBinding::MovedResource {
                        kind: "nested enum owner",
                    },
                );
            }
            return Ok(Some(RuntimeGenericBinding::EnumSlots {
                layout,
                tag_slot,
                payload_slots,
                mutable,
                owns_cleanup: true,
            }));
        }
        let values: Vec<RuntimeOperand> = match expr {
            Expr::EnumVariant { .. } => {
                if !variant.fields.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "enum variant requires a payload",
                        span,
                    )));
                }
                Vec::new()
            }
            Expr::EnumTupleVariant { args, .. } => {
                if args.len() != variant.fields.len() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "enum tuple payload arity mismatch",
                        span,
                    )));
                }
                let mut values = Vec::with_capacity(args.len());
                for (arg, (_, field_ty)) in args.iter().zip(variant.fields.iter()) {
                    values.push(self.lower_expr_as_scalar(arg, *field_ty)?);
                }
                values
            }
            Expr::EnumStructVariant { fields, .. } => {
                if fields.len() != variant.fields.len() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "enum named payload field count mismatch",
                        span,
                    )));
                }
                let mut by_name = HashMap::with_capacity(fields.len());
                for field in fields {
                    if by_name.insert(field.name.clone(), &field.expr).is_some() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "duplicate enum payload field",
                            field.span,
                        )));
                    }
                }
                let mut values = Vec::with_capacity(fields.len());
                for (name, field_ty) in &variant.fields {
                    let value = by_name.get(name).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            format!("missing enum payload field '{name}'").as_str(),
                            span,
                        ))
                    })?;
                    values.push(self.lower_expr_as_scalar(value, *field_ty)?);
                }
                values
            }
            _ => unreachable!("guarded enum constructor"),
        };
        let tag_slot = self.alloc_slot();
        self.emit(RuntimeInstr::Mov {
            dst: tag_slot,
            src: RuntimeOperand::Imm(variant.tag),
        });
        let mut payload_slots = Vec::with_capacity(layout.payload_slots);
        for index in 0..layout.payload_slots {
            let slot = self.alloc_slot();
            let value = values.get(index).cloned().unwrap_or(RuntimeOperand::Imm(0));
            self.emit(RuntimeInstr::Mov {
                dst: slot,
                src: value,
            });
            if let Some((_, field_ty)) = variant.fields.get(index) {
                self.normalize_scalar_slot(slot, *field_ty);
            }
            payload_slots.push(slot);
        }
        Ok(Some(RuntimeGenericBinding::EnumSlots {
            layout,
            tag_slot,
            payload_slots,
            mutable,
            owns_cleanup: true,
        }))
    }

    pub(super) fn runtime_enum_binding_for_expr(
        &self,
        expr: &Expr,
    ) -> Option<(RuntimeEnumLayout, usize, Vec<usize>)> {
        let Expr::Ident { name, .. } = expr else {
            return None;
        };
        match self.scopes.get(name) {
            Some(RuntimeGenericBinding::EnumSlots {
                layout,
                tag_slot,
                payload_slots,
                ..
            }) => Some((layout.clone(), *tag_slot, payload_slots.clone())),
            _ => None,
        }
    }

    pub(super) fn bind_runtime_enum_pattern(
        &mut self,
        pattern: &MatchPattern,
        variant: &RuntimeEnumVariantLayout,
        payload_slots: &[usize],
    ) -> RuntimeGenericLowerResult<()> {
        if let Some(resource) = &variant.resource {
            if payload_slots.len() < 4 {
                return Err(RuntimeGenericLowerError::Unsupported);
            }
            let binding_name = match pattern {
                MatchPattern::EnumTuple { bindings, span, .. } => {
                    if bindings.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "resource enum tuple pattern requires one binding",
                            *span,
                        )));
                    }
                    bindings[0]
                        .clone()
                        .unwrap_or_else(|| format!("__aziky_enum_drop_{}", variant.tag))
                }
                MatchPattern::EnumNamed { fields, span, .. } => {
                    let field = fields
                        .iter()
                        .find(|field| field.name == resource.name)
                        .ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("missing enum payload field '{}'", resource.name).as_str(),
                                *span,
                            ))
                        })?;
                    field
                        .binding
                        .clone()
                        .unwrap_or_else(|| format!("__aziky_enum_drop_{}", variant.tag))
                }
                _ => {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "resource enum payload must be explicitly destructured",
                        pattern.span(),
                    )));
                }
            };
            let binding = match &resource.kind {
                RuntimeEnumResourceKind::ListScalar(elem_ty) => {
                    RuntimeGenericBinding::OwnedListScalar {
                        ptr_slot: payload_slots[0],
                        len_slot: payload_slots[1],
                        capacity_slot: payload_slots[2],
                        allocation_bytes_slot: payload_slots[3],
                        mutable: false,
                        elem_ty: *elem_ty,
                    }
                }
                RuntimeEnumResourceKind::ListStruct(layout) => {
                    RuntimeGenericBinding::OwnedListStruct {
                        ptr_slot: payload_slots[0],
                        len_slot: payload_slots[1],
                        capacity_slot: payload_slots[2],
                        allocation_bytes_slot: payload_slots[3],
                        mutable: false,
                        layout: layout.clone(),
                    }
                }
                RuntimeEnumResourceKind::String => RuntimeGenericBinding::OwnedString {
                    ptr_slot: payload_slots[0],
                    len_slot: payload_slots[1],
                    capacity_slot: payload_slots[2],
                    allocation_bytes_slot: payload_slots[3],
                    mutable: false,
                    is_path: false,
                },
                RuntimeEnumResourceKind::Map(layout) => RuntimeGenericBinding::OwnedMap {
                    ptr_slot: payload_slots[0],
                    len_slot: payload_slots[1],
                    capacity_slot: payload_slots[2],
                    allocation_bytes_slot: payload_slots[3],
                    mutable: false,
                    layout: layout.clone(),
                },
            };
            self.scopes.insert(binding_name, binding);
            return Ok(());
        }
        if let Some(nested) = &variant.nested {
            if payload_slots.is_empty() {
                return Err(RuntimeGenericLowerError::Unsupported);
            }
            let binding_name = match pattern {
                MatchPattern::EnumTuple { bindings, span, .. } => {
                    if bindings.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "nested enum tuple pattern requires one binding",
                            *span,
                        )));
                    }
                    bindings[0].clone()
                }
                MatchPattern::EnumNamed { fields, span, .. } => fields
                    .iter()
                    .find(|field| field.name == nested.name)
                    .ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            format!("missing enum payload field '{}'", nested.name).as_str(),
                            *span,
                        ))
                    })?
                    .binding
                    .clone(),
                _ => None,
            };
            if let Some(binding_name) = binding_name {
                self.scopes.insert(
                    binding_name,
                    RuntimeGenericBinding::EnumSlots {
                        layout: nested.layout.as_ref().clone(),
                        tag_slot: payload_slots[0],
                        payload_slots: payload_slots
                            [1..(1 + nested.layout.payload_slots).min(payload_slots.len())]
                            .to_vec(),
                        mutable: false,
                        owns_cleanup: true,
                    },
                );
            }
            return Ok(());
        }
        match pattern {
            MatchPattern::Wildcard { .. } | MatchPattern::EnumUnit { .. } => Ok(()),
            MatchPattern::EnumTuple { bindings, span, .. } => {
                if bindings.len() != variant.fields.len() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "enum tuple pattern arity mismatch",
                        *span,
                    )));
                }
                for (index, binding) in bindings.iter().enumerate() {
                    let Some(name) = binding else { continue };
                    let (_, ty) = variant.fields[index];
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::Scalar {
                            slot: payload_slots[index],
                            mutable: false,
                            ty,
                        },
                    );
                }
                Ok(())
            }
            MatchPattern::EnumNamed { fields, span, .. } => {
                for field in fields {
                    let Some(name) = &field.binding else { continue };
                    let index = variant
                        .fields
                        .iter()
                        .position(|(candidate, _)| candidate == &field.name)
                        .ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("unknown enum payload field '{}'", field.name).as_str(),
                                *span,
                            ))
                        })?;
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::Scalar {
                            slot: payload_slots[index],
                            mutable: false,
                            ty: variant.fields[index].1,
                        },
                    );
                }
                Ok(())
            }
        }
    }

    pub(super) fn validate_runtime_match(
        &self,
        layout: &RuntimeEnumLayout,
        arms: &[MatchArm],
        span: Span,
    ) -> RuntimeGenericLowerResult<Vec<Option<RuntimeEnumVariantLayout>>> {
        let enum_def = self.enums.get(&layout.enum_name).ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                "runtime match references an unknown enum",
                span,
            ))
        })?;
        let mut covered = HashSet::new();
        let mut wildcard_seen = false;
        let mut variants = Vec::with_capacity(arms.len());
        for (index, arm) in arms.iter().enumerate() {
            if matches!(arm.pattern, MatchPattern::Wildcard { .. }) {
                if layout.owns_resources() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "resource-bearing enum matches require explicit variant arms",
                        arm.pattern.span(),
                    )));
                }
                if wildcard_seen || index + 1 != arms.len() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "wildcard match arm must appear exactly once and be last",
                        arm.pattern.span(),
                    )));
                }
                wildcard_seen = true;
                variants.push(None);
                continue;
            }
            let variant_name = validate_enum_match_pattern(&arm.pattern, enum_def)
                .map_err(RuntimeGenericLowerError::Diagnostic)?;
            if !covered.insert(variant_name.to_string()) {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!(
                        "unreachable duplicate match arm for '{}::{}'",
                        layout.enum_name, variant_name
                    )
                    .as_str(),
                    arm.pattern.span(),
                )));
            }
            let variant = layout
                .variants
                .iter()
                .find(|variant| variant.name == variant_name)
                .ok_or(RuntimeGenericLowerError::Unsupported)?;
            variants.push(Some(variant.clone()));
        }
        if !wildcard_seen {
            let missing: Vec<String> = layout
                .variants
                .iter()
                .filter(|variant| !covered.contains(&variant.name))
                .map(|variant| format!("{}::{}", layout.enum_name, variant.name))
                .collect();
            if !missing.is_empty() {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!(
                        "non-exhaustive match for enum '{}'; missing {}",
                        layout.enum_name,
                        missing.join(", ")
                    )
                    .as_str(),
                    span,
                )));
            }
        }
        Ok(variants)
    }

    pub(super) fn lower_runtime_match_scalar(
        &mut self,
        value: &Expr,
        arms: &[MatchArm],
        expected: RuntimeScalarType,
        span: Span,
    ) -> RuntimeGenericLowerResult<RuntimeOperand> {
        let Some((layout, tag_slot, payload_slots)) = self.runtime_enum_binding_for_expr(value)
        else {
            return Err(RuntimeGenericLowerError::Unsupported);
        };
        if layout.owns_resources() {
            let Expr::Ident {
                name,
                span: value_span,
            } = value
            else {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "resource-bearing enum match requires a named current-scope owner",
                    value.span(),
                )));
            };
            let Some(RuntimeGenericBinding::EnumSlots {
                owns_cleanup: true, ..
            }) = self.scopes.get_current(name)
            else {
                self.reject_moved_resource(name, *value_span)?;
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "resource-bearing enum match requires a live current-scope owner",
                    *value_span,
                )));
            };
            let _ = self.scopes.take_current(name);
            self.scopes.insert(
                name.clone(),
                RuntimeGenericBinding::MovedResource {
                    kind: "resource enum owner",
                },
            );
        }
        let variants = self.validate_runtime_match(&layout, arms, span)?;
        let dst = self.alloc_slot();
        let mut end_patches = Vec::new();
        for (arm, variant) in arms.iter().zip(variants) {
            let skip = variant.as_ref().map(|variant| {
                self.emit(RuntimeInstr::JumpIfCmpFalse {
                    op: RuntimeCmpOp::Eq,
                    lhs: RuntimeOperand::Slot(tag_slot),
                    rhs: RuntimeOperand::Imm(variant.tag),
                    target: usize::MAX,
                })
            });
            self.scopes.push();
            if let Some(variant) = variant.as_ref() {
                self.bind_runtime_enum_pattern(&arm.pattern, variant, &payload_slots)?;
            }
            let value = self.lower_expr_as_scalar(&arm.expr, expected)?;
            self.emit(RuntimeInstr::Mov { dst, src: value });
            self.normalize_scalar_slot(dst, expected);
            self.pop_scope_with_cleanup();
            end_patches.push(self.emit(RuntimeInstr::Jump { target: usize::MAX }));
            if let Some(skip) = skip {
                let next = self.instrs.len();
                self.patch_target(skip, next)?;
            }
        }
        let end = self.instrs.len();
        for patch in end_patches {
            self.patch_target(patch, end)?;
        }
        Ok(RuntimeOperand::Slot(dst))
    }

    pub(super) fn infer_runtime_match_scalar_type(
        &self,
        value: &Expr,
        arms: &[MatchArm],
        span: Span,
    ) -> RuntimeGenericLowerResult<RuntimeScalarType> {
        let Some((layout, _, payload_slots)) = self.runtime_enum_binding_for_expr(value) else {
            return Err(RuntimeGenericLowerError::Unsupported);
        };
        let variants = self.validate_runtime_match(&layout, arms, span)?;
        let mut inferred = None;
        for (arm, variant) in arms.iter().zip(variants) {
            let mut probe = self.clone();
            probe.scopes.push();
            if let Some(variant) = variant.as_ref() {
                probe.bind_runtime_enum_pattern(&arm.pattern, variant, &payload_slots)?;
            }
            let arm_ty = probe.infer_expr_scalar_type(&arm.expr)?;
            if inferred.is_some_and(|expected| expected != arm_ty) {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "runtime match arms must return one scalar type",
                    arm.span,
                )));
            }
            inferred = Some(arm_ty);
        }
        inferred.ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                "match requires at least one arm",
                span,
            ))
        })
    }

    /// Produces the leaf scalar fields of a struct in declaration order. Nested
    /// aggregates use qualified names internally, so an `Outer { inner: Inner }`
    /// has stable slots such as `inner.x` without introducing a pointer or an
    /// implicit allocation into the generated representation.
    pub(super) fn runtime_struct_scalar_fields(
        &self,
        struct_name: &str,
        span: Span,
    ) -> RuntimeGenericLowerResult<Vec<(String, RuntimeScalarType)>> {
        fn collect(
            layouts: &HashMap<String, Vec<LayoutField>>,
            struct_name: &str,
            prefix: &str,
            span: Span,
            out: &mut Vec<(String, RuntimeScalarType)>,
        ) -> RuntimeGenericLowerResult<()> {
            let fields = layouts.get(struct_name).ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("unknown struct '{struct_name}'").as_str(),
                    span,
                ))
            })?;
            for field in fields {
                let name = if prefix.is_empty() {
                    field.name.clone()
                } else {
                    format!("{prefix}.{}", field.name)
                };
                match &field.ty {
                    TypeName::Struct(child) => collect(layouts, child, &name, span, out)?,
                    ty => {
                        let scalar = ensure_runtime_generic_scalar_type(ty, span).map_err(|_| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                format!(
                                    "runtime native struct-list field '{name}' must be scalar or a nested struct"
                                )
                                .as_str(),
                                span,
                            ))
                        })?;
                        out.push((name, scalar));
                    }
                }
            }
            Ok(())
        }

        let mut fields = Vec::new();
        collect(self.struct_layouts, struct_name, "", span, &mut fields)?;
        Ok(fields)
    }

    pub(super) fn lower_runtime_owned_struct_literal(
        &mut self,
        struct_name: &str,
        init_fields: &[StructInitField],
        mutable: bool,
        span: Span,
    ) -> RuntimeGenericLowerResult<RuntimeGenericBinding> {
        let binding = self.allocate_runtime_owned_struct_binding(struct_name, mutable, span)?;
        let RuntimeGenericBinding::OwnedStruct {
            scalar_fields,
            list_fields,
            ..
        } = &binding
        else {
            return Err(RuntimeGenericLowerError::Unsupported);
        };
        let scalar_fields = scalar_fields.clone();
        let list_fields = list_fields.clone();

        fn initialize(
            builder: &mut RuntimeGenericBuilder<'_>,
            struct_name: &str,
            fields: &[StructInitField],
            prefix: &str,
            span: Span,
            scalars: &HashMap<String, (usize, RuntimeScalarType)>,
            lists: &HashMap<String, RuntimeOwnedStructListField>,
        ) -> RuntimeGenericLowerResult<()> {
            let layout = builder
                .struct_layouts
                .get(struct_name)
                .cloned()
                .ok_or(RuntimeGenericLowerError::Unsupported)?;
            let mut by_name = HashMap::with_capacity(fields.len());
            for field in fields {
                if by_name.insert(field.name.as_str(), &field.expr).is_some() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("duplicate field '{}' in struct literal", field.name).as_str(),
                        field.span,
                    )));
                }
            }
            for field in layout {
                let value = by_name.get(field.name.as_str()).copied().ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("missing field '{}' in struct literal", field.name).as_str(),
                        span,
                    ))
                })?;
                let path = if prefix.is_empty() {
                    field.name.clone()
                } else {
                    format!("{prefix}.{}", field.name)
                };
                match &field.ty {
                    TypeName::Struct(child) => {
                        let Expr::StructInit {
                            name: literal_name,
                            fields: child_fields,
                            ..
                        } = value
                        else {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("nested field '{path}' requires a struct literal").as_str(),
                                value.span(),
                            )));
                        };
                        if literal_name != child {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("nested field '{path}' has the wrong struct type").as_str(),
                                value.span(),
                            )));
                        }
                        initialize(builder, child, child_fields, &path, span, scalars, lists)?;
                    }
                    TypeName::List { .. } => {
                        let Expr::ArrayLit { elems, .. } = value else {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("resource-owning field '{path}' requires a list literal")
                                    .as_str(),
                                value.span(),
                            )));
                        };
                        let descriptor = lists
                            .get(&path)
                            .cloned()
                            .ok_or(RuntimeGenericLowerError::Unsupported)?;
                        match descriptor {
                            RuntimeOwnedStructListField::Scalar {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                elem_ty,
                            } => {
                                for slot in
                                    [ptr_slot, len_slot, capacity_slot, allocation_bytes_slot]
                                {
                                    builder.emit(RuntimeInstr::Mov {
                                        dst: slot,
                                        src: RuntimeOperand::Imm(0),
                                    });
                                }
                                for elem in elems {
                                    let value = builder.lower_expr_as_scalar(elem, elem_ty)?;
                                    builder.emit_owned_list_scalar_push(
                                        ptr_slot,
                                        len_slot,
                                        capacity_slot,
                                        allocation_bytes_slot,
                                        value,
                                        elem_ty,
                                    )?;
                                }
                            }
                            RuntimeOwnedStructListField::Struct {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                layout,
                            } => {
                                for slot in
                                    [ptr_slot, len_slot, capacity_slot, allocation_bytes_slot]
                                {
                                    builder.emit(RuntimeInstr::Mov {
                                        dst: slot,
                                        src: RuntimeOperand::Imm(0),
                                    });
                                }
                                for elem in elems {
                                    let values = builder
                                        .lower_runtime_struct_operands(elem, &layout, span)?;
                                    builder.emit_owned_list_struct_push(
                                        ptr_slot,
                                        len_slot,
                                        capacity_slot,
                                        allocation_bytes_slot,
                                        &layout,
                                        values,
                                    )?;
                                }
                            }
                        }
                    }
                    _ => {
                        let (slot, ty) = scalars
                            .get(&path)
                            .copied()
                            .ok_or(RuntimeGenericLowerError::Unsupported)?;
                        let source = builder.lower_expr_as_scalar(value, ty)?;
                        builder.emit(RuntimeInstr::Mov {
                            dst: slot,
                            src: source,
                        });
                        builder.normalize_scalar_slot(slot, ty);
                    }
                }
            }
            Ok(())
        }
        initialize(
            self,
            struct_name,
            init_fields,
            "",
            span,
            &scalar_fields,
            &list_fields,
        )?;
        Ok(binding)
    }

    pub(super) fn allocate_runtime_owned_struct_binding(
        &mut self,
        struct_name: &str,
        mutable: bool,
        span: Span,
    ) -> RuntimeGenericLowerResult<RuntimeGenericBinding> {
        let mut scalar_fields = HashMap::new();
        let mut list_fields = HashMap::new();
        fn collect(
            builder: &mut RuntimeGenericBuilder<'_>,
            struct_name: &str,
            prefix: &str,
            span: Span,
            visiting: &mut Vec<String>,
            scalars: &mut HashMap<String, (usize, RuntimeScalarType)>,
            lists: &mut HashMap<String, RuntimeOwnedStructListField>,
        ) -> RuntimeGenericLowerResult<()> {
            if visiting.iter().any(|name| name == struct_name) {
                let mut cycle = visiting.clone();
                cycle.push(struct_name.to_string());
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!(
                        "recursive resource layout is not allowed: {}",
                        cycle.join(" -> ")
                    )
                    .as_str(),
                    span,
                )));
            }
            let fields = builder
                .struct_layouts
                .get(struct_name)
                .cloned()
                .ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("unknown struct '{struct_name}'").as_str(),
                        span,
                    ))
                })?;
            visiting.push(struct_name.to_string());
            for field in fields {
                let path = if prefix.is_empty() {
                    field.name.clone()
                } else {
                    format!("{prefix}.{}", field.name)
                };
                match &field.ty {
                    TypeName::Struct(child) => {
                        collect(builder, child, &path, span, visiting, scalars, lists)?
                    }
                    TypeName::List { elem } => {
                        let ptr_slot = builder.alloc_slot();
                        let len_slot = builder.alloc_slot();
                        let capacity_slot = builder.alloc_slot();
                        let allocation_bytes_slot = builder.alloc_slot();
                        let binding = if let TypeName::Struct(element_struct) = elem.as_ref() {
                            RuntimeOwnedStructListField::Struct {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                layout: builder.runtime_struct_list_layout(element_struct, span)?,
                            }
                        } else {
                            RuntimeOwnedStructListField::Scalar {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                elem_ty: ensure_runtime_generic_scalar_type(elem, span)
                                    .map_err(RuntimeGenericLowerError::Diagnostic)?,
                            }
                        };
                        lists.insert(path, binding);
                    }
                    field_ty => {
                        scalars.insert(
                            path,
                            (
                                builder.alloc_slot(),
                                ensure_runtime_generic_scalar_type(field_ty, span)
                                    .map_err(RuntimeGenericLowerError::Diagnostic)?,
                            ),
                        );
                    }
                }
            }
            visiting.pop();
            Ok(())
        }
        collect(
            self,
            struct_name,
            "",
            span,
            &mut Vec::new(),
            &mut scalar_fields,
            &mut list_fields,
        )?;
        Ok(RuntimeGenericBinding::OwnedStruct {
            struct_name: struct_name.to_string(),
            scalar_fields,
            list_fields,
            mutable,
            owns_cleanup: true,
        })
    }

    pub(super) fn struct_has_direct_owned_list(&self, struct_name: &str) -> bool {
        fn visit(
            layouts: &HashMap<String, Vec<LayoutField>>,
            name: &str,
            visiting: &mut HashSet<String>,
        ) -> bool {
            if !visiting.insert(name.to_string()) {
                return true;
            }
            let result = layouts.get(name).is_some_and(|fields| {
                fields.iter().any(|field| match &field.ty {
                    TypeName::List { .. } => true,
                    TypeName::Struct(child) => visit(layouts, child, visiting),
                    _ => false,
                })
            });
            visiting.remove(name);
            result
        }
        visit(self.struct_layouts, struct_name, &mut HashSet::new())
    }

    pub(super) fn owned_struct_list_field(
        &self,
        expr: &Expr,
    ) -> Option<RuntimeOwnedStructListField> {
        let Expr::FieldAccess { base, field, .. } = expr else {
            return None;
        };
        fn root_and_path<'a>(expr: &'a Expr, tail: &str) -> Option<(&'a str, String)> {
            match expr {
                Expr::Ident { name, .. } => Some((name, tail.to_string())),
                Expr::FieldAccess { base, field, .. } => {
                    root_and_path(base, &format!("{field}.{tail}"))
                }
                _ => None,
            }
        }
        let (name, path) = root_and_path(base, field)?;
        match self.scopes.get(name)? {
            RuntimeGenericBinding::OwnedStruct { list_fields, .. } => {
                list_fields.get(&path).cloned()
            }
            _ => None,
        }
    }

    pub(super) fn emit_owned_struct_descriptor_move(
        &mut self,
        source: &RuntimeGenericBinding,
        destination: &RuntimeGenericBinding,
        span: Span,
    ) -> RuntimeGenericLowerResult<()> {
        let (
            RuntimeGenericBinding::OwnedStruct {
                struct_name: source_name,
                scalar_fields: source_scalars,
                list_fields: source_lists,
                ..
            },
            RuntimeGenericBinding::OwnedStruct {
                struct_name: destination_name,
                scalar_fields: destination_scalars,
                list_fields: destination_lists,
                ..
            },
        ) = (source, destination)
        else {
            return Err(RuntimeGenericLowerError::Unsupported);
        };
        if source_name != destination_name
            || source_scalars.len() != destination_scalars.len()
            || source_lists.len() != destination_lists.len()
        {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                "resource-owning struct layout does not match destination",
                span,
            )));
        }
        for (name, (source_slot, source_ty)) in source_scalars {
            let Some((destination_slot, destination_ty)) = destination_scalars.get(name) else {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "resource-owning struct scalar fields do not match destination",
                    span,
                )));
            };
            if source_ty != destination_ty {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "resource-owning struct scalar field type does not match destination",
                    span,
                )));
            }
            self.emit(RuntimeInstr::Mov {
                dst: *destination_slot,
                src: RuntimeOperand::Slot(*source_slot),
            });
            self.normalize_scalar_slot(*destination_slot, *destination_ty);
        }
        for (name, source_field) in source_lists {
            let Some(destination_field) = destination_lists.get(name) else {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "resource-owning struct list fields do not match destination",
                    span,
                )));
            };
            let (source_slots, destination_slots) = match (source_field, destination_field) {
                (
                    RuntimeOwnedStructListField::Scalar {
                        ptr_slot: source_ptr,
                        len_slot: source_len,
                        capacity_slot: source_capacity,
                        allocation_bytes_slot: source_bytes,
                        elem_ty: source_ty,
                    },
                    RuntimeOwnedStructListField::Scalar {
                        ptr_slot: destination_ptr,
                        len_slot: destination_len,
                        capacity_slot: destination_capacity,
                        allocation_bytes_slot: destination_bytes,
                        elem_ty: destination_ty,
                    },
                ) if source_ty == destination_ty => (
                    [*source_ptr, *source_len, *source_capacity, *source_bytes],
                    [
                        *destination_ptr,
                        *destination_len,
                        *destination_capacity,
                        *destination_bytes,
                    ],
                ),
                (
                    RuntimeOwnedStructListField::Struct {
                        ptr_slot: source_ptr,
                        len_slot: source_len,
                        capacity_slot: source_capacity,
                        allocation_bytes_slot: source_bytes,
                        layout: source_layout,
                    },
                    RuntimeOwnedStructListField::Struct {
                        ptr_slot: destination_ptr,
                        len_slot: destination_len,
                        capacity_slot: destination_capacity,
                        allocation_bytes_slot: destination_bytes,
                        layout: destination_layout,
                    },
                ) if source_layout == destination_layout => (
                    [*source_ptr, *source_len, *source_capacity, *source_bytes],
                    [
                        *destination_ptr,
                        *destination_len,
                        *destination_capacity,
                        *destination_bytes,
                    ],
                ),
                _ => {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "resource-owning struct list field type does not match destination",
                        span,
                    )));
                }
            };
            for (dst, src) in destination_slots.into_iter().zip(source_slots) {
                self.emit(RuntimeInstr::Mov {
                    dst,
                    src: RuntimeOperand::Slot(src),
                });
            }
        }
        Ok(())
    }

    pub(super) fn lower_runtime_struct_operands(
        &mut self,
        expr: &Expr,
        layout: &RuntimeStructListLayout,
        span: Span,
    ) -> RuntimeGenericLowerResult<Vec<(RuntimeStructFieldLayout, RuntimeOperand)>> {
        if let Expr::StructInit { name, .. } = expr
            && name != &layout.struct_name
        {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!(
                    "runtime struct-list element requires '{}', found '{name}'",
                    layout.struct_name
                )
                .as_str(),
                span,
            )));
        }
        let mut target_fields = HashMap::with_capacity(layout.fields.len());
        for field in &layout.fields {
            target_fields.insert(field.name.clone(), (self.alloc_slot(), field.ty));
        }
        self.lower_struct_slot_assign(&layout.struct_name, &target_fields, expr, span)?;
        Ok(layout
            .fields
            .iter()
            .map(|field| {
                let (slot, _) = target_fields
                    .get(&field.name)
                    .copied()
                    .expect("layout field slot");
                (field.clone(), RuntimeOperand::Slot(slot))
            })
            .collect())
    }

    pub(super) fn struct_field_index(
        &mut self,
        element_index: RuntimeOperand,
        layout: &RuntimeStructListLayout,
        field: &RuntimeStructFieldLayout,
    ) -> RuntimeOperand {
        let field_bytes = u64::from(field.ty.storage_bytes());
        let element_units = layout.stride_bytes / field_bytes;
        let field_units = field.offset_bytes / field_bytes;
        if let RuntimeOperand::Imm(index) = &element_index
            && let Some(index) = index
                .checked_mul(element_units)
                .and_then(|index| index.checked_add(field_units))
        {
            return RuntimeOperand::Imm(index);
        }
        let mut index = element_index;
        if element_units != 1 {
            let scaled = self.alloc_slot();
            self.emit(RuntimeInstr::BinOp {
                dst: scaled,
                op: RuntimeBinOp::Mul,
                lhs: index,
                rhs: RuntimeOperand::Imm(element_units),
            });
            index = RuntimeOperand::Slot(scaled);
        }
        if field_units != 0 {
            let offset = self.alloc_slot();
            self.emit(RuntimeInstr::BinOp {
                dst: offset,
                op: RuntimeBinOp::Add,
                lhs: index,
                rhs: RuntimeOperand::Imm(field_units),
            });
            index = RuntimeOperand::Slot(offset);
        }
        index
    }

    pub(super) fn emit_owned_list_struct_push(
        &mut self,
        ptr_slot: usize,
        len_slot: usize,
        capacity_slot: usize,
        allocation_bytes_slot: usize,
        layout: &RuntimeStructListLayout,
        values: Vec<(RuntimeStructFieldLayout, RuntimeOperand)>,
    ) -> RuntimeGenericLowerResult<()> {
        self.emit_guard_failure_exit(
            RuntimeCmpOp::Ne,
            RuntimeOperand::Slot(len_slot),
            RuntimeOperand::Imm(u64::MAX),
            101,
        )?;
        let required_slot = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: required_slot,
            op: RuntimeBinOp::Add,
            lhs: RuntimeOperand::Slot(len_slot),
            rhs: RuntimeOperand::Imm(1),
        });
        self.emit_owned_list_ensure_capacity(
            ptr_slot,
            len_slot,
            capacity_slot,
            allocation_bytes_slot,
            RuntimeOperand::Slot(required_slot),
            layout.stride_bytes,
        )?;
        for (field, value) in values {
            let index = self.struct_field_index(RuntimeOperand::Slot(len_slot), layout, &field);
            let value = self.canonicalize_scalar_operand(value, field.ty);
            self.emit(RuntimeInstr::HeapStoreInt {
                ptr: RuntimeOperand::Slot(ptr_slot),
                index,
                src: value,
                bytes: field.ty.storage_bytes(),
            });
        }
        self.emit(RuntimeInstr::Mov {
            dst: len_slot,
            src: RuntimeOperand::Slot(required_slot),
        });
        Ok(())
    }

    pub(super) fn native_option_scalar_type(&self, expr: &Expr) -> Option<RuntimeScalarType> {
        match expr {
            Expr::Ident { name, .. } => self.scopes.get(name).and_then(|binding| {
                if let Some((_, _, _, elem_ty)) = binding.as_option_scalar() {
                    return Some(elem_ty);
                }
                let RuntimeGenericBinding::EnumSlots { layout, .. } = binding else {
                    return None;
                };
                if layout.enum_name != "Option" {
                    return None;
                }
                let some = layout
                    .variants
                    .iter()
                    .find(|variant| variant.name == "Some")?;
                match some.fields.as_slice() {
                    [(_, elem_ty)] => Some(*elem_ty),
                    _ => None,
                }
            }),
            Expr::EnumVariant { .. } => None,
            Expr::EnumTupleVariant {
                enum_name,
                variant,
                args,
                ..
            } if enum_name == "Option" && variant == "Some" && args.len() == 1 => {
                self.infer_expr_scalar_type(&args[0]).ok()
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                ..
            } if name == "get"
                && args.len() == 1
                && matches!(
                    receiver.as_ref(),
                    Expr::Ident { name, .. }
                        if self.scopes.get(name).and_then(RuntimeGenericBinding::as_owned_map).is_some()
                ) =>
            {
                let Expr::Ident { name, .. } = receiver.as_ref() else {
                    return None;
                };
                self.scopes
                    .get(name)
                    .and_then(RuntimeGenericBinding::as_owned_map)
                    .map(|(_, _, _, _, _, layout)| layout.value_ty)
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                ..
            } if name == "char_at" && args.len() == 1 => {
                let Expr::Ident { name, .. } = receiver.as_ref() else {
                    return None;
                };
                self.scopes
                    .get(name)
                    .and_then(RuntimeGenericBinding::as_owned_string)
                    .map(|_| {
                        RuntimeScalarType::Int(RuntimeIntType {
                            signed: false,
                            bits: 32,
                        })
                    })
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                ..
            } if matches!(name.as_str(), "get" | "first" | "last" | "peek" | "pop")
                && ((name == "get" && args.len() == 1) || (name != "get" && args.is_empty())) =>
            {
                let Expr::Ident { name, .. } = receiver.as_ref() else {
                    return None;
                };
                self.scopes
                    .get(name)
                    .and_then(RuntimeGenericBinding::as_owned_list_scalar)
                    .map(|(_, _, _, _, _, elem_ty)| elem_ty)
            }
            _ => None,
        }
    }

    pub(super) fn native_result_ok_scalar_type(&self, expr: &Expr) -> Option<RuntimeScalarType> {
        let Expr::Ident { name, .. } = expr else {
            return None;
        };
        let RuntimeGenericBinding::EnumSlots { layout, .. } = self.scopes.get(name)? else {
            return None;
        };
        if layout.enum_name != "Result" {
            return None;
        }
        let ok = layout
            .variants
            .iter()
            .find(|variant| variant.name == "Ok")?;
        match ok.fields.as_slice() {
            [(_, ty)] => Some(*ty),
            _ => None,
        }
    }

    pub(super) fn native_result_ok_scalar_parts(
        &self,
        expr: &Expr,
    ) -> Option<(usize, usize, u64, RuntimeScalarType)> {
        let Expr::Ident { name, .. } = expr else {
            return None;
        };
        let RuntimeGenericBinding::EnumSlots {
            layout,
            tag_slot,
            payload_slots,
            ..
        } = self.scopes.get(name)?
        else {
            return None;
        };
        if layout.enum_name != "Result" {
            return None;
        }
        let ok = layout
            .variants
            .iter()
            .find(|variant| variant.name == "Ok")?;
        let [(_, ty)] = ok.fields.as_slice() else {
            return None;
        };
        Some((*tag_slot, *payload_slots.first()?, ok.tag, *ty))
    }

    pub(super) fn is_native_option_scalar_expr(&self, expr: &Expr) -> bool {
        self.native_option_scalar_type(expr).is_some()
            || matches!(
                expr,
                Expr::EnumVariant {
                    enum_name,
                    variant,
                    ..
                } if enum_name == "Option" && variant == "None"
            )
    }

    pub(super) fn lower_expr_as_option_scalar(
        &mut self,
        expr: &Expr,
        expected_elem_ty: Option<RuntimeScalarType>,
    ) -> RuntimeGenericLowerResult<(RuntimeOperand, RuntimeOperand, RuntimeScalarType)> {
        match expr {
            Expr::Ident { name, span } => {
                let (tag_slot, payload_slot, elem_ty) = match self.scopes.get(name) {
                    Some(binding) => {
                        if let Some((tag_slot, payload_slot, _, elem_ty)) =
                            binding.as_option_scalar()
                        {
                            (tag_slot, payload_slot, elem_ty)
                        } else if let RuntimeGenericBinding::EnumSlots {
                            layout,
                            tag_slot,
                            payload_slots,
                            ..
                        } = binding
                        {
                            let some = layout
                                .variants
                                .iter()
                                .find(|variant| variant.name == "Some");
                            match (layout.enum_name.as_str(), some, payload_slots.as_slice()) {
                                ("Option", Some(some), [payload_slot])
                                    if some.fields.len() == 1 =>
                                {
                                    (*tag_slot, *payload_slot, some.fields[0].1)
                                }
                                _ => {
                                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                        "runtime expression is not a scalar Option<T>",
                                        *span,
                                    )));
                                }
                            }
                        } else {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "runtime expression is not a scalar Option<T>",
                                *span,
                            )));
                        }
                    }
                    None => {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime expression is not a scalar Option<T>",
                            *span,
                        )));
                    }
                };
                if expected_elem_ty.is_some_and(|expected| expected != elem_ty) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime Option scalar payload type mismatch",
                        *span,
                    )));
                }
                Ok((
                    RuntimeOperand::Slot(tag_slot),
                    RuntimeOperand::Slot(payload_slot),
                    elem_ty,
                ))
            }
            Expr::EnumVariant {
                enum_name,
                variant,
                span,
            } if enum_name == "Option" && variant == "None" => {
                let elem_ty = expected_elem_ty.ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(type_error(
                        "Option::None requires an explicit Option<T> type",
                        *span,
                    ))
                })?;
                Ok((RuntimeOperand::Imm(0), RuntimeOperand::Imm(0), elem_ty))
            }
            Expr::EnumTupleVariant {
                enum_name,
                variant,
                args,
                span,
            } if enum_name == "Option" && variant == "Some" => {
                if args.len() != 1 {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "Option::Some expects exactly one payload",
                        *span,
                    )));
                }
                let elem_ty = if let Some(expected) = expected_elem_ty {
                    expected
                } else {
                    self.infer_expr_scalar_type(&args[0])?
                };
                let payload = self.lower_expr_as_scalar(&args[0], elem_ty)?;
                Ok((RuntimeOperand::Imm(1), payload, elem_ty))
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if name == "get"
                && matches!(
                    receiver.as_ref(),
                    Expr::Ident { name, .. }
                        if self.scopes.get(name).and_then(RuntimeGenericBinding::as_owned_map).is_some()
                ) =>
            {
                if args.len() != 1 {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "map get() expects exactly one key argument",
                        *span,
                    )));
                }
                let Expr::Ident {
                    name: receiver_name,
                    ..
                } = receiver.as_ref()
                else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                let Some((ptr_slot, len_slot, _, _, _, layout)) = self
                    .scopes
                    .get(receiver_name)
                    .and_then(RuntimeGenericBinding::as_owned_map)
                else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                if expected_elem_ty.is_some_and(|expected| expected != layout.value_ty) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime map Option payload type mismatch",
                        *span,
                    )));
                }
                let key = self.lower_expr_as_scalar(&args[0], layout.key_ty)?;
                let (found, index) =
                    self.emit_runtime_map_find(ptr_slot, len_slot, &layout, key)?;
                let payload = self.alloc_slot();
                self.emit(RuntimeInstr::Mov {
                    dst: payload,
                    src: RuntimeOperand::Imm(0),
                });
                let absent = self.emit(RuntimeInstr::JumpIfZero {
                    cond_slot: found,
                    target: usize::MAX,
                });
                let value_index = self.runtime_map_field_index(
                    RuntimeOperand::Slot(index),
                    &layout,
                    layout.value_offset_bytes,
                    layout.value_ty,
                );
                self.emit(RuntimeInstr::HeapLoadInt {
                    dst: payload,
                    ptr: RuntimeOperand::Slot(ptr_slot),
                    index: value_index,
                    bytes: layout.value_ty.storage_bytes(),
                });
                self.normalize_scalar_slot(payload, layout.value_ty);
                let done = self.instrs.len();
                self.patch_target(absent, done)?;
                Ok((
                    RuntimeOperand::Slot(found),
                    RuntimeOperand::Slot(payload),
                    layout.value_ty,
                ))
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if name == "char_at" => {
                if args.len() != 1 {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "char_at() expects exactly one scalar index",
                        *span,
                    )));
                }
                let Expr::Ident {
                    name: receiver_name,
                    ..
                } = receiver.as_ref()
                else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                let Some((ptr_slot, len_slot, _, _, _)) = self
                    .scopes
                    .get(receiver_name)
                    .and_then(RuntimeGenericBinding::as_owned_string)
                else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                let char_ty = RuntimeScalarType::Int(RuntimeIntType::new(false, 32)?);
                if expected_elem_ty.is_some_and(|expected| expected != char_ty) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "char_at() produces Option<char>",
                        *span,
                    )));
                }
                let index_ty = self.infer_expr_int_type(&args[0])?;
                if index_ty.signed {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "char_at() index must be unsigned",
                        args[0].span(),
                    )));
                }
                let index = self.lower_expr_as_type(&args[0], index_ty)?;
                let (tag, payload) = self.emit_owned_string_char_at(ptr_slot, len_slot, index)?;
                Ok((tag, payload, char_ty))
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if matches!(name.as_str(), "get" | "first" | "last" | "peek" | "pop") => {
                let Expr::Ident {
                    name: receiver_name,
                    ..
                } = receiver.as_ref()
                else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                let Some((ptr_slot, len_slot, _, _, mutable, elem_ty)) = self
                    .scopes
                    .get(receiver_name)
                    .and_then(RuntimeGenericBinding::as_owned_list_scalar)
                else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                if expected_elem_ty.is_some_and(|expected| expected != elem_ty) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime list Option payload type mismatch",
                        *span,
                    )));
                }

                if name == "get" {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "get() expects exactly one index argument",
                            *span,
                        )));
                    }
                } else if !args.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("{name}() expects no arguments").as_str(),
                        *span,
                    )));
                }
                if name == "pop" && !mutable {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("cannot mutate immutable '{receiver_name}'").as_str(),
                        *span,
                    )));
                }

                let tag_slot = self.alloc_slot();
                let payload_slot = self.alloc_slot();
                self.emit(RuntimeInstr::Mov {
                    dst: payload_slot,
                    src: RuntimeOperand::Imm(0),
                });

                let index = if name == "get" {
                    let index_ty = self
                        .infer_expr_int_type(&args[0])
                        .map_err(|err| match err {
                            RuntimeGenericLowerError::Unsupported => {
                                RuntimeGenericLowerError::Diagnostic(type_error(
                                    "get() index must be an integer",
                                    *span,
                                ))
                            }
                            other => other,
                        })?;
                    let index = self.lower_expr_as_type(&args[0], index_ty)?;
                    self.emit(RuntimeInstr::Cmp {
                        dst: tag_slot,
                        op: RuntimeCmpOp::LtUnsigned,
                        lhs: index,
                        rhs: RuntimeOperand::Slot(len_slot),
                    });
                    index
                } else {
                    self.emit(RuntimeInstr::Cmp {
                        dst: tag_slot,
                        op: RuntimeCmpOp::GtUnsigned,
                        lhs: RuntimeOperand::Slot(len_slot),
                        rhs: RuntimeOperand::Imm(0),
                    });
                    RuntimeOperand::Imm(0)
                };

                let absent = self.emit(RuntimeInstr::JumpIfCmpFalse {
                    op: RuntimeCmpOp::Ne,
                    lhs: RuntimeOperand::Slot(tag_slot),
                    rhs: RuntimeOperand::Imm(0),
                    target: usize::MAX,
                });
                let load_index = match name.as_str() {
                    "get" | "first" => index,
                    "last" | "peek" => {
                        let last_slot = self.alloc_slot();
                        self.emit(RuntimeInstr::BinOp {
                            dst: last_slot,
                            op: RuntimeBinOp::Sub,
                            lhs: RuntimeOperand::Slot(len_slot),
                            rhs: RuntimeOperand::Imm(1),
                        });
                        RuntimeOperand::Slot(last_slot)
                    }
                    "pop" => {
                        self.emit(RuntimeInstr::BinOpInPlace {
                            dst: len_slot,
                            op: RuntimeBinOp::Sub,
                            rhs: RuntimeOperand::Imm(1),
                        });
                        RuntimeOperand::Slot(len_slot)
                    }
                    _ => unreachable!("guarded option-producing list method"),
                };
                self.emit(RuntimeInstr::HeapLoadInt {
                    dst: payload_slot,
                    ptr: RuntimeOperand::Slot(ptr_slot),
                    index: load_index,
                    bytes: elem_ty.storage_bytes(),
                });
                self.normalize_scalar_slot(payload_slot, elem_ty);
                let done = self.instrs.len();
                self.patch_target(absent, done)?;
                Ok((
                    RuntimeOperand::Slot(tag_slot),
                    RuntimeOperand::Slot(payload_slot),
                    elem_ty,
                ))
            }
            _ => Err(RuntimeGenericLowerError::Unsupported),
        }
    }

    pub(super) fn native_option_struct_layout(
        &self,
        expr: &Expr,
    ) -> Option<RuntimeStructListLayout> {
        match expr {
            Expr::Ident { name, .. } => self
                .scopes
                .get(name)
                .and_then(RuntimeGenericBinding::as_option_struct)
                .map(|(_, _, _, layout)| layout),
            Expr::MethodCall {
                receiver,
                name,
                args,
                ..
            } if matches!(name.as_str(), "get" | "first" | "last" | "peek" | "pop")
                && ((name == "get" && args.len() == 1) || (name != "get" && args.is_empty())) =>
            {
                let Expr::Ident { name, .. } = receiver.as_ref() else {
                    return None;
                };
                self.scopes
                    .get(name)
                    .and_then(RuntimeGenericBinding::as_owned_list_struct)
                    .map(|(_, _, _, _, _, layout)| layout)
            }
            _ => None,
        }
    }

    pub(super) fn lower_expr_as_option_struct(
        &mut self,
        expr: &Expr,
        expected_layout: Option<&RuntimeStructListLayout>,
    ) -> RuntimeGenericLowerResult<(
        RuntimeOperand,
        Vec<(RuntimeStructFieldLayout, RuntimeOperand)>,
        RuntimeStructListLayout,
    )> {
        match expr {
            Expr::Ident { name, span } => {
                let Some((tag_slot, fields, _, layout)) = self
                    .scopes
                    .get(name)
                    .and_then(RuntimeGenericBinding::as_option_struct)
                else {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime expression is not an aggregate Option<T>",
                        *span,
                    )));
                };
                if expected_layout.is_some_and(|expected| expected != &layout) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime Option struct payload type mismatch",
                        *span,
                    )));
                }
                let values = layout
                    .fields
                    .iter()
                    .map(|field| {
                        let (slot, _) = fields
                            .get(&field.name)
                            .copied()
                            .expect("option layout field slot");
                        (field.clone(), RuntimeOperand::Slot(slot))
                    })
                    .collect();
                Ok((RuntimeOperand::Slot(tag_slot), values, layout))
            }
            Expr::EnumVariant {
                enum_name,
                variant,
                span,
            } if enum_name == "Option" && variant == "None" => {
                let layout = expected_layout.cloned().ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(type_error(
                        "Option::None requires an explicit Option<Struct> type",
                        *span,
                    ))
                })?;
                let values = layout
                    .fields
                    .iter()
                    .map(|field| (field.clone(), RuntimeOperand::Imm(0)))
                    .collect();
                Ok((RuntimeOperand::Imm(0), values, layout))
            }
            Expr::EnumTupleVariant {
                enum_name,
                variant,
                args,
                span,
            } if enum_name == "Option" && variant == "Some" => {
                if args.len() != 1 {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "Option::Some expects exactly one payload",
                        *span,
                    )));
                }
                let layout = expected_layout.cloned().ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(type_error(
                        "aggregate Option::Some requires an explicit Option<Struct> type",
                        *span,
                    ))
                })?;
                let values = self.lower_runtime_struct_operands(&args[0], &layout, *span)?;
                Ok((RuntimeOperand::Imm(1), values, layout))
            }
            Expr::MethodCall {
                receiver,
                name,
                args,
                span,
            } if matches!(name.as_str(), "get" | "first" | "last" | "peek" | "pop") => {
                let Expr::Ident {
                    name: receiver_name,
                    ..
                } = receiver.as_ref()
                else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                let Some((ptr_slot, len_slot, _, _, mutable, layout)) = self
                    .scopes
                    .get(receiver_name)
                    .and_then(RuntimeGenericBinding::as_owned_list_struct)
                else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                if expected_layout.is_some_and(|expected| expected != &layout) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime list Option struct payload type mismatch",
                        *span,
                    )));
                }
                if name == "get" {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "get() expects exactly one index argument",
                            *span,
                        )));
                    }
                } else if !args.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("{name}() expects no arguments").as_str(),
                        *span,
                    )));
                }
                if name == "pop" && !mutable {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("cannot mutate immutable '{receiver_name}'").as_str(),
                        *span,
                    )));
                }

                let tag_slot = self.alloc_slot();
                let index = if name == "get" {
                    let index_ty = self
                        .infer_expr_int_type(&args[0])
                        .map_err(|err| match err {
                            RuntimeGenericLowerError::Unsupported => {
                                RuntimeGenericLowerError::Diagnostic(type_error(
                                    "get() index must be an integer",
                                    *span,
                                ))
                            }
                            other => other,
                        })?;
                    let index = self.lower_expr_as_type(&args[0], index_ty)?;
                    self.emit(RuntimeInstr::Cmp {
                        dst: tag_slot,
                        op: RuntimeCmpOp::LtUnsigned,
                        lhs: index.clone(),
                        rhs: RuntimeOperand::Slot(len_slot),
                    });
                    index
                } else {
                    self.emit(RuntimeInstr::Cmp {
                        dst: tag_slot,
                        op: RuntimeCmpOp::GtUnsigned,
                        lhs: RuntimeOperand::Slot(len_slot),
                        rhs: RuntimeOperand::Imm(0),
                    });
                    RuntimeOperand::Imm(0)
                };
                let mut values = Vec::with_capacity(layout.fields.len());
                for field in &layout.fields {
                    let slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: slot,
                        src: RuntimeOperand::Imm(0),
                    });
                    values.push((field.clone(), RuntimeOperand::Slot(slot)));
                }
                let absent = self.emit(RuntimeInstr::JumpIfCmpFalse {
                    op: RuntimeCmpOp::Ne,
                    lhs: RuntimeOperand::Slot(tag_slot),
                    rhs: RuntimeOperand::Imm(0),
                    target: usize::MAX,
                });
                let load_index = match name.as_str() {
                    "get" | "first" => index,
                    "last" | "peek" => {
                        let last_slot = self.alloc_slot();
                        self.emit(RuntimeInstr::BinOp {
                            dst: last_slot,
                            op: RuntimeBinOp::Sub,
                            lhs: RuntimeOperand::Slot(len_slot),
                            rhs: RuntimeOperand::Imm(1),
                        });
                        RuntimeOperand::Slot(last_slot)
                    }
                    "pop" => {
                        self.emit(RuntimeInstr::BinOpInPlace {
                            dst: len_slot,
                            op: RuntimeBinOp::Sub,
                            rhs: RuntimeOperand::Imm(1),
                        });
                        RuntimeOperand::Slot(len_slot)
                    }
                    _ => unreachable!("guarded option-producing struct-list method"),
                };
                for (field, operand) in &values {
                    let RuntimeOperand::Slot(slot) = operand else {
                        unreachable!("aggregate option payload uses slots");
                    };
                    let index = self.struct_field_index(load_index.clone(), &layout, field);
                    self.emit(RuntimeInstr::HeapLoadInt {
                        dst: *slot,
                        ptr: RuntimeOperand::Slot(ptr_slot),
                        index,
                        bytes: field.ty.storage_bytes(),
                    });
                    self.normalize_scalar_slot(*slot, field.ty);
                }
                let done = self.instrs.len();
                self.patch_target(absent, done)?;
                Ok((RuntimeOperand::Slot(tag_slot), values, layout))
            }
            _ => Err(RuntimeGenericLowerError::Unsupported),
        }
    }

    pub(super) fn patch_target(
        &mut self,
        instr_index: usize,
        target: usize,
    ) -> RuntimeGenericLowerResult<()> {
        match self.instrs.get_mut(instr_index) {
            Some(RuntimeInstr::Jump { target: slot }) => {
                *slot = target;
                Ok(())
            }
            Some(RuntimeInstr::JumpIfZero { target: slot, .. }) => {
                *slot = target;
                Ok(())
            }
            Some(RuntimeInstr::JumpIfCmpFalse { target: slot, .. }) => {
                *slot = target;
                Ok(())
            }
            Some(RuntimeInstr::Call { target: slot }) => {
                *slot = target;
                Ok(())
            }
            Some(RuntimeInstr::ThreadSpawn { target: slot, .. }) => {
                *slot = target;
                Ok(())
            }
            _ => Err(RuntimeGenericLowerError::Unsupported),
        }
    }

    pub(super) fn queue_function(&mut self, name: &str) {
        if self.lowered_functions.contains(name) {
            return;
        }
        if self.queued_functions.insert(name.to_string()) {
            self.pending_functions.push_back(name.to_string());
        }
    }

    pub(super) fn function_param_layout(
        &mut self,
        name: &str,
    ) -> RuntimeGenericLowerResult<Vec<RuntimeFunctionParamLayout>> {
        if let Some(layout) = self.function_param_slots.get(name) {
            return Ok(layout.clone());
        }

        let function = self.functions.get(name).ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(unknown_function_diagnostic(name, Span::new(0, 0)))
        })?;
        let mut out = Vec::with_capacity(function.params.len());
        for param in &function.params {
            if matches!(
                &param.ty,
                TypeName::Applied { name, args }
                    if name == "Sender"
                        && matches!(args.as_slice(), [TypeName::Int { signed: false, bits: 64 }])
            ) {
                out.push(RuntimeFunctionParamLayout::OwnedSender {
                    name: param.name.clone(),
                    handle_slot: self.alloc_slot(),
                });
                continue;
            }
            if matches!(
                &param.ty,
                TypeName::Applied { name, args }
                    if name == "Receiver"
                        && matches!(args.as_slice(), [TypeName::Int { signed: false, bits: 64 }])
            ) {
                out.push(RuntimeFunctionParamLayout::OwnedReceiver {
                    name: param.name.clone(),
                    handle_slot: self.alloc_slot(),
                });
                continue;
            }
            if matches!(param.ty, TypeName::File) {
                out.push(RuntimeFunctionParamLayout::OwnedFile {
                    name: param.name.clone(),
                    fd_slot: self.alloc_slot(),
                });
                continue;
            }
            if matches!(
                param.ty,
                TypeName::Ref {
                    mutable: true,
                    ref inner
                } if matches!(inner.as_ref(), TypeName::File)
            ) {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "File parameters support shared '&File' borrows; mutable descriptor borrows are not allowed",
                    param.span,
                )));
            }
            if matches!(
                param.ty,
                TypeName::Ref {
                    mutable: false,
                    ref inner
                } if matches!(inner.as_ref(), TypeName::File)
            ) {
                out.push(RuntimeFunctionParamLayout::BorrowedFile {
                    name: param.name.clone(),
                    fd_slot: self.alloc_slot(),
                });
                continue;
            }
            let enum_name = match &param.ty {
                TypeName::Struct(name) | TypeName::Applied { name, .. } => Some(name),
                _ => None,
            };
            if enum_name.is_some_and(|name| self.enums.contains_key(name)) {
                let layout = self.runtime_enum_layout(&param.ty, param.span)?;
                let tag_slot = self.alloc_slot();
                let payload_slots = (0..layout.payload_slots)
                    .map(|_| self.alloc_slot())
                    .collect();
                out.push(RuntimeFunctionParamLayout::Enum {
                    name: param.name.clone(),
                    layout,
                    tag_slot,
                    payload_slots,
                });
            } else if let TypeName::Ref { mutable, inner } = &param.ty
                && let TypeName::Struct(struct_name) = inner.as_ref()
            {
                if self.struct_has_direct_owned_list(struct_name) {
                    let mut binding = self.allocate_runtime_owned_struct_binding(
                        struct_name,
                        *mutable,
                        param.span,
                    )?;
                    let RuntimeGenericBinding::OwnedStruct { owns_cleanup, .. } = &mut binding
                    else {
                        return Err(RuntimeGenericLowerError::Unsupported);
                    };
                    *owns_cleanup = false;
                    out.push(RuntimeFunctionParamLayout::BorrowedOwnedStruct {
                        name: param.name.clone(),
                        binding,
                        mutable: *mutable,
                    });
                    continue;
                }
                let layout = self.runtime_struct_list_layout(struct_name, param.span)?;
                let mut fields = HashMap::with_capacity(layout.fields.len());
                for field in &layout.fields {
                    fields.insert(field.name.clone(), (self.alloc_slot(), field.ty));
                }
                out.push(RuntimeFunctionParamLayout::Struct {
                    name: param.name.clone(),
                    layout,
                    fields,
                    mutable: *mutable,
                    by_ref: true,
                });
            } else if let TypeName::Struct(struct_name) = &param.ty {
                if self.struct_has_direct_owned_list(struct_name) {
                    out.push(RuntimeFunctionParamLayout::OwnedStruct {
                        name: param.name.clone(),
                        binding: self.allocate_runtime_owned_struct_binding(
                            struct_name,
                            false,
                            param.span,
                        )?,
                    });
                    continue;
                }
                let layout = self.runtime_struct_list_layout(struct_name, param.span)?;
                let mut fields = HashMap::with_capacity(layout.fields.len());
                for field in &layout.fields {
                    fields.insert(field.name.clone(), (self.alloc_slot(), field.ty));
                }
                out.push(RuntimeFunctionParamLayout::Struct {
                    name: param.name.clone(),
                    layout,
                    fields,
                    mutable: false,
                    by_ref: false,
                });
            } else if matches!(param.ty, TypeName::String) {
                out.push(RuntimeFunctionParamLayout::OwnedString {
                    name: param.name.clone(),
                    ptr_slot: self.alloc_slot(),
                    len_slot: self.alloc_slot(),
                    capacity_slot: self.alloc_slot(),
                    allocation_bytes_slot: self.alloc_slot(),
                });
            } else if let TypeName::Map { key, value } = &param.ty {
                out.push(RuntimeFunctionParamLayout::OwnedMap {
                    name: param.name.clone(),
                    ptr_slot: self.alloc_slot(),
                    len_slot: self.alloc_slot(),
                    capacity_slot: self.alloc_slot(),
                    allocation_bytes_slot: self.alloc_slot(),
                    layout: self.runtime_map_layout(key, value, param.span)?,
                });
            } else if let TypeName::List { elem } = &param.ty {
                let ptr_slot = self.alloc_slot();
                let len_slot = self.alloc_slot();
                let capacity_slot = self.alloc_slot();
                let allocation_bytes_slot = self.alloc_slot();
                if let TypeName::Struct(struct_name) = elem.as_ref() {
                    out.push(RuntimeFunctionParamLayout::OwnedListStruct {
                        name: param.name.clone(),
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        layout: self.runtime_struct_list_layout(struct_name, param.span)?,
                    });
                } else {
                    out.push(RuntimeFunctionParamLayout::OwnedListScalar {
                        name: param.name.clone(),
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        elem_ty: ensure_runtime_generic_scalar_type(elem, param.span)
                            .map_err(RuntimeGenericLowerError::Diagnostic)?,
                    });
                }
            } else {
                let ty = ensure_runtime_generic_scalar_type(&param.ty, param.span)
                    .map_err(RuntimeGenericLowerError::Diagnostic)?;
                out.push(RuntimeFunctionParamLayout::Scalar {
                    name: param.name.clone(),
                    ty,
                    slot: self.alloc_slot(),
                });
            }
        }
        self.function_param_slots
            .insert(name.to_string(), out.clone());
        Ok(out)
    }

    pub(super) fn function_return_layout(
        &mut self,
        name: &str,
    ) -> RuntimeGenericLowerResult<Option<RuntimeFunctionReturnLayout>> {
        if let Some(layout) = self.function_return_slots.get(name) {
            return Ok(layout.clone());
        }
        let function = self.functions.get(name).ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(unknown_function_diagnostic(name, Span::new(0, 0)))
        })?;
        if matches!(function.return_type.as_ref(), Some(TypeName::File)) {
            let layout = Some(RuntimeFunctionReturnLayout::OwnedFile {
                fd_slot: self.alloc_slot(),
            });
            self.function_return_slots
                .insert(name.to_string(), layout.clone());
            return Ok(layout);
        }
        let layout = if let Some(ret_ty) = &function.return_type {
            let enum_name = match ret_ty {
                TypeName::Struct(name) | TypeName::Applied { name, .. } => Some(name),
                _ => None,
            };
            if enum_name.is_some_and(|name| self.enums.contains_key(name)) {
                let layout = self.runtime_enum_layout(ret_ty, function.span)?;
                let tag_slot = self.alloc_slot();
                let payload_slots = (0..layout.payload_slots)
                    .map(|_| self.alloc_slot())
                    .collect();
                Some(RuntimeFunctionReturnLayout::Enum {
                    layout,
                    tag_slot,
                    payload_slots,
                })
            } else if let TypeName::Struct(struct_name) = ret_ty {
                if self.struct_has_direct_owned_list(struct_name) {
                    Some(RuntimeFunctionReturnLayout::OwnedStruct {
                        binding: self.allocate_runtime_owned_struct_binding(
                            struct_name,
                            false,
                            function.span,
                        )?,
                    })
                } else {
                    let layout = self.runtime_struct_list_layout(struct_name, function.span)?;
                    let mut fields = HashMap::with_capacity(layout.fields.len());
                    for field in &layout.fields {
                        fields.insert(field.name.clone(), (self.alloc_slot(), field.ty));
                    }
                    Some(RuntimeFunctionReturnLayout::Struct { layout, fields })
                }
            } else if matches!(ret_ty, TypeName::String) {
                Some(RuntimeFunctionReturnLayout::OwnedString {
                    ptr_slot: self.alloc_slot(),
                    len_slot: self.alloc_slot(),
                    capacity_slot: self.alloc_slot(),
                    allocation_bytes_slot: self.alloc_slot(),
                })
            } else if let TypeName::Map { key, value } = ret_ty {
                Some(RuntimeFunctionReturnLayout::OwnedMap {
                    ptr_slot: self.alloc_slot(),
                    len_slot: self.alloc_slot(),
                    capacity_slot: self.alloc_slot(),
                    allocation_bytes_slot: self.alloc_slot(),
                    layout: self.runtime_map_layout(key, value, function.span)?,
                })
            } else if let TypeName::List { elem } = ret_ty {
                let ptr_slot = self.alloc_slot();
                let len_slot = self.alloc_slot();
                let capacity_slot = self.alloc_slot();
                let allocation_bytes_slot = self.alloc_slot();
                if let TypeName::Struct(struct_name) = elem.as_ref() {
                    Some(RuntimeFunctionReturnLayout::OwnedListStruct {
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        layout: self.runtime_struct_list_layout(struct_name, function.span)?,
                    })
                } else {
                    Some(RuntimeFunctionReturnLayout::OwnedListScalar {
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        elem_ty: ensure_runtime_generic_scalar_type(elem, function.span)
                            .map_err(RuntimeGenericLowerError::Diagnostic)?,
                    })
                }
            } else {
                let scalar_ty = ensure_runtime_generic_scalar_type(ret_ty, function.span)
                    .map_err(RuntimeGenericLowerError::Diagnostic)?;
                Some(RuntimeFunctionReturnLayout::Scalar {
                    ty: scalar_ty,
                    slot: self.alloc_slot(),
                })
            }
        } else {
            None
        };
        self.function_return_slots
            .insert(name.to_string(), layout.clone());
        Ok(layout)
    }

    pub(super) fn emit_call_target(&mut self, name: &str, span: Span) {
        if let Some(target) = self.function_entries.get(name).copied() {
            self.emit(RuntimeInstr::Call { target });
        } else {
            let instr_index = self.emit(RuntimeInstr::Call { target: usize::MAX });
            self.call_patches.push(RuntimeCallPatch {
                instr_index,
                callee: name.to_string(),
                span,
            });
            self.queue_function(name);
        }
    }

    pub(super) fn capture_enum_return(
        &mut self,
        layout: RuntimeEnumLayout,
        return_tag_slot: usize,
        return_payload_slots: &[usize],
        mutable: bool,
    ) -> RuntimeGenericBinding {
        let tag_slot = self.alloc_slot();
        self.emit(RuntimeInstr::Mov {
            dst: tag_slot,
            src: RuntimeOperand::Slot(return_tag_slot),
        });
        let payload_slots = return_payload_slots
            .iter()
            .map(|return_slot| {
                let local_slot = self.alloc_slot();
                self.emit(RuntimeInstr::Mov {
                    dst: local_slot,
                    src: RuntimeOperand::Slot(*return_slot),
                });
                local_slot
            })
            .collect();
        RuntimeGenericBinding::EnumSlots {
            layout,
            tag_slot,
            payload_slots,
            mutable,
            owns_cleanup: true,
        }
    }

    pub(super) fn emit_call_arguments(
        &mut self,
        args: &[Expr],
        param_layout: &[RuntimeFunctionParamLayout],
    ) -> RuntimeGenericLowerResult<()> {
        for (arg, param) in args.iter().zip(param_layout.iter()) {
            match param {
                RuntimeFunctionParamLayout::Scalar { ty, slot, .. } => {
                    let src = self.lower_expr_as_scalar(arg, *ty)?;
                    if !matches!(src, RuntimeOperand::Slot(source_slot) if source_slot == *slot) {
                        self.emit(RuntimeInstr::Mov { dst: *slot, src });
                    }
                }
                RuntimeFunctionParamLayout::OwnedFile { fd_slot, .. } => {
                    let Expr::Ident { name, span } = arg else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "File arguments must move a named current-scope owner",
                            arg.span(),
                        )));
                    };
                    self.reject_moved_resource(name, *span)?;
                    let source_fd = self
                        .scopes
                        .get_current(name)
                        .and_then(|binding| match binding {
                            RuntimeGenericBinding::OwnedFile { fd_slot } => Some(*fd_slot),
                            _ => None,
                        })
                        .ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                "File argument requires a current-scope owner",
                                *span,
                            ))
                        })?;
                    self.emit(RuntimeInstr::Mov {
                        dst: *fd_slot,
                        src: RuntimeOperand::Slot(source_fd),
                    });
                    let _ = self.scopes.take_current(name);
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::MovedResource { kind: "file owner" },
                    );
                }
                RuntimeFunctionParamLayout::OwnedSender { handle_slot, .. }
                | RuntimeFunctionParamLayout::OwnedReceiver { handle_slot, .. } => {
                    let Expr::Ident { name, span } = arg else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "channel endpoints must move a named current-scope owner",
                            arg.span(),
                        )));
                    };
                    self.reject_moved_resource(name, *span)?;
                    let source = self
                        .scopes
                        .get_current(name)
                        .and_then(|binding| match (binding, param) {
                            (
                                RuntimeGenericBinding::OwnedSender { handle_slot },
                                RuntimeFunctionParamLayout::OwnedSender { .. },
                            )
                            | (
                                RuntimeGenericBinding::OwnedReceiver { handle_slot },
                                RuntimeFunctionParamLayout::OwnedReceiver { .. },
                            ) => Some(*handle_slot),
                            _ => None,
                        })
                        .ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                "channel endpoint argument has the wrong ownership role",
                                *span,
                            ))
                        })?;
                    self.emit(RuntimeInstr::Mov {
                        dst: *handle_slot,
                        src: RuntimeOperand::Slot(source),
                    });
                    let _ = self.scopes.take_current(name);
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::MovedResource {
                            kind: "channel endpoint",
                        },
                    );
                }
                RuntimeFunctionParamLayout::BorrowedFile { fd_slot, .. } => {
                    let Expr::Unary {
                        op: UnaryOp::Ref,
                        expr,
                        span,
                    } = arg
                    else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "borrowed File arguments require '&file'",
                            arg.span(),
                        )));
                    };
                    let Expr::Ident { name, .. } = expr.as_ref() else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "borrowed File arguments require a named owner",
                            *span,
                        )));
                    };
                    self.reject_moved_resource(name, *span)?;
                    let source_fd = self
                        .scopes
                        .get(name)
                        .and_then(RuntimeGenericBinding::as_owned_file)
                        .ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                "borrowed File argument requires a live owner",
                                *span,
                            ))
                        })?;
                    self.emit(RuntimeInstr::Mov {
                        dst: *fd_slot,
                        src: RuntimeOperand::Slot(source_fd),
                    });
                }
                RuntimeFunctionParamLayout::Struct { layout, fields, .. } => {
                    let values = self.lower_runtime_struct_operands(arg, layout, arg.span())?;
                    for (field, source) in values {
                        let Some((target_slot, target_ty)) = fields.get(&field.name) else {
                            return Err(RuntimeGenericLowerError::Unsupported);
                        };
                        if *target_ty != field.ty {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "runtime-native struct argument field type does not match parameter",
                                arg.span(),
                            )));
                        }
                        self.emit(RuntimeInstr::Mov {
                            dst: *target_slot,
                            src: source,
                        });
                        self.normalize_scalar_slot(*target_slot, *target_ty);
                    }
                }
                RuntimeFunctionParamLayout::OwnedStruct {
                    binding: destination,
                    ..
                } => {
                    let Expr::Ident { name, span } = arg else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "resource-owning struct arguments must move a named current-scope owner",
                            arg.span(),
                        )));
                    };
                    let Some(source) = self.scopes.get_current(name).cloned() else {
                        self.reject_moved_resource(name, *span)?;
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "resource-owning struct arguments require a current-scope owner",
                            *span,
                        )));
                    };
                    if !matches!(
                        source,
                        RuntimeGenericBinding::OwnedStruct {
                            owns_cleanup: true,
                            ..
                        }
                    ) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "resource-owning struct arguments require a matching owner binding",
                            *span,
                        )));
                    }
                    self.emit_owned_struct_descriptor_move(&source, destination, *span)?;
                    let consumed = self.scopes.take_current(name).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "resource-owning struct source disappeared during call move",
                            *span,
                        ))
                    })?;
                    debug_assert!(matches!(
                        consumed,
                        RuntimeGenericBinding::OwnedStruct { .. }
                    ));
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::MovedResource {
                            kind: "resource owner",
                        },
                    );
                }
                RuntimeFunctionParamLayout::BorrowedOwnedStruct {
                    binding: destination,
                    ..
                } => {
                    let Expr::Ident { name, span } = arg else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "borrowed resource-owning struct arguments require a named binding",
                            arg.span(),
                        )));
                    };
                    self.reject_moved_resource(name, *span)?;
                    let Some(source) = self.scopes.get(name).cloned() else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "borrowed resource-owning struct argument requires a live owner",
                            *span,
                        )));
                    };
                    if !matches!(source, RuntimeGenericBinding::OwnedStruct { .. }) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "borrowed resource-owning struct argument requires a live resource binding",
                            *span,
                        )));
                    }
                    self.emit_owned_struct_descriptor_move(&source, destination, *span)?;
                }
                RuntimeFunctionParamLayout::OwnedListScalar {
                    ptr_slot,
                    len_slot,
                    capacity_slot,
                    allocation_bytes_slot,
                    elem_ty,
                    ..
                } => {
                    let Expr::Ident { name, span } = arg else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "owned list arguments must move a named current-scope owner",
                            arg.span(),
                        )));
                    };
                    let Some((source_ptr, source_len, source_capacity, source_bytes, _, source_ty)) =
                        self.scopes
                            .get_current(name)
                            .and_then(RuntimeGenericBinding::as_owned_list_scalar)
                    else {
                        self.reject_moved_resource(name, *span)?;
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "owned list arguments require a current-scope list owner",
                            *span,
                        )));
                    };
                    if source_ty != *elem_ty {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "owned list argument type does not match parameter",
                            *span,
                        )));
                    }
                    for (dst, src) in [
                        (*ptr_slot, source_ptr),
                        (*len_slot, source_len),
                        (*capacity_slot, source_capacity),
                        (*allocation_bytes_slot, source_bytes),
                    ] {
                        self.emit(RuntimeInstr::Mov {
                            dst,
                            src: RuntimeOperand::Slot(src),
                        });
                    }
                    let consumed = self.scopes.take_current(name).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "list ownership source disappeared during call move",
                            *span,
                        ))
                    })?;
                    debug_assert!(matches!(
                        consumed,
                        RuntimeGenericBinding::OwnedListScalar { .. }
                    ));
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::MovedResource { kind: "list owner" },
                    );
                }
                RuntimeFunctionParamLayout::OwnedListStruct {
                    ptr_slot,
                    len_slot,
                    capacity_slot,
                    allocation_bytes_slot,
                    layout,
                    ..
                } => {
                    let Expr::Ident { name, span } = arg else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "owned list arguments must move a named current-scope owner",
                            arg.span(),
                        )));
                    };
                    let Some((
                        source_ptr,
                        source_len,
                        source_capacity,
                        source_bytes,
                        _,
                        source_layout,
                    )) = self
                        .scopes
                        .get_current(name)
                        .and_then(RuntimeGenericBinding::as_owned_list_struct)
                    else {
                        self.reject_moved_resource(name, *span)?;
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "owned struct-list arguments require a current-scope list owner",
                            *span,
                        )));
                    };
                    if source_layout != *layout {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "owned struct-list argument type does not match parameter",
                            *span,
                        )));
                    }
                    for (dst, src) in [
                        (*ptr_slot, source_ptr),
                        (*len_slot, source_len),
                        (*capacity_slot, source_capacity),
                        (*allocation_bytes_slot, source_bytes),
                    ] {
                        self.emit(RuntimeInstr::Mov {
                            dst,
                            src: RuntimeOperand::Slot(src),
                        });
                    }
                    let consumed = self.scopes.take_current(name).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "struct-list ownership source disappeared during call move",
                            *span,
                        ))
                    })?;
                    debug_assert!(matches!(
                        consumed,
                        RuntimeGenericBinding::OwnedListStruct { .. }
                    ));
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::MovedResource { kind: "list owner" },
                    );
                }
                RuntimeFunctionParamLayout::OwnedString {
                    ptr_slot,
                    len_slot,
                    capacity_slot,
                    allocation_bytes_slot,
                    ..
                } => {
                    let Expr::Ident { name, span } = arg else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "owned string arguments must move a named current-scope owner",
                            arg.span(),
                        )));
                    };
                    let Some((source_ptr, source_len, source_capacity, source_bytes, _)) = self
                        .scopes
                        .get_current(name)
                        .and_then(RuntimeGenericBinding::as_owned_string)
                    else {
                        self.reject_moved_resource(name, *span)?;
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "owned string argument requires a current-scope string owner",
                            *span,
                        )));
                    };
                    for (dst, src) in [
                        (*ptr_slot, source_ptr),
                        (*len_slot, source_len),
                        (*capacity_slot, source_capacity),
                        (*allocation_bytes_slot, source_bytes),
                    ] {
                        self.emit(RuntimeInstr::Mov {
                            dst,
                            src: RuntimeOperand::Slot(src),
                        });
                    }
                    let _ = self.scopes.take_current(name);
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::MovedResource {
                            kind: "string owner",
                        },
                    );
                }
                RuntimeFunctionParamLayout::OwnedMap {
                    ptr_slot,
                    len_slot,
                    capacity_slot,
                    allocation_bytes_slot,
                    layout,
                    ..
                } => {
                    let Expr::Ident { name, span } = arg else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "owned map arguments must move a named current-scope owner",
                            arg.span(),
                        )));
                    };
                    let Some((
                        source_ptr,
                        source_len,
                        source_capacity,
                        source_bytes,
                        _,
                        source_layout,
                    )) = self
                        .scopes
                        .get_current(name)
                        .and_then(RuntimeGenericBinding::as_owned_map)
                    else {
                        self.reject_moved_resource(name, *span)?;
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "owned map argument requires a current-scope map owner",
                            *span,
                        )));
                    };
                    if source_layout != *layout {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "owned map argument layout mismatch",
                            *span,
                        )));
                    }
                    for (dst, src) in [
                        (*ptr_slot, source_ptr),
                        (*len_slot, source_len),
                        (*capacity_slot, source_capacity),
                        (*allocation_bytes_slot, source_bytes),
                    ] {
                        self.emit(RuntimeInstr::Mov {
                            dst,
                            src: RuntimeOperand::Slot(src),
                        });
                    }
                    let _ = self.scopes.take_current(name);
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::MovedResource { kind: "map owner" },
                    );
                }
                RuntimeFunctionParamLayout::Enum {
                    layout,
                    tag_slot,
                    payload_slots,
                    ..
                } => {
                    let Expr::Ident { name, span } = arg else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime-native enum arguments must be named bindings",
                            arg.span(),
                        )));
                    };
                    let source = if layout.owns_resources() {
                        self.scopes.get_current(name).cloned()
                    } else {
                        self.scopes.get(name).cloned()
                    };
                    let Some(RuntimeGenericBinding::EnumSlots {
                        layout: source_layout,
                        tag_slot: source_tag,
                        payload_slots: source_payload,
                        owns_cleanup,
                        ..
                    }) = source
                    else {
                        self.reject_moved_resource(name, *span)?;
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime-native enum argument requires an enum binding",
                            *span,
                        )));
                    };
                    if source_layout != *layout || source_payload.len() != payload_slots.len() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime-native enum argument layout mismatch",
                            *span,
                        )));
                    }
                    if layout.owns_resources() && !owns_cleanup {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "resource enum argument requires an owning current-scope binding",
                            *span,
                        )));
                    }
                    self.emit(RuntimeInstr::Mov {
                        dst: *tag_slot,
                        src: RuntimeOperand::Slot(source_tag),
                    });
                    for (dst, src) in payload_slots.iter().zip(source_payload) {
                        self.emit(RuntimeInstr::Mov {
                            dst: *dst,
                            src: RuntimeOperand::Slot(src),
                        });
                    }
                    if layout.owns_resources() {
                        let _ = self.scopes.take_current(name);
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::MovedResource {
                                kind: "resource enum owner",
                            },
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
