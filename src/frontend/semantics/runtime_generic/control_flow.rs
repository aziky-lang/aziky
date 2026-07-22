//! Runtime-native function, statement, loop, and control-flow lowering.

use super::*;

impl<'a> RuntimeGenericBuilder<'a> {
    pub(super) fn push_loop_frame(&mut self, continue_target: Option<usize>) {
        self.loop_frames.push(RuntimeLoopFrame {
            continue_target,
            scope_depth: self.scopes.depth().saturating_sub(1),
            continue_patches: Vec::new(),
            break_patches: Vec::new(),
        });
    }

    pub(super) fn pop_loop_frame(&mut self) -> RuntimeGenericLowerResult<RuntimeLoopFrame> {
        self.loop_frames
            .pop()
            .ok_or(RuntimeGenericLowerError::Unsupported)
    }

    pub(super) fn emit_break_jump(&mut self, span: Span) -> RuntimeGenericLowerResult<()> {
        if self.loop_frames.is_empty() {
            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                "break used outside loop",
                span,
            )));
        }
        let scope_depth = self
            .loop_frames
            .last()
            .map(|frame| frame.scope_depth)
            .ok_or(RuntimeGenericLowerError::Unsupported)?;
        self.emit_owned_cleanup_from(scope_depth);
        let jump = self.emit(RuntimeInstr::Jump { target: usize::MAX });
        if let Some(frame) = self.loop_frames.last_mut() {
            frame.break_patches.push(jump);
        }
        Ok(())
    }

    pub(super) fn emit_continue_jump(&mut self, span: Span) -> RuntimeGenericLowerResult<()> {
        let (continue_target, scope_depth) = self
            .loop_frames
            .last()
            .ok_or_else(|| {
                RuntimeGenericLowerError::Diagnostic(type_error("continue used outside loop", span))
            })
            .map(|frame| (frame.continue_target, frame.scope_depth))?;
        self.emit_owned_cleanup_from(scope_depth);
        match continue_target {
            Some(target) => {
                self.emit(RuntimeInstr::Jump { target });
            }
            None => {
                let jump = self.emit(RuntimeInstr::Jump { target: usize::MAX });
                if let Some(frame) = self.loop_frames.last_mut() {
                    frame.continue_patches.push(jump);
                }
            }
        }
        Ok(())
    }

    pub(super) fn patch_loop_frame_targets(
        &mut self,
        frame: RuntimeLoopFrame,
        continue_target: usize,
        break_target: usize,
    ) -> RuntimeGenericLowerResult<()> {
        for patch in frame.continue_patches {
            self.patch_target(patch, continue_target)?;
        }
        for patch in frame.break_patches {
            self.patch_target(patch, break_target)?;
        }
        Ok(())
    }

    pub(super) fn collect_loop_bce_candidate_arrays(&self) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for scope in self.scopes.scopes.iter().rev() {
            for (name, binding) in scope {
                if !seen.insert(name.clone()) {
                    continue;
                }
                let Some((slots, _, _, _, full_len_known)) = binding.as_array_slots() else {
                    continue;
                };
                if !full_len_known {
                    continue;
                }
                out.push((name.clone(), slots.len()));
            }
        }
        out
    }

    pub(super) fn push_unchecked_loop_array_accesses(
        &mut self,
        iter_name: &str,
        iter_slot: usize,
        iter_ty: RuntimeIntType,
        start: RuntimeOperand,
        end: RuntimeOperand,
        span: Span,
    ) -> RuntimeGenericLowerResult<usize> {
        let candidates = self.collect_loop_bce_candidate_arrays();
        if candidates.is_empty() {
            return Ok(0);
        }

        let cmp_lt = iter_ty
            .cmp_from_binary(BinaryOp::Lt)
            .ok_or(RuntimeGenericLowerError::Unsupported)?;
        let cmp_le = iter_ty
            .cmp_from_binary(BinaryOp::Le)
            .ok_or(RuntimeGenericLowerError::Unsupported)?;

        let mut added = 0usize;
        for (array_name, array_len) in candidates {
            let Ok(len_imm) = iter_ty.encode_value(
                Value::UInt {
                    bits: 64,
                    value: array_len as u128,
                },
                span,
            ) else {
                continue;
            };

            // If the range is empty, skip bounds guards (no indexed access can execute).
            let skip_guards = self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: cmp_lt,
                lhs: start,
                rhs: end,
                target: usize::MAX,
            });

            let mut guard_patches = Vec::with_capacity(if iter_ty.signed { 2 } else { 1 });
            if iter_ty.signed {
                let zero_imm = iter_ty.encode_value(Value::Int { bits: 64, value: 0 }, span)?;
                guard_patches.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
                    op: RuntimeCmpOp::GeSigned,
                    lhs: start,
                    rhs: RuntimeOperand::Imm(zero_imm),
                    target: usize::MAX,
                }));
            }
            guard_patches.push(self.emit(RuntimeInstr::JumpIfCmpFalse {
                op: cmp_le,
                lhs: end,
                rhs: RuntimeOperand::Imm(len_imm),
                target: usize::MAX,
            }));

            let jump_after_oob = self.emit(RuntimeInstr::Jump { target: usize::MAX });
            let oob_target = self.instrs.len();
            self.emit(RuntimeInstr::Exit {
                code: RuntimeOperand::Imm(255),
            });
            let after_oob = self.instrs.len();
            self.patch_target(skip_guards, after_oob)?;
            for patch in guard_patches {
                self.patch_target(patch, oob_target)?;
            }
            self.patch_target(jump_after_oob, after_oob)?;

            self.unchecked_array_loop_accesses
                .push(RuntimeUncheckedArrayLoopAccess {
                    iter_name: iter_name.to_string(),
                    iter_slot,
                    array_name,
                });
            added += 1;
        }
        Ok(added)
    }

    pub(super) fn pop_unchecked_loop_array_accesses(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let keep = self
            .unchecked_array_loop_accesses
            .len()
            .saturating_sub(count);
        self.unchecked_array_loop_accesses.truncate(keep);
    }

    pub(super) fn loop_unchecked_index_slot(
        &self,
        array_name: &str,
        index: &Expr,
    ) -> Option<usize> {
        let Expr::Ident {
            name: index_name, ..
        } = index
        else {
            return None;
        };
        self.unchecked_array_loop_accesses
            .iter()
            .rev()
            .find(|assumption| {
                assumption.array_name == array_name && assumption.iter_name == *index_name
            })
            .map(|assumption| assumption.iter_slot)
    }

    pub(super) fn expr_unsigned_upper_bound(&self, expr: &Expr, depth: u8) -> Option<u64> {
        if depth == 0 {
            return None;
        }
        match expr {
            Expr::Bool { value, .. } => Some(u64::from(*value)),
            Expr::Number { .. } => runtime_const_array_index(expr)
                .ok()
                .and_then(|idx| u64::try_from(idx).ok()),
            Expr::Unary {
                op: UnaryOp::Plus,
                expr,
                ..
            } => self.expr_unsigned_upper_bound(expr, depth - 1),
            Expr::Ident { name, .. } => {
                let (slot, _, ty) = self.scopes.get(name)?.as_scalar()?;
                let RuntimeScalarType::Int(int_ty) = ty else {
                    return None;
                };
                if int_ty.signed {
                    return None;
                }
                let type_max = max_unsigned_for_bits(int_ty.bits);
                self.slot_unsigned_upper_bounds
                    .get(&slot)
                    .copied()
                    .map(|bound| bound.min(type_max))
            }
            Expr::Binary {
                op, left, right, ..
            } => match op {
                BinaryOp::Add => {
                    let lhs = self.expr_unsigned_upper_bound(left, depth - 1)?;
                    let rhs = self.expr_unsigned_upper_bound(right, depth - 1)?;
                    lhs.checked_add(rhs)
                }
                BinaryOp::Mul => {
                    let lhs = self.expr_unsigned_upper_bound(left, depth - 1)?;
                    let rhs = self.expr_unsigned_upper_bound(right, depth - 1)?;
                    lhs.checked_mul(rhs)
                }
                BinaryOp::Div => {
                    let lhs = self.expr_unsigned_upper_bound(left, depth - 1)?;
                    let rhs = runtime_const_array_index(right)
                        .ok()
                        .and_then(|value| u64::try_from(value).ok())?;
                    if rhs == 0 { None } else { Some(lhs / rhs) }
                }
                BinaryOp::Mod => {
                    let rhs = runtime_const_array_index(right)
                        .ok()
                        .and_then(|value| u64::try_from(value).ok())?;
                    if rhs == 0 { None } else { Some(rhs - 1) }
                }
                BinaryOp::BitAnd => {
                    let lhs = self.expr_unsigned_upper_bound(left, depth - 1);
                    let rhs = self.expr_unsigned_upper_bound(right, depth - 1);
                    match (lhs, rhs) {
                        (Some(l), Some(r)) => Some(l.min(r)),
                        (Some(l), None) => Some(l),
                        (None, Some(r)) => Some(r),
                        (None, None) => None,
                    }
                }
                BinaryOp::BitOr => {
                    let lhs = self.expr_unsigned_upper_bound(left, depth - 1)?;
                    let rhs = self.expr_unsigned_upper_bound(right, depth - 1)?;
                    Some(lhs | rhs)
                }
                BinaryOp::Shl => {
                    let lhs = self.expr_unsigned_upper_bound(left, depth - 1)?;
                    let shift = runtime_const_array_index(right)
                        .ok()
                        .and_then(|value| u32::try_from(value).ok())?;
                    lhs.checked_shl(shift)
                }
                BinaryOp::Shr => {
                    let lhs = self.expr_unsigned_upper_bound(left, depth - 1)?;
                    let shift = runtime_const_array_index(right)
                        .ok()
                        .and_then(|value| u32::try_from(value).ok())?;
                    if shift >= 64 {
                        Some(0)
                    } else {
                        Some(lhs >> shift)
                    }
                }
                BinaryOp::Sub
                | BinaryOp::BitXor
                | BinaryOp::And
                | BinaryOp::Or
                | BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge => None,
            },
            Expr::String { .. }
            | Expr::Char { .. }
            | Expr::Call { .. }
            | Expr::QualifiedCall { .. }
            | Expr::FieldAccess { .. }
            | Expr::Index { .. }
            | Expr::ArrayLit { .. }
            | Expr::StructInit { .. }
            | Expr::EnumVariant { .. }
            | Expr::EnumTupleVariant { .. }
            | Expr::EnumStructVariant { .. }
            | Expr::Match { .. }
            | Expr::DictLit { .. }
            | Expr::MethodCall { .. }
            | Expr::Unary {
                op: UnaryOp::Neg | UnaryOp::Not | UnaryOp::Ref | UnaryOp::RefMut,
                ..
            } => None,
        }
    }

    pub(super) fn infer_scalar_unsigned_upper_bound(
        &self,
        scalar_ty: RuntimeScalarType,
        expr: &Expr,
    ) -> Option<u64> {
        let RuntimeScalarType::Int(int_ty) = scalar_ty else {
            return None;
        };
        if int_ty.signed {
            return None;
        }
        self.expr_unsigned_upper_bound(expr, 8)
            .map(|bound| bound.min(max_unsigned_for_bits(int_ty.bits)))
    }

    pub(super) fn apply_scalar_unsigned_upper_bound(
        &mut self,
        slot: usize,
        scalar_ty: RuntimeScalarType,
        bound: Option<u64>,
    ) {
        let RuntimeScalarType::Int(int_ty) = scalar_ty else {
            self.slot_unsigned_upper_bounds.remove(&slot);
            return;
        };
        if int_ty.signed {
            self.slot_unsigned_upper_bounds.remove(&slot);
            return;
        }
        if let Some(bound) = bound {
            self.slot_unsigned_upper_bounds
                .insert(slot, bound.min(max_unsigned_for_bits(int_ty.bits)));
        } else {
            self.slot_unsigned_upper_bounds.remove(&slot);
        }
    }

    pub(super) fn index_expr_proven_in_bounds(
        &self,
        index: &Expr,
        idx_ty: RuntimeIntType,
        len: usize,
    ) -> bool {
        if idx_ty.signed {
            return false;
        }
        let Ok(len_u64) = u64::try_from(len) else {
            return false;
        };
        self.expr_unsigned_upper_bound(index, 8)
            .map(|bound| bound < len_u64)
            .unwrap_or(false)
    }

    pub(super) fn lower_function(
        &mut self,
        name: &str,
        is_main: bool,
    ) -> RuntimeGenericLowerResult<()> {
        if self.lowered_functions.contains(name) {
            return Ok(());
        }

        let function = self.functions.get(name).cloned().ok_or_else(|| {
            RuntimeGenericLowerError::Diagnostic(unknown_function_diagnostic(name, Span::new(0, 0)))
        })?;

        self.function_entries
            .insert(name.to_string(), self.instrs.len());
        self.lowered_functions.insert(name.to_string());
        self.queued_functions.remove(name);

        let saved_scopes = std::mem::replace(&mut self.scopes, RuntimeGenericScopeStack::new());
        let saved_active = self.active_function.replace(name.to_string());
        let param_layout = self.function_param_layout(name)?;
        let _ = self.function_return_layout(name)?;
        for param in param_layout {
            match param {
                RuntimeFunctionParamLayout::Scalar { name, ty, slot } => {
                    self.scopes.insert(
                        name,
                        RuntimeGenericBinding::Scalar {
                            slot,
                            mutable: false,
                            ty,
                        },
                    );
                }
                RuntimeFunctionParamLayout::OwnedFile { name, fd_slot } => {
                    self.scopes
                        .insert(name, RuntimeGenericBinding::OwnedFile { fd_slot });
                }
                RuntimeFunctionParamLayout::BorrowedFile { name, fd_slot } => {
                    self.scopes
                        .insert(name, RuntimeGenericBinding::BorrowedFile { fd_slot });
                }
                RuntimeFunctionParamLayout::OwnedSender { name, handle_slot } => {
                    self.scopes
                        .insert(name, RuntimeGenericBinding::OwnedSender { handle_slot });
                }
                RuntimeFunctionParamLayout::OwnedReceiver { name, handle_slot } => {
                    self.scopes
                        .insert(name, RuntimeGenericBinding::OwnedReceiver { handle_slot });
                }
                RuntimeFunctionParamLayout::Struct {
                    name,
                    layout,
                    fields,
                    mutable,
                    ..
                } => {
                    self.scopes.insert(
                        name,
                        RuntimeGenericBinding::StructSlots {
                            struct_name: layout.struct_name,
                            fields,
                            mutable,
                        },
                    );
                }
                RuntimeFunctionParamLayout::OwnedStruct { name, binding } => {
                    let RuntimeGenericBinding::OwnedStruct {
                        struct_name,
                        scalar_fields,
                        list_fields,
                        owns_cleanup,
                        ..
                    } = binding
                    else {
                        return Err(RuntimeGenericLowerError::Unsupported);
                    };
                    self.scopes.insert(
                        name,
                        RuntimeGenericBinding::OwnedStruct {
                            struct_name,
                            scalar_fields,
                            list_fields,
                            mutable: false,
                            owns_cleanup,
                        },
                    );
                }
                RuntimeFunctionParamLayout::BorrowedOwnedStruct {
                    name,
                    binding,
                    mutable,
                } => {
                    let RuntimeGenericBinding::OwnedStruct {
                        struct_name,
                        scalar_fields,
                        list_fields,
                        owns_cleanup: false,
                        ..
                    } = binding
                    else {
                        return Err(RuntimeGenericLowerError::Unsupported);
                    };
                    self.scopes.insert(
                        name,
                        RuntimeGenericBinding::OwnedStruct {
                            struct_name,
                            scalar_fields,
                            list_fields,
                            mutable,
                            owns_cleanup: false,
                        },
                    );
                }
                RuntimeFunctionParamLayout::OwnedListScalar {
                    name,
                    ptr_slot,
                    len_slot,
                    capacity_slot,
                    allocation_bytes_slot,
                    elem_ty,
                } => {
                    self.scopes.insert(
                        name,
                        RuntimeGenericBinding::OwnedListScalar {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            mutable: false,
                            elem_ty,
                        },
                    );
                }
                RuntimeFunctionParamLayout::OwnedListStruct {
                    name,
                    ptr_slot,
                    len_slot,
                    capacity_slot,
                    allocation_bytes_slot,
                    layout,
                } => {
                    self.scopes.insert(
                        name,
                        RuntimeGenericBinding::OwnedListStruct {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            mutable: false,
                            layout,
                        },
                    );
                }
                RuntimeFunctionParamLayout::OwnedString {
                    name,
                    ptr_slot,
                    len_slot,
                    capacity_slot,
                    allocation_bytes_slot,
                } => {
                    self.scopes.insert(
                        name,
                        RuntimeGenericBinding::OwnedString {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            mutable: false,
                            is_path: false,
                        },
                    );
                }
                RuntimeFunctionParamLayout::OwnedMap {
                    name,
                    ptr_slot,
                    len_slot,
                    capacity_slot,
                    allocation_bytes_slot,
                    layout,
                } => {
                    self.scopes.insert(
                        name,
                        RuntimeGenericBinding::OwnedMap {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            mutable: false,
                            layout,
                        },
                    );
                }
                RuntimeFunctionParamLayout::Enum {
                    name,
                    layout,
                    tag_slot,
                    payload_slots,
                } => {
                    self.scopes.insert(
                        name,
                        RuntimeGenericBinding::EnumSlots {
                            layout,
                            tag_slot,
                            payload_slots,
                            mutable: false,
                            owns_cleanup: true,
                        },
                    );
                }
            }
        }
        let lower_result = self.lower_stmts(&function.body);
        lower_result?;
        let guaranteed_terminal = runtime_stmts_guarantee_terminal(&function.body);

        if is_main {
            if !guaranteed_terminal {
                self.emit_terminal_cleanup_all();
                self.emit(RuntimeInstr::Exit {
                    code: RuntimeOperand::Imm(0),
                });
            }
        } else if !guaranteed_terminal {
            self.emit_owned_cleanup_all();
            self.emit(RuntimeInstr::Return);
        }
        self.active_function = saved_active;
        self.scopes = saved_scopes;
        Ok(())
    }

    pub(super) fn lower_queued_functions(&mut self) -> RuntimeGenericLowerResult<()> {
        while let Some(name) = self.pending_functions.pop_front() {
            self.lower_function(&name, false)?;
        }
        Ok(())
    }

    pub(super) fn patch_calls(&mut self) -> RuntimeGenericLowerResult<()> {
        let patches = self.call_patches.clone();
        for patch in patches {
            let target = self
                .function_entries
                .get(&patch.callee)
                .copied()
                .ok_or_else(|| {
                    RuntimeGenericLowerError::Diagnostic(unknown_function_diagnostic(
                        &patch.callee,
                        patch.span,
                    ))
                })?;
            self.patch_target(patch.instr_index, target)?;
        }
        Ok(())
    }

    pub(super) fn lower_stmts(&mut self, stmts: &[Stmt]) -> RuntimeGenericLowerResult<()> {
        for stmt in stmts {
            self.lower_stmt(stmt)?;
        }
        Ok(())
    }

    pub(super) fn try_lower_branchless_bool_clear(
        &mut self,
        cond: &Expr,
        then_branch: &[Stmt],
        else_branch: &Option<Vec<Stmt>>,
    ) -> RuntimeGenericLowerResult<bool> {
        if else_branch.is_some() || then_branch.len() != 1 {
            return Ok(false);
        }
        let Stmt::Assign {
            name,
            expr,
            span: _,
        } = &then_branch[0]
        else {
            return Ok(false);
        };
        let Some((slot, mutable, scalar_ty)) = self
            .scopes
            .get(name)
            .and_then(|binding| binding.as_scalar())
        else {
            return Ok(false);
        };
        if !mutable {
            return Ok(false);
        }
        let RuntimeScalarType::Int(int_ty) = scalar_ty else {
            return Ok(false);
        };
        let is_zero = runtime_const_int_from_expr(expr, Some(int_ty))
            .map(|v| v.encoded == 0)
            .unwrap_or(false);
        if !is_zero {
            return Ok(false);
        }

        let cond_slot = self.lower_cond(cond)?;
        let inv_cond = self.alloc_slot();
        self.emit(RuntimeInstr::BinOp {
            dst: inv_cond,
            op: RuntimeBinOp::BitXor,
            lhs: RuntimeOperand::Slot(cond_slot),
            rhs: RuntimeOperand::Imm(1),
        });
        self.emit(RuntimeInstr::BinOpInPlace {
            dst: slot,
            op: RuntimeBinOp::BitAnd,
            rhs: RuntimeOperand::Slot(inv_cond),
        });
        self.normalize_slot(slot, int_ty);
        self.slot_unsigned_upper_bounds.insert(slot, 1);
        Ok(true)
    }

    pub(super) fn lower_stmt(&mut self, stmt: &Stmt) -> RuntimeGenericLowerResult<()> {
        match stmt {
            Stmt::Let {
                name,
                mutable,
                ty,
                expr,
                span,
            } => {
                if self.scopes.current_contains(name) {
                    return Err(RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                        format!("redefinition of '{name}'"),
                        span,
                    )));
                }
                if let Expr::Call {
                    name: intrinsic,
                    args,
                    ..
                } = expr
                    && matches!(
                        intrinsic.as_str(),
                        "Channel::bounded"
                            | "Channel__bounded"
                            | "Channel::unbounded"
                            | "Channel__unbounded"
                    )
                {
                    let unbounded = matches!(
                        intrinsic.as_str(),
                        "Channel::unbounded" | "Channel__unbounded"
                    );
                    let valid_ty = matches!(
                        ty,
                        Some(TypeName::Applied { name, args })
                            if name == "Channel"
                                && matches!(args.as_slice(), [TypeName::Int { signed: false, bits: 64 }])
                    );
                    if *mutable || !valid_ty || args.len() != usize::from(!unbounded) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Channel<u64> construction requires bounded(capacity) or unbounded() and an immutable owner",
                            *span,
                        )));
                    }
                    let capacity = if unbounded {
                        RuntimeOperand::Imm(0)
                    } else {
                        self.lower_expr_as_type(&args[0], RuntimeIntType::new(false, 64)?)?
                    };
                    let handle_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::ChannelCreate {
                        dst: handle_slot,
                        capacity,
                        unbounded,
                    });
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::OwnedChannel {
                            handle_slot,
                            sender_taken: false,
                            receiver_taken: false,
                        },
                    );
                    return Ok(());
                }
                if let Expr::MethodCall {
                    receiver,
                    name: method,
                    args,
                    ..
                } = expr
                    && matches!(method.as_str(), "sender" | "receiver")
                    && let Expr::Ident {
                        name: channel_name,
                        span: channel_span,
                    } = receiver.as_ref()
                {
                    if !args.is_empty() || *mutable {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "channel endpoint extraction takes no arguments and returns an immutable owner",
                            *span,
                        )));
                    }
                    let expected = if method == "sender" {
                        "Sender"
                    } else {
                        "Receiver"
                    };
                    if !matches!(
                        ty,
                        Some(TypeName::Applied { name, args })
                            if name == expected
                                && matches!(args.as_slice(), [TypeName::Int { signed: false, bits: 64 }])
                    ) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "channel endpoints must retain the Channel<u64> element type",
                            *span,
                        )));
                    }
                    let Some(RuntimeGenericBinding::OwnedChannel {
                        handle_slot,
                        mut sender_taken,
                        mut receiver_taken,
                    }) = self.scopes.take_current(channel_name)
                    else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "endpoint extraction requires a current-scope Channel owner",
                            *channel_span,
                        )));
                    };
                    let already_taken = if method == "sender" {
                        let old = sender_taken;
                        sender_taken = true;
                        old
                    } else {
                        let old = receiver_taken;
                        receiver_taken = true;
                        old
                    };
                    if already_taken {
                        self.scopes.insert(
                            channel_name.clone(),
                            RuntimeGenericBinding::OwnedChannel {
                                handle_slot,
                                sender_taken,
                                receiver_taken,
                            },
                        );
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "each channel endpoint can be extracted exactly once",
                            *span,
                        )));
                    }
                    self.scopes.insert(
                        channel_name.clone(),
                        RuntimeGenericBinding::OwnedChannel {
                            handle_slot,
                            sender_taken,
                            receiver_taken,
                        },
                    );
                    self.scopes.insert(
                        name.clone(),
                        if method == "sender" {
                            RuntimeGenericBinding::OwnedSender { handle_slot }
                        } else {
                            RuntimeGenericBinding::OwnedReceiver { handle_slot }
                        },
                    );
                    return Ok(());
                }
                if let Expr::MethodCall {
                    receiver,
                    name: method,
                    args,
                    ..
                } = expr
                    && method == "recv"
                    && let Expr::Ident {
                        name: receiver_name,
                        span: receiver_span,
                    } = receiver.as_ref()
                {
                    if !args.is_empty()
                        || *mutable
                        || !matches!(
                            ty,
                            Some(TypeName::Int {
                                signed: false,
                                bits: 64
                            })
                        )
                    {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Receiver<u64>.recv() takes no arguments and returns u64",
                            *span,
                        )));
                    }
                    self.reject_moved_resource(receiver_name, *receiver_span)?;
                    let handle_slot = self
                        .scopes
                        .get(receiver_name)
                        .and_then(|binding| match binding {
                            RuntimeGenericBinding::OwnedReceiver { handle_slot } => {
                                Some(*handle_slot)
                            }
                            _ => None,
                        })
                        .ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                "recv() requires a live Receiver<u64> owner",
                                *receiver_span,
                            ))
                        })?;
                    let dst = self.alloc_slot();
                    self.emit(RuntimeInstr::ChannelRecv {
                        dst,
                        handle: RuntimeOperand::Slot(handle_slot),
                    });
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::Scalar {
                            slot: dst,
                            mutable: false,
                            ty: RuntimeScalarType::Int(RuntimeIntType {
                                signed: false,
                                bits: 64,
                            }),
                        },
                    );
                    return Ok(());
                }
                if let Expr::Call {
                    name: intrinsic,
                    args,
                    ..
                } = expr
                    && matches!(intrinsic.as_str(), "Thread__spawn" | "Thread::spawn")
                {
                    if *mutable || !matches!(ty, Some(TypeName::Thread)) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Thread::spawn() requires an immutable opaque Thread binding",
                            *span,
                        )));
                    }
                    let Some(Expr::Ident {
                        name: worker,
                        span: worker_span,
                    }) = args.first()
                    else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Thread::spawn() requires a statically named worker function",
                            *span,
                        )));
                    };
                    let params = self.function_param_layout(worker)?;
                    if params.len() != args.len().saturating_sub(1)
                        || !params.iter().all(|param| {
                            matches!(
                                param,
                                RuntimeFunctionParamLayout::Scalar { .. }
                                    | RuntimeFunctionParamLayout::OwnedSender { .. }
                                    | RuntimeFunctionParamLayout::OwnedReceiver { .. }
                            )
                        })
                    {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "native workers transfer scalar values and linear channel endpoints",
                            *span,
                        )));
                    }
                    let return_slot = match self.function_return_layout(worker)? {
                        Some(RuntimeFunctionReturnLayout::Scalar { ty, slot })
                            if ty
                                == RuntimeScalarType::Int(RuntimeIntType {
                                    signed: false,
                                    bits: 64,
                                }) =>
                        {
                            Some(slot)
                        }
                        None => None,
                        _ => {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "native worker functions must return u64 or have no return value",
                                *worker_span,
                            )));
                        }
                    };
                    self.emit_call_arguments(&args[1..], &params)?;
                    let handle_slot = self.alloc_slot();
                    let instr_index = self.emit(RuntimeInstr::ThreadSpawn {
                        handle_dst: handle_slot,
                        target: usize::MAX,
                        return_slot,
                    });
                    self.call_patches.push(RuntimeCallPatch {
                        instr_index,
                        callee: worker.clone(),
                        span: *worker_span,
                    });
                    self.queue_function(worker);
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::OwnedThread { handle_slot },
                    );
                    return Ok(());
                }
                if let Expr::MethodCall {
                    receiver,
                    name: method,
                    args,
                    ..
                } = expr
                    && method == "join"
                    && let Expr::Ident {
                        name: owner_name,
                        span: owner_span,
                    } = receiver.as_ref()
                    && matches!(
                        self.scopes.get_current(owner_name),
                        Some(RuntimeGenericBinding::OwnedThread { .. })
                    )
                {
                    if !args.is_empty()
                        || *mutable
                        || !matches!(
                            ty,
                            Some(TypeName::Int {
                                signed: false,
                                bits: 64
                            })
                        )
                    {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Thread.join() consumes its owner and returns u64",
                            *span,
                        )));
                    }
                    self.reject_moved_resource(owner_name, *owner_span)?;
                    let Some(RuntimeGenericBinding::OwnedThread { handle_slot }) =
                        self.scopes.take_current(owner_name)
                    else {
                        return Err(RuntimeGenericLowerError::Unsupported);
                    };
                    let dst = self.alloc_slot();
                    self.emit(RuntimeInstr::ThreadJoin {
                        dst,
                        handle: RuntimeOperand::Slot(handle_slot),
                    });
                    self.scopes.insert(
                        owner_name.clone(),
                        RuntimeGenericBinding::MovedResource {
                            kind: "Thread owner",
                        },
                    );
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::Scalar {
                            slot: dst,
                            mutable: false,
                            ty: RuntimeScalarType::Int(RuntimeIntType {
                                signed: false,
                                bits: 64,
                            }),
                        },
                    );
                    return Ok(());
                }
                let normalized_file_method = match expr {
                    Expr::MethodCall {
                        receiver,
                        name: method,
                        args,
                        span,
                    } if matches!(method.as_str(), "write_all" | "read" | "join") => {
                        let mut call_args = Vec::with_capacity(args.len() + 1);
                        call_args.push(receiver.as_ref().clone());
                        call_args.extend(args.iter().cloned());
                        Some(Expr::Call {
                            name: match method.as_str() {
                                "write_all" => "file_write_all".to_string(),
                                "read" => "file_read".to_string(),
                                _ => "Path__join".to_string(),
                            },
                            args: call_args,
                            span: *span,
                        })
                    }
                    _ => None,
                };
                let expr = normalized_file_method.as_ref().unwrap_or(expr);
                if let Expr::Ident {
                    name: source_name, ..
                } = expr
                    && let Some(RuntimeGenericBinding::OwnedFile { fd_slot }) =
                        self.scopes.get_current(source_name).cloned()
                {
                    if *mutable {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "moved file ownership bindings cannot be mutable or reassigned",
                            *span,
                        )));
                    }
                    if let Some(ty) = ty
                        && !matches!(ty, TypeName::File)
                    {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "file ownership bindings require the opaque File type",
                            *span,
                        )));
                    }
                    let moved = self.scopes.take_current(source_name).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "file ownership source disappeared during move",
                            *span,
                        ))
                    })?;
                    debug_assert!(matches!(moved, RuntimeGenericBinding::OwnedFile { .. }));
                    self.scopes.insert(
                        source_name.clone(),
                        RuntimeGenericBinding::MovedResource { kind: "file owner" },
                    );
                    self.scopes
                        .insert(name.clone(), RuntimeGenericBinding::OwnedFile { fd_slot });
                    return Ok(());
                }
                if let Expr::Ident {
                    name: source_name, ..
                } = expr
                    && let Some(RuntimeGenericBinding::OwnedPtr {
                        ptr_slot,
                        size_slot,
                    }) = self.scopes.get_current(source_name).cloned()
                {
                    if *mutable {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "moved heap ownership bindings cannot be mutable or reassigned",
                            *span,
                        )));
                    }
                    if let Some(ty) = ty
                        && !matches!(
                            ty,
                            TypeName::Int {
                                signed: false,
                                bits: 64
                            }
                        )
                    {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "moved heap ownership bindings currently require type u64",
                            *span,
                        )));
                    }
                    let moved = self.scopes.take_current(source_name).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "heap ownership source disappeared during move",
                            *span,
                        ))
                    })?;
                    debug_assert!(matches!(moved, RuntimeGenericBinding::OwnedPtr { .. }));
                    self.scopes.insert(
                        source_name.clone(),
                        RuntimeGenericBinding::MovedResource { kind: "heap owner" },
                    );
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::OwnedPtr {
                            ptr_slot,
                            size_slot,
                        },
                    );
                    return Ok(());
                }
                if let Expr::Ident {
                    name: source_name, ..
                } = expr
                    && let Some(source) = self.scopes.get_current(source_name).cloned()
                {
                    let moved = match source {
                        RuntimeGenericBinding::OwnedListScalar {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            elem_ty,
                            ..
                        } => {
                            if let Some(TypeName::List { elem }) = ty {
                                let expected = ensure_runtime_generic_scalar_type(elem, *span)
                                    .map_err(RuntimeGenericLowerError::Diagnostic)?;
                                if expected != elem_ty {
                                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                        "moved list binding type does not match its owner",
                                        *span,
                                    )));
                                }
                            }
                            Some(RuntimeGenericBinding::OwnedListScalar {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                mutable: *mutable,
                                elem_ty,
                            })
                        }
                        RuntimeGenericBinding::OwnedListStruct {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            layout,
                            ..
                        } => {
                            if let Some(TypeName::List { elem }) = ty {
                                let TypeName::Struct(expected_name) = elem.as_ref() else {
                                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                        "moved struct-list binding requires a matching list<Struct> type",
                                        *span,
                                    )));
                                };
                                if expected_name != &layout.struct_name {
                                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                        "moved struct-list binding type does not match its owner",
                                        *span,
                                    )));
                                }
                            }
                            Some(RuntimeGenericBinding::OwnedListStruct {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                mutable: *mutable,
                                layout,
                            })
                        }
                        RuntimeGenericBinding::OwnedStruct {
                            struct_name,
                            scalar_fields,
                            list_fields,
                            owns_cleanup: true,
                            ..
                        } => {
                            if let Some(TypeName::Struct(expected_name)) = ty
                                && expected_name != &struct_name
                            {
                                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                    "moved resource-owning struct binding type does not match its owner",
                                    *span,
                                )));
                            }
                            Some(RuntimeGenericBinding::OwnedStruct {
                                struct_name,
                                scalar_fields,
                                list_fields,
                                mutable: *mutable,
                                owns_cleanup: true,
                            })
                        }
                        RuntimeGenericBinding::OwnedString {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            is_path,
                            ..
                        } => {
                            let expected = if is_path {
                                TypeName::Path
                            } else {
                                TypeName::String
                            };
                            if ty.as_ref() != Some(&expected) {
                                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                    "moved owned text binding requires its matching type",
                                    *span,
                                )));
                            }
                            Some(RuntimeGenericBinding::OwnedString {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                mutable: *mutable,
                                is_path,
                            })
                        }
                        RuntimeGenericBinding::OwnedMap {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            layout,
                            ..
                        } => {
                            let Some(TypeName::Map { key, value }) = ty else {
                                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                    "moved map binding requires a map type",
                                    *span,
                                )));
                            };
                            if self.runtime_map_layout(key, value, *span)? != layout {
                                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                    "moved map binding type does not match its owner",
                                    *span,
                                )));
                            }
                            Some(RuntimeGenericBinding::OwnedMap {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                mutable: *mutable,
                                layout,
                            })
                        }
                        _ => None,
                    };
                    if let Some(moved) = moved {
                        let consumed = self.scopes.take_current(source_name).ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                "list ownership source disappeared during move",
                                *span,
                            ))
                        })?;
                        debug_assert!(matches!(
                            consumed,
                            RuntimeGenericBinding::OwnedListScalar { .. }
                                | RuntimeGenericBinding::OwnedListStruct { .. }
                                | RuntimeGenericBinding::OwnedStruct { .. }
                                | RuntimeGenericBinding::OwnedString { .. }
                                | RuntimeGenericBinding::OwnedMap { .. }
                        ));
                        self.scopes.insert(
                            source_name.clone(),
                            RuntimeGenericBinding::MovedResource {
                                kind: "resource owner",
                            },
                        );
                        self.scopes.insert(name.clone(), moved);
                        return Ok(());
                    }
                }
                if let Some(expected_ty) = ty
                    && let Expr::MethodCall {
                        receiver,
                        name: method,
                        args,
                        ..
                    } = expr
                    && args.is_empty()
                    && let Some(binding) = self.lower_native_integer_parse(
                        receiver,
                        method,
                        expected_ty,
                        *mutable,
                        *span,
                    )?
                {
                    self.scopes.insert(name.clone(), binding);
                    return Ok(());
                }
                if let Some(expected_ty) = ty
                    && let Expr::MethodCall {
                        receiver,
                        name: method,
                        args,
                        ..
                    } = expr
                    && args.is_empty()
                    && let Some(binding) = self.lower_native_bool_parse(
                        receiver,
                        method,
                        expected_ty,
                        *mutable,
                        *span,
                    )?
                {
                    self.scopes.insert(name.clone(), binding);
                    return Ok(());
                }
                if let Some(expected_ty) = ty
                    && match expected_ty {
                        TypeName::Struct(enum_name)
                        | TypeName::Applied {
                            name: enum_name, ..
                        } => self.enums.contains_key(enum_name),
                        _ => false,
                    }
                    && !matches!(
                        expected_ty,
                        TypeName::Applied { name, args }
                            if name == "Option"
                                && matches!(args.as_slice(), [TypeName::Struct(_)])
                    )
                    && let Some(binding) =
                        self.lower_runtime_enum_constructor(expected_ty, expr, *mutable, *span)?
                {
                    self.scopes.insert(name.clone(), binding);
                    return Ok(());
                }
                if let Expr::Call {
                    name: callee,
                    args,
                    span: call_span,
                } = expr
                    && self.functions.contains_key(callee)
                    && let Some(RuntimeFunctionReturnLayout::Enum {
                        layout,
                        tag_slot,
                        payload_slots,
                    }) = self.function_return_layout(callee)?
                {
                    if let Some(expected_ty) = ty {
                        let expected_layout = self.runtime_enum_layout(expected_ty, *call_span)?;
                        if expected_layout != layout {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "enum-returning function does not match binding type",
                                *call_span,
                            )));
                        }
                    }
                    let param_layout = self.function_param_layout(callee)?;
                    if args.len() != param_layout.len() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "function argument count does not match enum-returning function",
                            *call_span,
                        )));
                    }
                    self.emit_call_arguments(args, &param_layout)?;
                    self.emit_call_target(callee, *call_span);
                    let binding =
                        self.capture_enum_return(layout, tag_slot, &payload_slots, *mutable);
                    self.scopes.insert(name.clone(), binding);
                    return Ok(());
                }
                let annotated_option_struct_layout = match ty.as_ref() {
                    Some(TypeName::Applied {
                        name: option_name,
                        args,
                    }) if option_name == "Option" => match args.as_slice() {
                        [TypeName::Struct(struct_name)] => {
                            Some(self.runtime_struct_list_layout(struct_name, *span)?)
                        }
                        _ => None,
                    },
                    _ => None,
                };
                let inferred_option_struct_layout = if ty.is_none() {
                    self.native_option_struct_layout(expr)
                } else {
                    None
                };
                if let Some(layout) =
                    annotated_option_struct_layout.or(inferred_option_struct_layout)
                {
                    let (tag, values, result_layout) =
                        self.lower_expr_as_option_struct(expr, Some(&layout))?;
                    let tag_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: tag_slot,
                        src: tag,
                    });
                    let mut fields = HashMap::with_capacity(result_layout.fields.len());
                    for (field, value) in values {
                        let slot = self.alloc_slot();
                        self.emit(RuntimeInstr::Mov {
                            dst: slot,
                            src: value,
                        });
                        self.normalize_scalar_slot(slot, field.ty);
                        fields.insert(field.name, (slot, field.ty));
                    }
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::OptionStruct {
                            tag_slot,
                            fields,
                            mutable: *mutable,
                            layout: result_layout,
                        },
                    );
                    return Ok(());
                }
                let annotated_option_scalar_ty = match ty.as_ref() {
                    Some(TypeName::Applied { name, args }) if name == "Option" => {
                        match args.as_slice() {
                            [elem] => ensure_runtime_generic_scalar_type(elem, *span).ok(),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                let inferred_option_scalar_ty = if ty.is_none()
                    && !matches!(
                        expr,
                        Expr::EnumVariant {
                            enum_name,
                            variant,
                            ..
                        } if enum_name == "Option" && variant == "None"
                    ) {
                    self.native_option_scalar_type(expr)
                } else {
                    None
                };
                if let Some(expected_elem_ty) =
                    annotated_option_scalar_ty.or(inferred_option_scalar_ty)
                {
                    let (tag, payload, elem_ty) =
                        self.lower_expr_as_option_scalar(expr, Some(expected_elem_ty))?;
                    let tag_slot = self.alloc_slot();
                    let payload_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: tag_slot,
                        src: tag,
                    });
                    self.emit(RuntimeInstr::Mov {
                        dst: payload_slot,
                        src: payload,
                    });
                    self.normalize_scalar_slot(payload_slot, elem_ty);
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::OptionScalar {
                            tag_slot,
                            payload_slot,
                            mutable: *mutable,
                            elem_ty,
                        },
                    );
                    return Ok(());
                }
                if let (
                    Some(TypeName::Struct(struct_name)),
                    Expr::StructInit {
                        name: literal_name,
                        fields,
                        ..
                    },
                ) = (ty.as_ref(), expr)
                    && literal_name == struct_name
                    && self.struct_has_direct_owned_list(struct_name)
                {
                    let binding = self.lower_runtime_owned_struct_literal(
                        struct_name,
                        fields,
                        *mutable,
                        *span,
                    )?;
                    self.scopes.insert(name.clone(), binding);
                    return Ok(());
                }
                if let (Some(TypeName::String), Expr::String { value, .. }) = (ty.as_ref(), expr) {
                    let ptr_slot = self.alloc_slot();
                    let len_slot = self.alloc_slot();
                    let capacity_slot = self.alloc_slot();
                    let allocation_bytes_slot = self.alloc_slot();
                    for slot in [ptr_slot, len_slot, capacity_slot, allocation_bytes_slot] {
                        self.emit(RuntimeInstr::Mov {
                            dst: slot,
                            src: RuntimeOperand::Imm(0),
                        });
                    }
                    let byte_ty = RuntimeScalarType::Int(RuntimeIntType::new(false, 8)?);
                    for byte in value.as_bytes() {
                        self.emit_owned_list_scalar_push(
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            RuntimeOperand::Imm(u64::from(*byte)),
                            byte_ty,
                        )?;
                    }
                    self.emit_owned_list_scalar_push(
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        RuntimeOperand::Imm(0),
                        byte_ty,
                    )?;
                    self.emit(RuntimeInstr::BinOpInPlace {
                        dst: len_slot,
                        op: RuntimeBinOp::Sub,
                        rhs: RuntimeOperand::Imm(1),
                    });
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::OwnedString {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            mutable: *mutable,
                            is_path: false,
                        },
                    );
                    return Ok(());
                }
                if let (Some(TypeName::Map { key, value }), Expr::DictLit { entries, .. }) =
                    (ty.as_ref(), expr)
                {
                    if !entries.is_empty() {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime-native scalar maps must be initialized empty and populated with set()",
                            *span,
                        )));
                    }
                    let layout = self.runtime_map_layout(key, value, *span)?;
                    let ptr_slot = self.alloc_slot();
                    let len_slot = self.alloc_slot();
                    let capacity_slot = self.alloc_slot();
                    let allocation_bytes_slot = self.alloc_slot();
                    for slot in [ptr_slot, len_slot, capacity_slot, allocation_bytes_slot] {
                        self.emit(RuntimeInstr::Mov {
                            dst: slot,
                            src: RuntimeOperand::Imm(0),
                        });
                    }
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::OwnedMap {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            mutable: *mutable,
                            layout,
                        },
                    );
                    return Ok(());
                }
                if let (
                    Some(TypeName::Struct(struct_name)),
                    Expr::Ident {
                        name: source_name, ..
                    },
                ) = (ty.as_ref(), expr)
                    && matches!(
                        self.scopes.get(source_name),
                        Some(RuntimeGenericBinding::StructSlots { .. })
                    )
                {
                    let layout = self.runtime_struct_list_layout(struct_name, *span)?;
                    let mut fields = HashMap::with_capacity(layout.fields.len());
                    for field in &layout.fields {
                        fields.insert(field.name.clone(), (self.alloc_slot(), field.ty));
                    }
                    self.lower_struct_slot_assign(struct_name, &fields, expr, *span)?;
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::StructSlots {
                            struct_name: struct_name.clone(),
                            fields,
                            mutable: *mutable,
                        },
                    );
                    return Ok(());
                }
                if let (Some(TypeName::List { elem }), Expr::ArrayLit { elems, .. }) =
                    (ty.as_ref(), expr)
                {
                    let ptr_slot = self.alloc_slot();
                    let len_slot = self.alloc_slot();
                    let capacity_slot = self.alloc_slot();
                    let allocation_bytes_slot = self.alloc_slot();
                    for slot in [ptr_slot, len_slot, capacity_slot, allocation_bytes_slot] {
                        self.emit(RuntimeInstr::Mov {
                            dst: slot,
                            src: RuntimeOperand::Imm(0),
                        });
                    }
                    if let TypeName::Struct(struct_name) = elem.as_ref() {
                        let layout = self.runtime_struct_list_layout(struct_name, *span)?;
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::OwnedListStruct {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                mutable: *mutable,
                                layout: layout.clone(),
                            },
                        );
                        for elem in elems {
                            let values =
                                self.lower_runtime_struct_operands(elem, &layout, *span)?;
                            self.emit_owned_list_struct_push(
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                &layout,
                                values,
                            )?;
                        }
                        return Ok(());
                    }
                    let elem_ty = ensure_runtime_generic_scalar_type(elem, *span)
                        .map_err(RuntimeGenericLowerError::Diagnostic)?;
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::OwnedListScalar {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            mutable: *mutable,
                            elem_ty,
                        },
                    );
                    for elem in elems {
                        let value = self.lower_expr_as_scalar(elem, elem_ty)?;
                        self.emit_owned_list_scalar_push(
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            value,
                            elem_ty,
                        )?;
                    }
                    return Ok(());
                }
                if let Expr::Call {
                    name: callee,
                    args,
                    span: call_span,
                } = expr
                    && self.functions.contains_key(callee)
                {
                    let return_layout = self.function_return_layout(callee)?;
                    if let Some(RuntimeFunctionReturnLayout::OwnedFile { fd_slot }) =
                        return_layout.clone()
                    {
                        if let Some(expected_ty) = ty
                            && !matches!(expected_ty, TypeName::File)
                        {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "File-returning function requires a File binding",
                                *call_span,
                            )));
                        }
                        if *mutable {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "File ownership bindings cannot be mutable or reassigned",
                                *span,
                            )));
                        }
                        let param_layout = self.function_param_layout(callee)?;
                        if args.len() != param_layout.len() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "function argument count does not match File-returning function",
                                *call_span,
                            )));
                        }
                        self.emit_call_arguments(args, &param_layout)?;
                        self.emit_call_target(callee, *call_span);
                        self.scopes
                            .insert(name.clone(), RuntimeGenericBinding::OwnedFile { fd_slot });
                        return Ok(());
                    }
                    if let Some(RuntimeFunctionReturnLayout::Enum {
                        layout,
                        tag_slot,
                        payload_slots,
                    }) = return_layout.clone()
                    {
                        if let Some(expected_ty) = ty {
                            let expected_layout =
                                self.runtime_enum_layout(expected_ty, *call_span)?;
                            if expected_layout != layout {
                                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                    "enum-returning function does not match binding type",
                                    *call_span,
                                )));
                            }
                        }
                        let param_layout = self.function_param_layout(callee)?;
                        if args.len() != param_layout.len() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "function argument count does not match enum-returning function",
                                *call_span,
                            )));
                        }
                        self.emit_call_arguments(args, &param_layout)?;
                        self.emit_call_target(callee, *call_span);
                        let binding =
                            self.capture_enum_return(layout, tag_slot, &payload_slots, *mutable);
                        self.scopes.insert(name.clone(), binding);
                        return Ok(());
                    }
                    if let Some(RuntimeFunctionReturnLayout::OwnedStruct { binding }) =
                        return_layout.clone()
                    {
                        let RuntimeGenericBinding::OwnedStruct { struct_name, .. } = &binding
                        else {
                            return Err(RuntimeGenericLowerError::Unsupported);
                        };
                        if let Some(TypeName::Struct(expected_name)) = ty
                            && expected_name != struct_name
                        {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "resource-owning struct-returning function does not match binding type",
                                *call_span,
                            )));
                        }
                        let param_layout = self.function_param_layout(callee)?;
                        if args.len() != param_layout.len() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "function argument count does not match resource-owning struct-returning function",
                                *call_span,
                            )));
                        }
                        self.emit_call_arguments(args, &param_layout)?;
                        self.emit_call_target(callee, *call_span);
                        let RuntimeGenericBinding::OwnedStruct {
                            struct_name,
                            scalar_fields,
                            list_fields,
                            owns_cleanup,
                            ..
                        } = binding
                        else {
                            return Err(RuntimeGenericLowerError::Unsupported);
                        };
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::OwnedStruct {
                                struct_name,
                                scalar_fields,
                                list_fields,
                                mutable: *mutable,
                                owns_cleanup,
                            },
                        );
                        return Ok(());
                    }
                    if let Some(RuntimeFunctionReturnLayout::Struct { layout, fields }) =
                        return_layout.clone()
                    {
                        if let Some(TypeName::Struct(expected_name)) = ty
                            && expected_name != &layout.struct_name
                        {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "struct-returning function does not match binding type",
                                *call_span,
                            )));
                        }
                        let param_layout = self.function_param_layout(callee)?;
                        if args.len() != param_layout.len() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "function argument count does not match struct-returning function",
                                *call_span,
                            )));
                        }
                        self.emit_call_arguments(args, &param_layout)?;
                        self.emit_call_target(callee, *call_span);
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::StructSlots {
                                struct_name: layout.struct_name,
                                fields,
                                mutable: *mutable,
                            },
                        );
                        return Ok(());
                    }
                    if let Some(RuntimeFunctionReturnLayout::OwnedString {
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                    }) = return_layout.clone()
                    {
                        if !matches!(ty, Some(TypeName::String)) {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "owned text-returning function requires a matching binding type",
                                *call_span,
                            )));
                        }
                        let param_layout = self.function_param_layout(callee)?;
                        if args.len() != param_layout.len() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "function argument count does not match string-returning function",
                                *call_span,
                            )));
                        }
                        self.emit_call_arguments(args, &param_layout)?;
                        self.emit_call_target(callee, *call_span);
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::OwnedString {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                mutable: *mutable,
                                is_path: false,
                            },
                        );
                        return Ok(());
                    }
                    if let Some(RuntimeFunctionReturnLayout::OwnedMap {
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        layout,
                    }) = return_layout.clone()
                    {
                        let Some(TypeName::Map { key, value }) = ty else {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "map-returning function requires a map binding",
                                *call_span,
                            )));
                        };
                        if self.runtime_map_layout(key, value, *call_span)? != layout {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "map-returning function layout mismatch",
                                *call_span,
                            )));
                        }
                        let param_layout = self.function_param_layout(callee)?;
                        self.emit_call_arguments(args, &param_layout)?;
                        self.emit_call_target(callee, *call_span);
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::OwnedMap {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                mutable: *mutable,
                                layout,
                            },
                        );
                        return Ok(());
                    }
                    if let Some(RuntimeFunctionReturnLayout::OwnedListScalar {
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        elem_ty,
                    }) = return_layout
                    {
                        let param_layout = self.function_param_layout(callee)?;
                        if args.len() != param_layout.len() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "function argument count does not match list-returning function",
                                *call_span,
                            )));
                        }
                        self.emit_call_arguments(args, &param_layout)?;
                        self.emit_call_target(callee, *call_span);
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::OwnedListScalar {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                mutable: *mutable,
                                elem_ty,
                            },
                        );
                        return Ok(());
                    }
                    if let Some(RuntimeFunctionReturnLayout::OwnedListStruct {
                        ptr_slot,
                        len_slot,
                        capacity_slot,
                        allocation_bytes_slot,
                        layout,
                    }) = return_layout
                    {
                        let param_layout = self.function_param_layout(callee)?;
                        if args.len() != param_layout.len() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "function argument count does not match list-returning function",
                                *call_span,
                            )));
                        }
                        self.emit_call_arguments(args, &param_layout)?;
                        self.emit_call_target(callee, *call_span);
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::OwnedListStruct {
                                ptr_slot,
                                len_slot,
                                capacity_slot,
                                allocation_bytes_slot,
                                mutable: *mutable,
                                layout,
                            },
                        );
                        return Ok(());
                    }
                }
                if let Expr::Call {
                    name: alloc_name,
                    args,
                    ..
                } = expr
                    && alloc_name == "heap_alloc"
                {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "heap_alloc() expects exactly one argument",
                            *span,
                        )));
                    }
                    if *mutable {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "owned heap bindings cannot be mutable or reassigned",
                            *span,
                        )));
                    }
                    if let Some(ty) = ty
                        && !matches!(
                            ty,
                            TypeName::Int {
                                signed: false,
                                bits: 64
                            }
                        )
                    {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "heap_alloc() ownership bindings currently require type u64",
                            *span,
                        )));
                    }
                    let u64_ty = RuntimeIntType::new(false, 64)?;
                    let size = self.lower_expr_as_type(&args[0], u64_ty)?;
                    let size_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: size_slot,
                        src: size,
                    });
                    let ptr_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Alloc {
                        dst: ptr_slot,
                        size: RuntimeOperand::Slot(size_slot),
                    });
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::OwnedPtr {
                            ptr_slot,
                            size_slot,
                        },
                    );
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::Ne,
                        RuntimeOperand::Slot(ptr_slot),
                        RuntimeOperand::Imm(0),
                        101,
                    )?;
                    return Ok(());
                }
                if let Expr::Call {
                    name: intrinsic,
                    args,
                    ..
                } = expr
                    && matches!(
                        intrinsic.as_str(),
                        "runtime_argument" | "runtime_environment_entry"
                    )
                {
                    if args.len() != 1 || !matches!(ty, Some(TypeName::String)) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "indexed platform text access requires one u64 index and a string binding",
                            *span,
                        )));
                    }
                    let u64_ty = RuntimeIntType::new(false, 64)?;
                    let requested = self.lower_expr_as_type(&args[0], u64_ty)?;
                    let entry_ptr = self.emit_entry_stack_pointer();
                    let argc_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::HeapLoadInt {
                        dst: argc_slot,
                        ptr: RuntimeOperand::Slot(entry_ptr),
                        index: RuntimeOperand::Imm(0),
                        bytes: 8,
                    });

                    let array_index = if intrinsic == "runtime_argument" {
                        self.emit_guard_failure_exit(
                            RuntimeCmpOp::LtUnsigned,
                            requested,
                            RuntimeOperand::Slot(argc_slot),
                            106,
                        )?;
                        let slot = self.alloc_slot();
                        self.emit(RuntimeInstr::BinOp {
                            dst: slot,
                            op: RuntimeBinOp::Add,
                            lhs: requested,
                            rhs: RuntimeOperand::Imm(1),
                        });
                        slot
                    } else {
                        let base_slot = self.alloc_slot();
                        self.emit(RuntimeInstr::BinOp {
                            dst: base_slot,
                            op: RuntimeBinOp::Add,
                            lhs: RuntimeOperand::Slot(argc_slot),
                            rhs: RuntimeOperand::Imm(2),
                        });
                        let count_slot = self.alloc_slot();
                        self.emit(RuntimeInstr::Mov {
                            dst: count_slot,
                            src: RuntimeOperand::Imm(0),
                        });
                        let scan_index = self.alloc_slot();
                        let env_ptr = self.alloc_slot();
                        let scan = self.instrs.len();
                        self.emit(RuntimeInstr::BinOp {
                            dst: scan_index,
                            op: RuntimeBinOp::Add,
                            lhs: RuntimeOperand::Slot(base_slot),
                            rhs: RuntimeOperand::Slot(count_slot),
                        });
                        self.emit(RuntimeInstr::HeapLoadInt {
                            dst: env_ptr,
                            ptr: RuntimeOperand::Slot(entry_ptr),
                            index: RuntimeOperand::Slot(scan_index),
                            bytes: 8,
                        });
                        let done = self.emit(RuntimeInstr::JumpIfCmpFalse {
                            op: RuntimeCmpOp::Ne,
                            lhs: RuntimeOperand::Slot(env_ptr),
                            rhs: RuntimeOperand::Imm(0),
                            target: usize::MAX,
                        });
                        self.emit(RuntimeInstr::BinOpInPlace {
                            dst: count_slot,
                            op: RuntimeBinOp::Add,
                            rhs: RuntimeOperand::Imm(1),
                        });
                        self.emit(RuntimeInstr::Jump { target: scan });
                        let scanned = self.instrs.len();
                        self.patch_target(done, scanned)?;
                        self.emit_guard_failure_exit(
                            RuntimeCmpOp::LtUnsigned,
                            requested,
                            RuntimeOperand::Slot(count_slot),
                            106,
                        )?;
                        let slot = self.alloc_slot();
                        self.emit(RuntimeInstr::BinOp {
                            dst: slot,
                            op: RuntimeBinOp::Add,
                            lhs: RuntimeOperand::Slot(base_slot),
                            rhs: requested,
                        });
                        slot
                    };
                    let source_ptr = self.alloc_slot();
                    self.emit(RuntimeInstr::HeapLoadInt {
                        dst: source_ptr,
                        ptr: RuntimeOperand::Slot(entry_ptr),
                        index: RuntimeOperand::Slot(array_index),
                        bytes: 8,
                    });
                    self.bind_owned_c_string(name, source_ptr, *mutable)?;
                    return Ok(());
                }
                if let Expr::Call {
                    name: intrinsic,
                    args,
                    ..
                } = expr
                    && intrinsic == "Path__join"
                {
                    if args.len() != 2 || !matches!(ty, Some(TypeName::Path)) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Path::join() expects (Path, string) and a Path binding",
                            *span,
                        )));
                    }
                    if *mutable {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Path ownership bindings cannot be mutable or reassigned",
                            *span,
                        )));
                    }
                    let Expr::Ident {
                        name: base_name, ..
                    } = &args[0]
                    else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Path::join() requires a named current-scope Path owner",
                            args[0].span(),
                        )));
                    };
                    let Expr::Ident {
                        name: segment_name, ..
                    } = &args[1]
                    else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Path::join() requires a named owned string segment",
                            args[1].span(),
                        )));
                    };
                    let Some((base_ptr, base_len, _, base_bytes, _)) = self
                        .scopes
                        .get_current(base_name)
                        .and_then(RuntimeGenericBinding::as_owned_path)
                    else {
                        self.reject_moved_resource(base_name, args[0].span())?;
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Path::join() requires a live current-scope Path owner",
                            args[0].span(),
                        )));
                    };
                    let Some((segment_ptr, segment_len, _, _, _)) = self
                        .scopes
                        .get(segment_name)
                        .and_then(RuntimeGenericBinding::as_owned_string)
                    else {
                        self.reject_moved_resource(segment_name, args[1].span())?;
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Path::join() requires a live string segment",
                            args[1].span(),
                        )));
                    };
                    self.emit_validate_file_path(segment_ptr, segment_len)?;

                    let separator_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: separator_slot,
                        src: RuntimeOperand::Imm(0),
                    });
                    let empty_base = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Ne,
                        lhs: RuntimeOperand::Slot(base_len),
                        rhs: RuntimeOperand::Imm(0),
                        target: usize::MAX,
                    });
                    let last_index = self.alloc_slot();
                    self.emit(RuntimeInstr::BinOp {
                        dst: last_index,
                        op: RuntimeBinOp::Sub,
                        lhs: RuntimeOperand::Slot(base_len),
                        rhs: RuntimeOperand::Imm(1),
                    });
                    let last_byte = self.alloc_slot();
                    self.emit(RuntimeInstr::HeapLoadInt {
                        dst: last_byte,
                        ptr: RuntimeOperand::Slot(base_ptr),
                        index: RuntimeOperand::Slot(last_index),
                        bytes: 1,
                    });
                    let trailing_separator = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Ne,
                        lhs: RuntimeOperand::Slot(last_byte),
                        rhs: RuntimeOperand::Imm(u64::from(b'/')),
                        target: usize::MAX,
                    });
                    self.emit(RuntimeInstr::Mov {
                        dst: separator_slot,
                        src: RuntimeOperand::Imm(1),
                    });
                    let separator_decided = self.instrs.len();
                    self.patch_target(empty_base, separator_decided)?;
                    self.patch_target(trailing_separator, separator_decided)?;

                    let joined_len = self.alloc_slot();
                    self.emit(RuntimeInstr::BinOp {
                        dst: joined_len,
                        op: RuntimeBinOp::Add,
                        lhs: RuntimeOperand::Slot(base_len),
                        rhs: RuntimeOperand::Slot(segment_len),
                    });
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::GeUnsigned,
                        RuntimeOperand::Slot(joined_len),
                        RuntimeOperand::Slot(base_len),
                        101,
                    )?;
                    self.emit(RuntimeInstr::BinOpInPlace {
                        dst: joined_len,
                        op: RuntimeBinOp::Add,
                        rhs: RuntimeOperand::Slot(separator_slot),
                    });
                    let capacity_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::BinOp {
                        dst: capacity_slot,
                        op: RuntimeBinOp::Add,
                        lhs: RuntimeOperand::Slot(joined_len),
                        rhs: RuntimeOperand::Imm(1),
                    });
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::GtUnsigned,
                        RuntimeOperand::Slot(capacity_slot),
                        RuntimeOperand::Slot(joined_len),
                        101,
                    )?;
                    let joined_ptr = self.alloc_slot();
                    self.emit(RuntimeInstr::Alloc {
                        dst: joined_ptr,
                        size: RuntimeOperand::Slot(capacity_slot),
                    });
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::Ne,
                        RuntimeOperand::Slot(joined_ptr),
                        RuntimeOperand::Imm(0),
                        101,
                    )?;
                    self.emit(RuntimeInstr::HeapCopy {
                        dst_ptr: RuntimeOperand::Slot(joined_ptr),
                        src_ptr: RuntimeOperand::Slot(base_ptr),
                        bytes: RuntimeOperand::Slot(base_len),
                    });
                    self.emit(RuntimeInstr::HeapStoreInt {
                        ptr: RuntimeOperand::Slot(joined_ptr),
                        index: RuntimeOperand::Slot(base_len),
                        src: RuntimeOperand::Imm(u64::from(b'/')),
                        bytes: 1,
                    });
                    let segment_offset = self.alloc_slot();
                    self.emit(RuntimeInstr::BinOp {
                        dst: segment_offset,
                        op: RuntimeBinOp::Add,
                        lhs: RuntimeOperand::Slot(base_len),
                        rhs: RuntimeOperand::Slot(separator_slot),
                    });
                    let segment_dst = self.alloc_slot();
                    self.emit(RuntimeInstr::BinOp {
                        dst: segment_dst,
                        op: RuntimeBinOp::Add,
                        lhs: RuntimeOperand::Slot(joined_ptr),
                        rhs: RuntimeOperand::Slot(segment_offset),
                    });
                    self.emit(RuntimeInstr::HeapCopy {
                        dst_ptr: RuntimeOperand::Slot(segment_dst),
                        src_ptr: RuntimeOperand::Slot(segment_ptr),
                        bytes: RuntimeOperand::Slot(segment_len),
                    });
                    self.emit(RuntimeInstr::HeapStoreInt {
                        ptr: RuntimeOperand::Slot(joined_ptr),
                        index: RuntimeOperand::Slot(joined_len),
                        src: RuntimeOperand::Imm(0),
                        bytes: 1,
                    });
                    self.emit(RuntimeInstr::Free {
                        ptr: RuntimeOperand::Slot(base_ptr),
                        size: RuntimeOperand::Slot(base_bytes),
                    });
                    let _ = self.scopes.take_current(base_name);
                    self.scopes.insert(
                        base_name.clone(),
                        RuntimeGenericBinding::MovedResource { kind: "Path owner" },
                    );
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::OwnedString {
                            ptr_slot: joined_ptr,
                            len_slot: joined_len,
                            capacity_slot,
                            allocation_bytes_slot: capacity_slot,
                            mutable: false,
                            is_path: true,
                        },
                    );
                    return Ok(());
                }
                if let Expr::Call {
                    name: intrinsic,
                    args,
                    ..
                } = expr
                    && intrinsic == "Path__new"
                {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Path::new() expects exactly one owned string",
                            *span,
                        )));
                    }
                    if *mutable {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Path ownership bindings cannot be mutable or reassigned",
                            *span,
                        )));
                    }
                    if !matches!(ty, Some(TypeName::Path)) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Path::new() requires a Path binding",
                            *span,
                        )));
                    }
                    let Expr::Ident {
                        name: source_name, ..
                    } = &args[0]
                    else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Path::new() requires a named owned string",
                            args[0].span(),
                        )));
                    };
                    let Some((ptr_slot, len_slot, capacity_slot, allocation_bytes_slot, _)) = self
                        .scopes
                        .get_current(source_name)
                        .and_then(RuntimeGenericBinding::as_owned_string)
                    else {
                        self.reject_moved_resource(source_name, args[0].span())?;
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "Path::new() requires a current-scope owned string",
                            args[0].span(),
                        )));
                    };
                    self.emit_validate_file_path(ptr_slot, len_slot)?;
                    let _ = self.scopes.take_current(source_name);
                    self.scopes.insert(
                        source_name.clone(),
                        RuntimeGenericBinding::MovedResource {
                            kind: "string owner",
                        },
                    );
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::OwnedString {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            mutable: false,
                            is_path: true,
                        },
                    );
                    return Ok(());
                }
                if let Expr::Call {
                    name: intrinsic,
                    args,
                    ..
                } = expr
                    && matches!(
                        intrinsic.as_str(),
                        "file_open_read" | "file_create" | "File__open_read" | "File__create"
                    )
                {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "file open expects exactly one owned string path",
                            *span,
                        )));
                    }
                    if *mutable {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "file ownership bindings cannot be mutable or reassigned",
                            *span,
                        )));
                    }
                    if let Some(ty) = ty
                        && !matches!(ty, TypeName::File)
                    {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "file ownership bindings require the opaque File type",
                            *span,
                        )));
                    }
                    let Expr::Ident {
                        name: path_name, ..
                    } = &args[0]
                    else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "file path must be a named owned string",
                            args[0].span(),
                        )));
                    };
                    self.reject_moved_resource(path_name, args[0].span())?;
                    let (path_ptr, path_len) = self
                        .scopes
                        .get(path_name)
                        .and_then(RuntimeGenericBinding::as_owned_text)
                        .map(|binding| (binding.0, binding.1))
                        .ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                "file path must be an owned string",
                                args[0].span(),
                            ))
                        })?;
                    self.emit_validate_file_path(path_ptr, path_len)?;
                    let fd_slot = self.alloc_slot();
                    let (flags, mode) =
                        if matches!(intrinsic.as_str(), "file_create" | "File__create") {
                            (577, 0o644)
                        } else {
                            (0, 0)
                        };
                    self.emit(RuntimeInstr::FileOpen {
                        dst: fd_slot,
                        path_ptr: RuntimeOperand::Slot(path_ptr),
                        flags,
                        mode,
                    });
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::GeSigned,
                        RuntimeOperand::Slot(fd_slot),
                        RuntimeOperand::Imm(0),
                        102,
                    )?;
                    self.scopes
                        .insert(name.clone(), RuntimeGenericBinding::OwnedFile { fd_slot });
                    return Ok(());
                }
                if let Expr::Call {
                    name: intrinsic,
                    args,
                    ..
                } = expr
                    && intrinsic == "file_write_all"
                {
                    if args.len() != 2 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "file_write_all() expects exactly two arguments (file, text)",
                            *span,
                        )));
                    }
                    let Expr::Ident {
                        name: file_name, ..
                    } = &args[0]
                    else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "file_write_all() requires a named file owner",
                            args[0].span(),
                        )));
                    };
                    let Expr::Ident {
                        name: text_name, ..
                    } = &args[1]
                    else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "file_write_all() requires a named owned string",
                            args[1].span(),
                        )));
                    };
                    self.reject_moved_resource(file_name, args[0].span())?;
                    self.reject_moved_resource(text_name, args[1].span())?;
                    let fd_slot = self
                        .scopes
                        .get(file_name)
                        .and_then(RuntimeGenericBinding::as_owned_file)
                        .ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                "file_write_all() requires a live file owner",
                                args[0].span(),
                            ))
                        })?;
                    let (ptr_slot, len_slot, _, _, _) = self
                        .scopes
                        .get(text_name)
                        .and_then(RuntimeGenericBinding::as_owned_string)
                        .ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                "file_write_all() requires an owned string",
                                args[1].span(),
                            ))
                        })?;
                    let written_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::FileWrite {
                        dst: written_slot,
                        fd: RuntimeOperand::Slot(fd_slot),
                        ptr: RuntimeOperand::Slot(ptr_slot),
                        len: RuntimeOperand::Slot(len_slot),
                    });
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::Ne,
                        RuntimeOperand::Slot(written_slot),
                        RuntimeOperand::Imm(u64::MAX),
                        103,
                    )?;
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::Eq,
                        RuntimeOperand::Slot(written_slot),
                        RuntimeOperand::Slot(len_slot),
                        103,
                    )?;
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::Scalar {
                            slot: written_slot,
                            mutable: *mutable,
                            ty: RuntimeScalarType::Int(RuntimeIntType::new(false, 64)?),
                        },
                    );
                    return Ok(());
                }
                if let Expr::Call {
                    name: intrinsic,
                    args,
                    ..
                } = expr
                    && intrinsic == "file_read"
                {
                    if args.len() != 2 || !matches!(ty, Some(TypeName::String)) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "file_read() expects (file, max_bytes) and a string binding",
                            *span,
                        )));
                    }
                    let Expr::Ident {
                        name: file_name, ..
                    } = &args[0]
                    else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "file_read() requires a named file owner",
                            args[0].span(),
                        )));
                    };
                    self.reject_moved_resource(file_name, args[0].span())?;
                    let fd_slot = self
                        .scopes
                        .get(file_name)
                        .and_then(RuntimeGenericBinding::as_owned_file)
                        .ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(type_error(
                                "file_read() requires a live file owner",
                                args[0].span(),
                            ))
                        })?;
                    let u64_ty = RuntimeIntType::new(false, 64)?;
                    let max_bytes = self.lower_expr_as_type(&args[1], u64_ty)?;
                    let capacity_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::BinOp {
                        dst: capacity_slot,
                        op: RuntimeBinOp::Add,
                        lhs: max_bytes,
                        rhs: RuntimeOperand::Imm(1),
                    });
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::GtUnsigned,
                        RuntimeOperand::Slot(capacity_slot),
                        max_bytes,
                        101,
                    )?;
                    let ptr_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Alloc {
                        dst: ptr_slot,
                        size: RuntimeOperand::Slot(capacity_slot),
                    });
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::Ne,
                        RuntimeOperand::Slot(ptr_slot),
                        RuntimeOperand::Imm(0),
                        101,
                    )?;
                    let len_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: len_slot,
                        src: RuntimeOperand::Imm(0),
                    });
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::OwnedString {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot: capacity_slot,
                            mutable: *mutable,
                            is_path: false,
                        },
                    );
                    self.emit(RuntimeInstr::FileRead {
                        dst: len_slot,
                        fd: RuntimeOperand::Slot(fd_slot),
                        ptr: RuntimeOperand::Slot(ptr_slot),
                        len: max_bytes,
                    });
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::Ne,
                        RuntimeOperand::Slot(len_slot),
                        RuntimeOperand::Imm(u64::MAX),
                        104,
                    )?;
                    self.emit(RuntimeInstr::HeapStoreInt {
                        ptr: RuntimeOperand::Slot(ptr_slot),
                        index: RuntimeOperand::Slot(len_slot),
                        src: RuntimeOperand::Imm(0),
                        bytes: 1,
                    });
                    return Ok(());
                }
                if let Expr::MethodCall {
                    receiver,
                    name: method_name,
                    args,
                    ..
                } = expr
                    && method_name == "unwrap_or"
                    && let Some(layout) = self.native_option_struct_layout(receiver)
                {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "unwrap_or() expects exactly one fallback argument",
                            *span,
                        )));
                    }
                    match ty.as_ref() {
                        Some(TypeName::Struct(struct_name))
                            if struct_name == &layout.struct_name => {}
                        None => {}
                        _ => {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!(
                                    "aggregate unwrap_or() requires type '{}'",
                                    layout.struct_name
                                )
                                .as_str(),
                                *span,
                            )));
                        }
                    }
                    let fallback = self.lower_runtime_struct_operands(&args[0], &layout, *span)?;
                    let (tag, payload, _) =
                        self.lower_expr_as_option_struct(receiver, Some(&layout))?;
                    let mut fields = HashMap::with_capacity(layout.fields.len());
                    for (field, value) in fallback {
                        let slot = self.alloc_slot();
                        self.emit(RuntimeInstr::Mov {
                            dst: slot,
                            src: value,
                        });
                        self.normalize_scalar_slot(slot, field.ty);
                        fields.insert(field.name, (slot, field.ty));
                    }
                    let absent = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Ne,
                        lhs: tag,
                        rhs: RuntimeOperand::Imm(0),
                        target: usize::MAX,
                    });
                    for (field, value) in payload {
                        let (slot, _) = fields
                            .get(&field.name)
                            .copied()
                            .expect("aggregate unwrap destination");
                        self.emit(RuntimeInstr::Mov {
                            dst: slot,
                            src: value,
                        });
                        self.normalize_scalar_slot(slot, field.ty);
                    }
                    let done = self.instrs.len();
                    self.patch_target(absent, done)?;
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::StructSlots {
                            struct_name: layout.struct_name,
                            fields,
                            mutable: *mutable,
                        },
                    );
                    return Ok(());
                }
                if let Expr::Index { base, index, .. } = expr
                    && let Expr::Ident {
                        name: list_name, ..
                    } = base.as_ref()
                    && let Some((ptr_slot, index_operand, layout)) =
                        self.lower_owned_list_struct_checked_index(list_name, index, *span)?
                {
                    match ty.as_ref() {
                        Some(TypeName::Struct(name)) if name == &layout.struct_name => {}
                        None => {}
                        _ => {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!(
                                    "indexed '{}' list value requires type '{}'",
                                    layout.struct_name, layout.struct_name
                                )
                                .as_str(),
                                *span,
                            )));
                        }
                    }
                    let mut fields = HashMap::with_capacity(layout.fields.len());
                    for field in &layout.fields {
                        let slot = self.alloc_slot();
                        let field_index =
                            self.struct_field_index(index_operand.clone(), &layout, field);
                        self.emit(RuntimeInstr::HeapLoadInt {
                            dst: slot,
                            ptr: RuntimeOperand::Slot(ptr_slot),
                            index: field_index,
                            bytes: field.ty.storage_bytes(),
                        });
                        self.normalize_scalar_slot(slot, field.ty);
                        fields.insert(field.name.clone(), (slot, field.ty));
                    }
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::StructSlots {
                            struct_name: layout.struct_name,
                            fields,
                            mutable: *mutable,
                        },
                    );
                    return Ok(());
                }
                if let Some(container_binding) =
                    self.try_build_const_container_binding(*mutable, ty.as_ref(), expr, *span)?
                {
                    self.scopes.insert(name.clone(), container_binding);
                    return Ok(());
                }

                let scalar_ty = if let Some(ty) = ty {
                    ensure_runtime_generic_scalar_type(ty, *span)
                        .map_err(RuntimeGenericLowerError::Diagnostic)?
                } else {
                    self.infer_expr_scalar_type(expr)?
                };
                let src = self.lower_expr_as_scalar(expr, scalar_ty)?;
                let slot = self.alloc_slot();
                self.emit(RuntimeInstr::Mov { dst: slot, src });
                self.normalize_scalar_slot(slot, scalar_ty);
                self.scopes.insert(
                    name.clone(),
                    RuntimeGenericBinding::Scalar {
                        slot,
                        mutable: *mutable,
                        ty: scalar_ty,
                    },
                );
                let upper_bound = self.infer_scalar_unsigned_upper_bound(scalar_ty, expr);
                self.apply_scalar_unsigned_upper_bound(slot, scalar_ty, upper_bound);
                Ok(())
            }
            Stmt::Assign { name, expr, span } => {
                enum AssignTarget {
                    Scalar {
                        slot: usize,
                        mutable: bool,
                        ty: RuntimeScalarType,
                    },
                    Struct {
                        fields: HashMap<String, (usize, RuntimeScalarType)>,
                        mutable: bool,
                    },
                    OptionScalar {
                        tag_slot: usize,
                        payload_slot: usize,
                        mutable: bool,
                        elem_ty: RuntimeScalarType,
                    },
                    OptionStruct {
                        tag_slot: usize,
                        fields: HashMap<String, (usize, RuntimeScalarType)>,
                        mutable: bool,
                        layout: RuntimeStructListLayout,
                    },
                    Enum {
                        layout: RuntimeEnumLayout,
                        tag_slot: usize,
                        payload_slots: Vec<usize>,
                        mutable: bool,
                    },
                }

                let target = {
                    let binding = self.scopes.get(name).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                            format!("unknown identifier '{name}'"),
                            span,
                        ))
                    })?;
                    match binding {
                        RuntimeGenericBinding::Scalar { slot, mutable, ty } => {
                            AssignTarget::Scalar {
                                slot: *slot,
                                mutable: *mutable,
                                ty: *ty,
                            }
                        }
                        RuntimeGenericBinding::StructSlots {
                            fields, mutable, ..
                        } => AssignTarget::Struct {
                            fields: fields.clone(),
                            mutable: *mutable,
                        },
                        RuntimeGenericBinding::OptionScalar {
                            tag_slot,
                            payload_slot,
                            mutable,
                            elem_ty,
                        } => AssignTarget::OptionScalar {
                            tag_slot: *tag_slot,
                            payload_slot: *payload_slot,
                            mutable: *mutable,
                            elem_ty: *elem_ty,
                        },
                        RuntimeGenericBinding::OptionStruct {
                            tag_slot,
                            fields,
                            mutable,
                            layout,
                        } => AssignTarget::OptionStruct {
                            tag_slot: *tag_slot,
                            fields: fields.clone(),
                            mutable: *mutable,
                            layout: layout.clone(),
                        },
                        RuntimeGenericBinding::EnumSlots {
                            layout,
                            tag_slot,
                            payload_slots,
                            mutable,
                            ..
                        } => AssignTarget::Enum {
                            layout: layout.clone(),
                            tag_slot: *tag_slot,
                            payload_slots: payload_slots.clone(),
                            mutable: *mutable,
                        },
                        RuntimeGenericBinding::ArraySlots { .. }
                        | RuntimeGenericBinding::DictSlots { .. }
                        | RuntimeGenericBinding::ConstContainer { .. }
                        | RuntimeGenericBinding::OwnedStruct { .. }
                        | RuntimeGenericBinding::OwnedPtr { .. }
                        | RuntimeGenericBinding::OwnedFile { .. }
                        | RuntimeGenericBinding::OwnedThread { .. }
                        | RuntimeGenericBinding::OwnedChannel { .. }
                        | RuntimeGenericBinding::OwnedSender { .. }
                        | RuntimeGenericBinding::OwnedReceiver { .. }
                        | RuntimeGenericBinding::BorrowedFile { .. }
                        | RuntimeGenericBinding::MovedResource { .. }
                        | RuntimeGenericBinding::OwnedListScalar { .. }
                        | RuntimeGenericBinding::OwnedListStruct { .. }
                        | RuntimeGenericBinding::OwnedString { .. }
                        | RuntimeGenericBinding::OwnedMap { .. } => {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("cannot assign to runtime binding '{name}'").as_str(),
                                *span,
                            )));
                        }
                    }
                };

                match target {
                    AssignTarget::Scalar {
                        slot,
                        mutable,
                        ty: scalar_ty,
                    } => {
                        if !mutable {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("cannot assign to immutable '{name}'").as_str(),
                                *span,
                            )));
                        }
                        let upper_bound = self.infer_scalar_unsigned_upper_bound(scalar_ty, expr);

                        if let RuntimeScalarType::Int(int_ty) = scalar_ty {
                            if self.try_lower_assign_in_place_chain(name, slot, int_ty, expr)? {
                                self.apply_scalar_unsigned_upper_bound(
                                    slot,
                                    scalar_ty,
                                    upper_bound,
                                );
                                return Ok(());
                            }
                        }

                        let src = self.lower_expr_as_scalar(expr, scalar_ty)?;
                        self.emit(RuntimeInstr::Mov { dst: slot, src });
                        self.normalize_scalar_slot(slot, scalar_ty);
                        self.apply_scalar_unsigned_upper_bound(slot, scalar_ty, upper_bound);
                        Ok(())
                    }
                    AssignTarget::Struct { fields, mutable } => {
                        if !mutable {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("cannot assign to immutable '{name}'").as_str(),
                                *span,
                            )));
                        }
                        self.lower_struct_slot_assign(name, &fields, expr, *span)
                    }
                    AssignTarget::OptionScalar {
                        tag_slot,
                        payload_slot,
                        mutable,
                        elem_ty,
                    } => {
                        if !mutable {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("cannot assign to immutable '{name}'").as_str(),
                                *span,
                            )));
                        }
                        let (tag, payload, _) =
                            self.lower_expr_as_option_scalar(expr, Some(elem_ty))?;
                        self.emit(RuntimeInstr::Mov {
                            dst: tag_slot,
                            src: tag,
                        });
                        self.emit(RuntimeInstr::Mov {
                            dst: payload_slot,
                            src: payload,
                        });
                        self.normalize_scalar_slot(payload_slot, elem_ty);
                        Ok(())
                    }
                    AssignTarget::OptionStruct {
                        tag_slot,
                        fields,
                        mutable,
                        layout,
                    } => {
                        if !mutable {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("cannot assign to immutable '{name}'").as_str(),
                                *span,
                            )));
                        }
                        let (tag, values, _) =
                            self.lower_expr_as_option_struct(expr, Some(&layout))?;
                        self.emit(RuntimeInstr::Mov {
                            dst: tag_slot,
                            src: tag,
                        });
                        for (field, value) in values {
                            let (slot, _) = fields
                                .get(&field.name)
                                .copied()
                                .expect("option assignment field slot");
                            self.emit(RuntimeInstr::Mov {
                                dst: slot,
                                src: value,
                            });
                            self.normalize_scalar_slot(slot, field.ty);
                        }
                        Ok(())
                    }
                    AssignTarget::Enum {
                        layout,
                        tag_slot,
                        payload_slots,
                        mutable,
                    } => {
                        if !mutable {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("cannot assign to immutable '{name}'").as_str(),
                                *span,
                            )));
                        }
                        let ty = if layout.type_args.is_empty() {
                            TypeName::Struct(layout.enum_name.clone())
                        } else {
                            TypeName::Applied {
                                name: layout.enum_name.clone(),
                                args: layout.type_args.clone(),
                            }
                        };
                        let source_name = if let Expr::Ident {
                            name: source_name,
                            span: source_span,
                        } = expr
                        {
                            if layout.owns_resources() && source_name == name {
                                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                    "resource enum cannot be assigned to itself",
                                    *source_span,
                                )));
                            }
                            Some((source_name.clone(), *source_span))
                        } else {
                            None
                        };
                        let source = if let Some((source_name, source_span)) = &source_name {
                            let source = if layout.owns_resources() {
                                self.scopes.get_current(source_name).cloned()
                            } else {
                                self.scopes.get(source_name).cloned()
                            };
                            if source.is_none() {
                                self.reject_moved_resource(source_name, *source_span)?;
                            }
                            source
                        } else {
                            self.lower_runtime_enum_constructor(&ty, expr, false, *span)?
                        };
                        let Some(RuntimeGenericBinding::EnumSlots {
                            layout: source_layout,
                            tag_slot: source_tag,
                            payload_slots: source_payload,
                            owns_cleanup: source_owns_cleanup,
                            ..
                        }) = source
                        else {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "enum assignment requires a matching enum value",
                                *span,
                            )));
                        };
                        if source_layout != layout || source_payload.len() != payload_slots.len() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "enum assignment layout mismatch",
                                *span,
                            )));
                        }
                        if layout.owns_resources() && !source_owns_cleanup {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "resource enum assignment requires an owning source",
                                *span,
                            )));
                        }
                        if layout.owns_resources() {
                            self.emit_runtime_enum_resource_cleanup(
                                &layout,
                                tag_slot,
                                &payload_slots,
                            )?;
                        }
                        self.emit(RuntimeInstr::Mov {
                            dst: tag_slot,
                            src: RuntimeOperand::Slot(source_tag),
                        });
                        for (dst, src) in payload_slots.into_iter().zip(source_payload) {
                            self.emit(RuntimeInstr::Mov {
                                dst,
                                src: RuntimeOperand::Slot(src),
                            });
                        }
                        if layout.owns_resources()
                            && let Some((source_name, _)) = source_name
                        {
                            let _ = self.scopes.take_current(&source_name);
                            self.scopes.insert(
                                source_name,
                                RuntimeGenericBinding::MovedResource {
                                    kind: "resource enum owner",
                                },
                            );
                        }
                        Ok(())
                    }
                }
            }
            Stmt::AssignStructListIndex {
                receiver,
                field,
                index,
                expr,
                span,
            } => {
                let (list_field, mutable) = match self.scopes.get(receiver) {
                    Some(RuntimeGenericBinding::OwnedStruct {
                        list_fields,
                        mutable,
                        ..
                    }) => (list_fields.get(field).cloned(), *mutable),
                    Some(RuntimeGenericBinding::MovedResource { .. }) => {
                        self.reject_moved_resource(receiver, *span)?;
                        (None, false)
                    }
                    _ => (None, false),
                };
                if !mutable {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("cannot assign through immutable '{receiver}'").as_str(),
                        *span,
                    )));
                }
                match list_field {
                    Some(RuntimeOwnedStructListField::Scalar {
                        ptr_slot,
                        len_slot,
                        elem_ty,
                        ..
                    }) => {
                        let (_, index_operand, _) = self
                            .lower_owned_list_scalar_checked_index_slots(
                                ptr_slot, len_slot, elem_ty, index, *span,
                            )?;
                        let value = self.lower_expr_as_scalar(expr, elem_ty)?;
                        let value = self.canonicalize_scalar_operand(value, elem_ty);
                        self.emit(RuntimeInstr::HeapStoreInt {
                            ptr: RuntimeOperand::Slot(ptr_slot),
                            index: index_operand,
                            src: value,
                            bytes: elem_ty.storage_bytes(),
                        });
                        Ok(())
                    }
                    Some(RuntimeOwnedStructListField::Struct {
                        ptr_slot,
                        len_slot,
                        layout,
                        ..
                    }) => {
                        let index_ty = self.infer_expr_int_type(index)?;
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
                        for (field, value) in
                            self.lower_runtime_struct_operands(expr, &layout, *span)?
                        {
                            let field_index =
                                self.struct_field_index(index_operand.clone(), &layout, &field);
                            let value = self.canonicalize_scalar_operand(value, field.ty);
                            self.emit(RuntimeInstr::HeapStoreInt {
                                ptr: RuntimeOperand::Slot(ptr_slot),
                                index: field_index,
                                src: value,
                                bytes: field.ty.storage_bytes(),
                            });
                        }
                        Ok(())
                    }
                    None => Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "indexed struct field must be a runtime-native list",
                        *span,
                    ))),
                }
            }
            Stmt::AssignIndex {
                name,
                index,
                expr,
                span,
            } => {
                if let Some((ptr_slot, index_operand, layout)) =
                    self.lower_owned_list_struct_checked_index(name, index, *span)?
                {
                    let mutable = self
                        .scopes
                        .get(name)
                        .and_then(RuntimeGenericBinding::as_owned_list_struct)
                        .map(|(_, _, _, _, mutable, _)| mutable)
                        .unwrap_or(false);
                    if !mutable {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!("cannot assign to immutable '{name}'").as_str(),
                            *span,
                        )));
                    }
                    let values = self.lower_runtime_struct_operands(expr, &layout, *span)?;
                    for (field, value) in values {
                        let field_index =
                            self.struct_field_index(index_operand.clone(), &layout, &field);
                        let value = self.canonicalize_scalar_operand(value, field.ty);
                        self.emit(RuntimeInstr::HeapStoreInt {
                            ptr: RuntimeOperand::Slot(ptr_slot),
                            index: field_index,
                            src: value,
                            bytes: field.ty.storage_bytes(),
                        });
                    }
                    return Ok(());
                }
                if let Some((ptr_slot, index_operand, elem_ty)) =
                    self.lower_owned_list_scalar_checked_index(name, index, *span)?
                {
                    let mutable = self
                        .scopes
                        .get(name)
                        .and_then(RuntimeGenericBinding::as_owned_list_scalar)
                        .map(|(_, _, _, _, mutable, _)| mutable)
                        .unwrap_or(false);
                    if !mutable {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!("cannot assign to immutable '{name}'").as_str(),
                            *span,
                        )));
                    }
                    let value = self.lower_expr_as_scalar(expr, elem_ty)?;
                    let value = self.canonicalize_scalar_operand(value, elem_ty);
                    self.emit(RuntimeInstr::HeapStoreInt {
                        ptr: RuntimeOperand::Slot(ptr_slot),
                        index: index_operand,
                        src: value,
                        bytes: elem_ty.storage_bytes(),
                    });
                    return Ok(());
                }
                if self.lower_array_slot_assign(name, index, expr, *span)? {
                    return Ok(());
                }
                if self.lower_dict_slot_assign(name, index, expr, *span)? {
                    return Ok(());
                }
                Err(RuntimeGenericLowerError::Diagnostic(type_error(
                    format!("cannot index-assign non-container runtime binding '{name}'").as_str(),
                    *span,
                )))
            }
            Stmt::AssignField {
                receiver,
                field,
                expr,
                span,
            } => {
                let (dst_slot, mutable, field_ty) = {
                    let binding = self.scopes.get(receiver).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                            format!("unknown identifier '{receiver}'"),
                            span,
                        ))
                    })?;
                    let (fields, mutable) = match binding {
                        RuntimeGenericBinding::StructSlots {
                            fields, mutable, ..
                        } => (fields, *mutable),
                        RuntimeGenericBinding::OwnedStruct {
                            scalar_fields,
                            mutable,
                            ..
                        } => (scalar_fields, *mutable),
                        _ => {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!(
                                    "cannot assign field on non-struct runtime binding '{receiver}'"
                                )
                                .as_str(),
                                *span,
                            )));
                        }
                    };
                    let (slot, ty) = fields.get(field).copied().ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            format!("unknown struct field '{field}'").as_str(),
                            *span,
                        ))
                    })?;
                    (slot, mutable, ty)
                };
                if !mutable {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!("cannot assign to immutable '{receiver}'").as_str(),
                        *span,
                    )));
                }
                let src = self.lower_expr_as_scalar(expr, field_ty)?;
                self.emit(RuntimeInstr::Mov { dst: dst_slot, src });
                self.normalize_scalar_slot(dst_slot, field_ty);
                Ok(())
            }
            Stmt::StructListMethodCall {
                receiver,
                field,
                name,
                args,
                span,
            } => self.lower_runtime_owned_struct_list_method(receiver, field, name, args, *span),
            Stmt::MethodCall {
                receiver,
                name,
                args,
                span,
            } => self.lower_runtime_method_stmt(receiver, name, args, *span),
            Stmt::Block { stmts, .. } => {
                self.scopes.push();
                self.lower_stmts(stmts)?;
                self.pop_scope_with_cleanup();
                Ok(())
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                if self.try_lower_branchless_bool_clear(cond, then_branch, else_branch)? {
                    return Ok(());
                }
                let jz = self.emit_jump_if_false(cond)?;
                self.scopes.push();
                self.lower_stmts(then_branch)?;
                self.pop_scope_with_cleanup();

                if let Some(else_branch) = else_branch {
                    let jmp_end = self.emit(RuntimeInstr::Jump { target: usize::MAX });
                    let else_target = self.instrs.len();
                    self.patch_target(jz, else_target)?;
                    self.scopes.push();
                    self.lower_stmts(else_branch)?;
                    self.pop_scope_with_cleanup();
                    let end = self.instrs.len();
                    self.patch_target(jmp_end, end)?;
                } else {
                    let end = self.instrs.len();
                    self.patch_target(jz, end)?;
                }
                Ok(())
            }
            Stmt::While { cond, body, .. } => {
                let loop_start = self.instrs.len();
                let jz = self.emit_jump_if_false(cond)?;
                self.scopes.push();
                self.push_loop_frame(Some(loop_start));
                self.lower_stmts(body)?;
                let frame = self.pop_loop_frame()?;
                self.pop_scope_with_cleanup();
                self.emit(RuntimeInstr::Jump { target: loop_start });
                let end = self.instrs.len();
                self.patch_target(jz, end)?;
                self.patch_loop_frame_targets(frame, loop_start, end)?;
                Ok(())
            }
            Stmt::Loop { body, .. } => {
                let loop_start = self.instrs.len();
                self.scopes.push();
                self.push_loop_frame(Some(loop_start));
                self.lower_stmts(body)?;
                let frame = self.pop_loop_frame()?;
                self.pop_scope_with_cleanup();
                self.emit(RuntimeInstr::Jump { target: loop_start });
                let loop_end = self.instrs.len();
                self.patch_loop_frame_targets(frame, loop_start, loop_end)?;
                Ok(())
            }
            Stmt::For {
                name,
                start,
                end,
                body,
                span,
            } => {
                self.scopes.push();
                if self.scopes.current_contains(name) {
                    return Err(RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                        format!("redefinition of '{name}'"),
                        span,
                    )));
                }
                let iter_ty = self.infer_expr_int_type(start)?;
                let start_op = self.lower_expr_as_type(start, iter_ty)?;
                let end_op = self.lower_expr_as_type(end, iter_ty)?;
                let iter_slot = self.alloc_slot();
                self.emit(RuntimeInstr::Mov {
                    dst: iter_slot,
                    src: start_op,
                });
                self.normalize_slot(iter_slot, iter_ty);
                self.scopes.insert(
                    name.clone(),
                    RuntimeGenericBinding::Scalar {
                        slot: iter_slot,
                        mutable: false,
                        ty: RuntimeScalarType::Int(iter_ty),
                    },
                );

                let loop_start = self.instrs.len();
                let cmp_op = iter_ty
                    .cmp_from_binary(BinaryOp::Lt)
                    .ok_or(RuntimeGenericLowerError::Unsupported)?;
                let jz = self.emit(RuntimeInstr::JumpIfCmpFalse {
                    op: cmp_op,
                    lhs: RuntimeOperand::Slot(iter_slot),
                    rhs: end_op,
                    target: usize::MAX,
                });
                let unchecked_loop_accesses = self.push_unchecked_loop_array_accesses(
                    name, iter_slot, iter_ty, start_op, end_op, *span,
                )?;
                self.push_loop_frame(None);
                self.lower_stmts(body)?;
                let frame = self.pop_loop_frame()?;
                self.pop_unchecked_loop_array_accesses(unchecked_loop_accesses);
                let loop_scope_depth = self.scopes.depth().saturating_sub(1);
                self.emit_owned_cleanup_from(loop_scope_depth);
                let continue_target = self.instrs.len();
                self.emit(RuntimeInstr::BinOpInPlace {
                    dst: iter_slot,
                    op: RuntimeBinOp::Add,
                    rhs: RuntimeOperand::Imm(1),
                });
                self.normalize_slot(iter_slot, iter_ty);
                self.emit(RuntimeInstr::Jump { target: loop_start });
                let loop_end = self.instrs.len();
                self.patch_target(jz, loop_end)?;
                self.patch_loop_frame_targets(frame, continue_target, loop_end)?;
                self.scopes.pop();
                Ok(())
            }
            Stmt::ParFor {
                name,
                start,
                end,
                body,
                reduction,
                span,
            } => {
                let reduction_info = if let Some(reduction) = reduction {
                    let (target_slot, target_mutable, target_scalar_ty) = {
                        let binding = self.scopes.get(&reduction.target).ok_or_else(|| {
                            RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                                format!("unknown identifier '{}'", reduction.target),
                                reduction.span,
                            ))
                        })?;
                        let Some((slot, mutable, scalar_ty)) = binding.as_scalar() else {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                format!("reduction target '{}' must be scalar", reduction.target)
                                    .as_str(),
                                reduction.span,
                            )));
                        };
                        (slot, mutable, scalar_ty)
                    };
                    if !target_mutable {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            format!("reduction target '{}' must be mutable", reduction.target)
                                .as_str(),
                            reduction.span,
                        )));
                    }
                    let RuntimeScalarType::Int(target_int_ty) = target_scalar_ty else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "parfor reductions require integer targets for deterministic associativity",
                            reduction.span,
                        )));
                    };
                    Some((reduction.clone(), target_slot, target_int_ty))
                } else {
                    None
                };

                self.scopes.push();
                if self.scopes.current_contains(name) {
                    return Err(RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                        format!("redefinition of '{name}'"),
                        span,
                    )));
                }
                let iter_ty = self.infer_expr_int_type(start)?;
                let start_op = self.lower_expr_as_type(start, iter_ty)?;
                let end_op = self.lower_expr_as_type(end, iter_ty)?;
                let iter_slot = self.alloc_slot();
                self.emit(RuntimeInstr::Mov {
                    dst: iter_slot,
                    src: start_op,
                });
                self.normalize_slot(iter_slot, iter_ty);
                self.scopes.insert(
                    name.clone(),
                    RuntimeGenericBinding::Scalar {
                        slot: iter_slot,
                        mutable: false,
                        ty: RuntimeScalarType::Int(iter_ty),
                    },
                );
                let loop_start = self.instrs.len();
                let cmp_op = iter_ty
                    .cmp_from_binary(BinaryOp::Lt)
                    .ok_or(RuntimeGenericLowerError::Unsupported)?;
                let jz = self.emit(RuntimeInstr::JumpIfCmpFalse {
                    op: cmp_op,
                    lhs: RuntimeOperand::Slot(iter_slot),
                    rhs: end_op,
                    target: usize::MAX,
                });
                let unchecked_loop_accesses = self.push_unchecked_loop_array_accesses(
                    name, iter_slot, iter_ty, start_op, end_op, *span,
                )?;

                if let Some((reduction, target_slot, target_int_ty)) = reduction_info {
                    let snapshot_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: snapshot_slot,
                        src: RuntimeOperand::Slot(target_slot),
                    });
                    self.normalize_slot(snapshot_slot, target_int_ty);
                    if reduction.target != *name {
                        self.scopes.insert(
                            reduction.target.clone(),
                            RuntimeGenericBinding::Scalar {
                                slot: snapshot_slot,
                                mutable: false,
                                ty: RuntimeScalarType::Int(target_int_ty),
                            },
                        );
                    }

                    let acc_slot = self.alloc_slot();
                    let has_acc_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: has_acc_slot,
                        src: RuntimeOperand::Imm(0),
                    });

                    let reduced_value = self.lower_expr_as_type(&reduction.expr, target_int_ty)?;
                    let jump_if_has_acc = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Eq,
                        lhs: RuntimeOperand::Slot(has_acc_slot),
                        rhs: RuntimeOperand::Imm(0),
                        target: usize::MAX,
                    });
                    self.emit(RuntimeInstr::Mov {
                        dst: acc_slot,
                        src: reduced_value.clone(),
                    });
                    self.normalize_slot(acc_slot, target_int_ty);
                    self.emit(RuntimeInstr::Mov {
                        dst: has_acc_slot,
                        src: RuntimeOperand::Imm(1),
                    });
                    let jump_after_reduce = self.emit(RuntimeInstr::Jump { target: usize::MAX });

                    let reduce_merge_target = self.instrs.len();
                    self.patch_target(jump_if_has_acc, reduce_merge_target)?;
                    match reduction.op {
                        ReductionOp::Sum => {
                            self.emit(RuntimeInstr::BinOpInPlace {
                                dst: acc_slot,
                                op: RuntimeBinOp::Add,
                                rhs: reduced_value,
                            });
                            self.normalize_slot(acc_slot, target_int_ty);
                        }
                        ReductionOp::Min | ReductionOp::Max => {
                            let cmp_op = if matches!(reduction.op, ReductionOp::Min) {
                                target_int_ty
                                    .cmp_from_binary(BinaryOp::Lt)
                                    .ok_or(RuntimeGenericLowerError::Unsupported)?
                            } else {
                                target_int_ty
                                    .cmp_from_binary(BinaryOp::Gt)
                                    .ok_or(RuntimeGenericLowerError::Unsupported)?
                            };
                            let skip_update = self.emit(RuntimeInstr::JumpIfCmpFalse {
                                op: cmp_op,
                                lhs: reduced_value,
                                rhs: RuntimeOperand::Slot(acc_slot),
                                target: usize::MAX,
                            });
                            self.emit(RuntimeInstr::Mov {
                                dst: acc_slot,
                                src: reduced_value,
                            });
                            self.normalize_slot(acc_slot, target_int_ty);
                            let after_update = self.instrs.len();
                            self.patch_target(skip_update, after_update)?;
                        }
                    }
                    let after_reduce_target = self.instrs.len();
                    self.patch_target(jump_after_reduce, after_reduce_target)?;

                    self.emit(RuntimeInstr::BinOpInPlace {
                        dst: iter_slot,
                        op: RuntimeBinOp::Add,
                        rhs: RuntimeOperand::Imm(1),
                    });
                    self.normalize_slot(iter_slot, iter_ty);
                    self.emit(RuntimeInstr::Jump { target: loop_start });
                    let loop_end = self.instrs.len();
                    self.patch_target(jz, loop_end)?;

                    let jump_if_non_empty = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::Eq,
                        lhs: RuntimeOperand::Slot(has_acc_slot),
                        rhs: RuntimeOperand::Imm(0),
                        target: usize::MAX,
                    });
                    self.emit_terminal_cleanup_all();
                    self.emit(RuntimeInstr::Exit {
                        code: RuntimeOperand::Imm(1),
                    });
                    let assign_target = self.instrs.len();
                    self.patch_target(jump_if_non_empty, assign_target)?;
                    self.emit(RuntimeInstr::Mov {
                        dst: target_slot,
                        src: RuntimeOperand::Slot(acc_slot),
                    });
                    self.normalize_slot(target_slot, target_int_ty);
                } else {
                    self.push_loop_frame(None);
                    self.lower_stmts(body)?;
                    let frame = self.pop_loop_frame()?;
                    let loop_scope_depth = self.scopes.depth().saturating_sub(1);
                    self.emit_owned_cleanup_from(loop_scope_depth);
                    let continue_target = self.instrs.len();
                    self.emit(RuntimeInstr::BinOpInPlace {
                        dst: iter_slot,
                        op: RuntimeBinOp::Add,
                        rhs: RuntimeOperand::Imm(1),
                    });
                    self.normalize_slot(iter_slot, iter_ty);
                    self.emit(RuntimeInstr::Jump { target: loop_start });
                    let loop_end = self.instrs.len();
                    self.patch_target(jz, loop_end)?;
                    self.pop_unchecked_loop_array_accesses(unchecked_loop_accesses);
                    self.patch_loop_frame_targets(frame, continue_target, loop_end)?;
                    self.scopes.pop();
                    return Ok(());
                }
                self.pop_unchecked_loop_array_accesses(unchecked_loop_accesses);
                self.pop_scope_with_cleanup();
                Ok(())
            }
            Stmt::ForEach {
                name,
                iterable,
                body,
                span,
            } => {
                if let Some(RuntimeOwnedStructListField::Scalar {
                    ptr_slot,
                    len_slot,
                    elem_ty,
                    ..
                }) = self.owned_struct_list_field(iterable)
                {
                    self.scopes.push();
                    if self.scopes.current_contains(name) {
                        return Err(RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                            format!("redefinition of '{name}'"),
                            span,
                        )));
                    }
                    let index_slot = self.alloc_slot();
                    let elem_slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: index_slot,
                        src: RuntimeOperand::Imm(0),
                    });
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::Scalar {
                            slot: elem_slot,
                            mutable: false,
                            ty: elem_ty,
                        },
                    );
                    let loop_start = self.instrs.len();
                    let loop_done = self.emit(RuntimeInstr::JumpIfCmpFalse {
                        op: RuntimeCmpOp::LtUnsigned,
                        lhs: RuntimeOperand::Slot(index_slot),
                        rhs: RuntimeOperand::Slot(len_slot),
                        target: usize::MAX,
                    });
                    self.emit(RuntimeInstr::HeapLoadInt {
                        dst: elem_slot,
                        ptr: RuntimeOperand::Slot(ptr_slot),
                        index: RuntimeOperand::Slot(index_slot),
                        bytes: elem_ty.storage_bytes(),
                    });
                    self.normalize_scalar_slot(elem_slot, elem_ty);
                    self.push_loop_frame(None);
                    self.lower_stmts(body)?;
                    let frame = self.pop_loop_frame()?;
                    let loop_scope_depth = self.scopes.depth().saturating_sub(1);
                    self.emit_owned_cleanup_from(loop_scope_depth);
                    let continue_target = self.instrs.len();
                    self.emit(RuntimeInstr::BinOpInPlace {
                        dst: index_slot,
                        op: RuntimeBinOp::Add,
                        rhs: RuntimeOperand::Imm(1),
                    });
                    self.emit(RuntimeInstr::Jump { target: loop_start });
                    let loop_end = self.instrs.len();
                    self.patch_target(loop_done, loop_end)?;
                    self.patch_loop_frame_targets(frame, continue_target, loop_end)?;
                    self.scopes.pop();
                    return Ok(());
                }
                if let Expr::Ident { name: arr_name, .. } = iterable {
                    if let Some((ptr_slot, len_slot, _, _, _, layout)) = self
                        .scopes
                        .get(arr_name)
                        .and_then(RuntimeGenericBinding::as_owned_list_struct)
                    {
                        self.scopes.push();
                        let index_slot = self.alloc_slot();
                        self.emit(RuntimeInstr::Mov {
                            dst: index_slot,
                            src: RuntimeOperand::Imm(0),
                        });
                        let mut fields = HashMap::with_capacity(layout.fields.len());
                        for field in &layout.fields {
                            fields.insert(field.name.clone(), (self.alloc_slot(), field.ty));
                        }
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::StructSlots {
                                struct_name: layout.struct_name.clone(),
                                fields: fields.clone(),
                                mutable: false,
                            },
                        );

                        let loop_start = self.instrs.len();
                        let loop_done = self.emit(RuntimeInstr::JumpIfCmpFalse {
                            op: RuntimeCmpOp::LtUnsigned,
                            lhs: RuntimeOperand::Slot(index_slot),
                            rhs: RuntimeOperand::Slot(len_slot),
                            target: usize::MAX,
                        });
                        for field in &layout.fields {
                            let (slot, _) =
                                fields.get(&field.name).copied().expect("layout field slot");
                            let field_index = self.struct_field_index(
                                RuntimeOperand::Slot(index_slot),
                                &layout,
                                field,
                            );
                            self.emit(RuntimeInstr::HeapLoadInt {
                                dst: slot,
                                ptr: RuntimeOperand::Slot(ptr_slot),
                                index: field_index,
                                bytes: field.ty.storage_bytes(),
                            });
                            self.normalize_scalar_slot(slot, field.ty);
                        }
                        self.push_loop_frame(None);
                        self.lower_stmts(body)?;
                        let frame = self.pop_loop_frame()?;
                        let loop_scope_depth = self.scopes.depth().saturating_sub(1);
                        self.emit_owned_cleanup_from(loop_scope_depth);
                        let continue_target = self.instrs.len();
                        self.emit(RuntimeInstr::BinOpInPlace {
                            dst: index_slot,
                            op: RuntimeBinOp::Add,
                            rhs: RuntimeOperand::Imm(1),
                        });
                        self.emit(RuntimeInstr::Jump { target: loop_start });
                        let loop_end = self.instrs.len();
                        self.patch_target(loop_done, loop_end)?;
                        self.patch_loop_frame_targets(frame, continue_target, loop_end)?;
                        self.scopes.pop();
                        return Ok(());
                    }
                    if let Some((ptr_slot, len_slot, _, _, _, elem_ty)) = self
                        .scopes
                        .get(arr_name)
                        .and_then(RuntimeGenericBinding::as_owned_list_scalar)
                    {
                        self.scopes.push();
                        if self.scopes.current_contains(name) {
                            return Err(RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                                format!("redefinition of '{name}'"),
                                span,
                            )));
                        }
                        let index_slot = self.alloc_slot();
                        let elem_slot = self.alloc_slot();
                        self.emit(RuntimeInstr::Mov {
                            dst: index_slot,
                            src: RuntimeOperand::Imm(0),
                        });
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::Scalar {
                                slot: elem_slot,
                                mutable: false,
                                ty: elem_ty,
                            },
                        );

                        let loop_start = self.instrs.len();
                        let loop_done = self.emit(RuntimeInstr::JumpIfCmpFalse {
                            op: RuntimeCmpOp::LtUnsigned,
                            lhs: RuntimeOperand::Slot(index_slot),
                            rhs: RuntimeOperand::Slot(len_slot),
                            target: usize::MAX,
                        });
                        self.emit(RuntimeInstr::HeapLoadInt {
                            dst: elem_slot,
                            ptr: RuntimeOperand::Slot(ptr_slot),
                            index: RuntimeOperand::Slot(index_slot),
                            bytes: elem_ty.storage_bytes(),
                        });
                        self.normalize_scalar_slot(elem_slot, elem_ty);
                        self.push_loop_frame(None);
                        self.lower_stmts(body)?;
                        let frame = self.pop_loop_frame()?;
                        let loop_scope_depth = self.scopes.depth().saturating_sub(1);
                        self.emit_owned_cleanup_from(loop_scope_depth);
                        let continue_target = self.instrs.len();
                        self.emit(RuntimeInstr::BinOpInPlace {
                            dst: index_slot,
                            op: RuntimeBinOp::Add,
                            rhs: RuntimeOperand::Imm(1),
                        });
                        self.emit(RuntimeInstr::Jump { target: loop_start });
                        let loop_end = self.instrs.len();
                        self.patch_target(loop_done, loop_end)?;
                        self.patch_loop_frame_targets(frame, continue_target, loop_end)?;
                        self.scopes.pop();
                        return Ok(());
                    }
                    if let Some((slots, _len_slot, _mutable, elem_ty, full_len_known)) = self
                        .scopes
                        .get(arr_name)
                        .and_then(RuntimeGenericBinding::as_array_slots)
                    {
                        if full_len_known {
                            for elem_slot in slots {
                                self.scopes.push();
                                if self.scopes.current_contains(name) {
                                    return Err(RuntimeGenericLowerError::Diagnostic(
                                        Diagnostic::at_span(
                                            format!("redefinition of '{name}'"),
                                            span,
                                        ),
                                    ));
                                }
                                let slot = self.alloc_slot();
                                self.emit(RuntimeInstr::Mov {
                                    dst: slot,
                                    src: RuntimeOperand::Slot(elem_slot),
                                });
                                self.normalize_slot(slot, elem_ty);
                                self.scopes.insert(
                                    name.clone(),
                                    RuntimeGenericBinding::Scalar {
                                        slot,
                                        mutable: false,
                                        ty: RuntimeScalarType::Int(elem_ty),
                                    },
                                );
                                self.lower_stmts(body)?;
                                self.pop_scope_with_cleanup();
                            }
                            return Ok(());
                        }
                    }
                }
                let Some(RuntimeConstContainer::Array { elems }) =
                    self.resolve_const_container(iterable)
                else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                let elems = elems.clone();
                for elem in elems {
                    self.scopes.push();
                    if self.scopes.current_contains(name) {
                        return Err(RuntimeGenericLowerError::Diagnostic(Diagnostic::at_span(
                            format!("redefinition of '{name}'"),
                            span,
                        )));
                    }
                    let slot = self.alloc_slot();
                    self.emit(RuntimeInstr::Mov {
                        dst: slot,
                        src: RuntimeOperand::Imm(elem.encoded),
                    });
                    self.normalize_slot(slot, elem.ty);
                    self.scopes.insert(
                        name.clone(),
                        RuntimeGenericBinding::Scalar {
                            slot,
                            mutable: false,
                            ty: RuntimeScalarType::Int(elem.ty),
                        },
                    );
                    self.lower_stmts(body)?;
                    self.pop_scope_with_cleanup();
                }
                Ok(())
            }
            Stmt::Exit { expr, .. } => {
                let exit_ty = self.infer_expr_int_type(expr)?;
                let code = self.lower_expr_as_type(expr, exit_ty)?;
                self.emit_terminal_cleanup_all();
                self.emit(RuntimeInstr::Exit { code });
                Ok(())
            }
            Stmt::Print { expr, span } => {
                if let Expr::String { value, .. } = expr {
                    self.emit(RuntimeInstr::PrintConst {
                        text: value.clone(),
                    });
                    return Ok(());
                }
                let int_ty = match self.infer_expr_int_type(expr) {
                    Ok(ty) => ty,
                    Err(RuntimeGenericLowerError::Unsupported) => {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime generic print() currently supports integer and bool expressions",
                            *span,
                        )));
                    }
                    Err(err) => return Err(err),
                };
                let value = self.lower_expr_as_type(expr, int_ty)?;
                self.emit(RuntimeInstr::PrintInt {
                    value,
                    signed: int_ty.signed,
                    bits: int_ty.bits,
                });
                Ok(())
            }
            Stmt::Assert {
                cond,
                message,
                span,
            } => {
                if let Some(message) = message {
                    if !matches!(message, Expr::String { .. }) {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "runtime assert() currently requires a string literal message",
                            *span,
                        )));
                    }
                }

                let jz_fail = self.emit_jump_if_false(cond)?;
                let jmp_continue = self.emit(RuntimeInstr::Jump { target: usize::MAX });
                let fail_target = self.instrs.len();
                self.emit_terminal_cleanup_all();
                self.emit(RuntimeInstr::Exit {
                    code: RuntimeOperand::Imm(1),
                });
                let continue_target = self.instrs.len();
                self.patch_target(jz_fail, fail_target)?;
                self.patch_target(jmp_continue, continue_target)?;
                Ok(())
            }
            Stmt::Panic { message, span } => {
                if !matches!(message, Expr::String { .. }) {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "runtime panic() currently requires a string literal message",
                        *span,
                    )));
                }
                self.emit_terminal_cleanup_all();
                self.emit(RuntimeInstr::Exit {
                    code: RuntimeOperand::Imm(101),
                });
                Ok(())
            }
            Stmt::Call { name, args, span } => {
                if name == "file_close" {
                    if args.len() != 1 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "file_close() expects exactly one named file owner",
                            *span,
                        )));
                    }
                    let Expr::Ident {
                        name: owner_name, ..
                    } = &args[0]
                    else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "file_close() requires a named file owner",
                            args[0].span(),
                        )));
                    };
                    return self.lower_file_close_owner(owner_name, *span);
                }
                if name == "heap_alloc" {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        "heap_alloc() result must initialize a named immutable ownership binding",
                        *span,
                    )));
                }
                if name == "heap_free" {
                    if args.len() != 2 {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "heap_free() expects exactly two arguments (ptr, size)",
                            *span,
                        )));
                    }
                    let Expr::Ident {
                        name: owner_name, ..
                    } = &args[0]
                    else {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "heap_free() requires the named owner returned by heap_alloc()",
                            *span,
                        )));
                    };
                    self.reject_moved_resource(owner_name, *span)?;
                    let (ptr_slot, size_slot) = match self.scopes.get_current(owner_name) {
                        Some(RuntimeGenericBinding::OwnedPtr {
                            ptr_slot,
                            size_slot,
                        }) => (*ptr_slot, *size_slot),
                        _ => {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "heap_free() requires an owner declared in the current lexical scope",
                                *span,
                            )));
                        }
                    };
                    let u64_ty = RuntimeIntType::new(false, 64)?;
                    let supplied_size = self.lower_expr_as_type(&args[1], u64_ty)?;
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::Ne,
                        RuntimeOperand::Slot(ptr_slot),
                        RuntimeOperand::Imm(0),
                        101,
                    )?;
                    self.emit_guard_failure_exit(
                        RuntimeCmpOp::Eq,
                        supplied_size,
                        RuntimeOperand::Slot(size_slot),
                        101,
                    )?;
                    self.emit(RuntimeInstr::Free {
                        ptr: RuntimeOperand::Slot(ptr_slot),
                        size: RuntimeOperand::Slot(size_slot),
                    });
                    self.emit(RuntimeInstr::Mov {
                        dst: ptr_slot,
                        src: RuntimeOperand::Imm(0),
                    });
                    let consumed = self.scopes.take_current(owner_name).ok_or_else(|| {
                        RuntimeGenericLowerError::Diagnostic(type_error(
                            "heap ownership source disappeared during release",
                            *span,
                        ))
                    })?;
                    debug_assert!(matches!(consumed, RuntimeGenericBinding::OwnedPtr { .. }));
                    self.scopes.insert(
                        owner_name.clone(),
                        RuntimeGenericBinding::MovedResource { kind: "heap owner" },
                    );
                    return Ok(());
                }
                if let Some(diagnostic) = removed_benchmark_kernel_diagnostic(name, *span) {
                    return Err(RuntimeGenericLowerError::Diagnostic(diagnostic));
                }
                if !self.functions.contains_key(name) {
                    return Err(RuntimeGenericLowerError::Diagnostic(
                        unknown_function_diagnostic(name, *span),
                    ));
                }
                let param_layout = self.function_param_layout(name)?;
                if args.len() != param_layout.len() {
                    return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                        format!(
                            "function '{}' expects {} {}, got {}",
                            source_callable_name(name),
                            param_layout.len(),
                            argument_noun(param_layout.len()),
                            args.len()
                        )
                        .as_str(),
                        *span,
                    )));
                }
                self.emit_call_arguments(args, &param_layout)?;
                self.emit_call_target(name, *span);
                Ok(())
            }
            Stmt::Return { expr, span } => {
                let Some(function_name) = self.active_function.clone() else {
                    return Err(RuntimeGenericLowerError::Unsupported);
                };
                let return_layout = self.function_return_layout(&function_name)?;
                match (expr, return_layout) {
                    (
                        Some(expr),
                        Some(RuntimeFunctionReturnLayout::Scalar {
                            ty: ret_ty,
                            slot: ret_slot,
                        }),
                    ) => {
                        let src = self.lower_expr_as_scalar(expr, ret_ty)?;
                        self.emit(RuntimeInstr::Mov { dst: ret_slot, src });
                        self.normalize_scalar_slot(ret_slot, ret_ty);
                    }
                    (
                        Some(Expr::Ident {
                            name,
                            span: source_span,
                        }),
                        Some(RuntimeFunctionReturnLayout::OwnedFile { fd_slot }),
                    ) => {
                        self.reject_moved_resource(name, *source_span)?;
                        let source_fd = self
                            .scopes
                            .get_current(name)
                            .and_then(|binding| match binding {
                                RuntimeGenericBinding::OwnedFile { fd_slot } => Some(*fd_slot),
                                _ => None,
                            })
                            .ok_or_else(|| {
                                RuntimeGenericLowerError::Diagnostic(type_error(
                                    "returning File requires a current-scope owner",
                                    *source_span,
                                ))
                            })?;
                        self.emit(RuntimeInstr::Mov {
                            dst: fd_slot,
                            src: RuntimeOperand::Slot(source_fd),
                        });
                        let _ = self.scopes.take_current(name);
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::MovedResource { kind: "file owner" },
                        );
                    }
                    (
                        Some(Expr::Ident {
                            name,
                            span: source_span,
                        }),
                        Some(RuntimeFunctionReturnLayout::OwnedStruct {
                            binding: destination,
                        }),
                    ) => {
                        let Some(source) = self.scopes.get_current(name).cloned() else {
                            self.reject_moved_resource(name, *source_span)?;
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "returning a resource-owning struct requires a current-scope owner",
                                *source_span,
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
                                "returning a resource-owning struct requires a matching owner binding",
                                *source_span,
                            )));
                        }
                        self.emit_owned_struct_descriptor_move(
                            &source,
                            &destination,
                            *source_span,
                        )?;
                        let _ = self.scopes.take_current(name);
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::MovedResource {
                                kind: "resource owner",
                            },
                        );
                    }
                    (
                        Some(Expr::Ident {
                            name,
                            span: source_span,
                        }),
                        Some(RuntimeFunctionReturnLayout::Struct { layout, fields }),
                    ) => {
                        let Some(source_fields) =
                            self.scopes.get(name).and_then(|binding| match binding {
                                RuntimeGenericBinding::StructSlots { fields, .. } => {
                                    Some(fields.clone())
                                }
                                _ => None,
                            })
                        else {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "returning a runtime-native struct requires a scalar-field struct binding",
                                *source_span,
                            )));
                        };
                        for field in &layout.fields {
                            let Some((source_slot, source_ty)) = source_fields.get(&field.name)
                            else {
                                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                    "returned struct layout does not match function return type",
                                    *source_span,
                                )));
                            };
                            let Some((result_slot, result_ty)) = fields.get(&field.name) else {
                                return Err(RuntimeGenericLowerError::Unsupported);
                            };
                            if source_ty != result_ty || *source_ty != field.ty {
                                return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                    "returned struct field type does not match function return type",
                                    *source_span,
                                )));
                            }
                            self.emit(RuntimeInstr::Mov {
                                dst: *result_slot,
                                src: RuntimeOperand::Slot(*source_slot),
                            });
                            self.normalize_scalar_slot(*result_slot, *result_ty);
                        }
                    }
                    (
                        Some(Expr::Ident {
                            name,
                            span: source_span,
                        }),
                        Some(RuntimeFunctionReturnLayout::OwnedMap {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            layout,
                        }),
                    ) => {
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
                            self.reject_moved_resource(name, *source_span)?;
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "returning an owned map requires a current-scope owner",
                                *source_span,
                            )));
                        };
                        if source_layout != layout {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "returned map layout does not match function return type",
                                *source_span,
                            )));
                        }
                        for (dst, src) in [
                            (ptr_slot, source_ptr),
                            (len_slot, source_len),
                            (capacity_slot, source_capacity),
                            (allocation_bytes_slot, source_bytes),
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
                    (
                        Some(Expr::Ident {
                            name,
                            span: source_span,
                        }),
                        Some(RuntimeFunctionReturnLayout::OwnedString {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                        }),
                    ) => {
                        let Some((source_ptr, source_len, source_capacity, source_bytes, _)) = self
                            .scopes
                            .get_current(name)
                            .and_then(RuntimeGenericBinding::as_owned_string)
                        else {
                            self.reject_moved_resource(name, *source_span)?;
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "returning owned text requires a matching current-scope owner",
                                *source_span,
                            )));
                        };
                        for (dst, src) in [
                            (ptr_slot, source_ptr),
                            (len_slot, source_len),
                            (capacity_slot, source_capacity),
                            (allocation_bytes_slot, source_bytes),
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
                    (
                        Some(Expr::Ident {
                            name,
                            span: source_span,
                        }),
                        Some(RuntimeFunctionReturnLayout::OwnedListScalar {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            elem_ty,
                        }),
                    ) => {
                        let Some((
                            source_ptr,
                            source_len,
                            source_capacity,
                            source_bytes,
                            _,
                            source_ty,
                        )) = self
                            .scopes
                            .get_current(name)
                            .and_then(RuntimeGenericBinding::as_owned_list_scalar)
                        else {
                            self.reject_moved_resource(name, *source_span)?;
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "returning an owned list requires a current-scope list owner",
                                *source_span,
                            )));
                        };
                        if source_ty != elem_ty {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "returned list type does not match function return type",
                                *source_span,
                            )));
                        }
                        for (dst, src) in [
                            (ptr_slot, source_ptr),
                            (len_slot, source_len),
                            (capacity_slot, source_capacity),
                            (allocation_bytes_slot, source_bytes),
                        ] {
                            self.emit(RuntimeInstr::Mov {
                                dst,
                                src: RuntimeOperand::Slot(src),
                            });
                        }
                        let _ = self.scopes.take_current(name);
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::MovedResource { kind: "list owner" },
                        );
                    }
                    (
                        Some(Expr::Ident {
                            name,
                            span: source_span,
                        }),
                        Some(RuntimeFunctionReturnLayout::OwnedListStruct {
                            ptr_slot,
                            len_slot,
                            capacity_slot,
                            allocation_bytes_slot,
                            layout,
                        }),
                    ) => {
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
                            self.reject_moved_resource(name, *source_span)?;
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "returning an owned struct-list requires a current-scope list owner",
                                *source_span,
                            )));
                        };
                        if source_layout != layout {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "returned struct-list type does not match function return type",
                                *source_span,
                            )));
                        }
                        for (dst, src) in [
                            (ptr_slot, source_ptr),
                            (len_slot, source_len),
                            (capacity_slot, source_capacity),
                            (allocation_bytes_slot, source_bytes),
                        ] {
                            self.emit(RuntimeInstr::Mov {
                                dst,
                                src: RuntimeOperand::Slot(src),
                            });
                        }
                        let _ = self.scopes.take_current(name);
                        self.scopes.insert(
                            name.clone(),
                            RuntimeGenericBinding::MovedResource { kind: "list owner" },
                        );
                    }
                    (
                        Some(expr),
                        Some(RuntimeFunctionReturnLayout::Enum {
                            layout,
                            tag_slot,
                            payload_slots,
                        }),
                    ) => {
                        let source_name = if let Expr::Ident {
                            name,
                            span: source_span,
                        } = expr
                        {
                            Some((name.clone(), *source_span))
                        } else {
                            None
                        };
                        let source = if let Some((name, source_span)) = &source_name {
                            let source = if layout.owns_resources() {
                                self.scopes.get_current(name).cloned()
                            } else {
                                self.scopes.get(name).cloned()
                            };
                            if source.is_none() {
                                self.reject_moved_resource(name, *source_span)?;
                            }
                            source
                        } else {
                            let ty = if layout.type_args.is_empty() {
                                TypeName::Struct(layout.enum_name.clone())
                            } else {
                                TypeName::Applied {
                                    name: layout.enum_name.clone(),
                                    args: layout.type_args.clone(),
                                }
                            };
                            self.lower_runtime_enum_constructor(&ty, expr, false, *span)?
                        };
                        let Some(RuntimeGenericBinding::EnumSlots {
                            layout: source_layout,
                            tag_slot: source_tag,
                            payload_slots: source_payload,
                            owns_cleanup: source_owns_cleanup,
                            ..
                        }) = source
                        else {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "returning a runtime-native enum requires a matching enum value",
                                *span,
                            )));
                        };
                        if source_layout != layout || source_payload.len() != payload_slots.len() {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "returned enum layout does not match function return type",
                                *span,
                            )));
                        }
                        if layout.owns_resources() && !source_owns_cleanup {
                            return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                                "returning a resource enum requires an owning source",
                                *span,
                            )));
                        }
                        self.emit(RuntimeInstr::Mov {
                            dst: tag_slot,
                            src: RuntimeOperand::Slot(source_tag),
                        });
                        for (dst, src) in payload_slots.into_iter().zip(source_payload) {
                            self.emit(RuntimeInstr::Mov {
                                dst,
                                src: RuntimeOperand::Slot(src),
                            });
                        }
                        if layout.owns_resources()
                            && let Some((source_name, _)) = source_name
                        {
                            let _ = self.scopes.take_current(&source_name);
                            self.scopes.insert(
                                source_name,
                                RuntimeGenericBinding::MovedResource {
                                    kind: "resource enum owner",
                                },
                            );
                        }
                    }
                    (
                        Some(_),
                        Some(
                            RuntimeFunctionReturnLayout::Struct { .. }
                            | RuntimeFunctionReturnLayout::OwnedFile { .. }
                            | RuntimeFunctionReturnLayout::OwnedStruct { .. }
                            | RuntimeFunctionReturnLayout::OwnedListScalar { .. }
                            | RuntimeFunctionReturnLayout::OwnedListStruct { .. }
                            | RuntimeFunctionReturnLayout::OwnedString { .. }
                            | RuntimeFunctionReturnLayout::OwnedMap { .. },
                        ),
                    ) => {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "returning a runtime-native aggregate requires a named current-scope binding",
                            *span,
                        )));
                    }
                    (Some(_), None) => {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "cannot return value from function without return type",
                            *span,
                        )));
                    }
                    (None, Some(_)) => {
                        return Err(RuntimeGenericLowerError::Diagnostic(type_error(
                            "return value is required for this function",
                            *span,
                        )));
                    }
                    (None, None) => {}
                }
                self.emit_owned_cleanup_all();
                self.emit(RuntimeInstr::Return);
                Ok(())
            }
            Stmt::Break { span } => self.emit_break_jump(*span),
            Stmt::Continue { span } => self.emit_continue_jump(*span),
            Stmt::BenchLoop { .. } => Err(RuntimeGenericLowerError::Unsupported),
        }
    }
}
