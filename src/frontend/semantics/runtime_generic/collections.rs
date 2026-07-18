//! Runtime-native collection mutation, indexing, methods, and struct storage.

use super::*;

impl<'a> RuntimeGenericBuilder<'a> {
    pub(super) fn lower_array_slot_assign(
        &mut self,
        name: &str,
        index: &Expr,
        expr: &Expr,
        span: Span,
    ) -> RuntimeGenericLowerResult<bool> {
        let Some((slots, len_slot, is_mutable, elem_ty, full_len_known)) = self
            .scopes
            .get(name)
            .and_then(RuntimeGenericBinding::as_array_slots)
        else {
            return Ok(false);
        };
        if !is_mutable {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!("cannot assign to immutable '{name}'").as_str(),
                span,
            )));
        }
        let rhs = self.lower_expr_as_type(expr, elem_ty)?;

        if let Ok(idx) = runtime_const_array_index(index) {
            let dst_slot = slots.get(idx).copied().ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "array index out of bounds in runtime generic lowering",
                    span,
                ))
            })?;
            if full_len_known {
                self.emit(RuntimeInstr::Mov {
                    dst: dst_slot,
                    src: rhs,
                });
                self.normalize_slot(dst_slot, elem_ty);
                return Ok(true);
            }
            let check = self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Imm(idx as u64),
                rhs: RuntimeOperand::Slot(len_slot),
                target: usize::MAX,
            });
            self.emit(RuntimeInstr::Mov {
                dst: dst_slot,
                src: rhs,
            });
            self.normalize_slot(dst_slot, elem_ty);
            let done_jump = self.emit(RuntimeInstr::Jump { target: usize::MAX });
            let oob_target = self.instrs.len();
            self.emit(RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(255),
            });
            let done_target = self.instrs.len();
            self.patch_target(check, oob_target)?;
            self.patch_target(done_jump, done_target)?;
            return Ok(true);
        }

        let idx_ty = self.infer_expr_int_type(index).map_err(|err| match err {
            RuntimeGenericLowerError::Unsupported => {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "array index must be integer in runtime generic lowering",
                    span,
                ))
            }
            other => other,
        })?;
        if full_len_known {
            if let Some(iter_slot) = self.loop_unchecked_index_slot(name, index) {
                self.emit(RuntimeInstr::StoreIndexUnchecked {
                    base_slots: slots.clone(),
                    index: RuntimeOperand::Slot(iter_slot),
                    src: rhs,
                });
                return Ok(true);
            }
        }
        let statically_in_bounds =
            full_len_known && self.index_expr_proven_in_bounds(index, idx_ty, slots.len());
        let idx_op = self.lower_expr_as_type(index, idx_ty)?;

        // StoreIndex already performs an OOB check against backing slots.
        // For fixed-size arrays this avoids redundant guards in hot loops.
        if full_len_known {
            self.emit(if statically_in_bounds {
                RuntimeInstr::StoreIndexUnchecked {
                    base_slots: slots.clone(),
                    index: idx_op,
                    src: rhs,
                }
            } else {
                RuntimeInstr::StoreIndex {
                    base_slots: slots.clone(),
                    index: idx_op,
                    src: rhs,
                }
            });
            return Ok(true);
        }

        // Variable-length arrays still need a logical-len guard.
        let oob_check = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::LtUnsigned,
            lhs: idx_op.clone(),
            rhs: RuntimeOperand::Slot(len_slot),
            target: usize::MAX,
        });
        self.emit(RuntimeInstr::StoreIndexUnchecked {
            base_slots: slots.clone(),
            index: idx_op,
            src: rhs,
        });
        let done_jump = self.emit(RuntimeInstr::Jump { target: usize::MAX });
        let oob_target = self.instrs.len();
        self.emit(RuntimeInstr::Exit {
            code: RuntimeOperand::Imm(255),
        });
        let done_target = self.instrs.len();
        self.patch_target(oob_check, oob_target)?;
        self.patch_target(done_jump, done_target)?;
        Ok(true)
    }

    pub(super) fn lower_owned_list_scalar_checked_index(
        &mut self,
        name: &str,
        index: &Expr,
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<(usize, RuntimeOperand, RuntimeScalarType)>> {
        let Some((ptr_slot, len_slot, _, _, _, elem_ty)) = self
            .scopes
            .get(name)
            .and_then(RuntimeGenericBinding::as_owned_list_scalar)
        else {
            return Ok(None);
        };
        self.lower_owned_list_scalar_checked_index_slots(ptr_slot, len_slot, elem_ty, index, span)
            .map(Some)
    }

    pub(super) fn lower_owned_list_scalar_checked_index_slots(
        &mut self,
        ptr_slot: usize,
        len_slot: usize,
        elem_ty: RuntimeScalarType,
        index: &Expr,
        span: Span,
    ) -> RuntimeGenericLowerResult<(usize, RuntimeOperand, RuntimeScalarType)> {
        let index_ty = self.infer_expr_int_type(index).map_err(|err| match err {
            RuntimeGenericLowerError::Unsupported => {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "list index must be integer in runtime generic lowering",
                    span,
                ))
            }
            other => other,
        })?;
        let index_operand = self.lower_expr_as_type(index, index_ty)?;
        if index_ty.signed {
            self.emit_guard_failure_exit(
                RuntimeCmpOp::GeSigned,
                index_operand.clone(),
                RuntimeOperand::Imm(0),
                255,
            )?;
        }
        self.emit_guard_failure_exit(
            RuntimeCmpOp::LtUnsigned,
            index_operand.clone(),
            RuntimeOperand::Slot(len_slot),
            255,
        )?;
        Ok((ptr_slot, index_operand, elem_ty))
    }

    pub(super) fn lower_owned_struct_list_scalar_checked_index(
        &mut self,
        base: &Expr,
        index: &Expr,
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<(usize, RuntimeOperand, RuntimeScalarType)>> {
        let Some(RuntimeOwnedStructListField::Scalar {
            ptr_slot,
            len_slot,
            elem_ty,
            ..
        }) = self.owned_struct_list_field(base)
        else {
            return Ok(None);
        };
        self.lower_owned_list_scalar_checked_index_slots(ptr_slot, len_slot, elem_ty, index, span)
            .map(Some)
    }

    pub(super) fn lower_owned_list_struct_checked_index(
        &mut self,
        name: &str,
        index: &Expr,
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<(usize, RuntimeOperand, RuntimeStructListLayout)>> {
        let Some((ptr_slot, len_slot, _, _, _, layout)) = self
            .scopes
            .get(name)
            .and_then(RuntimeGenericBinding::as_owned_list_struct)
        else {
            return Ok(None);
        };
        let index_ty = self.infer_expr_int_type(index).map_err(|err| match err {
            RuntimeGenericLowerError::Unsupported => {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "list index must be integer in runtime generic lowering",
                    span,
                ))
            }
            other => other,
        })?;
        let index_operand = self.lower_expr_as_type(index, index_ty)?;
        if index_ty.signed {
            self.emit_guard_failure_exit(
                RuntimeCmpOp::GeSigned,
                index_operand.clone(),
                RuntimeOperand::Imm(0),
                255,
            )?;
        }
        self.emit_guard_failure_exit(
            RuntimeCmpOp::LtUnsigned,
            index_operand.clone(),
            RuntimeOperand::Slot(len_slot),
            255,
        )?;
        Ok(Some((ptr_slot, index_operand, layout)))
    }

    pub(super) fn lower_dict_slot_assign(
        &mut self,
        name: &str,
        index: &Expr,
        expr: &Expr,
        span: Span,
    ) -> RuntimeGenericLowerResult<bool> {
        let Some((entries, is_mutable, value_ty)) = self
            .scopes
            .get(name)
            .and_then(RuntimeGenericBinding::as_dict_slots)
        else {
            return Ok(false);
        };
        if !is_mutable {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!("cannot assign to immutable '{name}'").as_str(),
                span,
            )));
        }
        let key = runtime_const_dict_key(index).ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                "runtime generic mutable dictionary index must be a string literal",
                span,
            ))
        })?;
        let Some(dst_slot) = entries.get(&key).copied() else {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!("unknown dictionary key '{key}'").as_str(),
                span,
            )));
        };
        let rhs = self.lower_expr_as_type(expr, value_ty)?;
        self.emit(RuntimeInstr::Mov {
            dst: dst_slot,
            src: rhs,
        });
        self.normalize_slot(dst_slot, value_ty);
        Ok(true)
    }

    pub(super) fn mark_array_len_dynamic(&mut self, name: &str) {
        if let Some(RuntimeGenericBinding::ArraySlots { full_len_known, .. }) =
            self.scopes.get_mut(name)
        {
            *full_len_known = false;
        }
        self.unchecked_array_loop_accesses
            .retain(|assumption| assumption.array_name != name);
    }

    pub(super) fn resolve_runtime_kernel_u64_array_slots(
        &self,
        kernel: &str,
        expr: &Expr,
        require_mutable: bool,
        span: Span,
    ) -> RuntimeGenericLowerResult<Vec<usize>> {
        let Expr::Ident { name, .. } = expr else {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!("{kernel}() expects first argument to be an array identifier").as_str(),
                span,
            )));
        };
        let Some((slots, _len_slot, mutable, elem_ty, full_len_known)) = self
            .scopes
            .get(name)
            .and_then(RuntimeGenericBinding::as_array_slots)
        else {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!(
                    "{kernel}() expects first argument to be a mutable fixed-size [u64; N] array"
                )
                .as_str(),
                span,
            )));
        };
        if require_mutable && !mutable {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!("{kernel}() requires mutable array receiver").as_str(),
                span,
            )));
        }
        let u64_ty = RuntimeIntType::new(false, 64)?;
        if elem_ty != u64_ty {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!("{kernel}() requires [u64; N] array").as_str(),
                span,
            )));
        }
        if !full_len_known {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!("{kernel}() requires fixed-size array length").as_str(),
                span,
            )));
        }
        Ok(slots)
    }

    pub(super) fn lower_runtime_owned_struct_list_method(
        &mut self,
        receiver: &str,
        field: &str,
        name: &str,
        args: &[Expr],
        span: Span,
    ) -> RuntimeGenericLowerResult<()> {
        let (list_field, mutable) = match self.scopes.get(receiver) {
            Some(RuntimeGenericBinding::OwnedStruct {
                list_fields,
                mutable,
                ..
            }) => (list_fields.get(field).cloned(), *mutable),
            Some(RuntimeGenericBinding::MovedResource { .. }) => {
                self.reject_moved_resource(receiver, span)?;
                (None, false)
            }
            _ => (None, false),
        };
        if !mutable {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!("cannot mutate immutable '{receiver}'").as_str(),
                span,
            )));
        }
        let Some(list_field) = list_field else {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!("unknown list field '{field}'").as_str(),
                span,
            )));
        };
        match list_field {
            RuntimeOwnedStructListField::Scalar {
                ptr_slot,
                len_slot,
                capacity_slot,
                allocation_bytes_slot,
                elem_ty,
            } => match name {
                "push" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "push() expects exactly one argument",
                            span,
                        )));
                    }
                    let value = self.lower_expr_as_scalar(&args[0], elem_ty)?;
                    self.emit_owned_list_scalar_push(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        value,
                        elem_ty,
                    )
                }
                "pop" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "pop() expects no arguments",
                            span,
                        )));
                    }
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::GtUnsigned,
                        RuntimeOperand::Slot(len_slot),
                        RuntimeOperand::Imm(0),
                        255,
                    )?;
                    self.emit(RuntimeInstr::BinOpInPlace {
                        dst: len_slot,
                        op: RuntimeBinOp::Sub,
                        rhs: RuntimeOperand::Imm(1),
                    });
                    Ok(())
                }
                "clear" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "clear() expects no arguments",
                            span,
                        )));
                    }
                    self.emit(RuntimeInstr::Mov {
                        dst: len_slot,
                        src: RuntimeOperand::Imm(0),
                    });
                    Ok(())
                }
                "reserve" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "reserve() expects exactly one additional-capacity argument",
                            span,
                        )));
                    }
                    let additional =
                        self.lower_expr_as_type(&args[0], RuntimeIntType::new(false, 64)?)?;
                    let required_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::BinOp {
                        dst: required_slot,
                        op: RuntimeBinOp::Add,
                        lhs: RuntimeOperand::Slot(len_slot),
                        rhs: additional,
                    });
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::GeUnsigned,
                        RuntimeOperand::Slot(required_slot),
                        RuntimeOperand::Slot(len_slot),
                        101,
                    )?;
                    self.emit_owned_list_ensure_capacity(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        RuntimeOperand::Slot(required_slot),
                        u64::from(elem_ty.storage_bytes()),
                    )
                }
                "shrink_to_fit" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "shrink_to_fit() expects no arguments",
                            span,
                        )));
                    }
                    self.emit_owned_list_shrink_to(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        RuntimeOperand::Imm(0),
                        u64::from(elem_ty.storage_bytes()),
                    )
                }
                "shrink_to" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "shrink_to() expects exactly one minimum-capacity argument",
                            span,
                        )));
                    }
                    let minimum =
                        self.lower_expr_as_type(&args[0], RuntimeIntType::new(false, 64)?)?;
                    self.emit_owned_list_shrink_to(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        minimum,
                        u64::from(elem_ty.storage_bytes()),
                    )
                }
                _ => Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("unknown mutable list-field method '{name}'").as_str(),
                    span,
                ))),
            },
            RuntimeOwnedStructListField::Struct {
                ptr_slot,
                len_slot,
                capacity_slot,
                allocation_bytes_slot,
                layout,
            } => match name {
                "push" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "push() expects exactly one argument",
                            span,
                        )));
                    }
                    let values = self.lower_runtime_struct_operands(&args[0], &layout, span)?;
                    self.emit_owned_list_struct_push(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        &layout,
                        values,
                    )
                }
                "pop" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "pop() expects no arguments",
                            span,
                        )));
                    }
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::GtUnsigned,
                        RuntimeOperand::Slot(len_slot),
                        RuntimeOperand::Imm(0),
                        255,
                    )?;
                    self.emit(RuntimeInstr::BinOpInPlace {
                        dst: len_slot,
                        op: RuntimeBinOp::Sub,
                        rhs: RuntimeOperand::Imm(1),
                    });
                    Ok(())
                }
                "clear" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "clear() expects no arguments",
                            span,
                        )));
                    }
                    self.emit(RuntimeInstr::Mov {
                        dst: len_slot,
                        src: RuntimeOperand::Imm(0),
                    });
                    Ok(())
                }
                "reserve" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "reserve() expects exactly one additional-capacity argument",
                            span,
                        )));
                    }
                    let additional =
                        self.lower_expr_as_type(&args[0], RuntimeIntType::new(false, 64)?)?;
                    let required_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::BinOp {
                        dst: required_slot,
                        op: RuntimeBinOp::Add,
                        lhs: RuntimeOperand::Slot(len_slot),
                        rhs: additional,
                    });
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::GeUnsigned,
                        RuntimeOperand::Slot(required_slot),
                        RuntimeOperand::Slot(len_slot),
                        101,
                    )?;
                    self.emit_owned_list_ensure_capacity(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        RuntimeOperand::Slot(required_slot),
                        layout.stride_bytes,
                    )
                }
                "shrink_to_fit" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "shrink_to_fit() expects no arguments",
                            span,
                        )));
                    }
                    self.emit_owned_list_shrink_to(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        RuntimeOperand::Imm(0),
                        layout.stride_bytes,
                    )
                }
                "shrink_to" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "shrink_to() expects exactly one minimum-capacity argument",
                            span,
                        )));
                    }
                    let minimum =
                        self.lower_expr_as_type(&args[0], RuntimeIntType::new(false, 64)?)?;
                    self.emit_owned_list_shrink_to(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        minimum,
                        layout.stride_bytes,
                    )
                }
                _ => Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("unknown mutable struct list-field method '{name}'").as_str(),
                    span,
                ))),
            },
        }
    }

    pub(super) fn lower_runtime_method_stmt(
        &mut self,
        receiver: &str,
        name: &str,
        args: &[Expr],
        span: Span,
    ) -> RuntimeGenericLowerResult<()> {
        self.reject_moved_resource(receiver, span)?;
        if let Some(binding) = self.scopes.get(receiver).cloned() {
            match binding {
                RuntimeGenericBinding::OwnedSender { handle_slot } => match name {
                    "send" => {
                        if args.len() != 1 {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "Sender<u64>.send() expects exactly one u64 value",
                                span,
                            )));
                        }
                        let value =
                            self.lower_expr_as_type(&args[0], RuntimeIntType::new(false, 64)?)?;
                        self.emit(RuntimeInstr::ChannelSend {
                            handle: RuntimeOperand::Slot(handle_slot),
                            value,
                        });
                        return Ok(());
                    }
                    "close" => {
                        if !args.is_empty() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "Sender<u64>.close() expects no arguments",
                                span,
                            )));
                        }
                        self.emit(RuntimeInstr::ChannelClose {
                            handle: RuntimeOperand::Slot(handle_slot),
                            sender: true,
                        });
                        let _ = self.scopes.take_current(receiver);
                        self.scopes.insert(
                            receiver.to_string(),
                            RuntimeGenericBinding::MovedResource {
                                kind: "Sender owner",
                            },
                        );
                        return Ok(());
                    }
                    _ => {}
                },
                RuntimeGenericBinding::OwnedReceiver { handle_slot } if name == "close" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Receiver<u64>.close() expects no arguments",
                            span,
                        )));
                    }
                    self.emit(RuntimeInstr::ChannelClose {
                        handle: RuntimeOperand::Slot(handle_slot),
                        sender: false,
                    });
                    let _ = self.scopes.take_current(receiver);
                    self.scopes.insert(
                        receiver.to_string(),
                        RuntimeGenericBinding::MovedResource {
                            kind: "Receiver owner",
                        },
                    );
                    return Ok(());
                }
                _ => {}
            }
        }
        if name == "close" {
            if !args.is_empty() {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    "File.close() expects no arguments",
                    span,
                )));
            }
            return self.lower_file_close_owner(receiver, span);
        }
        let receiver_expr = Expr::Ident {
            name: receiver.to_string(),
            span,
        };
        if self
            .lower_runtime_user_method_call(&receiver_expr, name, args, span)?
            .is_some()
        {
            return Ok(());
        }
        if let Some((ptr_slot, len_slot, capacity_slot, bytes_slot, mutable, layout)) = self
            .scopes
            .get(receiver)
            .and_then(RuntimeGenericBinding::as_owned_map)
        {
            if !mutable {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("cannot mutate immutable '{receiver}'").as_str(),
                    span,
                )));
            }
            match name {
                "set" => {
                    if args.len() != 2 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "map set() expects exactly key and value arguments",
                            span,
                        )));
                    }
                    let key = self.lower_expr_as_scalar(&args[0], layout.key_ty)?;
                    let value = self.lower_expr_as_scalar(&args[1], layout.value_ty)?;
                    let (found, index) =
                        self.emit_runtime_map_find(ptr_slot, len_slot, &layout, key)?;
                    let insert = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Ne,
                        lhs: RuntimeOperand::Slot(found),
                        rhs: RuntimeOperand::Imm(0),
                        target: usize::MAX,
                    });
                    self.emit_runtime_map_store_field(
                        ptr_slot,
                        RuntimeOperand::Slot(index),
                        &layout,
                        layout.value_offset_bytes,
                        layout.value_ty,
                        value,
                    );
                    let done = self.emit(RuntimeInstr::Jump { target: usize::MAX });
                    let insert_target = self.instrs.len();
                    self.patch_target(insert, insert_target)?;
                    let required = self.alloc_slot();
                    self.emit(RuntimeInstr::BinOp {
                        dst: required,
                        op: RuntimeBinOp::Add,
                        lhs: RuntimeOperand::Slot(len_slot),
                        rhs: RuntimeOperand::Imm(1),
                    });
                    self.emit_owned_list_ensure_capacity(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        bytes_slot,
                        RuntimeOperand::Slot(required),
                        layout.stride_bytes,
                    )?;
                    self.emit_runtime_map_store_field(
                        ptr_slot,
                        RuntimeOperand::Slot(len_slot),
                        &layout,
                        0,
                        layout.key_ty,
                        key,
                    );
                    self.emit_runtime_map_store_field(
                        ptr_slot,
                        RuntimeOperand::Slot(len_slot),
                        &layout,
                        layout.value_offset_bytes,
                        layout.value_ty,
                        value,
                    );
                    self.emit(RuntimeInstr::Mov {
                        dst: len_slot,
                        src: RuntimeOperand::Slot(required),
                    });
                    let done_target = self.instrs.len();
                    self.patch_target(done, done_target)?;
                    return Ok(());
                }
                "remove" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "map remove() expects exactly one key argument",
                            span,
                        )));
                    }
                    let key = self.lower_expr_as_scalar(&args[0], layout.key_ty)?;
                    let (found, index) =
                        self.emit_runtime_map_find(ptr_slot, len_slot, &layout, key)?;
                    let absent = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Ne,
                        lhs: RuntimeOperand::Slot(found),
                        rhs: RuntimeOperand::Imm(0),
                        target: usize::MAX,
                    });
                    self.emit(RuntimeInstr::BinOpInPlace {
                        dst: len_slot,
                        op: RuntimeBinOp::Sub,
                        rhs: RuntimeOperand::Imm(1),
                    });
                    for (offset, ty) in [
                        (0, layout.key_ty),
                        (layout.value_offset_bytes, layout.value_ty),
                    ] {
                        let source_index = self.runtime_map_field_index(
                            RuntimeOperand::Slot(len_slot),
                            &layout,
                            offset,
                            ty,
                        );
                        let loaded = self.alloc_slot();
                        self.emit(RuntimeInstr::HeapLoadInt {
                            dst: loaded,
                            ptr: RuntimeOperand::Slot(ptr_slot),
                            index: source_index,
                            bytes: ty.storage_bytes(),
                        });
                        self.emit_runtime_map_store_field(
                            ptr_slot,
                            RuntimeOperand::Slot(index),
                            &layout,
                            offset,
                            ty,
                            RuntimeOperand::Slot(loaded),
                        );
                    }
                    let done = self.instrs.len();
                    self.patch_target(absent, done)?;
                    return Ok(());
                }
                "clear" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "map clear() expects no arguments",
                            span,
                        )));
                    }
                    self.emit(RuntimeInstr::Mov {
                        dst: len_slot,
                        src: RuntimeOperand::Imm(0),
                    });
                    return Ok(());
                }
                _ => {}
            }
        }
        if let Some((ptr_slot, len_slot, capacity_slot, allocation_bytes_slot, mutable, layout)) =
            self.scopes
                .get(receiver)
                .and_then(RuntimeGenericBinding::as_owned_list_struct)
        {
            if !mutable {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("cannot mutate immutable '{receiver}'").as_str(),
                    span,
                )));
            }
            match name {
                "push" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "push() expects exactly one argument",
                            span,
                        )));
                    }
                    let values = self.lower_runtime_struct_operands(&args[0], &layout, span)?;
                    self.emit_owned_list_struct_push(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        &layout,
                        values,
                    )?;
                    return Ok(());
                }
                "pop" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "pop() expects no arguments",
                            span,
                        )));
                    }
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::GtUnsigned,
                        RuntimeOperand::Slot(len_slot),
                        RuntimeOperand::Imm(0),
                        255,
                    )?;
                    self.emit(RuntimeInstr::BinOpInPlace {
                        dst: len_slot,
                        op: RuntimeBinOp::Sub,
                        rhs: RuntimeOperand::Imm(1),
                    });
                    return Ok(());
                }
                "clear" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "clear() expects no arguments",
                            span,
                        )));
                    }
                    self.emit(RuntimeInstr::Mov {
                        dst: len_slot,
                        src: RuntimeOperand::Imm(0),
                    });
                    return Ok(());
                }
                "reserve" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "reserve() expects exactly one additional-capacity argument",
                            span,
                        )));
                    }
                    let additional =
                        self.lower_expr_as_type(&args[0], RuntimeIntType::new(false, 64)?)?;
                    let required_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::BinOp {
                        dst: required_slot,
                        op: RuntimeBinOp::Add,
                        lhs: RuntimeOperand::Slot(len_slot),
                        rhs: additional,
                    });
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::GeUnsigned,
                        RuntimeOperand::Slot(required_slot),
                        RuntimeOperand::Slot(len_slot),
                        101,
                    )?;
                    self.emit_owned_list_ensure_capacity(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        RuntimeOperand::Slot(required_slot),
                        layout.stride_bytes,
                    )?;
                    return Ok(());
                }
                "shrink_to_fit" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "shrink_to_fit() expects no arguments",
                            span,
                        )));
                    }
                    self.emit_owned_list_shrink_to(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        RuntimeOperand::Imm(0),
                        layout.stride_bytes,
                    )?;
                    return Ok(());
                }
                "shrink_to" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "shrink_to() expects exactly one minimum-capacity argument",
                            span,
                        )));
                    }
                    let minimum =
                        self.lower_expr_as_type(&args[0], RuntimeIntType::new(false, 64)?)?;
                    self.emit_owned_list_shrink_to(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        minimum,
                        layout.stride_bytes,
                    )?;
                    return Ok(());
                }
                _ => {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("unknown mutable struct-list method '{name}'").as_str(),
                        span,
                    )));
                }
            }
        }
        if let Some((ptr_slot, len_slot, capacity_slot, allocation_bytes_slot, mutable, elem_ty)) =
            self.scopes
                .get(receiver)
                .and_then(RuntimeGenericBinding::as_owned_list_scalar)
        {
            if !mutable {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("cannot mutate immutable '{receiver}'").as_str(),
                    span,
                )));
            }
            match name {
                "push" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "push() expects exactly one argument",
                            span,
                        )));
                    }
                    let value = self.lower_expr_as_scalar(&args[0], elem_ty)?;
                    self.emit_owned_list_scalar_push(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        value,
                        elem_ty,
                    )?;
                    return Ok(());
                }
                "pop" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "pop() expects no arguments",
                            span,
                        )));
                    }
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::GtUnsigned,
                        RuntimeOperand::Slot(len_slot),
                        RuntimeOperand::Imm(0),
                        255,
                    )?;
                    self.emit(RuntimeInstr::BinOpInPlace {
                        dst: len_slot,
                        op: RuntimeBinOp::Sub,
                        rhs: RuntimeOperand::Imm(1),
                    });
                    return Ok(());
                }
                "clear" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "clear() expects no arguments",
                            span,
                        )));
                    }
                    self.emit(RuntimeInstr::Mov {
                        dst: len_slot,
                        src: RuntimeOperand::Imm(0),
                    });
                    return Ok(());
                }
                "reserve" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "reserve() expects exactly one additional-capacity argument",
                            span,
                        )));
                    }
                    let additional =
                        self.lower_expr_as_type(&args[0], RuntimeIntType::new(false, 64)?)?;
                    let required_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::BinOp {
                        dst: required_slot,
                        op: RuntimeBinOp::Add,
                        lhs: RuntimeOperand::Slot(len_slot),
                        rhs: additional,
                    });
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::GeUnsigned,
                        RuntimeOperand::Slot(required_slot),
                        RuntimeOperand::Slot(len_slot),
                        101,
                    )?;
                    self.emit_owned_list_ensure_capacity(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        RuntimeOperand::Slot(required_slot),
                        u64::from(elem_ty.storage_bytes()),
                    )?;
                    return Ok(());
                }
                "shrink_to_fit" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "shrink_to_fit() expects no arguments",
                            span,
                        )));
                    }
                    self.emit_owned_list_shrink_to(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        RuntimeOperand::Imm(0),
                        u64::from(elem_ty.storage_bytes()),
                    )?;
                    return Ok(());
                }
                "shrink_to" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "shrink_to() expects exactly one minimum-capacity argument",
                            span,
                        )));
                    }
                    let minimum =
                        self.lower_expr_as_type(&args[0], RuntimeIntType::new(false, 64)?)?;
                    self.emit_owned_list_shrink_to(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        minimum,
                        u64::from(elem_ty.storage_bytes()),
                    )?;
                    return Ok(());
                }
                _ => {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("unknown mutable list method '{name}'").as_str(),
                        span,
                    )));
                }
            }
        }
        if let Some((slots, len_slot, mutable, elem_ty, full_len_known)) = self
            .scopes
            .get(receiver)
            .and_then(RuntimeGenericBinding::as_array_slots)
        {
            if !mutable {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("cannot mutate immutable '{receiver}'").as_str(),
                    span,
                )));
            }
            match name {
                "push" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "push() expects exactly one argument",
                            span,
                        )));
                    }
                    self.mark_array_len_dynamic(receiver);
                    let rhs = self.lower_expr_as_type(&args[0], elem_ty)?;
                    let cap = slots.len() as u64;
                    let cap_check = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::LtUnsigned,
                        lhs: RuntimeOperand::Slot(len_slot),
                        rhs: RuntimeOperand::Imm(cap),
                        target: usize::MAX,
                    });
                    let mut done_jumps = Vec::with_capacity(slots.len());
                    for (i, slot) in slots.iter().enumerate() {
                        let miss = self.emit(RuntimeInstr::JumpIfCmpFalse {
                            op: RuntimeCmpOp::Eq,
                            lhs: RuntimeOperand::Slot(len_slot),
                            rhs: RuntimeOperand::Imm(i as u64),
                            target: usize::MAX,
                        });
                        self.emit(RuntimeInstr::Mov {
                            dst: *slot,
                            src: rhs,
                        });
                        self.normalize_slot(*slot, elem_ty);
                        self.emit(RuntimeInstr::BinOpInPlace {
                            dst: len_slot,
                            op: RuntimeBinOp::Add,
                            rhs: RuntimeOperand::Imm(1),
                        });
                        done_jumps.push(self.emit(RuntimeInstr::Jump { target: usize::MAX }));
                        let next_case = self.instrs.len();
                        self.patch_target(miss, next_case)?;
                    }
                    let oob_target = self.instrs.len();
                    self.emit(RuntimeInstr::Exit {
                        code: RuntimeOperand::Imm(255),
                    });
                    let done_target = self.instrs.len();
                    self.patch_target(cap_check, oob_target)?;
                    for jump in done_jumps {
                        self.patch_target(jump, done_target)?;
                    }
                    return Ok(());
                }
                "pop" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "pop() expects no arguments",
                            span,
                        )));
                    }
                    self.mark_array_len_dynamic(receiver);
                    let non_empty = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::GtUnsigned,
                        lhs: RuntimeOperand::Slot(len_slot),
                        rhs: RuntimeOperand::Imm(0),
                        target: usize::MAX,
                    });
                    self.emit(RuntimeInstr::BinOpInPlace {
                        dst: len_slot,
                        op: RuntimeBinOp::Sub,
                        rhs: RuntimeOperand::Imm(1),
                    });
                    let done = self.emit(RuntimeInstr::Jump { target: usize::MAX });
                    let oob_target = self.instrs.len();
                    self.emit(RuntimeInstr::Exit {
                        code: RuntimeOperand::Imm(255),
                    });
                    let done_target = self.instrs.len();
                    self.patch_target(non_empty, oob_target)?;
                    self.patch_target(done, done_target)?;
                    return Ok(());
                }
                "sort" | "sort_unstable" | "sort_stable" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!("{name}() expects no arguments").as_str(),
                            span,
                        )));
                    }
                    self.lower_runtime_array_sort_unrolled(
                        &slots,
                        len_slot,
                        elem_ty,
                        full_len_known,
                        span,
                    )?;
                    return Ok(());
                }
                "sort_radix_unstable" | "sort_radix_stable" => {
                    if !args.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!("{name}() expects no arguments").as_str(),
                            span,
                        )));
                    }
                    if !matches!(elem_ty.bits, 32 | 64) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime radix sort currently supports i32/i64/u32/u64 arrays",
                            span,
                        )));
                    }
                    if full_len_known && should_use_runtime_radix_kernel(slots.len(), elem_ty.bits)
                    {
                        self.emit(RuntimeInstr::RadixSortFixedInt {
                            slots: slots.clone(),
                            bits: elem_ty.bits,
                            signed: elem_ty.signed,
                            stable: name == "sort_radix_stable",
                        });
                    } else {
                        self.lower_runtime_array_sort_unrolled(
                            &slots,
                            len_slot,
                            elem_ty,
                            full_len_known,
                            span,
                        )?;
                    }
                    return Ok(());
                }
                "sort_by" => {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime sort_by() is currently unsupported; use sort() for runtime kernels",
                        span,
                    )));
                }
                _ => {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("unsupported runtime array method '{}()'", name).as_str(),
                        span,
                    )));
                }
            }
        }

        if let Some((entries, mutable, value_ty)) = self
            .scopes
            .get(receiver)
            .and_then(RuntimeGenericBinding::as_dict_slots)
        {
            if !mutable {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("cannot mutate immutable '{receiver}'").as_str(),
                    span,
                )));
            }
            match name {
                "set" => {
                    if args.len() != 2 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "set() expects exactly two arguments",
                            span,
                        )));
                    }
                    let key = runtime_const_dict_key(&args[0]).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime generic dictionary set() requires string literal key",
                            span,
                        ))
                    })?;
                    let Some(dst_slot) = entries.get(&key).copied() else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!("unknown dictionary key '{key}'").as_str(),
                            span,
                        )));
                    };
                    let rhs = self.lower_expr_as_type(&args[1], value_ty)?;
                    self.emit(RuntimeInstr::Mov {
                        dst: dst_slot,
                        src: rhs,
                    });
                    self.normalize_slot(dst_slot, value_ty);
                    return Ok(());
                }
                "remove" => {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "remove() expects exactly one argument",
                            span,
                        )));
                    }
                    let key = runtime_const_dict_key(&args[0]).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime generic dictionary remove() requires string literal key",
                            span,
                        ))
                    })?;
                    let Some(dst_slot) = entries.get(&key).copied() else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!("unknown dictionary key '{key}'").as_str(),
                            span,
                        )));
                    };
                    self.emit(RuntimeInstr::Mov {
                        dst: dst_slot,
                        src: RuntimeOperand::Imm(0),
                    });
                    self.normalize_slot(dst_slot, value_ty);
                    return Ok(());
                }
                _ => {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("unsupported runtime dictionary method '{}()'", name).as_str(),
                        span,
                    )));
                }
            }
        }

        Err(RuntimeGenericLowerError::Unsupported)
    }

    pub(super) fn lower_runtime_user_method_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        args: &[Expr],
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<Option<RuntimeFunctionReturnLayout>>> {
        let Expr::Ident {
            name: receiver_name,
            ..
        } = receiver
        else {
            return Ok(None);
        };
        let (struct_name, receiver_mutable, receiver_fields, receiver_owned_binding) = {
            let Some(binding) = self.scopes.get(receiver_name) else {
                return Err(RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                    format!("unknown identifier '{receiver_name}'"),
                    span,
                )));
            };
            match binding {
                RuntimeGenericBinding::StructSlots {
                    struct_name,
                    fields,
                    mutable,
                } => (struct_name.clone(), *mutable, Some(fields.clone()), None),
                RuntimeGenericBinding::OwnedStruct {
                    struct_name,
                    mutable,
                    ..
                } => (struct_name.clone(), *mutable, None, Some(binding.clone())),
                RuntimeGenericBinding::ConstContainer {
                    container: RuntimeConstContainer::Struct { struct_name, .. },
                } => (struct_name.clone(), false, None, None),
                _ => return Ok(None),
            }
        };

        let callee = format!("{struct_name}__{method}");
        let Some(function) = self.functions.get(&callee) else {
            return Ok(None);
        };
        if function.params.len() != args.len() + 1 {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!(
                    "method '{}' expects {} {}, got {}",
                    method,
                    function.params.len().saturating_sub(1),
                    argument_noun(function.params.len().saturating_sub(1)),
                    args.len()
                )
                .as_str(),
                span,
            )));
        }
        let Some(first_param) = function.params.first() else {
            return Ok(None);
        };
        let TypeName::Ref {
            mutable: receiver_requires_mut,
            inner,
        } = &first_param.ty
        else {
            return Ok(None);
        };
        if inner.as_ref() != &TypeName::Struct(struct_name.clone()) {
            return Ok(None);
        }
        if *receiver_requires_mut && !receiver_mutable {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!(
                    "cannot call mut method '{}' on immutable '{}'",
                    method, receiver_name
                )
                .as_str(),
                span,
            )));
        }

        let param_layout = self.function_param_layout(&callee)?;
        let mut call_args = Vec::with_capacity(args.len() + 1);
        call_args.push(receiver.clone());
        call_args.extend_from_slice(args);
        self.emit_call_arguments(&call_args, &param_layout)?;
        let return_layout = self.function_return_layout(&callee)?;
        self.emit_call_target(&callee, span);

        if *receiver_requires_mut {
            match param_layout.first() {
                Some(RuntimeFunctionParamLayout::Struct {
                    fields: param_fields,
                    by_ref: true,
                    mutable: true,
                    ..
                }) => {
                    let source_fields = receiver_fields.ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "mutable receiver requires addressable runtime struct storage",
                            span,
                        ))
                    })?;
                    let mut names: Vec<&String> = source_fields.keys().collect();
                    names.sort_unstable();
                    for name in names {
                        let (destination, destination_ty) = source_fields[name];
                        let (source, source_ty) =
                            param_fields.get(name).copied().ok_or_else(|| {
                                RuntimeGenericLowerError::Diagnostic(type_error(
                                    "mutable receiver layout does not match method parameter",
                                    span,
                                ))
                            })?;
                        if destination_ty != source_ty {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "mutable receiver field type does not match method parameter",
                                span,
                            )));
                        }
                        self.emit(RuntimeInstr::Mov {
                            dst: destination,
                            src: RuntimeOperand::Slot(source),
                        });
                        self.normalize_scalar_slot(destination, destination_ty);
                    }
                }
                Some(RuntimeFunctionParamLayout::BorrowedOwnedStruct {
                    binding: param_binding,
                    mutable: true,
                    ..
                }) => {
                    let destination = receiver_owned_binding.ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "mutable resource receiver requires addressable owned storage",
                            span,
                        ))
                    })?;
                    self.emit_owned_struct_descriptor_move(param_binding, &destination, span)?;
                }
                _ => return Err(RuntimeGenericLowerError::Unsupported),
            }
        }
        Ok(Some(return_layout))
    }

    pub(super) fn runtime_user_method_scalar_type(
        &self,
        receiver: &Expr,
        method: &str,
        args: &[Expr],
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<RuntimeScalarType>> {
        let Expr::Ident {
            name: receiver_name,
            ..
        } = receiver
        else {
            return Ok(None);
        };
        let struct_name = match self.scopes.get(receiver_name) {
            Some(RuntimeGenericBinding::StructSlots { struct_name, .. }) => struct_name,
            Some(RuntimeGenericBinding::OwnedStruct { struct_name, .. }) => struct_name,
            Some(RuntimeGenericBinding::ConstContainer {
                container: RuntimeConstContainer::Struct { struct_name, .. },
            }) => struct_name,
            _ => return Ok(None),
        };
        let callee = format!("{struct_name}__{method}");
        let Some(function) = self.functions.get(&callee) else {
            return Ok(None);
        };
        if function.params.len() != args.len() + 1 {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!(
                    "method '{}' expects {} {}, got {}",
                    method,
                    function.params.len().saturating_sub(1),
                    argument_noun(function.params.len().saturating_sub(1)),
                    args.len()
                )
                .as_str(),
                span,
            )));
        }
        let Some(return_type) = &function.return_type else {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                "expression method call requires a return value",
                span,
            )));
        };
        ensure_runtime_generic_scalar_type(return_type, span)
            .map(Some)
            .map_err(RuntimeGenericLowerError::Diagnostic)
    }

    pub(super) fn lower_runtime_array_sort_unrolled(
        &mut self,
        slots: &[usize],
        len_slot: usize,
        elem_ty: RuntimeIntType,
        full_len_known: bool,
        _span: Span,
    ) -> RuntimeGenericLowerResult<()> {
        if slots.len() <= 1 {
            return Ok(());
        }
        let signed = elem_ty.signed;
        let fast_pairs = runtime_fixed_sort_pairs(slots.len());

        if full_len_known {
            if let Some(pairs) = fast_pairs.as_deref() {
                self.lower_runtime_array_sort_pairs_fixed(slots, pairs, signed);
            } else {
                self.lower_runtime_array_sort_fixed_bubble(slots, signed);
            }
            return Ok(());
        }

        if slots.len() <= 16 {
            self.lower_runtime_array_sort_len_dispatch(slots, len_slot, signed)?;
            return Ok(());
        }

        let cap = slots.len() as u64;
        let partial_sort = self.emit(RuntimeInstr::JumpIfCmpFalse {
            op: RuntimeCmpOp::Eq,
            lhs: RuntimeOperand::Slot(len_slot),
            rhs: RuntimeOperand::Imm(cap),
            target: usize::MAX,
        });

        // Fast path: full logical length.
        if let Some(pairs) = fast_pairs.as_deref() {
            self.lower_runtime_array_sort_pairs_fixed(slots, pairs, signed);
        } else {
            self.lower_runtime_array_sort_fixed_bubble(slots, signed);
        }
        let done = self.emit(RuntimeInstr::Jump { target: usize::MAX });

        // Fallback path: mutable logical length after push/pop. Guard each compare/swap by len.
        let partial_start = self.instrs.len();
        self.patch_target(partial_sort, partial_start)?;
        if let Some(pairs) = fast_pairs.as_deref() {
            self.lower_runtime_array_sort_pairs_guarded(slots, pairs, len_slot, signed)?;
        } else {
            self.lower_runtime_array_sort_guarded_bubble(slots, len_slot, signed)?;
        }

        let done_target = self.instrs.len();
        self.patch_target(done, done_target)?;
        Ok(())
    }

    pub(super) fn lower_runtime_array_sort_len_dispatch(
        &mut self,
        slots: &[usize],
        len_slot: usize,
        signed: bool,
    ) -> RuntimeGenericLowerResult<()> {
        let mut done_jumps = Vec::with_capacity(slots.len() + 1);
        for k in 0..=slots.len() {
            let miss = self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::Eq,
                lhs: RuntimeOperand::Slot(len_slot),
                rhs: RuntimeOperand::Imm(k as u64),
                target: usize::MAX,
            });

            let active = &slots[..k];
            if let Some(pairs) = runtime_fixed_sort_pairs(k) {
                self.lower_runtime_array_sort_pairs_fixed(active, pairs.as_slice(), signed);
            } else {
                self.lower_runtime_array_sort_fixed_bubble(active, signed);
            }
            done_jumps.push(self.emit(RuntimeInstr::Jump { target: usize::MAX }));

            let next = self.instrs.len();
            self.patch_target(miss, next)?;
        }

        self.emit(RuntimeInstr::Exit {
            code: RuntimeOperand::Imm(255),
        });
        let done_target = self.instrs.len();
        for jump in done_jumps {
            self.patch_target(jump, done_target)?;
        }
        Ok(())
    }

    pub(super) fn lower_runtime_array_sort_pairs_fixed(
        &mut self,
        slots: &[usize],
        pairs: &[(usize, usize)],
        signed: bool,
    ) {
        for (left_idx, right_idx) in pairs.iter().copied() {
            self.emit(RuntimeInstr::CompareSwap {
                left: slots[left_idx],
                right: slots[right_idx],
                signed,
            });
        }
    }

    pub(super) fn lower_runtime_array_sort_pairs_guarded(
        &mut self,
        slots: &[usize],
        pairs: &[(usize, usize)],
        len_slot: usize,
        signed: bool,
    ) -> RuntimeGenericLowerResult<()> {
        for (left_idx, right_idx) in pairs.iter().copied() {
            let required_len = left_idx.max(right_idx) as u64;
            let in_len = self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::GtUnsigned,
                lhs: RuntimeOperand::Slot(len_slot),
                rhs: RuntimeOperand::Imm(required_len),
                target: usize::MAX,
            });
            self.emit(RuntimeInstr::CompareSwap {
                left: slots[left_idx],
                right: slots[right_idx],
                signed,
            });
            let next = self.instrs.len();
            self.patch_target(in_len, next)?;
        }
        Ok(())
    }

    pub(super) fn lower_runtime_array_sort_fixed_bubble(&mut self, slots: &[usize], signed: bool) {
        for pass in 0..slots.len() {
            let upper = slots.len() - 1 - pass;
            for j in 0..upper {
                self.emit(RuntimeInstr::CompareSwap {
                    left: slots[j],
                    right: slots[j + 1],
                    signed,
                });
            }
        }
    }

    pub(super) fn lower_runtime_array_sort_guarded_bubble(
        &mut self,
        slots: &[usize],
        len_slot: usize,
        signed: bool,
    ) -> RuntimeGenericLowerResult<()> {
        for pass in 0..slots.len() {
            let upper = slots.len() - 1 - pass;
            for j in 0..upper {
                let in_len = self.emit(RuntimeInstr::JumpIfCmpFalse {
                    op: RuntimeCmpOp::GtUnsigned,
                    lhs: RuntimeOperand::Slot(len_slot),
                    rhs: RuntimeOperand::Imm((j + 1) as u64),
                    target: usize::MAX,
                });
                self.emit(RuntimeInstr::CompareSwap {
                    left: slots[j],
                    right: slots[j + 1],
                    signed,
                });
                let next = self.instrs.len();
                self.patch_target(in_len, next)?;
            }
        }
        Ok(())
    }

    pub(super) fn try_lower_assign_in_place_chain(
        &mut self,
        target_name: &str,
        target_slot: usize,
        target_ty: RuntimeIntType,
        expr: &Expr,
    ) -> RuntimeGenericLowerResult<bool> {
        let mut ops = Vec::new();
        if !collect_runtime_generic_in_place_ops(target_name, expr, &mut ops) || ops.is_empty() {
            return Ok(false);
        }
        for (op, rhs_expr) in ops {
            let rhs = self.lower_expr_as_type(rhs_expr, target_ty)?;
            self.emit(RuntimeInstr::BinOpInPlace {
                dst: target_slot,
                op,
                rhs,
            });
            self.normalize_slot(target_slot, target_ty);
        }
        Ok(true)
    }

    pub(super) fn stage_runtime_struct_literal(
        &mut self,
        struct_name: &str,
        fields: &[StructInitField],
        prefix: &str,
        target_fields: &HashMap<String, (usize, RuntimeScalarType)>,
        span: Span,
        staged: &mut Vec<(usize, usize, RuntimeScalarType)>,
    ) -> RuntimeGenericLowerResult<()> {
        let layout = self
            .struct_layouts
            .get(struct_name)
            .ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("unknown struct '{struct_name}'").as_str(),
                    span,
                ))
            })?
            .clone();
        let mut by_name: HashMap<String, &Expr> = HashMap::with_capacity(fields.len());
        for field in fields {
            if by_name.insert(field.name.clone(), &field.expr).is_some() {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("duplicate field '{}' in struct literal", field.name).as_str(),
                    field.span,
                )));
            }
            if !layout.iter().any(|entry| entry.name == field.name) {
                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("unknown struct field '{}'", field.name).as_str(),
                    field.span,
                )));
            }
        }
        for layout_field in &layout {
            let field_expr = by_name.get(&layout_field.name).copied().ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("missing field '{}' in struct literal", layout_field.name).as_str(),
                    span,
                ))
            })?;
            let path = if prefix.is_empty() {
                layout_field.name.clone()
            } else {
                format!("{prefix}.{}", layout_field.name)
            };
            match &layout_field.ty {
                TypeName::Struct(child_name) => {
                    let Expr::StructInit { name, fields, .. } = field_expr else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!(
                                "nested field '{}' requires a '{}' struct literal in runtime-native lowering",
                                path, child_name
                            )
                            .as_str(),
                            span,
                        )));
                    };
                    if name != child_name {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!(
                                "nested field '{}' requires '{}', found '{name}'",
                                path, child_name
                            )
                            .as_str(),
                            span,
                        )));
                    }
                    self.stage_runtime_struct_literal(
                        child_name,
                        fields,
                        &path,
                        target_fields,
                        span,
                        staged,
                    )?;
                }
                ty => {
                    let field_ty = ensure_runtime_generic_scalar_type(ty, span).map_err(|_| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            format!(
                                "runtime native struct-list field '{path}' must be scalar or a nested struct"
                            )
                            .as_str(),
                            span,
                        ))
                    })?;
                    let (dst_slot, target_ty) =
                        target_fields.get(&path).copied().ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("missing runtime storage for struct field '{path}'")
                                    .as_str(),
                                span,
                            ))
                        })?;
                    if field_ty != target_ty {
                        return Err(RuntimeGenericLowerError::Unsupported);
                    }
                    let src = self.lower_expr_as_scalar(field_expr, field_ty)?;
                    let tmp_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov { dst: tmp_slot, src });
                    self.normalize_scalar_slot(tmp_slot, field_ty);
                    staged.push((dst_slot, tmp_slot, field_ty));
                }
            }
        }
        Ok(())
    }

    pub(super) fn lower_struct_slot_assign(
        &mut self,
        target_name: &str,
        target_fields: &HashMap<String, (usize, RuntimeScalarType)>,
        expr: &Expr,
        span: Span,
    ) -> RuntimeGenericLowerResult<()> {
        let mut field_names: Vec<String> = target_fields.keys().cloned().collect();
        field_names.sort_unstable();

        let mut staged: Vec<(usize, usize, RuntimeScalarType)> =
            Vec::with_capacity(field_names.len());

        match expr {
            Expr::StructInit { name, fields, .. } => {
                self.stage_runtime_struct_literal(
                    name,
                    fields,
                    "",
                    target_fields,
                    span,
                    &mut staged,
                )?;
            }
            Expr::Ident {
                name: source_name,
                span: source_span,
            } => {
                enum StructSource {
                    Slots(HashMap<String, (usize, RuntimeScalarType)>),
                    Const(HashMap<String, RuntimeConstInt>),
                }

                let source = {
                    let binding = self.scopes.get(source_name).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                            format!("unknown identifier '{source_name}'"),
                            source_span,
                        ))
                    })?;
                    match binding {
                        RuntimeGenericBinding::StructSlots { fields, .. } => {
                            StructSource::Slots(fields.clone())
                        }
                        RuntimeGenericBinding::ConstContainer {
                            container: RuntimeConstContainer::Struct { fields, .. },
                        } => StructSource::Const(fields.clone()),
                        _ => {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!(
                                    "runtime generic type mismatch: expected struct, found '{}'",
                                    source_name
                                )
                                .as_str(),
                                *source_span,
                            )));
                        }
                    }
                };

                match source {
                    StructSource::Slots(source_fields) => {
                        for field_name in &field_names {
                            let (dst_slot, dst_ty) = target_fields
                                .get(field_name)
                                .copied()
                                .ok_or(RuntimeGenericLowerError::Unsupported)?;
                            let (src_slot, src_ty) = source_fields.get(field_name).copied().ok_or_else(|| {
                                RuntimeGenericLowerError::Diagnostic(type_error(
                                    format!(
                                        "type mismatch assigning '{}' from '{}': missing field '{}'",
                                        target_name, source_name, field_name
                                    )
                                    .as_str(),
                                    span,
                                ))
                            })?;
                            if src_ty != dst_ty {
                                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                    format!(
                                        "type mismatch assigning '{}' from '{}': field '{}' differs ({}, {})",
                                        target_name,
                                        source_name,
                                        field_name,
                                        dst_ty.display(),
                                        src_ty.display()
                                    )
                                    .as_str(),
                                    span,
                                )));
                            }
                            let tmp_slot = self.alloc_slot();
                            self.emit(RuntimeInstr::Mov {
                                dst: tmp_slot,
                                src: RuntimeOperand::Slot(src_slot),
                            });
                            self.normalize_scalar_slot(tmp_slot, dst_ty);
                            staged.push((dst_slot, tmp_slot, dst_ty));
                        }
                    }
                    StructSource::Const(source_fields) => {
                        for field_name in &field_names {
                            let (dst_slot, dst_ty) = target_fields
                                .get(field_name)
                                .copied()
                                .ok_or(RuntimeGenericLowerError::Unsupported)?;
                            let value = source_fields.get(field_name).copied().ok_or_else(|| {
                                RuntimeGenericLowerError::Diagnostic(type_error(
                                    format!(
                                        "type mismatch assigning '{}' from '{}': missing field '{}'",
                                        target_name, source_name, field_name
                                    )
                                    .as_str(),
                                    span,
                                ))
                            })?;
                            if RuntimeScalarType::Int(value.ty) != dst_ty {
                                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                    format!(
                                        "type mismatch assigning '{}' from '{}': field '{}' differs ({}, {})",
                                        target_name,
                                        source_name,
                                        field_name,
                                        dst_ty.display(),
                                        RuntimeScalarType::Int(value.ty).display()
                                    )
                                    .as_str(),
                                    span,
                                )));
                            }
                            let tmp_slot = self.alloc_slot();
                            self.emit(RuntimeInstr::Mov {
                                dst: tmp_slot,
                                src: RuntimeOperand::Imm(value.encoded),
                            });
                            self.normalize_scalar_slot(tmp_slot, dst_ty);
                            staged.push((dst_slot, tmp_slot, dst_ty));
                        }
                    }
                }
            }
            _ => {
                return Err(RuntimeGenericLowerError::Unsupported);
            }
        }

        for (dst_slot, tmp_slot, field_ty) in staged {
            self.emit(RuntimeInstr::Mov {
                dst: dst_slot,
                src: RuntimeOperand::Slot(tmp_slot),
            });
            self.normalize_scalar_slot(dst_slot, field_ty);
        }
        Ok(())
    }

    pub(super) fn try_build_const_container_binding(
        &mut self,
        mutable: bool,
        ty_hint: Option<&TypeName>,
        expr: &Expr,
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<RuntimeGenericBinding>> {
        match expr {
            Expr::StructInit {
                name: struct_name,
                fields,
                ..
            } => {
                let resolved_name = match ty_hint {
                    Some(TypeName::Struct(name)) => {
                        if name != struct_name {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "type hint mismatch for struct literal",
                                span,
                            )));
                        }
                        name.clone()
                    }
                    Some(_) => {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "type hint mismatch for struct literal",
                            span,
                        )));
                    }
                    None => struct_name.clone(),
                };
                let layout = self.struct_layouts.get(&resolved_name).ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("unknown struct '{}'", resolved_name).as_str(),
                        span,
                    ))
                })?;
                if layout
                    .iter()
                    .any(|field| matches!(field.ty, TypeName::Struct(_)))
                {
                    let mut runtime_fields = HashMap::new();
                    for (field_name, field_ty) in
                        self.runtime_struct_scalar_fields(&resolved_name, span)?
                    {
                        runtime_fields.insert(field_name, (self.alloc_slot(), field_ty));
                    }
                    self.lower_struct_slot_assign(&resolved_name, &runtime_fields, expr, span)?;
                    return Ok(Some(RuntimeGenericBinding::StructSlots {
                        struct_name: resolved_name,
                        fields: runtime_fields,
                        mutable,
                    }));
                }
                let mut layout_fields = Vec::with_capacity(layout.len());
                let mut layout_ty_by_name = HashMap::with_capacity(layout.len());
                for item in layout {
                    let field_ty =
                        ensure_runtime_generic_scalar_type(&item.ty, span).map_err(|_| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                format!(
                                    "runtime generic struct field '{}' must be scalar",
                                    item.name
                                )
                                .as_str(),
                                span,
                            ))
                        })?;
                    layout_fields.push((item.name.clone(), field_ty));
                    layout_ty_by_name.insert(item.name.clone(), field_ty);
                }

                let mut by_name = HashMap::with_capacity(fields.len());
                for field in fields {
                    if by_name.insert(field.name.clone(), &field.expr).is_some() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!("duplicate field '{}' in struct literal", field.name).as_str(),
                            field.span,
                        )));
                    }
                    let expected_ty =
                        layout_ty_by_name.get(&field.name).copied().ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("unknown struct field '{}'", field.name).as_str(),
                                field.span,
                            ))
                        })?;
                    let _ = expected_ty;
                }
                for (field_name, _) in &layout_fields {
                    if !by_name.contains_key(field_name) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!("missing field '{}' in struct literal", field_name).as_str(),
                            span,
                        )));
                    }
                }

                if !mutable {
                    let mut const_fields = HashMap::with_capacity(layout_fields.len());
                    let mut all_const = true;
                    for (field_name, field_ty) in &layout_fields {
                        let field_expr = by_name
                            .get(field_name)
                            .copied()
                            .ok_or(RuntimeGenericLowerError::Unsupported)?;
                        let RuntimeScalarType::Int(field_int_ty) = field_ty else {
                            all_const = false;
                            break;
                        };
                        match runtime_const_int_from_expr(field_expr, Some(*field_int_ty)) {
                            Ok(value) => {
                                const_fields.insert(field_name.clone(), value);
                            }
                            Err(RuntimeGenericLowerError::Unsupported) => {
                                all_const = false;
                                break;
                            }
                            Err(err) => return Err(err),
                        }
                    }
                    if all_const {
                        return Ok(Some(RuntimeGenericBinding::ConstContainer {
                            container: RuntimeConstContainer::Struct {
                                struct_name: resolved_name.clone(),
                                fields: const_fields,
                            },
                        }));
                    }
                }

                let mut runtime_fields = HashMap::with_capacity(layout_fields.len());
                for (field_name, field_ty) in layout_fields {
                    let field_expr = by_name
                        .get(&field_name)
                        .copied()
                        .ok_or(RuntimeGenericLowerError::Unsupported)?;
                    let src = self.lower_expr_as_scalar(field_expr, field_ty)?;
                    let slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov { dst: slot, src });
                    self.normalize_scalar_slot(slot, field_ty);
                    runtime_fields.insert(field_name, (slot, field_ty));
                }
                return Ok(Some(RuntimeGenericBinding::StructSlots {
                    struct_name: resolved_name,
                    fields: runtime_fields,
                    mutable,
                }));
            }
            Expr::ArrayLit { elems, .. } => {
                let expected = match ty_hint {
                    Some(TypeName::Array { elem, len }) => {
                        if *len != elems.len() as u64 {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "runtime generic array length mismatch",
                                span,
                            )));
                        }
                        Some(
                            ensure_runtime_generic_int_type(elem.as_ref(), span)
                                .map_err(RuntimeGenericLowerError::Diagnostic)?,
                        )
                    }
                    Some(_) => {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "type hint mismatch for array literal",
                            span,
                        )));
                    }
                    None => None,
                };
                let mut values = Vec::with_capacity(elems.len());
                for elem in elems {
                    let value = runtime_const_int_from_expr(elem, expected)?;
                    values.push(value);
                }
                if values.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime generic array literal cannot be empty",
                        span,
                    )));
                }
                let first_ty = values[0].ty;
                if values.iter().any(|v| v.ty != first_ty) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime generic array elements must have one integer type",
                        span,
                    )));
                }
                const IMMUTABLE_ARRAY_SLOT_MAX: usize = 64;
                if !mutable && values.len() > IMMUTABLE_ARRAY_SLOT_MAX {
                    return Ok(Some(RuntimeGenericBinding::ConstContainer {
                        container: RuntimeConstContainer::Array { elems: values },
                    }));
                }
                // Materialize immutable arrays as slot-backed too so dynamic indexing
                // lowers to LoadIndex/StoreIndex-style kernels instead of compare chains.
                let mut slots = Vec::with_capacity(values.len());
                for value in values {
                    let slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: slot,
                        src: RuntimeOperand::Imm(value.encoded),
                    });
                    self.normalize_slot(slot, value.ty);
                    slots.push(slot);
                }
                let len_slot = self.alloc_slot();
                self.emit(RuntimeInstr::Mov {
                    dst: len_slot,
                    src: RuntimeOperand::Imm(slots.len() as u64),
                });
                Ok(Some(RuntimeGenericBinding::ArraySlots {
                    slots,
                    len_slot,
                    mutable,
                    elem_ty: first_ty,
                    full_len_known: true,
                }))
            }
            Expr::DictLit { entries, .. } => {
                let expected = match ty_hint {
                    Some(TypeName::Dict { key, value }) => {
                        if !matches!(key.as_ref(), TypeName::String) {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "runtime generic dictionary keys must be string",
                                span,
                            )));
                        }
                        Some(
                            ensure_runtime_generic_int_type(value.as_ref(), span)
                                .map_err(RuntimeGenericLowerError::Diagnostic)?,
                        )
                    }
                    Some(_) => {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "type hint mismatch for dictionary literal",
                            span,
                        )));
                    }
                    None => None,
                };
                let mut map = HashMap::new();
                for entry in entries {
                    let value = runtime_const_int_from_expr(&entry.value, expected)?;
                    map.insert(entry.key.clone(), value);
                }
                if map.is_empty() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime generic dictionary literal cannot be empty",
                        span,
                    )));
                }
                let first_ty = map.values().next().expect("non-empty").ty;
                if map.values().any(|v| v.ty != first_ty) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime generic dictionary values must have one integer type",
                        span,
                    )));
                }
                if mutable {
                    let mut slots = HashMap::new();
                    for (key, value) in map {
                        let slot = self.alloc_slot();
                        self.emit(RuntimeInstr::Mov {
                            dst: slot,
                            src: RuntimeOperand::Imm(value.encoded),
                        });
                        self.normalize_slot(slot, value.ty);
                        slots.insert(key, slot);
                    }
                    return Ok(Some(RuntimeGenericBinding::DictSlots {
                        entries: slots,
                        mutable: true,
                        value_ty: first_ty,
                    }));
                }
                Ok(Some(RuntimeGenericBinding::ConstContainer {
                    container: RuntimeConstContainer::Dict { entries: map },
                }))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn lower_const_struct_field(
        &self,
        base: &Expr,
        field: &str,
        span: Span,
    ) -> RuntimeGenericLowerResult<RuntimeConstInt> {
        let Expr::Ident { name, .. } = base else {
            return Err(RuntimeGenericLowerError::Unsupported);
        };
        let binding = self.scopes.get(name).ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                format!("unknown identifier '{name}'").as_str(),
                span,
            ))
        })?;
        let Some(RuntimeConstContainer::Struct { fields, .. }) = binding.container() else {
            return Err(RuntimeGenericLowerError::Unsupported);
        };
        fields.get(field).copied().ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                format!("unknown struct field '{field}'").as_str(),
                span,
            ))
        })
    }

    pub(super) fn lower_struct_slot_field_access(
        &self,
        base: &Expr,
        field: &str,
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<(RuntimeOperand, RuntimeScalarType)>> {
        fn root_and_path<'a>(expr: &'a Expr, tail: &str) -> Option<(&'a str, String)> {
            match expr {
                Expr::Ident { name, .. } => Some((name, tail.to_owned())),
                Expr::FieldAccess { base, field, .. } => {
                    let nested_tail = format!("{field}.{tail}");
                    root_and_path(base, &nested_tail)
                }
                _ => None,
            }
        }
        let Some((name, field_path)) = root_and_path(base, field) else {
            return Ok(None);
        };
        let binding = self.scopes.get(name).ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                format!("unknown identifier '{name}'").as_str(),
                span,
            ))
        })?;
        if let RuntimeGenericBinding::MovedResource { kind } = binding {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                format!("{kind} '{name}' was moved or consumed and cannot be used again").as_str(),
                span,
            )));
        }
        let fields = match binding {
            RuntimeGenericBinding::StructSlots { fields, .. }
            | RuntimeGenericBinding::OwnedStruct {
                scalar_fields: fields,
                ..
            } => fields,
            _ => return Ok(None),
        };
        let (slot, ty) = fields.get(&field_path).copied().ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                format!("unknown struct field '{field_path}'").as_str(),
                span,
            ))
        })?;
        Ok(Some((RuntimeOperand::Slot(slot), ty)))
    }

    pub(super) fn lower_const_index_access(
        &self,
        base: &Expr,
        index: &Expr,
        span: Span,
    ) -> RuntimeGenericLowerResult<RuntimeConstInt> {
        let Expr::Ident { name, .. } = base else {
            return Err(RuntimeGenericLowerError::Unsupported);
        };
        let binding = self.scopes.get(name).ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                format!("unknown identifier '{name}'").as_str(),
                span,
            ))
        })?;
        match binding.container() {
            Some(RuntimeConstContainer::Array { elems }) => {
                let idx = runtime_const_array_index(index)?;
                elems.get(idx).copied().ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(type_error(
                        "array index out of bounds in runtime generic lowering",
                        span,
                    ))
                })
            }
            Some(RuntimeConstContainer::Dict { entries }) => {
                let key = runtime_const_dict_key(index).ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(type_error(
                        "dictionary index must be a string literal in runtime generic lowering",
                        span,
                    ))
                })?;
                entries.get(&key).copied().ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("unknown dictionary key '{key}'").as_str(),
                        span,
                    ))
                })
            }
            _ => Err(RuntimeGenericLowerError::Unsupported),
        }
    }

    pub(super) fn lower_array_slot_index_access(
        &mut self,
        base: &Expr,
        index: &Expr,
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<(RuntimeOperand, RuntimeIntType)>> {
        let Expr::Ident { name, .. } = base else {
            return Ok(None);
        };
        let Some((slots, len_slot, _, elem_ty, full_len_known)) = self
            .scopes
            .get(name)
            .and_then(RuntimeGenericBinding::as_array_slots)
        else {
            return Ok(None);
        };

        if let Ok(idx) = runtime_const_array_index(index) {
            let dst = slots.get(idx).copied().ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "array index out of bounds in runtime generic lowering",
                    span,
                ))
            })?;
            if full_len_known {
                return Ok(Some((RuntimeOperand::Slot(dst), elem_ty)));
            }
            let in_bounds = self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtUnsigned,
                lhs: RuntimeOperand::Imm(idx as u64),
                rhs: RuntimeOperand::Slot(len_slot),
                target: usize::MAX,
            });
            let tmp = self.alloc_slot();
            self.emit(RuntimeInstr::Mov {
                dst: tmp,
                src: RuntimeOperand::Slot(dst),
            });
            self.normalize_slot(tmp, elem_ty);
            let done = self.emit(RuntimeInstr::Jump { target: usize::MAX });
            let oob_target = self.instrs.len();
            self.emit(RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(255),
            });
            let done_target = self.instrs.len();
            self.patch_target(in_bounds, oob_target)?;
            self.patch_target(done, done_target)?;
            return Ok(Some((RuntimeOperand::Slot(tmp), elem_ty)));
        }

        let idx_ty = self.infer_expr_int_type(index).map_err(|err| match err {
            RuntimeGenericLowerError::Unsupported => {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "array index must be integer in runtime generic lowering",
                    span,
                ))
            }
            other => other,
        })?;
        if full_len_known {
            if let Some(iter_slot) = self.loop_unchecked_index_slot(name, index) {
                let dst = self.alloc_slot();
                self.emit(RuntimeInstr::LoadIndexUnchecked {
                    dst,
                    base_slots: slots.clone(),
                    index: RuntimeOperand::Slot(iter_slot),
                });
                return Ok(Some((RuntimeOperand::Slot(dst), elem_ty)));
            }
        }

        let statically_in_bounds =
            full_len_known && self.index_expr_proven_in_bounds(index, idx_ty, slots.len());
        let idx_op = self.lower_expr_as_type(index, idx_ty)?;

        let dst = self.alloc_slot();
        self.emit(if statically_in_bounds {
            RuntimeInstr::LoadIndexUnchecked {
                dst,
                base_slots: slots.clone(),
                index: idx_op,
            }
        } else {
            RuntimeInstr::LoadIndex {
                dst,
                base_slots: slots.clone(),
                index: idx_op,
            }
        });
        Ok(Some((RuntimeOperand::Slot(dst), elem_ty)))
    }

    pub(super) fn lower_dict_slot_index_access(
        &self,
        base: &Expr,
        index: &Expr,
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<(RuntimeOperand, RuntimeIntType)>> {
        let Expr::Ident { name, .. } = base else {
            return Ok(None);
        };
        let Some((entries, _, value_ty)) = self
            .scopes
            .get(name)
            .and_then(RuntimeGenericBinding::as_dict_slots)
        else {
            return Ok(None);
        };
        let key = runtime_const_dict_key(index).ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                "runtime generic mutable dictionary index must be a string literal",
                span,
            ))
        })?;
        let slot = entries.get(&key).copied().ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                format!("unknown dictionary key '{key}'").as_str(),
                span,
            ))
        })?;
        Ok(Some((RuntimeOperand::Slot(slot), value_ty)))
    }

    pub(super) fn lower_const_array_dynamic_index(
        &mut self,
        base: &Expr,
        index: &Expr,
        span: Span,
    ) -> RuntimeGenericLowerResult<Option<(RuntimeOperand, RuntimeIntType)>> {
        let Some(RuntimeConstContainer::Array { elems }) = self.resolve_const_container(base)
        else {
            return Ok(None);
        };
        if runtime_const_array_index(index).is_ok() {
            return Ok(None);
        }

        let elems = elems.clone();
        let elem_ty = elems.first().map(|elem| elem.ty).ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(type_error(
                "runtime generic array literal cannot be empty",
                span,
            ))
        })?;
        let idx_ty = self.infer_expr_int_type(index).map_err(|err| match err {
            RuntimeGenericLowerError::Unsupported => {
                RuntimeGenericLowerError::Diagnostic(type_error(
                    "array index must be integer in runtime generic lowering",
                    span,
                ))
            }
            other => other,
        })?;
        let idx = self.lower_expr_as_type(index, idx_ty)?;

        let len_imm = idx_ty.encode_value(
            Value::UInt {
                bits: 64,
                value: elems.len() as u128,
            },
            span,
        )?;

        let mut oob_checks = Vec::new();
        if idx_ty.signed {
            let zero_imm = idx_ty.encode_value(Value::Int { bits: 64, value: 0 }, span)?;
            oob_checks.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::GeSigned,
                lhs: idx,
                rhs: RuntimeOperand::Imm(zero_imm),
                target: usize::MAX,
            }));
            oob_checks.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtSigned,
                lhs: idx,
                rhs: RuntimeOperand::Imm(len_imm),
                target: usize::MAX,
            }));
        } else {
            oob_checks.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::LtUnsigned,
                lhs: idx,
                rhs: RuntimeOperand::Imm(len_imm),
                target: usize::MAX,
            }));
        }

        let dst = self.alloc_slot();
        let mut done_jumps = Vec::with_capacity(elems.len());
        for (i, elem) in elems.iter().enumerate() {
            let idx_imm = idx_ty.encode_value(
                Value::UInt {
                    bits: 64,
                    value: i as u128,
                },
                span,
            )?;
            let miss = self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: RuntimeCmpOp::Eq,
                lhs: idx,
                rhs: RuntimeOperand::Imm(idx_imm),
                target: usize::MAX,
            });
            self.emit(RuntimeInstr::Mov {
                dst,
                src: RuntimeOperand::Imm(elem.encoded),
            });
            self.normalize_slot(dst, elem.ty);
            done_jumps.push(self.emit(RuntimeInstr::Jump { target: usize::MAX }));
            let next_case = self.instrs.len();
            self.patch_target(miss, next_case)?;
        }

        let oob_target = self.instrs.len();
        self.emit(RuntimeInstr::Exit {
            code: RuntimeOperand::Imm(255),
        });
        let done_target = self.instrs.len();

        for check in oob_checks {
            self.patch_target(check, oob_target)?;
        }
        for done in done_jumps {
            self.patch_target(done, done_target)?;
        }
        Ok(Some((RuntimeOperand::Slot(dst), elem_ty)))
    }

    pub(super) fn resolve_const_container(&self, expr: &Expr) -> Option<&RuntimeConstContainer> {
        let Expr::Ident { name, .. } = expr else {
            return None;
        };
        self.scopes
            .get(name)
            .and_then(|binding| binding.container())
    }
}
